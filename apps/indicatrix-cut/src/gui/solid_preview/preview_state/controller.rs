//! The editor-side controller struct itself ([`SolidPreviewState`]) and its
//! UI-thread-facing entry points: construction, the cheap camera-follow
//! reproject/facet-overlay submissions, the replan submission, and the tier-
//! cutoff cache. See the parent module's doc comment for the two-worker split
//! these methods feed into.

use super::{
    RedrawGate,
    request::{PlanJob, RedrawRequest, ReplanRequest},
    sink::PreviewSink,
    types::{CameraPose, FacetOverlay},
};
use glam::Vec3;
use indicatrix_solid::preview::{Outlines, SharedOutlines};
use std::sync::{
    Arc, Mutex, PoisonError, Weak,
    atomic::{AtomicU64, Ordering},
    mpsc::Sender,
};

/// The editor-side controller described in the parent module's doc comment.
pub struct SolidPreviewState {
    pub(super) sink: Arc<dyn PreviewSink>,
    /// Consumed only by the RENDER worker (`super::render`'s `spawn_worker`) --
    /// the two-worker split, see the module doc comment.
    pub(super) gate: Arc<RedrawGate<RedrawRequest>>,
    /// Wake channel for the RENDER worker, created lazily on the first
    /// `request_*`/finished-plan call. `None` until then, so a `SolidPreviewState`
    /// never asked to redraw never spawns a thread.
    pub(super) wake: Mutex<Option<Sender<()>>>,
    /// Consumed only by the PLAN worker (`super::plan_worker`'s `spawn_plan_worker`)
    /// -- a Separate queue from `gate` so a slow solve never blocks `Reproject` requests.
    pub(super) plan_gate: Arc<RedrawGate<PlanJob>>,
    /// [`Self::wake`]'s counterpart for the PLAN worker, created lazily the same
    /// way on the first [`Self::request_replan`] call.
    pub(super) plan_wake: Mutex<Option<Sender<()>>>,
    /// A weak handle to this same value, filled in immediately after
    /// construction -- lets the PLAN worker (which holds no other reference back
    /// to `Self`) hand a finished `super::request::PlannedFrame` to the RENDER
    /// worker through `super::render`'s `submit`, the exact same lazy-spawn-and-wake
    /// path a `Reproject` call already uses, rather than duplicating that contract
    /// a second time. `Weak`, not `Arc`: the PLAN worker thread must never be the
    /// reason a `SolidPreviewState` outlives every caller's own handle to it.
    pub(super) self_weak: Mutex<Weak<Self>>,
    /// "Show through tier N" slider: `Some(n)` truncates the plane arrangement
    /// to `design.tiers[..=n]`. Cached here for UI access via [`Self::set_tier_cutoff`].
    pub(super) tier_cutoff: Mutex<Option<usize>>,
    /// bumped by `gui::editor::auto_solve::scheduling::
    /// reset_for_new_design` on every wholesale design replacement (New/Load
    /// Selected/Open Native) via [`Self::bump_generation_floor`] -- lets the
    /// PLAN worker (`super::plan_worker::spawn_plan_worker`) tell a `PlanJob`
    /// queued or in flight for the design being REPLACED apart from one for
    /// the design that replaced it, even though `self.plan_gate`
    /// (`super::RedrawGate`) itself only ever coalesces to "the latest queued
    /// job," never "the latest queued job for the CURRENT design." A `PlanJob`
    /// whose own `generation` is below this floor is dropped rather than
    /// solved and handed back as a frame.
    pub(super) generation_floor: Arc<AtomicU64>,
    /// The Slice tool's provisional-tier outline and the drag-follower outline, shared
    /// with the RENDER worker's `WorkerMemory::outlines`. Written synchronously by
    /// [`Self::set_outlines`] (from the overlay's `provisional` / `moved` fields) and
    /// read by the worker when it DRAWS, so a `Planned` request that
    /// rebuilds the style, or a later request that supersedes an overlay update in
    /// the "latest wins" gate, can neither drop nor resurrect them.
    pub(super) outlines: SharedOutlines,
    /// When `Some`, replaces the `planes` of every [`Self::request_redraw_with_gear`]
    /// call: the Slice tool sets it to the provisional design's planes so a camera
    /// orbit, a view-mode switch or a background-solve redraw (all of which reproject
    /// the COMMITTED `RenderContext::active_planes`) keeps showing the provisional
    /// facet instead of snapping back to the committed stone.
    pub(super) planes_override: Mutex<Option<Vec<(Vec3, f32)>>>,
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
            plan_gate: Arc::new(RedrawGate::new()),
            plan_wake: Mutex::new(None),
            self_weak: Mutex::new(Weak::new()),
            tier_cutoff: Mutex::new(None),
            generation_floor: Arc::new(AtomicU64::new(0)),
            outlines: Arc::new(Mutex::new(Outlines::default())),
            planes_override: Mutex::new(None),
        });
        *state
            .self_weak
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Arc::downgrade(&state);
        state
    }

    /// Submits one cheap camera-follow redraw (see `super::request::RedrawRequest::
    /// Reproject`) and returns immediately; the finished frame reaches
    /// `super::sink::PreviewSink::apply` asynchronously. `size` must be the
    /// viewport's LOGICAL size (points, not render resolution). `view_mode` (0
    /// Solid / 1 Path-traced / 2 Both / 3 Diagram) decides whether the worker also
    /// renders the "Both" mode's edges layer, or the Diagram mode's 2D facet
    /// diagram, this frame -- a Diagram-mode request rebuilds the diagram at the
    /// worker's last-known gear info/labels (see `super::state::DiagramMemory`)
    /// since a `Reproject` request carries no `Design`. A burst of calls coalesces
    /// to exactly the LAST one submitted -- see `RedrawGate`'s doc comment.
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
    /// `super::state::DiagramMemory` -- see `super::request::RedrawRequest::
    /// Reproject`'s `gear` field doc comment. `None` leaves the worker's
    /// last-known gear info untouched.
    pub fn request_redraw_with_gear(
        &self,
        planes: Vec<(Vec3, f32)>,
        camera: CameraPose,
        size: (u32, u32),
        view_mode: u8,
        gear: Option<(u32, f32)>,
    ) {
        let planes = self
            .planes_override
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or(planes);
        self.submit(RedrawRequest::Reproject {
            planes,
            camera,
            size,
            view_mode,
            gear,
        });
    }

    /// Submits a facet-id-keyed highlight update ("which facet did I click", multi-
    /// select, hover) -- see `super::types::FacetOverlay`'s doc comment. Re-renders
    /// at whatever camera/planes/size/`view_mode` the worker last used for a
    /// [`Self::request_redraw_with_gear`]/[`Self::request_replan`] call, exactly
    /// like an ordinary camera-follow reproject, so a caller that already has a
    /// facet id from a pick buffer never needs to also track/resend the camera
    /// pose or the design's planes just to show it. A no-op before the worker has
    /// rendered anything at all.
    pub fn request_facet_overlay(&self, overlay: FacetOverlay) {
        self.submit(RedrawRequest::UpdateFacetOverlay(overlay));
    }

    /// Stores the provisional-tier and drag-follower outlines where the RENDER worker
    /// reads them when it draws -- see [`Self::outlines`] -- so they survive a
    /// `Planned` request and an overlay update superseded in the "latest wins" gate
    /// alike. Does NOT redraw: follow with [`Self::request_facet_overlay`]. The
    /// editor's 3D overlay path calls this with the merged overlay's two fields on
    /// every update; the Diagram view's own hover/click overlays never do, so they
    /// cannot clear an outline they know nothing about.
    pub fn set_outlines(&self, provisional: &[u32], moved: &[u32]) {
        let mut outlines = self.outlines.lock().unwrap_or_else(PoisonError::into_inner);
        outlines.provisional.clear();
        outlines.provisional.extend_from_slice(provisional);
        outlines.moved.clear();
        outlines.moved.extend_from_slice(moved);
    }

    /// Makes every later [`Self::request_redraw_with_gear`] draw `planes` instead of
    /// the planes its caller passes (`None` restores the caller's). Used by the Slice
    /// tool while a provisional facet is on screen -- see [`Self::planes_override`].
    pub fn set_planes_override(&self, planes: Option<Vec<(Vec3, f32)>>) {
        *self
            .planes_override
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = planes;
    }

    /// Submits a replan-on-edit request to the PLAN worker.
    /// `live_update::plan_preview` never runs on the UI thread.
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

    /// Sets the "show through tier N" cutoff: `Some(n)` truncates to
    /// `design.tiers[..=n]`. Does NOT trigger a redraw; the caller must submit
    /// [`Self::request_replan`] afterwards.
    ///
    /// Called from `gui::editor::view::submit_preview_replan_for`, which reads
    /// `SolidPreviewModel.tier_cutoff` (`-1` means "no cutoff", matching this
    /// crate's existing `-1`-means-`None` convention for e.g.
    /// `diagram_enlarged_panel`) and converts it with
    /// `usize::try_from(cutoff).ok()` before calling this and then
    /// `request_replan` -- the same three-file shape `show_preform_planes`/
    /// `diagram_enlarged_panel` already went through, just with the read happening
    /// here instead of as a `ReplanRequest` field (see this state's own
    /// `tier_cutoff` field doc comment for why).
    pub fn set_tier_cutoff(&self, cutoff: Option<usize>) {
        *self
            .tier_cutoff
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = cutoff;
    }

    /// bumps [`Self::generation_floor`] to (at least) `floor` -- called
    /// once per wholesale design replacement by `gui::editor::auto_solve::
    /// scheduling::reset_for_new_design`. `fetch_max`, not a plain store: this
    /// must never move BACKWARDS even if called out of order (defensive only
    /// -- `EditorState::generation` is itself only ever bumped, never reset,
    /// so a caller's own `floor` values are already non-decreasing in
    /// practice).
    pub fn bump_generation_floor(&self, floor: u64) {
        self.generation_floor.fetch_max(floor, Ordering::Relaxed);
    }
}
