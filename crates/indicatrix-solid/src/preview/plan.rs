//! The planning half of a redraw: [`build_planned_frame`], the only function in
//! the pipeline that calls the potentially multi-second
//! [`live_update::plan_preview`].
//!
//! Moved from the desktop's `gui::solid_preview::preview_state::plan_worker`
//! (which keeps its thread and a thin wrapper passing its `Instant`-backed clock);
//! the web app calls it on the main thread with a `performance.now()` clock.

use super::request::{PlanJob, PlannedFrame};
use crate::{facet_map::FacetMap, live_update, raster::SolidStyle};
use indicatrix::geometry::{meet_solver::SolvedTier, tool::ToolPrimitive};
use indicatrix_cut_core::design::{Design, TierRef};
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
    let (tools, placements) =
        concave_tools_for_display(&design, plan.solved.as_deref(), tier_cutoff);
    let facet_map = FacetMap::from_design_with_tools(
        &design,
        plan.solved.as_deref().unwrap_or(&[]),
        &placements,
    );
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
        tools,
        placements,
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

/// The concave tools to draw with `design`'s planes, and their `(tier, placement)`
/// list. Empty for a design without concave tiers (and so for every planar design,
/// leaving the frame byte-identical), when there is no solve to measure the stone
/// against, and when the concave tiers do not resolve (an invalid tier, too many
/// placements): the preview then shows the flat stone rather than failing, and the
/// editor's own validation reports why.
///
/// `tier_cutoff` is the viewport's "show through tier N" slider, an index into the
/// flat tiers. The tools are those that precede the first hidden flat tier in cutting
/// order, where "hidden" is [`live_update::visible_flat_tiers`]'s rule -- the one that
/// truncates the planes of the same frame, so a groove is never drawn into planes the
/// slider has removed.
fn concave_tools_for_display(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    tier_cutoff: Option<usize>,
) -> (Vec<ToolPrimitive>, Vec<(usize, usize)>) {
    let Some(solved) = solved else {
        return (Vec::new(), Vec::new());
    };
    if design.concave_tiers.is_empty() {
        return (Vec::new(), Vec::new());
    }
    // The first hidden flat tier in cutting order is the boundary; with no hidden
    // tier everything is shown.
    let boundary = tier_cutoff.and_then(|cutoff| {
        let visible = live_update::visible_flat_tiers(design, cutoff);
        design
            .cutting_order()
            .into_iter()
            .find(|tier| matches!(tier, TierRef::Flat(i) if !visible[*i]))
    });
    let resolved = boundary.map_or_else(
        || design.concave_tools_from_solved(solved),
        |first_hidden| design.concave_tools_through_tier(solved, first_hidden),
    );
    resolved.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With the slider after the stored-first crown tier of a crown-first design, the
    /// groove (a pavilion tool, cut before every crown tier) is drawn together with the
    /// pavilion planes it cuts, and the crown dimple, cut after the hidden crown tier, is not.
    #[test]
    fn concave_tools_follow_the_same_visible_tiers_as_the_planes() {
        let mut design = Design::concave_fixture();
        design.tiers.reverse();
        let solved = design.solve().expect("the fixture solves");
        let (all, _) = concave_tools_for_display(&design, Some(&solved), None);
        let (through, placements) = concave_tools_for_display(&design, Some(&solved), Some(0));
        assert_eq!(all.len(), 12, "eight groove placements and four dimples");
        assert_eq!(through.len(), 8);
        assert!(placements.iter().all(|&(tier, _)| tier == 0));
    }
}
