//! The RI-preservation rule shared by the catalogue-load material suggestion and a plain
//! in-editor material pick.
//!
//! Applying a built-in material must never silently rewrite a design's already-exported
//! refractive index.

use indicatrix_cut_core::MaterialSelection;

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
/// `current`'s `specific_gravity_override` and `body_colour_override` are
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
        body_colour_override: current.body_colour_override,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
