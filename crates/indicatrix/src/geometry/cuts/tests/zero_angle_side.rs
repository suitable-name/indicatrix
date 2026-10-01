//! The table/culet side rule end to end: real corpus zero-angle lines, parsed by
//! `parse_asc` and turned into planes by [`StandardGemCuts::from_asc_schedule`]
//! (and blocks by the meet solver's `classify_blocks`), land on the side the file
//! states -- never on the side of the tier before them.

use indicatrix_formats::asc::parse_asc;

use crate::geometry::{
    cuts::StandardGemCuts,
    meet_solver::{Block, classify_blocks, meet_tier_inputs_from_asc},
    plane::GpuFacetPlane,
};

const HDR: &str = "GemCad 5.0\ng 96 0.0\ny 1 n\nI 1.54\n";

/// The one flat (zero-angle) plane in `planes`: its normal is `(0, +-1, 0)`.
fn flat_plane(planes: &[GpuFacetPlane]) -> GpuFacetPlane {
    let flats: Vec<GpuFacetPlane> = planes
        .iter()
        .copied()
        .filter(|p| p.normal[0].abs() < 1e-6 && p.normal[2].abs() < 1e-6)
        .collect();
    assert_eq!(flats.len(), 1, "{planes:?}");
    flats[0]
}

/// Parses `tiers`, then checks the flat facet's side and depth in both the plane
/// builder and the meet solver's block classification (`flat_tier` is its
/// position in the file).
fn assert_flat_side(tiers: &str, flat_tier: usize, crown: bool, depth: f64) {
    let schedule = parse_asc(&format!("{HDR}{tiers}")).expect("fixture must parse");
    let plane = flat_plane(&StandardGemCuts::from_asc_schedule(&schedule));
    let expected_y = if crown { 1.0 } else { -1.0 };
    assert!(
        (plane.normal[1] - expected_y).abs() < 1e-6,
        "{tiers:?}: flat facet normal {:?}",
        plane.normal
    );
    assert!(
        (f64::from(plane.d) + depth).abs() < 1e-6,
        "{tiers:?}: d = {}",
        plane.d
    );

    let blocks = classify_blocks(&meet_tier_inputs_from_asc(&schedule));
    let expected_block = if crown { Block::Crown } else { Block::Pavilion };
    assert_eq!(blocks[flat_tier], expected_block, "{tiers:?}");
}

/// pc45015 (`a 0.00 -0.28924 0`, tier 4 of 7, here right after a crown tier):
/// the documented culet encoding, no longer a second table.
#[test]
fn a_zero_angle_negative_distance_culet_after_a_crown_tier_is_pavilion() {
    assert_flat_side("a 41 0.5 0 48\na 0.00 -0.28924 0\n", 1, false, 0.28924);
}

/// pc43001a (`a 0.00 -0.368 0`) as the very first tier -- `GemCAD`'s
/// pavilion-first sort order starts at the culet.
#[test]
fn a_culet_as_the_first_tier_is_pavilion() {
    assert_flat_side("a 0.00 -0.368 0\na -41 0.6 0 48\n", 0, false, 0.368);
}

/// pc45116 (`a -0.000000 -0.76837424 96 n F G SMALL CULET FACET`) after a crown tier.
#[test]
fn a_negative_zero_token_culet_after_a_crown_tier_is_pavilion() {
    assert_flat_side(
        "a 41 0.5 0 48\na -0.000000 -0.76837424 96 n F G SMALL CULET FACET\n",
        1,
        false,
        0.768_374_24,
    );
}

/// pc28212 (`a 0.000000 0.57655827 96 n T`) straight after a pavilion tier --
/// `GemCAD`'s table-first crown order -- is the table, not a culet.
#[test]
fn a_positive_distance_table_after_a_pavilion_tier_is_crown() {
    assert_flat_side(
        "a -41 0.6 0 48\na 0.000000 0.57655827 96 n T\n",
        1,
        true,
        0.576_558_27,
    );
}
