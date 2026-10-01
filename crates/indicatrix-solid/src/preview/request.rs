//! The request/job/result payloads the frame pipeline runs on: [`RedrawRequest`]
//! (one redraw), [`PlanJob`] (what [`super::build_planned_frame`] needs to run
//! [`crate::live_update::plan_preview`]), and [`PlannedFrame`] (its result, ready
//! to rasterize).
//!
//! Moved verbatim from the desktop's `gui::solid_preview::preview_state::request`,
//! which re-exports them next to its own thread-facing `ReplanRequest`.

use super::types::{CameraPose, FacetOverlay};
use crate::{diagram2d::PanelKind, raster::SolidStyle};
use glam::Vec3;
use indicatrix::geometry::meet_solver::SolvedTier;
use std::{collections::BTreeSet, sync::Arc};

/// One redraw request for [`super::render_request`].
pub enum RedrawRequest {
    /// Cheap camera-follow: re-render the SAME planes/mesh at a new pose (an
    /// orbit/zoom drag, no `Design` to plan against). Reuses whatever
    /// [`SolidStyle`] the pipeline last computed for a [`Self::Planned`] request, so
    /// a camera drag while viewing an overlay does not lose it.
    Reproject {
        /// The plane arrangement, `(normal, offset)` with `n . x <= m`.
        planes: Vec<(Vec3, f32)>,
        /// The camera pose to render at.
        camera: CameraPose,
        /// Output size in physical pixels.
        size: (u32, u32),
        /// 0 Solid, 1 Path-traced, 2 Both, 3 Diagram.
        view_mode: u8,
        /// The currently loaded design's gear tooth count/reference angle, for a
        /// Diagram-mode (`view_mode` 3) reproject. `Some` overwrites the
        /// pipeline's [`super::DiagramMemory`] gear fields before rendering; `None`
        /// leaves them exactly as they were.
        gear: Option<(u32, f32)>,
    },
    /// A facet-id-keyed highlight update (see [`FacetOverlay`]) with no new
    /// camera/planes/size of its own -- re-rendered at whatever the pipeline last
    /// used for a [`Self::Reproject`]/[`Self::Planned`] request (remembered in
    /// [`super::WorkerMemory`]).
    UpdateFacetOverlay(FacetOverlay),
    /// A finished replan ([`super::build_planned_frame`]), ready to rasterize.
    /// Boxed for `clippy::large_enum_variant` since [`PlannedFrame`] carries a
    /// whole `Design` plus its solved masts.
    Planned(Box<PlannedFrame>),
}

/// Which diagram panel a raw `enlarged_panel` index names.
///
/// `0` Crown, `1`
/// Pavilion, `2` Profile ([`PanelKind`]'s declaration order); anything else
/// (in particular the UI's own `-1` default) is `None`, the three-panel layout.
#[must_use]
pub const fn panel_kind_from_index(index: i32) -> Option<PanelKind> {
    match index {
        0 => Some(PanelKind::Crown),
        1 => Some(PanelKind::Pavilion),
        2 => Some(PanelKind::Profile),
        _ => None,
    }
}

/// Everything [`super::build_planned_frame`] needs to run
/// [`crate::live_update::plan_preview`] and style its result.
pub struct PlanJob {
    /// The design to plan (shared, never deep-cloned along the pipeline).
    pub design: Arc<indicatrix_cut_core::Design>,
    /// The tier indices the triggering edit touched.
    pub dirty: BTreeSet<usize>,
    /// The previous solve's masts, or `None` to force a full `Design::solve()`.
    pub last_solved: Option<Vec<SolvedTier>>,
    /// The camera pose to render at.
    pub camera: CameraPose,
    /// Output size in physical pixels.
    pub size: (u32, u32),
    /// The tier selected in the tier list, tinted in the view.
    pub selected_tier: Option<usize>,
    /// The design's effective refractive index (critical-angle overlay).
    pub n_d: f64,
    /// 0 Solid, 1 Path-traced, 2 Both, 3 Diagram.
    pub view_mode: u8,
    /// The design generation this job describes, echoed onto the frame.
    pub generation: u64,
    /// Whether the rough's own bounding (preform) planes are drawn.
    pub show_preform: bool,
    /// The raw enlarged-panel index -- see [`panel_kind_from_index`].
    pub enlarged_panel: i32,
    /// "Show through tier N": `Some(n)` truncates to `design.tiers[..=n]`.
    pub tier_cutoff: Option<usize>,
}

/// [`super::build_planned_frame`]'s result: everything
/// [`super::render_request`] needs to finish and rasterize without calling
/// `plan_preview` itself.
///
/// `style` is undimmed; the render step decides whether
/// the frame is held-over.
pub struct PlannedFrame {
    /// The planned design (the same `Arc` the job carried).
    pub design: Arc<indicatrix_cut_core::Design>,
    /// The plane arrangement to draw this frame.
    pub planes: Vec<(Vec3, f32)>,
    /// The facet-level style (flagged/pending/selected, preform handling).
    pub style: SolidStyle,
    /// The mast list to chain forward as the next call's `last_solved` -- already
    /// resolved to "the fresh result" or "the old masts chained forward
    /// unchanged" (an `Unsolvable` frame must not wipe this cache with `None`).
    pub solved: Option<Vec<SolvedTier>>,
    /// `live_update::Freshness::Stale`: the planes are the previous solve's.
    pub stale: bool,
    /// `live_update::Freshness::Unsolvable`'s "Preview cannot be solved: ..."
    /// text. `None` does NOT mean "this frame is fine": it may still turn out
    /// `Unbounded` once the render step checks the mesh.
    pub unsolvable_status: Option<String>,
    /// The camera pose to render at.
    pub camera: CameraPose,
    /// Output size in physical pixels.
    pub size: (u32, u32),
    /// 0 Solid, 1 Path-traced, 2 Both, 3 Diagram.
    pub view_mode: u8,
    /// The job's generation.
    pub generation: u64,
    /// The job's refractive index.
    pub n_d: f64,
    /// The job's raw enlarged-panel index.
    pub enlarged_panel: i32,
}
