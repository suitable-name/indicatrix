//! [`crate::edit::History::apply_coalescing`]: merging consecutive
//! same-key calls within a time window into one undo step, and the ways that
//! merge is NOT allowed to happen (window elapsed, different key, an ordinary
//! `apply` in between, or an explicit `end_coalesce_run`).

use super::fixtures::{fresh_design, tier};
use crate::edit::{Edit, History};
use indicatrix::geometry::meet_solver::MeetConstraint;
use std::time::{Duration, Instant};

#[test]
fn apply_coalescing_merges_consecutive_calls_with_the_same_key_into_one_undo_step() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let before = design.clone();
    let mut history = History::new();
    let t0 = Instant::now();

    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.1, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0,
        )
        .expect("first nudge must apply");
    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.2, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0 + Duration::from_millis(100),
        )
        .expect("second nudge must apply");
    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.3, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0 + Duration::from_millis(200),
        )
        .expect("third nudge must apply");
    assert_eq!(design.tiers[0].angle_deg, -40.3);

    // Three coalesced calls, one undo step: this must revert all the way back to
    // BEFORE the first nudge, not just undo the third.
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, before);
    assert!(!history.can_undo());
}

#[test]
fn apply_coalescing_starts_a_new_undo_step_once_the_window_elapses() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let mut history = History::new();
    let t0 = Instant::now();

    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.1, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0,
        )
        .expect("first nudge must apply");
    // Past the 500ms window -- must NOT merge with the call above.
    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.2, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0 + Duration::from_millis(600),
        )
        .expect("second nudge must apply");

    // Two separate undo steps: the first undo only reverts the second nudge.
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[0].angle_deg, -40.1);
    assert!(history.can_undo());
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[0].angle_deg, -40.0);
    assert!(!history.can_undo());
}

#[test]
fn apply_coalescing_with_a_different_key_starts_a_new_undo_step() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    design
        .tiers
        .push(tier("P2", -35.0, MeetConstraint::ScaleReference(0.6), &[]));
    let mut history = History::new();
    let t0 = Instant::now();

    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.1, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0,
        )
        .expect("first tier's nudge must apply");
    // A different tier's nudge (different key), well within the window -- must still
    // get its own undo step rather than merging with tier 0's.
    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 1,
                tier: tier("P2", -35.1, MeetConstraint::ScaleReference(0.6), &[]),
            },
            2,
            t0 + Duration::from_millis(50),
        )
        .expect("second tier's nudge must apply");

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[1].angle_deg, -35.0);
    assert_eq!(
        design.tiers[0].angle_deg, -40.1,
        "tier 0's nudge must still stand"
    );
    assert!(history.can_undo());
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[0].angle_deg, -40.0);
}

#[test]
fn an_ordinary_apply_between_two_coalescing_calls_breaks_the_merge() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let mut history = History::new();
    let t0 = Instant::now();

    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.1, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0,
        )
        .expect("nudge must apply");
    // An ordinary apply (e.g. a Save Tier click) in between, same design/instant.
    history
        .apply(
            &mut design,
            Edit::SetGirdleDiameterMm {
                girdle_diameter_mm: Some(8.0),
            },
        )
        .expect("ordinary apply must succeed");
    // A same-key coalescing call right after must NOT merge across the apply above.
    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.2, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0 + Duration::from_millis(10),
        )
        .expect("nudge after an ordinary apply must apply");

    // Three real undo steps: the second nudge, the girdle-diameter change, then the
    // first nudge -- none of them merged together.
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[0].angle_deg, -40.1);
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.girdle_diameter_mm, None);
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[0].angle_deg, -40.0);
    assert!(!history.can_undo());
}

/// [`crate::edit::History::with_coalesce_window`] lets a caller pick a
/// window other than the crate-wide 500ms default -- here, one short enough that
/// two calls 100ms apart (which the default-window test above merges) do NOT
/// merge.
#[test]
fn with_coalesce_window_uses_the_given_window_instead_of_the_default() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let mut history = History::with_coalesce_window(Duration::from_millis(50));
    let t0 = Instant::now();

    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.1, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0,
        )
        .expect("first nudge must apply");
    // 100ms > this history's own 50ms window -- must NOT merge.
    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.2, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0 + Duration::from_millis(100),
        )
        .expect("second nudge must apply");

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(
        design.tiers[0].angle_deg, -40.1,
        "only the second nudge undoes"
    );
    assert!(history.can_undo());
}

/// [`crate::edit::History::end_coalesce_run`] ends a coalescing run
/// explicitly, so a call with the SAME key that would otherwise merge (well
/// inside the window) starts a fresh undo step instead once a caller has named
/// its own interaction boundary (e.g. pointer release).
#[test]
fn end_coalesce_run_stops_the_next_same_key_call_from_merging() {
    let mut design = fresh_design();
    design
        .tiers
        .push(tier("P1", -40.0, MeetConstraint::ScaleReference(0.5), &[]));
    let mut history = History::new();
    let t0 = Instant::now();

    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.1, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0,
        )
        .expect("first nudge must apply");
    history.end_coalesce_run();
    // Same key, well inside the 500ms window -- would merge without the explicit
    // end_coalesce_run above.
    history
        .apply_coalescing(
            &mut design,
            Edit::ModifyTier {
                index: 0,
                tier: tier("P1", -40.2, MeetConstraint::ScaleReference(0.5), &[]),
            },
            1,
            t0 + Duration::from_millis(10),
        )
        .expect("second nudge must apply");

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(
        design.tiers[0].angle_deg, -40.1,
        "only the second nudge undoes"
    );
    assert!(history.can_undo());
}
