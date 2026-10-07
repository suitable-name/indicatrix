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
    RedrawGate, SolidPreviewState,
    controller::{PlanFindings, SharedWarnings},
    live_update,
    request::{PlanJob, PlannedFrame, RedrawRequest},
    sink::LateFindings,
};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::{Design, ManufacturabilityWarning};
use std::sync::{
    Arc, PoisonError,
    atomic::Ordering,
    mpsc::{self, Sender},
};

/// How many planned designs' findings [`store_findings`] keeps: the one being drawn, the
/// one the plan worker finished since, and a little slack. Older ones can never be asked
/// for again (the render worker only ever looks up a frame it has just been handed).
const KEPT_FINDINGS: usize = 4;

/// Whether a plan deserves the manufacturability pass at all.
///
/// Not a stale plan. Its masts DO describe its design (a stale plan draws the previous planes
/// but carries the fresh solve), but the preview could not keep up with that design: the frame
/// on screen is one edit behind, and a pass for every edit of such a design would hold up the
/// plan behind it. The idle replan that follows (`schedule_idle_replan_if_stale`) draws the
/// fresh frame, and that plan's pass brings the findings. Not an unsolvable plan (it carries
/// no masts), and not a Slice-tool provisional plan: the sink never shows a provisional
/// frame's findings (`gui::solid_sink` files rows for committed frames only), so the pass would
/// only delay the next plan.
#[must_use]
const fn findings_wanted(generation: u64, stale: bool, unsolvable: bool) -> bool {
    !stale && !unsolvable && crate::gui::editor::frame_updates_mast_cache(generation)
}

/// Whether a plan that will run its own pass is already queued behind the plan that just
/// finished, `queued` being the generation of the job waiting in the plan gate, if any.
///
/// A provisional plan (a Slice-tool drag) does not count: it owes no pass, and nothing else
/// would badge the committed design's rows. The sink replaces the rows of a plan a newer one
/// has overtaken anyway, so the finished plan's pass would be work for rows nobody sees.
#[must_use]
pub(super) fn newer_plan_will_run_a_pass(queued: Option<u64>) -> bool {
    queued.is_some_and(crate::gui::editor::frame_updates_mast_cache)
}

/// The manufacturability pass owed for one finished plan: the full pass over `design` and its
/// masts, which the plan worker runs AFTER it has handed the plan's frame on (check 6 builds
/// a solid per concave tool, and a frame must not wait for that). See [`findings_job`].
pub(super) struct FindingsJob {
    generation: u64,
    design: Arc<Design>,
    masts: Vec<SolvedTier>,
}

impl FindingsJob {
    /// The pass itself: the same one the inline UI code would run, but off the UI thread
    /// (see `PreviewFrame::warnings`).
    pub(super) fn run(&self) -> Vec<ManufacturabilityWarning> {
        indicatrix_editor::view_model::rows::solved_manufacturability_warnings(
            &self.design,
            &self.masts,
        )
    }
}

/// The pass a finished plan is owed, or `None` when it is owed none ([`findings_wanted`]) or
/// its masts do not describe its design: a list of another length (left over from before a
/// tier was added or removed) would badge the wrong rows.
pub(super) fn findings_job(
    generation: u64,
    design: &Arc<Design>,
    solved: Option<&[SolvedTier]>,
    stale: bool,
    unsolvable: bool,
) -> Option<FindingsJob> {
    if !findings_wanted(generation, stale, unsolvable) {
        return None;
    }
    let masts = solved.filter(|masts| masts.len() == design.tiers.len())?;
    Some(FindingsJob {
        generation,
        design: Arc::clone(design),
        masts: masts.to_vec(),
    })
}

/// Runs the pass `owed` to the plan the worker has just handed on, unless a plan with a pass of
/// its own is already waiting in `plan_gate`.
///
/// The pass runs on the plan worker, so every plan behind it waits for it. During a drag the
/// next plan is nearly always queued by the time the previous one is done, and its frame is the
/// one the cutter looks at: the pass of the plan it replaces would badge rows that are about to
/// be replaced. Skipping it keeps the drag as quick as the solves alone; the findings of the
/// newest plan, the one nothing queues behind, still arrive.
pub(super) fn finish_findings(
    state: &SolidPreviewState,
    warnings: &SharedWarnings,
    plan_gate: &RedrawGate<PlanJob>,
    owed: Option<FindingsJob>,
) {
    let Some(job) = owed else {
        return;
    };
    if newer_plan_will_run_a_pass(plan_gate.peek(|queued| queued.generation)) {
        return;
    }
    deliver_findings(state, warnings, job);
}

/// Runs `job` and hands its findings on: stored for the render worker (a frame it has not
/// drawn yet still carries them) and delivered to the sink as the follow-up update to the
/// frame the plan worker submitted before. A panic costs the findings (the rows keep what
/// their frame showed), never the thread.
fn deliver_findings(state: &SolidPreviewState, warnings: &SharedWarnings, job: FindingsJob) {
    let Some(findings) = survive_panic("a manufacturability check", || job.run()) else {
        return;
    };
    store_findings(warnings, &job.design, findings.clone());
    state.sink.apply_findings(LateFindings {
        generation: job.generation,
        design: job.design,
        masts: job.masts,
        warnings: Arc::new(findings),
    });
}

/// Remembers `warnings` as the findings of `design`, dropping the oldest entries past
/// [`KEPT_FINDINGS`].
pub(super) fn store_findings(
    slot: &SharedWarnings,
    design: &Arc<Design>,
    warnings: Vec<ManufacturabilityWarning>,
) {
    let mut kept = slot.lock().unwrap_or_else(PoisonError::into_inner);
    kept.push(PlanFindings {
        design: Arc::clone(design),
        warnings: Arc::new(warnings),
    });
    let surplus = kept.len().saturating_sub(KEPT_FINDINGS);
    kept.drain(..surplus);
}

/// The findings [`store_findings`] kept for exactly this `design` allocation, if the plan
/// worker computed (and has not yet forgotten) any.
pub(super) fn findings_of_plan(
    slot: &SharedWarnings,
    design: &Arc<Design>,
) -> Option<Arc<Vec<ManufacturabilityWarning>>> {
    slot.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .rev()
        .find(|entry| Arc::ptr_eq(&entry.design, design))
        .map(|entry| Arc::clone(&entry.warnings))
}

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
        let warnings = Arc::clone(&self.warnings);
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
                // A panic inside the planner must cost this one replan, not the thread:
                // a dead plan worker silently ignores every later edit and Cut slider
                // move, so the view freezes on its last frame until a restart.
                let Some(frame) = survive_panic("a replan", || {
                    build_planned_frame(job, live_update::DEFAULT_PREVIEW_BUDGET)
                }) else {
                    continue;
                };
                // The manufacturability pass this plan is owed, decided before the frame moves
                // on. It is worked out HERE so the UI thread only displays it: check 6 of the
                // pass builds the stone's solid with every concave tool carved out, which is
                // no work for a UI thread -- and no work to put in front of the picture
                // either, so the frame goes first and the findings follow as their own
                // update. A Slice-tool provisional plan owes none: its findings are never
                // shown.
                let owed = findings_job(
                    frame.generation,
                    &frame.design,
                    frame.solved.as_deref(),
                    frame.stale,
                    frame.unsolvable_status.is_some(),
                );
                if let Some(state) = self_weak.upgrade() {
                    state.submit(RedrawRequest::Planned(Box::new(frame)));
                    finish_findings(&state, &warnings, &plan_gate, owed);
                }
            }
        });
        tx
    }
}

/// Runs `work`, turning a panic into `None` (logged as `what` having panicked; the panic
/// hook has already printed the message) so a worker thread loop can carry on with its
/// next request.
pub(super) fn survive_panic<T>(what: &str, work: impl FnOnce() -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
        .map_err(|_| {
            tracing::error!("The solid-preview worker panicked on {what}; it keeps running.");
        })
        .ok()
}
