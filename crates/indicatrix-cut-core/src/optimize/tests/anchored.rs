//! Tests for varying `ScaleReference` tiers about a hinge, and for per-tier angle
//! bounds: the plane geometry ([`super::super::space`]) checked against the planes
//! `Design::planes_from_solved` really builds, and the candidate builders that use it.

use super::{
    super::{
        OptimizeOptions, candidate, free_tier_indices_with,
        space::{SearchSpace, measure_anchors},
    },
    fixtures::{imported_rbc, rbc_445},
};
use crate::design::{Design, mast_through, tier_hinges, tier_plane_ranges};
use glam::DVec3;
use indicatrix::geometry::{
    meet_solver::MeetConstraint,
    stone_metrics::{SolidStatus, build_solid_mesh},
};
use std::collections::BTreeMap;

/// The search space a run over `design` with `options` would resolve (anchors measured on
/// the design's own solve), and the free tiers it searches.
pub(super) fn space_for(design: &Design, options: &OptimizeOptions) -> (SearchSpace, Vec<usize>) {
    let free = free_tier_indices_with(design, options);
    let solved = design.solve().expect("fixture must solve");
    let planes = design.planes_from_solved(&solved);
    let anchors = measure_anchors(design, options, &free, &solved, &planes);
    (SearchSpace::new(options, anchors), free)
}

/// A real hinge for every movable tier of `design`: the first vertex of the tier's
/// first facet in the solid, so the point lies on that facet's plane exactly like a
/// vertex a front end would pick.
pub(super) fn hinge_points(design: &Design) -> BTreeMap<usize, DVec3> {
    let solved = design.solve().expect("fixture must solve");
    let planes = design.planes_from_solved(&solved);
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        panic!("fixture must close into a solid");
    };
    let ranges = tier_plane_ranges(design, &solved);
    design
        .tiers
        .iter()
        .enumerate()
        .filter(|(_, tier)| candidate::angle_is_variable(tier.angle_deg))
        .filter_map(|(index, _)| {
            let first_plane = ranges[index].start;
            mesh.rings
                .iter()
                .find(|(plane_index, _)| *plane_index == first_plane)
                .and_then(|(_, ring)| ring.first().copied())
                .map(|vertex| (index, vertex))
        })
        .collect()
}

/// The options that let every hinge-carrying tier of `design` vary.
pub(super) fn anchored_options(design: &Design) -> OptimizeOptions {
    OptimizeOptions {
        vary_anchored: true,
        anchor_hinges: hinge_points(design),
        ..OptimizeOptions::default()
    }
}

fn pinned_mast(design: &Design, index: usize) -> f64 {
    let MeetConstraint::ScaleReference(mast) = design.tiers[index].constraint else {
        panic!("tier {index} must be a ScaleReference tier");
    };
    mast
}

/// The first facet plane of tier `index` in the solid `design` bounds.
fn first_plane(design: &Design, index: usize) -> (DVec3, f64) {
    let solved = design.solve().expect("design must solve");
    let planes = design.planes_from_solved(&solved);
    let ranges = tier_plane_ranges(design, &solved);
    assert!(
        !ranges[index].is_empty(),
        "tier {index} must contribute a plane"
    );
    planes[ranges[index].start]
}

// --- the geometry ---

#[test]
fn the_fixture_gives_hinges_to_movable_tiers_only() {
    let design = imported_rbc();
    let hinges = hinge_points(&design);
    // RBC-445: tiers 0, 1, 4, 5, 6 (pavilion) and 7, 8, 9, 10 (crown) are movable; the
    // table (11) and the two -90 degree girdle facets (2, 3) never are. The later tests
    // lean on tiers 0 and 8.
    assert!(hinges.contains_key(&0) && hinges.contains_key(&8));
    for fixed in [2, 3, 11] {
        assert!(
            !hinges.contains_key(&fixed),
            "tier {fixed} must have no hinge"
        );
    }
}

/// With the angle left alone, a hinge that sits on the facet gives back the mast the
/// facet already has: the convention (side, azimuth, gear reference angle) matches the
/// planes the solid is really built from.
#[test]
fn a_hinge_on_the_plane_gives_back_the_tiers_own_mast() {
    let design = imported_rbc();
    let options = anchored_options(&design);
    let (space, free) = space_for(&design, &options);
    assert_ne!(free.len(), 0);
    for index in free {
        let mut rewritten = design.clone();
        assert!(
            space.write_angle(&mut rewritten, index, design.tiers[index].angle_deg),
            "tier {index}: the hinge must give a usable mast"
        );
        let mast = pinned_mast(&rewritten, index).abs();
        let expected = pinned_mast(&design, index).abs();
        assert!(
            (mast - expected).abs() < 1e-5,
            "tier {index}: hinge mast {mast} differs from the tier's mast {expected}"
        );
    }
}

/// The optimizer no longer carries a hinge formula of its own: the mast it writes for a
/// turned anchored tier is `design::mast_through` with the hinge and azimuth that
/// `design::tier_hinges` reports for that tier.
#[test]
fn the_anchored_mast_is_the_retarget_hinge_mast() {
    let design = imported_rbc();
    let solved = design.solve().expect("fixture must solve");
    let hinges = tier_hinges(&design, &solved);
    let (index, hinge) = hinges
        .iter()
        .find(|(index, _)| candidate::angle_is_variable(design.tiers[**index].angle_deg))
        .map(|(index, hinge)| (*index, *hinge))
        .expect("a movable tier has a hinge");
    let mut options = OptimizeOptions {
        vary_anchored: true,
        ..OptimizeOptions::default()
    };
    options.anchor_hinges.insert(index, hinge.point);
    let (space, free) = space_for(&design, &options);
    assert_eq!(free, vec![index]);

    // A smaller magnitude on the tier's own side is a turn.
    let angle_deg = design.tiers[index].angle_deg;
    let turned_deg = angle_deg.signum().mul_add(-1.5, angle_deg);
    let mut turned = design.clone();
    assert!(space.write_angle(&mut turned, index, turned_deg));

    let expected = mast_through(turned_deg, hinge.azimuth_rad, hinge.side, hinge.point);
    assert!(
        (pinned_mast(&turned, index) - expected).abs() < 1e-9,
        "optimizer mast {} differs from mast_through {expected}",
        pinned_mast(&turned, index)
    );
    assert!(
        (expected - pinned_mast(&design, index).abs()).abs() > 1e-4,
        "turning 1.5 degrees must move the mast"
    );
}

/// The point of the whole feature: turn a tier, and its first facet still passes
/// through the hinge, with the mast moved to make it so.
#[test]
fn turning_an_anchored_tier_keeps_its_first_facet_on_the_hinge() {
    let design = imported_rbc();
    let options = anchored_options(&design);
    let (space, free) = space_for(&design, &options);
    assert_ne!(free.len(), 0);

    for &index in &free {
        let hinge = options.anchor_hinges[&index];
        let original_mast = pinned_mast(&design, index);
        let turned_deg = design.tiers[index].angle_deg
            + if design.tiers[index].angle_deg > 0.0 {
                -1.5
            } else {
                1.5
            };
        let mut turned = design.clone();
        assert!(space.write_angle(&mut turned, index, turned_deg));
        assert_eq!(turned.tiers[index].angle_deg, turned_deg);

        let new_mast = pinned_mast(&turned, index);
        assert!(
            (new_mast - original_mast).abs() > 1e-4,
            "tier {index}: turning 1.5 degrees must move the mast ({original_mast} -> {new_mast})"
        );
        let (normal, offset) = first_plane(&turned, index);
        assert!(
            (normal.dot(hinge) - offset).abs() < 1e-5,
            "tier {index}: the turned facet must still pass through its hinge \
             (n.hinge {} vs offset {offset})",
            normal.dot(hinge)
        );
    }
}

#[test]
fn a_tier_without_a_hinge_keeps_its_mast_when_its_angle_is_written() {
    let design = rbc_445();
    // Tier 8 is meet-derived and has no hinge: write_angle sets the angle and nothing
    // else.
    let space = SearchSpace::default();
    let mut moved = design.clone();
    assert!(space.write_angle(&mut moved, 8, 27.5));
    assert_eq!(moved.tiers[8].angle_deg, 27.5);
    assert_eq!(moved.tiers[8].constraint, design.tiers[8].constraint);
}

#[test]
fn a_hinge_behind_the_plane_gives_no_mast() {
    let design = imported_rbc();
    // Tier 7 is a crown tier: a point far below the girdle is behind its plane.
    let mut options = OptimizeOptions {
        vary_anchored: true,
        ..OptimizeOptions::default()
    };
    options.anchor_hinges.insert(7, DVec3::new(0.0, -5.0, 0.0));
    let (space, free) = space_for(&design, &options);
    assert_eq!(free, vec![7]);
    let mut moved = design.clone();
    assert!(!space.write_angle(&mut moved, 7, 30.0));

    // A hinge that is not a finite point never frees the tier at all.
    options
        .anchor_hinges
        .insert(7, DVec3::new(f64::NAN, 0.0, 0.0));
    assert_eq!(free_tier_indices_with(&design, &options).len(), 0);
}

#[test]
fn a_cheater_offset_turns_the_normal_with_the_planes() {
    let mut design = imported_rbc();
    design.cheater_offsets_deg.insert(0, 2.0);
    let options = anchored_options(&design);
    assert!(
        options.anchor_hinges.contains_key(&0),
        "tier 0 keeps a facet after a small turn"
    );
    let (space, free) = space_for(&design, &options);
    assert!(free.contains(&0));
    let mut rewritten = design.clone();
    assert!(space.write_angle(&mut rewritten, 0, design.tiers[0].angle_deg));
    assert!(
        (pinned_mast(&rewritten, 0).abs() - pinned_mast(&design, 0).abs()).abs() < 1e-5,
        "the hinge of a rotated facet must give the facet's own mast back"
    );
}

// --- candidate building ---

#[test]
fn select_candidate_directions_recomputes_the_mast_of_an_anchored_tier() {
    let design = imported_rbc();
    let options = anchored_options(&design);
    let (space, _free) = space_for(&design, &options);
    let index = 0;
    let original_deg = design.tiers[index].angle_deg;

    let survivors =
        candidate::select_candidate_directions(&design, index, original_deg, 1.0, &space);
    assert_eq!(survivors.len(), 2, "a one degree step is safe both ways");
    let hinge = options.anchor_hinges[&index];
    for (deg, candidate_design) in &survivors {
        assert_eq!(candidate_design.tiers[index].angle_deg, *deg);
        let (normal, offset) = first_plane(candidate_design, index);
        assert!((normal.dot(hinge) - offset).abs() < 1e-5);
        assert!(
            (pinned_mast(candidate_design, index) - pinned_mast(&design, index)).abs() > 1e-4,
            "the mast must follow the angle"
        );
    }
}

#[test]
fn build_free_angle_candidate_moves_masts_with_angles_and_leaves_other_tiers_alone() {
    let design = imported_rbc();
    let mut options = anchored_options(&design);
    options
        .anchor_hinges
        .retain(|index, _| *index == 0 || *index == 8);
    let (space, free) = space_for(&design, &options);
    assert_eq!(free, vec![0, 8]);
    let reference: Vec<f64> = free.iter().map(|&i| design.tiers[i].angle_deg).collect();
    let angles: Vec<f64> = reference.iter().map(|&a| a + 0.5).collect();
    // Tier 0 is a pavilion tier (negative), tier 8 a crown tier: +0.5 stays on each side.
    let built = candidate::build_free_angle_candidate(&design, &free, &reference, &angles, &space)
        .expect("a half degree nudge is safe");
    for (index, tier) in design.tiers.iter().enumerate() {
        if free.contains(&index) {
            assert_ne!(built.tiers[index].constraint, tier.constraint);
        } else {
            assert_eq!(built.tiers[index], *tier, "tier {index} must be untouched");
        }
    }
}

// --- angle bounds ---

fn bounded_space(tier: usize, low: f64, high: f64) -> SearchSpace {
    let mut options = OptimizeOptions::default();
    options.angle_bounds.insert(tier, (low, high));
    SearchSpace::new(&options, BTreeMap::new())
}

#[test]
fn a_step_that_overshoots_the_bounds_is_clamped_to_the_edge() {
    let design = rbc_445();
    // Tier 8 ("B") sits at 29.0; +/-2 would reach 31.0 and 27.0, outside (28.5, 29.5).
    let space = bounded_space(8, 28.5, 29.5);
    let survivors = candidate::select_candidate_directions(&design, 8, 29.0, 2.0, &space);
    let mut degs: Vec<f64> = survivors.iter().map(|(deg, _)| *deg).collect();
    degs.sort_by(f64::total_cmp);
    assert_eq!(degs, vec![28.5, 29.5]);
}

#[test]
fn a_step_that_clamps_back_onto_the_tier_is_dropped() {
    let design = rbc_445();
    // The lower bound IS the current angle: the minus step clamps to where the tier
    // already is, so only the plus step is a move.
    let space = bounded_space(8, 29.0, 31.0);
    let survivors = candidate::select_candidate_directions(&design, 8, 29.0, 2.0, &space);
    let degs: Vec<f64> = survivors.iter().map(|(deg, _)| *deg).collect();
    assert_eq!(degs, vec![31.0]);
}

#[test]
fn two_steps_that_clamp_to_the_same_angle_are_one_candidate() {
    let design = rbc_445();
    // The tier is below the allowed range: both steps clamp to its lower edge.
    let space = bounded_space(8, 33.0, 35.0);
    let survivors = candidate::select_candidate_directions(&design, 8, 29.0, 2.0, &space);
    let degs: Vec<f64> = survivors.iter().map(|(deg, _)| *deg).collect();
    assert_eq!(degs, vec![33.0]);
}

#[test]
fn reversed_or_non_finite_bounds_are_read_sensibly() {
    let reversed = bounded_space(8, 29.5, 28.5);
    assert!(reversed.contains(8, 29.0));
    assert!(!reversed.contains(8, 30.0));
    let broken = bounded_space(8, f64::NAN, 29.5);
    assert!(broken.contains(8, 100.0), "a non-finite pair is ignored");
    assert_eq!(broken.clamp(8, 100.0), 100.0);
}

#[test]
fn a_polish_point_outside_the_bounds_is_rejected_not_clamped() {
    let design = rbc_445();
    let free = vec![8, 9];
    let mut options = OptimizeOptions::default();
    options.angle_bounds.insert(9, (27.9, 28.3));
    let space = SearchSpace::new(&options, BTreeMap::new());
    let reference = vec![29.0, 28.1];

    let inside =
        candidate::build_free_angle_candidate(&design, &free, &reference, &[29.2, 28.2], &space);
    assert!(inside.is_some());
    let outside =
        candidate::build_free_angle_candidate(&design, &free, &reference, &[29.2, 28.4], &space);
    assert!(outside.is_none(), "28.4 is past tier 9's upper bound");
}
