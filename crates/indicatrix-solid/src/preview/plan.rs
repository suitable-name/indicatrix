//! The planning half of a redraw: [`build_planned_frame`], the only function in
//! the pipeline that calls the potentially multi-second
//! [`live_update::plan_preview`].
//!
//! Moved from the desktop's `gui::solid_preview::preview_state::plan_worker`
//! (which keeps its thread and a thin wrapper passing its `Instant`-backed clock);
//! the web app calls it on the main thread with a `performance.now()` clock.

use super::request::{PlanJob, PlannedFrame};
use crate::{
    facet_map::FacetMap,
    live_update::{self, CutLimit},
    raster::SolidStyle,
};
use indicatrix::geometry::{meet_solver::SolvedTier, tool::ToolPrimitive};
use indicatrix_cut_core::design::Design;
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
    let limit = job.cut_limit();
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
        tier_cutoff: _,
        cut_steps: _,
    } = job;
    let plan = live_update::plan_preview_limited(
        &design,
        last_solved.as_deref(),
        &dirty,
        budget,
        &live_update::RealSolver,
        clock,
        limit,
    );
    // Use the WHOLE pending set, not just its first member. `empty_pending` gives
    // the non-`Stale` arm something to borrow.
    let empty_pending = std::collections::BTreeSet::new();
    let pending_tiers = match &plan.freshness {
        live_update::Freshness::Stale { pending } => pending,
        _ => &empty_pending,
    };
    // The masts the tools and the facet map are built from: the plan's own solve, or -- for an
    // `Unsolvable` plan, which solved nothing -- the previous masts the held-over stone is
    // drawn from. They follow the stone on screen, not the masts the frame carries: an
    // unsolvable frame carries none (see `solved` below) yet still shows the previous stone,
    // and that stone's concave tools and hover entries belong to it.
    let drawn = drawn_masts(plan.solved.as_deref(), last_solved.as_deref(), &design);
    let (tools, placements) = concave_tools_for_display(&design, drawn, limit);
    // The planes of a partly cut stone are the finished stone's with the hidden tiers'
    // slices removed, so the facet ids must be counted the same way.
    let visible_tiers = live_update::limit_visible_tiers(&design, limit);
    let facet_map = FacetMap::from_design_cut(
        &design,
        drawn.unwrap_or(&[]),
        &placements,
        visible_tiers.as_deref(),
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
    // An `Unsolvable` frame carries NO masts (`plan.solved` is always `None` on that path
    // -- see `live_update::plan_preview`). It used to chain the previous masts forward
    // unchanged, so the caller's `last_solved` cache was not wiped; but the frame is
    // stamped with the NEW generation, so those masts then read as the solve of a design
    // they do not describe: an export asking for the masts "of exactly this generation"
    // got the new angles on the old masts, and the next edit's `resolve_dirty` diffed
    // against masts that predate the edit that broke the solve. A caller keeps its own
    // cache untouched when a frame brings no masts (the desktop sink and the web app both
    // only file a frame's masts when it has some), so nothing is lost: the cache keeps
    // the masts under the generation that really was solved.
    let solved = plan.solved;
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
        visible_tiers,
    }
}

/// The masts a frame's concave tools and facet map are built from: the plan's own `solved`
/// masts, or, when the plan solved nothing (an unsolvable design), the `previous` masts the
/// held-over stone is drawn from -- as long as they still line up with `design`'s tiers, the
/// same test [`live_update::plan_preview`] applies before it draws the previous planes.
fn drawn_masts<'a>(
    solved: Option<&'a [SolvedTier]>,
    previous: Option<&'a [SolvedTier]>,
    design: &Design,
) -> Option<&'a [SolvedTier]> {
    solved.or_else(|| previous.filter(|masts| masts.len() == design.tiers.len()))
}

/// The concave tools to draw with `design`'s planes, and their `(tier, placement)`
/// list. Empty for a design without concave tiers (and so for every planar design,
/// leaving the frame byte-identical), when there is no solve to measure the stone
/// against, and when the concave tiers do not resolve (an invalid tier, too many
/// placements): the preview then shows the flat stone rather than failing, and the
/// editor's own validation reports why.
///
/// `limit` is the viewport's slider; the tools are the ones
/// [`live_update::display_tools`] picks for the planes of the same frame, so a groove is
/// never drawn into planes the slider has removed.
fn concave_tools_for_display(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    limit: CutLimit,
) -> (Vec<ToolPrimitive>, Vec<(usize, usize)>) {
    solved.map_or_else(
        || (Vec::new(), Vec::new()),
        |solved| live_update::display_tools(design, solved, limit),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::CameraPose;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use std::sync::Arc;

    /// With the slider after the stored-first crown tier of a crown-first design, the
    /// groove (a pavilion tool, cut before every crown tier) is drawn together with the
    /// pavilion planes it cuts, and the crown dimple, cut after the hidden crown tier, is not.
    #[test]
    fn concave_tools_follow_the_same_visible_tiers_as_the_planes() {
        let mut design = Design::concave_fixture();
        design.tiers.reverse();
        let solved = design.solve().expect("the fixture solves");
        let (all, _) = concave_tools_for_display(&design, Some(&solved), CutLimit::Finished);
        let (through, placements) =
            concave_tools_for_display(&design, Some(&solved), CutLimit::ThroughTier(0));
        assert_eq!(all.len(), 12, "eight groove placements and four dimples");
        assert_eq!(through.len(), 8);
        assert!(placements.iter().all(|&(tier, _)| tier == 0));
    }

    /// No solve, no tools: with no masts at all the frame draws the flat stone alone.
    #[test]
    fn no_masts_means_no_tools() {
        let design = Design::concave_fixture();
        let (tools, placements) = concave_tools_for_display(&design, None, CutLimit::Finished);
        assert!(tools.is_empty() && placements.is_empty());
    }

    /// F4-10: the plan's own masts win; without them the previous masts stand in, but only
    /// while they line up with the design's tiers.
    #[test]
    fn a_plan_that_solved_nothing_draws_from_the_previous_masts() {
        let design = Design::concave_fixture();
        let own = design.solve().expect("the fixture solves");
        let previous = design.solve().expect("the fixture solves");

        let drawn = drawn_masts(Some(&own), Some(&previous[..1]), &design);
        assert!(
            drawn.is_some_and(|masts| std::ptr::eq(masts, own.as_slice())),
            "the plan's own solve wins"
        );
        let held_over = drawn_masts(None, Some(&previous), &design);
        assert!(held_over.is_some_and(|masts| std::ptr::eq(masts, previous.as_slice())));
        assert!(
            drawn_masts(None, Some(&previous[..1]), &design).is_none(),
            "a list left over from before a tier was added or removed draws nothing"
        );
        assert!(drawn_masts(None, None, &design).is_none());
    }

    /// F4-10: an unsolvable frame still shows the previous stone, so it keeps that stone's
    /// concave tools: the grooves and dimples and their hover entries do not vanish for the
    /// frames in between. The frame still carries no masts of its own.
    #[test]
    fn an_unsolvable_frame_keeps_the_concave_tools_of_the_stone_it_shows() {
        let fixture = Design::concave_fixture();
        let previous = fixture.solve().expect("the fixture solves");
        // The same design with no tier anchored: it no longer solves.
        let mut broken = fixture.clone();
        for tier in &mut broken.tiers {
            tier.constraint = MeetConstraint::MeetExisting;
        }
        let tier_count = broken.tiers.len();
        let frame = build_planned_frame(
            PlanJob {
                design: Arc::new(broken),
                dirty: (0..tier_count).collect(),
                last_solved: Some(previous),
                camera: CameraPose {
                    yaw: 0.0,
                    pitch: 0.0,
                    distance: 5.0,
                },
                size: (16, 16),
                selected_tier: None,
                n_d: fixture.effective_refractive_index(),
                view_mode: 0,
                generation: 9,
                show_preform: true,
                enlarged_panel: -1,
                tier_cutoff: None,
                cut_steps: None,
            },
            Duration::from_secs(60),
            &ZeroClock,
        );
        assert!(
            frame.unsolvable_status.is_some(),
            "the premise: the design does not solve"
        );
        assert!(
            frame.solved.is_none(),
            "an unsolvable frame carries no masts"
        );
        assert!(!frame.planes.is_empty(), "the held-over stone is drawn");
        assert_eq!(
            frame.tools.len(),
            12,
            "eight groove placements and four dimples stay with the stone"
        );
        assert_eq!(frame.placements.len(), frame.tools.len());
    }

    /// A clock that never advances: every solve is "within budget".
    struct ZeroClock;

    impl live_update::Clock for ZeroClock {
        fn now_ms(&self) -> f64 {
            0.0
        }
    }
}
