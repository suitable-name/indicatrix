//! Tests for [`super::planes`]'s plane arrangement/cheater-offset behaviour,
//! [`super::refractive`]'s catalogue-aware refractive index, and
//! [`super::schedule`]'s `.asc` export -- including the panic-to-error
//! (`SolveMismatch`) conversions all three share.

use super::meet_name_is_asc_safe;
use crate::{
    design::{ConcaveTier, ConcaveTool, ConstraintTier, Design, ScheduleMeta, ToolMotion},
    material::MaterialSelection,
    preform::PreformSpec,
};
use glam::DVec3;
use indicatrix::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts, meet_solver::SolvedTier},
    optics::materials::GemMaterial,
};

fn round_brilliant_design() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

/// `planes_through_tier` at the last tier index must match
/// `planes_from_solved` exactly, and at tier 0 must include only the
/// preform planes plus the first tier's own facet (the "Table" tier has
/// no indices, so exactly one plane).
#[test]
fn planes_through_tier_truncates_the_schedule() {
    let design = round_brilliant_design();
    let solved = design
        .solve()
        .expect("every tier is pinned via ScaleReference");
    let full = design.planes_from_solved(&solved);
    let through_last = design.planes_through_tier(&solved, design.tiers.len() - 1);
    assert_eq!(full.len(), through_last.len());

    let preform_count = design.preform.planes().len();
    let through_first = design.planes_through_tier(&solved, 0);
    assert_eq!(through_first.len(), preform_count + 1);
}

/// A `through_tier` past the last tier index must behave exactly like
/// `planes_from_solved` (every tier included).
#[test]
fn planes_through_tier_past_the_end_includes_everything() {
    let design = round_brilliant_design();
    let solved = design
        .solve()
        .expect("every tier is pinned via ScaleReference");
    let full = design.planes_from_solved(&solved);
    let past_end = design.planes_through_tier(&solved, 1000);
    assert_eq!(full.len(), past_end.len());
}

/// `preform_y_offset` must actually shift the
/// preform's own two horizontal planes in BOTH `planes_from_solved` and
/// `planes_through_tier` -- the two production entry points a real
/// solid/preview build from -- while leaving every schedule-facet plane
/// (and the plane count) completely unchanged, matching
/// `PreformSpec::planes_offset`'s own contract.
#[test]
fn preform_y_offset_shifts_the_preforms_own_planes_in_every_production_arrangement() {
    let mut design = round_brilliant_design();
    let solved = design
        .solve()
        .expect("every tier is pinned via ScaleReference");
    let unshifted = design.planes_from_solved(&solved);
    let unshifted_through = design.planes_through_tier(&solved, design.tiers.len() - 1);

    design.preform_y_offset = 0.25;
    let shifted = design.planes_from_solved(&solved);
    let shifted_through = design.planes_through_tier(&solved, design.tiers.len() - 1);

    assert_eq!(unshifted.len(), shifted.len());
    assert_eq!(unshifted_through.len(), shifted_through.len());
    // Same plane count as the unshifted arrangement, so this is exactly
    // `preform.planes_offset(0.25)` followed by the same facet planes --
    // the preform's own two horizontal planes (pushed last by
    // `PreformSpec::planes`) must have moved; every other plane must not.
    let preform_count = design.preform.planes().len();
    for i in 0..preform_count - 2 {
        assert_eq!(unshifted[i], shifted[i], "non-vertical preform plane {i}");
    }
    assert_ne!(unshifted[preform_count - 2].1, shifted[preform_count - 2].1);
    assert_ne!(unshifted[preform_count - 1].1, shifted[preform_count - 1].1);
    for i in preform_count..unshifted.len() {
        assert_eq!(unshifted[i], shifted[i], "schedule facet plane {i}");
    }
}

/// A cheater offset recorded on exactly one tier must rotate that tier's OWN
/// facet plane(s) about `+Y` and leave every other plane -- preform planes
/// AND every other tier's facet planes -- byte-identical, in both
/// `planes_from_solved` and `planes_through_tier` (the two production
/// arrangements the solid/facet map/manufacturability checks all read from).
/// Without this rotation, this offset would be persisted and printed but
/// completely ignored by the geometry.
#[test]
fn cheater_offset_rotates_only_its_own_tiers_planes() {
    let mut design = round_brilliant_design();
    let solved = design
        .solve()
        .expect("every tier is pinned via ScaleReference");
    let unrotated = design.planes_from_solved(&solved);
    let unrotated_through = design.planes_through_tier(&solved, design.tiers.len() - 1);

    // Tier 2 ("Crown Main") gets a real cheater offset; every other tier stays
    // untouched.
    design.cheater_offsets_deg.insert(2, 0.5);
    let rotated = design.planes_from_solved(&solved);
    let rotated_through = design.planes_through_tier(&solved, design.tiers.len() - 1);

    assert_eq!(unrotated.len(), rotated.len());
    assert_eq!(unrotated_through.len(), rotated_through.len());

    let schedule = design.to_asc_schedule_from_solved(&solved);
    let boundaries = crate::manufacturability::facet_plane_boundaries(&schedule);
    let preform_len = design.preform.planes().len();
    let tier2_start = preform_len + boundaries[1];
    let tier2_end = preform_len + boundaries[2];
    assert!(
        tier2_end > tier2_start,
        "Crown Main must contribute at least one facet plane"
    );

    for i in 0..unrotated.len() {
        if (tier2_start..tier2_end).contains(&i) {
            assert_ne!(
                unrotated[i].0, rotated[i].0,
                "plane {i} (Crown Main's own facet) must have rotated"
            );
            // The offset only shifts azimuth: the plane's own distance from
            // the origin is unchanged.
            assert!((unrotated[i].1 - rotated[i].1).abs() < 1e-9);
        } else {
            assert_eq!(unrotated[i], rotated[i], "plane {i} must not have moved");
        }
    }
    for i in 0..unrotated_through.len() {
        assert_eq!(
            unrotated_through[i].1, rotated_through[i].1,
            "plane {i} offset must not have moved"
        );
        if (tier2_start..tier2_end).contains(&i) {
            assert_ne!(unrotated_through[i].0, rotated_through[i].0);
        } else {
            assert_eq!(unrotated_through[i].0, rotated_through[i].0);
        }
    }
}

/// The rotated plane normals (what the solid, tracer and diagram read) and the
/// normals rebuilt from the `.asc` export with the offset baked into its
/// indices must agree including sign: a positive offset moves the facet toward
/// higher index numbers in both.
///
/// The offset is half an index-wheel tooth (`360 / 96 / 2` degrees) so the baked
/// index shift of `0.5` survives the export's 3-decimal rounding exactly.
#[test]
fn cheater_offset_rotation_matches_the_exported_index_shift() {
    let mut design = round_brilliant_design();
    let solved = design
        .solve()
        .expect("every tier is pinned via ScaleReference");
    let unshifted = design.planes_from_solved(&solved);
    design.cheater_offsets_deg.insert(2, 1.875);
    let via_rotation = design.planes_from_solved(&solved);

    let exported = design.to_asc_schedule_from_solved_with_cheater_offsets(&solved, &[]);
    let via_indices: Vec<(DVec3, f64)> = StandardGemCuts::from_asc_schedule(&exported)
        .into_iter()
        .map(GpuFacetPlane::to_halfspace_f64)
        .collect();

    let preform_len = design.preform.planes().len();
    assert_eq!(via_rotation.len() - preform_len, via_indices.len());
    let mut moved = 0usize;
    for (i, expected) in via_indices.iter().enumerate() {
        let (normal, offset) = via_rotation[preform_len + i];
        assert!(
            (normal - expected.0).length() < 1e-5,
            "facet plane {i}: rotated normal {normal:?} vs exported-index normal {:?}",
            expected.0
        );
        assert!((offset - expected.1).abs() < 1e-5, "facet plane {i} offset");
        if (normal - unshifted[preform_len + i].0).length() > 1e-3 {
            moved += 1;
        }
    }
    assert!(
        moved > 0,
        "the offset must actually move Crown Main's facets"
    );
}

/// A cheater offset of exactly `0.0` (the default -- no offset actually
/// recorded, or one explicitly cleared back to zero) must leave every plane
/// unchanged, matching "never print a value the picture ignores" the other
/// way around: a zero offset the picture DOES apply is indistinguishable
/// from no offset at all.
#[test]
fn zero_cheater_offset_changes_nothing() {
    let mut design = round_brilliant_design();
    let solved = design
        .solve()
        .expect("every tier is pinned via ScaleReference");
    let before = design.planes_from_solved(&solved);
    design.cheater_offsets_deg.insert(2, 0.0);
    let after = design.planes_from_solved(&solved);
    assert_eq!(before, after);
}

/// `gear_reference_angle` is NOT geometry-inert for a block
/// preform (see [`crate::edit::Edit::SetMeta`]'s doc comment)
/// -- it rotates every solved facet plane's azimuth (see
/// `indicatrix::geometry::cuts::StandardGemCuts::index_to_azimuth`) while the
/// block's own fixed walls do not rotate with it, so the combined plane
/// arrangement genuinely changes, not just a relabeling of the same solid.
#[test]
fn gear_reference_angle_changes_planes_from_solved_on_a_block_preform() {
    let mut design = round_brilliant_design();
    let solved = design
        .solve()
        .expect("every tier is pinned via ScaleReference");
    let unrotated = design.planes_from_solved(&solved);

    design.meta.gear_reference_angle = 6.0;
    let rotated = design.planes_from_solved(&solved);

    assert_eq!(
        unrotated.len(),
        rotated.len(),
        "a reference-angle change must not add or drop planes"
    );
    assert_ne!(
        unrotated, rotated,
        "gear_reference_angle must rotate the facet planes on a block preform, \
         contrary to the old 'affects no geometry' doc"
    );
}

// --- meet_name_is_asc_safe ---

#[test]
fn meet_name_is_asc_safe_accepts_a_plain_alphanumeric_name() {
    assert!(meet_name_is_asc_safe("P1"));
    assert!(meet_name_is_asc_safe("Girdle"));
    assert!(meet_name_is_asc_safe("C1"));
}

#[test]
fn meet_name_is_asc_safe_rejects_whitespace_and_separators() {
    assert!(!meet_name_is_asc_safe("Crown Main"));
    assert!(!meet_name_is_asc_safe("P1,P2"));
    assert!(!meet_name_is_asc_safe("P1;P2"));
    assert!(!meet_name_is_asc_safe(""));
}

#[test]
fn meet_name_is_asc_safe_rejects_leading_or_trailing_punctuation() {
    // The mirrored-tier naming convention (`ConstraintTier::mirrored_to_other_block`)
    // is exactly the case this guards: `"Main'"` re-parses as `"Main"`.
    assert!(!meet_name_is_asc_safe("Main'"));
    assert!(!meet_name_is_asc_safe("'Main"));
}

// --- Design::effective_refractive_index_with ---

fn custom_garnet(n_d: f32) -> GemMaterial {
    let mut gem = GemMaterial::diamond();
    gem.name = "My Garnet".to_string();
    gem.dispersion = indicatrix::optics::dispersion::DispersionModel::Cauchy {
        a: n_d,
        b: 0.0,
        c: 0.0,
    };
    gem
}

/// An explicit RI override still wins even when a same-named custom material is
/// also present -- the same top-of-precedence rule
/// `effective_refractive_index` already follows.
#[test]
fn effective_refractive_index_with_prefers_the_override_over_a_custom_entry() {
    let mut design = round_brilliant_design();
    design.material = MaterialSelection {
        name: Some("My Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: Some(1.70),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let custom = [custom_garnet(1.90)];
    assert_eq!(design.effective_refractive_index_with(&custom), 1.70);
}

/// A design named after a CUSTOM catalogue entry (not one of the thirteen
/// built-ins) must resolve to that entry's own `n_D` -- the bug
/// `effective_refractive_index` alone cannot fix, since it only ever consults
/// the built-in table.
#[test]
fn effective_refractive_index_with_resolves_a_custom_material_by_name() {
    let mut design = round_brilliant_design();
    design.material = MaterialSelection {
        name: Some("My Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    // The built-ins-only version has no idea what "My Garnet" is and falls all
    // the way back to the legacy schedule RI.
    assert_eq!(
        design.effective_refractive_index(),
        design.meta.refractive_index
    );
    let custom = [custom_garnet(1.90)];
    assert!((design.effective_refractive_index_with(&custom) - 1.90).abs() < 1e-6);
}

/// A recognized built-in name still resolves correctly when `custom` has
/// nothing matching it -- the "no custom catalogue loaded" case must behave
/// exactly like `effective_refractive_index`.
#[test]
fn effective_refractive_index_with_falls_back_to_a_built_in_when_no_custom_entry_matches() {
    let mut design = round_brilliant_design();
    design.material = MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let custom: [GemMaterial; 0] = [];
    assert_eq!(
        design.effective_refractive_index_with(&custom),
        design.effective_refractive_index()
    );
}

/// With no material selection and no matching custom entry at all, both
/// functions must fall back to the same legacy schedule RI.
#[test]
fn effective_refractive_index_with_falls_back_to_the_legacy_value_when_nothing_resolves() {
    let design = round_brilliant_design();
    let custom: [GemMaterial; 0] = [];
    assert_eq!(
        design.effective_refractive_index_with(&custom),
        design.meta.refractive_index
    );
}

// --- to_asc_schedule_with / to_asc_schedule_from_solved_with (custom-material `I` line) ---

/// A design on a CUSTOM catalogue material must export an `I` line matching
/// that material's own `n_D` through the `_with` entry points -- the bug this
/// module fixes. `to_asc_schedule` (built-ins-only) must still fall back to the
/// legacy schedule RI for the exact same design, proving the two really do
/// differ only in whether a custom catalogue was supplied.
#[test]
fn to_asc_schedule_with_resolves_a_custom_materials_own_refractive_index() {
    let mut design = round_brilliant_design();
    design.material = MaterialSelection {
        name: Some("My Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let custom = [custom_garnet(1.9)];

    let with_catalogue = design
        .to_asc_schedule_with(&custom)
        .expect("every tier is pinned via ScaleReference");
    assert!((with_catalogue.refractive_index - 1.9).abs() < 1e-6);

    let built_ins_only = design
        .to_asc_schedule()
        .expect("every tier is pinned via ScaleReference");
    assert_eq!(
        built_ins_only.refractive_index,
        design.meta.refractive_index
    );
}

/// Same check via the already-solved entry points
/// (`to_asc_schedule_from_solved_with`/`to_asc_schedule_from_solved`), and via
/// the fallible `try_*_with` sibling.
#[test]
fn to_asc_schedule_from_solved_with_resolves_a_custom_materials_own_refractive_index() {
    let mut design = round_brilliant_design();
    design.material = MaterialSelection {
        name: Some("My Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let custom = [custom_garnet(1.9)];
    let solved = design
        .solve()
        .expect("every tier is pinned via ScaleReference");

    let with_catalogue = design.to_asc_schedule_from_solved_with(&solved, &custom);
    assert!((with_catalogue.refractive_index - 1.9).abs() < 1e-6);

    let via_try = design
        .try_to_asc_schedule_from_solved_with(&solved, &custom)
        .expect("solved is aligned with design.tiers");
    assert!((via_try.refractive_index - 1.9).abs() < 1e-6);

    let built_ins_only = design.to_asc_schedule_from_solved(&solved);
    assert_eq!(
        built_ins_only.refractive_index,
        design.meta.refractive_index
    );
}

/// A design on a recognized built-in ("Diamond") must export the exact same `I`
/// line -- and in fact the exact same full `.asc` text -- through both the old
/// (built-ins-only) and new (`_with`, `custom` empty or irrelevant) entry
/// points, matching this crate's own determinism requirement.
#[test]
fn to_asc_schedule_with_matches_the_old_entry_point_byte_for_byte_on_a_built_in() {
    let mut design = round_brilliant_design();
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    // A custom catalogue with an unrelated entry must not perturb a design named
    // after a built-in -- built-ins take precedence over nothing here, since
    // `custom` has no "Diamond" entry of its own to shadow it with.
    let custom = [custom_garnet(1.9)];

    let old = design
        .to_asc_schedule()
        .expect("every tier is pinned via ScaleReference");
    let new = design
        .to_asc_schedule_with(&custom)
        .expect("every tier is pinned via ScaleReference");
    assert!((old.refractive_index - 2.417).abs() < 1e-3);
    assert_eq!(old.refractive_index, new.refractive_index);
    assert_eq!(
        indicatrix_formats::asc::to_asc_string(&old),
        indicatrix_formats::asc::to_asc_string(&new)
    );
}

// --- SolveMismatch (panic-to-error conversion) ---

/// `try_to_asc_schedule_from_solved` must return [`crate::design::SolveMismatch`]
/// instead of panicking on a misaligned `solved`, and
/// `to_asc_schedule_from_solved` must still panic on the exact same
/// input.
#[test]
fn try_to_asc_schedule_from_solved_reports_a_mismatch_instead_of_panicking() {
    let design = round_brilliant_design();
    let bogus_solved: Vec<SolvedTier> = Vec::new();
    let err = design
        .try_to_asc_schedule_from_solved(&bogus_solved)
        .expect_err("empty solved list must not align with 8 tiers");
    assert_eq!(err.expected_tiers, design.tiers.len());
    assert_eq!(err.got_tiers, 0);
}

#[test]
#[should_panic(expected = "to_asc_schedule_from_solved: `solved`")]
fn to_asc_schedule_from_solved_still_panics_on_a_mismatch() {
    let design = round_brilliant_design();
    let bogus_solved: Vec<SolvedTier> = Vec::new();
    let _ = design.to_asc_schedule_from_solved(&bogus_solved);
}

/// Same pair of checks for `planes_through_tier`/`try_planes_through_tier`.
#[test]
fn try_planes_through_tier_reports_a_mismatch_instead_of_panicking() {
    let design = round_brilliant_design();
    let bogus_solved: Vec<SolvedTier> = Vec::new();
    let err = design
        .try_planes_through_tier(&bogus_solved, 0)
        .expect_err("empty solved list must not align with 8 tiers");
    assert_eq!(err.expected_tiers, design.tiers.len());
    assert_eq!(err.got_tiers, 0);
}

#[test]
#[should_panic(expected = "planes_through_tier: `solved`")]
fn planes_through_tier_still_panics_on_a_mismatch() {
    let design = round_brilliant_design();
    let bogus_solved: Vec<SolvedTier> = Vec::new();
    let _ = design.planes_through_tier(&bogus_solved, 0);
}

fn concave_pair(design: &mut Design) {
    design.concave_tiers = vec![
        ConcaveTier {
            name: "Groove".to_string(),
            angle_deg: -62.0,
            indices: vec![3.0, 11.5, 19.0],
            instructions: "cut to depth".to_string(),
            tool: ConcaveTool::Cylinder,
            tool_azimuth_deg: 90.0,
            displacement: [0.0, 0.12, 0.05],
            diameter_ratio: 0.25,
            tool_angle_deg: None,
            motion: ToolMotion::Reciprocating,
        },
        ConcaveTier {
            name: "Pit".to_string(),
            angle_deg: 40.0,
            indices: vec![0.0],
            instructions: String::new(),
            tool: ConcaveTool::Cone,
            tool_azimuth_deg: 0.0,
            displacement: [0.0; 3],
            diameter_ratio: 0.5,
            tool_angle_deg: Some(60.0),
            motion: ToolMotion::Plunge,
        },
    ];
}

#[test]
fn asc_export_omits_concave_tiers_and_appends_two_footnotes_per_tier() {
    let planar = round_brilliant_design();
    let mut design = round_brilliant_design();
    design.meta.footnotes = vec!["my own note".to_string()];
    concave_pair(&mut design);
    let solved = design.solve().expect("solves");
    let schedule = design.to_asc_schedule_for_export(&solved);

    assert_eq!(schedule.tiers.len(), planar.tiers.len());
    // Pavilion-side tier first (cutting order), then the crown-side one.
    assert_eq!(
        schedule.footnotes,
        vec![
            "my own note".to_string(),
            "Groove  -62.00  3 11.50 19  cut to depth  [concave tier]".to_string(),
            "CYL  +90.00 deg  X = 0.000, Y = 0.120, Z = 0.050  D/W = 0.250, reciprocating  \
             [concave tier]"
                .to_string(),
            "Pit  40.00  0  [concave tier]".to_string(),
            "CON  0.00 deg  X = 0.000, Y = 0.000, Z = 0.000  D/W = 0.500, angle = 60.00 deg, \
             plunge  [concave tier]"
                .to_string(),
        ]
    );
    // Every generated line is ASCII (GemCad reads an `.asc` as single-byte text) and
    // ends in the marker the load recognises them by; the user's own note has neither.
    for line in &schedule.footnotes[1..] {
        assert!(line.is_ascii(), "{line:?}");
        assert!(
            line.ends_with(crate::design::CONCAVE_FOOTNOTE_MARKER),
            "{line:?}"
        );
    }
    // The writer accepts every line, and the plain schedule is untouched.
    indicatrix_formats::asc::to_asc_string(&schedule).expect("every footnote is one line");
    assert_eq!(
        design.to_asc_schedule_from_solved(&solved).footnotes,
        vec!["my own note".to_string()]
    );
    assert_eq!(
        planar.export_warnings(),
        [] as [crate::manufacturability::ManufacturabilityWarning; 0]
    );
    assert_eq!(
        design.export_warnings(),
        vec![
            crate::manufacturability::ManufacturabilityWarning::ConcaveTiersOmittedFromExport {
                count: 2
            }
        ]
    );
}

/// A caller that holds a concave tier only as stored text gets the very lines the design's
/// export writes, from the same functions, and a degree sign typed into the instructions is
/// written as `deg` like the tool line's, so the whole line stays ASCII.
#[test]
fn the_footnote_builders_give_the_lines_the_design_export_writes() {
    let mut design = round_brilliant_design();
    concave_pair(&mut design);
    design.concave_tiers[0].instructions = "tilt 45°".to_string();
    let solved = design.solve().expect("solves");
    let schedule = design.to_asc_schedule_for_export(&solved);

    let [groove_facet, groove_tool, pit_facet, pit_tool] = schedule.footnotes.as_slice() else {
        panic!("two footnotes per tier: {:?}", schedule.footnotes);
    };
    assert_eq!(
        groove_facet,
        "Groove  -62.00  3 11.50 19  tilt 45 deg  [concave tier]"
    );
    assert_eq!(
        *groove_facet,
        crate::design::concave_facet_footnote("Groove", "-62.00", "3 11.50 19", "tilt 45°")
    );
    assert_eq!(
        *groove_tool,
        crate::design::concave_tool_footnote(
            "CYL  +90.00°  X = 0.000, Y = 0.120, Z = 0.050  D/W = 0.250, reciprocating"
        )
    );
    // An empty instruction leaves no trailing spaces before the marker.
    assert_eq!(
        *pit_facet,
        crate::design::concave_facet_footnote("Pit", "40.00", "0", "")
    );
    assert_eq!(
        *pit_tool,
        crate::design::concave_tool_footnote(
            "CON  0.00°  X = 0.000, Y = 0.000, Z = 0.000  D/W = 0.500, angle = 60.00°, plunge"
        )
    );
    assert!(schedule.footnotes.iter().all(|line| line.is_ascii()));
}

#[test]
fn asc_export_then_import_then_export_does_not_duplicate_concave_footnotes() {
    let mut design = round_brilliant_design();
    design.meta.footnotes = vec!["my own note".to_string()];
    concave_pair(&mut design);
    let solved = design.solve().expect("solves");
    let first = design.to_asc_schedule_for_export(&solved);
    let text = indicatrix_formats::asc::to_asc_string(&first).expect("writes");

    let parsed = indicatrix_formats::asc::parse_asc(&text).expect("parses");
    let mut reimported = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &parsed);
    assert_eq!(
        reimported.meta.footnotes, first.footnotes,
        "footnotes import"
    );
    // Re-importing leaves them as footnotes only; the author re-adds the tiers.
    concave_pair(&mut reimported);
    let solved = reimported.solve().expect("solves");
    let second = reimported.to_asc_schedule_for_export(&solved);
    assert_eq!(second.footnotes, first.footnotes);
    // The design's own footnote list was never mutated by the strip.
    assert_eq!(reimported.meta.footnotes, first.footnotes);
}

/// Editing a tier after an export must not leave the old export's lines beside the
/// new ones: the reimported footnotes are recognised by their shape and replaced,
/// leaving exactly two lines per tier and the user's own notes.
#[test]
fn an_edited_concave_tier_leaves_no_stale_footnotes_on_the_next_export() {
    let mut design = round_brilliant_design();
    design.meta.footnotes = vec!["my own note".to_string()];
    concave_pair(&mut design);
    let solved = design.solve().expect("solves");
    let first = design.to_asc_schedule_for_export(&solved);
    let text = indicatrix_formats::asc::to_asc_string(&first).expect("writes");

    let parsed = indicatrix_formats::asc::parse_asc(&text).expect("parses");
    let mut reimported = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &parsed);
    concave_pair(&mut reimported);
    // Edit Z of the first tier, its instructions and the second tier's tool.
    reimported.concave_tiers[0].displacement[2] = 0.2;
    reimported.concave_tiers[0].instructions = "cut deeper".to_string();
    reimported.concave_tiers[1].diameter_ratio = 0.45;
    let solved = reimported.solve().expect("solves");
    let second = reimported.to_asc_schedule_for_export(&solved);

    assert_eq!(
        second.footnotes,
        vec![
            "my own note".to_string(),
            "Groove  -62.00  3 11.50 19  cut deeper  [concave tier]".to_string(),
            "CYL  +90.00 deg  X = 0.000, Y = 0.120, Z = 0.200  D/W = 0.250, reciprocating  \
             [concave tier]"
                .to_string(),
            "Pit  40.00  0  [concave tier]".to_string(),
            "CON  0.00 deg  X = 0.000, Y = 0.000, Z = 0.000  D/W = 0.450, angle = 60.00 deg, \
             plunge  [concave tier]"
                .to_string(),
        ]
    );
    // A third round is stable.
    let text = indicatrix_formats::asc::to_asc_string(&second).expect("writes");
    let parsed = indicatrix_formats::asc::parse_asc(&text).expect("parses");
    let mut again = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &parsed);
    again.concave_tiers = reimported.concave_tiers.clone();
    again.concave_tier_ids = reimported.concave_tier_ids.clone();
    let solved = again.solve().expect("solves");
    assert_eq!(
        again.to_asc_schedule_for_export(&solved).footnotes,
        second.footnotes
    );
}

/// Only a line the export marked (or an unmarked line an earlier version wrote for a tier
/// the design has now, byte for byte) is generated; everything else is the user's, however
/// it looks.
#[test]
fn only_marked_or_legacy_lines_of_current_tiers_are_stripped() {
    let mut design = round_brilliant_design();
    concave_pair(&mut design);
    let mut notes: Vec<String> = [
        "Cut slowly, 45.00 rpm",
        // Marked: the lines of an export, for these tiers or an earlier shape of them.
        "Groove  -62.00  3 11.50 19  cut to depth  [concave tier]",
        "CYL  +90.00 deg  X = 0.000, Y = 0.500, Z = 0.050  D/W = 0.250, reciprocating  \
         [concave tier]",
        // Unmarked, written by an earlier version for the Groove tier as it is now.
        "Groove  -62.00  3 11.50 19  cut to depth",
        "CYL  +90.00°  X = 0.000, Y = 0.120, Z = 0.050  D/W = 0.250, reciprocating",
        // The user's: shaped like a tool line, but for a tool no tier has.
        "DSC  -3.50°  X = 1.000, Y = -0.500, Z = 0.000  D/W = 0.500, angle = 60.00°, plunge",
        "Notch  12.00  5  by hand",
        "SPH note: CYL  not a tool line",
        // The marker must be a field of its own.
        "tagged[concave tier]",
        "keep me",
    ]
    .map(String::from)
    .to_vec();
    super::schedule::strip_generated_concave_footnotes(&mut notes, &design.concave_tiers);
    assert_eq!(
        notes,
        vec![
            "Cut slowly, 45.00 rpm".to_string(),
            "DSC  -3.50°  X = 1.000, Y = -0.500, Z = 0.000  D/W = 0.500, angle = 60.00°, plunge"
                .to_string(),
            "Notch  12.00  5  by hand".to_string(),
            "SPH note: CYL  not a tool line".to_string(),
            "tagged[concave tier]".to_string(),
            "keep me".to_string(),
        ]
    );
}

/// A note the user typed in the shape of a facet line and a tool line (copied from a
/// sheet, say) is not the export's: it survives the paired load's strip and the next export,
/// and the export's own lines still replace each other.
#[test]
fn a_users_own_tool_shaped_footnotes_survive_the_export_round_trip() {
    let users_pair = [
        "Notch  12.00  5  by hand".to_string(),
        "CYL  +45.00°  X = 0.100, Y = 0.200, Z = 0.300  D/W = 0.300, plunge".to_string(),
    ];
    let mut design = round_brilliant_design();
    design.meta.footnotes = users_pair.to_vec();
    concave_pair(&mut design);
    let solved = design.solve().expect("solves");
    let first = design.to_asc_schedule_for_export(&solved);
    assert_eq!(
        first.footnotes[..2],
        users_pair,
        "the export keeps the user's lines first"
    );
    assert_eq!(first.footnotes.len(), 6);

    // What a paired load does to the footnotes the `.asc` carries.
    let text = indicatrix_formats::asc::to_asc_string(&first).expect("writes");
    let parsed = indicatrix_formats::asc::parse_asc(&text).expect("parses");
    let mut loaded = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &parsed);
    concave_pair(&mut loaded);
    super::schedule::strip_generated_concave_footnotes(
        &mut loaded.meta.footnotes,
        &loaded.concave_tiers,
    );
    assert_eq!(loaded.meta.footnotes, users_pair);

    let solved = loaded.solve().expect("solves");
    assert_eq!(
        loaded.to_asc_schedule_for_export(&solved).footnotes,
        first.footnotes
    );
}

/// The two footnotes per tier that `concave_pair`'s tiers got in an `.asc` written before the
/// export marked its lines, as a load reads them back (the `.asc` parser trims every
/// footnote line, so the empty instructions of the second tier leave no trailing spaces).
///
/// Written out by hand from what that build produced (the facet line, then the tool line
/// with `°`), NOT from the current formulas: a change to a formula must not quietly change
/// what counts as an earlier file's line.
const EARLIER_VERSION_FOOTNOTES: [&str; 4] = [
    "Groove  -62.00  3 11.50 19  cut to depth",
    "CYL  +90.00°  X = 0.000, Y = 0.120, Z = 0.050  D/W = 0.250, reciprocating",
    "Pit  40.00  0",
    "CON  0.00°  X = 0.000, Y = 0.000, Z = 0.000  D/W = 0.500, angle = 60.00°, plunge",
];

/// A file an earlier version exported carries unmarked lines with `°`; the paired load
/// still recognises those of the tiers it restores, so the next export does not write each
/// line twice.
#[test]
fn footnotes_an_earlier_version_wrote_are_replaced_not_duplicated() {
    let mut design = round_brilliant_design();
    design.meta.footnotes = vec!["my own note".to_string()];
    concave_pair(&mut design);
    assert!(
        EARLIER_VERSION_FOOTNOTES
            .iter()
            .any(|line| line.contains('°'))
    );
    let mut loaded = design.clone();
    loaded.meta.footnotes = std::iter::once("my own note".to_string())
        .chain(EARLIER_VERSION_FOOTNOTES.map(String::from))
        .collect();

    super::schedule::strip_generated_concave_footnotes(
        &mut loaded.meta.footnotes,
        &loaded.concave_tiers,
    );
    assert_eq!(loaded.meta.footnotes, vec!["my own note".to_string()]);
    let solved = loaded.solve().expect("solves");
    assert_eq!(
        loaded.to_asc_schedule_for_export(&solved).footnotes,
        design.to_asc_schedule_for_export(&solved).footnotes
    );
}
