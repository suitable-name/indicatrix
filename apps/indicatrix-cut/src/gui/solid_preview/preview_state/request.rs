//! The request/job/result payload types that travel between the UI thread and
//! the PLAN/RENDER worker threads: [`RedrawRequest`] (the RENDER worker's own
//! queue), [`ReplanRequest`]/[`PlanJob`] (what the PLAN worker needs to run
//! `live_update::plan_preview`), and [`PlannedFrame`] (the PLAN worker's handoff
//! to the RENDER worker).

use super::{PanelKind, types::CameraPose};
use glam::Vec3;
use indicatrix::geometry::meet_solver::SolvedTier;
use std::{collections::BTreeSet, sync::Arc};

/// One coalesced redraw request, carried through the `RedrawGate` from a
/// `request_*` call to the worker thread that renders it.
pub enum RedrawRequest {
    /// Cheap camera-follow: re-render the SAME planes/mesh at a new pose (an
    /// orbit/zoom drag, no `Design` to plan against). Reuses whatever
    /// `raster::SolidStyle` the worker last computed for a [`Self::Planned`] request, so a
    /// camera drag while viewing an overlay does not lose it.
    Reproject {
        planes: Vec<(Vec3, f32)>,
        camera: CameraPose,
        size: (u32, u32),
        view_mode: u8,
        /// The currently loaded design's gear tooth count/reference angle, for a
        /// Diagram-mode (`view_mode` 3) reproject -- see `super::state::
        /// DiagramMemory`'s doc comment for why a `Reproject` request otherwise
        /// has no `Design` to read this from. `Some` overwrites the worker's
        /// `DiagramMemory` gear fields before rendering; `None` leaves them
        /// exactly as they were (the only option
        /// [`super::SolidPreviewState::request_redraw`]'s caller-compatible
        /// wrapper can offer -- see its own doc comment).
        gear: Option<(u32, f32)>,
    },
    /// A facet-id-keyed highlight update (see `super::types::FacetOverlay`'s doc
    /// comment) with no new camera/planes/size of its own -- the worker
    /// re-renders at whatever it last used for a [`Self::Reproject`]/
    /// [`Self::Planned`] request (remembered in `super::state::WorkerMemory`),
    /// exactly the same "cheap camera-follow" shape [`Self::Reproject`] already
    /// has, just following a facet id instead of a camera pose. Never carries a
    /// `Design`: every id here already came from
    /// `SolidRasterizer::pick_at`/`DiagramFrame::pick_at`, or (`multi_selected`)
    /// from whatever tier->facet lookup the caller already had on hand.
    UpdateFacetOverlay(super::types::FacetOverlay),
    /// A finished replan from the PLAN worker, ready for the RENDER worker to
    /// finish and rasterize. Boxed for `clippy::large_enum_variant` since
    /// [`PlannedFrame`] carries a whole `Design` plus its solved masts.
    Planned(Box<PlannedFrame>),
}

/// [`super::SolidPreviewState::request_replan`]'s payload: bundles everything
/// `live_update::plan_preview` needs.
///
/// `design` is an `Arc` clone: cheap (refcount bump, not a deep clone) and lets
/// the worker thread own its handle without holding the UI thread's
/// `Rc<RefCell<EditorState>>` borrow open across the async round trip.
pub struct ReplanRequest {
    pub design: Arc<indicatrix_cut_core::Design>,
    /// The tier index/indices the triggering edit touched. An edit that cannot be
    /// described as "these tiers changed" (`Undo`/`Redo`, a gear remap, a
    /// symmetry/mirror change) should pass `last_solved: None` instead.
    pub dirty: BTreeSet<usize>,
    /// The previous call's resolved masts, or `None` to force a full
    /// `Design::solve()` rather than a subgraph `resolve_dirty` -- always safe (just
    /// slower), so prefer `None` over an under-approximated `dirty`.
    pub last_solved: Option<Vec<SolvedTier>>,
    pub camera: CameraPose,
    /// The Solid viewport's own LOGICAL size -- see
    /// [`super::SolidPreviewState::request_redraw`]'s doc comment.
    pub size: (u32, u32),
    /// The tier currently selected in the tier list (`facet_map::OverlayFlags::
    /// selected`), `None` when nothing is selected.
    pub selected_tier: Option<usize>,
    /// The design's effective refractive index the critical-angle overlay is
    /// computed against.
    pub n_d: f64,
    /// 0 = Solid, 1 = Path-traced, 2 = Both -- only `2` makes the worker also
    /// render the transparent-fill edges layer (see `super::sink::PreviewFrame::
    /// edges_image`).
    pub view_mode: u8,
    /// `gui::editor::state::EditorState::generation`'s value at the moment this
    /// request was submitted (`gui::editor::view::submit_preview_replan`) --
    /// Echoed onto the finished `super::sink::PreviewFrame` unchanged so the sink
    /// can tell a still-current result from one a later edit has superseded.
    pub generation: u64,
    /// `SolidPreviewModel.show_preform_planes`'s value at request time: the
    /// preform-visibility toggle, whether the rough-bounding preform facets
    /// (`facet_map::FacetMap::preform_plane_count`) should render at all
    /// rather than being hidden so only the schedule's own cut facets show.
    pub show_preform: bool,
    /// `SolidPreviewModel.diagram_enlarged_panel`'s raw value at request time
    /// (the "enlarge this panel" mode): `0`/`1`/`2` for Crown/Pavilion/
    /// Profile, anything else (in particular the property's own `-1` default)
    /// for "no panel enlarged." A plain `i32` (rather than `Option<diagram2d::
    /// PanelKind>`) purely so this module's caller does not need a
    /// `diagram2d::PanelKind` import of its own just to build this request --
    /// see [`panel_kind_from_index`] for the conversion this module itself
    /// applies on the WORKER-thread side.
    pub enlarged_panel: i32,
}

/// [`ReplanRequest::enlarged_panel`]'s raw index -> `PanelKind` conversion,
/// matching `diagram2d::PanelKind`'s declaration order (`0` = Crown, `1` =
/// Pavilion, `2` = Profile) -- anything else (including the property's own
/// `-1` "nothing enlarged" default) is `None`.
pub const fn panel_kind_from_index(index: i32) -> Option<PanelKind> {
    match index {
        0 => Some(PanelKind::Crown),
        1 => Some(PanelKind::Pavilion),
        2 => Some(PanelKind::Profile),
        _ => None,
    }
}

/// [`super::SolidPreviewState::request_replan`]'s payload for the PLAN worker. Same
/// fields as [`ReplanRequest`] plus `tier_cutoff`, read fresh from cache.
pub struct PlanJob {
    pub design: Arc<indicatrix_cut_core::Design>,
    pub dirty: BTreeSet<usize>,
    pub last_solved: Option<Vec<SolvedTier>>,
    pub camera: CameraPose,
    pub size: (u32, u32),
    pub selected_tier: Option<usize>,
    pub n_d: f64,
    pub view_mode: u8,
    pub generation: u64,
    pub show_preform: bool,
    pub enlarged_panel: i32,
    pub tier_cutoff: Option<usize>,
}

/// `super::plan_worker::build_planned_frame`'s result: everything the RENDER
/// worker needs to finish and rasterize without calling `live_update::
/// plan_preview` itself. `style` is undimmed; `super::state::resolve_planned_state`
/// decides if it's held-over.
pub struct PlannedFrame {
    pub design: Arc<indicatrix_cut_core::Design>,
    pub planes: Vec<(Vec3, f32)>,
    pub style: super::SolidStyle,
    /// The mast list to chain forward as the next call's `last_solved` -- already
    /// resolved to "the fresh result" or "the old masts chained forward
    /// unchanged" (an `Unsolvable` frame must not wipe this cache with `None`,
    /// same reasoning `super::state::resolve_planned_state`'s predecessor
    /// `plan_and_style` documented).
    pub solved: Option<Vec<SolvedTier>>,
    pub stale: bool,
    /// `live_update::Freshness::Unsolvable`'s "Preview cannot be solved: ..."
    /// text -- the one status this thread can already decide with no `MeshCache`
    /// of its own. `None` here does NOT mean "this frame is fine": it may still
    /// turn out `Unbounded` once `super::state::resolve_planned_state` checks the mesh.
    pub unsolvable_status: Option<String>,
    pub camera: CameraPose,
    pub size: (u32, u32),
    pub view_mode: u8,
    pub generation: u64,
    pub n_d: f64,
    pub enlarged_panel: i32,
}
