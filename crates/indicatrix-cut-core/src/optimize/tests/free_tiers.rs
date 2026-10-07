//! Tests for [`super::super::free_tier_indices`] and
//! [`super::super::candidate::candidate_angle_is_safe`] -- which tiers/angles the
//! search is ever allowed to propose.

use super::{
    super::{OptimizeOptions, candidate, free_tier_indices, free_tier_indices_with},
    fixtures::{imported_rbc, rbc_445},
};
use glam::DVec3;
use indicatrix::geometry::meet_solver::MeetConstraint;

// --- free_tier_indices ---

#[test]
fn free_tier_indices_excludes_scale_reference_horizontal_and_vertical_tiers() {
    let design = rbc_445();
    let free = free_tier_indices(&design);
    // RBC-445 has real prose G-instructions on two tiers ("G TCP"/"G PCP"), but
    // `meet_tier_inputs_from_asc` classifies those as MeetNamed/MeetExisting (see
    // that function's own doc comment: it is not a stated scale dimension) -- the
    // ONLY ScaleReference tiers are the ones this fixture's own bootstrap loop
    // synthesizes, one per populated block. It also carries two real girdle facets
    // at -90.0, which `free_tier_indices` excludes because no candidate angle for
    // them could ever pass `candidate_angle_is_safe` (see that function's own doc
    // comment), and a table at +0.0 ("E"), which has no angle to optimize.
    for (i, tier) in design.tiers.iter().enumerate() {
        let pinned = matches!(tier.constraint, MeetConstraint::ScaleReference(_));
        let vertical = tier.angle_deg.abs() >= candidate::MAX_SAFE_CANDIDATE_ANGLE_DEG;
        let horizontal = tier.angle_deg.abs() < candidate::MIN_VARIABLE_ANGLE_DEG;
        assert_eq!(
            free.contains(&i),
            !pinned && !vertical && !horizontal,
            "tier {i} ({} degrees, pinned {pinned}, vertical {vertical}, horizontal \
             {horizontal}) is on the wrong side of the free/not-free split",
            tier.angle_deg
        );
    }
    assert!(
        !free.is_empty(),
        "the fixture must leave something for the optimizer to move"
    );
    assert!(
        design
            .tiers
            .iter()
            .any(|tier| tier.angle_deg == 0.0 && !tier.angle_deg.is_sign_negative()),
        "the fixture must carry a +0.0 table for this test to mean anything"
    );
}

#[test]
fn a_table_or_a_culet_is_never_free_even_when_it_meets_something() {
    let mut design = rbc_445();
    // Tier 11 is the +0.0 table "E" (a meet-derived tier here); tier 1 is an ordinary
    // meet-derived pavilion tier, free until it is turned into a culet by giving it
    // the sign-negative zero that marks one.
    assert_eq!(design.tiers[11].angle_deg.to_bits(), 0.0f64.to_bits());
    assert!(free_tier_indices(&design).contains(&1));
    design.tiers[1].angle_deg = -0.0;
    let free = free_tier_indices(&design);
    assert!(!free.contains(&11), "the table must not be free");
    assert!(!free.contains(&1), "the culet must not be free");
}

#[test]
fn a_tier_at_exactly_the_vertical_bound_is_not_free_and_just_inside_it_is() {
    let mut design = rbc_445();
    // Tier 8 ("B") is a meet-derived crown tier.
    assert!(free_tier_indices(&design).contains(&8));
    design.tiers[8].angle_deg = candidate::MAX_SAFE_CANDIDATE_ANGLE_DEG;
    assert!(!free_tier_indices(&design).contains(&8));
    design.tiers[8].angle_deg = 89.4;
    assert!(free_tier_indices(&design).contains(&8));
}

#[test]
fn a_freshly_imported_design_has_no_free_tiers() {
    let imported = imported_rbc();
    assert_eq!(free_tier_indices(&imported), Vec::<usize>::new());
}

#[test]
fn the_default_options_ask_for_the_same_free_tiers_as_free_tier_indices() {
    let design = rbc_445();
    assert_eq!(
        free_tier_indices_with(&design, &OptimizeOptions::default()),
        free_tier_indices(&design)
    );
}

// --- anchored tiers join only on request ---

#[test]
fn an_anchored_tier_joins_the_free_set_only_with_vary_anchored_and_a_hinge() {
    let imported = imported_rbc();
    let hinge = DVec3::new(0.5, -0.3, 0.2);
    let mut options = OptimizeOptions::default();
    options.anchor_hinges.insert(0, hinge);

    // A hinge alone does nothing: the request has to opt in.
    assert_eq!(free_tier_indices_with(&imported, &options).len(), 0);

    options.vary_anchored = true;
    assert_eq!(free_tier_indices_with(&imported, &options), vec![0]);

    // Opting in without a hinge for a tier leaves that tier pinned.
    options.anchor_hinges.clear();
    assert_eq!(free_tier_indices_with(&imported, &options).len(), 0);
}

#[test]
fn a_hinge_never_frees_a_horizontal_or_vertical_tier_or_a_non_finite_hinge() {
    let imported = imported_rbc();
    let mut options = OptimizeOptions {
        vary_anchored: true,
        ..OptimizeOptions::default()
    };
    // Tier 2 is a -90 degree girdle facet, tier 11 the +0.0 table, tier 0 an ordinary
    // pavilion main.
    let hinge = DVec3::new(0.5, -0.3, 0.2);
    options.anchor_hinges.insert(2, hinge);
    options.anchor_hinges.insert(11, hinge);
    options
        .anchor_hinges
        .insert(0, DVec3::new(f64::NAN, 0.0, 0.0));
    assert_eq!(free_tier_indices_with(&imported, &options).len(), 0);
}

// --- candidate_angle_is_safe ---

#[test]
fn crossing_zero_from_a_nonzero_angle_is_unsafe() {
    assert!(!candidate::candidate_angle_is_safe(-43.0, 0.5));
    assert!(!candidate::candidate_angle_is_safe(30.0, -0.5));
}

#[test]
fn staying_on_the_same_side_of_zero_is_safe() {
    assert!(candidate::candidate_angle_is_safe(-43.0, -44.0));
    assert!(candidate::candidate_angle_is_safe(30.0, 32.0));
}

/// A `+0.0` origin (a table) used to be free to step to either side. It is not any
/// more: like every other tier it keeps its side, here the crown side.
#[test]
fn a_tier_authored_at_positive_zero_stays_on_the_crown_side() {
    assert!(candidate::candidate_angle_is_safe(0.0, 5.0));
    assert!(!candidate::candidate_angle_is_safe(0.0, -5.0));
    assert!(!candidate::candidate_angle_is_safe(0.0, 0.0));
}

/// `-0.0` is the crate's own pavilion-culet marker (see
/// `ConstraintTier::standard_round_brilliant`'s culet), distinct from the side-less
/// `+0.0` a table facet starts at. A `-0.0` origin must stay pinned to the negative
/// side like any other negative angle.
#[test]
fn a_tier_authored_at_negative_zero_stays_pinned_to_the_negative_side() {
    assert!(candidate::candidate_angle_is_safe(-0.0, -2.0));
    assert!(!candidate::candidate_angle_is_safe(-0.0, 2.0));
}

/// A candidate must never land exactly on the girdle plane, from either side.
/// The current check uses `signum` to distinguish between `0.0` and `-0.0`,
/// ensuring that a step landing at zero is always rejected as required.
#[test]
fn a_candidate_landing_exactly_on_zero_is_never_safe() {
    assert!(!candidate::candidate_angle_is_safe(5.0, 0.0));
    assert!(!candidate::candidate_angle_is_safe(-5.0, -0.0));
}

#[test]
fn a_non_finite_candidate_is_never_safe() {
    assert!(!candidate::candidate_angle_is_safe(10.0, f64::NAN));
    assert!(!candidate::candidate_angle_is_safe(10.0, f64::INFINITY));
    assert!(!candidate::candidate_angle_is_safe(
        -10.0,
        f64::NEG_INFINITY
    ));
}

#[test]
fn a_candidate_past_the_vertical_bound_is_unsafe_even_on_the_same_side() {
    // Same sign as the original in both cases -- only the magnitude bound (the
    // near-girdle vertical boundary, not the zero-crossing) should reject these.
    assert!(!candidate::candidate_angle_is_safe(85.0, 89.9));
    assert!(!candidate::candidate_angle_is_safe(-85.0, -89.9));
}

#[test]
fn a_candidate_just_inside_the_vertical_bound_is_safe() {
    assert!(candidate::candidate_angle_is_safe(85.0, 89.0));
    assert!(candidate::candidate_angle_is_safe(-85.0, -89.0));
}
