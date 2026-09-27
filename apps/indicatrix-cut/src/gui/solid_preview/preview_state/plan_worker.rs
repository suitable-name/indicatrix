//! The PLAN worker's own half of a replan: [`build_planned_frame`] (the only
//! function that ever calls the potentially multi-second `live_update::
//! plan_preview`), plus the [`super::SolidPreviewState`] methods that spawn and
//! feed that worker thread. See the parent module's doc comment ("Two workers:
//! planning vs. rendering") for why this is split from the RENDER worker.

use super::{
    FacetMap, SolidPreviewState, SolidStyle, live_update,
    request::{PlanJob, PlannedFrame, RedrawRequest},
};
use std::sync::{
    Arc, PoisonError,
    mpsc::{self, Sender},
};

/// Runs `live_update::plan_preview` (the expensive, potentially multi-second
/// call) and builds facet-level [`SolidStyle`] from its result via
/// `facet_map::FacetMap::overlay_flags`.
///
/// `budget` is a parameter (rather than always `live_update::DEFAULT_PREVIEW_BUDGET`
/// inline) so this module's tests can force the `Stale` branch deterministically
/// (`Duration::ZERO`, which a real `resolve_dirty` call can never finish within).
///
/// The full `plan.freshness`'s `Stale.pending` set is forwarded unchanged.
/// All tiers in a batch edit are outlined as pending.
///
/// Deliberately does NOT decide `Unbounded` vs. closed -- that needs a
/// `mesh_cache::MeshCache`, which this (PLAN-worker-only) function never touches;
/// see `super::state::resolve_planned_state` for the render-side half that
/// finishes the job.
pub fn build_planned_frame(job: PlanJob, budget: std::time::Duration) -> PlannedFrame {
    let PlanJob {
        design,
        dirty,
        last_solved,
        camera,
        size,
        selected_tier,
        n_d,
        view_mode,
        generation,
        show_preform,
        enlarged_panel,
        tier_cutoff,
    } = job;
    let plan = live_update::plan_preview(
        &design,
        last_solved.as_deref(),
        &dirty,
        budget,
        &live_update::RealSolver,
        tier_cutoff,
    );
    // Use the WHOLE pending set, not just its first member. `empty_pending` gives
    // the non-`Stale` arm something to borrow.
    let empty_pending = std::collections::BTreeSet::new();
    let pending_tiers = match &plan.freshness {
        live_update::Freshness::Stale { pending } => pending,
        _ => &empty_pending,
    };
    let facet_map = FacetMap::from_design(&design, plan.solved.as_deref().unwrap_or(&[]));
    let overlay = facet_map.overlay_flags(&design, n_d, selected_tier, pending_tiers);
    let unsolvable_status = match &plan.freshness {
        live_update::Freshness::Unsolvable(err) => {
            Some(format!("Preview cannot be solved: {err}."))
        }
        _ => None,
    };
    // No `MeshCache` on this thread. The `Unbounded` check is done by
    // `super::state::resolve_planned_state` on the RENDER worker. `style` below
    // is UNDIMMED regardless of whether this frame turns out unbounded.
    let is_unsolvable = unsolvable_status.is_some();
    let preform_plane_count = facet_map.preform_plane_count();
    let style = SolidStyle {
        flagged: overlay.flagged,
        pending: overlay.pending,
        selected: overlay.selected,
        preform_plane_count,
        show_preform,
        ..SolidStyle::default()
    };
    let stale = matches!(plan.freshness, live_update::Freshness::Stale { .. });
    // An `Unsolvable` frame must not wipe the shared `last_solved` cache with `None`
    // (`plan.solved` is always `None` on that path -- see `live_update::plan_preview`):
    // chain the OLD masts forward unchanged instead, so the next edit's
    // `resolve_dirty` still has something to diff against rather than being forced
    // into a full `Design::solve()`.
    let solved = if is_unsolvable {
        last_solved
    } else {
        plan.solved
    };
    PlannedFrame {
        design,
        planes: plan.planes,
        style,
        solved,
        stale,
        unsolvable_status,
        camera,
        size,
        view_mode,
        generation,
        n_d,
        enlarged_panel,
    }
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
                let frame = build_planned_frame(job, live_update::DEFAULT_PREVIEW_BUDGET);
                if let Some(state) = self_weak.upgrade() {
                    state.submit(RedrawRequest::Planned(Box::new(frame)));
                }
            }
        });
        tx
    }
}
