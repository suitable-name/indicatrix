//! Typed concave drafts, shared parsing and atomic save/delete edits.

use indicatrix_cut_core::Edit;
use indicatrix_editor::{
    loading::{ConcaveTierFormFields, concave_tier_form_fields, parse_concave_tier_form},
    tier_save::{concave_name_clash_message, concave_tier_save_edit},
};

use crate::{app_state::AppState, design_state::DirtyScope};

/// Separate concave-list addressing; this index is never a flat tier index.
pub struct ConcaveDraft {
    document: u64,
    generation: u64,
    pub index: Option<usize>,
    pub fields: ConcaveTierFormFields,
}

impl ConcaveDraft {
    /// Opens a draft on an existing concave tier, or a blank one when `index` is `None`.
    ///
    /// A blank draft has no angle on purpose: the facet angle has no sensible default
    /// (0 is refused by the rule `0 < |angle| < 90`), so Save asks for it.
    ///
    /// # Errors
    ///
    /// A message when `index` no longer names a concave tier.
    pub fn capture(state: &AppState, index: Option<usize>) -> Result<Self, String> {
        let fields = match index {
            Some(index) => concave_tier_form_fields(
                state
                    .session
                    .design
                    .concave_tiers
                    .get(index)
                    .ok_or_else(|| "This concave tier no longer exists.".to_owned())?,
            ),
            None => ConcaveTierFormFields {
                tool: "CYL".into(),
                reciprocating: true,
                ..Default::default()
            },
        };
        Ok(Self {
            document: state.operations.document_id(),
            generation: state.session.current_generation(),
            index,
            fields,
        })
    }

    fn check_current(&self, state: &AppState) -> Result<(), String> {
        if self.document != state.operations.document_id()
            || self.generation != state.session.current_generation()
        {
            return Err(
                "The design changed while this draft was open. Cancel and reopen the form.".into(),
            );
        }
        Ok(())
    }

    /// Parses at full precision and records one edit; failure leaves the live design untouched.
    ///
    /// # Errors
    ///
    /// A message prefixed with the field it is about when the form does not parse or
    /// validate, a name clash, or a stale draft; the edit engine's message when it
    /// refuses the edit.
    pub fn save(&self, state: &mut AppState) -> Result<(), String> {
        self.check_current(state)?;
        let tier = parse_concave_tier_form(&self.fields, state.session.design.meta.gear_teeth)?;
        if let Some(message) =
            concave_name_clash_message(&state.session.design, self.index, &tier.name)
        {
            return Err(format!("name: {message}"));
        }
        let edit = concave_tier_save_edit(&state.session.design, self.index, tier);
        state
            .session
            .apply(edit)
            .map_err(|error| error.to_string())?;
        state.session.selected_concave = Some(
            self.index
                .unwrap_or(state.session.design.concave_tiers.len() - 1),
        );
        state.selected_tier = None;
        self.invalidate(state);
        Ok(())
    }

    /// Caller confirms destructive removal before calling this method.
    ///
    /// # Errors
    ///
    /// A message when the draft is stale, has not been added yet, or the edit is refused.
    pub fn remove(&self, state: &mut AppState) -> Result<(), String> {
        self.check_current(state)?;
        let index = self
            .index
            .ok_or_else(|| "This draft has not been added yet.".to_owned())?;
        state
            .session
            .apply(Edit::RemoveConcaveTier { index })
            .map_err(|error| error.to_string())?;
        self.invalidate(state);
        Ok(())
    }

    fn invalidate(&self, state: &mut AppState) {
        state.operations.invalidate();
        state.solve.note_edit(
            self.generation,
            state.session.current_generation(),
            DirtyScope::All,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_mock::MockPlatform;
    use std::sync::Arc;

    #[test]
    fn stale_draft_cannot_cross_document_replacement() {
        let mut state = AppState::new(Arc::new(MockPlatform::default()));
        let draft = ConcaveDraft::capture(&state, None).unwrap();
        state.operations.replace_document();
        assert!(
            draft
                .save(&mut state)
                .unwrap_err()
                .contains("design changed")
        );
        assert_eq!(
            state.session.design.concave_tiers,
            [] as [indicatrix_cut_core::design::ConcaveTier; 0]
        );
    }

    #[test]
    fn each_supported_tool_and_motion_saves_as_one_undoable_edit() {
        for (tool, angle) in [
            ("CYL", ""),
            ("CON", "60"),
            ("CIR", ""),
            ("DSC", "90"),
            ("SPH", ""),
        ] {
            for reciprocating in [false, true] {
                let mut state = AppState::new(Arc::new(MockPlatform::default()));
                let mut draft = ConcaveDraft::capture(&state, None).unwrap();
                draft.fields = ConcaveTierFormFields {
                    name: "Tool".into(),
                    // A pavilion-side angle: the rule is 0 < |angle| < 90, so a zero angle
                    // (signed or not) is refused. The sign is what the assertion below reads.
                    angle_deg: "-42".into(),
                    indices: "0 12".into(),
                    tool: tool.into(),
                    tool_angle_deg: angle.into(),
                    diameter_ratio: "0.2".into(),
                    reciprocating,
                    ..Default::default()
                };
                draft.save(&mut state).unwrap();
                assert_eq!(state.session.design.concave_tiers.len(), 1);
                assert!(
                    state.session.design.concave_tiers[0]
                        .angle_deg
                        .is_sign_negative()
                );
                state.session.undo().unwrap();
                assert_eq!(
                    state.session.design.concave_tiers,
                    [] as [indicatrix_cut_core::design::ConcaveTier; 0]
                );
                state.session.redo().unwrap();
                assert_eq!(state.session.design.concave_tiers[0].tool.code(), tool);
            }
        }
    }
}
