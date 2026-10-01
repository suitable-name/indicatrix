//! The row/format, material-combo, proportions and proposed-angle helper tests that
//! lived next to the desktop's `state` re-exports before the move.

use crate::{
    material::{
        MaterialComboCache, design_material_index_from_name, design_material_name_from_index,
        design_material_options,
    },
    view_model::{
        row_format::orbit_status_text,
        rows::{apply_proposed_angles, tier_items_stale, tier_items_stale_with_last_solved},
        solid_status::external_proportions_note,
        yield_report::{girdle_and_ratio_texts, preform_y_offset_mm_text},
    },
};
use indicatrix::{geometry::stone_metrics::SolidMetrics, optics::materials::GemMaterial};
use indicatrix_cut_core::{Design, OrbitUnit, PreformSpec};
use std::collections::BTreeSet;

fn unit(members: usize, expected_len: usize) -> OrbitUnit {
    OrbitUnit {
        members: (0..members).map(|i| i as f64).collect(),
        expected_len,
    }
}

#[test]
fn orbit_status_text_reports_a_clean_multi_unit_fold_as_complete() {
    let (text, incomplete) = orbit_status_text(&[unit(4, 4), unit(4, 4)]);
    assert_eq!(text, "2 orbits");
    assert!(!incomplete);
}

#[test]
fn orbit_status_text_counts_how_many_units_are_incomplete() {
    let (text, incomplete) = orbit_status_text(&[unit(4, 4), unit(2, 4), unit(4, 4)]);
    assert_eq!(text, "3 orbits (1 incomplete)");
    assert!(incomplete);
}

#[test]
fn orbit_status_text_reports_a_mixed_fold_with_no_complete_unit_as_not_symmetric() {
    // `orbit::mod`'s own corpus doc comment's `mixed_fold` example: several
    // units, but every one of them short a member -- genuine incoherence,
    // not a single benign partial occurrence.
    let (text, incomplete) = orbit_status_text(&[unit(2, 4), unit(4, 8)]);
    assert_eq!(text, "not symmetric");
    assert!(incomplete);
}

fn metrics(
    width_axis: f64,
    length_axis: f64,
    total_height: f64,
    crown_height: Option<f64>,
    pavilion_depth: Option<f64>,
) -> SolidMetrics {
    SolidMetrics {
        volume: 1.0,
        width_axis,
        length_axis,
        width_caliper: width_axis,
        length_caliper: length_axis,
        total_height,
        crown_height,
        pavilion_depth,
        girdle_thickness: None,
        vertex_count: 8,
    }
}

#[test]
fn external_proportions_note_reports_lw_hw_cw_pw_when_a_girdle_is_present() {
    let m = metrics(2.0, 2.0, 1.2, Some(0.4), Some(0.6));
    let note = external_proportions_note(&m, None);
    assert!(note.contains("L/W 1.000"));
    assert!(note.contains("H/W 0.600"));
    assert!(note.contains("C/W 0.200"));
    assert!(note.contains("P/W 0.300"));
}

#[test]
fn external_proportions_note_omits_cw_pw_without_a_live_girdle_facet() {
    let m = metrics(2.0, 2.0, 1.2, None, None);
    let note = external_proportions_note(&m, None);
    assert!(note.contains("L/W"));
    assert!(note.contains("H/W"));
    assert!(!note.contains("C/W"));
    assert!(!note.contains("P/W"));
}

#[test]
fn external_proportions_note_is_empty_for_a_zero_width_solid() {
    let m = metrics(0.0, 2.0, 1.2, None, None);
    assert_eq!(external_proportions_note(&m, None).len(), 0);
}

#[test]
fn external_proportions_note_appends_absolute_mm_when_a_scale_is_known() {
    // Once a girdle diameter anchors a real scale, the
    // banner should show absolute size alongside the dimensionless ratios --
    // GemCad/GCS always show both, and the ratios alone still leave "how big
    // is it really" unanswered.
    let m = metrics(2.0, 2.0, 1.2, None, None);
    let note = external_proportions_note(&m, Some(2.5));
    assert!(note.contains("5.00 x 3.00 mm"), "note was: {note}");
}

#[test]
fn external_proportions_note_omits_mm_clause_without_a_known_scale() {
    let m = metrics(2.0, 2.0, 1.2, None, None);
    let note = external_proportions_note(&m, None);
    assert!(!note.contains("mm"));
}

// --- girdle_and_ratio_texts ---

fn fixture_design() -> Design {
    Design::fresh(PreformSpec::cylinder(96, 1.5, 1.0, 1.5), 96, 8, 1.54)
}

#[test]
fn girdle_and_ratio_texts_dashes_out_a_design_with_no_tiers() {
    // A brand-new design (`Design::fresh`) has no tiers at all -- it still
    // SOLVES (an empty mast list is a valid, closed, zero-plane solve), so
    // only the explicit `tiers.is_empty()` guard, not a `Design::solve`
    // failure, is what stops `stone_proportions` from measuring the bare
    // preform block. Every one of the four figures must read "-", the same
    // fallback `proportions_texts` uses, never a stale zero.
    let design = fixture_design();
    assert_eq!(
        girdle_and_ratio_texts(&design),
        (
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
            "-".to_string()
        )
    );
}

// --- design_material_options / design_material_name_from_index (a custom
// material colliding with a built-in name is shown, not silently hidden
// from this combo) ---

#[test]
fn design_material_options_lists_a_builtin_colliding_custom_material_under_a_suffixed_label() {
    let mut custom = GemMaterial::diamond();
    custom.name = "Diamond".to_string();
    let options = design_material_options(std::slice::from_ref(&custom));
    assert!(
        options.iter().any(|o| o == "Diamond (custom)"),
        "options was: {options:?}"
    );
    // The built-in entry itself must still be present too -- this is an
    // addition, not a replacement.
    assert!(options.iter().any(|o| o == "Diamond"));
}

/// A vault with no custom materials must still yield
/// the built-in list on the FIRST call -- an empty incoming name list must
/// not be mistaken for "cache already built", which would hand back an
/// empty combo model.
#[test]
fn material_combo_options_with_no_custom_materials_lists_the_builtins_on_first_call() {
    let mut cache = MaterialComboCache::default();
    let options = cache.options(&[]);
    assert_eq!(options, design_material_options(&[]));
    assert_eq!(options.first().map(String::as_str), Some("(none)"));
    assert!(
        options.iter().any(|o| o == "Diamond"),
        "options was: {options:?}"
    );
    // Second call with the same (empty) custom list is served from the cache.
    assert_eq!(cache.options(&[]), options);
}

#[test]
fn design_material_options_lists_a_non_colliding_custom_material_plainly() {
    let mut custom = GemMaterial::diamond();
    custom.name = "My Garnet".to_string();
    let options = design_material_options(std::slice::from_ref(&custom));
    assert!(options.iter().any(|o| o == "My Garnet"));
    assert!(!options.iter().any(|o| o.contains("(custom)")));
}

#[test]
fn design_material_name_from_index_strips_the_collision_suffix() {
    let mut custom = GemMaterial::diamond();
    custom.name = "Diamond".to_string();
    let options = design_material_options(std::slice::from_ref(&custom));
    let index = options
        .iter()
        .position(|o| o == "Diamond (custom)")
        .expect("labeled entry must exist");
    assert_eq!(
        design_material_name_from_index(index as i32, &options),
        Some("Diamond".to_string()),
        "the parsed name must be the real material name, not the display label, \
         so it still resolves through EditorMaterialLookup's custom-over-built-in \
         precedence"
    );
}

#[test]
fn design_material_index_from_name_still_finds_the_plain_builtin_entry() {
    let mut custom = GemMaterial::diamond();
    custom.name = "Diamond".to_string();
    let options = design_material_options(std::slice::from_ref(&custom));
    // `builtin_preset_names`' own "Diamond" entry comes first in the list, so
    // a plain lookup by name must still resolve to it, not the labeled
    // duplicate further down.
    assert_eq!(
        design_material_index_from_name(Some("Diamond"), &options),
        1
    );
}

// --- tier_items_stale_with_last_solved ---

#[test]
fn tier_items_stale_with_last_solved_falls_back_to_dashes_with_no_cached_solve() {
    let design = fixture_design();
    let rows = tier_items_stale_with_last_solved(&design, 1.54, None, &BTreeSet::new());
    assert!(rows.is_empty(), "a fresh design has no tiers to show");
}

fn scale_reference_tier(name: &str) -> indicatrix_cut_core::ConstraintTier {
    indicatrix_cut_core::ConstraintTier {
        angle_deg: 40.0,
        name: name.to_string(),
        indices: Vec::new(),
        constraint: indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(1.0),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

fn solved_tier(mast: f64) -> indicatrix::geometry::meet_solver::SolvedTier {
    indicatrix::geometry::meet_solver::SolvedTier {
        mast,
        strategy: indicatrix::geometry::meet_solver::SolveStrategy::ScaleReference,
        detail: "given (scale reference)".to_string(),
    }
}

/// A tier the edit itself touched (in `dirty`) must fall back to `"-"`/"not
/// solved" -- no previous mast can be trusted for it. A tier the edit left
/// alone must keep its previous mast, tagged "stale" rather than shown as a
/// fresh solve.
#[test]
fn tier_items_stale_with_last_solved_blanks_only_the_dirty_rows() {
    let mut design = fixture_design();
    design.tiers = vec![scale_reference_tier("Table"), scale_reference_tier("P1")];
    let last_solved = vec![solved_tier(1.0), solved_tier(0.75)];
    let dirty = BTreeSet::from([0]);

    let rows = tier_items_stale_with_last_solved(&design, 1.54, Some(&last_solved), &dirty);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].mast.as_str(), "-");
    assert_eq!(rows[0].strategy.as_str(), "not solved");
    assert_eq!(rows[1].mast.as_str(), "0.7500");
    assert_eq!(rows[1].strategy.as_str(), "stale (Scale reference)");
}

/// A tier count mismatch (a tier was added/removed since `last_solved` was
/// captured) must not be trusted positionally -- every row falls back to the
/// same blank treatment as no cached solve at all.
#[test]
fn tier_items_stale_with_last_solved_ignores_a_mismatched_tier_count() {
    let mut design = fixture_design();
    design.tiers = vec![scale_reference_tier("Table"), scale_reference_tier("P1")];
    let stale_last_solved = vec![solved_tier(1.0)];
    let rows = tier_items_stale_with_last_solved(
        &design,
        1.54,
        Some(&stale_last_solved),
        &BTreeSet::new(),
    );
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.mast.as_str() == "-"));
}

// --- preform_y_offset_mm_text ---

#[test]
fn preform_y_offset_mm_text_is_empty_with_no_scale() {
    assert_eq!(preform_y_offset_mm_text(0.3, None), "");
}

#[test]
fn preform_y_offset_mm_text_converts_through_mm_per_unit() {
    assert_eq!(preform_y_offset_mm_text(0.3, Some(4.0)), "1.20");
}

// --- apply_proposed_angles ---

#[test]
fn apply_proposed_angles_patches_only_the_named_rows() {
    let mut design = fixture_design();
    design.tiers = vec![scale_reference_tier("Table"), scale_reference_tier("P1")];
    let mut rows = tier_items_stale(&design, 1.54);
    let changes = vec![indicatrix_cut_core::AngleChange {
        index: 1,
        from_deg: 40.0,
        to_deg: 41.25,
    }];
    apply_proposed_angles(&mut rows, &changes);
    assert_eq!(rows[0].proposed_angle.as_str(), "");
    assert_eq!(rows[1].proposed_angle.as_str(), "41.25");
}

#[test]
fn apply_proposed_angles_ignores_an_out_of_range_index() {
    let design = fixture_design();
    let mut rows = tier_items_stale(&design, 1.54);
    let changes = vec![indicatrix_cut_core::AngleChange {
        index: 5,
        from_deg: 0.0,
        to_deg: 12.0,
    }];
    // A tierless design's row list is empty -- this must not panic.
    apply_proposed_angles(&mut rows, &changes);
    assert_eq!(rows.len(), 0);
}
