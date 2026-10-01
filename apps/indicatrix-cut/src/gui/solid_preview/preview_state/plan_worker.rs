//! The PLAN worker's own half of a replan: [`build_planned_frame`] (the only
//! function that ever calls the potentially multi-second `live_update::
//! plan_preview`), plus the [`super::SolidPreviewState`] methods that spawn and
//! feed that worker thread. See the parent module's doc comment ("Two workers:
//! planning vs. rendering") for why this is split from the RENDER worker.
//!
//! The planner itself moved to `indicatrix_solid::preview::build_planned_frame`
//! (shared with the web app, which runs it on its main thread with a
//! `performance.now()` clock); [`build_planned_frame`] here is the thin wrapper
//! passing this desktop's `Instant`-backed clock.

use super::{
    SolidPreviewState, live_update,
    request::{PlanJob, PlannedFrame, RedrawRequest},
};
use std::sync::{
    Arc, PoisonError,
    atomic::Ordering,
    mpsc::{self, Sender},
};

/// This desktop's real `live_update::Clock`: `std::time::Instant`-backed, relative
/// to a per-process epoch (`Instant` has no absolute "now" of its own to read).
///
/// Lives here, not in `indicatrix-solid`: that crate must contain no
/// `Instant::now` at all, since it needs to compile clean on
/// `wasm32-unknown-unknown`, where `Instant::now()` panics at runtime -- see
/// `live_update::Clock`'s own doc comment. A wasm caller passes its own
/// `performance.now()`-backed implementation instead.
#[derive(Debug, Clone, Copy, Default)]
struct InstantClock;

impl live_update::Clock for InstantClock {
    fn now_ms(&self) -> f64 {
        static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        EPOCH
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_secs_f64()
            * 1000.0
    }
}

/// `indicatrix_solid::preview::build_planned_frame` with this desktop's
/// [`InstantClock`]: runs `live_update::plan_preview` (the expensive, potentially
/// multi-second call) and builds the facet-level style from its result.
///
/// `budget` is a parameter (rather than always `live_update::DEFAULT_PREVIEW_BUDGET`
/// inline) so this module's tests can force the `Stale` branch deterministically
/// (`Duration::ZERO`, which a real `resolve_dirty` call can never finish within).
pub fn build_planned_frame(job: PlanJob, budget: std::time::Duration) -> PlannedFrame {
    indicatrix_solid::preview::build_planned_frame(job, budget, &InstantClock)
}

impl SolidPreviewState {
    /// [`super::SolidPreviewState::submit`]'s counterpart for the PLAN worker: lazily spawns it, then
    /// pushes `job` through `self.plan_gate` (coalescing
    /// exactly like a `Reproject`/overlay update -- a burst of edits against a
    /// slow design collapses to "whichever solve is running, then the latest one
    /// queued behind it," never a pile of concurrent solves) and wakes it if this
    /// call won the race.
    pub(super) fn submit_plan(&self, job: PlanJob) {
        let tx = {
            let mut guard = self
                .plan_wake
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if guard.is_none() {
                *guard = Some(self.spawn_plan_worker());
            }
            guard
                .clone()
                .expect("just initialized above if it was empty")
        };
        if self.plan_gate.submit(job).is_some() {
            let _ = tx.send(());
        }
    }

    /// Spawns the PLAN worker thread. Called at most once, guarded by `plan_wake`.
    /// This is the only thread that calls [`build_planned_frame`], so render
    /// requests never block on slow solves.
    ///
    /// Hands each finished [`PlannedFrame`] to the RENDER worker through
    /// [`super::SolidPreviewState::submit`] via `self_weak` -- see that field's own doc comment for
    /// why a weak handle rather than capturing `self` directly.
    fn spawn_plan_worker(&self) -> Sender<()> {
        let (tx, rx) = mpsc::channel::<()>();
        let plan_gate = Arc::clone(&self.plan_gate);
        let generation_floor = Arc::clone(&self.generation_floor);
        let self_weak = self
            .self_weak
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        std::thread::spawn(move || {
            for () in rx {
                // Same "may be newer than the request that woke us" coalescing
                // [`super::render`]'s own worker loop relies on -- a burst of edits
                // against a slow design collapses to the latest one queued behind
                // whichever solve is currently running.
                let Some(job) = plan_gate.take() else {
                    continue;
                };
                // a `PlanJob` queued (or already in flight) for a design
                // `reset_for_new_design` has since replaced must not be solved
                // and handed back as a frame -- `Self::bump_generation_floor`
                // moves this floor past every generation the OLD design could
                // still have queued the moment New/Load/Open replaces it.
                if job.generation < generation_floor.load(Ordering::Relaxed) {
                    continue;
                }
                let frame = build_planned_frame(job, live_update::DEFAULT_PREVIEW_BUDGET);
                if let Some(state) = self_weak.upgrade() {
                    state.submit(RedrawRequest::Planned(Box::new(frame)));
                }
            }
        });
        tx
    }
}
