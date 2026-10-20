//! The RI-preservation rule shared by the catalogue-load material suggestion and a plain
//! in-editor material pick.
//!
//! Applying a built-in material must never silently rewrite a design's already-exported
//! refractive index.

use crate::material_lookup::{MATERIAL_MATCH_TOLERANCE, nearest_built_in_material};
use indicatrix_cut_core::{Design, MaterialSelection, material::MaterialLookup};

/// How far a built-in material's own `n_D` may drift from the RI a design's exported
/// `.asc` currently reports before [`ri_override_to_preserve`] steps in to hold the
/// export steady.
///
/// See that function's own doc comment.
pub const RI_PRESERVE_TOLERANCE: f64 = 0.01;

/// `Design::effective_refractive_index` derives the EXPORTED RI from the selected
/// material's own built-in `n_D` whenever no override is set -- see that method's
/// own doc comment.
///
/// That is exactly right for a design that never had a
/// recorded RI of its own, but wrong for one that did: picking "Diamond" (`n_D`
/// 1.5442) for a design whose exported schedule currently reads `I 1.54` would
/// otherwise silently rewrite that line the moment a material is applied, even
/// though nothing about the facet geometry or the cutter's typed figure changed.
///
/// Returns `Some(original_ri)` -- to be pinned into
/// [`MaterialSelection::refractive_index_override`] -- when `name` resolves to a
/// built-in whose own `n_D` differs from `original_ri` by more than
/// [`RI_PRESERVE_TOLERANCE`], so the export keeps reading `original_ri` exactly
/// as before. Returns `None` (no override needed) when `name` is not a built-in
/// at all, or when the two already agree closely enough that pinning would be
/// pure noise.
///
/// Shared by [`material_selection_for_accepted_suggestion`] (the catalogue-load
/// suggestion, which already applied this reasoning under a different name) and
/// `callbacks::tier_actions::setup_apply_design_material_callback` (for a plain
/// in-editor material pick with no typed RI override of its own).
#[must_use]
pub fn ri_override_to_preserve(name: &str, original_ri: f64) -> Option<f64> {
    let built_in_ri = indicatrix_cut_core::built_in_refractive_index(name)?;
    ((built_in_ri - original_ri).abs() > RI_PRESERVE_TOLERANCE).then_some(original_ri)
}

/// Whether a plain material pick should pin the design's legacy exported RI.
///
/// `picked_name` is the pick, with no typed RI override of its own. The RI is pinned into
/// [`MaterialSelection::refractive_index_override`] to `legacy_ri` (i.e.
/// `meta.refractive_index` -- the schedule's own `I` line) so the pick does not silently
/// rewrite it. Returns `None` (pick
/// stays unpinned, the newly resolved material's own `n_D` wins) whenever
/// `previous_material_name` is `Some`: once a design already has a NAMED
/// material, that material's own `n_D` (or an explicit typed override) is the
/// design's RI story from then on, and pinning the OUTGOING material's RI onto
/// the incoming one would be exactly the bug this guard exists to prevent --
/// see [`ri_override_to_preserve`] for the tolerance check itself.
///
/// Moved from the desktop's `tier_actions::materials_symmetry` so the web design
/// settings apply a material pick identically.
#[must_use]
pub fn ri_override_for_material_pick(
    picked_name: Option<&str>,
    previous_material_name: Option<&str>,
    legacy_ri: f64,
) -> Option<f64> {
    if previous_material_name.is_some() {
        return None;
    }
    ri_override_to_preserve(picked_name?, legacy_ri)
}

/// What accepting the catalogue-load material suggestion applies.
///
/// See `super::callbacks::setup_load_selected_callback`'s own doc comment for when the
/// suggestion (`super::material_lookup::nearest_built_in_material`) is offered in the
/// first place.
///
/// `current`'s `specific_gravity_override` and `body_color_override` are
/// carried through unchanged -- this only ever touches
/// `name`/`refractive_index_override`; see [`ri_override_to_preserve`] for why
/// the override is pinned at all.
#[must_use]
pub fn material_selection_for_accepted_suggestion(
    name: &str,
    schedule_ri: f64,
    current: &MaterialSelection,
) -> MaterialSelection {
    MaterialSelection {
        name: Some(name.to_string()),
        specific_gravity_override: current.specific_gravity_override,
        refractive_index_override: ri_override_to_preserve(name, schedule_ri),
        body_color_override: current.body_color_override,
        body_color_bands_override: current.body_color_bands_override.clone(),
        absorption_path_scale_override: current.absorption_path_scale_override,
    }
}

/// Gives a design opened from the library the material the library shows for its entry,
/// so the Edit tab does not merely display it: the design carries it.
///
/// Applied while the design is built, before any session or history wraps it, so it is
/// neither an undo step nor an unsaved change (a loaded design is clean by definition).
///
/// The precedence is the library detail view's own (`apply_catalogue_material` on the
/// desktop) and the batches' (`gui::batch::catalogue_material`):
///
/// 1. the design's own material -- a design file that names one keeps it, untouched
///    (the file wins over the library entry; this function returns `None` for it, also
///    when the name resolves nowhere, since the editor refuses such a design on purpose);
/// 2. `preview_material`, the entry's persisted material, when `lookup` resolves it;
/// 3. the built-in the design's refractive index names
///    ([`nearest_built_in_material`] within [`MATERIAL_MATCH_TOLERANCE`]), the same
///    inference `traced_material_for` already renders the nameless design with.
///
/// The name goes through [`material_selection_for_accepted_suggestion`], so the schedule's
/// `I` line is pinned as a refractive-index override when the material's own `n_D` drifts
/// from it: applying the material never rewrites an exported refractive index.
///
/// Returns the applied name, or `None` when the design was left alone (it names a
/// material, or neither the entry nor the index names a material that resolves).
pub fn apply_library_material(
    design: &mut Design,
    preview_material: Option<&str>,
    lookup: &dyn MaterialLookup,
) -> Option<String> {
    if design
        .material
        .name
        .as_deref()
        .is_some_and(|name| !name.trim().is_empty())
    {
        return None;
    }
    let schedule_ri = design.meta.refractive_index;
    let name = preview_material
        .map(str::trim)
        .filter(|name| !name.is_empty() && lookup.lookup(name).is_some())
        .map(str::to_owned)
        .or_else(|| {
            nearest_built_in_material(schedule_ri, MATERIAL_MATCH_TOLERANCE).map(|(name, _)| name)
        })?;
    design.material =
        material_selection_for_accepted_suggestion(&name, schedule_ri, &design.material);
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::material::BuiltinMaterials;
    use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};

    /// A nameless design (as an `.asc` import is) whose schedule reads `ri`.
    fn nameless_design(ri: f64) -> Design {
        let mut design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        design.meta.refractive_index = ri;
        design.material = MaterialSelection::none();
        design
    }

    // --- apply_library_material ---

    #[test]
    fn a_library_entry_material_becomes_the_designs_material() {
        let mut design = nameless_design(1.76);
        let applied = apply_library_material(&mut design, Some("Ruby"), &BuiltinMaterials);
        assert_eq!(applied.as_deref(), Some("Ruby"));
        assert_eq!(design.material.name.as_deref(), Some("Ruby"));
    }

    #[test]
    fn the_refractive_index_names_the_material_when_the_entry_names_none() {
        let sapphire = indicatrix_cut_core::built_in_refractive_index("Sapphire").unwrap();
        let mut design = nameless_design(sapphire);
        let applied = apply_library_material(&mut design, None, &BuiltinMaterials);
        assert_eq!(applied.as_deref(), Some("Sapphire"));
        assert_eq!(design.material.name.as_deref(), Some("Sapphire"));
        assert_eq!(design.material.refractive_index_override, None);
    }

    #[test]
    fn an_unresolvable_entry_material_falls_back_to_the_refractive_index() {
        let quartz = indicatrix_cut_core::built_in_refractive_index("Quartz").unwrap();
        let mut design = nameless_design(quartz);
        let applied = apply_library_material(&mut design, Some("No Such Stone"), &BuiltinMaterials);
        assert_eq!(applied.as_deref(), Some("Quartz"));
    }

    #[test]
    fn the_design_files_own_material_wins_over_the_library_entry() {
        let mut design = nameless_design(1.76);
        design.material.name = Some("Spinel".to_owned());
        let before = design.material.clone();
        assert_eq!(
            apply_library_material(&mut design, Some("Ruby"), &BuiltinMaterials),
            None
        );
        assert!(design.material == before, "the file's selection is kept");
    }

    /// The material is part of the design the session starts from: no undo step, and the
    /// freshly loaded session reads clean (the load convention).
    #[test]
    fn the_applied_material_is_no_undo_step_and_not_an_unsaved_change() {
        use indicatrix_cut_core::History;
        let mut design = nameless_design(1.76);
        apply_library_material(&mut design, Some("Ruby"), &BuiltinMaterials);
        let session = crate::EditorSession::with_history(design, History::new());
        assert!(!session.is_dirty());
        assert!(!session.history.can_undo());
        assert_eq!(session.design.material.name.as_deref(), Some("Ruby"));
    }

    #[test]
    fn a_design_with_no_resolvable_material_is_left_alone() {
        let mut design = nameless_design(1.05);
        assert_eq!(
            apply_library_material(&mut design, None, &BuiltinMaterials),
            None
        );
        assert_eq!(design.material.name, None);
    }

    #[test]
    fn the_schedule_ri_is_pinned_when_the_entry_material_drifts_from_it() {
        let mut design = nameless_design(1.54);
        apply_library_material(&mut design, Some("Diamond"), &BuiltinMaterials);
        assert_eq!(design.material.refractive_index_override, Some(1.54));
    }

    // --- material_selection_for_accepted_suggestion ---

    #[test]
    fn accepted_suggestion_sets_no_override_when_the_built_in_ri_is_within_tolerance() {
        let quartz_ri = indicatrix_cut_core::built_in_refractive_index("Quartz").unwrap();
        let current = MaterialSelection::none();
        let selection = material_selection_for_accepted_suggestion("Quartz", quartz_ri, &current);
        assert_eq!(selection.name.as_deref(), Some("Quartz"));
        assert_eq!(selection.refractive_index_override, None);
    }

    #[test]
    fn accepted_suggestion_pins_the_schedule_ri_when_the_built_in_ri_differs_by_more_than_the_tolerance()
     {
        let quartz_ri = indicatrix_cut_core::built_in_refractive_index("Quartz").unwrap();
        // A schedule RI 0.02 away from Quartz's own real n_D -- still within the
        // suggestion's own 0.01 SEARCH tolerance is not guaranteed here (this
        // test picks the schedule RI directly, not via `nearest_built_in_material`),
        // but exercises exactly the "accepting would otherwise silently move the
        // exported RI" case this function exists to prevent.
        let schedule_ri = quartz_ri + 0.02;
        let current = MaterialSelection::none();
        let selection = material_selection_for_accepted_suggestion("Quartz", schedule_ri, &current);
        assert_eq!(selection.refractive_index_override, Some(schedule_ri));
    }

    #[test]
    fn accepted_suggestion_keeps_the_current_specific_gravity_override() {
        let mut current = MaterialSelection::none();
        current.specific_gravity_override = Some(3.9);
        let selection = material_selection_for_accepted_suggestion("Diamond", 2.417, &current);
        assert_eq!(selection.specific_gravity_override, Some(3.9));
    }

    // --- ri_override_to_preserve ---

    #[test]
    fn ri_override_to_preserve_pins_the_original_ri_when_the_built_in_drifts() {
        let diamond_ri = indicatrix_cut_core::built_in_refractive_index("Diamond").unwrap();
        // The design's exported schedule currently reads "I 1.54" -- far enough
        // from Diamond's real n_D (~1.5442) that applying Diamond outright would
        // silently rewrite it.
        let original_ri = 1.54;
        assert!((diamond_ri - original_ri).abs() > RI_PRESERVE_TOLERANCE);
        assert_eq!(
            ri_override_to_preserve("Diamond", original_ri),
            Some(original_ri)
        );
    }

    #[test]
    fn ri_override_to_preserve_does_nothing_when_already_close_enough() {
        let diamond_ri = indicatrix_cut_core::built_in_refractive_index("Diamond").unwrap();
        assert_eq!(ri_override_to_preserve("Diamond", diamond_ri), None);
    }

    #[test]
    fn ri_override_to_preserve_does_nothing_for_a_non_built_in_name() {
        assert_eq!(ri_override_to_preserve("Not A Real Material", 1.54), None);
    }
}
