//! The shot context and the driving helpers every shot shares: launching the app on the
//! library copy, waiting for the live render, finding elements, cropping, the privacy scan.

use crate::{
    config::{Config, MISSING_LIBRARY},
    support::{
        Ctx, ElementHandle, WindowView,
        app::{pump, wait_until},
        library_source::relaunch_with_database,
        query::is_clickable_role,
    },
};
use indicatrix_cut::{
    ActivityModel, BatchModel, LibraryModel, MainWindow, SettingsModel, ViewportModel,
};
use slint::{
    ComponentHandle, LogicalPosition, Model,
    platform::{PointerEventButton, WindowEvent},
};
use slint_harness::screenshot::{self, Shot};
use std::time::{Duration, Instant};

/// The main window's logical size in every shot.
pub const MAIN_SIZE: (u32, u32) = (1920, 1152);
/// Physical pixels per logical pixel: the shots are `HiDPI`, like the ones they replace.
pub const SCALE: f32 = 2.0;
/// A live render that was seen running is photographed no earlier than this after the wait began.
const MIN_SETTLE: Duration = Duration::from_millis(1500);
/// A live render that was never seen running (it converged within one frame) is photographed
/// after this long.
const NO_START_GRACE: Duration = Duration::from_secs(6);
/// How long the library copy may take to list its first design.
const LIBRARY_LOAD_SECS: u64 = 120;

/// A rectangle in logical window pixels.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// The rectangle `element` covers in its window.
    #[must_use]
    pub fn of(element: &ElementHandle) -> Self {
        let position = element.absolute_position();
        let size = element.size();
        Self {
            x: position.x,
            y: position.y,
            w: size.width,
            h: size.height,
        }
    }

    /// The right edge.
    #[must_use]
    pub const fn right(self) -> f32 {
        self.x + self.w
    }

    /// The bottom edge.
    #[must_use]
    pub const fn bottom(self) -> f32 {
        self.y + self.h
    }

    /// The centre.
    #[must_use]
    pub const fn center(self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// The smallest rectangle holding both.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self {
            x,
            y,
            w: self.right().max(other.right()) - x,
            h: self.bottom().max(other.bottom()) - y,
        }
    }

    /// `margin` more on every side.
    #[must_use]
    pub const fn grow(self, margin: f32) -> Self {
        Self {
            x: self.x - margin,
            y: self.y - margin,
            w: self.w + 2.0 * margin,
            h: self.h + 2.0 * margin,
        }
    }

    /// Whether `other` lies inside this rectangle (1 pixel of slack).
    #[must_use]
    pub const fn holds(self, other: Self) -> bool {
        other.x >= self.x - 1.0
            && other.y >= self.y - 1.0
            && other.right() <= self.right() + 1.0
            && other.bottom() <= self.bottom() + 1.0
    }

    /// The part of `shot` this rectangle covers.
    #[must_use]
    pub fn crop(self, shot: &Shot) -> Shot {
        shot.crop_logical(self.x, self.y, self.w, self.h)
    }
}

/// The first element below `view`'s root that is, or inherits, the component `name`.
#[must_use]
pub fn find_inherits(view: &WindowView, name: &str) -> Option<ElementHandle> {
    view.root
        .query_descendants()
        .match_inherits(name)
        .find_first()
}

/// The rectangle of a component, or an error naming it.
///
/// # Errors
///
/// The component is not on screen.
pub fn rect_of(view: &WindowView, component: &str) -> Result<Rect, String> {
    find_inherits(view, component)
        .map(|element| Rect::of(&element))
        .ok_or_else(|| format!("the component {component} is not on screen"))
}

/// The card of a modal dialog: the largest element below the dialog's root that lies inside it
/// and is smaller than it in both directions (the root is the dimmed backdrop over the window).
/// Falls back to the root's own rectangle.
///
/// # Errors
///
/// The dialog is not on screen.
pub fn card_rect(view: &WindowView, component: &str) -> Result<Rect, String> {
    let root = find_inherits(view, component)
        .ok_or_else(|| format!("the dialog {component} is not on screen"))?;
    let outer = Rect::of(&root);
    let card = root
        .query_descendants()
        .find_all()
        .iter()
        .map(Rect::of)
        .filter(|r| {
            r.w > 40.0
                && r.h > 40.0
                && r.w < outer.w - 1.0
                && r.h < outer.h - 1.0
                && outer.holds(*r)
        })
        .max_by(|a, b| (a.w * a.h).total_cmp(&(b.w * b.h)));
    Ok(card.unwrap_or(outer))
}

/// Whether the element is a control a click activates (a button, tab, radio button ...).
#[must_use]
pub fn clickable(element: &ElementHandle) -> bool {
    element.accessible_role().is_some_and(is_clickable_role)
}

/// Activates the first clickable element labelled `label`; whether there was one.
#[must_use]
pub fn click_first(view: &WindowView, label: &str) -> bool {
    view.by_label(label)
        .into_iter()
        .find(clickable)
        .is_some_and(|element| {
            element.invoke_accessible_default_action();
            true
        })
}

/// Activates the last clickable element labelled `label` (an overlay comes after the window's
/// own content); whether there was one.
#[must_use]
pub fn click_last(view: &WindowView, label: &str) -> bool {
    view.by_label(label)
        .into_iter()
        .rev()
        .find(clickable)
        .is_some_and(|element| {
            element.invoke_accessible_default_action();
            true
        })
}

/// Moves the pointer to the middle of `element`, so its hover state shows.
pub fn hover_over(window: &slint::Window, element: &ElementHandle) {
    let (x, y) = Rect::of(element).center();
    window.dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(x, y),
    });
    pump(120);
}

/// Presses and releases the left button in the middle of `element`.
pub fn press_on(window: &slint::Window, element: &ElementHandle) {
    let (x, y) = Rect::of(element).center();
    let position = LogicalPosition::new(x, y);
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    pump(120);
}

/// Resizes a window to `logical` pixels at [`SCALE`] and lets the layout follow.
pub fn size_window<C: ComponentHandle>(component: &C, logical: (u32, u32)) {
    screenshot::set_window_size(component.window(), logical, SCALE);
    pump(300);
}

/// Whether a "Path tracing" activity is running in `ui`.
#[must_use]
pub fn trace_running(ui: &MainWindow) -> bool {
    ui.global::<ActivityModel>()
        .get_activities()
        .iter()
        .any(|activity| activity.kind.as_str() == "trace")
}

/// Every label, placeholder text and field value in `view`.
fn visible_texts(view: &WindowView) -> Vec<String> {
    let mut texts = Vec::new();
    for element in view.elements() {
        for text in [
            element.accessible_label(),
            element.accessible_placeholder_text(),
            element.accessible_value(),
        ]
        .into_iter()
        .flatten()
        {
            let text = text.to_string();
            if !text.trim().is_empty() {
                texts.push(text);
            }
        }
    }
    texts
}

/// What a shot needs: the app (on the library copy) and the run's configuration.
pub struct Sc<'a, 'b> {
    /// The running app, the profile and the report.
    pub ctx: &'a mut Ctx<'b>,
    /// The run's configuration.
    pub cfg: &'a Config,
}

impl Sc<'_, '_> {
    /// A strong handle on the main window.
    #[must_use]
    pub fn ui(&self) -> MainWindow {
        self.ctx.app.ui().clone_strong()
    }

    /// Records a line in the report that is not a finding.
    pub fn note(&mut self, text: impl Into<String>) {
        self.ctx.report.note(text);
    }

    /// Pumps until `condition` holds or `secs` of real time pass.
    pub fn wait(secs: u64, condition: impl FnMut() -> bool) -> bool {
        wait_until(Duration::from_secs(secs), condition)
    }

    /// Relaunches the app on a copy of the library, sizes the window for the shots, waits for
    /// the list, declines the start-up batch offers and sets the sample count.
    ///
    /// # Errors
    ///
    /// No library is configured, the app does not start, or the list stays empty.
    pub fn prepare(&mut self) -> Result<(), String> {
        let library = self
            .cfg
            .library
            .clone()
            .ok_or_else(|| MISSING_LIBRARY.to_owned())?;
        relaunch_with_database(self.ctx, &library)?;
        size_window(self.ctx.app.ui(), MAIN_SIZE);
        let ui = self.ui();
        let listed = Self::wait(LIBRARY_LOAD_SECS, || {
            ui.global::<LibraryModel>().get_diagram_list().row_count() > 0
        });
        if !listed {
            return Err(format!(
                "the library copy lists no design after {LIBRARY_LOAD_SECS} s"
            ));
        }
        let batch = ui.global::<BatchModel>();
        let _ = Self::wait(2, || {
            batch.get_preview_visible() || batch.get_tilt_visible()
        });
        for _ in 0..3 {
            if batch.get_preview_visible() {
                batch.invoke_preview_dismiss_offer();
            }
            if batch.get_tilt_visible() {
                batch.invoke_tilt_dismiss_offer();
            }
            pump(200);
        }
        self.apply_samples();
        self.apply_resolution();
        pump(300);
        self.ctx.app.clear_notice();
        Ok(())
    }

    /// Renders the live view at the largest resolution pill (1920 x 1080) instead of the
    /// default 800 x 600: the shots are taken at [`SCALE`], where an 800 x 600 trace is
    /// stretched more than threefold and its noise shows as blocks.
    fn apply_resolution(&self) {
        let ui = self.ui();
        let settings = ui.global::<SettingsModel>();
        if settings.get_resolution_index() != 3 {
            settings.set_resolution_index(3);
            settings.invoke_resolution_changed(1920, 1080);
        }
    }

    /// Makes the live render's target the configured sample count (a power of two from 8 to
    /// 1024, the slider's range).
    fn apply_samples(&self) {
        let ui = self.ui();
        let exponent = self.cfg.spp.clamp(8, 1024).ilog2();
        let wanted = f32::from(u8::try_from(exponent).unwrap_or(8));
        let settings = ui.global::<SettingsModel>();
        if (settings.get_target_samples_exponent() - wanted).abs() > 0.4 {
            settings.set_target_samples_exponent(wanted);
            settings.invoke_target_samples_changed(i32::from(u8::try_from(exponent).unwrap_or(8)));
        }
    }

    /// Waits (real time) until the path-traced live view has reached its target sample count,
    /// or the configured timeout passes; a timeout is a note, not a finding. Returns whether
    /// the render settled.
    pub fn wait_live(&mut self) -> bool {
        let ui = self.ui();
        let start = Instant::now();
        let mut seen_running = false;
        let settled = wait_until(self.cfg.render_timeout, || {
            let running = trace_running(&ui);
            seen_running |= running;
            let age = start.elapsed();
            ui.global::<ViewportModel>().get_has_render()
                && !running
                && ((seen_running && age >= MIN_SETTLE) || age >= NO_START_GRACE)
        });
        if !settled {
            let seconds = self.cfg.render_timeout.as_secs();
            let spp = self.cfg.spp;
            self.note(format!(
                "the live render did not reach {spp} samples within {seconds} s; photographed anyway"
            ));
        }
        pump(300);
        settled
    }

    /// Refuses a shot that would show private text.
    ///
    /// # Errors
    ///
    /// `view` shows the user's name or part of a profile path.
    pub fn privacy(&self, view: &WindowView) -> Result<(), String> {
        for text in visible_texts(view) {
            if let Some(word) = self.cfg.private_hit(&text) {
                return Err(format!(
                    "{} shows private text ({word:?}): {text:?}",
                    view.name
                ));
            }
        }
        Ok(())
    }

    /// Hides the toast, checks the privacy rule and captures the main window.
    ///
    /// # Errors
    ///
    /// Private text is visible, or the capture fails.
    pub fn snap_main(&self) -> Result<Shot, String> {
        self.ctx.app.clear_notice();
        pump(150);
        let view = self.ctx.app.main_view();
        self.privacy(&view)?;
        screenshot::capture(self.ctx.app.ui().window())
    }

    /// Checks the privacy rule and captures a secondary window.
    ///
    /// # Errors
    ///
    /// Private text is visible, or the capture fails.
    pub fn snap_window<C: ComponentHandle>(
        &self,
        name: &str,
        component: &C,
    ) -> Result<Shot, String> {
        pump(150);
        let view = WindowView::of(name, component);
        self.privacy(&view)?;
        screenshot::capture(component.window())
    }

    /// The main window as a queryable view.
    #[must_use]
    pub fn view(&self) -> WindowView {
        self.ctx.app.main_view()
    }
}
