//! Adding tiers: the "+ Tier" button and Quick Add's three presets.
//!
//! A new tier goes right after the selected one (else at the end) and becomes the
//! selection. The form is parsed with `indicatrix_editor::loading::parse_tier_form`, the
//! code behind the desktop's Tier tab, so angle limits, the index shorthand and the
//! name-collision check behave identically.

use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_editor::{
    EditorSession,
    loading::{TierFormFields, next_free_block_name, non_integral_index_warning, parse_tier_form},
    session::tier_nudge_label,
    tier_save::{other_tier_names_excluding, tier_save_edit},
    view_model::row_format::first_unresolved_meet_name,
};

use super::{DirtyScope, EditOutcome, Edited, apply_edit, failed, tracked};
use crate::app_state::AppState;

/// The three starting-anchor presets of Quick Add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickAdd {
    /// A table: angle 0, anchored at 0.32.
    Table,
    /// A girdle: angle 90, anchored at 1.
    Girdle,
    /// A culet: angle -0, anchored at 0.88.
    Culet,
}

impl QuickAdd {
    /// The preset a Slint index names (0 Table, 1 Girdle, 2 Culet).
    #[must_use]
    pub const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Table),
            1 => Some(Self::Girdle),
            2 => Some(Self::Culet),
            _ => None,
        }
    }

    /// `(angle, anchor, name)` as the tier form's texts.
    const fn texts(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Table => ("0", "0.32", "Table"),
            Self::Girdle => ("90", "1", "Girdle"),
            Self::Culet => ("-0", "0.88", "Culet"),
        }
    }
}

/// The tier form's fields as typed.
struct TierForm<'a> {
    angle: &'a str,
    /// 0 meet existing, 1 meet named, 2 scale reference.
    kind: i32,
    meets: &'a str,
    name: &'a str,
    indices: &'a str,
}

/// What a successful add reports.
struct Added {
    index: usize,
    label: String,
    warning: Option<String>,
}

/// Parses `form` and adds the tier after `selected` (or at the end).
fn add_from_form(
    session: &mut EditorSession,
    selected: Option<usize>,
    form: &TierForm<'_>,
) -> Result<Added, String> {
    let other_names = other_tier_names_excluding(&session.design, -1);
    let mut tier = parse_tier_form(TierFormFields {
        angle: form.angle,
        constraint_kind: form.kind,
        constraint_text: form.meets,
        name: form.name,
        indices: form.indices,
        gear_teeth_abs: session.design.meta.gear_teeth_abs(),
        imported_meet: None,
        original_notes: None,
        other_tier_names: other_names.clone(),
    })?;
    // An unnamed tier can never be a meet target: name it like the desktop's Save Tier.
    if tier.name.is_empty() {
        tier.name = next_free_block_name(tier.angle_deg, &other_names);
    }
    if let MeetConstraint::MeetNamed(names) = &tier.constraint
        && let Some(bad_name) = first_unresolved_meet_name(&session.design, names)
    {
        return Err(format!(
            "No facet named '{bad_name}'. Check the Meets field."
        ));
    }
    let warning = non_integral_index_warning(&tier.indices);
    if indicatrix_cut_core::design::labelling::name_indicates_pavilion(&tier.name) {
        tier.angle_deg = -tier.angle_deg.abs();
    }
    let (index, edit) = tier_save_edit(&session.design, -1, tier, selected);
    apply_edit(session, edit)?;
    let label = session
        .design
        .tiers
        .get(index)
        .map_or_else(String::new, |tier| tier_nudge_label(tier, index));
    Ok(Added {
        index,
        label,
        warning,
    })
}

/// The index positions of a full orbit on the design's gear, as form text.
fn default_orbit_indices(state: &AppState) -> String {
    let meta = &state.session.design.meta;
    let gear = meta.gear_teeth_abs();
    let order = meta.symmetry_order.max(1);
    if gear == 0 || !gear.is_multiple_of(order) {
        return "0".to_owned();
    }
    let step = gear / order;
    (0..order)
        .map(|fold| (fold * step).to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Selects the added tier and words the outcome.
fn finish_add(state: &mut AppState, result: Result<Added, String>) -> (EditOutcome, DirtyScope) {
    match result {
        Ok(added) => {
            state.selected_tier = Some(added.index);
            state.session.multi_selected.clear();
            let outcome =
                Ok(Edited::applied_with(format!("Added {}.", added.label))
                    .with_extra(added.warning));
            (outcome, DirtyScope::one(added.index))
        }
        Err(message) => (Err(failed(message)), DirtyScope::All),
    }
}

/// Adds a crown tier at 40 degrees with a full orbit of indices right after the selected
/// tier (else at the end), and selects it. The cutter then sets the angle on the sheet.
///
/// # Errors
/// The form's parse error or the apply error, as text.
pub fn add_tier(state: &mut AppState) -> EditOutcome {
    let indices = default_orbit_indices(state);
    let selected = state.selected_tier;
    tracked(state, |state| {
        let result = add_from_form(
            &mut state.session,
            selected,
            &TierForm {
                angle: "40",
                kind: 0,
                meets: "",
                name: "",
                indices: &indices,
            },
        );
        finish_add(state, result)
    })
}

/// Adds one of the Quick Add presets (each pinned to an exact scale value) and selects it.
///
/// # Errors
/// The form's parse error (a name already taken, for example) or the apply error.
pub fn quick_add(state: &mut AppState, preset: QuickAdd) -> EditOutcome {
    let (angle, meets, name) = preset.texts();
    let selected = state.selected_tier;
    tracked(state, |state| {
        let result = add_from_form(
            &mut state.session,
            selected,
            &TierForm {
                angle,
                kind: 2,
                meets,
                name,
                indices: "",
            },
        );
        finish_add(state, result)
    })
}
