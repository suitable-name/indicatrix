//! Touch and mouse gesture recognition for the viewport, as a small state machine that
//! knows nothing about Slint.
//!
//! The Slint side forwards raw pointer events (down, move, up, a long-press tick, the
//! pinch handler's updates, the mouse wheel) and gets back at most one [`Gesture`] per
//! event. One finger (or the left mouse button) orbits, taps or drags a handle or a slice
//! line; two fingers (Slint's pinch handler) zoom and pan; the right mouse button pans
//! and the wheel zooms, which is how the desktop preview imitates a phone.
//!
//! Slint's own touch layer turns a second finger into a synthesized release of the first
//! (carrying the second finger's id) followed by pinch events, so an `up` whose finger id
//! differs from the one that went down is an interruption, never a tap.

use std::time::Duration;

/// How far a pointer may travel and still count as a tap or a press, in logical px.
pub const TAP_SLOP_PX: f32 = 8.0;
/// The longest press that still counts as a tap.
pub const TAP_MAX: Duration = Duration::from_millis(400);
/// How long a still press must last to become a long press.
pub const LONG_PRESS: Duration = Duration::from_millis(450);
/// The longest gap between two taps of a double tap.
pub const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(350);
/// The farthest two taps of a double tap may be apart, in logical px.
pub const DOUBLE_TAP_DISTANCE_PX: f32 = 48.0;
/// A press that starts this close to a guarded edge belongs to the system back gesture.
pub const EDGE_GUARD_PX: f32 = 24.0;
/// How fast the mouse wheel zooms: the exponent per scrolled pixel.
const WHEEL_RATE: f32 = 0.0015;
/// The largest wheel step taken into account (a free-spinning wheel reports big deltas).
const WHEEL_MAX_DELTA: f32 = 400.0;

/// The pointer button an event came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// A finger or the left mouse button.
    Left,
    /// The right mouse button (the preview's two-finger pan).
    Right,
    /// Anything else.
    Other,
}

/// What the viewport knows about the spot where a press lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context {
    /// The press landed on a manipulation handle.
    pub handle_hit: bool,
    /// The Slice tool is on: a drag draws a line.
    pub slice_mode: bool,
    /// A one-finger drag pans instead of orbiting (the Diagram view).
    pub drag_pans: bool,
}

/// A recognized gesture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    /// A one-finger drag: orbit by `(dx, dy)` logical px since the last report.
    Orbit {
        /// Horizontal travel.
        dx: f32,
        /// Vertical travel.
        dy: f32,
    },
    /// A drag that moves the picture by `(dx, dy)` logical px.
    Pan {
        /// Horizontal travel.
        dx: f32,
        /// Vertical travel.
        dy: f32,
    },
    /// A zoom by `factor` (above 1 zooms in) about `(x, y)`, from the wheel.
    Zoom {
        /// The zoom factor.
        factor: f32,
        /// Horizontal anchor.
        x: f32,
        /// Vertical anchor.
        y: f32,
    },
    /// A two-finger update: zoom by `factor` about `(x, y)` and pan by `(dx, dy)`.
    Pinch {
        /// The zoom factor since the last report.
        factor: f32,
        /// Horizontal anchor (the fingers' centre).
        x: f32,
        /// Vertical anchor.
        y: f32,
        /// Horizontal travel of the centre.
        dx: f32,
        /// Vertical travel of the centre.
        dy: f32,
    },
    /// A short press and release in place.
    Tap {
        /// Horizontal position.
        x: f32,
        /// Vertical position.
        y: f32,
    },
    /// The second tap of a double tap.
    DoubleTap {
        /// Horizontal position.
        x: f32,
        /// Vertical position.
        y: f32,
    },
    /// A press held still.
    LongPress {
        /// Horizontal position.
        x: f32,
        /// Vertical position.
        y: f32,
    },
    /// A handle was grabbed.
    HandleBegin {
        /// Horizontal position.
        x: f32,
        /// Vertical position.
        y: f32,
    },
    /// The grabbed handle moved.
    HandleMove {
        /// Horizontal position.
        x: f32,
        /// Vertical position.
        y: f32,
    },
    /// The grabbed handle was released.
    HandleEnd,
    /// The handle drag was interrupted and should be undone.
    HandleCancel,
    /// A slice line started.
    SliceBegin {
        /// Horizontal position.
        x: f32,
        /// Vertical position.
        y: f32,
    },
    /// The slice line's end moved.
    SliceMove {
        /// Horizontal position.
        x: f32,
        /// Vertical position.
        y: f32,
    },
    /// The slice line was released.
    SliceEnd {
        /// Horizontal position.
        x: f32,
        /// Vertical position.
        y: f32,
    },
    /// The slice line was interrupted and should be dropped.
    SliceCancel,
}

/// What the tracker is doing between events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Idle,
    /// A press that edge rejection refused; ignored until it ends.
    Rejected,
    /// A press that has not moved far enough to be a drag yet.
    Pending,
    Orbit,
    Pan,
    Handle,
    Slice,
    Pinch,
}

/// The gesture state machine. See the module documentation.
#[derive(Debug)]
pub struct GestureTracker {
    mode: Mode,
    finger: i32,
    start: (f32, f32),
    last: (f32, f32),
    down_at: Duration,
    long_pressed: bool,
    drag_pans: bool,
    last_tap: Option<(Duration, (f32, f32))>,
    view: (f32, f32),
    edge_guard: f32,
    pinch_scale: f32,
    pinch_centre: (f32, f32),
}

impl Default for GestureTracker {
    fn default() -> Self {
        Self {
            mode: Mode::Idle,
            finger: 0,
            start: (0.0, 0.0),
            last: (0.0, 0.0),
            down_at: Duration::ZERO,
            long_pressed: false,
            drag_pans: false,
            last_tap: None,
            view: (0.0, 0.0),
            edge_guard: 0.0,
            pinch_scale: 1.0,
            pinch_centre: (0.0, 0.0),
        }
    }
}

impl GestureTracker {
    /// Sets the viewport size (logical px) and the width of the guarded strip along its
    /// left and right edges (`0` for none).
    pub const fn set_view(&mut self, width: f32, height: f32, edge_guard: f32) {
        self.view = (width, height);
        self.edge_guard = edge_guard;
    }

    /// Whether no gesture is in progress.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.mode == Mode::Idle
    }

    /// Whether a press is held that has neither moved nor been released (the state in
    /// which a long press can still fire).
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.mode == Mode::Pending && !self.long_pressed
    }

    const fn reset(&mut self) {
        self.mode = Mode::Idle;
        self.long_pressed = false;
    }

    /// A pointer went down at `(x, y)` at time `now`.
    pub fn down(
        &mut self,
        now: Duration,
        finger: i32,
        button: Button,
        x: f32,
        y: f32,
        context: Context,
    ) -> Option<Gesture> {
        if self.mode == Mode::Pinch {
            return None;
        }
        self.reset();
        self.finger = finger;
        self.start = (x, y);
        self.last = (x, y);
        self.down_at = now;
        self.drag_pans = context.drag_pans;
        let guarded =
            self.edge_guard > 0.0 && (x < self.edge_guard || x > self.view.0 - self.edge_guard);
        if guarded {
            self.mode = Mode::Rejected;
            return None;
        }
        match button {
            Button::Right => {
                self.mode = Mode::Pan;
                None
            }
            Button::Other => {
                self.mode = Mode::Rejected;
                None
            }
            Button::Left if context.handle_hit => {
                self.mode = Mode::Handle;
                Some(Gesture::HandleBegin { x, y })
            }
            Button::Left if context.slice_mode => {
                self.mode = Mode::Slice;
                Some(Gesture::SliceBegin { x, y })
            }
            Button::Left => {
                self.mode = Mode::Pending;
                None
            }
        }
    }

    /// The pointer moved to `(x, y)`.
    pub fn moved(&mut self, x: f32, y: f32) -> Option<Gesture> {
        let (dx, dy) = (x - self.last.0, y - self.last.1);
        match self.mode {
            Mode::Pending => {
                let travelled = (x - self.start.0).hypot(y - self.start.1);
                if travelled <= TAP_SLOP_PX {
                    return None;
                }
                self.last = (x, y);
                let (dx, dy) = (x - self.start.0, y - self.start.1);
                if self.drag_pans {
                    self.mode = Mode::Pan;
                    Some(Gesture::Pan { dx, dy })
                } else {
                    self.mode = Mode::Orbit;
                    Some(Gesture::Orbit { dx, dy })
                }
            }
            Mode::Orbit => {
                self.last = (x, y);
                Some(Gesture::Orbit { dx, dy })
            }
            Mode::Pan => {
                self.last = (x, y);
                Some(Gesture::Pan { dx, dy })
            }
            Mode::Handle => {
                self.last = (x, y);
                Some(Gesture::HandleMove { x, y })
            }
            Mode::Slice => {
                self.last = (x, y);
                Some(Gesture::SliceMove { x, y })
            }
            Mode::Idle | Mode::Rejected | Mode::Pinch => None,
        }
    }

    /// The pointer went up at `(x, y)` at time `now`. `finger` is the id the event
    /// carries (`0` for a mouse).
    pub fn up(&mut self, now: Duration, finger: i32, x: f32, y: f32) -> Option<Gesture> {
        if self.mode == Mode::Pinch {
            return None;
        }
        let interrupted = finger != 0 && self.finger != 0 && finger != self.finger;
        let mode = self.mode;
        let was_long = self.long_pressed;
        self.reset();
        if interrupted {
            return match mode {
                Mode::Handle => Some(Gesture::HandleEnd),
                Mode::Slice => Some(Gesture::SliceCancel),
                _ => None,
            };
        }
        match mode {
            Mode::Pending if !was_long && now.saturating_sub(self.down_at) <= TAP_MAX => {
                Some(self.tap(now, x, y))
            }
            Mode::Handle => Some(Gesture::HandleEnd),
            Mode::Slice => Some(Gesture::SliceEnd { x, y }),
            _ => None,
        }
    }

    /// The tap at `(x, y)`: a double tap when it follows a near, recent one.
    fn tap(&mut self, now: Duration, x: f32, y: f32) -> Gesture {
        let double = self.last_tap.is_some_and(|(at, (tx, ty))| {
            now.saturating_sub(at) <= DOUBLE_TAP_WINDOW
                && (x - tx).hypot(y - ty) <= DOUBLE_TAP_DISTANCE_PX
        });
        if double {
            self.last_tap = None;
            Gesture::DoubleTap { x, y }
        } else {
            self.last_tap = Some((now, (x, y)));
            Gesture::Tap { x, y }
        }
    }

    /// The pointer stream was cancelled (focus lost, the grab was taken away).
    pub const fn cancel(&mut self) -> Option<Gesture> {
        let mode = self.mode;
        self.reset();
        match mode {
            Mode::Handle => Some(Gesture::HandleCancel),
            Mode::Slice => Some(Gesture::SliceCancel),
            _ => None,
        }
    }

    /// Fires the long press when a press has been held still for [`LONG_PRESS`].
    pub fn poll_long_press(&mut self, now: Duration) -> Option<Gesture> {
        if self.mode == Mode::Pending
            && !self.long_pressed
            && now.saturating_sub(self.down_at) >= LONG_PRESS
        {
            self.long_pressed = true;
            return Some(Gesture::LongPress {
                x: self.start.0,
                y: self.start.1,
            });
        }
        None
    }

    /// The mouse wheel turned by `delta_y` logical px at `(x, y)` (positive: zoom in).
    #[must_use]
    pub fn wheel(&self, delta_y: f32, x: f32, y: f32) -> Gesture {
        let delta = delta_y.clamp(-WHEEL_MAX_DELTA, WHEEL_MAX_DELTA);
        Gesture::Zoom {
            factor: (delta * WHEEL_RATE).exp(),
            x,
            y,
        }
    }

    /// Two fingers started a pinch with their centre at `(x, y)`.
    pub const fn pinch_begin(&mut self, x: f32, y: f32) {
        self.reset();
        self.mode = Mode::Pinch;
        self.pinch_scale = 1.0;
        self.pinch_centre = (x, y);
    }

    /// The pinch's cumulative `scale` (1 at the start) and centre changed.
    pub fn pinch_update(&mut self, scale: f32, x: f32, y: f32) -> Option<Gesture> {
        if self.mode != Mode::Pinch || !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let factor = scale / self.pinch_scale.max(f32::EPSILON);
        let (dx, dy) = (x - self.pinch_centre.0, y - self.pinch_centre.1);
        self.pinch_scale = scale;
        self.pinch_centre = (x, y);
        Some(Gesture::Pinch {
            factor,
            x,
            y,
            dx,
            dy,
        })
    }

    /// The pinch ended or was cancelled.
    pub fn pinch_end(&mut self) {
        if self.mode == Mode::Pinch {
            self.mode = Mode::Idle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTX: Context = Context {
        handle_hit: false,
        slice_mode: false,
        drag_pans: false,
    };

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn tracker() -> GestureTracker {
        let mut tracker = GestureTracker::default();
        tracker.set_view(400.0, 300.0, EDGE_GUARD_PX);
        tracker
    }

    #[test]
    fn a_short_press_in_place_is_a_tap() {
        let mut t = tracker();
        assert_eq!(t.down(ms(0), 1, Button::Left, 200.0, 150.0, CTX), None);
        assert_eq!(
            t.up(ms(120), 1, 202.0, 151.0),
            Some(Gesture::Tap { x: 202.0, y: 151.0 })
        );
        assert!(t.is_idle());
    }

    #[test]
    fn two_quick_taps_make_a_double_tap() {
        let mut t = tracker();
        t.down(ms(0), 1, Button::Left, 200.0, 150.0, CTX);
        let first = t.up(ms(80), 1, 200.0, 150.0);
        assert!(matches!(first, Some(Gesture::Tap { .. })));
        t.down(ms(200), 1, Button::Left, 205.0, 152.0, CTX);
        assert!(matches!(
            t.up(ms(260), 1, 205.0, 152.0),
            Some(Gesture::DoubleTap { .. })
        ));
        // The third tap starts over.
        t.down(ms(300), 1, Button::Left, 205.0, 152.0, CTX);
        assert!(matches!(
            t.up(ms(340), 1, 205.0, 152.0),
            Some(Gesture::Tap { .. })
        ));
    }

    #[test]
    fn slow_second_tap_is_a_plain_tap() {
        let mut t = tracker();
        t.down(ms(0), 1, Button::Left, 200.0, 150.0, CTX);
        t.up(ms(80), 1, 200.0, 150.0);
        t.down(ms(900), 1, Button::Left, 200.0, 150.0, CTX);
        assert!(matches!(
            t.up(ms(950), 1, 200.0, 150.0),
            Some(Gesture::Tap { .. })
        ));
    }

    #[test]
    fn movement_past_the_slop_orbits_from_the_press_point() {
        let mut t = tracker();
        t.down(ms(0), 1, Button::Left, 200.0, 150.0, CTX);
        assert_eq!(t.moved(204.0, 150.0), None);
        assert_eq!(
            t.moved(220.0, 160.0),
            Some(Gesture::Orbit { dx: 20.0, dy: 10.0 })
        );
        assert_eq!(
            t.moved(225.0, 160.0),
            Some(Gesture::Orbit { dx: 5.0, dy: 0.0 })
        );
        // Releasing after a drag is not a tap.
        assert_eq!(t.up(ms(300), 1, 225.0, 160.0), None);
    }

    #[test]
    fn diagram_drags_pan() {
        let mut t = tracker();
        let ctx = Context {
            drag_pans: true,
            ..CTX
        };
        t.down(ms(0), 1, Button::Left, 200.0, 150.0, ctx);
        assert_eq!(
            t.moved(230.0, 150.0),
            Some(Gesture::Pan { dx: 30.0, dy: 0.0 })
        );
    }

    #[test]
    fn edge_presses_are_ignored() {
        let mut t = tracker();
        assert_eq!(t.down(ms(0), 1, Button::Left, 10.0, 150.0, CTX), None);
        assert_eq!(t.moved(100.0, 150.0), None);
        assert_eq!(t.up(ms(100), 1, 100.0, 150.0), None);
        assert_eq!(t.down(ms(200), 1, Button::Left, 395.0, 150.0, CTX), None);
        assert_eq!(t.moved(300.0, 150.0), None);
        t.up(ms(300), 1, 300.0, 150.0);
        // Inside the guard the press works again.
        t.down(ms(400), 1, Button::Left, 200.0, 150.0, CTX);
        assert!(matches!(
            t.up(ms(450), 1, 200.0, 150.0),
            Some(Gesture::Tap { .. })
        ));
    }

    #[test]
    fn a_still_press_becomes_a_long_press_once() {
        let mut t = tracker();
        t.down(ms(0), 1, Button::Left, 200.0, 150.0, CTX);
        assert_eq!(t.poll_long_press(ms(200)), None);
        assert_eq!(
            t.poll_long_press(ms(460)),
            Some(Gesture::LongPress { x: 200.0, y: 150.0 })
        );
        assert_eq!(t.poll_long_press(ms(900)), None);
        // The release after a long press is not a tap.
        assert_eq!(t.up(ms(1000), 1, 200.0, 150.0), None);
    }

    #[test]
    fn moving_cancels_the_long_press() {
        let mut t = tracker();
        t.down(ms(0), 1, Button::Left, 200.0, 150.0, CTX);
        t.moved(240.0, 150.0);
        assert_eq!(t.poll_long_press(ms(600)), None);
    }

    #[test]
    fn a_second_finger_interrupts_instead_of_tapping() {
        let mut t = tracker();
        t.down(ms(0), 1, Button::Left, 200.0, 150.0, CTX);
        // Slint's synthesized release carries the second finger's id.
        assert_eq!(t.up(ms(40), 2, 200.0, 150.0), None);
        assert!(t.is_idle());
    }

    #[test]
    fn handle_drags_report_begin_move_end() {
        let mut t = tracker();
        let ctx = Context {
            handle_hit: true,
            ..CTX
        };
        assert_eq!(
            t.down(ms(0), 1, Button::Left, 100.0, 100.0, ctx),
            Some(Gesture::HandleBegin { x: 100.0, y: 100.0 })
        );
        assert_eq!(
            t.moved(101.0, 100.0),
            Some(Gesture::HandleMove { x: 101.0, y: 100.0 })
        );
        assert_eq!(t.up(ms(50), 1, 101.0, 100.0), Some(Gesture::HandleEnd));
        t.down(ms(100), 1, Button::Left, 100.0, 100.0, ctx);
        assert_eq!(t.cancel(), Some(Gesture::HandleCancel));
    }

    #[test]
    fn slice_mode_draws_a_line() {
        let mut t = tracker();
        let ctx = Context {
            slice_mode: true,
            ..CTX
        };
        assert_eq!(
            t.down(ms(0), 1, Button::Left, 100.0, 100.0, ctx),
            Some(Gesture::SliceBegin { x: 100.0, y: 100.0 })
        );
        assert_eq!(
            t.moved(150.0, 120.0),
            Some(Gesture::SliceMove { x: 150.0, y: 120.0 })
        );
        assert_eq!(
            t.up(ms(90), 1, 160.0, 130.0),
            Some(Gesture::SliceEnd { x: 160.0, y: 130.0 })
        );
    }

    #[test]
    fn right_button_drag_pans() {
        let mut t = tracker();
        assert_eq!(t.down(ms(0), 0, Button::Right, 200.0, 150.0, CTX), None);
        assert_eq!(
            t.moved(210.0, 145.0),
            Some(Gesture::Pan { dx: 10.0, dy: -5.0 })
        );
        assert_eq!(t.up(ms(80), 0, 210.0, 145.0), None);
    }

    #[test]
    fn pinch_reports_incremental_factors() {
        let mut t = tracker();
        t.pinch_begin(200.0, 150.0);
        let first = t.pinch_update(1.5, 210.0, 150.0);
        assert_eq!(
            first,
            Some(Gesture::Pinch {
                factor: 1.5,
                x: 210.0,
                y: 150.0,
                dx: 10.0,
                dy: 0.0
            })
        );
        let Some(Gesture::Pinch { factor, .. }) = t.pinch_update(3.0, 210.0, 150.0) else {
            panic!("a pinch update");
        };
        assert!((factor - 2.0).abs() < 1e-6);
        t.pinch_end();
        assert!(t.is_idle());
    }

    #[test]
    fn pinch_ignores_updates_when_not_pinching() {
        let mut t = tracker();
        assert_eq!(t.pinch_update(2.0, 0.0, 0.0), None);
    }

    #[test]
    fn wheel_zooms_in_for_positive_deltas() {
        let t = tracker();
        let Gesture::Zoom { factor, x, y } = t.wheel(120.0, 50.0, 60.0) else {
            panic!("a zoom");
        };
        assert!(factor > 1.0);
        assert_eq!((x, y), (50.0, 60.0));
        let Gesture::Zoom { factor, .. } = t.wheel(-120.0, 0.0, 0.0) else {
            panic!("a zoom");
        };
        assert!(factor < 1.0);
    }
}
