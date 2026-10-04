//! Tests for [`super::sheet`]'s `CuttingSheet`/`CutSheetRow` rendering,
//! [`super::build`]'s `Design` methods, and [`super::diff`]'s [`diff_tiers`].

use super::{
    ConcaveRowInfo, CutSheetRow, CuttingSheet, diff_concave_tiers, diff_tiers,
    sheet::format_indices,
};
use crate::{
    design::{ConcaveTool, ConstraintTier, Design, ScheduleMeta, TierRef, ToolMotion},
    material::MaterialSelection,
    preform::PreformSpec,
};
use indicatrix::{
    geometry::meet_solver::{MeetConstraint, SolvedTier},
    optics::materials::GemMaterial,
};

fn round_brilliant_design() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

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

/// A design on a CUSTOM catalogue material must print that material's own
/// `n_D` in the "Refractive index" header line via `cutting_sheet_with` -- the
/// bug this module fixes. The built-ins-only `cutting_sheet` must still print
/// the legacy schedule RI for the exact same design.
#[test]
fn cutting_sheet_with_resolves_a_custom_materials_own_refractive_index() {
    let mut design = round_brilliant_design();
    design.material = MaterialSelection {
        name: Some("My Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
    };
    let custom = [custom_garnet(1.9)];
    let solved = design
        .solve()
        .expect("every tier is pinned via ScaleReference");

    let with_catalogue = design.cutting_sheet_with(&solved, &custom);
    assert!(
        with_catalogue
            .header
            .iter()
            .any(|l| l == "Refractive index: 1.900"),
        "{:?}",
        with_catalogue.header
    );

    let built_ins_only = design.cutting_sheet(&solved);
    assert!(
        built_ins_only
            .header
            .iter()
            .any(|l| l == &format!("Refractive index: {:.3}", design.meta.refractive_index))
    );
}

/// `cutting_sheet` must produce one row per tier, in tier order, with
/// the solved mast carried through and a non-empty meet instruction for
/// every row (this template's tiers are all `ScaleReference`).
#[test]
fn cutting_sheet_has_one_row_per_tier_in_order() {
    let design = round_brilliant_design();
    let solved = design
        .solve()
        .expect("every tier is pinned via ScaleReference");
    let sheet = design.cutting_sheet(&solved);
    assert_eq!(sheet.rows.len(), 8);
    for (i, row) in sheet.rows.iter().enumerate() {
        assert_eq!(row.sequence, i + 1);
        assert_eq!(row.name, design.tiers[i].name);
        assert_eq!(row.mast, solved[i].mast);
        assert_ne!(row.meet_instruction, "");
        assert_eq!(row.meets_tiers, Vec::<usize>::new()); // all ScaleReference here
    }
    assert!(sheet.header.iter().any(|l| l.starts_with("Index gear: 96")));
}

/// A tier with a recorded `Design::cheater_offset_deg` must carry it
/// through to its own `CutSheetRow` and appear in `CuttingSheet::to_text`;
/// every other row's `cheater_offset_deg` must stay `None` and print
/// nothing extra.
#[test]
fn cutting_sheet_carries_the_cheater_offset_into_its_own_row_and_text() {
    let mut design = round_brilliant_design();
    design.cheater_offsets_deg.insert(1, -0.75);
    let solved = design
        .solve()
        .expect("every tier is pinned via ScaleReference");
    let sheet = design.cutting_sheet(&solved);
    assert_eq!(sheet.rows[0].cheater_offset_deg, None);
    assert_eq!(sheet.rows[1].cheater_offset_deg, Some(-0.75));

    let text = sheet.to_text();
    let lines: Vec<&str> = text.lines().collect();
    let row1_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("2."))
        .expect("row 2 must be printed");
    assert!(row1_line.contains("cheater: -0.75 deg"), "{row1_line}");
    let row0_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("1."))
        .expect("row 1 must be printed");
    assert!(!row0_line.contains("cheater"), "{row0_line}");
}

/// Every row's `angle_of_elevation_deg` is the unsigned magnitude of
/// its `angle_deg`.
#[test]
fn angle_of_elevation_is_the_unsigned_angle_magnitude() {
    let design = round_brilliant_design();
    let solved = design
        .solve()
        .expect("every tier is pinned via ScaleReference");
    let sheet = design.cutting_sheet(&solved);
    for row in &sheet.rows {
        assert_eq!(row.angle_of_elevation_deg, row.angle_deg.abs());
    }
}

/// `depth_mm` is `None` for every row until `girdle_diameter_mm` is set,
/// and `Some` (mast times `mm_per_unit`) once it is.
#[test]
fn depth_mm_is_populated_only_once_a_girdle_diameter_anchors_a_real_scale() {
    let mut design = round_brilliant_design();
    let solved = design
        .solve()
        .expect("every tier is pinned via ScaleReference");
    let unanchored = design.cutting_sheet(&solved);
    assert!(unanchored.rows.iter().all(|r| r.depth_mm.is_none()));

    design.girdle_diameter_mm = Some(6.5);
    let anchored = design.cutting_sheet(&solved);
    let scale = design
        .yield_report(&solved)
        .mm_per_unit
        .expect("a closed design with a girdle diameter set must measure a scale");
    for (row, solved_tier) in anchored.rows.iter().zip(&solved) {
        let expected = solved_tier.mast * scale;
        assert!((row.depth_mm.expect("mm scale resolved") - expected).abs() < 1e-9);
    }
}

/// The header must carry a "Carat weight" line once a girdle diameter
/// anchors a real scale, and must not otherwise.
#[test]
fn header_carries_a_carat_weight_line_only_once_anchored() {
    let mut design = round_brilliant_design();
    let solved = design
        .solve()
        .expect("every tier is pinned via ScaleReference");
    let unanchored = design.cutting_sheet(&solved);
    assert!(
        !unanchored
            .header
            .iter()
            .any(|l| l.starts_with("Carat weight"))
    );

    design.girdle_diameter_mm = Some(6.5);
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
    };
    let anchored = design.cutting_sheet(&solved);
    assert!(
        anchored
            .header
            .iter()
            .any(|l| l.starts_with("Carat weight (estimate):")),
        "{:?}",
        anchored.header
    );
}

/// `facet_meets` must resolve a `MeetNamed` reference through the real
/// solver-grade resolver (here: an exact name match), return empty for
/// `MeetExisting`/`ScaleReference`, and error on an out-of-range index.
#[test]
fn facet_meets_resolves_named_references() {
    let tiers = vec![
        ConstraintTier {
            angle_deg: 34.5,
            name: "Crown Main".to_string(),
            indices: vec![0.0],
            constraint: MeetConstraint::ScaleReference(0.5),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        },
        ConstraintTier {
            angle_deg: 41.0,
            name: "Star".to_string(),
            indices: vec![0.0],
            constraint: MeetConstraint::MeetNamed(vec!["Crown Main".to_string()]),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        },
    ];
    let design = Design::new(
        PreformSpec::block(1.0, 1.0, 1.0),
        ScheduleMeta::standard_round_brilliant(),
        tiers,
    );
    assert_eq!(design.facet_meets(1).unwrap(), vec![0]);
    assert_eq!(design.facet_meets(0).unwrap(), Vec::<usize>::new());
    assert!(design.facet_meets(2).is_err());
}

/// `to_text` must render a header block followed by one line per row,
/// containing the row's own name, angle and mast figures.
#[test]
fn to_text_renders_header_and_rows() {
    let design = round_brilliant_design();
    let solved = design
        .solve()
        .expect("every tier is pinned via ScaleReference");
    let text = design.cutting_sheet(&solved).to_text();
    assert!(text.contains("Material: (unset)"));
    assert!(text.contains("Table"));
    assert!(text.contains("Girdle"));
    assert!(text.contains("mast"));
    // 3 header lines + 1 blank separator + 8 rows.
    assert_eq!(text.lines().count(), 3 + 1 + 8);
}

/// An index list is formatted as `"-"` when empty and with whole-tooth
/// integers otherwise.
#[test]
fn format_indices_matches_whole_vs_fractional_convention() {
    assert_eq!(format_indices(&[]), "-");
    assert_eq!(format_indices(&[12.0, 24.0]), "12, 24");
    assert_eq!(format_indices(&[11.5]), "11.50");
}

fn named_tier(name: &str, angle_deg: f64, constraint: MeetConstraint) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: Vec::new(),
        constraint,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// `diff_tiers` must report a changed angle for a common position, and
/// mark a tier past either list's end as added/removed.
#[test]
fn diff_tiers_reports_changes_and_added_removed_positions() {
    let before = vec![
        named_tier("Table", 0.0, MeetConstraint::ScaleReference(0.3)),
        named_tier("Star", 15.0, MeetConstraint::ScaleReference(0.4)),
    ];
    let mut after = before.clone();
    after[1].angle_deg = 16.0;
    after.push(named_tier(
        "Main",
        34.5,
        MeetConstraint::ScaleReference(0.5),
    ));

    let deltas = diff_tiers(&before, None, &after, None);
    assert_eq!(deltas.len(), 3);
    assert!(!deltas[0].angle_changed());
    assert!(deltas[1].angle_changed());
    assert!(!deltas[1].added());
    assert!(!deltas[1].removed());
    assert!(deltas[2].added());
    assert!(!deltas[2].removed());
}

/// A mismatched `solved` length must be treated as "no solved masts",
/// not a panic.
#[test]
fn diff_tiers_ignores_a_mismatched_solved_length() {
    let before = vec![named_tier(
        "Table",
        0.0,
        MeetConstraint::ScaleReference(0.3),
    )];
    let after = before.clone();
    let bogus_solved = [];
    let deltas = diff_tiers(&before, Some(&bogus_solved), &after, None);
    assert_eq!(deltas.len(), 1);
    assert!(deltas[0].mast_before.is_none());
    assert!(deltas[0].mast_after.is_none());
}

/// `try_cutting_sheet` must return [`crate::design::SolveMismatch`] (naming both the
/// expected and the actual tier count) instead of panicking when
/// `solved` is the wrong length; `cutting_sheet` itself must still
/// `panic!` on the exact same input (Finding: the four solved-list
/// alignment `panic!`s stay in place for `apps/**` callers, with a
/// `try_*`/`_with` fallible sibling next to each).
#[test]
fn try_cutting_sheet_reports_a_mismatch_instead_of_panicking() {
    let design = round_brilliant_design();
    let bogus_solved: Vec<SolvedTier> = Vec::new();
    let err = design
        .try_cutting_sheet(&bogus_solved)
        .expect_err("empty solved list must not align with 8 tiers");
    assert_eq!(err.expected_tiers, design.tiers.len());
    assert_eq!(err.got_tiers, 0);
}

/// `cutting_sheet` (the original, panicking entry point) must still
/// panic on the exact misalignment `try_cutting_sheet` now reports as
/// an error -- no behavior change for existing callers.
#[test]
#[should_panic(expected = "cutting_sheet: `solved`")]
fn cutting_sheet_still_panics_on_a_mismatch() {
    let design = round_brilliant_design();
    let bogus_solved: Vec<SolvedTier> = Vec::new();
    let _ = design.cutting_sheet(&bogus_solved);
}

/// A flat row with every optional column absent.
fn flat_row(sequence: usize) -> CutSheetRow {
    CutSheetRow {
        sequence,
        name: "Pav".to_string(),
        angle_deg: -42.0,
        indices: vec![0.0, 4.0],
        mast: 0.6,
        meet_instruction: "Meet P1".to_string(),
        meets_tiers: Vec::new(),
        cheater_offset_deg: None,
        angle_of_elevation_deg: 42.0,
        depth_mm: None,
        concave: None,
    }
}

fn cylinder_row(sequence: usize) -> CutSheetRow {
    CutSheetRow {
        mast: 0.0,
        meet_instruction: "cut to depth".to_string(),
        concave: Some(ConcaveRowInfo {
            tool: ConcaveTool::Cylinder,
            tool_azimuth_deg: 90.0,
            displacement: [0.0, 0.12, 0.05],
            diameter_ratio: 0.25,
            tool_angle_deg: None,
            motion: ToolMotion::Reciprocating,
        }),
        ..flat_row(sequence)
    }
}

/// The concave line sits under the facet line's columns: the tool code in the
/// name column, θ's last digit under φ's, the displacement under `indices`.
#[test]
fn cutting_sheet_text_for_concave_fixture_matches_expected_string() {
    let sheet = CuttingSheet {
        header: vec!["Material: Test".to_string()],
        rows: vec![flat_row(1), cylinder_row(2)],
    };
    let expected = "Material: Test\n\
\n\
\x20 1. Pav              angle  42.00 deg  indices [0, 4]  mast   0.6000  meet: Meet P1\n\
\x20 2. Pav              angle  42.00 deg  indices [0, 4]  mast   0.0000  meet: cut to depth\n\
\x20    CYL                   +90.00°      X = 0.000, Y = 0.120, Z = 0.050     D/W = 0.250, reciprocating\n";
    assert_eq!(sheet.to_text(), expected);
}

/// A planar row prints with positive angle.
#[test]
fn cutting_sheet_text_for_planar_fixture_is_unchanged() {
    let sheet = CuttingSheet {
        header: Vec::new(),
        rows: vec![flat_row(1)],
    };
    assert_eq!(
        sheet.to_text(),
        "  1. Pav              angle  42.00 deg  indices [0, 4]  mast   0.6000  meet: Meet P1\n"
    );
}

#[test]
fn cutting_sheet_line_count_is_flat_rows_plus_two_per_concave_row() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture's flat tiers solve");
    let sheet = design.cutting_sheet(&solved);
    let text = sheet.to_text();
    let flat = design.tiers.len();
    let concave = design.concave_tiers.len();
    assert_eq!(sheet.rows.len(), flat + concave);
    // Header block, one blank line, then one line per tier plus one tool line
    // per concave tier.
    assert_eq!(
        text.lines().count(),
        sheet.header.len() + 1 + flat + 2 * concave
    );

    // Rows follow `cutting_order`, numbered across both kinds.
    let order = design.cutting_order();
    for (position, (row, tier_ref)) in sheet.rows.iter().zip(&order).enumerate() {
        assert_eq!(row.sequence, position + 1);
        match *tier_ref {
            TierRef::Flat(i) => {
                if design.tiers[i].name.is_empty() {
                    assert!(!row.name.is_empty());
                } else {
                    assert_eq!(row.name, design.tiers[i].name);
                }
            }
            TierRef::Concave(i) => {
                assert_eq!(row.name, design.concave_tiers[i].name);
                let info = row
                    .concave
                    .as_ref()
                    .expect("a concave row carries its tool");
                assert_eq!(
                    info.second_line_fields(),
                    design.concave_tiers[i].second_line_fields()
                );
                assert_eq!(row.mast, 0.0);
            }
        }
    }
    // The fixture has no table, so the crown-side tool line closes the sheet.
    assert_eq!(order.last(), Some(&TierRef::Concave(1)));
}

#[test]
fn diff_concave_tiers_reports_added_removed_and_changed_positions() {
    let tiers = Design::concave_fixture().concave_tiers;
    let (a, b) = (tiers[0].clone(), tiers[1].clone());
    assert!(diff_concave_tiers(&tiers, &tiers).is_empty());

    let mut moved = a.clone();
    moved.tool_azimuth_deg = 15.0;
    let deltas = diff_concave_tiers(&[a.clone(), b.clone()], &[moved.clone()]);
    assert_eq!(deltas.len(), 2);
    assert_eq!(deltas[0].position, 0);
    assert!(deltas[0].changed());
    assert_eq!(deltas[0].before.as_ref(), Some(&a));
    assert_eq!(deltas[0].after.as_ref(), Some(&moved));
    assert_eq!(deltas[1].position, 1);
    assert_eq!(
        (deltas[1].before.as_ref(), deltas[1].after.as_ref()),
        (Some(&b), None)
    );
    assert!(!deltas[1].changed());

    let added = diff_concave_tiers(&[], std::slice::from_ref(&a));
    assert_eq!(added.len(), 1);
    assert!(added[0].before.is_none() && added[0].after.as_ref() == Some(&a));
}
