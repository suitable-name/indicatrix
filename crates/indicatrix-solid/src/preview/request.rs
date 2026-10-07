//! The request/job/result payloads the frame pipeline runs on: [`RedrawRequest`]
//! (one redraw), [`PlanJob`] (what [`super::build_planned_frame`] needs to run
//! [`crate::live_update::plan_preview`]), and [`PlannedFrame`] (its result, ready
//! to rasterize).
//!
//! Moved verbatim from the desktop's `gui::solid_preview::preview_state::request`,
//! which re-exports them next to its own thread-facing `ReplanRequest`.

use super::types::{CameraPose, FacetOverlay, StoneGeometryBuf};
use crate::{diagram2d::PanelKind, raster::SolidStyle};
use glam::Vec3;
use indicatrix::geometry::{ToolPrimitive, meet_solver::SolvedTier};
use std::{collections::BTreeSet, sync::Arc};

/// One redraw request for [`super::render_request`].
pub enum RedrawRequest {
    /// Cheap camera-follow: re-render the SAME geometry/mesh at a new pose (an
    /// orbit/zoom drag, no `Design` to plan against). Reuses whatever
    /// [`SolidStyle`] the pipeline last computed for a [`Self::Planned`] request, so
    /// a camera drag while viewing an overlay does not lose it.
    Reproject {
        /// The stone to draw: its planes plus any concave tools (see
        /// [`StoneGeometryBuf`]); [`StoneGeometryBuf::from_halfspaces`] wraps a bare
        /// `(normal, offset)` plane arrangement.
        geometry: StoneGeometryBuf,
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
    /// The desktop's Cut slider: `Some(k)` draws the stone after the first `k` cutting
    /// steps (`Design::preview_steps`), `0` being the rough. Wins over
    /// [`Self::tier_cutoff`] when set; `None` leaves `tier_cutoff` in charge, which is
    /// all the web app uses.
    pub cut_steps: Option<usize>,
}

impl PlanJob {
    /// The [`crate::live_update::CutLimit`] this job asks for: `cut_steps` first, then
    /// `tier_cutoff`, else the finished stone.
    #[must_use]
    pub fn cut_limit(&self) -> crate::live_update::CutLimit {
        self.cut_steps.map_or_else(
            || crate::live_update::CutLimit::from_tier_cutoff(self.tier_cutoff),
            crate::live_update::CutLimit::Steps,
        )
    }
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
    /// The concave tools subtracted from `planes`; empty for a design with no
    /// concave tiers. Tool `k` is facet id `planes.len() + k`.
    pub tools: Vec<ToolPrimitive>,
    /// `(concave tier, placement)` of each tool, parallel to `tools`.
    pub placements: Vec<(usize, usize)>,
    /// The facet-level style (flagged/pending/selected, preform handling).
    pub style: SolidStyle,
    /// The masts this frame solved, to chain forward as the next call's `last_solved`.
    /// `None` when nothing solved -- an `Unsolvable` frame in particular carries none: the
    /// previous masts do not describe this frame's design, and a caller that files a
    /// frame's masts under its generation must not be handed them. A caller keeps its
    /// own cache as it is when this is `None`.
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
    /// Which flat tiers (indexed like `design.tiers`) `planes` contains: `None` for the
    /// finished stone, `Some` for a partly cut one (the Cut slider). The facet-id
    /// tables of the frame ([`crate::facet_map::FacetMap::from_design_cut`]) number the
    /// facets the way these planes are numbered.
    pub visible_tiers: Option<Vec<bool>>,
}
