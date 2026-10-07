//! Where a finished (or status-only) solid-preview frame goes once the worker
//! thread has it -- the [`PreviewSink`] trait and its [`PreviewFrame`] payload.

use super::types::{FacetOwners, FrameGeometry, PickBuffer};
use glam::Vec3;
use indicatrix::geometry::{ToolPrimitive, meet_solver::SolvedTier};
use indicatrix_cut_core::{Design, ManufacturabilityWarning};
use std::sync::Arc;

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
/// `!has_solid`, empty otherwise -- EXCEPT for a [`super::live_update::Freshness::
/// Unsolvable`] `Planned` frame, which always sets a "Preview cannot be solved: ..."
/// `status` regardless of `has_solid`, since the drawn solid (if any) is the held-
/// over previous one, not this edit's own result (see `super::plan_worker::
/// build_planned_frame`'s doc comment). `solved` is `super::live_update::
/// PreviewPlan::solved` for a `super::request::RedrawRequest::Planned` request in
/// every other case -- `None` when no solve could produce masts, an `Unsolvable`
/// frame in particular: it carries no masts, so the shared `last_solved` cache keeps
/// the masts of the design that really was solved. A `Reproject` frame carries the
/// worker's last masts forward under its last planned generation (`solved` is then
/// only a repeat, never a solve of its own: a sink files masts from `planned`
/// frames only). `stale` mirrors `super::live_update::Freshness::Stale`
/// (always `false` for `Reproject`, and for `Unsolvable` too -- a distinct case
/// from staleness, carried instead through `status`). `pick` is this frame's
/// facet-picking buffer. `edges_image` is `Some` only when `view_mode` was `2`
/// (Both) and the arrangement closed.
///
/// A raw `slint::SharedPixelBuffer<slint::Rgba8Pixel>`, deliberately NOT a
/// `slint::Image` (not `Send`, so it cannot cross the closure above). A real
/// implementation converts via `slint::Image::from_rgba8` on the UI thread.
pub trait PreviewSink: Send + Sync + 'static {
    fn apply(&self, frame: PreviewFrame);

    /// Called from the PLAN worker once it has the manufacturability findings of a plan whose
    /// frame it already handed on (see [`LateFindings`]). Never called for a plan that has
    /// none, nor for a Slice-tool provisional plan. The default does nothing: a sink that
    /// only draws has no rows to refresh.
    fn apply_findings(&self, _findings: LateFindings) {}
}

/// The manufacturability findings of one plan, delivered AFTER its frame.
///
/// The frame is submitted first so the picture never waits for the costly concave-tool check
/// (check 6); the findings follow as their own update. They describe exactly the plan for
/// [`Self::generation`]: its `design` and its freshly solved `masts`, which the sink needs to
/// rebuild the rows the findings badge.
pub struct LateFindings {
    /// The generation of the plan the findings were computed for.
    pub generation: u64,
    /// The planned design (the allocation the plan job carried).
    pub design: Arc<Design>,
    /// The plan's own solved masts.
    pub masts: Vec<SolvedTier>,
    /// The full pass for `design` and `masts`.
    pub warnings: Arc<Vec<ManufacturabilityWarning>>,
}

/// One finished worker-thread render -- everything `super::render::render_request`
/// produces. See [`PreviewSink`]'s doc comment for what each field means.
pub struct PreviewFrame {
    pub image: slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    pub has_solid: bool,
    pub status: String,
    pub solved: Option<Vec<SolvedTier>>,
    pub stale: bool,
    pub pick: PickBuffer,
    pub edges_image: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    /// View mode 3's three-panel 2D faceting diagram (`diagram2d::render_diagram`),
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
    /// The index wheel's own per-pixel tooth-picking buffer. A SEPARATE buffer
    /// from `diagram_pick` (different hit regions), using the same `PickBuffer`
    /// encoding. `None` when no diagram or arrangement did not close.
    pub diagram_tooth_pick: Option<PickBuffer>,
    /// Which panel painted each pixel: the same `PickBuffer` encoding, but a lookup
    /// yields the panel index (`0` Crown, `1` Pavilion, `2` Profile) that
    /// `SolidPreviewModel.diagram_enlarged_panel` uses. Backs the double-click that
    /// enlarges a panel. `None` when no diagram or arrangement did not close.
    pub diagram_panel_pick: Option<PickBuffer>,
    /// Facet id -> full hover tooltip text (`facet_map::FacetMap::hover_text`),
    /// built alongside the diagram image so the UI thread can resolve a diagram
    /// hover without needing `Design`/`FacetMap` access itself. `None` when no
    /// diagram was requested this frame; a `super::request::RedrawRequest::
    /// Reproject` request in view mode 3 carries forward the WORKER's last-known
    /// table rather than rebuilding it (there is no `Design` to rebuild it from on
    /// that path).
    pub diagram_hover_text: Option<Vec<String>>,
    /// Facet id -> owning flat tier (`facet_map::FacetMap::tier_of`) and owning concave
    /// tier, the other half of what a diagram click needs to select a tier -- see
    /// `diagram_hover_text`'s doc comment for why this travels with the frame
    /// instead of being recomputed on the UI thread.
    pub diagram_facet_owners: Option<FacetOwners>,
    /// The plane arrangement this frame was actually rendered from, in the same
    /// `(normal, offset)` convention every other `solid_preview` module uses --
    /// ALWAYS present (never gated on view mode, unlike `diagram_pick`), so a real
    /// implementation (`SlintSolidSink::apply`) can publish it back into
    /// `bridge::render_thread::RenderContext::active_planes` alongside the image
    /// swap.
    ///
    /// Only `gui::editor::view::refresh_viewport`/`gui::editor::auto_solve` write
    /// `RenderContext::active_planes` directly, and neither runs for an ordinary
    /// in-budget edit (`submit_preview_replan`) -- so publishing every frame's own
    /// `planes` here, with the sink overwriting `RenderContext::active_planes`
    /// only when they actually differ, is what keeps the two in sync without
    /// `gui::editor` needing to remember to do it itself. Otherwise the next
    /// camera orbit would re-issue `RenderContext`'s stale, PRE-EDIT
    /// `active_planes` (`render::camera_lighting::resubmit_at_current_pose`),
    /// snapping the shown geometry back until the next explicit Solve.
    pub planes: Vec<(Vec3, f32)>,
    /// The concave tools subtracted from [`Self::planes`] in this frame (empty for a
    /// planar design), published to the path tracer beside the planes for the same
    /// reason: a frame the editor never claimed directly (an in-budget edit's replan)
    /// would otherwise leave `RenderContext::active_tools` describing the pre-edit stone.
    pub tools: Vec<ToolPrimitive>,
    /// `(concave tier, placement)` of each of [`Self::tools`].
    pub placements: Vec<(usize, usize)>,
    /// Facet id -> full hover tooltip text (`facet_map::FacetMap::hover_text`),
    /// valid for EVERY view mode -- the Solid view's own counterpart to
    /// `diagram_hover_text`, which is only populated for Diagram-mode requests.
    ///
    /// Rebuilding a fresh `facet_map::FacetMap` from `EditorState`'s `Design` on
    /// every mouse move would be an O(plane-count^2) candidate/dedup pass, and
    /// matching it against whatever `pick` buffer `SlintSolidSink` happened to
    /// have stored risks a stale generation right after an
    /// `AddTier`/`RemoveTier`/`Undo` (the two id spaces only agree once the next
    /// replan lands). This table travels WITH the same frame as `pick`, computed
    /// once on the worker thread (`update_diagram_memory_from_design`,
    /// unconditionally run for every `Replan`), so indexing it can never disagree
    /// with the displayed frame's own facet ids. See `hover_text`'s handoff note
    /// at its `SlintSolidSink` storage site (`gui::mod`) for what still needs to
    /// change in `tier_actions.rs` to actually use it.
    pub hover_text: Vec<String>,
    /// Facet id -> owning flat tier (`facet_map::FacetMap::tier_of`) and owning concave
    /// tier, the Solid view's own counterpart to `diagram_facet_owners` -- see
    /// `hover_text`'s doc comment.
    pub facet_owners: FacetOwners,
    /// The design-state generation this frame reflects -- `super::request::
    /// ReplanRequest::generation` for a `Replan` frame, or `super::state::
    /// WorkerMemory::generation` (carried forward unchanged) for a `Reproject`/
    /// `UpdateFacetOverlay` one; `0` for a frame from a non-`editor` build, or
    /// before this worker thread has ever received a `Replan`. Lets a real
    /// [`PreviewSink`] recognize a frame whose already-solved [`Self::solved`]
    /// masts still describe the LIVE design, and rebuild the tier table's rows
    /// from them directly instead of leaving that to a second, separately
    /// dispatched `Design::solve()`.
    pub generation: u64,
    /// `true` for a frame answering a [`super::request::RedrawRequest::Planned`]
    /// request (a fresh replan of the design); `false` for a `Reproject`/
    /// `UpdateFacetOverlay` frame, whose [`Self::generation`] is only the worker's
    /// LAST planned generation carried forward. `gui::solid_sink`'s out-of-order
    /// guard applies to planned frames only: a camera-follow frame legitimately
    /// trails the `solid_last_solved` watermark whenever that cache was written by a
    /// redraw-only path (`gui::editor::view::refresh_viewport`,
    /// `gui::editor::auto_solve::apply::push_viewport_after_background_solve`),
    /// neither of which ever sends a `Planned` request.
    pub planned: bool,
    /// The current solid's own bounding radius (`mesh_cache::CachedMesh::
    /// bounding_radius`) -- whichever mesh this frame actually rendered
    /// from (a fresh build, or the dimmed `last_closed` fallback `super::render::
    /// render_request` uses when the live arrangement does not close), or
    /// [`DEFAULT_MESH_BOUNDING_RADIUS`] before anything has ever closed. Lets a
    /// real [`PreviewSink`] keep the orbit camera's own distance clamp
    /// (`render::camera_lighting`) sized to the design actually loaded instead
    /// of a fixed range that clips a large preform or strands a tiny one in
    /// empty space.
    pub mesh_bounding_radius: f64,
    /// The geometry of the mesh this frame was drawn from (corner points, facet
    /// centroids, bounding radius) together with the camera pose and raster size the
    /// rasterizer used -- what the direct-manipulation handles project with. `None`
    /// only when no closed solid was ever built. Stored by the sink inside the same
    /// UI-thread closure that swaps `pick`, so it always describes the pick buffer a
    /// click resolves against.
    pub geometry: Option<FrameGeometry>,
    /// The full manufacturability findings of this frame's design (all six checks,
    /// including the costly concave-tool check 6), computed by the PLAN worker for
    /// [`Self::generation`] -- `None` when the worker has no findings for that
    /// generation (a design that was never planned, or a solve that could not produce
    /// masts) or has not finished them yet: the pass runs AFTER the frame is handed on, so a
    /// frame the render worker draws first comes without them and the sink gets them from
    /// [`PreviewSink::apply_findings`]. A sink that fills the tier table from
    /// [`Self::solved`] passes them to the row builders instead of running the pass itself,
    /// so the UI thread never builds a solid from the planes and tools.
    pub warnings: Option<Arc<Vec<ManufacturabilityWarning>>>,
}

/// [`PreviewFrame::mesh_bounding_radius`]'s fallback before any arrangement has
/// ever closed (`1.5`, roughly a standard round brilliant's own half-width) --
/// moved with the render step to `indicatrix_solid::preview`, re-exported here.
pub use indicatrix_solid::preview::DEFAULT_MESH_BOUNDING_RADIUS;
