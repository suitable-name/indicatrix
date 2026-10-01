//! The request/job/result payload types that travel between the UI thread and
//! the PLAN/RENDER worker threads: `RedrawRequest` (the RENDER worker's own
//! queue), [`ReplanRequest`]/`PlanJob` (what the PLAN worker needs to run
//! `live_update::plan_preview`), and `PlannedFrame` (the PLAN worker's handoff
//! to the RENDER worker).
//!
//! `RedrawRequest`, `PlanJob` and `PlannedFrame` moved to
//! `indicatrix_solid::preview` (shared with the web app) and are re-exported here
//! at their old paths; [`ReplanRequest`], this controller's own thread-facing
//! entry point, stays.

use super::types::CameraPose;
use indicatrix::geometry::meet_solver::SolvedTier;
use std::{collections::BTreeSet, sync::Arc};

pub use indicatrix_solid::preview::{PlanJob, PlannedFrame, RedrawRequest};

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
    /// see `indicatrix_solid::preview::panel_kind_from_index` for the conversion
    /// applied on the WORKER-thread side.
    pub enlarged_panel: i32,
}
