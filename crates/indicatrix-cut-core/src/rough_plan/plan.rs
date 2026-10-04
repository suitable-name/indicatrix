//! Unified entry point for rough planning across plain block and shaped rough geometries.

use glam::DVec3;

use super::{
    Axis, BarCut, CandidateDesign, CutOrder, CutPlan, DesignHull, FINAL_TOP, LayoutGroup,
    PlacedStone, PlanProgress, PlanSettings, REFINE_TOP, RoughBlock, RoughLayout, RoughModel,
    SHAPED_UNIFORM_DESIGNS, SingleFit, SlabCut, fit_single_stones, fit_single_stones_with,
    merge_and_rank,
    pareto::{pareto_front, sanitize},
    plan_rough,
    rank::{best_layout, flatten_groups, rank_indices},
    refine::{final_ranking, own_pool},
    shape::RoughBase,
    shaped::{
        BuildClipParams, ShapedAltParams, ShapedCtx, build_clipped_table, choose_shaped_grid_at,
        plan_shaped_alternatives, plan_shaped_for_order, refine_shaped, shaped_uniform_layouts,
    },
};

/// Single-stone fits turned into layouts for the plain block's final list: as many as the
/// list holds ([`FINAL_TOP`]).
pub const PLAIN_SINGLE_FITS: usize = FINAL_TOP;
/// Single-stone fits computed for a shaped rough (they also seed the uniform pass): twice
/// the final list ([`FINAL_TOP`]), so the uniform pass has fitted designs to start from.
pub const SHAPED_SINGLE_FITS: usize = 2 * FINAL_TOP;
/// Single-stone fits turned into layouts for the shaped rough's final list: as many as the
/// list holds ([`FINAL_TOP`]).
pub const SHAPED_SINGLE_LAYOUTS: usize = FINAL_TOP;
/// Layouts in the final list of the plain block path ([`FINAL_TOP`]).
pub const FINAL_LAYOUTS: usize = FINAL_TOP;
/// Lanes `plan` gives the shaped stages: one, so everything runs on the calling thread and the
/// standalone entry point needs no threads (it also builds for `wasm32`). The result does not
/// depend on the lane count; an application that wants more drives the stages itself.
const SEQUENTIAL_LANES: usize = 1;

/// All inputs required to plan rough cuts.
#[derive(Debug, Clone)]
pub struct PlanInput<'a> {
    /// The modelled rough geometry.
    pub model: &'a RoughModel,
    /// Settings controlling kerf, allowance, skin, limits, and density.
    pub settings: &'a PlanSettings,
    /// All candidate designs to choose from.
    pub designs: &'a [CandidateDesign],
    /// Cached 3D convex outline hulls for exact single-stone fitting.
    pub hulls: &'a [DesignHull],
}

/// The execution path selected for a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanPath {
    /// Plain axis-aligned block without cuts.
    PlainBlock,
    /// Shaped rough (cut block, cylinder, or pebble).
    Shaped,
}

impl PlanInput<'_> {
    /// Determines whether the input uses the plain block or shaped planning path.
    #[must_use]
    pub const fn path(&self) -> PlanPath {
        match self.model.base {
            RoughBase::Block { .. } if self.model.cuts.is_empty() => PlanPath::PlainBlock,
            _ => PlanPath::Shaped,
        }
    }
}

/// Converts a single-stone exact fit into a 1-stone [`RoughLayout`].
///
/// The stone's piece is its bounding box in the rough frame, so the layout's
/// cut plan is one slab, one bar and one piece of that box's size.
#[must_use]
pub fn layout_from_single_fit(
    fit: &SingleFit,
    hull: &DesignHull,
    model_volume: f64,
) -> RoughLayout {
    let c = DVec3::from(fit.pose.center_mm);
    let ax = DVec3::from(fit.pose.axes[0]);
    let ay = DVec3::from(fit.pose.axes[1]);
    let az = DVec3::from(fit.pose.axes[2]);
    let s = fit.pose.mm_per_unit;

    let mut min_p = c;
    let mut max_p = c;
    if let Some((&first, rest)) = hull.vertices.split_first() {
        let place = |v: [f64; 3]| c + s * (v[0] * ax + v[1] * ay + v[2] * az);
        min_p = place(first);
        max_p = min_p;
        for &v in rest {
            let p = place(v);
            min_p = min_p.min(p);
            max_p = max_p.max(p);
        }
    }

    let stone_size_mm = (max_p - min_p).max(DVec3::ZERO).to_array();

    let dot_x = ay.x.abs();
    let dot_y = ay.y.abs();
    let dot_z = ay.z.abs();
    let table_axis = if dot_x >= dot_y && dot_x >= dot_z {
        Axis::X
    } else if dot_y >= dot_x && dot_y >= dot_z {
        Axis::Y
    } else {
        Axis::Z
    };

    let stone = PlacedStone {
        entry_id: fit.entry_id,
        piece_origin_mm: min_p.to_array(),
        piece_size_mm: stone_size_mm,
        stone_size_mm,
        table_axis,
        carat: fit.carat,
        volume_mm3: fit.volume_mm3,
        pose: fit.pose,
    };

    let cut_plan = CutPlan {
        slabs: vec![SlabCut {
            thickness_mm: stone_size_mm[0],
            bars: vec![BarCut {
                width_mm: stone_size_mm[1],
                pieces_mm: vec![stone_size_mm[2]],
            }],
        }],
    };

    let total_volume = fit.volume_mm3;
    let yield_fraction = if model_volume > 0.0 {
        total_volume / model_volume
    } else {
        0.0
    };

    RoughLayout {
        cut_order: CutOrder::Xyz,
        stones: vec![stone],
        cut_plan,
        total_carat: fit.carat,
        total_volume_mm3: total_volume,
        yield_fraction,
        exact_fit: true,
    }
}

/// The first `keep` fits as 1-stone layouts. A fit whose hull is missing is skipped and does
/// not use up one of the `keep` places: the following fits move up.
#[must_use]
pub fn single_fit_layouts(
    fits: &[SingleFit],
    hulls: &[DesignHull],
    keep: usize,
    model_volume: f64,
) -> Vec<RoughLayout> {
    fits.iter()
        .filter_map(|fit| {
            hulls
                .iter()
                .find(|h| h.entry_id == fit.entry_id)
                .map(|hull| layout_from_single_fit(fit, hull, model_volume))
        })
        .take(keep)
        .collect()
}

/// Executes the complete planning pipeline for the given input.
///
/// Dispatches to either the plain block or shaped planning path based on the model geometry.
/// Returns up to 10 ranked layouts ([`FINAL_TOP`]), with at most three per design set (see
/// [`plan_rough`]). The result is `None` only when cancelled via `on_progress`; invalid
/// settings ([`PlanSettings::validate`]), an invalid or empty model (the app validates the
/// model before calling), no valid design, or a rough too small for any stone all give
/// `Some(Vec::new())`.
pub fn plan(
    input: &PlanInput<'_>,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<RoughLayout>> {
    if input.settings.validate().is_err() {
        return Some(Vec::new());
    }
    match input.path() {
        PlanPath::PlainBlock => plan_plain_block(input, on_progress),
        PlanPath::Shaped => plan_shaped(input, on_progress),
    }
}

/// The plain block: exactly [`plan_rough`] (same stages, same progress events),
/// with the exact single-stone fits merged into its final list.
fn plan_plain_block(
    input: &PlanInput<'_>,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<RoughLayout>> {
    let inset = input.settings.skin_mm + input.settings.allowance_mm;
    let (Ok(region), Ok(coarse_region)) = (
        input.model.canonical_usable_halfspaces(inset),
        input.model.canonical_coarse_usable_halfspaces(inset),
    ) else {
        return Some(Vec::new());
    };
    if sanitize(input.designs).is_empty() {
        return Some(Vec::new());
    }

    let extents = input.model.base.bounding_box_extents();
    let rough = RoughBlock {
        x_mm: extents[0],
        y_mm: extents[1],
        z_mm: extents[2],
    };
    if rough.validate().is_err() {
        return Some(Vec::new());
    }
    let mut layouts = plan_rough(&rough, input.settings, input.designs, on_progress)?;
    let single_fits = fit_single_stones(
        &region,
        &coarse_region,
        input.hulls,
        input.settings,
        PLAIN_SINGLE_FITS,
        on_progress,
    )?;
    layouts.extend(single_fit_layouts(
        &single_fits,
        input.hulls,
        PLAIN_SINGLE_FITS,
        rough.volume_mm3(),
    ));
    Some(merge_and_rank(layouts, FINAL_LAYOUTS))
}

/// The size table, clipped table, six DPs and alternatives of a shaped rough,
/// as refinement groups in the order of the plain planner (mixed, alternatives).
fn run_shaped_dp(
    input: &PlanInput<'_>,
    ctx: &ShapedCtx,
    clean: &[CandidateDesign],
    front: &[CandidateDesign],
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<LayoutGroup>> {
    let settings = input.settings;
    let grid = choose_shaped_grid_at(ctx.bbox_min, ctx.bbox_extents, settings);
    let size_table = grid.size_table(settings, front, on_progress)?;

    let clip_params = BuildClipParams {
        grid: &grid,
        front,
        non_box_planes: &ctx.non_box,
        mesh: ctx.mesh.as_deref(),
        size_table: &size_table,
        settings,
        slice: 0..grid.cells[0],
        cached_classes: None,
    };
    let clipped = build_clipped_table(&clip_params, on_progress)?;

    let mut mixed = Vec::new();
    for order in CutOrder::ALL {
        mixed.extend(plan_shaped_for_order(
            &grid,
            &clipped,
            ctx,
            front,
            order,
            settings,
            on_progress,
        )?);
    }

    let alternatives = match best_layout(&mixed) {
        Some(best) => {
            let alt_params = ShapedAltParams {
                grid: &grid,
                table: &clipped,
                ctx,
                all_designs: clean,
                best,
                settings,
                lanes: SEQUENTIAL_LANES,
            };
            plan_shaped_alternatives(&alt_params, on_progress)?
        }
        None => Vec::new(),
    };

    let mut groups = vec![LayoutGroup {
        pool: front.to_vec(),
        layouts: mixed,
    }];
    groups.extend(alternatives);
    Some(groups)
}

/// The designs of the shaped uniform pass: the best single-stone fits first,
/// then the Pareto front, deduplicated by `entry_id` and capped at
/// [`SHAPED_UNIFORM_DESIGNS`] (64).
#[must_use]
pub fn uniform_pool(
    front: &[CandidateDesign],
    fits: &[SingleFit],
    clean: &[CandidateDesign],
) -> Vec<CandidateDesign> {
    let mut pool: Vec<CandidateDesign> = Vec::new();
    let fitted = fits
        .iter()
        .filter_map(|fit| clean.iter().find(|d| d.entry_id == fit.entry_id));
    for design in fitted.chain(front) {
        if !pool.iter().any(|p| p.entry_id == design.entry_id) {
            pool.push(*design);
        }
    }
    pool.truncate(SHAPED_UNIFORM_DESIGNS);
    pool
}

fn plan_shaped(
    input: &PlanInput<'_>,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<RoughLayout>> {
    if !on_progress(PlanProgress::Pareto) {
        return None;
    }

    let clean = sanitize(input.designs);
    let front = pareto_front(&clean);
    if front.is_empty() {
        return Some(Vec::new());
    }

    let inset = input.settings.skin_mm + input.settings.allowance_mm;
    let (Ok(ctx), Ok(coarse_region)) = (
        ShapedCtx::new(input.model, input.settings),
        input.model.canonical_coarse_usable_halfspaces(inset),
    ) else {
        return Some(Vec::new());
    };

    let mut groups = run_shaped_dp(input, &ctx, &clean, &front, on_progress)?;
    let single_fits = fit_single_stones_with(
        &ctx.usable,
        &coarse_region,
        input.hulls,
        input.settings,
        SHAPED_SINGLE_FITS,
        ctx.fit_mesh(),
        on_progress,
    )?;

    let pool = uniform_pool(&front, &single_fits, &clean);
    let uniforms =
        shaped_uniform_layouts(&ctx, &pool, input.settings, SEQUENTIAL_LANES, on_progress)?;
    groups.push(LayoutGroup {
        pool: Vec::new(),
        layouts: uniforms,
    });

    finish_shaped(input, &ctx, &clean, &groups, &single_fits, on_progress)
}

/// Refines the best 20 candidates of `groups`, adds the exact single-stone
/// fits and returns the final ranked list (topped up like the plain planner's).
///
/// A group with an empty pool restricts each of its layouts to its own designs.
fn finish_shaped(
    input: &PlanInput<'_>,
    ctx: &ShapedCtx,
    clean: &[CandidateDesign],
    groups: &[LayoutGroup],
    single_fits: &[SingleFit],
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<RoughLayout>> {
    let (flat, group_of) = flatten_groups(groups);
    let ranked = rank_indices(&flat, REFINE_TOP);
    let mut refined = Vec::with_capacity(ranked.len() + SHAPED_SINGLE_LAYOUTS);
    for &idx in &ranked {
        if !on_progress(PlanProgress::Refine) {
            return None;
        }
        let group = &groups[group_of[idx]];
        let pool = if group.pool.is_empty() {
            own_pool(clean, flat[idx])
        } else {
            sanitize(&group.pool)
        };
        refined.push(refine_shaped(ctx, flat[idx], &pool, input.settings));
    }

    refined.extend(single_fit_layouts(
        single_fits,
        input.hulls,
        SHAPED_SINGLE_LAYOUTS,
        ctx.model_volume,
    ));
    Some(final_ranking(refined, &flat, &ranked))
}
