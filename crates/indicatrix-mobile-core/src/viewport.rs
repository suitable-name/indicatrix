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
//!   the status line, dragging the light, the export options).
//!
//! Nothing here knows Slint. The app crate forwards pointer events in, runs
//! [`frame::RasterWorker`] on a background queue and pushes the results out; every
//! decision is made here so that it can be tested natively.

pub mod frame;
pub mod gesture;
pub mod manip;
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

impl ViewportState {
    // ---- configuration -------------------------------------------------------------

    /// Records the viewport's size (logical px) and the display scale. `compact` says
    /// the viewport spans the window's width, so its left and right edges belong to the
    /// system back gesture. Returns whether anything changed.
    pub fn set_view(&mut self, width: f32, height: f32, scale_factor: f32, compact: bool) -> bool {
        let guard = if compact { EDGE_GUARD_PX } else { 0.0 };
        let scale = sane_scale(scale_factor);
        let changed = self.view_size != (width, height)
            || (self.scale_factor.get() - scale).abs() > f32::EPSILON
            || (self.edge_guard - guard).abs() > f32::EPSILON;
        self.view_size = (width, height);
        self.scale_factor.set(scale);
        self.edge_guard = guard;
        self.tracker.set_view(width, height, guard);
        changed
    }

    /// Notes the display's scale factor as the window reports it now. The window may
    /// learn its real scale after the first layout (and a resize does not always follow),
    /// and a raster drawn at the wrong scale is blurry, so the push path calls this on
    /// every push. Returns whether the scale changed (the raster size then changes, which
    /// the next frame key picks up).
    pub fn note_scale_factor(&self, scale_factor: f32) -> bool {
        let scale = sane_scale(scale_factor);
        let changed = (self.scale_factor.get() - scale).abs() > f32::EPSILON;
        self.scale_factor.set(scale);
        changed
    }

    /// Applies the settings the viewport reads: whether handles are large (and whether
    /// the window is compact, which makes them larger still), snapping, and the raster
    /// cap (battery saver lowers it).
    pub fn set_options(&self, large_handles: bool, compact: bool, snap: bool, max_edge: u32) {
        self.options.set(Options {
            handle_scale: handle_scale_for(large_handles, compact),
            snap,
            max_edge,
        });
    }

    /// Marks the picture as owing a new job.
    pub(crate) const fn touch(&mut self) {
        self.revision += 1;
    }

    /// Shows or hides the manipulation handles (the Handles chip).
    pub const fn set_handles_on(&mut self, on: bool) {
        if self.handles_on != on {
            self.handles_on = on;
            self.touch();
        }
    }

    // ---- queries -------------------------------------------------------------------

    /// The raster size the current view, mode and settings ask for.
    #[must_use]
    pub fn raster_size(&self) -> (u32, u32) {
        frame::raster_size(
            self.mode,
            self.enlarged_panel >= 0,
            self.view_size,
            self.scale_factor.get(),
            self.options.get().max_edge,
        )
    }

    /// The pan and zoom of the current mode.
    #[must_use]
    pub const fn transform(&self) -> ViewTransform {
        self.transform_of(self.mode)
    }

    /// The size of the picture on screen (the newest landed frame), if there is one.
    #[must_use]
    pub fn image_size(&self) -> Option<(u32, u32)> {
        self.frame
            .as_ref()
            .map(|frame| frame.size)
            .filter(|size| size.0 > 0 && size.1 > 0)
    }

    /// Where the newest picture sits in the view.
    #[must_use]
    pub fn image_fit(&self) -> Option<ContainFit> {
        let frame = self.frame.as_ref()?;
        self.transform_of(frame.shown)
            .fit(self.view_size, self.image_size()?)
    }

    /// The pan and zoom that belong to a picture of `shown`.
    const fn transform_of(&self, shown: ViewMode) -> ViewTransform {
        match shown {
            ViewMode::Solid | ViewMode::Render => self.solid_view,
            ViewMode::Diagram => self.diagram_view,
        }
    }

    /// The picture's rectangle in view coordinates as `(x, y, width, height)`.
    #[must_use]
    pub fn image_rect(&self) -> Option<(f32, f32, f32, f32)> {
        let fit = self.image_fit()?;
        let (width, height) = self.image_size()?;
        Some((
            fit.offset_x,
            fit.offset_y,
            width as f32 * fit.scale,
            height as f32 * fit.scale,
        ))
    }

    /// The shown solid's bounding radius (zoom clamp and Fit).
    #[must_use]
    pub fn bounding_radius(&self) -> f64 {
        self.frame
            .as_ref()
            .map_or(DEFAULT_MESH_BOUNDING_RADIUS, |frame| frame.bounding_radius)
    }

    /// The pick-frame point under the view position `(x, y)` (not clamped).
    pub(crate) fn pick_point(&self, x: f32, y: f32) -> Option<ScreenPoint> {
        let (px, py) = self.image_fit()?.to_image(x, y);
        Some(ScreenPoint::new(px, py))
    }

    /// The facet under the view position `(x, y)` in the newest picture.
    #[must_use]
    pub fn facet_at(&self, x: f32, y: f32) -> Option<u32> {
        let frame = self.frame.as_ref()?;
        let size = self.image_size()?;
        let fit = self.transform_of(frame.shown).fit(self.view_size, size)?;
        let (px, py) = pixel_at(fit, size, x, y)?;
        match frame.shown {
            // A frame is only ever drawn as Solid or Diagram (`raster_mode`).
            ViewMode::Solid | ViewMode::Render => frame.pick.as_ref()?.facet_at(px, py),
            ViewMode::Diagram => frame.diagram.as_ref()?.pick_at(px, py),
        }
    }

    /// The tier that owns `facet` in the newest picture.
    #[must_use]
    pub fn tier_of_facet(&self, facet: u32) -> Option<usize> {
        self.frame
            .as_ref()?
            .facet_tier
            .get(facet as usize)
            .copied()
            .flatten()
    }

    /// The handle target that is on show: in Slice mode only the provisional tier's, else
    /// the selected tier's (when handles are on and the Solid view is shown).
    pub(crate) fn active_target(&self, selected_tier: Option<usize>) -> Option<&HandleTarget> {
        let solid = self
            .frame
            .as_ref()
            .is_some_and(|frame| frame.shown == ViewMode::Solid)
            && !self.mode.is_render();
        if !solid {
            return None;
        }
        let target = self.target.as_ref()?;
        if self.slice.mode {
            return target.provisional.then_some(target);
        }
        (self.handles_on && !target.provisional && selected_tier == Some(target.tier))
            .then_some(target)
    }

    /// The handles to draw for `selected_tier`, in view coordinates.
    #[must_use]
    pub fn handle_overlay(&self, selected_tier: Option<usize>) -> HandleOverlay {
        let hidden = HandleOverlay {
            active: -1,
            ..HandleOverlay::default()
        };
        let (Some(target), Some(fit)) = (self.active_target(selected_tier), self.image_fit())
        else {
            return hidden;
        };
        let at = |p: ScreenPoint| fit.to_view(p.x, p.y);
        let layout = &target.layout;
        HandleOverlay {
            visible: true,
            anchor: at(layout.anchor),
            angle: at(layout.angle_tip),
            depth: at(layout.depth_tip),
            index: at(layout.index_tip),
            index_visible: !target.frame.is_indexless(),
            active: self
                .drag
                .as_ref()
                .map_or(-1, |drag| kind_to_int(drag.start.kind)),
        }
    }

    /// The hint line to show: the transient hint, else what the Slice tool or the
    /// selected tier's handles do.
    #[must_use]
    pub fn hint_text(&self, selected_tier: Option<usize>) -> String {
        if !self.hint.is_empty() {
            return self.hint.clone();
        }
        if self.slice.mode {
            return if self.slice.session.is_none() {
                slice::SLICE_MODE_HINT.to_owned()
            } else {
                self.provisional_hint()
            };
        }
        self.active_target(selected_tier)
            .map(|target| text::handle_select_hint(&target.label))
            .unwrap_or_default()
    }

    // ---- camera and view -----------------------------------------------------------

    /// Switches between Solid and Diagram.
    pub fn set_mode(&mut self, mode: ViewMode) -> bool {
        if self.mode == mode {
            return false;
        }
        self.mode = mode;
        self.info = None;
        self.info_facet = None;
        self.touch();
        true
    }

    /// Applies a camera pill: `0` Front, `1` Top, `2` Bottom, `3` Left, `4` Right,
    /// `5` Fit, `6` Reset.
    pub fn set_standard_view(&mut self, kind: i32) {
        self.camera = if kind == 6 {
            RESET_POSE
        } else {
            camera::standard_view(kind, self.camera, self.bounding_radius())
        };
        self.solid_view = ViewTransform::IDENTITY;
        if kind == 6 || kind == 5 {
            self.diagram_view = ViewTransform::IDENTITY;
        }
    }

    /// Orbits by `(dx, dy)` logical px.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.camera = camera::orbit_step(self.camera, dx, dy);
    }

    /// Zooms the Solid view by `factor` (above 1 moves the camera closer).
    pub fn zoom_camera(&mut self, factor: f32) {
        self.camera.distance = pinch_distance(self.camera.distance, factor, self.bounding_radius());
    }

    /// Pans the current view by `(dx, dy)` logical px.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let size = self.image_size().unwrap_or((1, 1));
        let view = self.view_size;
        match self.mode {
            ViewMode::Solid => self.solid_view = self.solid_view.panned(dx, dy).clamped(view, size),
            ViewMode::Diagram => {
                self.diagram_view = self.diagram_view.panned(dx, dy).clamped(view, size);
            }
            // The rendered picture always sits centered; two fingers only zoom it.
            ViewMode::Render => {}
        }
    }

    /// Zooms the current view by `factor` about `(x, y)`: the camera in Solid, the
    /// picture (1x to 6x) in Diagram.
    pub fn zoom_about(&mut self, factor: f32, x: f32, y: f32) {
        match self.mode {
            ViewMode::Solid | ViewMode::Render => self.zoom_camera(factor),
            ViewMode::Diagram => {
                let size = self.image_size().unwrap_or((1, 1));
                self.diagram_view = self
                    .diagram_view
                    .zoomed(factor, (x, y), self.view_size, MAX_ZOOM)
                    .clamped(self.view_size, size);
            }
        }
    }

    /// A double tap at `(x, y)`: resets the Solid view, enlarges or restores a diagram
    /// panel.
    fn double_tap(&mut self, x: f32, y: f32) {
        match self.mode {
            ViewMode::Solid | ViewMode::Render => self.set_standard_view(6),
            ViewMode::Diagram => {
                let panel = self.frame.as_ref().and_then(|frame| {
                    let size = self.image_size()?;
                    let fit = self.transform_of(frame.shown).fit(self.view_size, size)?;
                    let (px, py) = pixel_at(fit, size, x, y)?;
                    frame.diagram.as_ref()?.panel_at(px, py)
                });
                self.enlarged_panel = toggle_enlarged_panel(self.enlarged_panel, panel);
                self.diagram_view = ViewTransform::IDENTITY;
            }
        }
    }

    // ---- pointer input -------------------------------------------------------------

    /// Feeds a raw pointer event through the recognizer and acts on the result.
    pub fn pointer(&mut self, event: Pointer, env: &mut Env<'_>) -> Outcome {
        let gesture = match event {
            Pointer::Down {
                finger,
                button,
                x,
                y,
            } => {
                let render = self.mode.is_render();
                let context = Context {
                    handle_hit: button == Button::Left
                        && !render
                        && self.hit_handle(x, y, *env.selected_tier).is_some(),
                    slice_mode: self.slice.mode && !render,
                    drag_pans: self.mode == ViewMode::Diagram,
                };
                self.tracker.down(env.now, finger, button, x, y, context)
            }
            Pointer::Move { x, y } => self.tracker.moved(x, y),
            Pointer::Up { finger, x, y } => self.tracker.up(env.now, finger, x, y),
            Pointer::Cancel => self.tracker.cancel(),
        };
        gesture.map_or_else(Outcome::default, |gesture| self.gesture(gesture, env))
    }

    /// Fires the long press if the held press is due.
    pub fn long_press_tick(&mut self, env: &mut Env<'_>) -> Outcome {
        self.tracker
            .poll_long_press(env.now)
            .map_or_else(Outcome::default, |gesture| self.gesture(gesture, env))
    }

    /// Feeds a mouse-wheel step (positive zooms in).
    pub fn wheel(&mut self, delta_y: f32, x: f32, y: f32, env: &mut Env<'_>) -> Outcome {
        let gesture = self.tracker.wheel(delta_y, x, y);
        self.gesture(gesture, env)
    }

    /// Feeds a two-finger pinch event.
    pub fn pinch(&mut self, event: Pinch, env: &mut Env<'_>) -> Outcome {
        let gesture = match event {
            Pinch::Begin { x, y } => {
                // A pinch takes over from whatever one-finger gesture was running.
                let mut out = Outcome::default();
                if let Some(cancelled) = self.tracker.cancel() {
                    out.merge(self.gesture(cancelled, env));
                }
                self.tracker.pinch_begin(x, y);
                return out;
            }
            Pinch::Update { scale, x, y } => self.tracker.pinch_update(scale, x, y),
            Pinch::End => {
                self.tracker.pinch_end();
                None
            }
        };
        gesture.map_or_else(Outcome::default, |gesture| self.gesture(gesture, env))
    }

    /// Acts on one recognized gesture.
    pub fn gesture(&mut self, gesture: Gesture, env: &mut Env<'_>) -> Outcome {
        match gesture {
            Gesture::Orbit { dx, dy } => {
                self.orbit(dx, dy);
                Outcome::redraw()
            }
            Gesture::Pan { dx, dy } => {
                self.pan(dx, dy);
                Outcome::redraw()
            }
            Gesture::Zoom { factor, x, y } => {
                self.zoom_about(factor, x, y);
                Outcome::redraw()
            }
            Gesture::Pinch {
                factor,
                x,
                y,
                dx,
                dy,
            } => {
                self.zoom_about(factor, x, y);
                self.pan(dx, dy);
                Outcome::redraw()
            }
            Gesture::Tap { x, y } => self.tap(x, y, env),
            Gesture::DoubleTap { x, y } => {
                self.double_tap(x, y);
                Outcome::redraw()
            }
            Gesture::LongPress { x, y } => self.long_press(x, y),
            Gesture::HandleBegin { x, y } => self.handle_begin(x, y, env),
            Gesture::HandleMove { x, y } => self.handle_move(x, y, env),
            Gesture::HandleEnd => self.handle_end(env),
            Gesture::HandleCancel => self.handle_cancel(env),
            Gesture::SliceBegin { x, y } => self.slice_begin(x, y),
            Gesture::SliceMove { x, y } => self.slice_move(x, y),
            Gesture::SliceEnd { x, y } => self.slice_end(x, y, env),
            Gesture::SliceCancel => self.slice_cancel(),
        }
    }

    /// A tap at `(x, y)`: selects the facet's tier, or clears the selection when it hit
    /// nothing. While a provisional slice exists taps select nothing.
    fn tap(&mut self, x: f32, y: f32, env: &mut Env<'_>) -> Outcome {
        if self.slice.session.is_some() || self.slice.mode || self.mode.is_render() {
            return Outcome::default();
        }
        let had_info = self.info.take().is_some();
        self.info_facet = None;
        let facet = self.facet_at(x, y);
        let generation = self.frame.as_ref().map_or(0, |frame| frame.generation);
        self.remembered_facet = facet;
        self.remembered_generation = generation;
        self.hint.clear();
        self.touch();
        let mut out = Outcome::redraw();
        out.refresh = had_info;
        match facet {
            None => {
                env.select(None);
                out.select = Some(None);
                out.refresh = true;
            }
            Some(facet) => {
                // A facet with no tier (a preform plane) stays lit but selects nothing.
                if let Some(tier) = self.tier_of_facet(facet) {
                    env.select(Some(tier));
                    out.select = Some(Some(tier));
                    out.refresh = true;
                }
            }
        }
        out
    }

    /// A long press at `(x, y)`: opens the text card of the facet under it.
    fn long_press(&mut self, x: f32, y: f32) -> Outcome {
        if self.mode.is_render() {
            return Outcome::default();
        }
        let Some(facet) = self.facet_at(x, y) else {
            return Outcome::default();
        };
        let text = self
            .frame
            .as_ref()
            .and_then(|frame| frame.hover_text.get(facet as usize))
            .filter(|text| !text.is_empty())
            .cloned();
        let Some(text) = text else {
            return Outcome::default();
        };
        self.info = Some(text);
        self.info_facet = Some(facet);
        self.touch();
        Outcome::redraw()
    }

    /// Closes the long-press card.
    pub fn close_info(&mut self) -> bool {
        let was_open = self.info.take().is_some();
        if was_open {
            self.info_facet = None;
            self.touch();
        }
        was_open
    }

    // ---- jobs and frames -----------------------------------------------------------

    /// The facet to outline as selected, if it still describes `generation` and the
    /// selected tier.
    fn outlined_facet(&self, generation: u64, selected_tier: Option<usize>) -> Option<u32> {
        if let Some(facet) = self.info_facet {
            return Some(facet);
        }
        let facet = self.remembered_facet?;
        let tier = self.tier_of_facet(facet)?;
        (self.remembered_generation == generation && selected_tier == Some(tier)).then_some(facet)
    }

    /// The facet the handles anchor on. It survives edits (the facet ids of one tier stay
    /// put while it is dragged); the worker only uses it while it belongs to the tier.
    const fn anchor_facet(&self) -> Option<u32> {
        self.remembered_facet
    }

    /// The key that tells whether a new job is needed: `generation` is the session's,
    /// `solved` says whether a solve is current for it.
    #[must_use]
    pub fn frame_key(
        &self,
        generation: u64,
        solved: bool,
        selected_tier: Option<usize>,
        multi_selected: &BTreeSet<usize>,
    ) -> FrameKey {
        let provisional = self.slice.session.is_some();
        FrameKey {
            generation: if provisional {
                PROVISIONAL_GENERATION
            } else {
                generation
            },
            solved,
            selected_tier,
            multi_selected: multi_selected.clone(),
            camera: self.camera,
            size: self.raster_size(),
            mode: self.mode.raster_mode(),
            enlarged_panel: self.enlarged_panel,
            want_handles: self.handles_on,
            revision: self.revision,
        }
    }

    /// The job for the current state: the committed design, or the Slice tool's
    /// provisional design while it exists.
    #[must_use]
    pub fn job(&self, token: u64, inputs: JobInputs) -> RasterJob {
        let provisional = self.slice.session.as_ref();
        let (design, generation, chain) = match provisional {
            Some(session) => {
                let (masts, dirty) = session.replan_chain(self.committed_masts.as_deref());
                (
                    Arc::clone(&session.design),
                    PROVISIONAL_GENERATION,
                    masts.map(|masts| (masts, dirty)),
                )
            }
            None => (Arc::clone(&inputs.design), inputs.generation, None),
        };
        RasterJob {
            token,
            design,
            generation,
            solved: if provisional.is_some() {
                None
            } else {
                inputs.solved
            },
            chain,
            camera: self.camera,
            size: self.raster_size(),
            mode: self.mode.raster_mode(),
            selected_tier: inputs.selected_tier,
            multi_selected: inputs.multi_selected,
            selected_facet: self.outlined_facet(inputs.generation, inputs.selected_tier),
            provisional_facets: self.slice.outline.clone(),
            moved_facets: self.moved_facets.clone(),
            enlarged_panel: self.enlarged_panel,
            remembered_facet: self.anchor_facet(),
            want_handles: self.handles_on,
            provisional: provisional.is_some(),
        }
    }

    /// Takes a finished frame. The caller makes the image from `frame.rgba` first; the
    /// pixels are dropped here.
    pub fn land(&mut self, mut frame: RasterFrame) -> Outcome {
        if frame.kind == FrameKind::Waiting {
            self.status = frame.status;
            return Outcome::redraw();
        }
        frame.rgba = Vec::new();
        if let Some(masts) = frame.masts.take() {
            if frame.provisional {
                if let Some(session) = self.slice.session.as_mut() {
                    session.note_masts(masts);
                }
            } else {
                self.committed_masts = Some(masts);
            }
        }
        self.status.clone_from(&frame.status);
        let target = if frame.provisional {
            frame.geometry.as_ref().and_then(|geometry| {
                self.slice
                    .session
                    .as_mut()
                    .and_then(|session| session.place(geometry))
            })
        } else {
            frame.target.clone()
        };
        self.target = target;
        self.frame = Some(frame);
        let outline_changed = self.slice_sync_outline();
        if outline_changed {
            self.touch();
        }
        self.refresh_drag_feedback();
        Outcome::redraw()
    }
}

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
