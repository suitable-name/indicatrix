//! What the views keep between frames: the pipeline, the solve cache it chains
//! from, the last frame's pick buffers and tables, and the signature of the app
//! state the frame reflects.

use super::manip::ManipState;
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;
use indicatrix_solid::{
    diagram2d::DiagramFrame,
    preview::{CameraPose, FacetOverlay, FrameGeometry, PickBuffer, PreviewPipeline},
};
use std::{collections::BTreeSet, sync::Arc};

/// The desktop's view mode for a header tab: Solid (tab 1) is `0`, Diagram
/// (tab 2) is `3`; the Render tab has none.
#[must_use]
pub const fn view_mode_for_tab(tab: i32) -> Option<u8> {
    match tab {
        1 => Some(0),
        2 => Some(3),
        _ => None,
    }
}

/// The part of the app state a frame depends on -- compared by [`super::refresh`]
/// to decide between a replan, a reproject, an overlay update, or nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    /// The design generation.
    pub generation: u64,
    /// `(solved, failed)` generations of the app's `SolveState`, if any.
    pub solve: (Option<u64>, Option<u64>),
    /// The selected tier.
    pub selected_tier: Option<usize>,
    /// The Ctrl+click multi-selection.
    pub multi_selected: BTreeSet<usize>,
    /// How many custom materials are registered (the refractive index may change).
    pub custom_materials: usize,
    /// The Preform toggle.
    pub show_preform: bool,
    /// The Cut slider (`-1` for all).
    pub tier_cutoff: i32,
    /// The enlarged diagram panel (`-1` for three).
    pub enlarged_panel: i32,
    /// The view mode (0 Solid, 3 Diagram).
    pub view_mode: u8,
    /// The orbit camera.
    pub camera: CameraPose,
    /// The raster size.
    pub size: (u32, u32),
}

impl Seen {
    /// Whether going from `self` to `now` needs a replan (the planned style,
    /// masts or planes may change) rather than just a reproject or overlay.
    #[must_use]
    pub fn needs_replan(&self, now: &Self) -> bool {
        self.generation != now.generation
            || self.solve != now.solve
            || self.selected_tier != now.selected_tier
            || self.custom_materials != now.custom_materials
            || self.show_preform != now.show_preform
            || self.tier_cutoff != now.tier_cutoff
            || self.enlarged_panel != now.enlarged_panel
    }

    /// Whether only the pose, size or view mode moved (a reproject).
    #[must_use]
    pub fn needs_reproject(&self, now: &Self) -> bool {
        self.view_mode != now.view_mode || self.camera != now.camera || self.size != now.size
    }
}

/// The views' state (one per page, in `super::VIEWS`).
#[derive(Default)]
pub struct ViewsState {
    /// The planner/rasterizer and its memory (the desktop RENDER worker's state).
    pub pipeline: PreviewPipeline,
    /// The mounted view's logical size (both tabs fill the same main area).
    pub view_size: (f32, f32),
    /// The design and masts the last planned frame came from (the desktop's
    /// `solid_last_solved`, keyed by the design itself so the next edit's dirty
    /// tiers can be diffed).
    pub cache: Option<(Arc<Design>, Vec<SolvedTier>)>,
    /// Bumped whenever [`Self::cache`] is replaced, so anything derived from it (the
    /// direct-manipulation tools' facet map) knows when to rebuild.
    pub cache_rev: u64,
    /// The design generation [`Self::cache`] describes.
    pub cache_generation: u64,
    /// The geometry of the mesh the last frame was drawn from, with the pose and size
    /// the raster used: what the drag handles are placed and hit-tested with.
    pub geometry: Option<FrameGeometry>,
    /// The generation the last frame reflects (`PROVISIONAL_GENERATION` for a Slice
    /// tool frame).
    pub frame_generation: u64,
    /// The direct-manipulation tools' state (`super::manip`).
    pub manip: ManipState,
    /// What the last frame reflects; `None` before the first one.
    pub seen: Option<Seen>,
    /// A replan is owed regardless of [`Seen`] (a `Stale` frame's follow-up).
    pub force_replan: bool,
    /// The generation a `Stale` follow-up was already scheduled for (once each).
    pub followup_for: Option<u64>,
    /// The generation a solve was already requested for (once each).
    pub solve_requested_for: Option<u64>,
    /// The hover/click/multi-select highlight, merged (the desktop's
    /// `FACET_OVERLAY`).
    pub overlay: FacetOverlay,
    /// [`Self::overlay`] changed since the last frame (hover, click).
    pub overlay_dirty: bool,
    /// The last frame's solid pick buffer.
    pub pick: Option<PickBuffer>,
    /// The last Diagram frame (its pick, tooth and panel buffers).
    pub diagram: Option<DiagramFrame>,
    /// Facet id -> hover text, from the last frame.
    pub hover_text: Vec<String>,
    /// Facet id -> owning tier, from the last frame.
    pub facet_tier: Vec<Option<usize>>,
    /// The last clicked facet's label, shown once the pointer leaves it (Solid).
    pub selected_label: String,
    /// The same for the Diagram (a per-view concept on the desktop too).
    pub diagram_selected_label: String,
    /// The shown solid's bounding radius (zoom clamp, "Fit"); `0.0` before the
    /// first frame -- read it through [`Self::radius`].
    pub bounding_radius: f64,
}

impl ViewsState {
    /// The shown solid's bounding radius, or the desktop's default before any
    /// frame (`DEFAULT_MESH_BOUNDING_RADIUS`).
    #[must_use]
    pub fn radius(&self) -> f64 {
        if self.bounding_radius > 0.0 {
            self.bounding_radius
        } else {
            indicatrix_solid::preview::DEFAULT_MESH_BOUNDING_RADIUS
        }
    }
}
