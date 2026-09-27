//! Unit tests for the scheduling decisions, the sequence-number epoch, the
//! design/generation stash, and the cancellable-solve latency guarantee.

use super::{
    dispatch::{cancel_in_flight_solve, is_current, solve_cancellably, solving_banner},
    replan::{stash_current_design, take_matching_design},
    runtime::{PendingDispatch, RUNTIME},
    scheduling::{
        auto_solve_off_note, last_measured_solve_duration, last_solve, record_solve_duration,
        reset_for_new_design, should_schedule_auto_solve, should_solve_synchronously,
    },
};
use indicatrix::geometry::meet_solver::SolveError;
use indicatrix_cut_core::{Design, DesignSolveError};
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

// --- should_schedule_auto_solve ---

#[test]
fn zero_budget_disables_auto_solve_regardless_of_measurement() {
    assert!(!should_schedule_auto_solve(None, Duration::ZERO));
    assert!(!should_schedule_auto_solve(
        Some(Duration::from_millis(1)),
        Duration::ZERO
    ));
}

#[test]
fn no_measurement_yet_is_scheduled_optimistically() {
    // A fresh design (or one just New/Loaded) has no measurement -- see this
    // function's own doc comment for why that is treated as "try it," not
    // "refuse until proven cheap."
    assert!(should_schedule_auto_solve(None, Duration::from_millis(300)));
}

#[test]
fn a_measurement_under_budget_is_scheduled() {
    assert!(should_schedule_auto_solve(
        Some(Duration::from_millis(120)),
        Duration::from_millis(300)
    ));
}

#[test]
fn a_measurement_at_or_over_budget_is_not_scheduled() {
    assert!(!should_schedule_auto_solve(
        Some(Duration::from_millis(300)),
        Duration::from_millis(300)
    ));
    assert!(!should_schedule_auto_solve(
        Some(Duration::from_secs(6)),
        Duration::from_millis(300)
    ));
}

// --- should_solve_synchronously / last_measured_solve_duration ---

#[test]
fn a_fresh_design_with_few_planes_solves_synchronously() {
    assert!(should_solve_synchronously(4, None));
}

#[test]
fn a_fresh_design_with_many_planes_does_not_solve_synchronously() {
    assert!(!should_solve_synchronously(210, None));
}

#[test]
fn a_real_fast_measurement_wins_over_a_high_plane_count() {
    // A design can have many planes yet still measure fast (or vice versa) --
    // once a real measurement exists it alone decides, exactly like
    // `should_schedule_auto_solve`.
    assert!(should_solve_synchronously(
        210,
        Some(Duration::from_millis(50))
    ));
}

#[test]
fn a_real_slow_measurement_loses_even_with_few_planes() {
    assert!(!should_solve_synchronously(4, Some(Duration::from_secs(2))));
}

#[test]
fn last_measured_solve_duration_reflects_record_solve_duration() {
    reset_for_new_design();
    assert_eq!(last_measured_solve_duration(), None);
    record_solve_duration(Duration::from_millis(77));
    assert_eq!(
        last_measured_solve_duration(),
        Some(Duration::from_millis(77))
    );
    reset_for_new_design();
    assert_eq!(last_measured_solve_duration(), None);
}

#[test]
fn last_solve_is_the_production_counterpart_of_last_measured_solve_duration() {
    // `view::refresh_all` reads this getter for
    // every non-wholesale caller -- it must agree with the test-only window
    // onto the same `Runtime` field at every point in the same sequence.
    reset_for_new_design();
    assert_eq!(last_solve(), None);
    record_solve_duration(Duration::from_millis(123));
    assert_eq!(last_solve(), Some(Duration::from_millis(123)));
    assert_eq!(last_solve(), last_measured_solve_duration());
}

// --- is_current / the sequence-number epoch ---

#[test]
fn a_freshly_recorded_sequence_number_is_current() {
    let seq = RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.current_seq += 1;
        rt.current_seq
    });
    assert!(is_current(seq));
}

#[test]
fn an_older_sequence_number_is_superseded_by_a_newer_dispatch() {
    let old_seq = RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.current_seq += 1;
        rt.current_seq
    });
    // A second dispatch bumps the counter again, exactly like
    // `dispatch_background_solve` does on every call.
    RUNTIME.with(|cell| cell.borrow_mut().current_seq += 1);
    assert!(!is_current(old_seq));
}

// --- cancel_in_flight_solve ---

#[test]
fn cancel_in_flight_solve_supersedes_whatever_sequence_was_current() {
    let seq = RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.current_seq += 1;
        rt.current_seq
    });
    assert!(is_current(seq));
    cancel_in_flight_solve();
    assert!(
        !is_current(seq),
        "cancelling must supersede whatever solve was in flight, so its \
         eventual completion is dropped as stale"
    );
}

#[test]
fn cancel_in_flight_solve_drops_a_queued_pending_dispatch() {
    RUNTIME.with(|cell| {
        cell.borrow_mut().pending_dispatch = Some(PendingDispatch {
            design: fixture_design(),
            generation: Arc::new(AtomicU64::new(0)),
            multi_selected: BTreeSet::new(),
        });
    });
    cancel_in_flight_solve();
    RUNTIME.with(|cell| {
        assert!(
            cell.borrow().pending_dispatch.is_none(),
            "a cancel must drop whatever dispatch was queued behind the \
             cancelled solve, not replay it once the in-flight worker \
             eventually frees the slot"
        );
    });
}

#[test]
fn cancel_in_flight_solve_drops_the_pending_debounce() {
    RUNTIME.with(|cell| cell.borrow_mut().debounce = Some(slint::Timer::default()));
    cancel_in_flight_solve();
    RUNTIME.with(|cell| {
        assert!(
            cell.borrow().debounce.is_none(),
            "a cancel must also drop whatever debounced auto-solve was pending"
        );
    });
}

// --- idle_replan ---

#[test]
fn reset_for_new_design_drops_the_idle_replan_timer() {
    RUNTIME.with(|cell| cell.borrow_mut().idle_replan = Some(slint::Timer::default()));
    reset_for_new_design();
    RUNTIME.with(|cell| {
        assert!(
            cell.borrow().idle_replan.is_none(),
            "a wholesale New/Load must drop a partial-frame idle-replan timer \
             armed for whatever design it is replacing -- otherwise that timer \
             can fire later and resubmit the OLD design's stashed masts on top \
             of the one just loaded"
        );
    });
}

#[test]
fn cancel_in_flight_solve_drops_the_idle_replan_timer() {
    RUNTIME.with(|cell| cell.borrow_mut().idle_replan = Some(slint::Timer::default()));
    cancel_in_flight_solve();
    RUNTIME.with(|cell| {
        assert!(
            cell.borrow().idle_replan.is_none(),
            "cancelling a solve must also drop whatever idle-replan timer was pending"
        );
    });
}

#[test]
fn cancel_in_flight_solve_leaves_the_measurement_and_design_stash_untouched() {
    reset_for_new_design();
    record_solve_duration(Duration::from_millis(250));
    stash_current_design(3, Arc::new(fixture_design()), BTreeSet::new());
    cancel_in_flight_solve();
    assert_eq!(
        last_measured_solve_duration(),
        Some(Duration::from_millis(250)),
        "cancelling an in-flight solve must not discard this design's own \
         last REAL measurement -- unlike reset_for_new_design, no different \
         design has just replaced it"
    );
    assert!(
        take_matching_design(3).is_some(),
        "cancelling an in-flight solve must not clear an unrelated stash \
         from a concurrent edit's own submit_preview_replan"
    );
}

// --- record_solve_duration / reset_for_new_design ---

#[test]
fn record_solve_duration_is_visible_to_the_next_schedule_decision() {
    record_solve_duration(Duration::from_millis(42));
    let last = RUNTIME.with(|cell| cell.borrow().last_solve);
    assert_eq!(last, Some(Duration::from_millis(42)));
    reset_for_new_design();
    let last = RUNTIME.with(|cell| cell.borrow().last_solve);
    assert_eq!(last, None);
}

// --- banner text ---

#[test]
fn solving_banner_pluralizes_the_tier_count() {
    assert!(solving_banner(1, Duration::ZERO).contains("1 tier)"));
    assert!(solving_banner(2, Duration::ZERO).contains("2 tiers)"));
}

#[test]
fn auto_solve_off_note_reports_the_measured_seconds() {
    let text = auto_solve_off_note(Duration::from_millis(5900));
    assert!(text.contains("5.9s"));
}

// --- stash_current_design / take_matching_design ---

/// A minimal, cheap-to-build fixture -- content never matters to these tests,
/// only that a `Design` value round-trips through the stash unchanged.
fn fixture_design() -> Design {
    Design::fresh(
        indicatrix_cut_core::PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        96,
        8,
        1.54,
    )
}

#[test]
fn take_matching_design_returns_none_when_nothing_stashed() {
    // `reset_for_new_design` (not just letting the field default) since this
    // module's `RUNTIME` is a `thread_local!` a prior test on the same pooled
    // test thread may have already stashed into.
    reset_for_new_design();
    assert!(take_matching_design(1).is_none());
}

#[test]
fn take_matching_design_returns_the_stash_for_its_own_generation() {
    stash_current_design(5, Arc::new(fixture_design()), BTreeSet::from([2]));
    let (design, multi_selected) = take_matching_design(5).expect("stashed at generation 5");
    assert!(
        design.tiers.is_empty(),
        "a fresh design starts with no tiers"
    );
    assert_eq!(multi_selected, BTreeSet::from([2]));
}

#[test]
fn take_matching_design_only_matches_once_per_stash() {
    // A camera-drag `Reproject` frame carries the SAME generation/masts
    // forward as the `Replan` that produced them (see
    // `solid_preview::preview_state::WorkerMemory::generation`'s own doc
    // comment) -- so a second call for the identical generation, with no
    // new `stash_current_design` in between, must find nothing left to
    // give, or every orbit frame after an edit would re-trigger a full
    // tier-table rebuild.
    stash_current_design(11, Arc::new(fixture_design()), BTreeSet::new());
    assert!(
        take_matching_design(11).is_some(),
        "the first call must match"
    );
    assert!(
        take_matching_design(11).is_none(),
        "a second call for the same generation must not match again"
    );
}

#[test]
fn take_matching_design_rejects_a_superseded_generation() {
    stash_current_design(5, Arc::new(fixture_design()), BTreeSet::new());
    assert!(
        take_matching_design(6).is_none(),
        "a newer generation must not match an older stash -- an edit landed \
         after the frame asking for generation 6 was submitted"
    );
}

#[test]
fn stash_current_design_shares_the_arc_rather_than_deep_cloning() {
    // `stash_current_design`
    // must accept the CALLER's own `Arc<Design>` snapshot (an `Arc::clone`,
    // cheap) rather than taking ownership of a value the caller had to deep-
    // clone again just to hand over -- `Arc::strong_count` rising by exactly
    // one confirms no hidden deep clone happened inside this function.
    let snapshot = Arc::new(fixture_design());
    assert_eq!(Arc::strong_count(&snapshot), 1);
    stash_current_design(7, Arc::clone(&snapshot), BTreeSet::new());
    assert_eq!(
        Arc::strong_count(&snapshot),
        2,
        "the stash must hold a SHARED clone of the same allocation, not a deep copy"
    );
    let (taken, _) = take_matching_design(7).expect("stashed at generation 7");
    assert!(
        Arc::ptr_eq(&snapshot, &taken),
        "take_matching_design must hand back the SAME allocation stash_current_design \
         was given, never a fresh deep clone"
    );
}

// --- solve_cancellably / "Abandon Solve" actually stops the worker ---

/// "PC 05.115 CrackOtto-Step" by Ottorino Invernizzi, 103 tiers -- the same
/// real fixture `indicatrix-cut-core`'s own `resolve_dirty_speed_on_a_large_real_design`
/// test measures a 5.9s full solve against. Read from the crate's own fixture
/// file (not re-embedded) so the two copies can never drift -- see
/// `solve_service.rs`'s own identical helper for the full provenance comment.
const CRACKOTTO_STEP_ASC: &str = include_str!(
    "../../../../../../crates/indicatrix-cut-core/src/optimize_cost_probe_crackotto_step.asc"
);

/// Rebuilds the "mostly implicit `MeetExisting`, one `ScaleReference` anchor
/// per block" structure a meaningful cancellation test needs -- see
/// `solve_service.rs`'s own `crackotto_step_with_real_meet_structure` for why a
/// plain `Design::from_asc_schedule` (every tier pinned) solves too fast to
/// prove anything about mid-solve cancellation.
fn crackotto_step_with_real_meet_structure() -> Design {
    use indicatrix::geometry::meet_solver::{Block, classify_blocks};

    let schedule =
        indicatrix_formats::asc::parse_asc(CRACKOTTO_STEP_ASC).expect("fixture must parse");
    let mut design = Design::from_asc_schedule(
        indicatrix_cut_core::PreformSpec::block(2.0, 1.0, 2.0),
        &schedule,
    );
    let inputs = design.meet_tier_inputs();
    let blocks = classify_blocks(&inputs);
    let mut anchor_kept = [false; 3];
    let slot = |b: Block| match b {
        Block::Crown => 0,
        Block::Pavilion => 1,
        Block::Girdle => 2,
    };
    for (i, tier) in design.tiers.iter_mut().enumerate() {
        let s = slot(blocks[i]);
        if !anchor_kept[s] {
            anchor_kept[s] = true;
            continue;
        }
        if let Some(original) = tier.imported_meet.clone() {
            tier.constraint = original;
        }
    }
    design
}

#[test]
fn solve_cancellably_stops_within_100ms_on_the_largest_real_fixture() {
    // This test encodes the "Abandon
    // stops the worker (test: cancel flag set -> worker returns within
    // 100 ms)" acceptance criterion. `dispatch_background_solve`'s worker calls exactly this
    // function; `cancel_in_flight_solve` is what flips the flag it reads.
    let design = crackotto_step_with_real_meet_structure();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_thread = Arc::clone(&cancel);
    let worker = thread::spawn(move || solve_cancellably(&design, &cancel_for_thread));

    // Let the solve get well past the cheap missing-anchor check and into real
    // refinement work before asking it to stop.
    thread::sleep(Duration::from_millis(50));
    let cancel_requested_at = Instant::now();
    cancel.store(true, Ordering::Relaxed);
    let result = worker.join().expect("worker thread must not panic");
    let cancel_latency = cancel_requested_at.elapsed();

    assert!(
        cancel_latency < Duration::from_millis(100),
        "cancel took {cancel_latency:?}, want < 100ms (full uncancelled solve is ~5.9s)"
    );
    assert!(
        matches!(result, Err(DesignSolveError::Solve(SolveError::Cancelled))),
        "expected a cancelled solve"
    );
}

#[test]
fn take_matching_design_drops_the_pending_debounce_on_a_match() {
    stash_current_design(9, Arc::new(fixture_design()), BTreeSet::new());
    RUNTIME.with(|cell| cell.borrow_mut().debounce = Some(slint::Timer::default()));
    assert!(take_matching_design(9).is_some());
    RUNTIME.with(|cell| {
        assert!(
            cell.borrow().debounce.is_none(),
            "a matching frame must cancel whatever debounced auto-solve was pending"
        );
    });
}
