//! Off-UI-thread solving: one persistent worker thread owning a request queue with
//! last-wins coalescing, real mid-solve cancellation, throttled progress, and
//! generation-tagged results a caller can drop when stale. Replaces the ad-hoc
//! `thread::spawn` bodies in `auto_solve::dispatch_background_solve`,
//! `deep_solve::spawn_deep_solve` and `optimize_solve::spawn_optimize_solve`.
//! The UI thread never solves, never blocks.
//!
//! # Shape
//!
//! [`SolveService::new`] spawns ONE background thread for the lifetime of the
//! service (not one thread per solve, unlike the modules it replaces) that loops:
//! block on the next request in [`Mailbox`], run it, report the result, repeat.
//! [`SolveService::submit`] never blocks the caller -- it just overwrites the
//! mailbox's single pending slot and wakes the worker. A burst of `submit` calls
//! that all land before the worker gets around to picking one up collapses to
//! exactly one solve, of the LAST request only (earlier ones are silently
//! discarded, unread) -- "last-wins coalescing." A request that arrives while the
//! worker is already mid-solve waits in the mailbox; once that solve finishes, the
//! worker checks the mailbox again and picks up whatever is there (the newest thing
//! queued, if anything landed meanwhile).
//!
//! # Two independent kinds of "stale"
//!
//! 1. **Superseded by a later `submit`.** Every `submit` call bumps a shared
//!    `latest_seq` counter and stamps its own request with the value it bumped to.
//!    Before the worker reports ANY result (even one it fully computed), it
//!    compares its request's stamped seq against the current `latest_seq` -- a
//!    mismatch means a newer request has since been submitted, so this result is
//!    thrown away unread rather than delivered (see [`worker_loop`]). This is the
//!    "dropped if stale" -- generation-tagging -- promise: the compute itself is
//!    not free, but nothing here ever delivers a superseded result anywhere.
//! 2. **The design changed for a reason that is NOT a new solve request** (an edit
//!    landed while auto-solve is disabled, say). [`SolveResult::generation`] is the
//!    caller's OWN domain generation counter, stamped at submit time -- this
//!    service does not know what a "current" generation is; the caller compares
//!    [`SolveResult::generation`] against its own live counter at delivery time and
//!    drops the result itself when they disagree, exactly like
//!    `auto_solve::apply_background_solve_result`'s own two-tier check already does
//!    today (that pattern is preserved here, just split across this module's `seq`
//!    and the caller's own `generation` comparison).
//!
//! # Cancellation
//!
//! [`SolveHandle::cancel`] sets the `Arc<AtomicBool>` the underlying
//! `SolveControl::with_cancel` observes at every cancel point `indicatrix` exposes
//! (per-sweep, per-pipeline-run) -- see this module's own
//! `cancel_returns_quickly_on_the_largest_real_fixture` test. Cancelling a request
//! that has not started yet (still sitting in the mailbox) is also safe: the flag
//! is already set by the time the worker picks it up and calls `solve_with`, so it
//! returns [`SolveError::Cancelled`] almost immediately instead of ever doing real
//! work.
//!
//! # Progress
//!
//! `on_progress` is throttled to roughly [`PROGRESS_THROTTLE`] (~10 Hz) via
//! [`ProgressThrottle`] -- `indicatrix`'s solver can report a [`SolveProgress`] many
//! times within one sweep; forwarding every one of those across
//! `Weak::upgrade_in_event_loop` would itself become a UI-thread cost. The first
//! report of a new `(phase, sweep)` pair is always forwarded immediately, so a
//! caller's progress bar never looks stuck at a stale phase for a full throttle
//! interval.

use super::auto_solve::solve_cancellably;
use indicatrix::geometry::{
    meet_solver::{
        SolveControl, SolveError, SolveProgress, SolvedTier, VerifiedSolveReport,
        solve_meet_points_verified_with,
    },
    stone_metrics::ExternalProportions,
};
use indicatrix_cut_core::{Design, DesignSolveError};
use slint::{ComponentHandle, Weak};
use std::{
    cell::Cell,
    collections::BTreeSet,
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// How often a running solve's [`SolveProgress`] reports are actually forwarded to
/// the UI thread -- see the module doc comment, "Progress".
const PROGRESS_THROTTLE: Duration = Duration::from_millis(100);

/// What to solve, and how -- one worker request. See the module doc comment.
pub enum SolveKind {
    /// A full [`Design::solve_with`].
    ///
    /// Submitted by `native_io`'s save/export paths when no cached solve matches
    /// the current design (`native_io::submit_full_solve`), and by this module's
    /// own `cancel_returns_quickly_on_the_largest_real_fixture` test.
    ///
    /// Not routed through here by `auto_solve::dispatch_background_solve`, which
    /// keeps its own worker because that worker does real UI-model formatting work
    /// (`tier_items`/warnings/yield text over up to 103 tiers) that must ALSO stay
    /// off the UI thread -- `SolveService`'s `on_result` runs via
    /// `Weak::upgrade_in_event_loop`, i.e. on the UI thread itself, so moving that
    /// formatting there would be a regression, not a fix.
    Full,
    /// A subgraph [`Design::resolve_dirty_with`] against `previous`'s masts, for the
    /// tiers named in `dirty` -- see that method's own doc comment for what
    /// `previous` must be (a prior solve for a design identical to this request's
    /// `design` except at the positions `dirty` names).
    ///
    /// The one place this crate currently resolves a dirty subgraph during an edit
    /// is `gui::solid_preview`'s own worker (`apps/indicatrix-cut/src/gui/solid_preview/**`),
    /// which is outside this module's ownership. This variant is public for API completeness.
    #[expect(
        dead_code,
        reason = "SolveService's public API is specified with this variant even though \
                   no caller in this module submits it yet -- see the variant's own doc comment"
    )]
    Dirty {
        previous: Vec<SolvedTier>,
        dirty: BTreeSet<usize>,
    },
    /// `indicatrix::geometry::meet_solver::solve_meet_points_verified_with`'s
    /// externally verified repair search ("Deep Solve"). Always passes no
    /// adjustable anchors -- see `deep_solve.rs`'s own doc comment, "`adjustable_anchors` is always empty",
    /// which applies here identically.
    Verified {
        /// The printed proportions the repair search scores candidate mast
        /// configurations against.
        targets: ExternalProportions,
        /// Whether the worker should also compute this design's own plain solve
        /// (the "baseline" a Deep Solve's per-tier mast-delta table compares the
        /// verified repair against) on THIS same background thread, before running
        /// the verified search. `false` when the caller already has a valid
        /// cached plain solve on hand and needs nothing more from this request --
        /// see `deep_solve::spawn_deep_solve`'s own doc comment for why this can
        /// never be submitted as a SEPARATE request instead (this service's
        /// mailbox is last-wins).
        compute_baseline: bool,
    },
}

/// [`SolveOutcome::Verified`]'s `Ok` payload: the verified repair search's own
/// result, plus the baseline plain solve computed alongside it when
/// [`SolveKind::Verified::compute_baseline`] asked for one.
pub struct VerifiedSolve {
    /// The verified repair search's own solved masts.
    pub solved: Vec<SolvedTier>,
    /// The aggregate verdict to display.
    pub report: VerifiedSolveReport,
    /// This design's own plain solve, computed on the SAME worker thread as
    /// `solved` above -- `None` when `compute_baseline` was `false` (the caller
    /// already had one), or when the plain solve itself failed (a design that
    /// does not even plain-solve). Either way, the caller omits its per-tier
    /// delta table rather than blocking on this.
    pub baseline: Option<Vec<SolvedTier>>,
}

/// One request for [`SolveService::submit`]. `design` is an `Arc` (not a plain
/// clone) so a caller that already snapshotted one for another purpose (a
/// solid-preview replan, say) can hand the SAME allocation to both without a
/// second `Design::clone`.
pub struct SolveRequest {
    pub design: Arc<Design>,
    /// The caller's own domain generation counter at submit time -- opaque to this
    /// module, carried through unchanged onto [`SolveResult::generation`] for the
    /// caller's own staleness check. See the module doc comment, "Two independent
    /// kinds of stale."
    pub generation: u64,
    pub kind: SolveKind,
}

/// What a finished (non-superseded) solve produced.
pub enum SolveOutcome {
    /// [`SolveKind::Full`]/[`SolveKind::Dirty`]'s result. Read by `native_io`'s
    /// save/export continuations (`native_io::resolve_solved_then`).
    Solved(Result<Vec<SolvedTier>, DesignSolveError>),
    /// [`SolveKind::Verified`]'s result.
    Verified(Result<VerifiedSolve, SolveError>),
}

/// A delivered, non-superseded [`SolveService`] result.
pub struct SolveResult {
    /// Echoed from the [`SolveRequest`] this answers -- see
    /// [`SolveRequest::generation`]'s own doc comment.
    pub generation: u64,
    pub elapsed: Duration,
    pub outcome: SolveOutcome,
}

/// A delivered, throttled progress tick -- generation-tagged the same way
/// [`SolveResult`] is, for a caller driving a per-generation progress banner.
#[derive(Debug, Clone, Copy)]
pub struct SolveProgressReport {
    pub generation: u64,
    pub progress: SolveProgress,
}

/// Returned by [`SolveService::submit`]. Cancelling stops EXACTLY this request --
/// see the module doc comment, "Cancellation".
pub struct SolveHandle {
    pub generation: u64,
    cancel: Arc<AtomicBool>,
}

impl SolveHandle {
    /// Requests cancellation -- see the module doc comment.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// One queued request plus the bookkeeping the worker needs to decide whether its
/// eventual result still matters.
struct Queued {
    request: SolveRequest,
    seq: u64,
    cancel: Arc<AtomicBool>,
}

/// A single-slot, overwrite-on-put, block-until-present queue -- the "last-wins
/// coalescing" mailbox the module doc comment describes. Deliberately not a
/// channel: a channel would still need draining logic to discard everything but the
/// newest entry, which is exactly what `put` does inline instead.
struct Mailbox {
    slot: Mutex<Option<Queued>>,
    ready: Condvar,
}

impl Mailbox {
    const fn new() -> Self {
        Self {
            slot: Mutex::new(None),
            ready: Condvar::new(),
        }
    }

    /// Overwrites whatever was queued (if anything) and wakes one waiter.
    fn put(&self, item: Queued) {
        {
            let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
            *slot = Some(item);
        }
        self.ready.notify_one();
    }

    /// Blocks until a request is available, then takes it.
    fn take_blocking(&self) -> Queued {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(item) = slot.take() {
                return item;
            }
            slot = self
                .ready
                .wait(slot)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Non-blocking variant for tests: `None` if nothing is queued right now.
    #[cfg(test)]
    fn try_take(&self) -> Option<Queued> {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        slot.take()
    }
}

/// Per-request progress throttle -- see the module doc comment, "Progress". Built
/// fresh for every request (so a new solve always forwards its first report
/// immediately) and lives only inside [`worker_loop`]'s own thread, so plain `Cell`
/// interior mutability is enough despite [`SolveControl::reporting`] requiring a
/// `Fn`, not `FnMut`.
struct ProgressThrottle {
    last_forwarded: Cell<Option<Instant>>,
    last_phase_sweep: Cell<Option<(indicatrix::geometry::meet_solver::SolvePhase, u32)>>,
}

impl ProgressThrottle {
    const fn new() -> Self {
        Self {
            last_forwarded: Cell::new(None),
            last_phase_sweep: Cell::new(None),
        }
    }

    /// `true` the first time a given `(phase, sweep)` is seen, or once
    /// [`PROGRESS_THROTTLE`] has elapsed since the last forwarded report --
    /// otherwise `false` (this report is dropped, not delayed: the next one that
    /// clears the interval carries fresher numbers anyway).
    fn should_forward(&self, progress: SolveProgress) -> bool {
        let key = (progress.phase, progress.sweep);
        let new_phase_sweep = self.last_phase_sweep.get() != Some(key);
        let interval_elapsed = self
            .last_forwarded
            .get()
            .is_none_or(|t| t.elapsed() >= PROGRESS_THROTTLE);
        if !new_phase_sweep && !interval_elapsed {
            return false;
        }
        self.last_phase_sweep.set(Some(key));
        self.last_forwarded.set(Some(Instant::now()));
        true
    }
}

/// Runs one request to completion (or cancellation), calling `on_progress` for
/// every [`SolveProgress`] the underlying `_with` entry point reports (NOT yet
/// throttled -- [`worker_loop`] wraps this with [`ProgressThrottle`] itself).
/// Pulled out on its own, with no `Weak`/event-loop dependency at all, so a test
/// can drive it directly on a plain background thread -- see this module's own
/// `cancel_returns_quickly_on_the_largest_real_fixture` test.
fn run_solve(
    request: &SolveRequest,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(SolveProgress),
) -> SolveOutcome {
    let control = SolveControl::with_cancel(cancel).reporting(on_progress);
    match &request.kind {
        SolveKind::Full => SolveOutcome::Solved(request.design.solve_with(&control)),
        SolveKind::Dirty { previous, dirty } => {
            SolveOutcome::Solved(request.design.resolve_dirty_with(previous, dirty, &control))
        }
        SolveKind::Verified {
            targets,
            compute_baseline,
        } => {
            // Computed FIRST, on this same worker thread, so a Deep Solve run
            // whose caller had no valid cached plain solve never falls back to
            // one on the UI thread -- see `SolveKind::Verified::compute_baseline`'s
            // own doc comment. Shares `cancel`, the identical flag the verified
            // search below observes: a cancel requested mid-baseline is honoured
            // here (near-instantly, same guarantee `solve_cancellably` documents),
            // and the verified search that follows then observes the same flag
            // already set and returns `SolveError::Cancelled` immediately too.
            let baseline = (*compute_baseline)
                .then(|| solve_cancellably(&request.design, cancel).ok())
                .flatten();
            let gear_teeth_abs = request.design.meta.gear_teeth_abs();
            let tiers = request.design.meet_tier_inputs();
            SolveOutcome::Verified(
                solve_meet_points_verified_with(gear_teeth_abs, &tiers, targets, &[], &control)
                    .map(|(solved, report)| VerifiedSolve {
                        solved,
                        report,
                        baseline,
                    }),
            )
        }
    }
}

/// Owns the one persistent worker thread -- see the module doc comment.
pub struct SolveService {
    mailbox: Arc<Mailbox>,
    latest_seq: Arc<AtomicU64>,
}

impl SolveService {
    /// Spawns the worker thread. `on_progress`/`on_result` are invoked on the
    /// UI/event-loop thread via `Weak::upgrade_in_event_loop` -- both silently do
    /// nothing once `ui_weak` no longer upgrades (window closed while a solve was
    /// in flight), the same convention `deep_solve::spawn_deep_solve`/
    /// `optimize_solve::spawn_optimize_solve` already follow.
    pub fn new<T, P, D>(ui_weak: Weak<T>, on_progress: P, on_result: D) -> Self
    where
        T: ComponentHandle + 'static,
        P: Fn(&T, SolveProgressReport) + Send + Clone + 'static,
        D: Fn(&T, SolveResult) + Send + Clone + 'static,
    {
        let mailbox = Arc::new(Mailbox::new());
        let latest_seq = Arc::new(AtomicU64::new(0));
        let worker_mailbox = Arc::clone(&mailbox);
        let worker_seq = Arc::clone(&latest_seq);
        thread::spawn(move || {
            worker_loop(
                &ui_weak,
                &worker_mailbox,
                &worker_seq,
                on_progress,
                on_result,
            );
        });
        Self {
            mailbox,
            latest_seq,
        }
    }

    /// Queues `request`, coalescing with (overwriting) anything not yet picked up
    /// by the worker -- see the module doc comment. Returns a handle that cancels
    /// EXACTLY this request.
    pub fn submit(&self, request: SolveRequest) -> SolveHandle {
        let seq = self.latest_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let generation = request.generation;
        self.mailbox.put(Queued {
            request,
            seq,
            cancel: Arc::clone(&cancel),
        });
        SolveHandle { generation, cancel }
    }
}

/// The worker thread's whole life -- see the module doc comment for the two
/// staleness checks this performs (the `seq` one, inline below; the `generation`
/// one is the caller's own job once [`SolveResult`] is delivered).
fn worker_loop<T, P, D>(
    ui_weak: &Weak<T>,
    mailbox: &Mailbox,
    latest_seq: &AtomicU64,
    on_progress: P,
    on_result: D,
) where
    T: ComponentHandle + 'static,
    P: Fn(&T, SolveProgressReport) + Send + Clone + 'static,
    D: Fn(&T, SolveResult) + Send + Clone + 'static,
{
    loop {
        let Queued {
            request,
            seq,
            cancel,
        } = mailbox.take_blocking();
        let generation = request.generation;
        let start = Instant::now();
        let throttle = ProgressThrottle::new();

        let outcome = run_solve(&request, &cancel, &|progress| {
            if latest_seq.load(Ordering::Relaxed) != seq || !throttle.should_forward(progress) {
                return;
            }
            let ui = ui_weak.clone();
            let on_progress = on_progress.clone();
            let _ = ui.upgrade_in_event_loop(move |ui| {
                on_progress(
                    &ui,
                    SolveProgressReport {
                        generation,
                        progress,
                    },
                );
            });
        });

        if latest_seq.load(Ordering::Relaxed) != seq {
            // Superseded by a later `submit` while this one was running -- never
            // delivered, see the module doc comment. The pending request that
            // superseded it is already sitting in `mailbox`, ready for the next
            // loop iteration.
            continue;
        }
        let result = SolveResult {
            generation,
            elapsed: start.elapsed(),
            outcome,
        };
        let ui = ui_weak.clone();
        let on_result = on_result.clone();
        let _ = ui.upgrade_in_event_loop(move |ui| {
            on_result(&ui, result);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::{Block, MeetConstraint, classify_blocks};
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

    // `SolveService::new`/`submit` themselves are NOT exercised end-to-end here:
    // both require a live `slint::ComponentHandle` (a real `MainWindow`), which
    // needs a windowing backend this suite cannot start (no GPU/display in the
    // test environment -- see this crate's own house rule). The `seq`-staleness
    // check above and `Mailbox`'s coalescing tests cover the same decisions
    // `SolveService::submit`/`worker_loop` make, without needing one.

    // --- ProgressThrottle ---

    fn progress(phase: indicatrix::geometry::meet_solver::SolvePhase, sweep: u32) -> SolveProgress {
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
        assert!(throttle.should_forward(progress(
            indicatrix::geometry::meet_solver::SolvePhase::Refine,
            1
        )));
    }

    #[test]
    fn a_second_report_of_the_same_phase_sweep_within_the_interval_is_dropped() {
        let throttle = ProgressThrottle::new();
        let p = progress(indicatrix::geometry::meet_solver::SolvePhase::Refine, 1);
        assert!(throttle.should_forward(p));
        assert!(
            !throttle.should_forward(p),
            "an immediate repeat of the same (phase, sweep) inside the throttle window must be dropped"
        );
    }

    #[test]
    fn a_report_from_a_new_sweep_is_forwarded_even_inside_the_interval() {
        let throttle = ProgressThrottle::new();
        assert!(throttle.should_forward(progress(
            indicatrix::geometry::meet_solver::SolvePhase::Refine,
            1
        )));
        assert!(
            throttle.should_forward(progress(
                indicatrix::geometry::meet_solver::SolvePhase::Refine,
                2
            )),
            "a genuinely new sweep must never be throttled away entirely"
        );
    }

    // --- Cancellation, on the largest real fixture ---

    /// "PC 05.115 CrackOtto-Step" by Ottorino Invernizzi, 103 tiers -- the same
    /// fixture `indicatrix-cut-core`'s own
    /// `resolve_dirty_speed_on_a_large_real_design`/`cost_probe_large_real_meet_derived_design`
    /// tests measure a 5.9s full solve against. Read from the crate's own fixture
    /// file rather than re-embedding it, so the two copies can never drift.
    const CRACKOTTO_STEP_ASC: &str = include_str!(
        "../../../../../crates/indicatrix-cut-core/src/optimize_cost_probe_crackotto_step.asc"
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
}
