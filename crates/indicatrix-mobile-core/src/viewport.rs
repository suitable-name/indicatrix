//! The design viewport: the camera, the gestures that move it, the solid and diagram
//! rasters, picking, the manipulation handles and the Slice tool.
//!
//! Layout of this module:
//! - this file: [`ViewportState`] (what the viewport remembers), [`ViewMode`], the
//!   camera and view operations, gesture dispatch and how a finished frame lands;
//! - [`gesture`]: the touch and mouse recognizer (taps, orbit, pinch, long press);
//! - [`transform`]: pan and zoom of the picture, and the pointer to raster-pixel maps;
//! - [`frame`]: raster jobs and the stateful worker that plans and draws them;
//! - [`manip`]: dragging the angle, depth and index handles of the selected tier;
//! - [`slice`]: the Slice tool (a drawn line becomes a provisional tier);
//! - [`render_view`]: the Render mode's decisions (trace raster size, coalesced restarts,
//!   the status line, dragging the light, the export options);
//! - [`render_drag`]: what the Render mode shows while the pose or the light is dragged
//!   (the live Solid raster, the held trace, the single restart after the finger lifts).
//!
//! Nothing here knows Slint. The app crate forwards pointer events in, runs
//! [`frame::RasterWorker`] on a background queue and pushes the results out; every
//! decision is made here so that it can be tested natively.

pub mod frame;
pub mod gesture;
pub mod manip;
pub mod render_drag;
pub mod render_view;
pub mod slice;
pub mod transform;

#[cfg(test)]
mod tests;

use std::{cell::Cell, collections::BTreeSet, sync::Arc, time::Duration};

use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;
use indicatrix_editor::{
    EditorSession,
    manipulate::{
        ActiveDrag, HandleTarget, ScreenPoint, gesture::kind_to_int,
        provisional::PROVISIONAL_GENERATION, text,
    },
};
use indicatrix_solid::preview::{
    CameraPose, DEFAULT_MESH_BOUNDING_RADIUS,
    camera::{self, RESET_POSE},
    view::{ContainFit, toggle_enlarged_panel},
};

use frame::{DEFAULT_MAX_EDGE, FrameKind, RasterFrame, RasterJob};
use gesture::{Button, Context, EDGE_GUARD_PX, Gesture, GestureTracker};
use slice::SliceTool;
use transform::{MAX_ZOOM, ViewTransform, pixel_at};

/// How large the handles are drawn and grabbed on a compact window with large handles on.
pub const LARGE_HANDLE_SCALE_COMPACT: f32 = 1.6;
/// The same on a larger window.
pub const LARGE_HANDLE_SCALE: f32 = 1.3;
/// The radius, in logical px, within which a press grabs a handle (at scale 1).
pub const HANDLE_GRAB_RADIUS_DP: f32 = 24.0;
/// The diameter, in logical px, a handle is drawn at (at scale 1).
pub const HANDLE_DRAW_DIAMETER_DP: f32 = 32.0;

/// Which picture the viewport shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// The shaded solid.
    #[default]
    Solid,
    /// The three-panel 2D faceting diagram.
    Diagram,
    /// The progressive ray-traced picture (`crate::render`). The viewport's own raster
    /// worker is idle in this mode; the camera is the Solid view's.
    Render,
}

impl ViewMode {
    /// The pipeline's view-mode code (`0` Solid, `3` Diagram). The Render mode has no
    /// raster of its own and reads as Solid.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Solid | Self::Render => 0,
            Self::Diagram => 3,
        }
    }

    /// The Slint index (`0` Solid, `1` Diagram, `2` Render).
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Solid => 0,
            Self::Diagram => 1,
            Self::Render => 2,
        }
    }

    /// The mode a Slint index names; anything but `1` and `2` is Solid.
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Diagram,
            2 => Self::Render,
            _ => Self::Solid,
        }
    }

    /// Whether this is the Render mode.
    #[must_use]
    pub const fn is_render(self) -> bool {
        matches!(self, Self::Render)
    }

    /// The mode the raster worker draws for this one (Render shares the Solid raster).
    #[must_use]
    pub const fn raster_mode(self) -> Self {
        match self {
            Self::Render => Self::Solid,
            other => other,
        }
    }
}

/// What a viewport operation asks of the app.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a request record; redraw, edited, live_edit and refresh are independent things the app may be asked to do"
)]
pub struct Outcome {
    /// The picture or its overlays changed: push the viewport.
    pub redraw: bool,
    /// A tap chose this tier (`Some(None)` clears the selection); the shared selection
    /// and the multi-selection were already updated.
    pub select: Option<Option<usize>>,
    /// The design changed and the gesture is over: run the after-edit path.
    pub edited: bool,
    /// The design changed in the middle of a drag: redraw live, run the after-edit path
    /// when the drag ends.
    pub live_edit: bool,
    /// Other design surfaces must be refreshed (the selection or a tool changed).
    pub refresh: bool,
    /// A message for the snackbar.
    pub toast: Option<String>,
}

impl Outcome {
    /// An outcome that only asks for a redraw.
    #[must_use]
    pub fn redraw() -> Self {
        Self {
            redraw: true,
            ..Self::default()
        }
    }

    /// Merges `other` into `self`.
    pub fn merge(&mut self, other: Self) {
        self.redraw |= other.redraw;
        self.edited |= other.edited;
        self.live_edit |= other.live_edit;
        self.refresh |= other.refresh;
        if other.select.is_some() {
            self.select = other.select;
        }
        if other.toast.is_some() {
            self.toast = other.toast;
        }
    }
}

/// The shared state a gesture may change besides the viewport itself.
pub struct Env<'a> {
    /// The design under edit.
    pub session: &'a mut EditorSession,
    /// The tier selected in the tier list and the viewport.
    pub selected_tier: &'a mut Option<usize>,
    /// The time since the app started (stamps coalescing edits and gesture timing).
    pub now: Duration,
}

impl Env<'_> {
    /// Selects `tier` the way a plain tap does: sets the shared selection and empties
    /// the multi-selection.
    pub fn select(&mut self, tier: Option<usize>) {
        let count = self.session.design.tiers.len();
        *self.selected_tier = tier.filter(|&index| index < count);
        self.session.multi_selected.clear();
    }
}

/// A raw pointer event from the UI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pointer {
    /// A finger or mouse button went down.
    Down {
        /// The touch id (`0` for a mouse).
        finger: i32,
        /// The button.
        button: Button,
        /// Horizontal position in the view.
        x: f32,
        /// Vertical position in the view.
        y: f32,
    },
    /// The pointer moved.
    Move {
        /// Horizontal position in the view.
        x: f32,
        /// Vertical position in the view.
        y: f32,
    },
    /// The pointer went up.
    Up {
        /// The touch id (`0` for a mouse).
        finger: i32,
        /// Horizontal position in the view.
        x: f32,
        /// Vertical position in the view.
        y: f32,
    },
    /// The pointer stream was cancelled.
    Cancel,
}

/// A two-finger gesture event from the UI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pinch {
    /// Two fingers started a pinch with their centre here.
    Begin {
        /// Horizontal position of the centre.
        x: f32,
        /// Vertical position of the centre.
        y: f32,
    },
    /// The cumulative scale and the centre changed.
    Update {
        /// The cumulative scale since the pinch began (1 at the start).
        scale: f32,
        /// Horizontal position of the centre.
        x: f32,
        /// Vertical position of the centre.
        y: f32,
    },
    /// The pinch ended or was cancelled.
    End,
}

/// Everything one picture depends on, compared to decide whether a new job is needed.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameKey {
    generation: u64,
    solved: bool,
    selected_tier: Option<usize>,
    multi_selected: BTreeSet<usize>,
    camera: CameraPose,
    size: (u32, u32),
    mode: ViewMode,
    enlarged_panel: i32,
    want_handles: bool,
    revision: u64,
}

/// The inputs of a job that the app owns.
#[derive(Debug, Clone)]
pub struct JobInputs {
    /// A snapshot of the committed design.
    pub design: Arc<Design>,
    /// The session's generation.
    pub generation: u64,
    /// The masts of a solve that is current for `generation`, if there is one.
    pub solved: Option<Vec<SolvedTier>>,
    /// The selected tier.
    pub selected_tier: Option<usize>,
    /// The multi-selected tiers.
    pub multi_selected: BTreeSet<usize>,
}

/// The settings the viewport reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    /// How large the handles are drawn and grabbed (`1.0`, or a large-handle factor).
    pub handle_scale: f32,
    /// Whether dragging snaps to the 0.1 degree and 0.01 mast steps.
    pub snap: bool,
    /// The longest raster edge in pixels.
    pub max_edge: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            handle_scale: 1.0,
            snap: true,
            max_edge: DEFAULT_MAX_EDGE,
        }
    }
}

/// The handle scale for the "large handles" setting on a compact or larger window.
#[must_use]
pub const fn handle_scale_for(large_handles: bool, compact: bool) -> f32 {
    match (large_handles, compact) {
        (true, true) => LARGE_HANDLE_SCALE_COMPACT,
        (true, false) => LARGE_HANDLE_SCALE,
        (false, _) => 1.0,
    }
}

/// Where the handles are drawn, in view coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HandleOverlay {
    /// Whether any handle is shown.
    pub visible: bool,
    /// The facet centroid the handles fan out from.
    pub anchor: (f32, f32),
    /// The angle handle's tip.
    pub angle: (f32, f32),
    /// The depth handle's tip.
    pub depth: (f32, f32),
    /// The index handle's tip.
    pub index: (f32, f32),
    /// Whether the index handle exists (a tier without index positions has none).
    pub index_visible: bool,
    /// The handle being dragged: `-1` none, `0` angle, `1` depth, `2` index.
    pub active: i32,
}

/// What the viewport remembers. See the module documentation.
#[derive(Debug)]
pub struct ViewportState {
    /// The orbit camera.
    pub camera: CameraPose,
    /// Solid or Diagram.
    pub mode: ViewMode,
    /// The Solid view's pan (its zoom is the camera distance, so `zoom` stays 1).
    pub solid_view: ViewTransform,
    /// The Diagram view's zoom and pan.
    pub diagram_view: ViewTransform,
    /// The enlarged diagram panel (`-1` for all three).
    pub enlarged_panel: i32,
    /// The viewport's size in logical px.
    pub view_size: (f32, f32),
    /// The display's scale factor (a `Cell`: the push path notes a scale the window
    /// reports late, see [`Self::note_scale_factor`]).
    pub scale_factor: Cell<f32>,
    /// The settings the viewport reads (see [`Self::set_options`]); a `Cell` because
    /// the push path only holds a shared reference to the state.
    pub options: Cell<Options>,
    /// Whether a finger is down on the picture (the Render mode coalesces restarts then).
    pub touch: render_view::TouchState,
    /// Whether a one-finger drag moves the light instead of the camera (Render mode).
    pub light_drag: render_view::LightDrag,
    /// What starts and stops the Render mode's trace (see [`render_view::RenderDriver`]).
    pub driver: render_view::RenderDriver,
    /// What the Render mode shows while the pose or the light is dragged.
    pub render_live: render_drag::LiveDrag,
    /// Whether the manipulation handles are shown.
    pub handles_on: bool,
    /// The newest picture that landed (its `rgba` is emptied: the UI holds the image).
    pub frame: Option<RasterFrame>,
    /// A line explaining why the picture is not the design's current state.
    pub status: String,
    /// The masts of the committed design as the newest committed frame planned them.
    pub committed_masts: Option<Vec<SolvedTier>>,
    /// The long-press text card, if one is open.
    pub info: Option<String>,
    /// The hint line (drag feedback, Slice instructions).
    pub hint: String,
    /// The Slice tool.
    pub slice: SliceTool,
    pub(crate) tracker: GestureTracker,
    pub(crate) target: Option<HandleTarget>,
    pub(crate) drag: Option<ActiveDrag>,
    pub(crate) drag_applied_at: Duration,
    pub(crate) moved_facets: Vec<u32>,
    pub(crate) remembered_facet: Option<u32>,
    pub(crate) remembered_generation: u64,
    pub(crate) info_facet: Option<u32>,
    pub(crate) edge_guard: f32,
    pub(crate) revision: u64,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            camera: RESET_POSE,
            mode: ViewMode::Solid,
            solid_view: ViewTransform::IDENTITY,
            diagram_view: ViewTransform::IDENTITY,
            enlarged_panel: -1,
            view_size: (0.0, 0.0),
            scale_factor: Cell::new(1.0),
            options: Cell::new(Options::default()),
            touch: render_view::TouchState::default(),
            light_drag: render_view::LightDrag::default(),
            driver: render_view::RenderDriver::default(),
            render_live: render_drag::LiveDrag::default(),
            handles_on: true,
            frame: None,
            status: String::new(),
            committed_masts: None,
            info: None,
            hint: String::new(),
            slice: SliceTool::default(),
            tracker: GestureTracker::default(),
            target: None,
            drag: None,
            drag_applied_at: Duration::ZERO,
            moved_facets: Vec::new(),
            remembered_facet: None,
            remembered_generation: 0,
            info_facet: None,
            edge_guard: 0.0,
            revision: 0,
        }
    }
}

// Same lexical module: private fields and helpers retain their existing visibility.
include!("viewport/state.rs");

/// A usable display scale: finite and at least 1.
const fn sane_scale(scale_factor: f32) -> f32 {
    if scale_factor.is_finite() {
        scale_factor.max(1.0)
    } else {
        1.0
    }
}

/// The camera distance after zooming by `factor` (above 1 moves closer), clamped to the
/// range the solid's bounding radius allows.
#[must_use]
pub fn pinch_distance(distance: f32, factor: f32, bounding_radius: f64) -> f32 {
    if !factor.is_finite() || factor <= 0.0 {
        return distance;
    }
    let (min, max) = camera::orbit_distance_bounds(bounding_radius);
    (distance / factor).clamp(min, max)
}
