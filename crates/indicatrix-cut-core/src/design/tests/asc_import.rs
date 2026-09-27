//! Tests for how [`crate::design::Design::from_asc_schedule`] classifies a
//! tier's constraint depending on whether the source `.asc` text states an
//! explicit scale-reference instruction of its own.

use crate::{design::Design, preform::PreformSpec};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// Importing a real `.asc` schedule whose tiers already state explicit
/// scale-reference instructions must classify them as
/// `MeetConstraint::ScaleReference` directly (no synthesized anchor needed), and
/// re-solving must reproduce the original masts.
///
/// Every tier here is a stated anchor, so nothing is actually meet-*derived*
/// (`ScaleReference` copies its given value straight through `Design::solve` with no
/// geometry involved) -- see `discard_and_resolve_gate_on_real_fixtures` for a
/// version that actually exercises meet-derived tiers on real fixtures.
#[test]
fn importing_an_asc_schedule_with_stated_anchors_round_trips_masts() {
    let schedule = indicatrix_formats::asc::parse_asc(
        "GemCad 5.0\n\
         g 4 0.0\n\
         y 1 n\n\
         I 1.62\n\
         a 90.000000 1.00000000 0 1 2 3 G Set girdle thickness\n\
         a 0.000000 0.60000000 G Set stone size\n\
         a -0.000000 0.55000000 G Set stone size\n",
    )
    .expect("must parse");
    let design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    for tier in &design.tiers {
        assert!(
            matches!(tier.constraint, MeetConstraint::ScaleReference(_)),
            "every tier here states an explicit anchor instruction"
        );
        // A stated scale reference has nothing left to adopt -- `constraint`
        // already reflects it.
        assert_eq!(tier.imported_meet, None);
    }
    let solved = design.solve().expect("every block is anchored");
    for (solved, original) in solved.iter().zip(&schedule.tiers) {
        assert!((solved.mast - original.mast).abs() < 1e-9);
    }
}

/// Importing a schedule with NO stated anchor instruction at all must still
/// pin every tier to its own real recorded mast (see
/// `Design::from_asc_schedule`'s doc comment) -- not `apply_ratio_anchors`'
/// estimate, and not a silent default -- while stashing each tier's
/// classified `MeetExisting` in [`crate::design::ConstraintTier::imported_meet`] so the
/// editor can still show/offer what the file's geometry implies, even
/// though nothing here is actually meet-*derived* any more.
#[test]
fn importing_an_unanchored_asc_schedule_pins_every_tier_to_its_real_mast() {
    let schedule = indicatrix_formats::asc::parse_asc(
        "GemCad 5.0\n\
         g 4 0.0\n\
         y 1 n\n\
         I 1.62\n\
         a 90.000000 1.00000000 0 1 2 3\n\
         a 0.000000 0.60000000\n\
         a -0.000000 0.55000000\n",
    )
    .expect("must parse");
    let design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    // None of these tiers stated an anchor instruction, yet every one must
    // still be pinned to its own real recorded mast, with the file's
    // implicit "meet existing" classification preserved for one-click
    // adoption rather than silently lost.
    for (tier, original) in design.tiers.iter().zip(&schedule.tiers) {
        match tier.constraint {
            MeetConstraint::ScaleReference(v) => {
                assert!((v - original.mast).abs() < 1e-9);
            }
            ref other => panic!("expected a pinned ScaleReference, got {other:?}"),
        }
        assert_eq!(tier.imported_meet, Some(MeetConstraint::MeetExisting));
    }
    assert!(design.solve().is_ok());
}
