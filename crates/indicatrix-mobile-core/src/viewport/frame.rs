//! The viewport's raster work: a [`RasterJob`] describes one picture, a
//! [`RasterWorker`] turns it into a [`RasterFrame`] with the same `indicatrix_solid`
//! pipeline the web app and the desktop run.
//!
//! The worker is stateful, exactly like the desktop's render thread: it keeps the
//! planner/rasterizer pipeline, the mesh cache and the masts of the last committed
//! design, and decides per job whether the picture needs a **replan** (the design, its
//! solve, the selection or the diagram layout changed), a **reproject** (only the camera,
//! size or view mode changed) or an **overlay** pass (only highlights changed). A replan
//! that overruns the preview budget comes back `stale` (the previous planes with the
//! edited tiers outlined) and is followed by one more pass from the late result.
//!
//! The worker never touches the UI. The app keeps one worker behind a mutex on a
//! dedicated [`BackgroundQueue`](crate::tasks::BackgroundQueue) and hands every job a
//! `Generation` token; a job that a newer one has superseded is skipped before it does
//! any work.

use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};

use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;
use indicatrix_editor::{
    manipulate::{
        HandleTarget,
        target::{masts_aligned, target_for},
    },
    solve_policy::{SolveCostEstimate, should_solve_synchronously_for},
};
use indicatrix_solid::{
    diagram2d::DiagramFrame,
    facet_map::FacetMap,
    live_update::{Clock, DEFAULT_PREVIEW_BUDGET},
    preview::{
        CameraPose, DEFAULT_MESH_BOUNDING_RADIUS, FacetOverlay, FrameGeometry, PickBuffer,
        PreviewPipeline, RenderedFrame,
        view::{
            RasterLimits, ReplanBasis, ReplanInputs, diagram_raster_size, facets_of_tiers,
            plan_job, plans_without_solver, raster_size_for_view, replan_basis, with_min_aspect,
        },
    },
};

use super::ViewMode;

/// The longest raster edge on a phone (a compact window) by default.
pub const DEFAULT_MAX_EDGE: u32 = 1280;
/// The longest raster edge on a medium or expanded window (a tablet).
pub const LARGE_MAX_EDGE: u32 = 1600;
/// The longest raster edge in battery saver mode.
pub const BATTERY_SAVER_MAX_EDGE: u32 = 960;
/// The largest device pixel ratio honored (the raster cost grows with its square). The
/// picture is drawn at the window's scale factor up to this, then shown at logical size.
pub const MAX_DEVICE_PIXEL_RATIO: f32 = 3.0;

/// The longest raster edge for a window class and the battery saver setting.
#[must_use]
pub const fn max_edge_for(compact: bool, battery_saver: bool) -> u32 {
    if battery_saver {
        BATTERY_SAVER_MAX_EDGE
    } else if compact {
        DEFAULT_MAX_EDGE
    } else {
        LARGE_MAX_EDGE
    }
}
/// The smallest raster edge, in pixels.
pub const MIN_EDGE: u32 = 64;
/// The narrowest aspect (width over height) the Solid raster is drawn at.
///
/// The camera's field of view is vertical, so a narrower raster would crop the stone's sides; a
/// narrower view letterboxes a raster of this aspect instead.
pub const SOLID_MIN_ASPECT: f32 = 1.6;
/// The narrowest Diagram raster with all three panels (keeps neighbouring labels apart).
pub const DIAGRAM_MIN_WIDTH: u32 = 1200;
/// The narrowest Diagram raster with one enlarged panel.
pub const DIAGRAM_MIN_WIDTH_ENLARGED: u32 = 720;

/// The raster size for a view of `view` logical px on a display with `scale_factor`.
#[must_use]
pub fn raster_size(
    mode: ViewMode,
    enlarged: bool,
    view: (f32, f32),
    scale_factor: f32,
    max_edge: u32,
) -> (u32, u32) {
    let limits = RasterLimits {
        max_device_pixel_ratio: MAX_DEVICE_PIXEL_RATIO,
        min_edge: MIN_EDGE,
        max_edge: max_edge.max(MIN_EDGE),
    };
    match mode {
        ViewMode::Diagram => diagram_raster_size(
            view.0,
            view.1,
            scale_factor,
            limits,
            if enlarged {
                DIAGRAM_MIN_WIDTH_ENLARGED
            } else {
                DIAGRAM_MIN_WIDTH
            },
        ),
        ViewMode::Solid | ViewMode::Render => with_min_aspect(
            raster_size_for_view(view.0, view.1, scale_factor, limits),
            SOLID_MIN_ASPECT,
            MIN_EDGE,
        ),
    }
}

/// The status of a [`FrameKind::Waiting`] frame.
pub const WAITING_STATUS: &str = "Solve the design to see it in the viewport.";

/// Everything one picture depends on. Built on the UI thread, run on the worker.
#[derive(Debug, Clone)]
pub struct RasterJob {
    /// The queue token: the job runs only while it is still the newest.
    pub token: u64,
    /// The design to draw: the committed design, or the Slice tool's provisional clone.
    pub design: Arc<Design>,
    /// The design generation (`PROVISIONAL_GENERATION` for a provisional design).
    pub generation: u64,
    /// The masts of a solve that is current for `generation`, if there is one.
    pub solved: Option<Vec<SolvedTier>>,
    /// For a provisional design: the masts to chain from and the tiers to re-solve.
    pub chain: Option<(Vec<SolvedTier>, BTreeSet<usize>)>,
    /// The orbit camera.
    pub camera: CameraPose,
    /// The raster size in pixels.
    pub size: (u32, u32),
    /// Solid or Diagram.
    pub mode: ViewMode,
    /// The selected tier (tinted in the picture and carrying the handles).
    pub selected_tier: Option<usize>,
    /// The multi-selected tiers (outlined).
    pub multi_selected: BTreeSet<usize>,
    /// The facet the last tap or long press landed on (outlined).
    pub selected_facet: Option<u32>,
    /// The provisional tier's facets (outlined green).
    pub provisional_facets: Vec<u32>,
    /// The facets of tiers that follow a handle drag (outlined orange).
    pub moved_facets: Vec<u32>,
    /// The enlarged diagram panel (`-1` for all three).
    pub enlarged_panel: i32,
    /// The facet the handles sit on, when it belongs to the selected tier.
    pub remembered_facet: Option<u32>,
    /// Whether to work out where the handles go.
    pub want_handles: bool,
    /// Whether `design` is the Slice tool's provisional clone.
    pub provisional: bool,
}

/// What a frame carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// A finished picture.
    Image,
    /// The design needs a solve that cannot run here; the last picture stays up.
    Waiting,
}

/// One finished picture with the tables picking and the handles need.
#[derive(Debug)]
pub struct RasterFrame {
    /// Whether this is a picture or a request to wait.
    pub kind: FrameKind,
    /// The token of the job it answers.
    pub token: u64,
    /// The design generation it shows.
    pub generation: u64,
    /// The view the picture belongs to (a Diagram job whose diagram could not be built
    /// shows the solid).
    pub shown: ViewMode,
    /// Whether it shows the Slice tool's provisional design.
    pub provisional: bool,
    /// Width and height of the picture in pixels.
    pub size: (u32, u32),
    /// RGBA8 pixels, row-major (emptied by the UI once it has made an image of them).
    pub rgba: Vec<u8>,
    /// The solid's per-pixel facet ids (Solid view).
    pub pick: Option<PickBuffer>,
    /// The diagram's pick, tooth and panel tables, with its `color` emptied (Diagram view).
    pub diagram: Option<DiagramFrame>,
    /// Facet id to the text a long press shows.
    pub hover_text: Vec<String>,
    /// Facet id to the tier that owns it.
    pub facet_tier: Vec<Option<usize>>,
    /// The mesh the picture was drawn from, with the pose and size used.
    pub geometry: Option<FrameGeometry>,
    /// Where the handles go for the selected tier (committed designs only).
    pub target: Option<HandleTarget>,
    /// The masts the picture was planned from; aligned with the design's tiers.
    pub masts: Option<Vec<SolvedTier>>,
    /// A one-line reason when the picture is not the design's current state.
    pub status: String,
    /// The planner overran its budget: the planes are the previous solve's.
    pub stale: bool,
    /// Whether a real solid was drawn.
    pub has_solid: bool,
    /// The shown solid's bounding radius.
    pub bounding_radius: f64,
    /// How long the job took on the worker.
    pub took: Duration,
}

impl RasterFrame {
    /// A frame that only says the design must be solved first.
    const fn waiting(job: &RasterJob, status: String) -> Self {
        Self {
            kind: FrameKind::Waiting,
            token: job.token,
            generation: job.generation,
            shown: job.mode,
            provisional: job.provisional,
            size: job.size,
            rgba: Vec::new(),
            pick: None,
            diagram: None,
            hover_text: Vec::new(),
            facet_tier: Vec::new(),
            geometry: None,
            target: None,
            masts: None,
            status,
            stale: false,
            has_solid: false,
            bounding_radius: DEFAULT_MESH_BOUNDING_RADIUS,
            took: Duration::ZERO,
        }
    }
}

/// The part of a job the worker compares to its last one.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    generation: u64,
    solved: bool,
    selected_tier: Option<usize>,
    enlarged_panel: i32,
    provisional: bool,
    mode: ViewMode,
    camera: CameraPose,
    size: (u32, u32),
    multi_selected: BTreeSet<usize>,
    selected_facet: Option<u32>,
    provisional_facets: Vec<u32>,
    moved_facets: Vec<u32>,
}

impl Seen {
    fn of(job: &RasterJob) -> Self {
        Self {
            generation: job.generation,
            solved: job.solved.is_some(),
            selected_tier: job.selected_tier,
            enlarged_panel: job.enlarged_panel,
            provisional: job.provisional,
            mode: job.mode,
            camera: job.camera,
            size: job.size,
            multi_selected: job.multi_selected.clone(),
            selected_facet: job.selected_facet,
            provisional_facets: job.provisional_facets.clone(),
            moved_facets: job.moved_facets.clone(),
        }
    }

    fn needs_replan(&self, now: &Self) -> bool {
        self.generation != now.generation
            || self.solved != now.solved
            || self.selected_tier != now.selected_tier
            || self.enlarged_panel != now.enlarged_panel
            || self.provisional != now.provisional
    }

    fn needs_reproject(&self, now: &Self) -> bool {
        self.mode != now.mode || self.camera != now.camera || self.size != now.size
    }

    fn needs_overlay(&self, now: &Self) -> bool {
        self.multi_selected != now.multi_selected
            || self.selected_facet != now.selected_facet
            || self.provisional_facets != now.provisional_facets
            || self.moved_facets != now.moved_facets
    }
}

/// A monotonic clock for the planner's budget check.
struct WallClock {
    origin: Instant,
}

impl Clock for WallClock {
    fn now_ms(&self) -> f64 {
        self.origin.elapsed().as_secs_f64() * 1000.0
    }
}

/// Plans and draws jobs; see the module documentation.
pub struct RasterWorker {
    pipeline: PreviewPipeline,
    /// The design the last committed replan came from and its masts.
    cache: Option<(Arc<Design>, Vec<SolvedTier>)>,
    cache_rev: u64,
    cache_generation: u64,
    /// The facet map of `cache`, with the revision it was built for.
    facet_map: Option<(u64, FacetMap)>,
    seen: Option<Seen>,
    clock: WallClock,
}

impl Default for RasterWorker {
    fn default() -> Self {
        Self::new()
    }
}

/// The result of planning and drawing once.
struct Pass {
    frame: RasterFrame,
    /// The late masts to chain a follow-up from when the pass was `stale`.
    follow_up: Option<Vec<SolvedTier>>,
}

impl RasterWorker {
    /// A worker that has drawn nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pipeline: PreviewPipeline::new(),
            cache: None,
            cache_rev: 0,
            cache_generation: 0,
            facet_map: None,
            seen: None,
            clock: WallClock {
                origin: Instant::now(),
            },
        }
    }

    /// Runs `job`. `is_current` says whether a newer job has superseded it (checked
    /// before any work and before a follow-up pass); `emit` receives each finished frame
    /// (one, or two when the first was `stale`). Emits nothing for a superseded job or
    /// when nothing the picture depends on changed.
    pub fn run(
        &mut self,
        job: &RasterJob,
        is_current: &dyn Fn() -> bool,
        emit: &mut dyn FnMut(RasterFrame),
    ) {
        if !is_current() {
            return;
        }
        let started = Instant::now();
        let now = Seen::of(job);
        let planned_before = self.pipeline.memory().planes.is_some();
        let (replan, reproject, overlay) = match &self.seen {
            Some(previous) if planned_before => (
                previous.needs_replan(&now),
                previous.needs_reproject(&now),
                previous.needs_overlay(&now),
            ),
            _ => (true, false, false),
        };
        if replan {
            self.replan(job, is_current, emit, started);
        } else if reproject || overlay {
            if let Some(frame) = self.redraw(job, reproject, overlay, started) {
                emit(frame);
            }
            self.seen = Some(now);
        }
    }

    /// The replan path: plan, draw, and, for a `stale` plan, one more pass.
    fn replan(
        &mut self,
        job: &RasterJob,
        is_current: &dyn Fn() -> bool,
        emit: &mut dyn FnMut(RasterFrame),
        started: Instant,
    ) {
        let mut chain_override: Option<(Vec<SolvedTier>, BTreeSet<usize>)> = None;
        for pass in 0..2 {
            let Some(done) = self.plan_and_draw(job, chain_override.take(), started) else {
                return;
            };
            let waiting = done.frame.kind == FrameKind::Waiting;
            let follow_up = done.follow_up;
            emit(done.frame);
            if waiting {
                // Leave `seen` alone: the next job must try again.
                return;
            }
            self.seen = Some(Seen::of(job));
            let Some(masts) = follow_up else {
                return;
            };
            if pass == 1 || !is_current() {
                return;
            }
            chain_override = Some((masts, BTreeSet::new()));
        }
    }

    /// Whether a full solve of `design` may run on the worker: the editor's rule for an
    /// inline solve (few planes, no tier targets), or every tier pinned.
    fn full_solve_allowed(design: &Design) -> bool {
        plans_without_solver(design)
            || should_solve_synchronously_for(SolveCostEstimate::of(design), None)
    }

    /// Plans `job` and draws it. `None` when the pipeline produced no frame.
    fn plan_and_draw(
        &mut self,
        job: &RasterJob,
        chain_override: Option<(Vec<SolvedTier>, BTreeSet<usize>)>,
        started: Instant,
    ) -> Option<Pass> {
        let basis = if let Some((last_solved, dirty)) = chain_override {
            ReplanBasis::Chain { last_solved, dirty }
        } else if job.provisional {
            job.chain
                .clone()
                .map_or(ReplanBasis::FullSolve, |(last_solved, dirty)| {
                    ReplanBasis::Chain { last_solved, dirty }
                })
        } else {
            replan_basis(
                &job.design,
                job.solved.as_deref(),
                self.cache
                    .as_ref()
                    .map(|(design, masts)| (design.as_ref(), masts.as_slice())),
            )
        };
        if matches!(basis, ReplanBasis::FullSolve) && !Self::full_solve_allowed(&job.design) {
            return Some(Pass {
                frame: RasterFrame::waiting(job, WAITING_STATUS.to_owned()),
                follow_up: None,
            });
        }
        let planned = PreviewPipeline::plan(
            plan_job(ReplanInputs {
                design: Arc::clone(&job.design),
                generation: job.generation,
                basis,
                camera: job.camera,
                size: job.size,
                selected_tier: job.selected_tier,
                custom_materials: &[],
                view_mode: job.mode.code(),
                show_preform: true,
                enlarged_panel: job.enlarged_panel,
                tier_cutoff: -1,
            }),
            DEFAULT_PREVIEW_BUDGET,
            &self.clock,
        );
        let stale = planned.stale;
        let solved = planned
            .unsolvable_status
            .is_none()
            .then(|| planned.solved.clone())
            .flatten();
        if !job.provisional
            && let Some(masts) = &solved
        {
            self.cache = Some((Arc::clone(&job.design), masts.clone()));
            self.cache_rev += 1;
            self.cache_generation = job.generation;
        }
        let rendered = self.pipeline.render_planned(planned)?;
        let rendered = self.with_overlay(job, rendered);
        let masts = if job.provisional {
            solved.clone()
        } else {
            self.aligned_cache_masts(&job.design)
        };
        let frame = self.package(job, rendered, masts, started);
        Some(Pass {
            frame,
            follow_up: if stale { solved } else { None },
        })
    }

    /// The overlay a job asks for, with the multi-selection resolved to facet ids.
    fn overlay_of(job: &RasterJob, facet_tier: &[Option<usize>]) -> FacetOverlay {
        FacetOverlay {
            hovered: None,
            selected_facet: job.selected_facet,
            multi_selected: facets_of_tiers(facet_tier, &job.multi_selected),
            provisional: job.provisional_facets.clone(),
            moved: job.moved_facets.clone(),
        }
    }

    /// A planned frame starts without highlights; put the job's back.
    fn with_overlay(&mut self, job: &RasterJob, frame: RenderedFrame) -> RenderedFrame {
        let overlay = Self::overlay_of(job, &frame.facet_tier);
        if overlay == FacetOverlay::default() {
            return frame;
        }
        match self.pipeline.update_overlay(overlay) {
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

    /// The reproject / overlay path: no planning.
    fn redraw(
        &mut self,
        job: &RasterJob,
        reproject: bool,
        overlay: bool,
        started: Instant,
    ) -> Option<RasterFrame> {
        let mut frame = if reproject {
            let meta = &job.design.meta;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "f32 is the GPU/render precision; a gear reference angle in degrees is far inside f32's range"
            )]
            let gear = (meta.gear_teeth_abs(), meta.gear_reference_angle as f32);
            self.pipeline
                .reproject(job.camera, job.size, job.mode.code(), Some(gear))
        } else {
            None
        };
        if overlay {
            let overlay = Self::overlay_of(job, &self.pipeline.memory().diagram.facet_tier);
            frame = self.pipeline.update_overlay(overlay).or(frame);
        }
        let rendered = frame?;
        let masts = if job.provisional {
            None
        } else {
            self.aligned_cache_masts(&job.design)
        };
        Some(self.package(job, rendered, masts, started))
    }

    /// The cached masts when they describe `design`'s tiers one to one.
    fn aligned_cache_masts(&self, design: &Design) -> Option<Vec<SolvedTier>> {
        self.cache
            .as_ref()
            .filter(|(_, masts)| masts_aligned(design.tiers.len(), masts.len()))
            .map(|(_, masts)| masts.clone())
    }

    /// Turns a rendered frame into the UI's [`RasterFrame`].
    fn package(
        &mut self,
        job: &RasterJob,
        mut rendered: RenderedFrame,
        masts: Option<Vec<SolvedTier>>,
        started: Instant,
    ) -> RasterFrame {
        let diagram = if job.mode == ViewMode::Diagram {
            rendered.diagram.take()
        } else {
            None
        };
        let (shown, size, rgba, pick, diagram) = match diagram {
            Some(mut diagram) => {
                let size = (diagram.width, diagram.height);
                let rgba = std::mem::take(&mut diagram.color);
                (ViewMode::Diagram, size, rgba, None, Some(diagram))
            }
            None => (
                ViewMode::Solid,
                self.pipeline.size(),
                self.pipeline.solid_rgba().to_vec(),
                Some(rendered.pick),
                None,
            ),
        };
        let target = if shown == ViewMode::Solid
            && job.want_handles
            && !job.provisional
            && rendered.generation == job.generation
        {
            rendered
                .geometry
                .as_ref()
                .and_then(|geometry| self.place_target(job, geometry))
        } else {
            None
        };
        RasterFrame {
            kind: FrameKind::Image,
            token: job.token,
            generation: rendered.generation,
            shown,
            provisional: job.provisional,
            size,
            rgba,
            pick,
            diagram,
            hover_text: rendered.hover_text,
            facet_tier: rendered.facet_tier,
            geometry: rendered.geometry,
            target,
            masts,
            status: rendered.status,
            stale: rendered.stale,
            has_solid: rendered.has_solid,
            bounding_radius: rendered.mesh_bounding_radius,
            took: started.elapsed(),
        }
    }

    /// Where the handles go on the selected tier of the committed design, when the
    /// facet map of the cached masts describes the frame on screen.
    fn place_target(&mut self, job: &RasterJob, geometry: &FrameGeometry) -> Option<HandleTarget> {
        if self.cache_generation != job.generation {
            return None;
        }
        let tier = job.selected_tier?;
        let (design, masts) = self.cache.as_ref()?;
        if !masts_aligned(design.tiers.len(), masts.len()) {
            return None;
        }
        let design = Arc::clone(design);
        if self.facet_map.as_ref().map(|(rev, _)| *rev) != Some(self.cache_rev) {
            let map = FacetMap::from_design(&design, masts);
            self.facet_map = Some((self.cache_rev, map));
        }
        let (_, map) = self.facet_map.as_ref()?;
        target_for(geometry, map, &design, tier, job.remembered_facet, false)
    }
}

/// Runs one job on a fresh worker and returns its (first) frame: the one-shot form of
/// [`RasterWorker::run`], for tests and tools.
#[must_use]
pub fn render(job: &RasterJob) -> Option<RasterFrame> {
    let mut worker = RasterWorker::new();
    let mut first = None;
    worker.run(job, &|| true, &mut |frame| {
        if first.is_none() {
            first = Some(frame);
        }
    });
    first
}
