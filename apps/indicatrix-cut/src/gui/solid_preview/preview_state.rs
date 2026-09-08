//! The editor-side controller for the solid preview.
//!
//! Owns a [`mesh_cache::MeshCache`] and a [`raster::SolidRasterizer`] on one
//! dedicated worker thread, and coalesces a burst of redraw requests (an orbit
//! drag, a fast-typed edit) down to whichever was submitted LAST, via the same
//! [`RedrawGate`] contract `bridge::render_thread::frame_helpers::push_frame_to_ui`
//! already relies on for the path-traced view.
//!
//! # Threading
//!
//! [`SolidPreviewState::request_redraw`]/[`SolidPreviewState::request_replan`] must be
//! called from the UI thread and return immediately: they only submit into the
//! [`RedrawGate`] and, if this call won the race to be the one that must act, wake
//! the worker thread over a plain [`std::sync::mpsc`] channel. The worker thread
//! (spawned lazily on the first call) owns the [`mesh_cache::MeshCache`] and
//! [`raster::SolidRasterizer`] -- neither is `Sync`, and there is no reason for
//! either to be: exactly one thread ever touches them. Once a frame is ready, the
//! worker calls [`PreviewSink::apply`], whose only real (Slint-backed)
//! implementation hops back to the UI thread via `slint::Weak::upgrade_in_event_loop`
//! to do the actual image swap -- that hop lives on the trait implementation's
//! side, not in this module.
//!
//! # Planning happens on the WORKER thread, never the UI thread
//!
//! `resolve_dirty` was measured at 1.89s for a single-tier edit on the 103-tier
//! "CrackOtto-Step" design, and calling it on the UI thread would freeze the editor
//! that long on every keystroke. [`RedrawRequest::Replan`] (behind the `editor`
//! feature) instead carries everything [`super::live_update::plan_preview`] needs
//! (a `Design` clone, dirty tier set, previous solve, camera/size/selection/RI/
//! view-mode); [`render_request`], inside the worker loop, is what actually calls
//! it, builds the `facet_map`/overlay/[`SolidStyle`], and renders. The UI thread
//! never blocks on any of that; [`PreviewSink::apply`] hands the result back
//! asynchronously.
//!
//! # Where `last_solved` lives
//!
//! `EditorState` is `Rc<RefCell<..>>`, which is why it cannot be reached from
//! [`PreviewSink::apply`]'s WORKER-thread call, and `PreviewSink` itself must stay
//! `Send + Sync` -- an `Rc`/`RefCell` cannot be a field of a type with that bound.
//! So the round trip goes through the wiring layer instead: [`PreviewSink::apply`]
//! is handed the freshly solved [`super::live_update::PreviewPlan::solved`] (`Send`
//! plain data), and `gui::mod`'s real [`PreviewSink`] stashes it into a plain
//! `Arc<Mutex<Option<Vec<SolvedTier>>>>` shared with `gui::editor`'s callbacks,
//! which read it back out building the next [`ReplanRequest`].
//!
//! # `PreviewSink`: kept generic over Slint on purpose
//!
//! [`SolidPreviewState`] is built against the [`PreviewSink`] trait rather than a
//! `slint::Weak<MainWindow>` directly, so a fake, synchronous sink can drive this
//! module's tests with no Slint window (see `tests::FakeSink`).

use super::{
    diagram2d::{self, DiagramConfig, DiagramStyle},
    edges_layer::render_edges_layer,
    mesh_cache::MeshCache,
    raster::{SolidRasterizer, SolidStyle},
    to_diagram_pixel_buffer, to_pixel_buffer,
};
#[cfg(feature = "editor")]
use super::{facet_map::FacetMap, live_update};
use glam::Vec3;
use indicatrix::{
    geometry::{meet_solver::SolvedTier, stone_metrics::SolidMesh},
    optics::raytracer::Camera,
};
use std::sync::{
    Arc, Mutex, PoisonError,
    mpsc::{self, Sender},
};

use crate::bridge::render_thread::RedrawGate;

/// The shared cache backing [`ReplanRequest::last_solved`] across edits.
///
/// See this module's doc comment ("Where `last_solved` lives") for why a plain
/// `Arc<Mutex<..>>` rather than a field on `gui::editor::state::EditorState`.
/// Constructed once in `gui::mod::build_main_window`, shared between the sink
/// (writer) and `gui::editor`'s edit callbacks (reader).
pub type SolidLastSolved = Arc<Mutex<Option<Vec<SolvedTier>>>>;

/// The shared `yaw`/`pitch`/`distance` orbit camera.
///
/// See `raster.rs`'s module doc comment for why the solid view must use exactly
/// `indicatrix::optics::raytracer::Camera::new(yaw, pitch, distance, 42.0)`, the
/// same call the path tracer uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraPose {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

/// A finished frame's per-pixel facet-picking buffer, alongside the size needed to
/// index it (`pick[y * width + x]`, mirroring `SolidRasterizer::pick_at`).
///
/// Handed to [`PreviewSink::apply`] so a real implementation can stash it for
/// hover/click callbacks to read against the LAST rendered frame, without holding
/// the worker thread's own `SolidRasterizer` past this call.
#[derive(Debug, Clone)]
pub struct PickBuffer {
    pub width: u32,
    pub height: u32,
    pick: Vec<u32>,
}

impl PickBuffer {
    /// Same contract as [`raster::SolidRasterizer::pick_at`].
    #[must_use]
    pub fn facet_at(&self, x: u32, y: u32) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let v = self.pick[(y * self.width + x) as usize];
        (v != 0).then(|| v - 1)
    }
}

/// One coalesced redraw request, carried through the [`RedrawGate`] from a
/// `request_*` call to the worker thread that renders it.
enum RedrawRequest {
    /// Cheap camera-follow: re-render the SAME planes/mesh at a new pose (an
    /// orbit/zoom drag, no `Design` to plan against). Reuses whatever
    /// [`SolidStyle`] the worker last computed for a [`Self::Replan`] request, so a
    /// camera drag while viewing an overlay does not lose it.
    Reproject {
        planes: Vec<(Vec3, f32)>,
        camera: CameraPose,
        size: (u32, u32),
        view_mode: u8,
        /// The currently loaded design's gear tooth count/reference angle, for a
        /// Diagram-mode (`view_mode` 3) reproject -- see [`DiagramMemory`]'s doc
        /// comment for why a `Reproject` request otherwise has no `Design` to read
        /// this from. `Some` overwrites the worker's [`DiagramMemory`] gear fields
        /// before rendering; `None` leaves them exactly as they were (the only
        /// option [`SolidPreviewState::request_redraw`]'s caller-compatible wrapper
        /// can offer -- see its own doc comment).
        gear: Option<(u32, f32)>,
    },
    /// Full replan-on-edit: the WORKER thread calls `live_update::plan_preview`
    /// itself and builds the facet-level overlay/style from the result. `design`
    /// is a full clone (see [`ReplanRequest`]'s doc comment), boxed so this
    /// variant does not make `RedrawRequest` (and every `Reproject` request too)
    /// several times larger than needed (`clippy::large_enum_variant`).
    #[cfg(feature = "editor")]
    Replan {
        design: Box<indicatrix_cut_core::Design>,
        dirty: std::collections::BTreeSet<usize>,
        last_solved: Option<Vec<SolvedTier>>,
        camera: CameraPose,
        size: (u32, u32),
        selected_tier: Option<usize>,
        n_d: f64,
        view_mode: u8,
    },
}

/// [`SolidPreviewState::request_replan`]'s payload: bundles everything
/// [`super::live_update::plan_preview`] and the facet-overlay build need.
///
/// `design` is cloned by the caller: cheap relative to the solve it might trigger,
/// and it lets the worker thread own its own copy without holding the UI thread's
/// `Rc<RefCell<EditorState>>` borrow open across the async round trip.
#[cfg(feature = "editor")]
pub struct ReplanRequest {
    pub design: indicatrix_cut_core::Design,
    /// The tier index/indices the triggering edit touched. An edit that cannot be
    /// described as "these tiers changed" (`Undo`/`Redo`, a gear remap, a
    /// symmetry/mirror change) should pass `last_solved: None` instead.
    pub dirty: std::collections::BTreeSet<usize>,
    /// The previous call's resolved masts, or `None` to force a full
    /// `Design::solve()` rather than a subgraph `resolve_dirty` -- always safe (just
    /// slower), so prefer `None` over an under-approximated `dirty`.
    pub last_solved: Option<Vec<SolvedTier>>,
    pub camera: CameraPose,
    /// The Solid viewport's own LOGICAL size -- see
    /// [`SolidPreviewState::request_redraw`]'s doc comment.
    pub size: (u32, u32),
    /// The tier currently selected in the tier list (`facet_map::OverlayFlags::
    /// selected`), `None` when nothing is selected.
    pub selected_tier: Option<usize>,
    /// The design's effective refractive index the critical-angle overlay is
    /// computed against.
    pub n_d: f64,
    /// 0 = Solid, 1 = Path-traced, 2 = Both -- only `2` makes the worker also
    /// render the transparent-fill edges layer (see [`PreviewFrame::edges_image`]).
    pub view_mode: u8,
}

/// Where a finished (or status-only) solid-preview frame goes once the worker
/// thread has it.
///
/// Called from the WORKER thread, never the UI thread -- an
/// implementation that needs to touch a Slint window must hop over via
/// `slint::Weak::upgrade_in_event_loop` itself.
///
/// `image` is always a valid, fully-formed pixel buffer sized to the request's
/// `size` -- background-colored/transparent, never garbage or stale, when
/// `has_solid` is `false`. `status` is a short human-readable reason when
/// `!has_solid`, empty otherwise. `solved` is
/// [`super::live_update::PreviewPlan::solved`] for a [`RedrawRequest::Replan`]
/// request (`None` for `Reproject`, or when no solve could produce masts). `stale`
/// mirrors `super::live_update::Freshness::Stale` (always `false` for `Reproject`).
/// `pick` is this frame's facet-picking buffer. `edges_image` is `Some` only when
/// `view_mode` was `2` (Both) and the arrangement closed.
///
/// A raw `slint::SharedPixelBuffer<slint::Rgba8Pixel>`, deliberately NOT a
/// `slint::Image` (not `Send`, so it cannot cross the closure above). A real
/// implementation converts via `slint::Image::from_rgba8` on the UI thread.
pub trait PreviewSink: Send + Sync + 'static {
    fn apply(&self, frame: PreviewFrame);
}

/// One finished worker-thread render -- everything [`render_request`] produces.
/// See [`PreviewSink`]'s doc comment for what each field means.
pub struct PreviewFrame {
    pub image: slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    pub has_solid: bool,
    pub status: String,
    pub solved: Option<Vec<SolvedTier>>,
    pub stale: bool,
    pub pick: PickBuffer,
    pub edges_image: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    /// View mode 3's three-panel 2D facet diagram (`diagram2d::render_diagram`),
    /// built only when the request's `view_mode` was `3` and the arrangement
    /// closed -- `None` otherwise (never built for view modes 0/1/2, matching
    /// `edges_image`'s own "only when asked for" contract).
    pub diagram_image: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    pub has_diagram: bool,
    /// The diagram's own per-pixel facet-picking buffer -- a SEPARATE buffer from
    /// `pick` (the solid rasterizer's), since the diagram's pixel layout is
    /// entirely different, but resolving to the SAME globally-unique `facet_id`
    /// space, so a click on any panel maps to the correct facet through the exact
    /// same `facet_map::FacetMap` lookups the solid view's hover/click already use.
    pub diagram_pick: Option<PickBuffer>,
    /// Facet id -> full hover tooltip text (`facet_map::FacetMap::hover_text`),
    /// built alongside the diagram image so the UI thread can resolve a diagram
    /// hover without needing `Design`/`FacetMap` access itself. `None` when no
    /// diagram was requested this frame; a [`RedrawRequest::Reproject`] request in
    /// view mode 3 carries forward the WORKER's last-known table rather than
    /// rebuilding it (there is no `Design` to rebuild it from on that path).
    pub diagram_hover_text: Option<Vec<String>>,
    /// Facet id -> owning tier index (`facet_map::FacetMap::tier_of`), the other
    /// half of what a diagram click needs to select a tier -- see
    /// `diagram_hover_text`'s doc comment for why this travels with the frame
    /// instead of being recomputed on the UI thread.
    pub diagram_facet_tier: Option<Vec<Option<usize>>>,
}

/// The worker thread's memory of the last diagram-relevant data it computed --
/// carried across a [`RedrawRequest::Reproject`] request as gear info/labels
/// have no `Design` to be recomputed from on that path, exactly like `last_style`
/// is already carried forward for the ordinary solid render. Defaults to a
/// 96-tooth gear with no reference angle and no labels -- a reasonable "nothing
/// solved into the diagram yet" starting point.
struct DiagramMemory {
    gear_teeth: u32,
    gear_reference_angle: f32,
    style: DiagramStyle,
    hover_text: Vec<String>,
    facet_tier: Vec<Option<usize>>,
}

impl Default for DiagramMemory {
    fn default() -> Self {
        Self {
            gear_teeth: 96,
            gear_reference_angle: 0.0,
            style: DiagramStyle::default(),
            hover_text: Vec::new(),
            facet_tier: Vec::new(),
        }
    }
}

/// Calls `live_update::plan_preview`, then builds the facet-level [`SolidStyle`]
/// (flagged/pending/selected) from its result via `facet_map::FacetMap::overlay_flags`,
/// on the WORKER thread (this module's doc comment, "Planning happens on the
/// WORKER thread").
///
/// Split out of [`render_request`]'s `Replan` arm, with `budget` as a parameter
/// (rather than always `live_update::DEFAULT_PREVIEW_BUDGET` inline), so this
/// module's tests can force the `Stale` branch deterministically (`Duration::ZERO`,
/// which a real `resolve_dirty` call can never finish within).
///
/// `pending_tier` is `plan.freshness`'s `Stale.pending` set's first member
/// (`overlay_flags` only accepts one tier to outline) -- in practice always exactly
/// one, since every edit callback's `dirty` set names a single edited tier.
///
/// Returns: the planes to draw this frame, the facet-level style, the mast list to
/// chain forward as the next call's `last_solved`, and whether this frame is stale.
#[cfg(feature = "editor")]
type PlannedStyle = (Vec<(Vec3, f32)>, SolidStyle, Option<Vec<SolvedTier>>, bool);

#[cfg(feature = "editor")]
fn plan_and_style(
    design: &indicatrix_cut_core::Design,
    last_solved: Option<&[SolvedTier]>,
    dirty: &std::collections::BTreeSet<usize>,
    budget: std::time::Duration,
    selected_tier: Option<usize>,
    n_d: f64,
) -> PlannedStyle {
    let plan =
        live_update::plan_preview(design, last_solved, dirty, budget, &live_update::RealSolver);
    let pending_tier = match &plan.freshness {
        live_update::Freshness::Stale { pending } => pending.iter().next().copied(),
        _ => None,
    };
    let facet_map = FacetMap::from_design(design, plan.solved.as_deref().unwrap_or(&[]));
    let overlay = facet_map.overlay_flags(design, n_d, selected_tier, pending_tier);
    let style = SolidStyle {
        flagged: overlay.flagged,
        pending: overlay.pending,
        selected: overlay.selected,
        ..SolidStyle::default()
    };
    let stale = matches!(plan.freshness, live_update::Freshness::Stale { .. });
    (plan.planes, style, plan.solved, stale)
}

/// Rebuilds `last_diagram`'s facet-label/hover-text/tier tables and style from a
/// freshly (re)solved `design` -- split out of [`render_request`]'s `Replan` arm
/// purely to keep that function short. `solid_style` is the SAME [`SolidStyle`]
/// [`plan_and_style`] just built, so the diagram's flagged/pending/selected
/// overlay is guaranteed to agree with the ordinary Solid view's.
#[cfg(feature = "editor")]
fn update_diagram_memory_from_design(
    last_diagram: &mut DiagramMemory,
    design: &indicatrix_cut_core::Design,
    solved: Option<&[SolvedTier]>,
    solid_style: &SolidStyle,
    n_d: f64,
) {
    let facet_map = FacetMap::from_design(design, solved.unwrap_or(&[]));
    let facet_count = facet_map.facet_count();
    let mut labels = vec![String::new(); facet_count];
    let mut hover_text = vec![String::new(); facet_count];
    let mut facet_tier = vec![None; facet_count];
    for id in 0..facet_count {
        labels[id] = facet_map.facet_label(id).to_string();
        hover_text[id] = facet_map.hover_text(id, n_d);
        facet_tier[id] = facet_map.tier_of(id);
    }
    last_diagram.style = DiagramStyle {
        flagged: solid_style.flagged.clone(),
        pending: solid_style.pending.clone(),
        selected: solid_style.selected.clone(),
        facet_labels: labels,
        ..DiagramStyle::default()
    };
    last_diagram.hover_text = hover_text;
    last_diagram.facet_tier = facet_tier;
}

/// [`build_diagram_outputs`]'s return: the diagram image, whether it was actually
/// built, its own pick buffer, and the facet-id-indexed hover-text/tier tables --
/// see [`PreviewFrame`]'s matching fields for what each means.
type DiagramOutputs = (
    Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    bool,
    Option<PickBuffer>,
    Option<Vec<String>>,
    Option<Vec<Option<usize>>>,
);

/// View mode 3's whole diagram-building step, split out of [`render_request`]
/// purely to keep that function short.
///
/// Camera-independent (never reads a camera pose) so a [`RedrawRequest::
/// Reproject`] request (an orbit drag on the OTHER viewport, which still
/// re-issues the current `view_mode`) simply rebuilds the diagram at the current
/// size from the worker's own `last_diagram` memory -- see [`DiagramMemory`]'s doc
/// comment. Builds nothing in any other view mode, matching `edges_image`'s own
/// "only when asked for" contract in [`render_request`].
fn build_diagram_outputs(
    mesh_cache: &mut MeshCache,
    planes: &[(Vec3, f32)],
    size: (u32, u32),
    view_mode: u8,
    last_diagram: &DiagramMemory,
) -> DiagramOutputs {
    if view_mode != 3 {
        return (None, false, None, None, None);
    }
    let (image, has_diagram, pick) =
        mesh_cache
            .get_or_build(planes)
            .map_or((None, false, None), |cached| {
                let config = DiagramConfig {
                    width: size.0,
                    height: size.1,
                    gear_teeth: last_diagram.gear_teeth,
                    gear_reference_angle: last_diagram.gear_reference_angle,
                };
                let diagram_frame =
                    diagram2d::render_diagram(&cached.mesh, &config, &last_diagram.style);
                let image = to_diagram_pixel_buffer(&diagram_frame);
                let pick = PickBuffer {
                    width: diagram_frame.width,
                    height: diagram_frame.height,
                    pick: diagram_frame.pick,
                };
                (Some(image), true, Some(pick))
            });
    (
        image,
        has_diagram,
        pick,
        Some(last_diagram.hover_text.clone()),
        Some(last_diagram.facet_tier.clone()),
    )
}

/// Applies a [`RedrawRequest::Reproject`] request's optional `gear` override onto
/// `last_diagram` -- split out of [`render_request`] purely to keep that function
/// under clippy's function-length lint. `None` (the `request_redraw` compatibility
/// wrapper's only option -- see its own doc comment) leaves `last_diagram`'s gear
/// fields exactly as they were.
const fn apply_reproject_gear(last_diagram: &mut DiagramMemory, gear: Option<(u32, f32)>) {
    if let Some((gear_teeth, gear_reference_angle)) = gear {
        last_diagram.gear_teeth = gear_teeth;
        last_diagram.gear_reference_angle = gear_reference_angle;
    }
}

/// The plan/style/camera/solve state [`resolve_request_state`] resolves one
/// [`RedrawRequest`] into, for [`render_request`] to actually draw.
type RequestState = (
    Vec<(Vec3, f32)>,
    CameraPose,
    (u32, u32),
    u8,
    SolidStyle,
    Option<Vec<SolvedTier>>,
    bool,
);

/// The request-kind `match` half of [`render_request`] -- split out purely to keep
/// that function under clippy's function-length lint. `last_style` is the worker's
/// memory of the last [`RedrawRequest::Replan`]'s style, reused (never mutated) by a
/// [`RedrawRequest::Reproject`] request.
fn resolve_request_state(
    last_style: &mut SolidStyle,
    last_diagram: &mut DiagramMemory,
    request: RedrawRequest,
) -> RequestState {
    match request {
        RedrawRequest::Reproject {
            planes,
            camera,
            size,
            view_mode,
            gear,
        } => {
            apply_reproject_gear(last_diagram, gear);
            (
                planes,
                camera,
                size,
                view_mode,
                last_style.clone(),
                None,
                false,
            )
        }
        #[cfg(feature = "editor")]
        RedrawRequest::Replan {
            design,
            dirty,
            last_solved,
            camera,
            size,
            selected_tier,
            n_d,
            view_mode,
        } => {
            let (planes, style, solved, stale) = plan_and_style(
                &design,
                last_solved.as_deref(),
                &dirty,
                live_update::DEFAULT_PREVIEW_BUDGET,
                selected_tier,
                n_d,
            );
            *last_style = style.clone();
            last_diagram.gear_teeth = design.meta.gear_teeth_abs();
            last_diagram.gear_reference_angle = design.meta.gear_reference_angle as f32;
            if view_mode == 3 {
                update_diagram_memory_from_design(
                    last_diagram,
                    &design,
                    solved.as_deref(),
                    &style,
                    n_d,
                );
            }
            (planes, camera, size, view_mode, style, solved, stale)
        }
    }
}

/// Renders one request against `mesh_cache`/`rasterizer`/`edges_rasterizer`, all
/// three owned by the worker thread for the process lifetime. `last_style` is the
/// worker's memory of the last [`RedrawRequest::Replan`]'s style, reused (never
/// mutated) by a [`RedrawRequest::Reproject`] request.
fn render_request(
    mesh_cache: &mut MeshCache,
    rasterizer: &mut SolidRasterizer,
    edges_rasterizer: &mut SolidRasterizer,
    last_style: &mut SolidStyle,
    last_diagram: &mut DiagramMemory,
    request: RedrawRequest,
) -> PreviewFrame {
    let (planes, camera_pose, size, view_mode, style, solved, stale) =
        resolve_request_state(last_style, last_diagram, request);

    rasterizer.resize(size.0, size.1);
    let camera = Camera::new(
        camera_pose.yaw,
        camera_pose.pitch,
        camera_pose.distance,
        42.0,
    );
    let (image, has_solid, status) = if let Some(cached) = mesh_cache.get_or_build(&planes) {
        rasterizer.render_prepared(cached, &camera, &style);
        (to_pixel_buffer(rasterizer), true, String::new())
    } else {
        // Not closed: still produce a real, correctly-sized (background-colored)
        // image -- the viewport must never go blank/stale -- plus the reason, for
        // the caller's status banner.
        rasterizer.render(&SolidMesh::default(), &camera, &style);
        (
            to_pixel_buffer(rasterizer),
            false,
            mesh_cache.status_message(),
        )
    };
    let pick = PickBuffer {
        width: rasterizer.width,
        height: rasterizer.height,
        pick: rasterizer.pick.clone(),
    };

    // "Both" mode: the transparent-fill/opaque-edges layer, composited by the Slint
    // side over the path-traced image -- only built when the toggle asks for it.
    let edges_image = if view_mode == 2 {
        mesh_cache.get_or_build(&planes).map(|cached| {
            edges_rasterizer.resize(size.0, size.1);
            render_edges_layer(edges_rasterizer, cached, &camera, &style);
            to_pixel_buffer(edges_rasterizer)
        })
    } else {
        None
    };

    let (diagram_image, has_diagram, diagram_pick, diagram_hover_text, diagram_facet_tier) =
        build_diagram_outputs(mesh_cache, &planes, size, view_mode, last_diagram);

    PreviewFrame {
        image,
        has_solid,
        status,
        solved,
        stale,
        pick,
        edges_image,
        diagram_image,
        has_diagram,
        diagram_pick,
        diagram_hover_text,
        diagram_facet_tier,
    }
}

/// The editor-side controller described in this module's doc comment.
pub struct SolidPreviewState {
    sink: Arc<dyn PreviewSink>,
    gate: Arc<RedrawGate<RedrawRequest>>,
    /// Wake channel, created lazily on the first `request_*` call. `None` until
    /// then, so a `SolidPreviewState` never asked to redraw never spawns a thread.
    wake: Mutex<Option<Sender<()>>>,
}

impl SolidPreviewState {
    /// Builds a controller that hands every finished frame to `sink`. No thread is
    /// spawned yet -- see [`Self::request_redraw`].
    #[must_use]
    pub fn new(sink: Arc<dyn PreviewSink>) -> Arc<Self> {
        Arc::new(Self {
            sink,
            gate: Arc::new(RedrawGate::new()),
            wake: Mutex::new(None),
        })
    }

    /// Submits one cheap camera-follow redraw (see [`RedrawRequest::Reproject`]) and
    /// returns immediately; the finished frame reaches [`PreviewSink::apply`]
    /// asynchronously. `size` must be the viewport's LOGICAL size (points, not
    /// render resolution). `view_mode` (0 Solid / 1 Path-traced / 2 Both / 3
    /// Diagram) decides whether the worker also renders the "Both" mode's edges
    /// layer, or the Diagram mode's 2D facet diagram, this frame -- a Diagram-mode
    /// request rebuilds the diagram at the worker's last-known gear info/labels
    /// (see [`DiagramMemory`]) since a `Reproject` request carries no `Design`. A
    /// burst of calls coalesces to exactly the LAST one submitted -- see
    /// [`RedrawGate`]'s doc comment.
    ///
    /// A thin `gear: None` wrapper over [`Self::request_redraw_with_gear`], kept at
    /// this exact signature so every pre-existing caller (`gui::editor::view::
    /// refresh_viewport`, `gui::editor::auto_solve`'s background-solve resubmit)
    /// keeps compiling unchanged. `None` means the worker's last-known Diagram-mode
    /// gear info is left as-is, which is only correct when that caller's design is
    /// the same one the worker last replanned -- prefer
    /// [`Self::request_redraw_with_gear`] with the loaded design's actual gear info
    /// (`bridge::render_thread::RenderContext::design_gear`) wherever that is
    /// available, as `gui::render::camera_lighting::resubmit_at_current_pose`
    /// already does.
    pub fn request_redraw(
        &self,
        planes: Vec<(Vec3, f32)>,
        camera: CameraPose,
        size: (u32, u32),
        view_mode: u8,
    ) {
        self.request_redraw_with_gear(planes, camera, size, view_mode, None);
    }

    /// Same contract as [`Self::request_redraw`], plus `gear`: the currently loaded
    /// design's gear tooth count/reference angle (`bridge::render_thread::
    /// RenderContext::design_gear`), threaded through to a Diagram-mode (`view_mode`
    /// 3) reproject so it shows THIS design's gear wheel rather than whatever a
    /// previous editor replan happened to leave in the worker's own
    /// [`DiagramMemory`] -- see [`RedrawRequest::Reproject`]'s `gear` field doc
    /// comment. `None` leaves the worker's last-known gear info untouched.
    pub fn request_redraw_with_gear(
        &self,
        planes: Vec<(Vec3, f32)>,
        camera: CameraPose,
        size: (u32, u32),
        view_mode: u8,
        gear: Option<(u32, f32)>,
    ) {
        self.submit(RedrawRequest::Reproject {
            planes,
            camera,
            size,
            view_mode,
            gear,
        });
    }

    /// Submits a full replan-on-edit request -- see [`ReplanRequest`]'s doc comment
    /// for its fields; `live_update::plan_preview` never runs on this thread.
    #[cfg(feature = "editor")]
    pub fn request_replan(&self, request: ReplanRequest) {
        self.submit(RedrawRequest::Replan {
            design: Box::new(request.design),
            dirty: request.dirty,
            last_solved: request.last_solved,
            camera: request.camera,
            size: request.size,
            selected_tier: request.selected_tier,
            n_d: request.n_d,
            view_mode: request.view_mode,
        });
    }

    /// Common submit path: lazily spawns the worker thread, then pushes `request`
    /// through the [`RedrawGate`] and wakes the worker if this call won the race.
    ///
    /// # Panics
    ///
    /// Never in practice: the `.expect(..)` below can only fail if another thread
    /// cleared `wake` between the check and this read, which never happens.
    fn submit(&self, request: RedrawRequest) {
        let tx = {
            let mut guard = self.wake.lock().unwrap_or_else(PoisonError::into_inner);
            if guard.is_none() {
                *guard = Some(self.spawn_worker());
            }
            guard
                .clone()
                .expect("just initialized above if it was empty")
        };
        if self.gate.submit(request).is_some() {
            // A `send` failing here would mean the worker thread panicked. Not
            // fatal: the next submit call finds a dead channel and this call's
            // frame is silently skipped, rather than the UI thread panicking too.
            let _ = tx.send(());
        }
    }

    /// Spawns the one dedicated worker thread and returns its wake channel's
    /// sending half. Called at most once, guarded by `wake` being `Some` after.
    fn spawn_worker(&self) -> Sender<()> {
        let (tx, rx) = mpsc::channel::<()>();
        let sink = Arc::clone(&self.sink);
        let gate = Arc::clone(&self.gate);
        std::thread::spawn(move || {
            let mut mesh_cache = MeshCache::default();
            let mut rasterizer = SolidRasterizer::new(1, 1);
            let mut edges_rasterizer = SolidRasterizer::new(1, 1);
            let mut last_style = SolidStyle::default();
            let mut last_diagram = DiagramMemory::default();
            for () in rx {
                // May be newer than the request that caused this wake-up, if more
                // `submit` calls arrived while this thread was rendering the
                // previous one -- the coalescing this module exists for.
                let Some(request) = gate.take() else {
                    continue;
                };
                let frame = render_request(
                    &mut mesh_cache,
                    &mut rasterizer,
                    &mut edges_rasterizer,
                    &mut last_style,
                    &mut last_diagram,
                    request,
                );
                sink.apply(frame);
            }
        });
        tx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    struct FakeSink {
        calls: Mutex<Vec<(bool, String)>>,
    }

    impl FakeSink {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
            })
        }

        fn calls(&self) -> Vec<(bool, String)> {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl PreviewSink for FakeSink {
        fn apply(&self, frame: PreviewFrame) {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((frame.has_solid, frame.status));
        }
    }

    /// Polls `f` until it stops changing for a short stability window, or panics
    /// after `timeout` -- the worker thread runs asynchronously, so there is no
    /// single event to block on.
    fn wait_for_settled<T: PartialEq + Clone>(mut f: impl FnMut() -> T, timeout: Duration) -> T {
        let deadline = Instant::now() + timeout;
        let mut last = f();
        let mut stable_polls = 0u32;
        loop {
            std::thread::sleep(Duration::from_millis(15));
            let current = f();
            if current == last {
                stable_polls += 1;
                if stable_polls >= 4 {
                    return current;
                }
            } else {
                stable_polls = 0;
                last = current;
            }
            assert!(Instant::now() < deadline, "worker thread never settled");
        }
    }

    fn box_planes(y_half: f32) -> Vec<(Vec3, f32)> {
        vec![
            (Vec3::X, 1.0),
            (Vec3::NEG_X, 1.0),
            (Vec3::Y, y_half),
            (Vec3::NEG_Y, y_half),
            (Vec3::Z, 1.0),
            (Vec3::NEG_Z, 1.0),
        ]
    }

    fn unbounded_planes() -> Vec<(Vec3, f32)> {
        vec![(Vec3::X, 1.0), (Vec3::NEG_X, 1.0)]
    }

    const CAMERA: CameraPose = CameraPose {
        yaw: 0.0,
        pitch: 0.0,
        distance: 5.0,
    };

    #[test]
    fn a_single_request_eventually_reaches_the_sink() {
        let sink = FakeSink::new();
        let state = SolidPreviewState::new(sink.clone());
        state.request_redraw(box_planes(0.6), CAMERA, (16, 16), 0);

        let calls = wait_for_settled(|| sink.calls().len(), Duration::from_secs(5));
        assert_eq!(calls, 1);
        assert!(sink.calls()[0].0, "a closed box must report has_solid");
    }

    #[test]
    fn coalesces_a_burst_of_requests_to_the_latest() {
        let sink = FakeSink::new();
        let state = SolidPreviewState::new(sink.clone());

        // Nine requests that never close, then one that does -- if coalescing works,
        // the LAST frame the sink sees must be the closed one.
        for _ in 0..9 {
            state.request_redraw(unbounded_planes(), CAMERA, (16, 16), 0);
        }
        state.request_redraw(box_planes(0.6), CAMERA, (16, 16), 0);

        let final_len = wait_for_settled(|| sink.calls().len(), Duration::from_secs(5));
        assert!(
            final_len < 10,
            "expected coalescing to avoid one render per request, got {final_len}"
        );
        let calls = sink.calls();
        let (has_solid, _status) = calls.last().expect("at least one call must have landed");
        assert!(
            *has_solid,
            "the last frame the sink receives must reflect the LAST submitted request"
        );
    }

    #[test]
    fn a_non_closed_request_reports_has_solid_false_with_a_reason() {
        let sink = FakeSink::new();
        let state = SolidPreviewState::new(sink.clone());
        state.request_redraw(unbounded_planes(), CAMERA, (16, 16), 0);

        wait_for_settled(|| sink.calls().len(), Duration::from_secs(5));
        let calls = sink.calls();
        let (has_solid, status) = calls.last().unwrap();
        assert!(!has_solid);
        assert!(status.contains("Unbounded"), "got: {status}");
    }

    #[cfg(feature = "editor")]
    mod replan {
        use super::*;
        use indicatrix::geometry::meet_solver::MeetConstraint;
        use indicatrix_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};

        fn tier(name: &str, angle_deg: f64, constraint: MeetConstraint) -> ConstraintTier {
            ConstraintTier {
                angle_deg,
                name: name.to_string(),
                indices: vec![0.0],
                constraint,
                imported_meet: None,
                detached: Vec::new(),
            }
        }

        /// Every tier `ScaleReference` -- `plan_preview`'s "Pinned" tier, resolved
        /// with no solver call, so this plans in microseconds regardless of budget.
        /// No real closure guarantee -- fine for tests that never touch
        /// `mesh_cache`; see [`closed_design`] for the render-path fixture.
        fn pinned_design() -> Design {
            Design::new(
                PreformSpec::block(1.0, 1.0, 1.0),
                ScheduleMeta {
                    gear_teeth: 96,
                    ..ScheduleMeta::default()
                },
                vec![
                    tier("Table", 0.0, MeetConstraint::ScaleReference(0.5)),
                    tier("Pavilion", -40.0, MeetConstraint::ScaleReference(0.6)),
                ],
            )
        }

        /// A synthetic "RBC-445"-style design, reauthored as [`ConstraintTier`]s
        /// (mirrors `facet_map.rs`'s private `standard_round_brilliant_design`
        /// fixture). Every tier pinned via `ScaleReference`, proven
        /// `SolidStatus::Closed` elsewhere (`raster.rs`) -- used here, unlike
        /// [`pinned_design`], because these tests need a real closed solid.
        fn closed_design() -> Design {
            const GIRDLE_INDICES: [f64; 16] = [
                0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0,
                84.0, 90.0,
            ];
            const BREAK_INDICES: [f64; 16] = [
                95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0,
                83.0, 85.0,
            ];
            const MAIN_INDICES: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
            const STAR_INDICES: [f64; 8] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];

            fn rbc_tier(name: &str, angle_deg: f64, indices: &[f64], mast: f64) -> ConstraintTier {
                ConstraintTier {
                    angle_deg,
                    name: name.to_string(),
                    indices: indices.to_vec(),
                    constraint: MeetConstraint::ScaleReference(mast),
                    imported_meet: None,
                    detached: Vec::new(),
                }
            }

            Design::new(
                PreformSpec::block(2.0, 1.0, 2.0),
                ScheduleMeta {
                    gemcad_version: "GemCad 5.0".to_string(),
                    gear_teeth: 96,
                    gear_reference_angle: 0.0,
                    symmetry_order: 8,
                    mirror: true,
                    refractive_index: 1.54,
                    headers: Vec::new(),
                    footnotes: Vec::new(),
                },
                vec![
                    rbc_tier("Table", 0.0, &[], 0.32),
                    rbc_tier("Star", 15.0, &STAR_INDICES, 0.45),
                    rbc_tier("Crown Main", 34.5, &MAIN_INDICES, 0.59),
                    rbc_tier("Upper Girdle", 41.0, &BREAK_INDICES, 0.67),
                    rbc_tier("Girdle", 90.0, &GIRDLE_INDICES, 1.0),
                    rbc_tier("Pavilion Main", -41.0, &MAIN_INDICES, 0.67),
                    rbc_tier("Lower Girdle", -42.5, &BREAK_INDICES, 0.68),
                    rbc_tier("Culet", -0.0, &[], 0.88),
                ],
            )
        }

        /// One anchored tier plus one free (`MeetExisting`) tier -- a real
        /// `resolve_dirty` call happens for this fixture, which is what the
        /// `Duration::ZERO` budget below needs to force `Stale` deterministically.
        fn free_design() -> Design {
            Design::new(
                PreformSpec::block(2.0, 1.0, 2.0),
                ScheduleMeta {
                    gear_teeth: 96,
                    ..ScheduleMeta::default()
                },
                vec![
                    tier("C1", 30.0, MeetConstraint::ScaleReference(0.6)),
                    tier("C2", 40.0, MeetConstraint::MeetExisting),
                ],
            )
        }

        #[test]
        fn plan_and_style_returns_the_solved_masts_for_a_pinned_design() {
            let design = pinned_design();
            let (planes, _style, solved, stale) = plan_and_style(
                &design,
                None,
                &std::collections::BTreeSet::new(),
                live_update::DEFAULT_PREVIEW_BUDGET,
                None,
                design.effective_refractive_index(),
            );
            assert_ne!(planes, Vec::<(Vec3, f32)>::new());
            assert_eq!(solved.map(|s| s.len()), Some(2));
            assert!(!stale, "a pinned design never goes over budget");
        }

        #[test]
        fn a_zero_budget_forces_stale_and_marks_the_dirty_tier_pending() {
            let design = free_design();
            let previous = design.solve().expect("fixture must solve");
            let dirty = std::collections::BTreeSet::from([1]);

            let (_planes, style, solved, stale) = plan_and_style(
                &design,
                Some(&previous),
                &dirty,
                Duration::ZERO,
                None,
                design.effective_refractive_index(),
            );
            assert!(
                stale,
                "a real resolve_dirty call can never finish within 0ns"
            );
            assert!(
                solved.is_some(),
                "the fresh (late) result must still be chained forward"
            );
            // Built from the SAME (new) solved masts `plan_and_style` used internally
            // -- a `FacetMap` built from `previous`'s (different) masts is not
            // guaranteed to assign the same facet ids.
            let facet_map = FacetMap::from_design(&design, solved.as_deref().unwrap_or(&[]));
            assert!(
                facet_map
                    .facets_of_tier(1)
                    .iter()
                    .all(|&id| style.pending[id as usize]),
                "the edited tier's own facets must be marked pending"
            );
        }

        #[test]
        fn selected_tier_flags_reach_solid_style_selected() {
            let design = pinned_design();
            let (_planes, style, _solved, _stale) = plan_and_style(
                &design,
                None,
                &std::collections::BTreeSet::new(),
                live_update::DEFAULT_PREVIEW_BUDGET,
                Some(0),
                design.effective_refractive_index(),
            );
            // Same masts `plan_and_style` solved internally (an all-pinned design's
            // `Design::solve()` reads each tier's own `ScaleReference` value).
            let solved = design.solve().expect("every tier is pinned");
            let facet_map = FacetMap::from_design(&design, &solved);
            assert!(
                facet_map
                    .facets_of_tier(0)
                    .iter()
                    .all(|&id| style.selected[id as usize]),
                "tier 0's own facets must be marked selected"
            );
            assert!(
                facet_map
                    .facets_of_tier(1)
                    .iter()
                    .all(|&id| !style.selected[id as usize]),
                "tier 1 was never selected"
            );
        }

        #[test]
        fn render_request_carries_solved_and_freshness_through_to_the_worker_frame() {
            let design = closed_design();
            let mut mesh_cache = MeshCache::default();
            let mut rasterizer = SolidRasterizer::new(16, 16);
            let mut edges_rasterizer = SolidRasterizer::new(16, 16);
            let mut last_style = SolidStyle::default();
            let mut last_diagram = DiagramMemory::default();
            let n_d = design.effective_refractive_index();

            let frame = render_request(
                &mut mesh_cache,
                &mut rasterizer,
                &mut edges_rasterizer,
                &mut last_style,
                &mut last_diagram,
                RedrawRequest::Replan {
                    design: Box::new(design),
                    dirty: std::collections::BTreeSet::new(),
                    last_solved: None,
                    camera: CAMERA,
                    size: (16, 16),
                    selected_tier: None,
                    n_d,
                    view_mode: 0,
                },
            );
            assert!(frame.has_solid);
            assert!(!frame.stale);
            assert_eq!(frame.solved.map(|s| s.len()), Some(8));
            assert!(
                frame.edges_image.is_none(),
                "view_mode 0 never builds edges"
            );
            assert!(
                frame.diagram_image.is_none(),
                "view_mode 0 never builds the diagram"
            );
        }

        #[test]
        fn view_mode_both_also_produces_an_edges_image() {
            let design = closed_design();
            let mut mesh_cache = MeshCache::default();
            let mut rasterizer = SolidRasterizer::new(16, 16);
            let mut edges_rasterizer = SolidRasterizer::new(16, 16);
            let mut last_style = SolidStyle::default();
            let mut last_diagram = DiagramMemory::default();
            let n_d = design.effective_refractive_index();

            let frame = render_request(
                &mut mesh_cache,
                &mut rasterizer,
                &mut edges_rasterizer,
                &mut last_style,
                &mut last_diagram,
                RedrawRequest::Replan {
                    design: Box::new(design),
                    dirty: std::collections::BTreeSet::new(),
                    last_solved: None,
                    camera: CAMERA,
                    size: (16, 16),
                    selected_tier: None,
                    n_d,
                    view_mode: 2,
                },
            );
            assert!(frame.edges_image.is_some());
            assert!(
                frame.diagram_image.is_none(),
                "view_mode 2 never builds the diagram"
            );
        }

        /// View mode 3: the worker must build the 2D facet diagram, its own pick
        /// buffer, and the facet-id-indexed hover-text/tier tables a diagram click
        /// needs -- see `PreviewFrame::diagram_hover_text`'s doc comment for why
        /// those travel with the frame instead of being recomputed on the UI thread.
        #[test]
        fn view_mode_diagram_produces_the_diagram_image_and_its_side_tables() {
            let design = closed_design();
            let mut mesh_cache = MeshCache::default();
            let mut rasterizer = SolidRasterizer::new(16, 16);
            let mut edges_rasterizer = SolidRasterizer::new(16, 16);
            let mut last_style = SolidStyle::default();
            let mut last_diagram = DiagramMemory::default();
            let n_d = design.effective_refractive_index();

            let frame = render_request(
                &mut mesh_cache,
                &mut rasterizer,
                &mut edges_rasterizer,
                &mut last_style,
                &mut last_diagram,
                RedrawRequest::Replan {
                    design: Box::new(design),
                    dirty: std::collections::BTreeSet::new(),
                    last_solved: None,
                    camera: CAMERA,
                    size: (240, 120),
                    selected_tier: None,
                    n_d,
                    view_mode: 3,
                },
            );
            assert!(frame.has_diagram);
            assert!(frame.diagram_image.is_some());
            let pick = frame
                .diagram_pick
                .expect("view_mode 3 must produce a diagram pick buffer");
            assert_eq!((pick.width, pick.height), (240, 120));
            let hover_text = frame
                .diagram_hover_text
                .expect("view_mode 3 must produce a facet-id-indexed hover-text table");
            assert_eq!(
                hover_text.len(),
                frame.diagram_facet_tier.as_ref().unwrap().len()
            );
            // The Table tier's facet (id 0, right after the preform's own planes on
            // this fixture -- see `facet_map.rs`'s own doc comment) must have a
            // non-empty hover string and a resolved tier index somewhere in the table.
            assert!(hover_text.iter().any(|t| t.contains("Table")));
        }
    }
}
