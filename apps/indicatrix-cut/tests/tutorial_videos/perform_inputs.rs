//! Filmed low-level input primitives shared by the `Perform` routes: clicking a control, typing
//! into a field character-by-character, turning the mouse wheel, and finding an editable input
//! inside the (scrolling) Inspector.

use crate::{director::Film, pacing, pure_targets::wheel_steps, support::app::pump, targets::Spot};
use indicatrix_cut::GuideModel;
use slint::{
    ComponentHandle, LogicalPosition, SharedString,
    platform::{PointerEventButton, WindowEvent},
};
use std::time::Duration;

/// Time held between characters when typing into a field.
const KEY_HOLD: Duration = Duration::from_millis(55);
/// Time held after finishing a field or toggling a control before moving to the next one.
pub const FIELD_PAUSE: Duration = Duration::from_millis(200);

/// Time held after each wheel tick.
const WHEEL_HOLD: Duration = Duration::from_millis(40);

/// Time held after one sideways wheel turn, long enough for the content to be seen moving.
const WHEEL_ACROSS_HOLD: Duration = Duration::from_millis(120);

impl Film<'_> {
    /// Focuses the control at `spot` with a real pointer press/release and types `text`
    /// character-by-character into it.
    pub(crate) fn click_and_type(&mut self, spot: &Spot, text: &str) -> Result<(), String> {
        let (cx, cy) = spot.center();
        let position = LogicalPosition::new(cx, cy);
        self.glide_to((cx, cy))?;
        self.rec.hold(pacing::BEFORE_CLICK)?;
        let window = self.ui.window();
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
        self.rec.click()?;
        pump(30);
        uiprobe::view::press(window, "Ctrl+A")
            .map_err(|error| format!("keyboard input failed: {error:?}"))?;
        uiprobe::view::press(window, "Backspace")
            .map_err(|error| format!("keyboard input failed: {error:?}"))?;
        pump(20);
        self.rec.hold(Duration::from_millis(80))?;
        for ch in text.chars() {
            let key: SharedString = ch.to_string().into();
            window.dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
            window.dispatch_event(WindowEvent::KeyReleased { text: key });
            pump(15);
            self.rec.hold(KEY_HOLD)?;
        }
        if let Some(handle) = &spot.handle {
            let actual = handle.accessible_value();
            if actual.as_deref() != Some(text) {
                return Err(format!(
                    "typed field readback mismatch: expected {text:?}, observed {actual:?}"
                ));
            }
        }
        self.rec.hold(FIELD_PAUSE)
    }

    /// One activation per control: the pointer press/release is the activation. The
    /// accessible handle supplies bounds and disabled-state readback only.
    pub(crate) fn click_and_invoke(&mut self, spot: &Spot) -> Result<(), String> {
        if !spot.usable(self.view().size) {
            return Err("control is outside the visible window".into());
        }
        if let Some(handle) = &spot.handle
            && handle.accessible_enabled() == Some(false)
        {
            let guide = self.ui.global::<GuideModel>();
            return Err(format!(
                "control {:?} is disabled; guide hint={:?}, tier table allowed={}, waiting={:?}",
                handle.accessible_label(),
                guide.get_locked_hint(),
                guide.invoke_allows_tier_table(),
                guide.get_current().waiting
            ));
        }
        self.click_on(spot)?;
        let (x, y) = spot.center();
        let position = LogicalPosition::new(x, y);
        let window = self.ui.window();
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
        pump(60);
        Ok(())
    }

    /// Finds the visible editable input (`LineEdit`) labelled `label`.
    /// When `inspector` is true, searches only the Inspector subtree.
    pub(crate) fn input_spot_in(&self, label: &str, inspector: bool) -> Option<Spot> {
        let view = self.view();
        let window = view.size;
        let matches = if inspector {
            view.root
                .query_descendants()
                .match_inherits("EditorInspector")
                .find_first()?
                .query_descendants()
                .find_all()
        } else {
            view.by_label(label)
        };
        matches
            .into_iter()
            .filter(|e| e.accessible_label().is_some_and(|name| name == label))
            .filter(|e| e.accessible_role() == Some(uiprobe::AccessibleRole::TextInput))
            .map(|e| Spot::of(&e))
            .find(|s| s.usable(window))
    }

    /// Turns the mouse wheel over `spot` the way a learner would: the cursor glides to the centre
    /// of `spot`, then the wheel turns in ticks ([`wheel_steps`]) with a short hold after each so
    /// the content visibly scrolls.
    /// `delta_y` is in Slint's wheel convention (positive scrolls the content down towards its
    /// top). Lane scripts call this instead of `ElementHandle::scroll`, which moves nothing the
    /// viewer can see before the content jumps.
    pub(crate) fn wheel_over(&mut self, spot: &Spot, delta_y: f32) -> Result<(), String> {
        let (cx, cy) = spot.center();
        let position = LogicalPosition::new(cx, cy);
        self.glide_to((cx, cy))?;
        let window = self.ui.window();
        window.dispatch_event(WindowEvent::PointerMoved { position });
        for step in wheel_steps(delta_y) {
            window.dispatch_event(WindowEvent::PointerScrolled {
                position,
                delta_x: 0.0,
                delta_y: step,
            });
            pump(20);
            self.rec.hold(WHEEL_HOLD)?;
        }
        Ok(())
    }

    /// Turns the mouse wheel sideways over `spot`, once: the cursor glides to the centre of
    /// `spot`, then the wheel turns by `delta_x` (negative moves the content to the left, so that
    /// later columns come into view) and a short hold lets the movement show.
    pub(crate) fn wheel_across(&mut self, spot: &Spot, delta_x: f32) -> Result<(), String> {
        let (cx, cy) = spot.center();
        let position = LogicalPosition::new(cx, cy);
        self.glide_to((cx, cy))?;
        let window = self.ui.window();
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::PointerScrolled {
            position,
            delta_x,
            delta_y: 0.0,
        });
        pump(20);
        self.rec.hold(WHEEL_ACROSS_HOLD)
    }

    pub(crate) fn scroll_inspector(&mut self, delta: f32) -> Result<(), String> {
        let view = self.view();
        let inspector = view
            .root
            .query_descendants()
            .match_inherits("EditorInspector")
            .find_first();
        if let Some(inspector) = inspector {
            let area = inspector
                .query_descendants()
                .match_inherits("ScrollView")
                .find_first();
            let target = Spot::of(area.as_ref().unwrap_or(&inspector));
            self.wheel_over(&target, delta)?;
            pump(100);
            self.rec.hold(std::time::Duration::from_millis(180))?;
        }
        Ok(())
    }

    pub(crate) fn input_spot(&mut self, label: &str) -> Result<Option<Spot>, String> {
        let view = self.view();
        let Some(inspector) = view
            .root
            .query_descendants()
            .match_inherits("EditorInspector")
            .find_first()
        else {
            return Ok(None);
        };
        let Some(scroll) = inspector
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
        else {
            return Ok(None);
        };
        let viewport = Spot::of(&scroll);
        self.scroll_inspector(3000.0)?;
        for _ in 0..12 {
            if let Some(spot) = self.input_spot_in(label, true) {
                let (x, y) = spot.center();
                if x >= viewport.x
                    && x <= viewport.x + viewport.w
                    && y >= viewport.y
                    && y <= viewport.y + viewport.h
                {
                    return Ok(Some(spot));
                }
            }
            self.scroll_inspector(-250.0)?;
        }
        Ok(None)
    }
}
