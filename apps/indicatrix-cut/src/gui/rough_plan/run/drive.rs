//! One plan, start to finish, on scoped threads.
//!
//! This is the core's sequential `plan()` stage for stage (same stages in the same order,
//! same group order, same ranking); only the independent work inside a stage is spread
//! over lanes and merged in a fixed order. The result is therefore the core's result,
//! bit for bit, for any lane count.

use super::{
    stages::{
        ClipJob, FitJob, build_clipped_table_parallel, fit_single_stones_parallel, refine_parallel,
        run_orders,
    },
    tracker::{Note, Progress},
};
use indicatrix_cut_core::rough_plan::{
    CandidateDesign, CutOrder, FINAL_LAYOUTS, LayoutGroup, PLAIN_SINGLE_FITS, PlanInput, PlanPath,
    PlanProgress, PlanSettings, REFINE_TOP, RoughBlock, RoughLayout, SHAPED_SINGLE_FITS,
    SHAPED_SINGLE_LAYOUTS, best_layout, build_piece_table_lanes, choose_grid, final_ranking_within,
    finish_plan_lanes, flatten_groups, merge_and_rank_within, own_pool, pareto_front,
    partial_ranking_within, plan_alternatives_lanes, plan_design_cap, plan_rough_for_order,
    rank_indices_within,
    shaped::{
        ShapedAltParams, ShapedCtx, ShapedGrid, grid_poll_events, plan_shaped_alternatives,
        plan_shaped_design_cap, plan_shaped_for_order, refine_shaped_capped,
        shaped_uniform_layouts,
    },
    single_fit_layouts, uniform_layouts_lanes, uniform_pool,
};

/// DP ticks the plain planner reports per cell of the first cut axis and order.
const PLAIN_TICKS_PER_CELL: usize = 1;

/// DP ticks the shaped planner reports per cell of the first cut axis and order.
const SHAPED_TICKS_PER_CELL: usize = 2;

/// Plans the rough of `input` on up to `lanes` threads. `None` when `progress` cancelled;
/// an empty list when nothing is feasible (or the model is invalid: the caller validates
/// it first to say so).
///
/// `input.designs` must be valid and sorted by entry id, as `candidates_from` yields
/// them.
pub(super) fn drive(
    input: &PlanInput<'_>,
    lanes: usize,
    progress: &dyn Progress,
) -> Option<Vec<RoughLayout>> {
    match input.path() {
        PlanPath::PlainBlock => plan_plain_block(input, lanes, progress),
        PlanPath::Shaped => plan_shaped(input, lanes, progress),
    }
}

/// The DP ticks of all six orders on `cells`.
fn dp_ticks(cells: [usize; 3], ticks_per_cell: usize) -> usize {
    CutOrder::ALL
        .iter()
        .map(|order| ticks_per_cell * cells[order.axes()[0]])
        .sum()
}

/// How many times an order's DP can report its ticks: once without a stone cap, twice with
/// one (the free DP, then the count DP when the free optimum holds more stones than the cap).
/// The announced total counts both passes, so a cap that does not bind leaves the second pass
/// out and the progress stops short of the end of the DP band (about halfway when no order
/// binds); it moves on at the next stage and is never taken back.
fn dp_passes(settings: &PlanSettings) -> usize {
    1 + usize::from(settings.stone_cap().is_some())
}

/// The `Grid` events of a shaped rough's two piece tables: the size table reports one per
/// plane of the first axis; the clipped table is built one a-range at a time (see
/// `build_clipped_table_parallel`) and each such job reports one event per
/// `GRID_POLL_PIECES` entries, rounded up.
///
/// The unit is one `PlanProgress::Grid` event, and only the first two tables count: the
/// leave-one-out rounds build their own tables and report their own `Grid` events, which
/// the tracker routes to the alternatives stage once an `Alternatives` event was seen.
/// Every a-range holds `range_count(1) * range_count(2)` entries, and there are
/// `range_count(0)` of them.
const fn shaped_grid_events(grid: &ShapedGrid) -> usize {
    let per_plane = grid.range_count(1) * grid.range_count(2);
    grid.cells[0] + grid.range_count(0) * grid_poll_events(per_plane)
}

/// The plain block: the legacy stages, plus the exact single-stone fits merged into the
/// final list.
fn plan_plain_block(
    input: &PlanInput<'_>,
    lanes: usize,
    progress: &dyn Progress,
) -> Option<Vec<RoughLayout>> {
    let settings = input.settings;
    let inset = settings.skin_mm + settings.allowance_mm;
    let (Ok(region), Ok(coarse_region)) = (
        input.model.canonical_usable_halfspaces(inset),
        input.model.canonical_coarse_usable_halfspaces(inset),
    ) else {
        return Some(Vec::new());
    };
    // The front is computed once for the whole run; it is empty exactly when no design is
    // valid.
    let front = pareto_front(input.designs);
    if front.is_empty() {
        return Some(Vec::new());
    }

    let extents = input.model.base.bounding_box_extents();
    let rough = RoughBlock {
        x_mm: extents[0],
        y_mm: extents[1],
        z_mm: extents[2],
    };
    let mut layouts = plan_block_layouts(&rough, input, front, lanes, progress)?;
    let job = FitJob {
        region: &region,
        coarse_region: &coarse_region,
        hulls: input.hulls,
        settings,
        keep: PLAIN_SINGLE_FITS,
        // A plain block is never a mesh rough.
        mesh: None,
    };
    let fits = fit_single_stones_parallel(&job, lanes, progress)?;
    layouts.extend(single_fit_layouts(
        &fits,
        input.hulls,
        PLAIN_SINGLE_FITS,
        rough.volume_mm3(),
    ));
    Some(merge_and_rank_within(
        layouts,
        FINAL_LAYOUTS,
        settings.caps(),
    ))
}

/// The core's `plan_rough` over the Pareto `front`, every stage that splits on threads: the
/// piece tables, the six orders, the design-cap chain, the single-design pass and the
/// refinement.
fn plan_block_layouts(
    rough: &RoughBlock,
    input: &PlanInput<'_>,
    front: Vec<CandidateDesign>,
    lanes: usize,
    progress: &dyn Progress,
) -> Option<Vec<RoughLayout>> {
    let (settings, designs) = (input.settings, input.designs);
    let mut on_progress = |event: PlanProgress| progress.event(event);
    if !on_progress(PlanProgress::Pareto) {
        return None;
    }
    let grid = choose_grid(rough, settings);
    progress.note(Note::Grid(grid.cells()[0]));
    progress.note(Note::Dp(
        dp_ticks(grid.cells(), PLAIN_TICKS_PER_CELL) * dp_passes(settings),
    ));

    let table = build_piece_table_lanes(&grid, &front, lanes, &mut on_progress)?;
    let mixed = run_orders(lanes, progress, |order, on| {
        plan_rough_for_order(&grid, &table, &front, order, on)
    })?;
    let (capped, alternatives) = match best_layout(&mixed) {
        Some(best) => (
            plan_design_cap(
                &grid,
                designs,
                best,
                settings.max_designs_usize(),
                lanes,
                &mut on_progress,
            )?,
            // The FULL candidate list, not the front: a design dominated only by a removed
            // one may be needed once that one is gone.
            plan_alternatives_lanes(&grid, designs, best, lanes, &mut on_progress)?,
        ),
        None => (Vec::new(), Vec::new()),
    };

    let mut groups = vec![LayoutGroup {
        pool: front,
        layouts: mixed,
    }];
    groups.extend(capped);
    groups.extend(alternatives);
    if !on_progress(PlanProgress::Uniform { done: 0, total: 1 }) {
        return None;
    }
    groups.push(LayoutGroup {
        pool: Vec::new(),
        layouts: uniform_layouts_lanes(rough, settings, designs, lanes, &mut on_progress)?,
    });
    if !on_progress(PlanProgress::Uniform { done: 1, total: 1 }) {
        return None;
    }
    finish_plan_lanes(rough, settings, designs, &groups, lanes, &mut on_progress)
}

/// A shaped rough: the positional DPs over the clipped table, the design-cap chain, the
/// leave-one-out alternatives, the exact single-stone fits, the shaped uniform pass, the refinement of
/// the best layouts and the final ranking.
fn plan_shaped(
    input: &PlanInput<'_>,
    lanes: usize,
    progress: &dyn Progress,
) -> Option<Vec<RoughLayout>> {
    let mut on_progress = |event: PlanProgress| progress.event(event);
    if !on_progress(PlanProgress::Pareto) {
        return None;
    }
    let designs = input.designs;
    let front = pareto_front(designs);
    if front.is_empty() {
        return Some(Vec::new());
    }
    let settings = input.settings;
    let caps = settings.caps();
    let partial = |groups: &[LayoutGroup], extra: Vec<RoughLayout>| {
        partial_ranking_within(groups, extra, caps)
    };
    let inset = settings.skin_mm + settings.allowance_mm;
    let (Ok(ctx), Ok(coarse_region)) = (
        ShapedCtx::new(input.model, settings),
        input.model.canonical_coarse_usable_halfspaces(inset),
    ) else {
        return Some(Vec::new());
    };

    // The time limit belongs to mesh roughs alone: a convex or hull rough is never armed, so
    // none of the early exits below can happen for it.
    if ctx.fit_mesh().is_some() {
        progress.arm_deadline();
    }

    let mut groups = match shaped_dp_groups(input, &ctx, &front, lanes, progress) {
        Some(groups) => groups,
        // Stopped by the limit inside the tables or the six DPs: nothing to show yet.
        None if progress.time_stopped() => return Some(Vec::new()),
        None => return None,
    };
    if progress.time_stopped() {
        return Some(partial(&groups, Vec::new()));
    }
    let job = FitJob {
        region: &ctx.usable,
        coarse_region: &coarse_region,
        hulls: input.hulls,
        settings,
        keep: SHAPED_SINGLE_FITS,
        mesh: ctx.fit_mesh(),
    };
    let Some(single_fits) = fit_single_stones_parallel(&job, lanes, progress) else {
        return stopped_layouts(progress, || partial(&groups, Vec::new()));
    };
    let single_layouts = || {
        single_fit_layouts(
            &single_fits,
            input.hulls,
            SHAPED_SINGLE_LAYOUTS,
            ctx.model_volume,
        )
    };

    let pool = uniform_pool(&front, &single_fits, designs);
    let Some(uniforms) = shaped_uniform_layouts(&ctx, &pool, settings, lanes, &mut on_progress)
    else {
        return stopped_layouts(progress, || partial(&groups, single_layouts()));
    };
    groups.push(LayoutGroup {
        pool: Vec::new(),
        layouts: uniforms,
    });

    let (flat, group_of) = flatten_groups(&groups);
    let ranked = rank_indices_within(&flat, REFINE_TOP, caps);
    let Some(mut refined) = refine_parallel(ranked.len(), lanes, progress, |slot| {
        let index = ranked[slot];
        let group = &groups[group_of[index]];
        let pool = if group.pool.is_empty() {
            own_pool(designs, flat[index])
        } else {
            group.pool.clone()
        };
        refine_shaped_capped(&ctx, flat[index], &pool, designs, settings)
    }) else {
        return stopped_layouts(progress, || partial(&groups, single_layouts()));
    };
    refined.extend(single_layouts());
    Some(final_ranking_within(refined, &flat, &ranked, caps))
}

/// What a stage that returned `None` means: the time limit stopped the plan (the layouts
/// found so far, built by `partial`) or the user cancelled (`None`).
fn stopped_layouts(
    progress: &dyn Progress,
    partial: impl FnOnce() -> Vec<RoughLayout>,
) -> Option<Vec<RoughLayout>> {
    progress.time_stopped().then(partial)
}

/// The size table, the clipped table, the six DPs, the design-cap chain and the
/// alternatives of a shaped rough, as refinement groups in the plain planner's order
/// (mixed, design cap, alternatives).
fn shaped_dp_groups(
    input: &PlanInput<'_>,
    ctx: &ShapedCtx,
    front: &[CandidateDesign],
    lanes: usize,
    progress: &dyn Progress,
) -> Option<Vec<LayoutGroup>> {
    let settings = input.settings;
    let mut on_progress = |event: PlanProgress| progress.event(event);
    let grid = ctx.choose_grid(settings);
    progress.note(Note::Grid(shaped_grid_events(&grid)));
    progress.note(Note::Dp(
        dp_ticks(grid.cells, SHAPED_TICKS_PER_CELL) * dp_passes(settings),
    ));

    let size_table = grid.size_table_lanes(settings, front, lanes, &mut on_progress)?;
    let clip = ClipJob {
        grid: &grid,
        front,
        non_box: &ctx.non_box,
        size_table: &size_table,
        settings,
        mesh: ctx.mesh.as_deref(),
    };
    let clipped = build_clipped_table_parallel(&clip, lanes, progress)?;
    let mixed = run_orders(lanes, progress, |order, on| {
        plan_shaped_for_order(&grid, &clipped, ctx, front, order, settings, on)
    })?;
    let mut extra = Vec::new();
    if let Some(best) = best_layout(&mixed) {
        let params = ShapedAltParams {
            grid: &grid,
            table: &clipped,
            ctx,
            all_designs: input.designs,
            best,
            settings,
            lanes,
        };
        // The limit may stop either stage: the layouts found so far stand.
        match plan_shaped_design_cap(&params, &mut on_progress) {
            Some(capped) => extra.extend(capped),
            None if progress.time_stopped() => {}
            None => return None,
        }
        if !progress.time_stopped() {
            match plan_shaped_alternatives(&params, &mut on_progress) {
                Some(alternatives) => extra.extend(alternatives),
                None if progress.time_stopped() => {}
                None => return None,
            }
        }
    }

    let mut groups = vec![LayoutGroup {
        pool: front.to_vec(),
        layouts: mixed,
    }];
    groups.extend(extra);
    Some(groups)
}
