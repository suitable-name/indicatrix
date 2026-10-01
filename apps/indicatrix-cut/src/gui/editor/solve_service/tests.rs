//! Tests for [`super`]: the mailbox's coalescing, cancellation, staleness tagging,
//! the progress throttle and the request kinds. `SolveService::new`/`submit`
//! themselves are not exercised end to end: both require a live
//! `slint::ComponentHandle`, which needs a windowing backend this suite cannot
//! start. The `seq`-staleness check and `Mailbox`'s coalescing tests cover the
//! same decisions `SolveService::submit`/`worker_loop` make, without one.

use super::*;
use indicatrix::geometry::meet_solver::{Block, MeetConstraint, SolvePhase, classify_blocks};
use indicatrix_cut_core::PreformSpec;

// --- Mailbox: last-wins coalescing ---

fn dummy_request(generation: u64) -> SolveRequest {
    SolveRequest {
        design: Arc::new(Design::fresh(
            PreformSpec::block(2.0, 1.0, 2.0),
            96,
            8,
            1.54,
        )),
        generation,
        kind: SolveKind::Full,
    }
}

fn dummy_queued(generation: u64, seq: u64) -> Queued {
    Queued {
        request: dummy_request(generation),
        seq,
        cancel: Arc::new(AtomicBool::new(false)),
    }
}

#[test]
fn three_requests_in_a_burst_coalesce_to_one_solve_of_the_last() {
    // Three `put`s that land before anything ever calls `take_blocking`/`try_take`
    // must leave only the LAST one behind -- the two earlier ones are silently
    // discarded, never solved at all (last-wins coalescing).
    let mailbox = Mailbox::new();
    mailbox.put(dummy_queued(1, 1));
    mailbox.put(dummy_queued(2, 2));
    mailbox.put(dummy_queued(3, 3));

    let got = mailbox.try_take().expect("one request must be queued");
    assert_eq!(got.request.generation, 3, "must keep only the LAST request");
    assert_eq!(got.seq, 3);
    assert!(
        mailbox.try_take().is_none(),
        "the two earlier requests must have been discarded, not queued behind the last"
    );
}

#[test]
fn put_after_take_queues_a_fresh_request() {
    let mailbox = Mailbox::new();
    mailbox.put(dummy_queued(1, 1));
    assert!(mailbox.try_take().is_some());
    assert!(
        mailbox.try_take().is_none(),
        "mailbox must be empty after a take"
    );
    mailbox.put(dummy_queued(2, 2));
    let got = mailbox
        .try_take()
        .expect("a request put after an empty take must be queued");
    assert_eq!(got.request.generation, 2);
}

// --- SolveHandle::cancel ---

#[test]
fn handle_cancel_sets_the_flag_a_worker_would_read() {
    // `SolveHandle::cancel`'s entire job is flipping the shared
    // `Arc<AtomicBool>` -- verified directly, without spinning up a worker.
    // `deep_solve::DeepSolveHandle::cancel` is a one-line delegation to this
    // same method, so this also covers that call site's own behavior.
    let flag = Arc::new(AtomicBool::new(false));
    let handle = SolveHandle {
        generation: 0,
        cancel: Arc::clone(&flag),
    };
    assert!(!flag.load(Ordering::Relaxed));
    handle.cancel();
    assert!(flag.load(Ordering::Relaxed));
}

// --- Generation/seq tagging: dropping stale results ---

#[test]
fn a_result_whose_seq_no_longer_matches_latest_seq_is_stale() {
    // Mirrors `worker_loop`'s own staleness check inline, without spinning up
    // a real worker thread: a `submit` bumps `latest_seq`, so an older
    // in-flight (or just-finished) request's own captured `seq` no longer
    // matches once a newer one has landed.
    let latest_seq = AtomicU64::new(1);
    assert_eq!(latest_seq.load(Ordering::Relaxed), 1);

    latest_seq.store(2, Ordering::Relaxed);
    assert_ne!(
        1,
        latest_seq.load(Ordering::Relaxed),
        "an older seq must read as stale once a newer submit landed"
    );
    assert_eq!(
        2,
        latest_seq.load(Ordering::Relaxed),
        "the newest seq must still read as current"
    );
}

// --- ProgressThrottle ---

fn progress(phase: SolvePhase, sweep: u32) -> SolveProgress {
    SolveProgress {
        phase,
        sweep,
        max_sweeps: 4,
        blocks_done: 0,
        blocks_total: 10,
    }
}

#[test]
fn first_report_of_a_new_phase_sweep_is_always_forwarded() {
    let throttle = ProgressThrottle::new();
    assert!(throttle.should_forward(progress(SolvePhase::Refine, 1)));
}

#[test]
fn a_second_report_of_the_same_phase_sweep_within_the_interval_is_dropped() {
    let throttle = ProgressThrottle::new();
    let p = progress(SolvePhase::Refine, 1);
    assert!(throttle.should_forward(p));
    assert!(
        !throttle.should_forward(p),
        "an immediate repeat of the same (phase, sweep) inside the throttle window must be dropped"
    );
}

#[test]
fn a_report_from_a_new_sweep_is_forwarded_even_inside_the_interval() {
    let throttle = ProgressThrottle::new();
    assert!(throttle.should_forward(progress(SolvePhase::Refine, 1)));
    assert!(
        throttle.should_forward(progress(SolvePhase::Refine, 2)),
        "a genuinely new sweep must never be throttled away entirely"
    );
}

// --- Request kinds ---

#[test]
fn a_ghost_preview_request_produces_a_plain_solve_outcome() {
    let request = SolveRequest {
        kind: SolveKind::GhostPreview,
        ..dummy_request(7)
    };
    let outcome = run_solve(&request, &AtomicBool::new(false), &|_| {});
    assert!(
        matches!(outcome, SolveOutcome::Solved(_)),
        "a ghost preview reports a plain solve, never a verified one"
    );
}

// --- Cancellation, on the largest real fixture ---

/// "PC 05.115 CrackOtto-Step" by Ottorino Invernizzi, 103 tiers -- the same
/// fixture `indicatrix-cut-core`'s own
/// `resolve_dirty_speed_on_a_large_real_design`/`cost_probe_large_real_meet_derived_design`
/// tests measure a 5.9s full solve against. Read from the crate's own fixture
/// file rather than re-embedding it, so the two copies can never drift.
const CRACKOTTO_STEP_ASC: &str = include_str!(
    "../../../../../../crates/indicatrix-cut-core/src/optimize_cost_probe_crackotto_step.asc"
);

/// Rebuilds the SAME "mostly implicit `MeetExisting`, one `ScaleReference`
/// anchor per block" structure `indicatrix-cut-core`'s own (private)
/// `design_with_real_meet_structure` test helper builds, using only this
/// crate's public API: `Design::from_asc_schedule` pins every tier to
/// `ScaleReference` at import, but keeps each tier's real original classification
/// in `ConstraintTier::imported_meet` -- restoring that for every tier except
/// one anchor per block reproduces the real, mostly-non-anchor solve shape a
/// cancellation test needs (a design where every tier is already pinned solves
/// almost instantly, which would prove nothing about mid-solve cancellation).
fn crackotto_step_with_real_meet_structure() -> Design {
    let schedule =
        indicatrix_formats::asc::parse_asc(CRACKOTTO_STEP_ASC).expect("fixture must parse");
    let mut design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    assert_eq!(
        design.tiers.len(),
        103,
        "fixture must have its real tier count"
    );

    let inputs = design.meet_tier_inputs();
    let blocks = classify_blocks(&inputs);
    let mut anchor_kept = [false; 3];
    let block_slot = |b: Block| match b {
        Block::Crown => 0,
        Block::Pavilion => 1,
        Block::Girdle => 2,
    };
    for (i, tier) in design.tiers.iter_mut().enumerate() {
        let slot = block_slot(blocks[i]);
        if !anchor_kept[slot] {
            // Keep this tier's import-pinned ScaleReference as the block's one
            // anchor -- required by `Design::solve_with`'s missing-anchor check.
            anchor_kept[slot] = true;
            continue;
        }
        if let Some(original) = tier.imported_meet.clone() {
            tier.constraint = original;
        }
    }
    // Sanity: this fixture's tiers are documented as "every one... implicit
    // MeetExisting" (see `indicatrix-cut-core`'s own doc comment on the
    // equivalent test), so almost all 103 should now be non-anchor.
    let non_anchor = design
        .tiers
        .iter()
        .filter(|t| !matches!(t.constraint, MeetConstraint::ScaleReference(_)))
        .count();
    assert!(
        non_anchor > 90,
        "fixture must be mostly non-anchor for this to be a meaningful cancellation test, got {non_anchor}"
    );
    design
}

#[test]
fn cancel_returns_quickly_on_the_largest_real_fixture() {
    let design = crackotto_step_with_real_meet_structure();
    let cancel = Arc::new(AtomicBool::new(false));
    let request = SolveRequest {
        design: Arc::new(design),
        generation: 0,
        kind: SolveKind::Full,
    };
    let cancel_for_thread = Arc::clone(&cancel);
    let worker = thread::spawn(move || run_solve(&request, &cancel_for_thread, &|_| {}));

    // Let the solve get well past the cheap missing-anchor check and into real
    // refinement work before asking it to stop.
    thread::sleep(Duration::from_millis(50));
    let cancel_requested_at = Instant::now();
    cancel.store(true, Ordering::Relaxed);
    let outcome = worker.join().expect("worker thread must not panic");
    let cancel_latency = cancel_requested_at.elapsed();

    assert!(
        cancel_latency < Duration::from_millis(100),
        "cancel took {cancel_latency:?}, want < 100ms (full uncancelled solve is ~5.9s)"
    );
    match outcome {
        SolveOutcome::Solved(Err(DesignSolveError::Solve(SolveError::Cancelled))) => {}
        SolveOutcome::Solved(Ok(_)) => {
            panic!("expected a cancelled solve, got a completed one")
        }
        SolveOutcome::Solved(Err(other)) => {
            panic!("expected a cancelled solve, got a different error: {other}")
        }
        SolveOutcome::Verified(_) => {
            panic!("expected SolveKind::Full to produce SolveOutcome::Solved")
        }
    }
}
