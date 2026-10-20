//! Film the actual Compare pop-out as a second captured window.

use crate::{
    director::Film,
    pacing, routes,
    support::{WindowView, app::pump},
    targets::{self, Spot},
};
use indicatrix_cut::{CompareModel, CompareWindow, RetargetModel, harness_support};
use slint::{
    ComponentHandle, LogicalPosition, Model,
    platform::{PointerEventButton, WindowEvent},
};
use std::time::Duration;
use uiprobe::screenshot;

impl Film<'_> {
    /// The pointer press/release is the sole activation; the recorder supplies the
    /// visible cursor and ripple while the overlay is attached.
    fn click_compare(
        &mut self,
        window: &slint::Window,
        origin: (f32, f32),
        spot: &Spot,
    ) -> Result<(), String> {
        let (x, y) = spot.center();
        self.glide_to((origin.0 + x, origin.1 + y))?;
        self.rec.hold(pacing::BEFORE_CLICK)?;
        self.rec.click()?;
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
        pump(60);
        Ok(())
    }

    pub(crate) fn film_compare_table(&mut self, visual: bool) -> Result<(), String> {
        let button = targets::resolve(&self.view(), "compare_button");
        let targets::Resolved::Found(button) = button else {
            return Err("Compare Snapshot button is missing".into());
        };
        self.click_and_invoke(&button)?;
        let table = self.ui.global::<RetargetModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            table.get_compare_open() && table.get_compare_rows().row_count() > 0
        }) {
            return Err("Compare Snapshot did not show its populated tier table".into());
        }
        self.rec.hold(pacing::RESULT_HOLD)?;
        if !visual {
            let close = targets::run_label(&self.view(), "Close")
                .ok_or("snapshot comparison table Close button is missing")?;
            self.click_and_invoke(&close)?;
            if table.get_compare_open() {
                return Err("snapshot table Close click did not close it".into());
            }
            return Ok(());
        }
        let visual = targets::run_label_start(&self.view(), "Compare Visually")
            .ok_or("Compare Visually button is missing from the snapshot table")?;
        self.click_and_invoke(&visual)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            harness_support::compare_window()
                .is_some_and(|window| window.global::<CompareModel>().get_is_open())
        }) {
            return Err("Compare click did not open a live comparison window".into());
        }
        let compare = harness_support::compare_window().ok_or("Compare window disappeared")?;
        screenshot::set_window_size(compare.window(), (1100, 780), crate::config::SCALE);
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            let model = compare.global::<CompareModel>();
            !model.get_before_label().is_empty()
                && !model.get_after_label().is_empty()
                && model.get_before_image().size().width > 0
                && model.get_after_image().size().width > 0
        }) {
            return Err("Compare window did not render both labelled images".into());
        }
        let origin = self.rec.place_window(compare.window());
        self.rec.attach_window(&compare, origin);
        self.scan_compare_text(&compare)?;
        self.rec.chapter("Compare Snapshot");
        self.rec
            .caption("Compare the two designs in the separate window.");
        self.rec.hold(pacing::RESULT_HOLD)?;
        Ok(())
    }

    pub(crate) fn film_compare_read_close(&mut self) -> Result<(), String> {
        let compare = harness_support::compare_window()
            .ok_or("Compare window is missing on its reading step")?;
        let origin = self
            .rec
            .attached_origin()
            .ok_or("Compare window is not attached to the recording")?;
        if !compare.global::<CompareModel>().get_is_open() {
            return Err("Compare window has no live comparison".into());
        }
        self.compare_pick_controls(&compare, origin)?;
        self.compare_drag_divider(&compare, origin)?;
        self.compare_pan_and_reset(&compare, origin)?;
        self.compare_close(&compare, origin)
    }

    /// Clicks each mode and each renderer control of the Compare window and reads the choice back.
    fn compare_pick_controls(
        &mut self,
        compare: &CompareWindow,
        origin: (f32, f32),
    ) -> Result<(), String> {
        let model = compare.global::<CompareModel>();
        for (label, index) in [("Overlay", 2), ("Split Slider", 1), ("Side by Side", 0)] {
            let view = WindowView::of("CompareWindow", compare);
            let spot = targets::run_label(&view, label)
                .ok_or_else(|| format!("Compare window control {label:?} is missing"))?;
            self.click_compare(compare.window(), origin, &spot)?;
            if model.get_mode_index() != index {
                return Err(format!(
                    "Compare {label:?} click did not select mode {index}"
                ));
            }
            self.rec.hold(Duration::from_millis(450))?;
            self.scan_compare_text(compare)?;
        }
        for (label, index) in [("Path Traced", 1), ("Ray Traced", 2), ("Solid", 0)] {
            let spot = targets::run_label(&WindowView::of("CompareWindow", compare), label)
                .ok_or_else(|| format!("Compare renderer control {label:?} is missing"))?;
            self.click_compare(compare.window(), origin, &spot)?;
            if model.get_renderer_index() != index {
                return Err(format!("Compare renderer {label:?} did not select {index}"));
            }
            self.rec.hold(Duration::from_millis(600))?;
            self.scan_compare_text(compare)?;
        }
        Ok(())
    }

    /// Presses at `start`, glides the cursor to `end` and releases there (both are window-local
    /// positions in the Compare window; `origin` is where that window sits in the video). The
    /// release is sent even when the glide fails, and the glide's error is returned after it.
    fn compare_drag(
        &mut self,
        window: &slint::Window,
        origin: (f32, f32),
        start: LogicalPosition,
        end: LogicalPosition,
    ) -> Result<(), String> {
        window.dispatch_event(WindowEvent::PointerMoved { position: start });
        window.dispatch_event(WindowEvent::PointerPressed {
            position: start,
            button: PointerEventButton::Left,
        });
        let moved = self.glide_to((origin.0 + end.x, origin.1 + end.y));
        window.dispatch_event(WindowEvent::PointerMoved { position: end });
        window.dispatch_event(WindowEvent::PointerReleased {
            position: end,
            button: PointerEventButton::Left,
        });
        moved
    }

    /// Switches to the split view and drags its divider sideways.
    fn compare_drag_divider(
        &mut self,
        compare: &CompareWindow,
        origin: (f32, f32),
    ) -> Result<(), String> {
        let model = compare.global::<CompareModel>();
        let split = targets::run_label(&WindowView::of("CompareWindow", compare), "Split Slider")
            .ok_or("Compare Split Slider control is missing")?;
        self.click_compare(compare.window(), origin, &split)?;
        let divider =
            targets::run_label(&WindowView::of("CompareWindow", compare), "Split Divider")
                .ok_or("Compare Split Divider is missing")?;
        let (x, y) = divider.center();
        let before = model.get_split_fraction();
        self.glide_to((origin.0 + x, origin.1 + y))?;
        self.compare_drag(
            compare.window(),
            origin,
            LogicalPosition::new(x, y),
            LogicalPosition::new(x + 70.0, y),
        )?;
        pump(60);
        if (model.get_split_fraction() - before).abs() < f32::EPSILON {
            return Err("Compare split divider drag did not change its position".into());
        }
        self.rec.hold(Duration::from_millis(450))
    }

    /// Zooms the comparison picture with the wheel, pans it by a drag and resets it with two clicks.
    fn compare_pan_and_reset(
        &mut self,
        compare: &CompareWindow,
        origin: (f32, f32),
    ) -> Result<(), String> {
        let picture = targets::run_label(
            &WindowView::of("CompareWindow", compare),
            "Comparison of the Two Stones",
        )
        .ok_or("Compare image area is missing")?;
        let (x, y) = picture.center();
        self.glide_to((origin.0 + x, origin.1 + y))?;
        compare
            .window()
            .dispatch_event(WindowEvent::PointerScrolled {
                position: LogicalPosition::new(x, y),
                delta_x: 0.0,
                delta_y: -120.0,
            });
        self.rec.hold(Duration::from_millis(450))?;
        self.compare_drag(
            compare.window(),
            origin,
            LogicalPosition::new(x, y),
            LogicalPosition::new(x + 48.0, y + 22.0),
        )?;
        self.rec.hold(Duration::from_millis(450))?;
        let reset = Spot {
            x: x - 2.0,
            y: y - 2.0,
            w: 4.0,
            h: 4.0,
            handle: None,
        };
        self.click_compare(compare.window(), origin, &reset)?;
        self.click_compare(compare.window(), origin, &reset)?;
        self.rec.hold(Duration::from_millis(450))?;
        self.scan_compare_text(compare)
    }

    /// Closes the Compare window, then the snapshot table behind it when that is still open.
    fn compare_close(&mut self, compare: &CompareWindow, origin: (f32, f32)) -> Result<(), String> {
        let close = targets::run_label(&WindowView::of("CompareWindow", compare), "Close")
            .ok_or("Compare window Close button is missing")?;
        self.click_compare(compare.window(), origin, &close)?;
        if harness_support::compare_window().is_some()
            || compare.global::<CompareModel>().get_is_open()
        {
            return Err("Compare Close click left the window open".into());
        }
        self.rec.detach_window();
        let table = self.ui.global::<RetargetModel>();
        if table.get_compare_open() {
            let close = targets::run_label(&self.view(), "Close")
                .ok_or("snapshot comparison table Close button is missing")?;
            self.click_and_invoke(&close)?;
            if table.get_compare_open() {
                return Err("snapshot table Close click did not close it".into());
            }
        }
        self.rec.caption("");
        Ok(())
    }

    fn scan_compare_text(&self, compare: &CompareWindow) -> Result<(), String> {
        for text in routes::visible_texts(&WindowView::of("CompareWindow", compare)) {
            if let Some(word) = self.cfg.private_hit(&text) {
                return Err(format!(
                    "private text in Compare window ({word:?}): {text:?}"
                ));
            }
        }
        Ok(())
    }
}
