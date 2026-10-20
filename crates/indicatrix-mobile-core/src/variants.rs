//! Immutable variant captures and stale-safe atomic opening.
use crate::recipe_steps::Recipe;
use crate::{app_state::AppState, design_state::DirtyScope};
use indicatrix_cut_core::{
    Design,
    native::{DesignExtras, LoadedDesign, design_to_string, gem_material_from_custom_snapshot},
};
use indicatrix_editor::variants::{open_label, replacement_edit};
use indicatrix_web_core::custom_material::builtin_name_conflict;

/// Installed-document stamp captured before any asynchronous vault access.
#[derive(Clone, Copy)]
pub struct VariantStamp {
    document: u64,
    generation: u64,
}

impl VariantStamp {
    #[must_use]
    pub fn capture(state: &AppState) -> Self {
        Self {
            document: state.operations.document_id(),
            generation: state.session.current_generation(),
        }
    }

    #[must_use]
    pub fn is_current(self, state: &AppState) -> bool {
        self.document == state.operations.document_id()
            && self.generation == state.session.current_generation()
    }
}

pub struct VariantCapture {
    pub uuid: String,
    pub text: String,
    pub stamp: VariantStamp,
}

impl VariantCapture {
    /// Capture the chosen design now (current, historical, or validated candidate),
    /// before a worker can observe any subsequent live edits.
    ///
    /// # Errors
    ///
    /// A message when the design uses a custom material whose recipe is unavailable, or
    /// when the design cannot be encoded.
    pub fn capture(state: &mut AppState, design: &Design) -> Result<Self, String> {
        crate::document::ensure_document_id(state);
        let custom = match design.material.name.as_deref() {
            Some(name) if indicatrix_web_core::custom_material::builtin_name_conflict(name).is_none() => {
                Some(state.props.snapshot_for(name).cloned().ok_or_else(|| {
                    format!("The custom material {name:?} is unavailable. Restore its recipe before saving this variant.")
                })?)
            }
            _ => None,
        };
        let text = design_to_string(
            design,
            state.document.printed_proportions.as_ref(),
            &DesignExtras {
                custom_material: custom.as_ref(),
                metadata: Some(&state.document.metadata),
                ..Default::default()
            },
        )
        .map_err(|error| error.to_string())?;
        Ok(Self {
            uuid: state.document.metadata.id.clone(),
            text,
            stamp: VariantStamp::capture(state),
        })
    }
}

/// The catalogue entry a variant file would install, with the material's name: `None` when
/// the variant needs no change (a built-in material, no recipe in the file, or the same
/// recipe the catalogue already has).
fn recipe_change(state: &AppState, loaded: &LoadedDesign) -> Option<(String, Recipe)> {
    let (Some(name), Some(snapshot)) = (
        loaded.design.material.name.as_deref(),
        loaded.restorable_custom_material.as_ref(),
    ) else {
        return None;
    };
    // A built-in's name resolves on its own; the file's data is never registered under it.
    if builtin_name_conflict(name).is_some() {
        return None;
    }
    if state.props.snapshot_for(name) == Some(snapshot) {
        return None;
    }
    Some((
        name.to_owned(),
        Recipe {
            material: gem_material_from_custom_snapshot(name, snapshot),
            snapshot: Some(snapshot.clone()),
        },
    ))
}

/// Opens a loaded variant as one labeled edit.
///
/// Caller confirms replacing a dirty working copy before requesting the load.
/// The family UUID and redo semantics stay put. A custom-material recipe the variant
/// carries that differs from the catalogue's is installed as part of that same step (see
/// [`crate::recipe_steps`]).
///
/// # Errors
///
/// A message when the design changed while the variant loaded, the replacement cannot be
/// built or applied, or the variant carries another definition of a custom material but
/// the same cutting design (there would be no step to undo).
pub fn open(
    state: &mut AppState,
    stamp: VariantStamp,
    loaded: &LoadedDesign,
    name: &str,
) -> Result<bool, String> {
    if !stamp.is_current(state) {
        return Err("The design changed while loading this variant. Open it again.".into());
    }
    // A variant saved with another definition of a custom material than the catalogue
    // holds brings that definition with it. The catalogue is not part of a design, so the
    // change is recorded against the history step below (`recipe_steps`): Undo restores
    // the old recipe with the old design, Redo the variant's with the variant.
    let recipe = recipe_change(state, loaded);
    let Some(edit) = replacement_edit(&state.session.design, &loaded.design)? else {
        if recipe.is_some() {
            return Err("This variant has the same cutting design but another definition of its custom material, so opening it would change no step to undo. Save the material under a new name first.".into());
        }
        return Ok(false);
    };
    let before = state.session.current_generation();
    state
        .session
        .try_apply_mapped(edit, Some(&open_label(name)))
        .map_err(|error| error.to_string())?;
    if let Some((material, recipe)) = recipe
        && let Err(message) = crate::recipe_steps::record(state, &material, recipe)
    {
        // Take the step back so the design never names a recipe it does not have.
        let _ = state.session.undo();
        return Err(message);
    }
    state.selected_tier = None;
    state.session.selected_concave = None;
    state.session.multi_selected.clear();
    state
        .solve
        .note_edit(before, state.session.current_generation(), DirtyScope::All);
    state.operations.invalidate();
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        design_props::{CustomMaterialInput, add_custom_material, set_stone_width},
        platform_mock::MockPlatform,
    };
    use indicatrix_cut_core::{native::design_from_str, templates::TEMPLATES};
    use indicatrix_editor::EditorSession;
    use std::sync::Arc;

    /// A session on the first template whose design uses a custom material "Zorbium" of
    /// refractive index `ri`.
    fn state_with_recipe(ri: &str) -> AppState {
        let mut state = AppState::new(Arc::new(MockPlatform::default()));
        let template = &TEMPLATES[0];
        state.session = EditorSession::from_template(
            template.fresh_spec(template.default_material_selection()),
            1,
        );
        add_custom_material(
            &mut state,
            &CustomMaterialInput {
                name: "Zorbium".to_owned(),
                ri: ri.to_owned(),
                ..CustomMaterialInput::default()
            },
        )
        .expect("the material is accepted");
        state
    }

    /// The variant file of `source`'s design as a saved variant would hold it.
    fn variant_of(source: &mut AppState) -> LoadedDesign {
        let design = source.session.design.clone();
        let capture = VariantCapture::capture(source, &design).expect("captures");
        design_from_str(&capture.text).expect("the saved text loads")
    }

    fn zorbium_index(state: &AppState) -> f32 {
        state
            .props
            .find_custom("Zorbium")
            .expect("the material is in the catalogue")
            .dispersion
            .n_d()
    }

    #[test]
    fn a_variant_with_another_recipe_installs_it_as_part_of_its_undoable_step() {
        let mut working = state_with_recipe("1.70");
        let first = working
            .props
            .snapshot_for("Zorbium")
            .cloned()
            .expect("snapshot");
        let mut other = state_with_recipe("1.85");
        other.session.design.tiers[0].angle_deg += 2.0;
        let second = other
            .props
            .snapshot_for("Zorbium")
            .cloned()
            .expect("snapshot");
        assert_ne!(first, second);
        let loaded = variant_of(&mut other);
        let angle_before = working.session.design.tiers[0].angle_deg;

        let stamp = VariantStamp::capture(&working);
        assert_eq!(open(&mut working, stamp, &loaded, "Variant"), Ok(true));
        assert!((zorbium_index(&working) - 1.85).abs() < 1e-4);
        assert_eq!(working.props.snapshot_for("Zorbium"), Some(&second));
        assert_ne!(working.session.design.tiers[0].angle_deg, angle_before);

        // Undo takes the design AND its recipe back; Redo brings both again.
        let _ = crate::design_state::undo(&mut working);
        assert!((zorbium_index(&working) - 1.70).abs() < 1e-4);
        assert_eq!(working.props.snapshot_for("Zorbium"), Some(&first));
        assert_eq!(working.session.design.tiers[0].angle_deg, angle_before);
        let _ = crate::design_state::redo(&mut working);
        assert!((zorbium_index(&working) - 1.85).abs() < 1e-4);
        assert_eq!(working.props.snapshot_for("Zorbium"), Some(&second));

        // A jump back does the same through the history list.
        let back = working.session.history_position() - 1;
        crate::history::jump_to(&mut working, back).expect("the jump replays");
        assert_eq!(working.props.snapshot_for("Zorbium"), Some(&first));
        let forward = working.session.history_position() + 1;
        crate::history::jump_to(&mut working, forward).expect("the jump replays");
        assert_eq!(working.props.snapshot_for("Zorbium"), Some(&second));
    }

    #[test]
    fn a_new_edit_after_undo_drops_the_variants_recipe_with_its_redo_branch() {
        let mut working = state_with_recipe("1.70");
        let first = working
            .props
            .snapshot_for("Zorbium")
            .cloned()
            .expect("snapshot");
        let mut other = state_with_recipe("1.85");
        other.session.design.tiers[0].angle_deg += 2.0;
        let loaded = variant_of(&mut other);
        let stamp = VariantStamp::capture(&working);
        open(&mut working, stamp, &loaded, "Variant").expect("opens");
        let _ = crate::design_state::undo(&mut working);
        assert_eq!(working.props.recipe_steps.len(), 1);

        // Another edit ends the redo branch the variant lived on.
        assert_eq!(set_stone_width(&mut working, "6.5"), Ok(true));
        assert!(!crate::recipe_steps::sync(&mut working));
        assert_eq!(
            working.props.recipe_steps,
            [] as [crate::recipe_steps::RecipeStep; 0]
        );
        assert_eq!(working.props.snapshot_for("Zorbium"), Some(&first));
    }

    #[test]
    fn the_same_design_with_another_recipe_is_refused_not_half_applied() {
        let mut working = state_with_recipe("1.70");
        let first = working
            .props
            .snapshot_for("Zorbium")
            .cloned()
            .expect("snapshot");
        let mut other = state_with_recipe("1.85");
        let loaded = variant_of(&mut other);
        let stamp = VariantStamp::capture(&working);
        let error = open(&mut working, stamp, &loaded, "Variant").expect_err("nothing to undo");
        assert!(error.contains("another definition"), "{error}");
        assert_eq!(working.props.snapshot_for("Zorbium"), Some(&first));
        assert_eq!(
            working.props.recipe_steps,
            [] as [crate::recipe_steps::RecipeStep; 0]
        );
    }

    #[test]
    fn a_variant_with_the_catalogues_own_recipe_records_no_recipe_step() {
        let mut working = state_with_recipe("1.70");
        let mut other = state_with_recipe("1.70");
        other.session.design.tiers[0].angle_deg += 2.0;
        let loaded = variant_of(&mut other);
        let stamp = VariantStamp::capture(&working);
        assert_eq!(open(&mut working, stamp, &loaded, "Variant"), Ok(true));
        assert_eq!(
            working.props.recipe_steps,
            [] as [crate::recipe_steps::RecipeStep; 0]
        );
    }
}
