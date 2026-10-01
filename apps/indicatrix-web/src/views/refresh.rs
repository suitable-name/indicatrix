//! [`refresh`]: brings the shown Solid/Diagram frame up to date with the app --
//! see the parent module's doc comment for the replan/reproject/overlay choice.

use super::{
    VIEWS,
    manip::{self, ProvisionalPlan},
    present,
    state::{Seen, ViewsState, view_mode_for_tab},
};
use crate::{
    AppModel, AppWindow, ManipulateModel, SolidModel,
    app::{
        Ctx, coalesce_now,
        state::{DesignState, SolveState, WebApp},
    },
};
use indicatrix_cut_core::Design;
use indicatrix_editor::{
    manipulate::provisional::PROVISIONAL_GENERATION,
    solve_policy::{SolveCostEstimate, should_solve_synchronously_for},
};
use indicatrix_solid::{
    live_update::{Clock, DEFAULT_PREVIEW_BUDGET},
    preview::{
        CameraPose, FacetOverlay, PreviewPipeline, RenderedFrame,
        view::{
            RasterLimits, ReplanBasis, ReplanInputs, diagram_raster_size, facets_of_tiers,
            plan_job, plans_without_solver, raster_size_for_view, replan_basis, with_min_aspect,
        },
    },
};
use slint::ComponentHandle;
use std::sync::Arc;

/// The raster caps: the render view's own (`crate::render::viewport`), applied
/// uniformly so the raster keeps the view's aspect.
const LIMITS: RasterLimits = RasterLimits {
    max_device_pixel_ratio: crate::render::viewport::MAX_DEVICE_PIXEL_RATIO,
    min_edge: crate::render::viewport::MIN_RENDER_DIM,
    max_edge: crate::render::viewport::MAX_RENDER_DIM,
};

/// The Diagram's raster caps: larger than [`LIMITS`], because the diagram's facet labels
/// are 1-pixel-per-glyph bitmap text laid out from the facets' size in raster pixels.
const DIAGRAM_LIMITS: RasterLimits = RasterLimits {
    max_edge: 1600,
    ..LIMITS
};

/// The narrowest Diagram raster with all three panels, in pixels: about the width of the
/// desktop's diagram at its usual window size. A narrower view (this page's dock takes
/// 400px of it) draws the same raster and shows it scaled down, instead of a small raster
/// on which the labels of neighbouring facets pile up.
const DIAGRAM_MIN_WIDTH: u32 = 1200;

/// The narrowest Diagram raster with ONE enlarged panel: that panel has the whole raster
/// to itself, so a raster near the width the label placement is tuned for (about 700
/// pixels, `indicatrix_solid::diagram2d`'s `MIN_LABEL_SPAN`) already keeps the labels apart
/// and, shown nearly 1:1 in a small window, readable. The Diagram tab opens on the Crown
/// panel in a view narrower than 800 logical pixels (`small-view` in `ui/views/diagram.slint`).
const DIAGRAM_MIN_WIDTH_ENLARGED: u32 = 720;

/// The narrowest aspect (width over height) the Solid raster is drawn at: 16:10, about the
/// view a desktop window gives it. The camera's field of view is vertical and the default
/// pose fills roughly 90% of the width at 1.5:1, so a narrower raster crops the stone's
/// sides; a narrower view instead gets a 16:10 raster letterboxed by `image-fit: contain`
/// (the whole stone stays visible).
const SOLID_MIN_ASPECT: f32 = 1.6;

/// The raster size for a view of `width x height` logical pixels in `view_mode`, with the
/// Diagram's panel enlarged or not (`enlarged`).
fn raster_size(
    view_mode: u8,
    enlarged: bool,
    width: f32,
    height: f32,
    scale_factor: f32,
) -> (u32, u32) {
    if view_mode == 3 {
        diagram_raster_size(
            width,
            height,
            scale_factor,
            DIAGRAM_LIMITS,
            if enlarged {
                DIAGRAM_MIN_WIDTH_ENLARGED
            } else {
                DIAGRAM_MIN_WIDTH
            },
        )
    } else {
        with_min_aspect(
            raster_size_for_view(width, height, scale_factor, LIMITS),
            SOLID_MIN_ASPECT,
            LIMITS.min_edge,
        )
    }
}

/// `performance.now()` as the planner's budget clock.
struct PerfClock;

impl Clock for PerfClock {
    fn now_ms(&self) -> f64 {
        coalesce_now().as_secs_f64() * 1000.0
    }
}

/// What [`refresh`] must do once every borrow is released.
enum After {
    /// Nothing.
    Nothing,
    /// A `Stale` frame's follow-up replan, on the next event-loop turn.
    FollowUp,
    /// Ask the app for a solve of the current design.
    RequestSolve,
}

/// Brings the Solid/Diagram view up to date with the app (a no-op on the Render
/// tab, without a design, or when nothing it depends on changed).
pub fn refresh(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let Some(view_mode) = view_mode_for_tab(ui.global::<AppModel>().get_view_tab()) else {
        return;
    };
    // A provisional slice cannot outlive an edit of the design or another selection.
    manip::expire(ctx);
    let after = {
        let app = ctx.state.borrow();
        VIEWS.with(|cell| {
            let mut views = cell.borrow_mut();
            update(&ui, &app, &mut views, view_mode)
        })
    };
    match after {
        After::Nothing => {}
        After::FollowUp => super::request_refresh(ctx),
        After::RequestSolve => {
            crate::app::solve::with_solved(ctx, |ctx, _| super::request_refresh(ctx));
        }
    }
}

/// `(solved, failed)` generations of `solve`.
const fn solve_key(solve: &SolveState) -> (Option<u64>, Option<u64>) {
    match solve {
        SolveState::NotSolved => (None, None),
        SolveState::Solved { generation, .. } => (Some(*generation), None),
        SolveState::Failed { generation, .. } => (None, Some(*generation)),
    }
}

/// The app state a frame would reflect right now.
fn seen_now(
    ui: &AppWindow,
    app: &WebApp,
    design: &DesignState,
    views: &ViewsState,
    view_mode: u8,
) -> Seen {
    let model = ui.global::<SolidModel>();
    let tier_count = design.session.design.tiers.len();
    Seen {
        generation: design.session.current_generation(),
        solve: solve_key(&app.solve),
        selected_tier: app.selected_tier.filter(|&t| t < tier_count),
        multi_selected: design.session.multi_selected.clone(),
        custom_materials: app.custom_materials.len(),
        show_preform: model.get_show_preform(),
        tier_cutoff: model.get_tier_cutoff(),
        enlarged_panel: model.get_enlarged_panel(),
        view_mode,
        camera: CameraPose {
            yaw: app.view.yaw,
            pitch: app.view.pitch,
            distance: app.view.distance,
        },
        size: raster_size(
            view_mode,
            model.get_enlarged_panel() >= 0,
            views.view_size.0,
            views.view_size.1,
            ui.window().scale_factor(),
        ),
    }
}

/// [`refresh`]'s body, under the `WebApp` and views borrows.
fn update(ui: &AppWindow, app: &WebApp, views: &mut ViewsState, view_mode: u8) -> After {
    let Some(design) = &app.design else {
        return After::Nothing;
    };
    let now = seen_now(ui, app, design, views, view_mode);
    let planned_before = views.pipeline.memory().planes.is_some();
    let (replan, reproject, overlay) = match &views.seen {
        Some(previous) if planned_before && !views.force_replan => (
            previous.needs_replan(&now),
            previous.needs_reproject(&now),
            previous.multi_selected != now.multi_selected || views.overlay_dirty,
        ),
        _ => (true, false, false),
    };
    if !(replan || reproject || overlay) {
        return After::Nothing;
    }
    let start = coalesce_now();
    // Consumed here: the direct-manipulation tools may dirty it again once the frame
    // has landed (a new outline), which then owes one more overlay pass.
    views.overlay_dirty = false;
    let after = if replan {
        replan_now(ui, app, design, views, &now)
    } else {
        let gear = gear_of(design);
        let mut frame = if reproject {
            views
                .pipeline
                .reproject(now.camera, now.size, view_mode, Some(gear))
        } else {
            None
        };
        if overlay {
            views.overlay.multi_selected = facets_of_tiers(&views.facet_tier, &now.multi_selected);
            frame = views
                .pipeline
                .update_overlay(views.overlay.clone())
                .or(frame);
        }
        if let Some(frame) = frame {
            present::show_frame(ui, views, frame, view_mode);
            manip::frame_landed(ui, app, views, view_mode);
        }
        views.seen = Some(now);
        After::Nothing
    };
    let took = coalesce_now().saturating_sub(start);
    present::show_timing(ui, took);
    if matches!(after, After::Nothing) && views.overlay_dirty {
        return After::FollowUp;
    }
    after
}

/// The loaded design's gear, for a Diagram reproject (the desktop's
/// `RenderContext::design_gear`).
const fn gear_of(design: &DesignState) -> (u32, f32) {
    let meta = &design.session.design.meta;
    (meta.gear_teeth_abs(), meta.gear_reference_angle as f32)
}

/// Whether a full solve of `design` may run right here on the main thread: the
/// desktop's synchronous-solve rule (few planes, or a fast last solve), or every
/// tier pinned (no solver call at all).
fn full_solve_allowed(app: &WebApp, design: &DesignState) -> bool {
    full_solve_allowed_for(app, &design.session.design)
}

/// [`full_solve_allowed`] for any design (the Slice tool's provisional clone).
fn full_solve_allowed_for(app: &WebApp, design: &Design) -> bool {
    if plans_without_solver(design) {
        return true;
    }
    let last = match &app.solve {
        SolveState::Solved { took, .. } => Some(*took),
        _ => None,
    };
    should_solve_synchronously_for(SolveCostEstimate::of(design), last)
}

/// A replan: plan (budgeted), rasterize, re-apply the highlight, and schedule a
/// `Stale` frame's follow-up -- or, with no usable masts for a design too large to
/// solve here, keep the last frame and ask for a solve.
fn replan_now(
    ui: &AppWindow,
    app: &WebApp,
    design: &DesignState,
    views: &mut ViewsState,
    now: &Seen,
) -> After {
    views.force_replan = false;
    // While the Slice tool holds a provisional tier, what is drawn is that design.
    if views.manip.has_provisional() {
        return replan_provisional(ui, app, views, now);
    }
    let basis = replan_basis(
        &design.session.design,
        app.current_solved(),
        views
            .cache
            .as_ref()
            .map(|(cached, masts)| (cached.as_ref(), masts.as_slice())),
    );
    if matches!(basis, ReplanBasis::FullSolve) && !full_solve_allowed(app, design) {
        views.seen = Some(now.clone());
        let failed = matches!(&app.solve, SolveState::Failed { generation, .. } if *generation == now.generation);
        present::show_waiting(ui, app, now.generation);
        let ask = !failed && views.solve_requested_for != Some(now.generation);
        views.solve_requested_for = Some(now.generation);
        return if ask {
            After::RequestSolve
        } else {
            After::Nothing
        };
    }
    let design_arc = Arc::new(design.session.design.clone());
    let job = plan_job(ReplanInputs {
        design: Arc::clone(&design_arc),
        generation: now.generation,
        basis,
        camera: now.camera,
        size: now.size,
        selected_tier: now.selected_tier,
        custom_materials: &app.custom_materials,
        view_mode: now.view_mode,
        show_preform: now.show_preform,
        enlarged_panel: now.enlarged_panel,
        tier_cutoff: now.tier_cutoff,
    });
    let planned = PreviewPipeline::plan(job, DEFAULT_PREVIEW_BUDGET, &PerfClock);
    let stale = planned.stale;
    // An unsolvable frame chains the OLD masts forward; they belong to the design
    // they were solved for, so the cache keeps that design as the diff base.
    if planned.unsolvable_status.is_none()
        && let Some(solved) = &planned.solved
    {
        views.cache = Some((design_arc, solved.clone()));
        views.cache_rev += 1;
        views.cache_generation = now.generation;
    }
    let generation_moved = views
        .seen
        .as_ref()
        .is_none_or(|previous| previous.generation != now.generation);
    let Some(frame) = views.pipeline.render_planned(planned) else {
        return After::Nothing;
    };
    let frame = reapply_overlay(views, frame, &now.multi_selected, generation_moved);
    present::show_frame(ui, views, frame, now.view_mode);
    views.seen = Some(now.clone());
    manip::frame_landed(ui, app, views, now.view_mode);
    if stale && views.followup_for != Some(now.generation) {
        views.followup_for = Some(now.generation);
        views.force_replan = true;
        return After::FollowUp;
    }
    After::Nothing
}

/// The hint when a slice cannot be previewed here: the provisional design needs a full
/// solve, which would freeze the page for a design this large.
const SLICE_TOO_LARGE_HINT: &str = "This design is too large to preview a slice on this page. Add the tier from the tier table instead.";

/// The replan while the Slice tool holds a provisional tier: plans the session's design
/// (the committed design plus the new tier) under `PROVISIONAL_GENERATION`, so the frame
/// stays out of the solve cache, and hands the solved masts back to the session (the
/// handles and the green outline are placed from them). Chains from the session's own
/// masts once it has any; the first frame chains from the committed masts plus the new
/// tier's pinned mast, and only a design with neither is solved in full -- and only when
/// that is cheap enough to do on this page.
fn replan_provisional(ui: &AppWindow, app: &WebApp, views: &mut ViewsState, now: &Seen) -> After {
    let committed = views.cache.as_ref().map(|(_, masts)| masts.as_slice());
    let Some(ProvisionalPlan {
        design,
        last_solved,
        dirty,
    }) = views.manip.provisional_inputs(committed)
    else {
        return After::Nothing;
    };
    let basis = last_solved.map_or(ReplanBasis::FullSolve, |last_solved| ReplanBasis::Chain {
        last_solved,
        dirty,
    });
    if matches!(basis, ReplanBasis::FullSolve) && !full_solve_allowed_for(app, &design) {
        views.seen = Some(now.clone());
        ui.global::<ManipulateModel>()
            .set_hint_text(SLICE_TOO_LARGE_HINT.into());
        return After::Nothing;
    }
    let job = plan_job(ReplanInputs {
        design,
        generation: PROVISIONAL_GENERATION,
        basis,
        camera: now.camera,
        size: now.size,
        selected_tier: now.selected_tier,
        custom_materials: &app.custom_materials,
        view_mode: now.view_mode,
        show_preform: now.show_preform,
        enlarged_panel: now.enlarged_panel,
        tier_cutoff: now.tier_cutoff,
    });
    let planned = PreviewPipeline::plan(job, DEFAULT_PREVIEW_BUDGET, &PerfClock);
    let stale = planned.stale;
    let masts = planned
        .unsolvable_status
        .is_none()
        .then(|| planned.solved.clone())
        .flatten();
    let Some(frame) = views.pipeline.render_planned(planned) else {
        return After::Nothing;
    };
    if let Some(masts) = masts {
        views.manip.note_provisional_masts(masts);
    }
    let frame = reapply_overlay(views, frame, &now.multi_selected, false);
    present::show_frame(ui, views, frame, now.view_mode);
    views.seen = Some(now.clone());
    manip::frame_landed(ui, app, views, now.view_mode);
    // A `Stale` frame drew the previous planes; one more replan (chained from the fresh
    // masts it just noted) catches the picture up.
    if views.manip.wants_followup(stale) {
        views.force_replan = true;
        return After::FollowUp;
    }
    After::Nothing
}

/// A planned frame starts with no hover/click/multi-select highlight (the
/// desktop's `Planned` style); put the current one back. Facet ids from before an
/// edit may name other facets now, so hover and the clicked facet are dropped when
/// the generation moved; the multi-selection is re-resolved from the new frame's
/// own facet -> tier table.
fn reapply_overlay(
    views: &mut ViewsState,
    frame: RenderedFrame,
    multi_selected: &std::collections::BTreeSet<usize>,
    generation_moved: bool,
) -> RenderedFrame {
    if generation_moved {
        views.overlay.hovered = None;
        views.overlay.selected_facet = None;
    }
    views.overlay.multi_selected = facets_of_tiers(&frame.facet_tier, multi_selected);
    if views.overlay == FacetOverlay::default() {
        return frame;
    }
    match views.pipeline.update_overlay(views.overlay.clone()) {
        // An overlay frame reports neither staleness nor the planner's status; the
        // planned frame's still describe what is on screen.
        Some(overlaid) => RenderedFrame {
            stale: frame.stale,
            status: frame.status,
            ..overlaid
        },
        None => frame,
    }
}
