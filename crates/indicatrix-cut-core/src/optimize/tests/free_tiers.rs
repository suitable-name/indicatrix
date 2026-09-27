//! Tests for [`super::super::free_tier_indices`] and
//! [`super::super::candidate::candidate_angle_is_safe`] -- which tiers/angles the
//! search is ever allowed to propose.

use super::{
    super::{candidate, free_tier_indices},
    fixtures::{RBC_445, rbc_445},
};
use crate::{design::Design, preform::PreformSpec};
use indicatrix::geometry::meet_solver::MeetConstraint;

// --- free_tier_indices ---

#[test]
fn free_tier_indices_excludes_scale_reference_and_vertical_tiers() {
    let design = rbc_445();
    let free = free_tier_indices(&design);
    // RBC-445 has real prose G-instructions on two tiers ("G TCP"/"G PCP"), but
    // `meet_tier_inputs_from_asc` classifies those as MeetNamed/MeetExisting (see
    // that function's own doc comment: it is not a stated scale dimension) -- the
    // ONLY ScaleReference tiers are the ones this fixture's own bootstrap loop
    // synthesizes, one per populated block. It also carries two real girdle facets
    // at -90.0, which `free_tier_indices` excludes because no candidate angle for
    // them could ever pass `candidate_angle_is_safe` (see that function's own doc
    // comment).
    for (i, tier) in design.tiers.iter().enumerate() {
        let pinned = matches!(tier.constraint, MeetConstraint::ScaleReference(_));
        let vertical = tier.angle_deg.abs() > candidate::MAX_SAFE_CANDIDATE_ANGLE_DEG;
        assert_eq!(
            free.contains(&i),
            !pinned && !vertical,
            "tier {i} ({} degrees, pinned {pinned}, vertical {vertical}) is on the wrong side \
             of the free/not-free split",
            tier.angle_deg
        );
    }
    assert!(
        !free.is_empty(),
        "the fixture must leave something for the optimizer to move"
    );
}

#[test]
fn a_freshly_imported_design_has_no_free_tiers() {
    let schedule = indicatrix_formats::asc::parse_asc(RBC_445).expect("fixture must parse");
    let imported = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    assert_eq!(free_tier_indices(&imported), Vec::<usize>::new());
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

#[test]
fn a_tier_authored_at_exactly_zero_may_move_either_direction() {
    assert!(candidate::candidate_angle_is_safe(0.0, 5.0));
    assert!(candidate::candidate_angle_is_safe(0.0, -5.0));
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
