//! The planning half of a redraw: [`build_planned_frame`], the only function in
//! the pipeline that calls the potentially multi-second
//! [`live_update::plan_preview`].
//!
//! Moved from the desktop's `gui::solid_preview::preview_state::plan_worker`
//! (which keeps its thread and a thin wrapper passing its `Instant`-backed clock);
//! the web app calls it on the main thread with a `performance.now()` clock.

use super::request::{PlanJob, PlannedFrame};
use crate::{facet_map::FacetMap, live_update, raster::SolidStyle};
use std::time::Duration;

/// Runs [`live_update::plan_preview`] (the expensive, potentially multi-second
/// call) and builds the facet-level [`SolidStyle`] from its result via
/// [`FacetMap::overlay_flags`].
///
/// `budget` is a parameter (the desktop and web both pass
/// [`live_update::DEFAULT_PREVIEW_BUDGET`]) so tests can force the `Stale` branch
/// deterministically (`Duration::ZERO`); `clock` is the caller's monotonic clock
/// (see [`live_update::Clock`]).
///
/// The full `plan.freshness`'s `Stale.pending` set is forwarded unchanged.
/// All tiers in a batch edit are outlined as pending.
///
/// Deliberately does NOT decide `Unbounded` vs. closed -- that needs a
/// [`crate::mesh_cache::MeshCache`]; see [`super::render_request`] for the half that
/// finishes the job.
#[must_use]
pub fn build_planned_frame(
    job: PlanJob,
    budget: Duration,
    clock: &dyn live_update::Clock,
) -> PlannedFrame {
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
        clock,
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
    // No `MeshCache` here. The `Unbounded` check is done by the render step.
    // `style` below is UNDIMMED regardless of whether this frame turns out
    // unbounded.
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
    // An `Unsolvable` frame must not wipe the caller's `last_solved` cache with
    // `None` (`plan.solved` is always `None` on that path -- see
    // `live_update::plan_preview`): chain the OLD masts forward unchanged instead,
    // so the next edit's `resolve_dirty` still has something to diff against
    // rather than being forced into a full `Design::solve()`.
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
