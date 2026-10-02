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
//!    Before the worker reports a result, it compares its request's stamped seq
//!    against the current `latest_seq` -- a mismatch means a newer request has
//!    since been submitted. The result is STILL delivered, tagged
//!    [`SolveResult::superseded`] `true`, rather than silently dropped: an earlier
//!    version of this module discarded it outright (`continue`, no delivery at
//!    all), which left any per-request bookkeeping keyed by
//!    [`SolveResult::generation`] permanently stranded whenever two requests
//!    landed close together on the SAME `SolveService` (`native_io::solve`'s
//!    shared one, the case that actually hits this: a Save immediately
//!    followed by an Export) -- no file, no toast, no error, forever. See
//!    [`SolveResult::superseded`]'s own doc comment for what a caller must do
//!    with one.
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
    /// A candidate design solved for display in the viewport in place of the real
    /// one (Retarget's live overlay, Optimize's Preview toggle): the cancellable
    /// counterpart of [`Design::solve`], so it keeps that method's over-plane-cap
    /// fallback instead of surfacing it as an error the way [`Self::Full`] does.
    /// The request's own `generation` is the ghost's sequence number.
    ///
    /// Submit it on a service of its own: the mailbox is last-wins, so sharing one
    /// with a save or export would supersede that request's solve.
    GhostPreview,
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
    /// [`SolveKind::Full`]/[`SolveKind::GhostPreview`]'s result.
    /// Read by `native_io`'s save/export continuations
    /// (`native_io::resolve_solved_then`) and by the ghost preview's landing handler.
    Solved(Result<Vec<SolvedTier>, DesignSolveError>),
    /// [`SolveKind::Verified`]'s result.
    Verified(Result<VerifiedSolve, SolveError>),
}

/// A delivered [`SolveService`] result -- ALWAYS delivered, even for a request a
/// later `submit` superseded while this one was still computing: see
/// [`Self::superseded`]'s own doc comment for why this module stopped silently
/// discarding those instead.
pub struct SolveResult {
    /// Echoed from the [`SolveRequest`] this answers -- see
    /// [`SolveRequest::generation`]'s own doc comment.
    pub generation: u64,
    pub elapsed: Duration,
    pub outcome: SolveOutcome,
    /// `true` when a later `submit` bumped [`SolveService`]'s own `latest_seq`
    /// counter past this request's stamped value before the worker got around to
    /// reporting it -- i.e. this design/kind is no longer the one anybody asked
    /// about last. `outcome` is still the REAL computed result (the compute
    /// itself was not wasted), but a caller must not apply or display it as
    /// current; it is delivered at all ONLY so per-request bookkeeping keyed by
    /// [`Self::generation`] (`native_io::solve::PENDING_SOLVES`, the motivating
    /// case) gets a chance to clean itself up and tell its own caller what
    /// happened, instead of leaking that entry and leaving whoever was waiting
    /// on it (a Save, say) with no file and no toast forever. Previously
    /// this case was silently `continue`d past with no delivery at all -- exactly
    /// the "never leave a continuation stranded" bug the superseded-result delivery guards against.
    pub superseded: bool,
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
///
/// Also carries [`SolveService`]'s own shutdown signal: `shutdown` is
/// checked by [`Self::take_blocking`] every time it wakes, and
/// [`Self::request_shutdown`] sets it and wakes the worker immediately, the
/// same `Condvar` [`Self::put`] already uses -- there is no separate channel or
/// second wait to coordinate.
struct Mailbox {
    slot: Mutex<Option<Queued>>,
    ready: Condvar,
    shutdown: AtomicBool,
}

impl Mailbox {
    const fn new() -> Self {
        Self {
            slot: Mutex::new(None),
            ready: Condvar::new(),
            shutdown: AtomicBool::new(false),
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

    /// Requests the worker's own exit -- see [`SolveService`]'s `Drop`
    /// impl, the one caller. Idempotent: a second call is a harmless repeat of
    /// the same store/wake.
    fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.ready.notify_one();
    }

    /// Blocks until a request is available OR shutdown was requested --
    /// `None` means the latter: the worker's own signal to return from
    /// [`worker_loop`] instead of looping again. A request already sitting in
    /// the mailbox at shutdown time is still delivered (checked first, below)
    /// rather than discarded, so a `submit` that raced a `Drop` is never
    /// silently dropped -- though nothing in this crate submits to a
    /// `SolveService` it no longer holds a reference to in practice.
    fn take_blocking(&self) -> Option<Queued> {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(item) = slot.take() {
                return Some(item);
            }
            if self.shutdown.load(Ordering::Relaxed) {
                return None;
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
        SolveKind::GhostPreview => SolveOutcome::Solved(solve_cancellably(&request.design, cancel)),
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

impl Drop for SolveService {
    /// Signals the worker thread to exit instead of leaving it parked
    /// forever on [`Mailbox::take_blocking`]'s `Condvar` wait -- every caller
    /// that owns a `SolveService` for less than the app's own lifetime
    /// (`deep_solve::spawn_deep_solve` spawns one PER run, held only by its
    /// `DeepSolveHandle`) used to leak exactly that one worker thread per run,
    /// since dropping the `Arc<Mailbox>` clone this struct itself holds does
    /// nothing on its own -- the worker's own clone of the SAME `Arc` keeps it
    /// allocated, and nothing was ever asked to stop reading from it. Does not
    /// join the thread: `Drop` runs on whichever thread drops the last
    /// `SolveService` (the UI thread, for every caller in this crate), and
    /// blocking it on the worker's own exit would defeat the entire point of
    /// running solves off that thread in the first place -- the worker still
    /// exits promptly on its own, it just is not waited for here.
    fn drop(&mut self) {
        self.mailbox.request_shutdown();
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
        let Some(Queued {
            request,
            seq,
            cancel,
        }) = mailbox.take_blocking()
        else {
            // Shutdown requested (`SolveService::drop`) -- exit for
            // real, rather than parking on this mailbox forever. Nothing to
            // report: a `SolveService` being dropped means no caller can
            // still be waiting on this worker's `on_result`.
            return;
        };
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

        // Superseded by a later `submit` while this one was running -- still
        // delivered (see `SolveResult::superseded`'s own doc comment),
        // just tagged so the caller knows not to trust/apply `outcome`. The
        // pending request that superseded it is already sitting in `mailbox`,
        // ready for the next loop iteration either way.
        let superseded = latest_seq.load(Ordering::Relaxed) != seq;
        let result = SolveResult {
            generation,
            elapsed: start.elapsed(),
            outcome,
            superseded,
        };
        let ui = ui_weak.clone();
        let on_result = on_result.clone();
        let _ = ui.upgrade_in_event_loop(move |ui| {
            on_result(&ui, result);
        });
    }
}

#[cfg(test)]
mod tests;
