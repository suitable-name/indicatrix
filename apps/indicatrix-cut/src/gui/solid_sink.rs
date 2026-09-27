//! The real [`PreviewSink`] that hops a finished solid-preview frame back onto the
//! UI thread, plus the trace/solid plane-arrangement agreement check it feeds.

use crate::{
    MainWindow, SolidPreviewModel,
    bridge::render_thread::{PlanesOwner, RenderContext},
    gui::{
        editor,
        solid_preview::{
            diagram_wiring::{DiagramFacetTier, DiagramHoverText, DiagramPick},
            preview_state::{PickBuffer, PreviewFrame, PreviewSink, SolidLastSolved},
        },
    },
};
use indicatrix::geometry::plane::GpuFacetPlane;
use slint::{ComponentHandle, Weak};
use std::sync::{Arc, Mutex};

/// The real [`PreviewSink`] -- hops back to the UI thread exactly like
/// `bridge::render_thread::frame_helpers::push_frame_to_ui` already does for the
/// path-traced view, pushing a finished solid-preview frame into `MainWindow`'s
/// `editor_solid_image`/`editor_has_solid`/`editor_solid_status`/`editor_solid_stale`/
/// `editor_solid_edges_image`/`editor_has_solid_edges`/`editor_solid_diagram_image`/
/// `editor_has_diagram` properties. See `gui::solid_preview::preview_state`'s own
/// module doc comment ("`PreviewSink`: kept generic over Slint on purpose") for why
/// this hop lives here rather than inside `SolidPreviewState` itself.
///
/// `pick`/`last_solved`/`diagram_pick`/`diagram_hover_text`/`diagram_facet_tier` are
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
    pub(super) diagram_facet_tier: DiagramFacetTier,
    /// The index wheel's own per-pixel tooth-picking buffer
    /// (`PreviewFrame::diagram_tooth_pick`) -- a separate buffer from
    /// `diagram_pick` above, same reasoning (the diagram's pixel layout has no
    /// relation to the Solid rasterizer's own pick buffer). Reuses the
    /// `DiagramPick` type alias since it is exactly the same shape
    /// (`Arc<Mutex<Option<PickBuffer>>>`).
    pub(super) diagram_tooth_pick: DiagramPick,
    /// The Solid view's own facet id -> hover-tooltip-text table, stored
    /// alongside `pick` from every frame's [`PreviewFrame::hover_text`] so a future
    /// `gui::editor::callbacks::tier_actions` hover/click handler can index it
    /// instead of rebuilding a `facet_map::FacetMap` per mouse move. See this
    /// field's `build_main_window` construction site (`solid_hover_text`) for the
    /// handoff this sets up but does not finish (that indexing change lives
    /// elsewhere in `gui::editor`).
    pub(super) hover_text: Arc<Mutex<Vec<String>>>,
    /// The Solid view's own facet id -> owning tier index table, from every
    /// frame's [`PreviewFrame::facet_tier`] -- see `hover_text`'s doc comment.
    pub(super) facet_tier: Arc<Mutex<Vec<Option<usize>>>>,
    /// The shared render context.
    ///
    /// A finished frame's own [`PreviewFrame::planes`] are published back into
    /// `RenderContext::active_planes` here (only when they actually differ from
    /// what is already there), so a camera orbit right after an in-budget live
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

/// Publishes `planes` into `render_ctx`'s `active_planes` when they actually
/// differ from what is already there (skipped entirely when `planes` is empty, so
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
    planes: &[(glam::Vec3, f32)],
    render_ctx: &Mutex<RenderContext>,
    solid_active_planes: &Mutex<Option<Arc<Vec<GpuFacetPlane>>>>,
    trace_active_planes: &Mutex<Option<Arc<Vec<GpuFacetPlane>>>>,
    generation: u64,
) -> bool {
    if !planes.is_empty() {
        let converted: Vec<GpuFacetPlane> = planes
            .iter()
            .map(|&(normal, offset)| GpuFacetPlane::new(normal, -offset))
            .collect();
        let mut ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if generation != 0 && ctx.active_planes.as_ref() != &converted {
            let design_gear = ctx.design_gear;
            if ctx.claim_active_planes(
                Arc::new(converted),
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

/// Stores `value` into `state` when `Some`, leaving `state` untouched for `None`
/// -- the Diagram-mode side tables (`PreviewFrame::diagram_pick`/
/// `diagram_tooth_pick`/`diagram_hover_text`/`diagram_facet_tier`) are only ever
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
            diagram_hover_text,
            diagram_facet_tier,
            planes,
            hover_text,
            facet_tier,
            generation,
            mesh_bounding_radius,
        } = frame;

        // Cloned Arcs (cheap), not `self` -- the closure below outlives this call.
        let pick_state = Arc::clone(&self.pick);
        let last_solved_state = Arc::clone(&self.last_solved);
        let diagram_pick_state = Arc::clone(&self.diagram_pick);
        let diagram_tooth_pick_state = Arc::clone(&self.diagram_tooth_pick);
        let diagram_hover_text_state = Arc::clone(&self.diagram_hover_text);
        let diagram_facet_tier_state = Arc::clone(&self.diagram_facet_tier);
        let hover_text_state = Arc::clone(&self.hover_text);
        let facet_tier_state = Arc::clone(&self.facet_tier);
        let render_ctx_state = Arc::clone(&self.render_ctx);
        let solid_active_planes_state = Arc::clone(&self.solid_active_planes);
        let trace_active_planes_state = Arc::clone(&self.trace_active_planes);
        let mesh_bounding_radius_state = Arc::clone(&self.mesh_bounding_radius);

        // `slint::Image::from_rgba8` runs HERE, not on the worker thread -- see
        // `gui::solid_preview::to_pixel_buffer`'s doc comment (`slint::Image` is not
        // `Send`).
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            *pick_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pick);
            // Kept alongside the image/pick swap in this same atomicity
            // boundary -- see this function's own doc comment -- so the orbit
            // camera's distance clamp (`render::camera_lighting`) never reads a
            // radius that describes a different frame than the one on screen.
            *mesh_bounding_radius_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = mesh_bounding_radius;
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
            if let Some(masts) = solved.as_ref() {
                editor::apply_matching_preview_frame(&ui, &render_ctx_state, generation, masts);
            }
            *last_solved_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = solved;
            // Diagram mode's side tables are only ever `Some` for a view_mode-3
            // request -- see `PreviewFrame::diagram_hover_text`'s doc comment.
            // `diagram_tooth_pick` shares that same "only `Some` together"
            // contract.
            store_if_some(&diagram_pick_state, diagram_pick);
            store_if_some(&diagram_tooth_pick_state, diagram_tooth_pick);
            store_if_some(&diagram_hover_text_state, diagram_hover_text);
            store_if_some(&diagram_facet_tier_state, diagram_facet_tier);
            // Unconditional (unlike the Diagram-only tables above) -- see
            // `SlintSolidSink::hover_text`'s doc comment.
            *hover_text_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = hover_text;
            *facet_tier_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = facet_tier;

            // See `sync_planes_and_check_trace_match`'s own doc comment. Also
            // passes this SAME frame's `generation` (already destructured above,
            // for `apply_matching_preview_frame`) so a stale frame's planes can be
            // told apart from the live design's.
            let trace_matches = sync_planes_and_check_trace_match(
                &planes,
                &render_ctx_state,
                &solid_active_planes_state,
                &trace_active_planes_state,
                generation,
            );
            ui.global::<SolidPreviewModel>()
                .set_trace_matches_solid(trace_matches);

            ui.global::<SolidPreviewModel>()
                .set_image(slint::Image::from_rgba8(image));
            ui.global::<SolidPreviewModel>().set_has_solid(has_solid);
            ui.global::<SolidPreviewModel>().set_status(status.into());
            ui.global::<SolidPreviewModel>().set_stale(stale);
            if let Some(edges_image) = edges_image {
                ui.global::<SolidPreviewModel>()
                    .set_edges_image(slint::Image::from_rgba8(edges_image));
                ui.global::<SolidPreviewModel>().set_has_solid_edges(true);
            } else {
                ui.global::<SolidPreviewModel>().set_has_solid_edges(false);
            }
            if let Some(diagram_image) = diagram_image {
                ui.global::<SolidPreviewModel>()
                    .set_diagram_image(slint::Image::from_rgba8(diagram_image));
            }
            ui.global::<SolidPreviewModel>()
                .set_has_diagram(has_diagram);
        });
    }
}
