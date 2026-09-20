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
//! # Planning happens on a WORKER thread, never the UI thread
//!
//! `resolve_dirty` was measured at 1.89s for a single-tier edit on the 103-tier
//! "CrackOtto-Step" design, and calling it on the UI thread would freeze the editor
//! that long on every keystroke. [`ReplanRequest`] (behind the `editor` feature)
//! carries everything [`super::live_update::plan_preview`] needs (a `Design`
//! clone, dirty tier set, previous solve, camera/size/selection/RI/view-mode). The
//! UI thread never blocks on any of that; [`PreviewSink::apply`] hands the result
//! back asynchronously.
//!
//! # Two workers: planning vs. rendering (CAD audit item 110)
//!
//! A single worker used to do BOTH the solve/`resolve_dirty` call and the
//! mesh-build/rasterize call, one after the other, for every request -- so a
//! `Reproject` (a camera orbit/zoom frame, no `Design` to plan against) submitted
//! while a `Replan`'s multi-second solve was already running sat queued behind it,
//! freezing the viewport for exactly as long as that solve took. Splitting the two
//! across separate threads/gates fixes this without changing either one's own
//! cost:
//!
//! - The **PLAN worker** ([`SolidPreviewState::spawn_plan_worker`]) is the ONLY
//!   thread that ever calls [`build_planned_frame`]/`live_update::plan_preview` --
//!   the expensive half. It has its own [`RedrawGate`] (`SolidPreviewState::
//!   plan_gate`), which coalesces a burst of edits to "whichever solve is
//!   currently running, then the latest one queued behind it" as it always did,
//!   except now that queue is entirely separate from the render worker's own.
//! - The **RENDER worker** ([`SolidPreviewState::spawn_worker`]) still owns the
//!   [`mesh_cache::MeshCache`]/[`raster::SolidRasterizer`] pair and is the only
//!   thread that ever touches them (neither is `Sync`) -- it now handles THREE
//!   request kinds instead of two: [`RedrawRequest::Reproject`],
//!   [`RedrawRequest::UpdateFacetOverlay`], and the plan worker's own finished
//!   [`RedrawRequest::Planned`] frame. None of the three ever calls
//!   `plan_preview`/`Design::solve`, so this worker's own queue is never blocked
//!   by a slow solve -- a camera drag submitted mid-solve renders against the
//!   last-known planes immediately, exactly like an ordinary `Reproject` always
//!   has.
//!
//! The one piece of the old single-call `plan_and_style` that still needs a
//! `MeshCache` (CAD audit item 63's tier-named "Unbounded: ... never close the
//! solid" banner, which requires actually attempting the mesh build) is therefore
//! split across the boundary too: [`build_planned_frame`] (PLAN worker) computes
//! everything else and leaves [`PlannedFrame::style`] undimmed and
//! [`PlannedFrame::unsolvable_status`] as the only status this thread can already
//! decide (no mesh check needed for `Freshness::Unsolvable`); [`resolve_planned_state`]
//! (RENDER worker) finishes the job once it knows whether THIS frame's own
//! arrangement actually closes, using the exact same `mesh_cache.get_or_build`
//! call `render_request`'s own tail immediately re-queries with the identical
//! planes right afterward -- always a guaranteed cache hit, never a second build.
//!
//! The PLAN worker hands a finished [`PlannedFrame`] to the RENDER worker through
//! [`SolidPreviewState::submit`] -- the SAME lazy-spawn-and-wake path a
//! `Reproject`/`UpdateFacetOverlay` call already uses -- via a [`Weak`] handle
//! back to this same `SolidPreviewState` ([`SolidPreviewState::self_weak`]),
//! rather than duplicating that spawn-on-first-use contract a second time.
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
    diagram2d::{self, DiagramConfig, DiagramStyle, PanelKind},
    edges_layer::render_edges_layer,
    mesh_cache::{CachedMesh, MeshCache},
    raster::{SolidRasterizer, SolidStyle},
    to_diagram_pixel_buffer, to_pixel_buffer,
};
#[cfg(feature = "editor")]
use super::{facet_map::FacetMap, live_update};
use glam::Vec3;
use indicatrix::{
    geometry::{
        meet_solver::SolvedTier,
        stone_metrics::{SolidMesh, SolidStatus},
    },
    optics::raytracer::Camera,
};
use std::sync::{
    Arc, Mutex, PoisonError,
    mpsc::{self, Sender},
};
// Only `SolidPreviewState::self_weak` (CAD audit item 110's plan/render worker
// split) needs this, and that field/its use are both `editor`-feature-gated --
// see `SolidPreviewState`'s own doc comment.
#[cfg(feature = "editor")]
use std::sync::Weak;

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

/// A facet-id-keyed highlight update.
///
/// Identifies one facet under the cursor (#20), one facet a click resolved within
/// its tier (#18), or lists every facet belonging to a multi-selected set of tiers
/// (#19).
///
/// Every field names facet ids directly rather than tier indices, so applying an
/// update never needs a `Design`/`facet_map::FacetMap` on the worker thread -- the
/// caller already resolved the id(s) it wants highlighted (from a pick buffer, or
/// from `FacetMap::facets_of_tier` for each multi-selected tier) before calling
/// [`SolidPreviewState::request_facet_overlay`]. `Default` (every field
/// empty/`None`) clears every highlight.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct FacetOverlay {
    /// The one facet under the cursor, or `None` when nothing is hovered.
    pub hovered: Option<u32>,
    /// The one facet a click identified within its (possibly multi-facet) tier
    /// selection, or `None`.
    pub selected_facet: Option<u32>,
    /// Every facet id belonging to a multi-selected tier; empty when nothing is
    /// multi-selected.
    pub multi_selected: Vec<u32>,
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

/// The Solid viewport's shared pick-buffer/hover-text/facet-tier state.
///
/// The last rendered frame's own [`PickBuffer`], its per-facet hover strings, and
/// its facet-id-to-tier table -- all indexed by facet id and written together by
/// `gui::SlintSolidSink::apply` as each frame lands. Bundled into one struct,
/// rather than three parameters threaded separately through
/// `gui::editor::setup_editor_callbacks`/`setup_editor_secondary_callbacks`, since
/// all three always travel together and are read together by the Solid
/// viewport's own hover/click callbacks
/// (`gui::editor::callbacks::tier_actions::setup_solid_facet_hover_callback`/
/// `setup_solid_facet_click_callback`).
pub struct SolidPickState {
    /// The last rendered frame's per-pixel facet-picking buffer.
    pub pick: Arc<Mutex<Option<PickBuffer>>>,
    /// The last rendered frame's per-facet hover strings, indexed by facet id.
    pub hover_text: Arc<Mutex<Vec<String>>>,
    /// The last rendered frame's facet-id-to-tier table, indexed by facet id.
    pub facet_tier: Arc<Mutex<Vec<Option<usize>>>>,
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
    /// A facet-id-keyed highlight update (#18/#19/#20, see [`FacetOverlay`]'s doc
    /// comment) with no new camera/planes/size of its own -- the worker re-renders
    /// at whatever it last used for a [`Self::Reproject`]/[`Self::Planned`] request
    /// (remembered in [`WorkerMemory`]), exactly the same "cheap camera-follow"
    /// shape [`Self::Reproject`] already has, just following a facet id instead of
    /// a camera pose. Never carries a `Design`: every id here already came from
    /// `SolidRasterizer::pick_at`/`DiagramFrame::pick_at`, or (`multi_selected`)
    /// from whatever tier->facet lookup the caller already had on hand.
    UpdateFacetOverlay(FacetOverlay),
    /// A finished replan from the PLAN worker (CAD audit item 110), ready for the
    /// RENDER worker to finish (the one remaining mesh-dependent status check,
    /// see [`resolve_planned_state`]) and rasterize -- never built directly from a
    /// `RedrawRequest::Replan`-style request any more; see this module's doc
    /// comment, "Two workers: planning vs. rendering". Boxed for the same
    /// `clippy::large_enum_variant` reason the old `Replan` variant was: a
    /// [`PlannedFrame`] carries a whole `Design` plus its solved masts, several
    /// times larger than the plain `Reproject`/`UpdateFacetOverlay` variants.
    #[cfg(feature = "editor")]
    Planned(Box<PlannedFrame>),
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
    /// `gui::editor::state::EditorState::generation`'s value at the moment this
    /// request was submitted (`gui::editor::view::submit_preview_replan`) --
    /// echoed onto the finished [`PreviewFrame`] unchanged (see
    /// [`PreviewFrame::generation`]'s own doc comment) so the sink that
    /// eventually receives it can tell a still-current result from one a later
    /// edit has already superseded, WITHOUT this module or `live_update`
    /// needing to know anything about `EditorState` itself -- a plain `u64`
    /// crosses the worker-thread boundary same as every other field here
    /// (`cad_todo.md` #73).
    pub generation: u64,
    /// `SolidPreviewModel.show_preform_planes`'s value at request time (P1 item
    /// 29's preform-visibility toggle): whether the rough-bounding preform
    /// facets (`facet_map::FacetMap::preform_plane_count`) should render at all
    /// rather than being hidden so only the schedule's own cut facets show.
    pub show_preform: bool,
    /// `SolidPreviewModel.diagram_enlarged_panel`'s raw value at request time
    /// (P1 item 29's "enlarge this panel" mode): `0`/`1`/`2` for Crown/Pavilion/
    /// Profile, anything else (in particular the property's own `-1` default)
    /// for "no panel enlarged." A plain `i32` (rather than `Option<diagram2d::
    /// PanelKind>`) purely so this module's caller does not need a
    /// `diagram2d::PanelKind` import of its own just to build this request --
    /// see [`panel_kind_from_index`] for the conversion this module itself
    /// applies on the WORKER-thread side.
    pub enlarged_panel: i32,
}

/// [`ReplanRequest::enlarged_panel`]'s raw index -> [`PanelKind`] conversion,
/// matching [`diagram2d::PanelKind`]'s declaration order (`0` = Crown, `1` =
/// Pavilion, `2` = Profile) -- anything else (including the property's own
/// `-1` "nothing enlarged" default) is `None`.
#[cfg(feature = "editor")]
const fn panel_kind_from_index(index: i32) -> Option<PanelKind> {
    match index {
        0 => Some(PanelKind::Crown),
        1 => Some(PanelKind::Pavilion),
        2 => Some(PanelKind::Profile),
        _ => None,
    }
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
/// `!has_solid`, empty otherwise -- EXCEPT for a [`live_update::Freshness::
/// Unsolvable`] `Planned` frame, which always sets a "Preview cannot be solved: ..."
/// `status` regardless of `has_solid`, since the drawn solid (if any) is the held-
/// over previous one, not this edit's own result (see [`build_planned_frame`]'s doc
/// comment). `solved` is [`super::live_update::PreviewPlan::solved`] for a
/// [`RedrawRequest::Planned`] request in every other case (`None` for `Reproject`, or
/// when no solve could produce masts) -- an `Unsolvable` frame instead chains the
/// OLD masts forward unchanged, so the shared `last_solved` cache is never wiped by
/// an edit that merely left the design unsolvable. `stale` mirrors
/// `super::live_update::Freshness::Stale` (always `false` for `Reproject`, and for
/// `Unsolvable` too -- a distinct case from staleness, carried instead through
/// `status`). `pick` is this frame's facet-picking buffer. `edges_image` is `Some`
/// only when `view_mode` was `2` (Both) and the arrangement closed.
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
    /// The index wheel's own per-pixel tooth-picking buffer (#121, `cad_todo.md`
    /// item 121's remaining half) -- a SEPARATE buffer from `diagram_pick` above
    /// (different hit regions: a wheel tick's small box, not a facet's fill), but
    /// the same `PickBuffer` shape and `+1`/`0` encoding, reusing `facet_at` to
    /// mean "tooth at" for this buffer. `None` exactly when `diagram_pick` is
    /// (no diagram requested, or the arrangement did not close).
    pub diagram_tooth_pick: Option<PickBuffer>,
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
    /// The plane arrangement this frame was actually rendered from, in the same
    /// `(normal, offset)` convention every other `solid_preview` module uses --
    /// ALWAYS present (never gated on view mode, unlike `diagram_pick`), so a real
    /// implementation (`SlintSolidSink::apply`) can publish it back into
    /// `bridge::render_thread::RenderContext::active_planes` alongside the image
    /// swap.
    ///
    /// #30: before this existed, a `Replan` frame's own (freshly re-solved) planes
    /// never reached `RenderContext`, which only `gui::editor::view::
    /// refresh_viewport`/`gui::editor::auto_solve` wrote directly -- neither of
    /// which runs for an ordinary in-budget edit (`submit_preview_replan`). The
    /// next camera orbit then re-issued `RenderContext`'s stale, PRE-EDIT
    /// `active_planes` (`render::camera_lighting::resubmit_at_current_pose`),
    /// snapping the shown geometry back until the next explicit Solve. Publishing
    /// every frame's own `planes` here -- and letting the sink only overwrite
    /// `RenderContext::active_planes` when they actually differ -- keeps the two
    /// in sync without needing `gui::editor` to remember to do it itself.
    pub planes: Vec<(Vec3, f32)>,
    /// Facet id -> full hover tooltip text (`facet_map::FacetMap::hover_text`),
    /// valid for EVERY view mode -- the Solid view's own counterpart to
    /// `diagram_hover_text`, which is only populated for Diagram-mode requests.
    ///
    /// #22: before this existed, the Solid view's hover/click callbacks
    /// (`gui::editor::callbacks::tier_actions::setup_solid_facet_hover_callback`/
    /// `setup_solid_facet_click_callback`) rebuilt a fresh `facet_map::FacetMap`
    /// from `EditorState`'s `Design` on every mouse move -- an O(plane-count^2)
    /// candidate/dedup pass -- and matched it against whatever `pick` buffer
    /// `SlintSolidSink` happened to have stored, which can be a stale generation
    /// right after an `AddTier`/`RemoveTier`/`Undo` (the two id spaces only agree
    /// once the next replan lands). This table travels WITH the same frame as
    /// `pick`, computed once on the worker thread
    /// (`update_diagram_memory_from_design`, unconditionally run for every
    /// `Replan` since #24), so indexing it can never disagree with the displayed
    /// frame's own facet ids. See `hover_text`'s handoff note at its
    /// `SlintSolidSink` storage site (`gui::mod`) for what still needs to change
    /// in `tier_actions.rs` to actually use it.
    pub hover_text: Vec<String>,
    /// Facet id -> owning tier index (`facet_map::FacetMap::tier_of`), the Solid
    /// view's own counterpart to `diagram_facet_tier` -- see `hover_text`'s doc
    /// comment.
    pub facet_tier: Vec<Option<usize>>,
    /// The design-state generation this frame reflects -- [`ReplanRequest::
    /// generation`] for a `Replan` frame, or [`WorkerMemory::generation`]
    /// (carried forward unchanged) for a `Reproject`/`UpdateFacetOverlay` one;
    /// `0` for a frame from a non-`editor` build, or before this worker thread
    /// has ever received a `Replan`. Lets a real [`PreviewSink`] recognize a
    /// frame whose already-solved [`Self::solved`] masts still describe the
    /// LIVE design, and rebuild the tier table's rows from them directly
    /// instead of leaving that to a second, separately dispatched
    /// `Design::solve()` (`cad_todo.md` #73) -- see
    /// `gui::editor::auto_solve::take_matching_design`'s own doc comment for
    /// the exact staleness check this enables.
    pub generation: u64,
    /// The current solid's own bounding radius (`mesh_cache::CachedMesh::
    /// bounding_radius`, #118) -- whichever mesh this frame actually rendered
    /// from (a fresh build, or the dimmed `last_closed` fallback [`render_request`]
    /// uses when the live arrangement does not close), or
    /// [`DEFAULT_MESH_BOUNDING_RADIUS`] before anything has ever closed. Lets a
    /// real [`PreviewSink`] keep the orbit camera's own distance clamp
    /// (`render::camera_lighting`) sized to the design actually loaded instead
    /// of a fixed range that clips a large preform or strands a tiny one in
    /// empty space.
    pub mesh_bounding_radius: f64,
}

/// [`PreviewFrame::mesh_bounding_radius`]'s fallback before any arrangement has
/// ever closed.
///
/// Roughly a standard round brilliant's own half-width (`ui/models/editor.slint`'s
/// preform defaults are `1.20`/`1.50`), so the very first frame's distance clamp
/// is already in a sensible range rather than `0.0` collapsing it to nothing.
pub const DEFAULT_MESH_BOUNDING_RADIUS: f64 = 1.5;

/// The worker thread's memory of the last diagram-relevant data it computed --
/// carried across a [`RedrawRequest::Reproject`] request as gear info/labels
/// have no `Design` to be recomputed from on that path, exactly like `last_style`
/// is already carried forward for the ordinary solid render. Defaults to a
/// 96-tooth gear with no reference angle and no labels -- a reasonable "nothing
/// solved into the diagram yet" starting point.
struct DiagramMemory {
    gear_teeth: u32,
    gear_reference_angle: f32,
    /// The schedule's own rotational symmetry order (`ScheduleMeta::
    /// symmetry_order`), carried forward exactly like `gear_teeth` above --
    /// see [`DiagramConfig::symmetry_order`]'s own doc comment for what it
    /// draws.
    symmetry_order: u32,
    /// The schedule's own mirror flag (`ScheduleMeta::mirror`), carried
    /// forward exactly like `gear_teeth` above -- see [`DiagramConfig::
    /// mirror`]'s own doc comment for what it draws.
    mirror: bool,
    /// `SolidPreviewModel.diagram_enlarged_panel`'s value at the last `Replan`
    /// (P1 item 29's "enlarge this panel" mode), carried forward exactly like
    /// `gear_teeth` above -- `None` (the default) means the ordinary
    /// three-column [`diagram2d::render_diagram`] layout;
    /// `Some(panel)` means [`build_diagram_outputs`] instead calls
    /// [`diagram2d::render_diagram_single_panel`] for just that one panel.
    enlarged_panel: Option<PanelKind>,
    style: DiagramStyle,
    hover_text: Vec<String>,
    facet_tier: Vec<Option<usize>>,
}

impl Default for DiagramMemory {
    fn default() -> Self {
        Self {
            gear_teeth: 96,
            gear_reference_angle: 0.0,
            // Matches `indicatrix_cut_core::ScheduleMeta::standard_round_brilliant`'s
            // own 8-fold mirrored symmetry -- a reasonable "nothing solved into the
            // diagram yet" starting point, same spirit as `gear_teeth: 96` above.
            symmetry_order: 8,
            mirror: true,
            enlarged_panel: None,
            style: DiagramStyle::default(),
            hover_text: Vec::new(),
            facet_tier: Vec::new(),
        }
    }
}

/// Everything the worker thread remembers from one request to the next, bundled
/// into a single value so [`resolve_request_state`]/[`render_request`] each take
/// one `&mut` parameter instead of growing past clippy's argument-count lint every
/// time a `Reproject` request needs one more piece of carried-forward state.
///
/// `solved_masts` and `planes` exist for [`RedrawRequest::Reproject`]'s sake (#21,
/// #25): a plain camera drag/zoom/pose-button redraw has no `Design` to replan
/// against, so it must reuse the WORKER's own memory of the last real
/// [`RedrawRequest::Planned`] rather than reporting "nothing solved" or leaking a
/// previous design's leftover style.
#[derive(Default)]
struct WorkerMemory {
    /// The last [`RedrawRequest::Planned`]'s facet-level style, reused (never
    /// mutated in place) by a `Reproject` request.
    style: SolidStyle,
    /// The last [`RedrawRequest::Planned`]'s diagram label/hover/tier tables.
    diagram: DiagramMemory,
    /// The last non-empty mast list a `Planned` frame produced, chained forward
    /// as a `Reproject` frame's own `solved` -- see [`render_request`]'s doc
    /// comment.
    solved_masts: Option<Vec<SolvedTier>>,
    /// The plane arrangement the worker last rendered (`Planned` or `Reproject`
    /// alike), `None` before the very first request -- used only to detect that a
    /// `Reproject` request's planes actually changed (a different design was just
    /// loaded/solved), never compared for anything else.
    planes: Option<Vec<(Vec3, f32)>>,
    /// The camera/size/`view_mode` the worker last rendered at (`Planned` or
    /// `Reproject` alike) -- what a [`RedrawRequest::UpdateFacetOverlay`] request
    /// re-renders with, since it carries none of its own (see that variant's doc
    /// comment). `None` only before the very first request; an overlay update
    /// arriving that early has nothing to redraw and is a no-op.
    camera: Option<CameraPose>,
    size: Option<(u32, u32)>,
    view_mode: Option<u8>,
    /// The generation the fields above were last (re)computed for -- `0`
    /// before the very first `Planned` frame (matching [`PreviewFrame::generation`]'s
    /// own "nothing solved yet" default). Set from a `Planned` frame's own
    /// `generation`; carried forward UNCHANGED by `Reproject`/
    /// `UpdateFacetOverlay`, exactly like `solved_masts`/`planes` above --
    /// neither carries a new generation of its own, so the frame they produce
    /// still names whichever design the LAST real replan solved.
    generation: u64,
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
/// Dims `style` for a held-over solid that is not this frame's own fresh result --
/// a flatter, greyed base shading so it reads as visibly not current rather than
/// indistinguishable from a genuinely fresh one. Shared by [`resolve_planned_state`]
/// (a [`live_update::Freshness::Unsolvable`] frame, which reuses the SAME planes as
/// the last successful one) and [`render_request`] (an `Unbounded`/`Degenerate`
/// frame, which shows [`MeshCache::last_closed`] instead -- CAD audit item 63).
/// Keeps every facet-overlay field (`flagged`/`pending`/`selected`/hover/etc.)
/// from `style` untouched, only overriding the base shading. NOT gated on the
/// `editor` feature -- unlike `resolve_planned_state`, [`render_request`]'s own use
/// of this (the `last_closed` fallback) runs in every build.
fn dim_style(style: SolidStyle) -> SolidStyle {
    SolidStyle {
        base_color: [120, 122, 128],
        ambient: 0.4,
        diffuse: 0.35,
        ..style
    }
}

/// One `Unbounded` escaping plane's label for [`resolve_planned_state`]'s banner --
/// `"Girdle (tier 5)"` when the owning tier is named, else `"tier 5"` (1-based,
/// matching the tier table's own `#` column), or the raw `"plane <n>"` fallback
/// when `Design::tier_for_plane_index` can't place it (a preform plane, or an
/// index past the arrangement -- see that method's own doc comment for both).
#[cfg(feature = "editor")]
fn escaping_tier_label(
    design: &indicatrix_cut_core::Design,
    solved: &[SolvedTier],
    plane_index: usize,
) -> String {
    design
        .tier_for_plane_index(solved, plane_index)
        .map_or_else(
            || format!("plane {plane_index}"),
            |tier_index| {
                design.tiers.get(tier_index).map_or_else(
                    || format!("tier {}", tier_index + 1),
                    |tier| {
                        if tier.name.is_empty() {
                            format!("tier {}", tier_index + 1)
                        } else {
                            format!("{} (tier {})", tier.name, tier_index + 1)
                        }
                    },
                )
            },
        )
}

/// [`SolidPreviewState::request_replan`]'s payload once handed off to the PLAN
/// worker (CAD audit item 110) -- the same fields [`ReplanRequest`] carries, just
/// renamed to mark that this is now an internal cross-thread message rather than
/// the public entry point, PLUS `tier_cutoff` (CAD audit item 211): unlike every
/// other field here, it is NOT copied from a same-named [`ReplanRequest`] field
/// (there is no such field there yet -- see [`SolidPreviewState::set_tier_cutoff`]'s
/// doc comment for why) but read fresh off [`SolidPreviewState`]'s own cache at
/// [`SolidPreviewState::request_replan`] time. See this module's doc comment,
/// "Two workers: planning vs. rendering".
#[cfg(feature = "editor")]
struct PlanJob {
    design: indicatrix_cut_core::Design,
    dirty: std::collections::BTreeSet<usize>,
    last_solved: Option<Vec<SolvedTier>>,
    camera: CameraPose,
    size: (u32, u32),
    selected_tier: Option<usize>,
    n_d: f64,
    view_mode: u8,
    generation: u64,
    show_preform: bool,
    enlarged_panel: i32,
    tier_cutoff: Option<usize>,
}

/// [`build_planned_frame`]'s result -- everything the RENDER worker needs to
/// finish and rasterize a replan without ever calling `live_update::plan_preview`
/// itself. `style` is still UNDIMMED here: whether this frame ends up "holding
/// over" a stale/unbounded result is decided by [`resolve_planned_state`], the
/// only place with a [`MeshCache`] to check the arrangement's own closure against
/// (CAD audit item 110's plan/render split -- see this module's doc comment).
#[cfg(feature = "editor")]
struct PlannedFrame {
    design: indicatrix_cut_core::Design,
    planes: Vec<(Vec3, f32)>,
    style: SolidStyle,
    /// The mast list to chain forward as the next call's `last_solved` -- already
    /// resolved to "the fresh result" or "the old masts chained forward
    /// unchanged" (an `Unsolvable` frame must not wipe this cache with `None`,
    /// same reasoning [`resolve_planned_state`]'s predecessor `plan_and_style`
    /// documented).
    solved: Option<Vec<SolvedTier>>,
    stale: bool,
    /// [`live_update::Freshness::Unsolvable`]'s "Preview cannot be solved: ..."
    /// text -- the one status this thread can already decide with no `MeshCache`
    /// of its own. `None` here does NOT mean "this frame is fine": it may still
    /// turn out `Unbounded` once [`resolve_planned_state`] checks the mesh.
    unsolvable_status: Option<String>,
    camera: CameraPose,
    size: (u32, u32),
    view_mode: u8,
    generation: u64,
    n_d: f64,
    enlarged_panel: i32,
}

/// Runs `live_update::plan_preview` (the expensive, potentially multi-second
/// half CAD audit item 110 moves off the render worker's own queue) and builds
/// the facet-level [`SolidStyle`] (flagged/pending/selected) from its result via
/// `facet_map::FacetMap::overlay_flags` -- called ONLY from
/// [`SolidPreviewState::spawn_plan_worker`]'s loop in production.
///
/// `budget` is a parameter (rather than always `live_update::DEFAULT_PREVIEW_BUDGET`
/// inline) so this module's tests can force the `Stale` branch deterministically
/// (`Duration::ZERO`, which a real `resolve_dirty` call can never finish within).
///
/// `pending_tier` is `plan.freshness`'s `Stale.pending` set's first member
/// (`overlay_flags` only accepts one tier to outline) -- in practice always exactly
/// one, since every edit callback's `dirty` set names a single edited tier.
///
/// Deliberately does NOT decide `Unbounded` vs. closed -- that needs a
/// [`MeshCache`], which this (PLAN-worker-only) function never touches; see
/// [`resolve_planned_state`] for the render-side half that finishes the job.
#[cfg(feature = "editor")]
fn build_planned_frame(job: PlanJob, budget: std::time::Duration) -> PlannedFrame {
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
    let pending_tier = match &plan.freshness {
        live_update::Freshness::Stale { pending } => pending.iter().next().copied(),
        _ => None,
    };
    let facet_map = FacetMap::from_design(&design, plan.solved.as_deref().unwrap_or(&[]));
    let overlay = facet_map.overlay_flags(&design, n_d, selected_tier, pending_tier);
    let unsolvable_status = match &plan.freshness {
        live_update::Freshness::Unsolvable(err) => {
            Some(format!("Preview cannot be solved: {err}."))
        }
        _ => None,
    };
    // No `MeshCache` on this (PLAN worker) thread -- CAD audit item 63's
    // tier-named `Unbounded` banner and the resulting dim-or-not decision are
    // finished by [`resolve_planned_state`] on the RENDER worker instead, the
    // only place that can actually attempt the mesh build. `style` below is
    // therefore UNDIMMED regardless of whether this frame turns out unbounded --
    // only a genuine `Unsolvable` (no mesh check needed at all) is reflected here.
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

/// Rebuilds `last_diagram`'s facet-label/hover-text/tier tables and style from a
/// freshly (re)solved `design` -- split out of [`resolve_planned_state`] purely to
/// keep that function short. `solid_style` is the SAME [`SolidStyle`]
/// [`resolve_planned_state`] just finished, so the diagram's flagged/pending/selected
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
    let mut facet_index_on_gear = vec![0u32; facet_count];
    for id in 0..facet_count {
        labels[id] = facet_map.facet_label(id);
        hover_text[id] = facet_map.hover_text(id, n_d);
        facet_tier[id] = facet_map.tier_of(id);
        facet_index_on_gear[id] = facet_map.index_on_gear(id);
    }
    last_diagram.style = DiagramStyle {
        flagged: solid_style.flagged.clone(),
        pending: solid_style.pending.clone(),
        selected: solid_style.selected.clone(),
        facet_labels: labels,
        // P1 item 29: the index-wheel radial-line pass's facet->tooth lookup,
        // and the meet-point markers -- both need nothing beyond `facet_map`,
        // already built above for the label/hover/tier tables.
        facet_index_on_gear,
        meet_marker_pairs: facet_map.meeting_facet_pairs(design),
        ..DiagramStyle::default()
    };
    last_diagram.hover_text = hover_text;
    last_diagram.facet_tier = facet_tier;
}

/// [`build_diagram_outputs`]'s return: the diagram image, whether it was actually
/// built, its own facet pick buffer, the index-wheel's own tooth pick buffer
/// (#121), and the facet-id-indexed hover-text/tier tables -- see
/// [`PreviewFrame`]'s matching fields for what each means.
type DiagramOutputs = (
    Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    bool,
    Option<PickBuffer>,
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
        return (None, false, None, None, None, None);
    }
    let (image, has_diagram, pick, tooth_pick) =
        mesh_cache
            .get_or_build(planes)
            .map_or((None, false, None, None), |cached| {
                let config = DiagramConfig {
                    width: size.0,
                    height: size.1,
                    gear_teeth: last_diagram.gear_teeth,
                    gear_reference_angle: last_diagram.gear_reference_angle,
                    symmetry_order: last_diagram.symmetry_order,
                    mirror: last_diagram.mirror,
                };
                // P1 item 29's "enlarge this panel" mode: draw just that one panel
                // filling the whole frame instead of the ordinary three-column
                // layout -- see `DiagramMemory::enlarged_panel`'s own doc comment.
                let diagram_frame = last_diagram.enlarged_panel.map_or_else(
                    || diagram2d::render_diagram(&cached.mesh, &config, &last_diagram.style),
                    |panel| {
                        diagram2d::render_diagram_single_panel(
                            &cached.mesh,
                            &config,
                            &last_diagram.style,
                            panel,
                        )
                    },
                );
                let image = to_diagram_pixel_buffer(&diagram_frame);
                // #121: the index wheel's own tooth pick buffer, threaded through
                // exactly like `pick` (the facet buffer) above -- both are the SAME
                // `+1`/`0`-encoded shape (`DiagramFrame::tooth`'s own doc comment),
                // so `diagram_wiring`'s hover/click callbacks can query "which
                // facet" and "which tooth" from the same `(x, y)` with no new
                // buffer type.
                let tooth_pick = PickBuffer {
                    width: diagram_frame.width,
                    height: diagram_frame.height,
                    pick: diagram_frame.tooth,
                };
                let pick = PickBuffer {
                    width: diagram_frame.width,
                    height: diagram_frame.height,
                    pick: diagram_frame.pick,
                };
                (Some(image), true, Some(pick), Some(tooth_pick))
            });
    (
        image,
        has_diagram,
        pick,
        tooth_pick,
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
/// [`RedrawRequest`] into, for [`render_request`] to actually draw. The last field is
/// the ready-to-show unsolvable-status message, always `None` for a
/// [`RedrawRequest::Reproject`] request (it never plans, so it can never be
/// unsolvable) -- a plain `String` (not gated on the `editor` feature) so this type
/// stays usable in a non-editor, `Reproject`-only build.
type RequestState = (
    Vec<(Vec3, f32)>,
    CameraPose,
    (u32, u32),
    u8,
    SolidStyle,
    Option<Vec<SolvedTier>>,
    bool,
    Option<String>,
    u64,
);

/// The request-kind `match` half of [`render_request`] -- split out purely to keep
/// that function under clippy's function-length lint. See [`WorkerMemory`]'s doc
/// comment for what each of its fields is carried forward for. `mesh_cache` is
/// only touched by the `Planned` arm (via [`resolve_planned_state`], CAD audit item
/// 63/110) -- the SAME cache [`render_request`] itself queries right afterward with
/// the identical planes, so that later call is always a cache hit, never a second
/// build.
fn resolve_request_state(
    memory: &mut WorkerMemory,
    mesh_cache: &mut MeshCache,
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
            apply_reproject_gear(&mut memory.diagram, gear);
            // A `Reproject` also carries every New/Load/explicit-Solve redraw (see
            // `gui::editor::view::refresh_viewport`), not only a camera drag/zoom --
            // so when the incoming plane arrangement differs from the worker's own
            // last-known one, a genuinely different geometry just replaced the old
            // one (#25). Reset the leftover flagged/pending/selected style and
            // diagram labels rather than let them leak onto a design that never
            // produced them: loading design B after editing A must not show B with
            // some of A's facets still tinted. An ordinary orbit/zoom always
            // resubmits the SAME planes, so this never fires mid-drag.
            if memory.planes.as_deref() != Some(planes.as_slice()) {
                memory.style = SolidStyle::default();
                memory.diagram = DiagramMemory::default();
            }
            memory.planes = Some(planes.clone());
            memory.camera = Some(camera);
            memory.size = Some(size);
            memory.view_mode = Some(view_mode);
            (
                planes,
                camera,
                size,
                view_mode,
                memory.style.clone(),
                memory.solved_masts.clone(),
                false,
                None,
                memory.generation,
            )
        }
        // See `FacetOverlay`'s and this variant's own doc comments -- no `Design`,
        // no new camera/planes/size, just an id-keyed style tweak re-rendered at
        // whatever the worker last used.
        RedrawRequest::UpdateFacetOverlay(overlay) => {
            memory.style.hovered = overlay.hovered;
            memory.style.selected_facet = overlay.selected_facet;
            memory.style.multi_selected = overlay.multi_selected.clone();
            memory.diagram.style.hovered = overlay.hovered;
            memory.diagram.style.selected_facet = overlay.selected_facet;
            memory.diagram.style.multi_selected = overlay.multi_selected;
            (
                memory.planes.clone().unwrap_or_default(),
                memory.camera.unwrap_or(CameraPose {
                    yaw: 0.0,
                    pitch: 0.0,
                    distance: 5.0,
                }),
                memory.size.unwrap_or((1, 1)),
                memory.view_mode.unwrap_or(0),
                memory.style.clone(),
                memory.solved_masts.clone(),
                false,
                None,
                memory.generation,
            )
        }
        #[cfg(feature = "editor")]
        RedrawRequest::Planned(frame) => resolve_planned_state(memory, mesh_cache, *frame),
    }
}

/// [`RedrawRequest::Planned`]'s own half of [`resolve_request_state`], run on the
/// RENDER worker (CAD audit item 110) -- split out purely to keep that function
/// under clippy's function-length lint. Finishes what [`build_planned_frame`]
/// (the PLAN worker) could not: whether `frame`'s own arrangement actually closes
/// (needs [`MeshCache`], which only this thread owns), the resulting CAD audit
/// item 63 tier-named `Unbounded` status when it doesn't, and the corresponding
/// dim-or-not [`SolidStyle`] decision -- then updates `memory` exactly like the
/// old single-call `plan_and_style`/`resolve_replan_state` pair used to, in one
/// synchronous step, before this item split planning off onto its own thread.
#[cfg(feature = "editor")]
fn resolve_planned_state(
    memory: &mut WorkerMemory,
    mesh_cache: &mut MeshCache,
    frame: PlannedFrame,
) -> RequestState {
    let PlannedFrame {
        design,
        planes,
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
    } = frame;
    // The design solved fine (`unsolvable_status` is `None`), but THIS frame's own
    // plane arrangement may still not close -- an unfinished multi-digit angle
    // edit widening a gap mid-keystroke, say. Name the tier whose facet escapes
    // instead of leaving `MeshCache::status_message`'s raw plane-index fallback on
    // screen (CAD audit item 63). Cheap even though it looks like a second mesh
    // build: `mesh_cache` is keyed by plane hash, so `render_request`'s own later
    // `get_or_build(&planes)` call is a guaranteed cache hit here.
    let closes = mesh_cache.get_or_build(&planes).is_some();
    let unbounded_status = if unsolvable_status.is_none() && !closes {
        match mesh_cache.status() {
            Some(SolidStatus::Unbounded { escaping }) => {
                let solved_ref = solved.as_deref().unwrap_or(&[]);
                let names = escaping
                    .iter()
                    .map(|&plane_index| escaping_tier_label(&design, solved_ref, plane_index))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!("Unbounded: {names} never close the solid."))
            }
            _ => None,
        }
    } else {
        None
    };
    // Both a missing anchor and a solid that never closes leave the viewport
    // showing geometry that is NOT what the design currently says, so both dim
    // it: the status line says why, and the dimming is what stops a cutter
    // reading a held-over solid as current.
    let holding_over = unsolvable_status.is_some() || unbounded_status.is_some();
    let preview_status = unsolvable_status.or(unbounded_status);
    let style = if holding_over {
        dim_style(style)
    } else {
        style
    };

    memory.style = style.clone();
    if solved.is_some() {
        memory.solved_masts.clone_from(&solved);
    }
    memory.planes = Some(planes.clone());
    memory.camera = Some(camera);
    memory.size = Some(size);
    memory.view_mode = Some(view_mode);
    memory.generation = generation;
    memory.diagram.gear_teeth = design.meta.gear_teeth_abs();
    memory.diagram.gear_reference_angle = design.meta.gear_reference_angle as f32;
    memory.diagram.symmetry_order = design.meta.symmetry_order;
    memory.diagram.mirror = design.meta.mirror;
    memory.diagram.enlarged_panel = panel_kind_from_index(enlarged_panel);
    // Unconditional (dropped the earlier `view_mode == 3` gate, #24): an edit made
    // while Solid/Both is on screen must still leave the diagram's label/hover/
    // tier tables fresh, so switching to Diagram mode afterward (a `Reproject`,
    // which has no `Design` to rebuild them from) shows a live, correct diagram
    // immediately instead of empty/no-op hover and click until the next
    // Diagram-active edit.
    update_diagram_memory_from_design(&mut memory.diagram, &design, solved.as_deref(), &style, n_d);
    (
        planes,
        camera,
        size,
        view_mode,
        style,
        solved,
        stale,
        preview_status,
        generation,
    )
}

/// Renders one request against `mesh_cache`/`rasterizer`/`edges_rasterizer`, all
/// three owned by the worker thread for the process lifetime, plus `memory` --
/// see [`WorkerMemory`]'s doc comment. `memory.solved_masts` matters most (#21):
/// before this, a `Reproject` request (every camera drag/zoom/pose button, and
/// every `Both`/`Diagram` redraw that isn't a fresh edit) reported `solved: None`,
/// and `SlintSolidSink::apply` stores whatever `solved` it is handed with no
/// `Some`-check -- so simply orbiting the stone wiped the shared `last_solved`
/// cache the NEXT edit needs for a cheap subgraph `resolve_dirty`, silently
/// downgrading it to a full `Design::solve()` (also zeroing every mast a
/// same-normal-direction facet lookup depends on). Chaining the worker's own
/// last-known masts forward on every `Reproject` fixes this without touching the
/// cache's writer at all.
fn render_request(
    mesh_cache: &mut MeshCache,
    rasterizer: &mut SolidRasterizer,
    edges_rasterizer: &mut SolidRasterizer,
    memory: &mut WorkerMemory,
    request: RedrawRequest,
) -> PreviewFrame {
    let (planes, camera_pose, size, view_mode, style, solved, stale, unsolvable_status, generation) =
        resolve_request_state(memory, mesh_cache, request);

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
    } else if let Some(cached) = mesh_cache.last_closed() {
        // CAD audit item 63: this frame's own arrangement doesn't close (a
        // mid-keystroke Unbounded/Degenerate edit), but a real solid was built
        // before -- show THAT, dimmed, rather than blanking the viewport. `status`
        // still carries the reason (overridden below by `unsolvable_status` when
        // there is one).
        let dimmed = dim_style(style.clone());
        rasterizer.render_prepared(cached, &camera, &dimmed);
        (
            to_pixel_buffer(rasterizer),
            true,
            mesh_cache.status_message(),
        )
    } else {
        // Never built a closed solid at all yet: still produce a real,
        // correctly-sized (background-colored) image -- the viewport must never
        // go blank/stale -- plus the reason, for the caller's status banner.
        rasterizer.render(&SolidMesh::default(), &camera, &style);
        (
            to_pixel_buffer(rasterizer),
            false,
            mesh_cache.status_message(),
        )
    };
    // A `preview_status` override (an `Unsolvable` replan, or `resolve_planned_state`'s
    // tier-mapped `Unbounded` text) always wins the status banner -- it names the
    // actual reason the CURRENT edit can't be shown, which matters whether or not
    // the held-over solid happens to still be closed (`has_solid` here reflects the
    // OLD/last-good planes, not necessarily this edit's own outcome).
    let status = unsolvable_status.unwrap_or(status);
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

    let (
        diagram_image,
        has_diagram,
        diagram_pick,
        diagram_tooth_pick,
        diagram_hover_text,
        diagram_facet_tier,
    ) = build_diagram_outputs(mesh_cache, &planes, size, view_mode, &memory.diagram);
    // #22: the Solid view's own hover/tier tables -- the SAME ones `diagram_hover_text`/
    // `diagram_facet_tier` above carry, just handed out unconditionally (not only
    // for a `view_mode == 3` request) since every view mode's facet ids come from
    // the same `FacetMap`. See `PreviewFrame::hover_text`'s doc comment.
    let hover_text = memory.diagram.hover_text.clone();
    let facet_tier = memory.diagram.facet_tier.clone();

    // #118: mirrors the `mesh_cache.get_or_build(&planes).or_else(last_closed)`
    // fallback chain the image itself was rendered from above, so the distance
    // clamp always describes the SAME solid the viewport is actually showing --
    // never the live (possibly not-yet-closing) arrangement when a dimmed
    // `last_closed` mesh is what's on screen instead.
    // Each `.map(CachedMesh::bounding_radius)` converts the borrowed `&CachedMesh`
    // into an OWNED `f64` before the expression ends, so `mesh_cache`'s mutable
    // borrow from `get_or_build` is released before `last_closed` reborrows it in
    // the `or_else` closure -- chaining the two `Option<&CachedMesh>` calls
    // directly (or via `if let`/`else if let`) does not compile here, since the
    // first borrow would still be considered live.
    let mesh_bounding_radius = mesh_cache
        .get_or_build(&planes)
        .map(CachedMesh::bounding_radius)
        .or_else(|| mesh_cache.last_closed().map(CachedMesh::bounding_radius))
        .unwrap_or(DEFAULT_MESH_BOUNDING_RADIUS);

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
        diagram_tooth_pick,
        diagram_hover_text,
        diagram_facet_tier,
        planes,
        hover_text,
        facet_tier,
        generation,
        mesh_bounding_radius,
    }
}

/// The editor-side controller described in this module's doc comment.
pub struct SolidPreviewState {
    sink: Arc<dyn PreviewSink>,
    /// Consumed only by the RENDER worker ([`Self::spawn_worker`]) -- CAD audit
    /// item 110's two-worker split, see the module doc comment.
    gate: Arc<RedrawGate<RedrawRequest>>,
    /// Wake channel for the RENDER worker, created lazily on the first
    /// `request_*`/finished-plan call. `None` until then, so a `SolidPreviewState`
    /// never asked to redraw never spawns a thread.
    wake: Mutex<Option<Sender<()>>>,
    /// Consumed only by the PLAN worker ([`Self::spawn_plan_worker`]) -- a
    /// SEPARATE queue from `gate` so a slow solve queued here can never block a
    /// `Reproject`/`UpdateFacetOverlay` request sitting in `gate` (CAD audit item
    /// 110).
    #[cfg(feature = "editor")]
    plan_gate: Arc<RedrawGate<PlanJob>>,
    /// [`Self::wake`]'s counterpart for the PLAN worker, created lazily the same
    /// way on the first [`Self::request_replan`] call.
    #[cfg(feature = "editor")]
    plan_wake: Mutex<Option<Sender<()>>>,
    /// A weak handle to this same value, filled in immediately after
    /// construction -- lets the PLAN worker (which holds no other reference back
    /// to `Self`) hand a finished [`PlannedFrame`] to the RENDER worker through
    /// [`Self::submit`], the exact same lazy-spawn-and-wake path a `Reproject`
    /// call already uses, rather than duplicating that contract a second time.
    /// `Weak`, not `Arc`: the PLAN worker thread must never be the reason a
    /// `SolidPreviewState` outlives every caller's own handle to it.
    #[cfg(feature = "editor")]
    self_weak: Mutex<Weak<Self>>,
    /// CAD audit item 211's "show through tier N" viewport slider: `Some(n)` makes
    /// every SUBSEQUENT [`Self::request_replan`] truncate the drawn plane
    /// arrangement to `design.tiers[..=n]` (preform planes always kept) via
    /// [`live_update::plan_preview`]'s own `tier_cutoff` parameter. Cached here
    /// (rather than added as a [`ReplanRequest`] field) because the slider lives in
    /// `solid_viewport.slint`/`ui/models/solid_preview.slint` -- outside this
    /// module's file ownership -- so [`Self::set_tier_cutoff`] is the one call this
    /// state exposes for that UI to reach once its own property/callback exists;
    /// see that method's doc comment for the exact remaining wiring.
    #[cfg(feature = "editor")]
    tier_cutoff: Mutex<Option<usize>>,
}

impl SolidPreviewState {
    /// Builds a controller that hands every finished frame to `sink`. No thread is
    /// spawned yet -- see [`Self::request_redraw`].
    #[must_use]
    pub fn new(sink: Arc<dyn PreviewSink>) -> Arc<Self> {
        let state = Arc::new(Self {
            sink,
            gate: Arc::new(RedrawGate::new()),
            wake: Mutex::new(None),
            #[cfg(feature = "editor")]
            plan_gate: Arc::new(RedrawGate::new()),
            #[cfg(feature = "editor")]
            plan_wake: Mutex::new(None),
            #[cfg(feature = "editor")]
            self_weak: Mutex::new(Weak::new()),
            #[cfg(feature = "editor")]
            tier_cutoff: Mutex::new(None),
        });
        #[cfg(feature = "editor")]
        {
            *state
                .self_weak
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Arc::downgrade(&state);
        }
        state
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

    /// Submits a facet-id-keyed highlight update (#18 "which facet did I click",
    /// #19 multi-select, #20 hover) -- see [`FacetOverlay`]'s doc comment. Re-renders
    /// at whatever camera/planes/size/`view_mode` the worker last used for a
    /// [`Self::request_redraw_with_gear`]/[`Self::request_replan`] call, exactly
    /// like an ordinary camera-follow reproject, so a caller that already has a
    /// facet id from a pick buffer never needs to also track/resend the camera
    /// pose or the design's planes just to show it. A no-op before the worker has
    /// rendered anything at all.
    pub fn request_facet_overlay(&self, overlay: FacetOverlay) {
        self.submit(RedrawRequest::UpdateFacetOverlay(overlay));
    }

    /// Submits a full replan-on-edit request to the PLAN worker (CAD audit item
    /// 110) -- see [`ReplanRequest`]'s doc comment for its fields;
    /// `live_update::plan_preview` never runs on the UI thread, and (since this
    /// item) never blocks the RENDER worker's own queue either -- see the module
    /// doc comment, "Two workers: planning vs. rendering".
    #[cfg(feature = "editor")]
    pub fn request_replan(&self, request: ReplanRequest) {
        let tier_cutoff = *self
            .tier_cutoff
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.submit_plan(PlanJob {
            design: request.design,
            dirty: request.dirty,
            last_solved: request.last_solved,
            camera: request.camera,
            size: request.size,
            selected_tier: request.selected_tier,
            n_d: request.n_d,
            view_mode: request.view_mode,
            generation: request.generation,
            show_preform: request.show_preform,
            enlarged_panel: request.enlarged_panel,
            tier_cutoff,
        });
    }

    /// Sets CAD audit item 211's "show through tier N" cutoff: `Some(n)` makes every
    /// subsequent [`Self::request_replan`] draw only `design.tiers[..=n]` (preform
    /// planes always kept); `None` (the default) shows every tier, unchanged from
    /// before this existed.
    ///
    /// Does NOT itself trigger a redraw -- exactly like every other per-replan
    /// setting `SolidPreviewState` does not own a redraw timer for (`n_d`,
    /// `selected_tier`, `show_preform`, ...): the caller must still submit a
    /// [`Self::request_replan`] afterwards for the change to actually reach the
    /// screen.
    ///
    /// HANDOFF (not this module's file ownership): nothing calls this yet.
    /// `ui/models/solid_preview.slint` needs an `in-out property <int> tier_cutoff:
    /// -1;` (Slint has no `Option<int>`; `-1` is "no cutoff", matching this
    /// crate's existing `-1`-means-`None` convention for e.g.
    /// `diagram_enlarged_panel`), a slider in `solid_viewport.slint` bound to it,
    /// and `gui::editor::view::submit_preview_replan` calling
    /// `preview_state.set_tier_cutoff((cutoff >= 0).then_some(cutoff as usize))`
    /// before `request_replan` -- the same three-file shape `show_preform_planes`/
    /// `diagram_enlarged_panel` already went through, just with the read happening
    /// here instead of as a `ReplanRequest` field (see this state's own
    /// `tier_cutoff` field doc comment for why).
    #[cfg(feature = "editor")]
    pub fn set_tier_cutoff(&self, cutoff: Option<usize>) {
        *self
            .tier_cutoff
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = cutoff;
    }

    /// Common submit path for the RENDER worker: lazily spawns it, then pushes
    /// `request` through its [`RedrawGate`] and wakes it if this call won the
    /// race. Also how [`Self::spawn_plan_worker`] hands off a finished
    /// [`PlannedFrame`] -- see that method's own doc comment.
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

    /// Spawns the RENDER worker thread and returns its wake channel's sending
    /// half. Called at most once, guarded by `wake` being `Some` after. Never
    /// calls `live_update::plan_preview`/`Design::solve` itself -- every request
    /// it handles (`Reproject`, `UpdateFacetOverlay`, and the PLAN worker's own
    /// finished `Planned` frame) is cheap relative to a real solve (CAD audit
    /// item 110), which is exactly why a slow solve queued on [`Self::plan_gate`]
    /// can never block this worker's own queue.
    fn spawn_worker(&self) -> Sender<()> {
        let (tx, rx) = mpsc::channel::<()>();
        let sink = Arc::clone(&self.sink);
        let gate = Arc::clone(&self.gate);
        std::thread::spawn(move || {
            let mut mesh_cache = MeshCache::default();
            let mut rasterizer = SolidRasterizer::new(1, 1);
            let mut edges_rasterizer = SolidRasterizer::new(1, 1);
            let mut memory = WorkerMemory::default();
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
                    &mut memory,
                    request,
                );
                sink.apply(frame);
            }
        });
        tx
    }

    /// [`Self::submit`]'s counterpart for the PLAN worker (CAD audit item 110):
    /// lazily spawns it, then pushes `job` through [`Self::plan_gate`] (coalescing
    /// exactly like a `Reproject`/overlay update -- a burst of edits against a
    /// slow design collapses to "whichever solve is running, then the latest one
    /// queued behind it," never a pile of concurrent solves) and wakes it if this
    /// call won the race.
    #[cfg(feature = "editor")]
    fn submit_plan(&self, job: PlanJob) {
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

    /// Spawns the PLAN worker thread and returns its wake channel's sending half.
    /// Called at most once, guarded by `plan_wake` being `Some` after. This is the
    /// ONLY thread that ever calls [`build_planned_frame`]/`live_update::
    /// plan_preview` -- CAD audit item 110's fix: splitting it from
    /// [`Self::spawn_worker`] means a `Reproject`/`UpdateFacetOverlay` request
    /// queued on the RENDER worker's own, separate gate is never stuck behind a
    /// multi-second solve here.
    ///
    /// Hands each finished [`PlannedFrame`] to the RENDER worker through
    /// [`Self::submit`] via `self_weak` -- see that field's own doc comment for
    /// why a weak handle rather than capturing `self` directly.
    #[cfg(feature = "editor")]
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
                // [`Self::spawn_worker`]'s own loop relies on -- a burst of edits
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

    /// CAD audit item 63: once a real solid has been shown, an edit that
    /// temporarily stops closing (typing '4' on the way to '41', say) must keep
    /// showing that last solid (dimmed) rather than blanking the viewport --
    /// `has_solid` stays `true` and the reason still reaches `status`.
    #[test]
    fn an_unbounded_request_after_a_closed_one_keeps_showing_the_last_solid() {
        let sink = FakeSink::new();
        let state = SolidPreviewState::new(sink.clone());
        state.request_redraw(box_planes(0.6), CAMERA, (16, 16), 0);
        wait_for_settled(|| sink.calls().len(), Duration::from_secs(5));
        assert!(sink.calls().last().unwrap().0, "the first frame must close");

        state.request_redraw(unbounded_planes(), CAMERA, (16, 16), 0);
        let final_len = wait_for_settled(|| sink.calls().len(), Duration::from_secs(5));
        assert!(final_len >= 2);
        let (has_solid, status) = sink.calls().last().unwrap().clone();
        assert!(
            has_solid,
            "a held-over closed solid must still be shown, not blanked"
        );
        assert!(status.contains("Unbounded"), "got: {status}");
    }

    /// #18/#19/#20: a facet-overlay update carries no camera/planes/`Design` of its
    /// own -- it must re-render at whatever the worker already used for its last
    /// `Reproject`/`Replan`, and apply to BOTH the solid and diagram styles so
    /// whichever view is on screen picks it up.
    #[test]
    fn facet_overlay_update_reuses_the_last_context_and_applies_to_both_styles() {
        let mut memory = WorkerMemory {
            planes: Some(box_planes(0.6)),
            camera: Some(CAMERA),
            size: Some((16, 16)),
            view_mode: Some(0),
            ..WorkerMemory::default()
        };

        let overlay = FacetOverlay {
            hovered: Some(3),
            selected_facet: Some(5),
            multi_selected: vec![1, 2],
        };
        let mut mesh_cache = MeshCache::default();
        let (planes, camera, size, view_mode, style, ..) = resolve_request_state(
            &mut memory,
            &mut mesh_cache,
            RedrawRequest::UpdateFacetOverlay(overlay.clone()),
        );

        assert_eq!(planes, box_planes(0.6), "must reuse the last-known planes");
        assert_eq!(camera, CAMERA, "must reuse the last-known camera pose");
        assert_eq!(size, (16, 16), "must reuse the last-known viewport size");
        assert_eq!(view_mode, 0, "must reuse the last-known view mode");
        assert_eq!(style.hovered, overlay.hovered);
        assert_eq!(style.selected_facet, overlay.selected_facet);
        assert_eq!(style.multi_selected, overlay.multi_selected);
        assert_eq!(
            memory.diagram.style.hovered, overlay.hovered,
            "the diagram's own style must agree, so switching to Diagram mode \
             afterward shows the same highlight"
        );
    }

    /// A `Reproject` request with an unchanged plane arrangement (an ordinary
    /// camera drag/zoom) must leave a previously-set facet overlay in place --
    /// only a genuinely different design (#25's plane-change check) clears it.
    #[test]
    fn an_ordinary_reproject_does_not_clear_a_facet_overlay() {
        let mut memory = WorkerMemory::default();
        let mut mesh_cache = MeshCache::default();
        resolve_request_state(
            &mut memory,
            &mut mesh_cache,
            RedrawRequest::UpdateFacetOverlay(FacetOverlay::default()),
        );
        resolve_request_state(
            &mut memory,
            &mut mesh_cache,
            RedrawRequest::Reproject {
                planes: box_planes(0.6),
                camera: CAMERA,
                size: (16, 16),
                view_mode: 0,
                gear: None,
            },
        );
        resolve_request_state(
            &mut memory,
            &mut mesh_cache,
            RedrawRequest::UpdateFacetOverlay(FacetOverlay {
                hovered: Some(7),
                ..FacetOverlay::default()
            }),
        );

        let (_planes, _camera, _size, _view_mode, style, ..) = resolve_request_state(
            &mut memory,
            &mut mesh_cache,
            RedrawRequest::Reproject {
                planes: box_planes(0.6),
                camera: CAMERA,
                size: (16, 16),
                view_mode: 0,
                gear: None,
            },
        );
        assert_eq!(
            style.hovered,
            Some(7),
            "an orbit/zoom reproject at the SAME planes must not drop the hover"
        );
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
                original_notes: None,
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
                    original_notes: None,
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

        /// A minimal, all-default [`PlanJob`] for a test that only cares about a
        /// couple of fields -- built with `..` from this so a new `PlanJob` field
        /// never forces every one of this module's tests to list it.
        fn plan_job(design: Design) -> PlanJob {
            let n_d = design.effective_refractive_index();
            PlanJob {
                design,
                dirty: std::collections::BTreeSet::new(),
                last_solved: None,
                camera: CAMERA,
                size: (16, 16),
                selected_tier: None,
                n_d,
                view_mode: 0,
                generation: 0,
                show_preform: true,
                enlarged_panel: -1,
                tier_cutoff: None,
            }
        }

        #[test]
        fn build_planned_frame_returns_the_solved_masts_for_a_pinned_design() {
            let design = pinned_design();
            let frame = build_planned_frame(plan_job(design), live_update::DEFAULT_PREVIEW_BUDGET);
            assert_ne!(frame.planes, Vec::<(Vec3, f32)>::new());
            assert_eq!(frame.solved.map(|s| s.len()), Some(2));
            assert!(!frame.stale, "a pinned design never goes over budget");
        }

        #[test]
        fn a_zero_budget_forces_stale_and_marks_the_dirty_tier_pending() {
            let design = free_design();
            let previous = design.solve().expect("fixture must solve");
            let dirty = std::collections::BTreeSet::from([1]);
            let design_for_facet_map = design.clone();

            let frame = build_planned_frame(
                PlanJob {
                    last_solved: Some(previous),
                    dirty,
                    ..plan_job(design)
                },
                Duration::ZERO,
            );
            assert!(
                frame.stale,
                "a real resolve_dirty call can never finish within 0ns"
            );
            assert!(
                frame.solved.is_some(),
                "the fresh (late) result must still be chained forward"
            );
            // Built from the SAME (new) solved masts `build_planned_frame` used
            // internally -- a `FacetMap` built from the OLD masts is not
            // guaranteed to assign the same facet ids.
            let facet_map = FacetMap::from_design(
                &design_for_facet_map,
                frame.solved.as_deref().unwrap_or(&[]),
            );
            assert!(
                facet_map
                    .facets_of_tier(1)
                    .iter()
                    .all(|&id| frame.style.pending[id as usize]),
                "the edited tier's own facets must be marked pending"
            );
        }

        /// CAD audit item 63: [`resolve_planned_state`]'s `Unbounded` banner must name the
        /// escaping plane's owning tier, not the raw index -- exercised directly
        /// against `escaping_tier_label` rather than via a real failing
        /// `build_solid_mesh` call, since every `PreformSpec::planes` is
        /// documented to be closed on its own (see that method's own doc
        /// comment), so nothing here can assert a specific fixture ends up
        /// `Unbounded`.
        #[test]
        fn escaping_tier_label_names_the_owning_tier() {
            let design = pinned_design();
            let solved = design.solve().expect("every tier is pinned");
            let preform_plane_count = design.preform.planes().len();
            // The first plane past the preform's own is tier 0's ("Table")
            // facet -- `Design::tier_for_plane_index`'s own doc comment: it
            // subtracts `preform.planes().len()` before mapping into the
            // schedule tiers, which are laid out in tier order.
            let label = escaping_tier_label(&design, &solved, preform_plane_count);
            assert_eq!(label, "Table (tier 1)");
        }

        #[test]
        fn escaping_tier_label_falls_back_to_a_raw_plane_for_a_preform_plane() {
            let design = pinned_design();
            let solved = design.solve().expect("every tier is pinned");
            // Index 0 is always one of the preform's own planes -- not a
            // schedule-tier facet, so `Design::tier_for_plane_index` returns
            // `None` and the label falls back to the raw index.
            let label = escaping_tier_label(&design, &solved, 0);
            assert_eq!(label, "plane 0");
        }

        #[test]
        fn selected_tier_flags_reach_solid_style_selected() {
            let design = pinned_design();
            let design_for_facet_map = design.clone();
            let frame = build_planned_frame(
                PlanJob {
                    selected_tier: Some(0),
                    ..plan_job(design)
                },
                live_update::DEFAULT_PREVIEW_BUDGET,
            );
            let style = frame.style;
            // Same masts `build_planned_frame` solved internally (an all-pinned
            // design's `Design::solve()` reads each tier's own `ScaleReference`
            // value).
            let solved = design_for_facet_map.solve().expect("every tier is pinned");
            let facet_map = FacetMap::from_design(&design_for_facet_map, &solved);
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

        /// CAD audit item 211: a `PlanJob.tier_cutoff` of `Some(0)` must truncate
        /// `build_planned_frame`'s drawn planes to `pinned_design`'s first tier
        /// ("Table") only, dropping the second ("Pavilion") -- exercised through
        /// `build_planned_frame` (production's own entry point for a `PlanJob`)
        /// rather than `live_update::plan_preview` directly, since this is the
        /// boundary [`SolidPreviewState::request_replan`] actually threads the
        /// cached cutoff across.
        #[test]
        fn tier_cutoff_truncates_the_planned_frame() {
            let design = pinned_design();
            let full = build_planned_frame(
                plan_job(design.clone()),
                live_update::DEFAULT_PREVIEW_BUDGET,
            );
            let truncated = build_planned_frame(
                PlanJob {
                    tier_cutoff: Some(0),
                    ..plan_job(design)
                },
                live_update::DEFAULT_PREVIEW_BUDGET,
            );
            assert!(
                truncated.planes.len() < full.planes.len(),
                "cutting off after tier 0 must drop tier 1's (\"Pavilion\") facet(s): \
                 full={}, truncated={}",
                full.planes.len(),
                truncated.planes.len()
            );
        }

        /// [`SolidPreviewState::set_tier_cutoff`] only caches the value for the
        /// NEXT [`SolidPreviewState::request_replan`] -- this guards that the
        /// getter side of that cache (the private `tier_cutoff` field itself,
        /// read back through the same lock `set_tier_cutoff` writes through)
        /// round-trips both `Some` and back to `None`, independent of the worker
        /// thread machinery `request_replan` also drives.
        #[test]
        fn set_tier_cutoff_round_trips_through_the_cache() {
            let state = SolidPreviewState::new(FakeSink::new());
            let cached = || {
                *state
                    .tier_cutoff
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
            };
            assert_eq!(cached(), None);
            state.set_tier_cutoff(Some(3));
            assert_eq!(cached(), Some(3));
            state.set_tier_cutoff(None);
            assert_eq!(cached(), None);
        }

        /// Builds a [`RedrawRequest::Planned`] request the way production code
        /// does since CAD audit item 110 split planning off the render worker's
        /// own queue: [`build_planned_frame`] on a `PlanJob` for `design`, then
        /// boxed into the request `render_request` accepts. A test-only stand-in
        /// for the two real worker threads, kept synchronous and deterministic.
        fn planned_request(
            design: Design,
            view_mode: u8,
            size: (u32, u32),
            generation: u64,
        ) -> RedrawRequest {
            let frame = build_planned_frame(
                PlanJob {
                    size,
                    view_mode,
                    generation,
                    ..plan_job(design)
                },
                live_update::DEFAULT_PREVIEW_BUDGET,
            );
            RedrawRequest::Planned(Box::new(frame))
        }

        #[test]
        fn render_request_carries_solved_and_freshness_through_to_the_worker_frame() {
            let design = closed_design();
            let mut mesh_cache = MeshCache::default();
            let mut rasterizer = SolidRasterizer::new(16, 16);
            let mut edges_rasterizer = SolidRasterizer::new(16, 16);
            let mut memory = WorkerMemory::default();

            let frame = render_request(
                &mut mesh_cache,
                &mut rasterizer,
                &mut edges_rasterizer,
                &mut memory,
                planned_request(design, 0, (16, 16), 7),
            );
            assert!(frame.has_solid);
            assert!(!frame.stale);
            assert_eq!(frame.solved.map(|s| s.len()), Some(8));
            assert_eq!(
                frame.generation, 7,
                "the request's own generation must reach the finished frame unchanged"
            );
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
            let mut memory = WorkerMemory::default();

            let frame = render_request(
                &mut mesh_cache,
                &mut rasterizer,
                &mut edges_rasterizer,
                &mut memory,
                planned_request(design, 2, (16, 16), 0),
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
            let mut memory = WorkerMemory::default();

            let frame = render_request(
                &mut mesh_cache,
                &mut rasterizer,
                &mut edges_rasterizer,
                &mut memory,
                planned_request(design, 3, (240, 120), 0),
            );
            assert!(frame.has_diagram);
            assert!(frame.diagram_image.is_some());
            let pick = frame
                .diagram_pick
                .expect("view_mode 3 must produce a diagram pick buffer");
            assert_eq!((pick.width, pick.height), (240, 120));
            // #121: the index wheel's own tooth pick buffer must be threaded
            // through alongside the facet one, at the SAME size (both are built
            // from the same `DiagramFrame`).
            let tooth_pick = frame
                .diagram_tooth_pick
                .expect("view_mode 3 must also produce a tooth pick buffer");
            assert_eq!((tooth_pick.width, tooth_pick.height), (240, 120));
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

        /// #118: every frame -- not just Diagram-mode ones -- must carry a real,
        /// positive bounding radius once a solid has closed, so `render::
        /// camera_lighting`'s orbit-zoom clamp/"Fit" pose never reads the
        /// [`DEFAULT_MESH_BOUNDING_RADIUS`] placeholder for a design that
        /// actually solved.
        #[test]
        fn replan_frame_carries_a_positive_mesh_bounding_radius() {
            let design = closed_design();
            let mut mesh_cache = MeshCache::default();
            let mut rasterizer = SolidRasterizer::new(16, 16);
            let mut edges_rasterizer = SolidRasterizer::new(16, 16);
            let mut memory = WorkerMemory::default();

            let frame = render_request(
                &mut mesh_cache,
                &mut rasterizer,
                &mut edges_rasterizer,
                &mut memory,
                planned_request(design, 0, (16, 16), 0),
            );
            assert!(frame.has_solid);
            assert!(frame.mesh_bounding_radius > 0.0);
        }

        /// `cad_todo.md` #73: a `Reproject`/`UpdateFacetOverlay` frame (an ordinary
        /// camera drag, or a hover/click highlight update) carries no `generation`
        /// of its own -- it must reuse whichever generation the LAST `Replan`
        /// recorded into `WorkerMemory`, exactly like `solved_masts`/`planes`
        /// already are, so a camera orbit right after an in-budget edit does not
        /// make `gui::SlintSolidSink::apply` think the design regressed to "no
        /// generation yet."
        #[test]
        fn a_reproject_frame_carries_forward_the_last_replans_generation() {
            let design = closed_design();
            let mut mesh_cache = MeshCache::default();
            let mut rasterizer = SolidRasterizer::new(16, 16);
            let mut edges_rasterizer = SolidRasterizer::new(16, 16);
            let mut memory = WorkerMemory::default();

            let replan_frame = render_request(
                &mut mesh_cache,
                &mut rasterizer,
                &mut edges_rasterizer,
                &mut memory,
                planned_request(design, 0, (16, 16), 42),
            );
            assert_eq!(replan_frame.generation, 42);

            let reproject_frame = render_request(
                &mut mesh_cache,
                &mut rasterizer,
                &mut edges_rasterizer,
                &mut memory,
                RedrawRequest::Reproject {
                    planes: replan_frame.planes,
                    camera: CAMERA,
                    size: (16, 16),
                    view_mode: 0,
                    gear: None,
                },
            );
            assert_eq!(
                reproject_frame.generation, 42,
                "a camera-follow reproject must not lose the last replan's generation"
            );
        }
    }
}
