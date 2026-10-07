//! The real [`PreviewSink`] that hops a finished solid-preview frame back onto the
//! UI thread, plus the trace/solid plane-arrangement agreement check it feeds.

mod late_rows;

use crate::{
    MainWindow, SolidPreviewModel,
    bridge::render_thread::{PlanesOwner, RenderContext},
    gui::{
        editor,
        solid_preview::{
            cut_slider,
            diagram_wiring::{DiagramFacetOwners, DiagramHoverText, DiagramPick},
            facet_selection,
            preview_state::{
                FacetOwners, FrameGeometry, LateFindings, PickBuffer, PreviewFrame, PreviewSink,
                SolidLastSolved,
            },
        },
    },
};
use indicatrix::geometry::{ToolPrimitive, meet_solver::SolvedTier, plane::GpuFacetPlane};
use indicatrix_cut_core::ManufacturabilityWarning;
use late_rows::LateRows;
use slint::{ComponentHandle, Weak};
use std::{
    cell::RefCell,
    sync::{Arc, Mutex},
    time::Duration,
};

thread_local! {
    /// Which committed frames have pushed their rows, and findings that arrived before their
    /// frame -- see [`LateRows`]. Only ever touched inside [`PreviewSink`]'s
    /// `upgrade_in_event_loop` closures, so it lives on the UI thread.
    static LATE_ROWS: RefCell<LateRows> = RefCell::default();
}

/// The real [`PreviewSink`] -- hops back to the UI thread exactly like
/// `bridge::render_thread::frame_helpers::push_frame_to_ui` already does for the
/// path-traced view, pushing a finished solid-preview frame into `MainWindow`'s
/// `editor_solid_image`/`editor_has_solid`/`editor_solid_status`/`editor_solid_stale`/
/// `editor_solid_edges_image`/`editor_has_solid_edges`/`editor_solid_diagram_image`/
/// `editor_has_diagram` properties. See `gui::solid_preview::preview_state`'s own
/// module doc comment ("`PreviewSink`: kept generic over Slint on purpose") for why
/// this hop lives here rather than inside `SolidPreviewState` itself.
///
/// `pick`/`last_solved`/`diagram_pick`/`diagram_hover_text`/`diagram_facet_owners` are
/// plain `Arc<Mutex<..>>` state, not Slint properties, so `gui::editor`'s hover/click
/// callbacks, `solid_preview::diagram_wiring`'s Diagram-mode hover/click callbacks,
/// and the next edit's [`solid_preview::preview_state::ReplanRequest::last_solved`]
/// can all read them without needing a Slint window handle of their own. See
/// `preview_state`'s own module doc comment ("Where `last_solved` lives") for why
/// these live here rather than on `gui::editor::state::EditorState`.
///
/// They are stored inside [`PreviewSink::apply`]'s `upgrade_in_event_loop` closure,
/// on the UI thread, together with the image swap that closure already performs --
/// NOT immediately on the calling worker thread. See that `impl`'s own doc comment
/// ("Atomicity guarantee") for why: a click landing between an immediate worker-
/// thread store and a deferred image swap would resolve against a pick buffer
/// newer than the image still on screen.
pub(super) struct SlintSolidSink {
    pub(super) ui: Weak<MainWindow>,
    pub(super) pick: Arc<Mutex<Option<PickBuffer>>>,
    pub(super) last_solved: SolidLastSolved,
    // Diagram mode's (view_mode 3) own pick buffer/hover-text/tier tables -- a
    // separate `Arc<Mutex<..>>` triple from `pick`/`last_solved` above, since the
    // diagram is an entirely different pixel layout resolved without ever needing
    // `Design`/`FacetMap` access on the UI thread (see
    // `solid_preview::diagram_wiring`'s own module doc comment).
    pub(super) diagram_pick: DiagramPick,
    pub(super) diagram_hover_text: DiagramHoverText,
    pub(super) diagram_facet_owners: DiagramFacetOwners,
    /// The index wheel's own per-pixel tooth-picking buffer
    /// (`PreviewFrame::diagram_tooth_pick`) -- a separate buffer from
    /// `diagram_pick` above, same reasoning (the diagram's pixel layout has no
    /// relation to the Solid rasterizer's own pick buffer). Reuses the
    /// `DiagramPick` type alias since it is exactly the same shape
    /// (`Arc<Mutex<Option<PickBuffer>>>`).
    pub(super) diagram_tooth_pick: DiagramPick,
    /// The diagram's per-pixel panel buffer (`PreviewFrame::diagram_panel_pick`),
    /// which the double-click that enlarges a panel reads. Same shape and lifetime
    /// as `diagram_tooth_pick`.
    pub(super) diagram_panel_pick: DiagramPick,
    /// The Solid view's own facet id -> hover-tooltip-text table, stored
    /// alongside `pick` from every frame's [`PreviewFrame::hover_text`] so a future
    /// `gui::editor::callbacks::tier_actions` hover/click handler can index it
    /// instead of rebuilding a `facet_map::FacetMap` per mouse move. See this
    /// field's `build_main_window` construction site (`solid_hover_text`) for the
    /// handoff this sets up but does not finish (that indexing change lives
    /// elsewhere in `gui::editor`).
    pub(super) hover_text: Arc<Mutex<Vec<String>>>,
    /// The Solid view's own facet id -> owning flat and concave tier tables, from every
    /// frame's [`PreviewFrame::facet_owners`] -- see `hover_text`'s doc comment.
    pub(super) facet_owners: Arc<Mutex<FacetOwners>>,
    /// The shared render context.
    ///
    /// A finished frame's own [`PreviewFrame::planes`] are published back into
    /// `RenderContext::active_planes` here (only when they differ from what is
    /// already there by more than `PLANE_REPUBLISH_EPSILON` per component, so an
    /// ULP-level `normalize()` wobble never counts), so a camera orbit right after
    /// an in-budget live
    /// edit re-issues the just-edited geometry instead of snapping back to
    /// whatever `RenderContext` last held -- see [`PreviewFrame::planes`]'s own
    /// doc comment for the full mechanism.
    ///
    /// Also read (via `active_planes`'s `Arc` pointer identity) to decide
    /// whether this frame's own geometry still matches the path tracer's
    /// last-pushed one -- see `solid_active_planes`/`trace_active_planes` below.
    pub(super) render_ctx: Arc<Mutex<RenderContext>>,
    /// The plane arrangement (`RenderContext::active_planes`, by `Arc`
    /// pointer identity) this sink last published a solid-preview frame with.
    /// Compared against `trace_active_planes` (written by the path tracer's own
    /// frame-push closure in [`super::main_window::build_main_window`]) to keep
    /// `SolidPreviewModel.trace_matches_solid` current -- see
    /// [`planes_generations_match`].
    pub(super) solid_active_planes: Arc<Mutex<Option<Arc<Vec<GpuFacetPlane>>>>>,
    /// The path tracer's own last-pushed plane arrangement -- see
    /// `solid_active_planes`'s doc comment.
    pub(super) trace_active_planes: Arc<Mutex<Option<Arc<Vec<GpuFacetPlane>>>>>,
    /// The current solid's own bounding radius
    /// (`PreviewFrame::mesh_bounding_radius`), stashed here so
    /// `render::camera_lighting`'s orbit-zoom clamp and "Fit" pose can read the
    /// design actually loaded instead of a fixed range. Read by `camera_lighting::
    /// setup_camera_and_lighting_callbacks`'s own copy of this `Arc`, written
    /// only here.
    pub(super) mesh_bounding_radius: Arc<Mutex<f64>>,
    /// The mesh geometry, camera pose and raster size of the frame on screen
    /// (`PreviewFrame::geometry`), stored inside the same UI-thread closure that
    /// swaps `pick` -- so the manipulation handles are always projected with the
    /// pose and size of the pick buffer a click resolves against. Shared with
    /// `SolidPickState::geometry`.
    pub(super) geometry: Arc<Mutex<Option<FrameGeometry>>>,
}

/// Whether the LAST path-traced frame and the LAST solid-preview frame were
/// both produced from the exact same plane arrangement, compared by `Arc` pointer
/// identity of `RenderContext::active_planes` at the moment each was pushed --
/// never by value, since two DIFFERENT designs can coincidentally solve to
/// identical planes, and a cheap pointer compare is all either frame-push closure
/// can afford to do on the UI thread on every frame.
///
/// `true` whenever either side has not published a frame yet -- nothing to
/// disagree with, matching `SolidPreviewModel.trace_matches_solid`'s own
/// documented default.
pub(super) fn planes_generations_match(
    trace: &Mutex<Option<Arc<Vec<GpuFacetPlane>>>>,
    solid: &Mutex<Option<Arc<Vec<GpuFacetPlane>>>>,
) -> bool {
    let trace = trace
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let solid = solid
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match (trace, solid) {
        (Some(trace), Some(solid)) => Arc::ptr_eq(&trace, &solid),
        _ => true,
    }
}

/// Largest per-component difference two plane arrangements may show and still count
/// as the SAME geometry for `RenderContext::active_planes` publication. A camera-follow
/// frame's planes are `RenderContext::active_planes` round-tripped through
/// `resubmit_at_current_pose` and `GpuFacetPlane::new`, whose `normalize()` is not
/// idempotent in `f32` (about one unit normal in six moves by one ULP and then
/// oscillates with period 2) -- an exact compare therefore failed on nearly every
/// frame, republishing a fresh `Arc` (flipping `SolidPreviewModel.trace_matches_solid`
/// until the tracer's next push) and restarting accumulation for a geometry change of
/// ~1e-7. Anything a solver can distinguish is orders of magnitude above this.
const PLANE_REPUBLISH_EPSILON: f32 = 1e-5;

/// Whether `a` and `b` describe the same plane arrangement within `epsilon` per
/// component (same length, same order; `normal[0..3]` and `d`).
#[must_use]
fn planes_equal_within(a: &[GpuFacetPlane], b: &[GpuFacetPlane], epsilon: f32) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(left, right)| {
            (left.d - right.d).abs() <= epsilon
                && left
                    .normal
                    .iter()
                    .zip(&right.normal)
                    .all(|(l, r)| (l - r).abs() <= epsilon)
        })
}

/// The stone a solid-preview frame was rendered from, as
/// [`sync_planes_and_check_trace_match`] publishes it: the `(normal, offset)` planes, the
/// concave tools cut out of them and each tool's `(tier, placement)`.
#[derive(Clone, Copy)]
struct StonePublish<'a> {
    planes: &'a [(glam::Vec3, f32)],
    tools: &'a [ToolPrimitive],
    placements: &'a [(usize, usize)],
}

impl<'a> StonePublish<'a> {
    const fn new(
        planes: &'a [(glam::Vec3, f32)],
        tools: &'a [ToolPrimitive],
        placements: &'a [(usize, usize)],
    ) -> Self {
        Self {
            planes,
            tools,
            placements,
        }
    }
}

/// Publishes `stone`'s planes into `render_ctx`'s `active_planes` -- and its concave
/// tools into `active_tools` with them -- when they actually differ (planes beyond
/// [`PLANE_REPUBLISH_EPSILON`] per component, tools exactly: they are copied, never
/// re-normalised, so a round trip leaves them bit-equal) from what is already
/// there (skipped entirely when `planes` is empty, so
/// this never clobbers `RenderContext::default`'s own placeholder cut), stashes
/// the resulting (possibly unchanged) arrangement into `solid_active_planes`, and
/// returns whether it now agrees with `trace_active_planes` (via
/// [`planes_generations_match`]). Split out of [`SlintSolidSink::apply`] purely to
/// keep that function under clippy's function-length lint -- see
/// [`PreviewFrame::planes`]'s doc comment for the full mechanism and
/// [`SlintSolidSink::solid_active_planes`]'s doc comment for the plane-arrangement
/// comparison this feeds.
///
/// A write here goes through `RenderContext::claim_active_planes` tagged
/// `PlanesOwner::Editor { generation }`, the same arbitration
/// `editor::auto_solve::apply_background_solve_result` already uses, so a solid-preview
/// frame that is behind the live design (a slow replan for design A still finishing
/// after New/Load switched to design B) can never clobber a newer claim's planes --
/// the same generation check that `editor::apply_matching_preview_frame` (called
/// just above this function's call site, on the SAME frame's `generation`) applies
/// to the tier table also applies here to `active_planes`. A frame whose
/// `generation` is `0` -- no `Replan` has ever landed for the live design, see
/// [`PreviewFrame::generation`]'s own doc comment -- is never published at all.
fn sync_planes_and_check_trace_match(
    stone: &StonePublish<'_>,
    render_ctx: &Mutex<RenderContext>,
    solid_active_planes: &Mutex<Option<Arc<Vec<GpuFacetPlane>>>>,
    trace_active_planes: &Mutex<Option<Arc<Vec<GpuFacetPlane>>>>,
    generation: u64,
) -> bool {
    let StonePublish {
        planes,
        tools,
        placements,
    } = *stone;
    if !planes.is_empty() {
        let converted: Vec<GpuFacetPlane> = planes
            .iter()
            .map(|&(normal, offset)| GpuFacetPlane::new(normal, -offset))
            .collect();
        let mut ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A Library selection owns the slot until the editor claims it again with
        // a load or edit; a trailing solid frame must not flip the viewport back.
        if generation != 0
            && !matches!(ctx.planes_owner, PlanesOwner::Catalogue { .. })
            && (!planes_equal_within(
                ctx.active_planes.as_slice(),
                &converted,
                PLANE_REPUBLISH_EPSILON,
            ) || ctx.active_tools.as_slice() != tools)
        {
            let design_gear = ctx.design_gear;
            if ctx.claim_active_geometry(
                Arc::new(converted),
                Arc::new(tools.to_vec()),
                placements.to_vec(),
                design_gear,
                PlanesOwner::Editor { generation },
            ) {
                ctx.dirty = true;
            }
        }
        *solid_active_planes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(Arc::clone(&ctx.active_planes));
    }
    planes_generations_match(trace_active_planes, solid_active_planes)
}

/// Files a frame's freshly solved `masts`: into the shared `solid_last_solved` cache
/// (tagged with `generation`) for a frame of the committed design, or -- for the Slice
/// tool's provisional frames (`committed` is `false`) -- only to the provisional tier's
/// own session, which needs them for its outline and handles. Nothing else may see
/// a provisional frame's masts. Split out of [`SlintSolidSink::apply`] to keep that
/// function under clippy's function-length lint.
fn store_solved(
    committed: bool,
    generation: u64,
    masts: Vec<SolvedTier>,
    last_solved: &SolidLastSolved,
) {
    if committed {
        *last_solved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((generation, masts));
    } else {
        editor::note_provisional_frame_masts(masts);
    }
}

/// Overwrites `state` with `value` (poison-tolerant): one line per per-frame table
/// [`SlintSolidSink::apply`] swaps, which keeps that function under clippy's
/// function-length lint.
fn store_value<T>(state: &Mutex<T>, value: T) {
    *state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = value;
}

/// Stores `value` into `state` when `Some`, leaving `state` untouched for `None`
/// -- the Diagram-mode side tables (`PreviewFrame::diagram_pick`/
/// `diagram_tooth_pick`/`diagram_hover_text`/`diagram_facet_owners`) are only ever
/// `Some` together, for a `view_mode`-3 request (see `PreviewFrame::
/// diagram_hover_text`'s own doc comment). Split out of
/// [`SlintSolidSink::apply`] purely to keep that function under clippy's
/// function-length lint: four separate four-line `if let` blocks collapse to one
/// call each.
fn store_if_some<T>(state: &Mutex<Option<T>>, value: Option<T>) {
    if let Some(value) = value {
        *state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(value);
    }
}

/// Pushes a committed frame's tier rows, banner and yield figures from its own `masts`.
///
/// `warnings` is the manufacturability pass the plan worker ran for the frame; it rides on the
/// frame so the tier table only displays it (check 6 of the pass builds a solid, which a UI
/// thread must not). A frame that came without the findings is followed by the rows' second
/// push, once [`LateRows`] says its findings are due. Split out of [`SlintSolidSink::apply`] to
/// keep that function under clippy's function-length lint.
///
/// [`LateRows`] is told about the frame only when it really pushed rows (see
/// [`late_rows::land_committed_frame`]), and a frame that comes without its own findings is given
/// the kept copy for its generation in that first push, so a Cut slider drag builds each frame's
/// rows once.
fn push_committed_rows(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    generation: u64,
    masts: &[SolvedTier],
    warnings: Option<&Arc<Vec<ManufacturabilityWarning>>>,
) {
    let due = LATE_ROWS.with(|rows| {
        late_rows::land_committed_frame(rows, generation, warnings, |shown| {
            editor::apply_matching_preview_frame(ui, render_ctx, generation, masts, shown)
        })
    });
    push_due_findings(ui, render_ctx, due, FINDINGS_RETRIES);
}

/// Records which flat tiers the committed frame just stored contains (the Cut slider's cut),
/// for the facet maps the tier-selection outlines are built from. A provisional frame
/// describes a design the editor does not have, so it records nothing.
fn note_drawn_cut(committed: bool, geometry: Option<&FrameGeometry>) {
    if committed {
        facet_selection::note_drawn_tiers(geometry.and_then(|shown| shown.visible_tiers.clone()));
    }
}

/// Whether a landed frame is an out-of-order PLANNED frame -- one whose own
/// `generation` is strictly older than the `solid_last_solved` watermark
/// (`cached_generation`) a later frame or redraw-only path already moved past -- and
/// so must not replace anything on screen. A camera-follow frame (`planned == false`)
/// is never superseded by this rule: its `generation` is only the worker's last
/// planned generation carried forward, and it legitimately trails the watermark after
/// `editor::view::refresh_viewport` or `editor::auto_solve::apply::
/// push_viewport_after_background_solve`, both of which write the cache and send only
/// a `Reproject` (see `PreviewFrame::planned`). Dropping those froze the solid raster
/// while the path tracer kept orbiting.
pub(super) const fn frame_is_superseded(
    planned: bool,
    generation: u64,
    cached_generation: u64,
) -> bool {
    planned && generation < cached_generation
}

/// Whether a landed frame's masts may be filed in the shared `solid_last_solved` cache, and
/// pushed to the tier table, under the frame's own `generation`: only a PLANNED frame that is
/// not behind the cache's watermark (`superseded`).
///
/// A camera-follow frame (`Reproject`/`UpdateFacetOverlay`) carries the worker's LAST masts
/// forward under the worker's last planned generation. After an `Unsolvable` plan that
/// generation has no masts of its own -- the list is the previous design's -- and filing it
/// would make the exact-generation test of `gui::editor::finished_stone` accept the new angles
/// on the old masts as the finished stone. Every generation that DID solve has filed its masts
/// from its planned frame already, so a camera-follow frame can only repeat them.
pub(in crate::gui) const fn frame_files_masts(planned: bool, superseded: bool) -> bool {
    planned && !superseded
}

/// How long findings that met a held editor state wait before the next try.
const FINDINGS_RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// How many times findings that met a held editor state are tried again: three seconds in
/// all. A state that is still held after that belongs to a writer that keeps the event loop
/// busy for longer; the rows then get the findings with the next frame of their generation
/// ([`late_rows::land_committed_frame`] hands the kept copy out again).
const FINDINGS_RETRIES: u8 = 30;

/// Pushes the rows the findings of a late-arriving plan belong to, when they are due (see
/// [`LateRows`]). `due` is what the state machine returned for the frame or the findings
/// just handled. Split out of [`PreviewSink::apply`] and [`PreviewSink::apply_findings`] to
/// keep them short.
///
/// A push that finds the editor state held by a callback (one that pumps the event loop) did
/// not show anything: the findings go back to [`LateRows`] as still owed, and are tried again
/// on a timer, `retries_left` more times. They are never marked as shown on a refusal.
fn push_due_findings(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    due: Option<Arc<LateFindings>>,
    retries_left: u8,
) {
    let Some(late) = due else {
        return;
    };
    let outcome = editor::apply_late_findings(
        ui,
        render_ctx,
        late.generation,
        &late.design,
        &late.masts,
        &late.warnings,
    );
    if outcome != editor::LatePush::Held {
        return;
    }
    LATE_ROWS.with(|rows| rows.borrow_mut().put_back(&late));
    let Some(next) = cut_slider::retries_after_busy(retries_left) else {
        return;
    };
    let ui_weak = ui.as_weak();
    let render_ctx = Arc::clone(render_ctx);
    slint::Timer::single_shot(FINDINGS_RETRY_INTERVAL, move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let due = LATE_ROWS.with(|rows| rows.borrow_mut().owed());
        push_due_findings(&ui, &render_ctx, due, next);
    });
}

impl PreviewSink for SlintSolidSink {
    /// # Atomicity guarantee
    ///
    /// Every piece of this frame's state -- `pick`/`last_solved`/the Diagram-mode
    /// side tables AND the displayed image -- is swapped together, inside this one
    /// `upgrade_in_event_loop` closure, on the UI thread. Storing the pick buffers
    /// immediately on the WORKER thread while deferring only the image swap to the
    /// UI thread's event loop would open a window where a click landing on the OLD
    /// (still-displayed) image resolves against the NEW pick buffer already stashed
    /// for the next frame, picking the wrong facet.
    /// Deferring every store into the closure closes that window: whichever frame's
    /// image is on screen, that SAME frame's pick/solved/diagram data is what a click
    /// arriving right after the swap will see -- never a newer buffer paired with an
    /// older image, or vice versa.
    fn apply(&self, frame: PreviewFrame) {
        let PreviewFrame {
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
            diagram_panel_pick,
            diagram_hover_text,
            diagram_facet_owners,
            planes,
            tools,
            placements,
            hover_text,
            facet_owners,
            generation,
            planned,
            mesh_bounding_radius,
            geometry,
            warnings,
        } = frame;

        // Cloned Arcs (cheap), not `self` -- the closure below outlives this call.
        let pick_state = Arc::clone(&self.pick);
        let last_solved_state = Arc::clone(&self.last_solved);
        let diagram_pick_state = Arc::clone(&self.diagram_pick);
        let diagram_tooth_pick_state = Arc::clone(&self.diagram_tooth_pick);
        let diagram_panel_pick_state = Arc::clone(&self.diagram_panel_pick);
        let diagram_hover_text_state = Arc::clone(&self.diagram_hover_text);
        let diagram_facet_owners_state = Arc::clone(&self.diagram_facet_owners);
        let hover_text_state = Arc::clone(&self.hover_text);
        let facet_owners_state = Arc::clone(&self.facet_owners);
        let render_ctx_state = Arc::clone(&self.render_ctx);
        let solid_active_planes_state = Arc::clone(&self.solid_active_planes);
        let trace_active_planes_state = Arc::clone(&self.trace_active_planes);
        let mesh_bounding_radius_state = Arc::clone(&self.mesh_bounding_radius);
        let geometry_state = Arc::clone(&self.geometry);

        // `slint::Image::from_rgba8` runs HERE, not on the worker thread -- see
        // `gui::solid_preview::to_pixel_buffer`'s doc comment (`slint::Image` is not
        // `Send`).
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            // `EditorState::generation` is one ever-increasing counter
            // for the whole session (see `auto_solve::scheduling::
            // reset_for_new_design`'s own doc comment), so the generation
            // already cached here is a reliable "highest frame applied so
            // far" watermark -- it rejects an out-of-order PLANNED frame: one
            // whose OWN generation is strictly
            // older than it arrived out of order (a slow in-flight solve for
            // a design a NEWER frame has already superseded) and must not
            // replace any of this frame's image/pick/status/masts with older
            // content a later frame has already moved past. Comparing
            // against this cache (rather than reaching into `gui::editor`'s
            // own, privately-scoped live-generation state from this sibling
            // module) is enough: any legitimate in-order frame's own
            // `generation` is always >= whatever is already cached, by
            // construction (`ReplanRequest::generation`'s own doc comment).
            let cached_generation = last_solved_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .map_or(0, |(cached_generation, _)| *cached_generation);
            // A camera-follow frame behind the watermark still redraws (see
            // `frame_is_superseded`); it only must not push its older masts back
            // into the cache or the tier table below -- `frame_files_masts` gates those.
            let superseded = generation < cached_generation;
            if frame_is_superseded(planned, generation, cached_generation) {
                return;
            }
            // The Slice tool's PROVISIONAL frames (`gui::editor::manipulate::
            // PROVISIONAL_GENERATION`, the reserved generation just below
            // `u64::MAX`) show the committed design plus a tier that is not in
            // `EditorState`. Everything that describes "what is on screen" below
            // (pick buffer, hover/tier tables, geometry, images) follows them, but
            // everything that describes "the committed design" must not: they never
            // reach `solid_last_solved` (the next replan's mast chain and the
            // manipulation handles' facet map), `apply_matching_preview_frame` (the
            // tier table would list a tier the design does not have) or the path
            // tracer's plane slot.
            let committed = editor::frame_updates_mast_cache(generation);
            store_value(&pick_state, Some(pick));
            // Which tiers this frame's facet ids number (the Cut slider's cut), for the
            // facet maps the tier-selection outlines are built from. A provisional
            // frame describes a design the editor does not have.
            note_drawn_cut(committed, geometry.as_ref());
            // The pose/size/mesh geometry of this SAME frame, swapped with its pick
            // buffer -- see `SlintSolidSink::geometry`.
            store_value(&geometry_state, geometry);
            // Kept alongside the image/pick swap in this same atomicity
            // boundary -- see this function's own doc comment -- so the orbit
            // camera's distance clamp (`render::camera_lighting`) never reads a
            // radius that describes a different frame than the one on screen.
            store_value(&mesh_bounding_radius_state, mesh_bounding_radius);
            // Every replan frame's freshly solved masts land here, unconditionally,
            // on every edit -- including ones the tier table still painted "Not
            // solved" for, because `auto_solve`'s own debounced background solve
            // (`editor::auto_solve::on_edit`/`dispatch_background_solve`) is a
            // SEPARATE `Design::solve()` against the same design, racing this one.
            // `editor::apply_matching_preview_frame` resolves that race: when
            // `generation` still names the live design (see
            // `editor::auto_solve::take_matching_design`'s own doc comment for the
            // exact check -- a superseded frame, from a design an edit has since
            // moved past, is a deliberate no-op here), it pushes the tier table's
            // rows/status/warnings/yield figures straight from `solved` via
            // `editor::view::push_solved_preview`, AND cancels whatever debounced
            // auto-solve was about to recompute the exact same thing.
            // `solved.as_ref()` only borrows -- `solved` itself still moves into
            // `last_solved_state` below.
            //
            // The plan worker delivers the manufacturability findings AFTER its frame
            // (`PreviewSink::apply_findings`): a frame that came without them is followed
            // by a second push of the rows, which `LateRows` schedules once this one is out.
            let files_masts = frame_files_masts(planned, superseded);
            if committed
                && files_masts
                && let Some(masts) = solved.as_ref()
            {
                push_committed_rows(&ui, &render_ctx_state, generation, masts, warnings.as_ref());
            }
            // Only overwrite the shared cache with masts this frame SOLVED, tagged with
            // THIS frame's own `generation`: a planned frame. An `Unsolvable` plan carries
            // none (it leaves the cache alone, under the generation that did solve), and a
            // camera-follow `Reproject`/`UpdateFacetOverlay` frame only repeats the
            // worker's last masts under its last planned generation -- see
            // `frame_files_masts` for why filing those would be wrong.
            if files_masts && let Some(masts) = solved {
                store_solved(committed, generation, masts, &last_solved_state);
            }
            // A provisional frame's planes go to the Slice tool too (after its masts,
            // which it needs to recognise them): redraws that reproject the COMMITTED
            // planes -- a camera orbit, a view-mode switch, a background solve --
            // would otherwise draw the committed stone over the provisional facet.
            if !committed {
                editor::note_provisional_frame_planes(&planes);
            }
            // Diagram mode's side tables are only ever `Some` for a view_mode-3
            // request -- see `PreviewFrame::diagram_hover_text`'s doc comment.
            // `diagram_tooth_pick` shares that same "only `Some` together"
            // contract.
            store_if_some(&diagram_pick_state, diagram_pick);
            store_if_some(&diagram_tooth_pick_state, diagram_tooth_pick);
            store_if_some(&diagram_panel_pick_state, diagram_panel_pick);
            store_if_some(&diagram_hover_text_state, diagram_hover_text);
            store_if_some(&diagram_facet_owners_state, diagram_facet_owners);
            // Unconditional (unlike the Diagram-only tables above) -- see
            // `SlintSolidSink::hover_text`'s doc comment.
            store_value(&hover_text_state, hover_text);
            store_value(&facet_owners_state, facet_owners);

            // See `sync_planes_and_check_trace_match`'s own doc comment. Also
            // passes this SAME frame's `generation` (already destructured above,
            // for `apply_matching_preview_frame`) so a stale frame's planes can be
            // told apart from the live design's.
            let trace_matches = sync_planes_and_check_trace_match(
                &StonePublish::new(&planes, &tools, &placements),
                &render_ctx_state,
                &solid_active_planes_state,
                &trace_active_planes_state,
                // `0` means "never publish these planes" (see the function's doc
                // comment): a provisional frame's planes must not reach the tracer.
                if committed { generation } else { 0 },
            );
            let model = ui.global::<SolidPreviewModel>();
            model.set_trace_matches_solid(trace_matches);

            model.set_image(slint::Image::from_rgba8(image));
            model.set_has_solid(has_solid);
            model.set_status(status.into());
            model.set_stale(stale);
            if let Some(edges_image) = edges_image {
                model.set_edges_image(slint::Image::from_rgba8(edges_image));
                model.set_has_solid_edges(true);
            } else {
                model.set_has_solid_edges(false);
            }
            if let Some(diagram_image) = diagram_image {
                model.set_diagram_image(slint::Image::from_rgba8(diagram_image));
            }
            model.set_has_diagram(has_diagram);
            // LAST, so every store above is already visible: the manipulation
            // handles refresh from here. The frame's own generation tells the Slice
            // tool whether this frame replaced its provisional picture.
            editor::on_solid_frame_landed(&ui, generation);
        });
    }

    /// The plan worker's manufacturability findings for a plan whose frame it handed on
    /// first. Hops to the UI thread like [`Self::apply`]; [`LateRows`] decides there whether
    /// the rows are refreshed now (their frame landed without the findings), later (the
    /// frame is still on its way) or not at all (the frame carried them, or the editor has
    /// moved on). A copy is kept for any further frame of the same generation (a tier
    /// selection or a Cut slider move plans the same design again), whose rows would
    /// otherwise start without them.
    fn apply_findings(&self, findings: LateFindings) {
        let render_ctx = Arc::clone(&self.render_ctx);
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            let due = LATE_ROWS.with(|rows| rows.borrow_mut().findings_arrived(findings));
            push_due_findings(&ui, &render_ctx, due, FINDINGS_RETRIES);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GpuFacetPlane, PLANE_REPUBLISH_EPSILON, frame_files_masts, frame_is_superseded,
        planes_equal_within,
    };
    use glam::Vec3;

    /// A few dozen planes with unit normals spread over yaw/pitch angles.
    fn sample_planes() -> Vec<GpuFacetPlane> {
        let mut planes = Vec::new();
        for yaw_step in 0..8_u8 {
            for pitch_step in 0..6_u8 {
                let yaw = f32::from(yaw_step) * 0.7;
                let pitch = f32::from(pitch_step).mul_add(0.4, -1.0);
                let normal = Vec3::new(
                    pitch.cos() * yaw.cos(),
                    pitch.sin(),
                    pitch.cos() * yaw.sin(),
                );
                planes.push(GpuFacetPlane::new(normal, -0.5));
            }
        }
        planes
    }

    #[test]
    fn identical_planes_are_equal() {
        let planes = sample_planes();
        assert!(planes_equal_within(
            &planes,
            &planes.clone(),
            PLANE_REPUBLISH_EPSILON
        ));
    }

    #[test]
    fn a_one_ulp_normal_difference_does_not_republish() {
        let planes = sample_planes();
        let mut perturbed = planes.clone();
        perturbed[3].normal[1] = f32::from_bits(perturbed[3].normal[1].to_bits() + 1);
        assert!(planes_equal_within(
            &planes,
            &perturbed,
            PLANE_REPUBLISH_EPSILON
        ));
    }

    #[test]
    fn a_real_geometry_change_republishes() {
        let planes = sample_planes();
        let mut changed = planes.clone();
        changed[3].normal[1] += 0.01;
        assert!(!planes_equal_within(
            &planes,
            &changed,
            PLANE_REPUBLISH_EPSILON
        ));
    }

    #[test]
    fn different_plane_counts_never_match() {
        let planes = sample_planes();
        assert!(!planes_equal_within(
            &planes,
            &planes[..planes.len() - 1],
            PLANE_REPUBLISH_EPSILON
        ));
    }

    /// Pins the behaviour behind the flicker: re-normalising an already published
    /// plane through `GpuFacetPlane::new` moves a normal by an ULP at most, which the
    /// republish tolerance must absorb (twice, since the wobble has period 2).
    #[test]
    fn gpu_facet_plane_new_round_trip_stays_within_epsilon() {
        let planes = sample_planes();
        let once: Vec<GpuFacetPlane> = planes
            .iter()
            .map(|p| GpuFacetPlane::new(Vec3::from(p.normal), p.d))
            .collect();
        let twice: Vec<GpuFacetPlane> = once
            .iter()
            .map(|p| GpuFacetPlane::new(Vec3::from(p.normal), p.d))
            .collect();
        assert!(planes_equal_within(&planes, &once, PLANE_REPUBLISH_EPSILON));
        assert!(planes_equal_within(
            &planes,
            &twice,
            PLANE_REPUBLISH_EPSILON
        ));
    }

    #[test]
    fn a_camera_follow_frame_behind_the_cache_watermark_is_never_superseded() {
        assert!(!frame_is_superseded(false, 3, 9));
    }

    #[test]
    fn a_planned_frame_behind_the_cache_watermark_is_superseded() {
        assert!(frame_is_superseded(true, 3, 9));
    }

    #[test]
    fn a_planned_frame_at_or_past_the_watermark_is_not_superseded() {
        assert!(!frame_is_superseded(true, 9, 9));
        assert!(!frame_is_superseded(true, 10, 9));
    }

    /// F4-8: masts are filed from planned frames that are not behind the watermark. A
    /// camera-follow frame repeats the worker's last masts under its last planned
    /// generation, which after an unsolvable plan is a generation those masts do not
    /// describe.
    #[test]
    fn only_a_current_planned_frame_files_its_masts() {
        assert!(frame_files_masts(true, false));
        assert!(!frame_files_masts(true, true), "behind the watermark");
        assert!(!frame_files_masts(false, false), "a camera-follow frame");
        assert!(!frame_files_masts(false, true));
    }
}
