//! The Optimize result SUMMARY table as Slint rows ([`optimize_result_rows`]) and
//! the ghost preview ([`submit_design_ghost_preview`]): a candidate design shown in
//! the solid viewport in place of the real one, solved on a background worker so a
//! Retarget slider drag or the Optimize "Preview" toggle never blocks the UI thread.
//! The status line, the preview candidate design and the weight-form parser live in
//! `indicatrix_editor::optimize_view` (shared with the web app) and are re-exported
//! here at their old paths. See [`super::solve_results`] for the per-tier result
//! tables and the Deep Solve/Optimize availability hints.

use super::viewport::scaled_viewport_size;
use crate::{
    MainWindow, OptimizeResultRow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            auto_solve,
            solve_service::{
                SolveHandle, SolveKind, SolveOutcome, SolveRequest, SolveResult, SolveService,
            },
        },
        solid_preview::preview_state::{CameraPose, SolidPreviewState},
    },
};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::{Design, OptimizeOutcome};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    sync::{Arc, Mutex},
};

pub(in crate::gui::editor) use indicatrix_editor::optimize_view::{
    build_optimize_preview_design, optimize_status_text, parse_optimize_weights,
};

/// [`indicatrix_editor::optimize_view::optimize_result_rows`], mapped to the
/// Optimize tab's Slint result rows -- each component on its own row, the blended
/// score last, every "after" cell naming its own signed delta and verdict.
pub(in crate::gui::editor) fn optimize_result_rows(
    outcome: &OptimizeOutcome,
) -> Vec<OptimizeResultRow> {
    indicatrix_editor::optimize_view::optimize_result_rows(outcome)
        .into_iter()
        .map(|line| OptimizeResultRow {
            label: line.label.into(),
            before: line.before.into(),
            after: line.after.into(),
            direction: line.direction,
        })
        .collect()
}

/// What a landing ghost result needs to reach the viewport: the candidate design
/// (to turn its solved masts into planes) and the two handles that draw them.
struct PendingGhost {
    design: Arc<Design>,
    render_ctx: Arc<Mutex<RenderContext>>,
    preview_state: Arc<SolidPreviewState>,
}

/// Bookkeeping for the newest ghost request: at most one is wanted at a time, and
/// a result is applied only if it answers exactly that one.
///
/// Generic over the pending payload so the staleness rules are testable without a
/// window.
struct GhostTracker<P> {
    /// Generation of the newest request, or of the last invalidation.
    latest: u64,
    /// The payload of the request `latest` names, until it lands or is invalidated.
    pending: Option<P>,
}

impl<P> GhostTracker<P> {
    const fn new() -> Self {
        Self {
            latest: 0,
            pending: None,
        }
    }

    /// Records a new request, superseding any earlier one, and returns the
    /// generation to stamp it with.
    fn begin(&mut self, pending: P) -> u64 {
        self.latest = self.latest.wrapping_add(1);
        self.pending = Some(pending);
        self.latest
    }

    /// Drops the wanted request: no result already in flight will be applied.
    fn invalidate(&mut self) {
        self.latest = self.latest.wrapping_add(1);
        self.pending = None;
    }

    /// The payload for a result of `generation`, exactly once -- `None` when the
    /// service reports it `superseded`, when a newer request or an invalidation
    /// has moved `latest` on, or when it was already taken.
    const fn take_if_current(&mut self, generation: u64, superseded: bool) -> Option<P> {
        if superseded || generation != self.latest {
            return None;
        }
        self.pending.take()
    }
}

/// The ghost preview's state. Thread-local because every reader and writer runs on
/// the Slint event-loop thread, including the result handler (delivered through
/// `Weak::upgrade_in_event_loop`).
struct GhostState {
    /// The worker, created on first use. A service of its own: its mailbox is
    /// last-wins, so sharing one with a save or export would supersede that solve.
    service: Option<SolveService>,
    /// Cancels the solve of the newest request, which may still be running.
    handle: Option<SolveHandle>,
    tracker: GhostTracker<PendingGhost>,
}

impl GhostState {
    const fn new() -> Self {
        Self {
            service: None,
            handle: None,
            tracker: GhostTracker::new(),
        }
    }

    /// Stops wanting whatever is in flight and asks its solve to stop.
    fn cancel(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.cancel();
        }
        self.tracker.invalidate();
    }
}

thread_local! {
    static GHOST: RefCell<GhostState> = const { RefCell::new(GhostState::new()) };
}

/// Drops any ghost still being solved, so it cannot land over what the viewport is
/// about to show. Called by every path that puts the REAL design into the viewport
/// (`super::viewport::submit_preview_replan_chained`, the synchronous refresh).
pub(super) fn cancel_ghost_preview() {
    GHOST.with(|cell| cell.borrow_mut().cancel());
}

/// Queues `design` -- a candidate, not the real design -- to be solved on a
/// background worker and drawn in the shared solid viewport at the camera pose of
/// the moment it lands: a raw, generation-independent reproject
/// (`SolidPreviewState::request_redraw_with_gear`, the same call
/// `gui::render::camera_lighting::resubmit_at_current_pose` uses for a camera
/// drag), deliberately NOT [`super::viewport::submit_preview_replan`]'s
/// worker-queued `ReplanRequest` path: a ghost preview must never stamp
/// `solid_last_solved`/the design-generation stash with a CANDIDATE design's own
/// solved masts, which would corrupt the next real edit's `resolve_dirty`
/// baseline and the next landed worker frame's tier-table push
/// (`super::viewport::push_solved_preview`) into showing the ghost's numbers
/// instead of the live design's.
///
/// Never blocks: a newer call supersedes an older one whose solve has not landed
/// (the running solve is cancelled, a result that still arrives is dropped), and a
/// later real-design replan does the same through [`cancel_ghost_preview`].
///
/// Returns whether the request was queued. `true` means the ghost will appear when
/// its solve lands, so the caller must not submit the real design over it; a
/// candidate that turns out not to solve leaves the viewport showing whatever it
/// already had, since there is no honest candidate geometry to draw for a design
/// that does not close.
///
/// Used by both Optimize's own "Preview" toggle (via
/// [`build_optimize_preview_design`]) and Retarget's live ghost overlay
/// (`callbacks::retarget_actions`), so the two features share one implementation of
/// "show this candidate, without disturbing anything the REAL design's next edit
/// depends on."
#[must_use]
pub(in crate::gui::editor) fn submit_design_ghost_preview(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    design: &Design,
) -> bool {
    GHOST.with(|cell| {
        let mut ghost = cell.borrow_mut();
        let state = &mut *ghost;
        let service = state.service.get_or_insert_with(|| {
            SolveService::new(ui.as_weak(), |_ui, _progress| {}, apply_ghost_result)
        });
        if let Some(previous) = state.handle.take() {
            previous.cancel();
        }
        let design = Arc::new(design.clone());
        let generation = state.tracker.begin(PendingGhost {
            design: Arc::clone(&design),
            render_ctx: Arc::clone(render_ctx),
            preview_state: Arc::clone(preview_state),
        });
        state.handle = Some(service.submit(SolveRequest {
            design,
            generation,
            kind: SolveKind::GhostPreview,
        }));
        true
    })
}

/// The ghost worker's result handler, on the UI thread: draws the planes of the
/// newest request's solve, drops everything else.
fn apply_ghost_result(ui: &MainWindow, result: SolveResult) {
    let pending = GHOST.with(|cell| {
        cell.borrow_mut()
            .tracker
            .take_if_current(result.generation, result.superseded)
    });
    let Some(pending) = pending else {
        return;
    };
    if let SolveOutcome::Solved(Ok(solved)) = result.outcome {
        draw_ghost(ui, &pending, &solved);
    }
}

/// Redraws the solid viewport with `pending`'s design at the CURRENT camera pose.
fn draw_ghost(ui: &MainWindow, pending: &PendingGhost, solved: &[SolvedTier]) {
    let planes_gpu = auto_solve::design_to_gpu_planes_from_solved(&pending.design, solved);
    let planes: Vec<(glam::Vec3, f32)> = planes_gpu
        .iter()
        .map(|p| (glam::Vec3::from(p.normal), -p.d))
        .collect();
    let ctx = pending
        .render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let camera = CameraPose {
        yaw: ctx.yaw,
        pitch: ctx.pitch,
        distance: ctx.distance,
    };
    let design_gear = ctx.design_gear;
    let view_mode = ui.global::<crate::SolidPreviewModel>().get_view_mode() as u8;
    let size = crate::gui::render::camera_lighting::contained_request_size(
        view_mode,
        scaled_viewport_size(ui),
        (ctx.width, ctx.height),
    );
    drop(ctx);
    pending
        .preview_state
        .request_redraw_with_gear(planes, camera, size, view_mode, design_gear);
}

#[cfg(test)]
mod tests {
    use super::GhostTracker;

    #[test]
    fn the_newest_request_is_applied_exactly_once() {
        let mut tracker = GhostTracker::new();
        let generation = tracker.begin("candidate");
        assert_eq!(
            tracker.take_if_current(generation, false),
            Some("candidate")
        );
        assert_eq!(
            tracker.take_if_current(generation, false),
            None,
            "a result is applied once"
        );
    }

    #[test]
    fn a_result_for_an_older_request_is_dropped_and_keeps_the_newer_one_wanted() {
        let mut tracker = GhostTracker::new();
        let first = tracker.begin("first");
        let second = tracker.begin("second");
        assert_ne!(first, second);
        assert_eq!(tracker.take_if_current(first, false), None);
        assert_eq!(
            tracker.take_if_current(second, false),
            Some("second"),
            "the stale result must not consume the newer request"
        );
    }

    #[test]
    fn a_superseded_result_is_dropped_even_for_the_newest_generation() {
        let mut tracker = GhostTracker::new();
        let generation = tracker.begin("candidate");
        assert_eq!(tracker.take_if_current(generation, true), None);
    }

    #[test]
    fn an_invalidation_drops_the_result_still_in_flight() {
        let mut tracker = GhostTracker::new();
        let generation = tracker.begin("candidate");
        tracker.invalidate();
        assert_eq!(tracker.take_if_current(generation, false), None);
    }

    #[test]
    fn a_request_after_an_invalidation_is_wanted_again() {
        let mut tracker = GhostTracker::new();
        let stale = tracker.begin("stale");
        tracker.invalidate();
        let fresh = tracker.begin("fresh");
        assert_eq!(tracker.take_if_current(stale, false), None);
        assert_eq!(tracker.take_if_current(fresh, false), Some("fresh"));
    }
}
