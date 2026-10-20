//! Visible UI routes for the rendering, cutting, help, and rough-planning lessons.

use crate::{
    director::Film,
    pacing, routes,
    support::{WindowView, app::pump},
    targets,
};
use indicatrix_cut::{
    CuttingModeModel, DesignLightingModel, DispersionEditorModel, HelpModel, ParametersModel,
    RoughPlanModel, ShortcutsModel, SolidPreviewModel, ViewportModel, harness_support,
};
use slint::{
    ComponentHandle, LogicalPosition, Model, SharedString,
    platform::{PointerEventButton, WindowEvent},
};
use std::time::Duration;
use uiprobe::screenshot;

impl Film<'_> {
    pub(crate) fn attach_help_window(&mut self) -> Result<(), String> {
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            harness_support::help_window().is_some()
        }) {
            return Err("User Manual did not open the help window".into());
        }
        let help = harness_support::help_window().ok_or("Help window disappeared")?;
        screenshot::set_window_size(help.window(), (1100, 780), crate::config::SCALE);
        for text in routes::visible_texts(&WindowView::of("HelpWindow", &help)) {
            if let Some(word) = self.cfg.private_hit(&text) {
                return Err(format!("private text in Help window ({word:?}): {text:?}"));
            }
        }
        match self.attached_window {
            // The same manual window is already in the video (a second step opens it again).
            Some("help") => {}
            Some(other) => {
                return Err(format!(
                    "the {other} window is still attached to the video; the manual cannot be attached too"
                ));
            }
            None => {
                let origin = self.rec.place_window(help.window());
                self.rec.attach_window(&help, origin);
                self.attached_window = Some("help");
            }
        }
        Ok(())
    }
    fn extra_click(&mut self, label: &str) -> Result<(), String> {
        let spot = targets::run_label(&self.view(), label)
            .ok_or_else(|| format!("{label:?} is not visible"))?;
        self.click_and_invoke(&spot)?;
        Ok(())
    }

    fn extra_click_panel(&mut self, label: &str) -> Result<(), String> {
        let panel = self
            .view()
            .root
            .query_descendants()
            .match_inherits("ParametersPanel")
            .find_first()
            .ok_or("Parameters panel is missing")?;
        let scroll = panel
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .ok_or("Parameters panel scroll view is missing")?;
        let viewport = targets::Spot::of(&scroll);
        self.wheel_over(&viewport, 3000.0)?;
        for _ in 0..15 {
            let spot = targets::run_label(&self.view(), label);
            if let Some(spot) = spot {
                let (_, y) = spot.center();
                if y >= viewport.y && y <= viewport.y + viewport.h {
                    self.click_and_invoke(&spot)?;
                    return Ok(());
                }
            }
            self.wheel_over(&viewport, -250.0)?;
            self.hold(Duration::from_millis(120))?;
        }
        Err(format!(
            "Parameters control {label:?} is not visible after scrolling"
        ))
    }

    fn extra_menu(&mut self, menu: &str, item: &'static str) -> Result<(), String> {
        // These commands also have in-window palette rows. Native popup item
        // coordinates are relative to the popup, not the main recording window.
        match (menu, item) {
            ("Help", "Keyboard Shortcuts") => return self.film_command("help.shortcuts"),
            ("Help", "User Manual") => return self.film_command("help.manual"),
            ("Library", "Plan Rough...") => return self.film_command("tools.plan_rough"),
            ("Help", "Glossary") => {}
            _ => return Err(format!("no visible menu route for {menu} > {item}")),
        }
        // Glossary has no palette entry. Help's menu in app.slint starts with
        // User Manual, Help for This Screen (F1), Glossary. Slint PopupMenuImpl
        // opens at highlight -1, and Down visibly advances one entry at a time.
        self.extra_click(menu)?;
        self.hold(pacing::MENU_HOLD)?;
        for title in ["User Manual", "Help for This Screen (F1)", "Glossary"] {
            if targets::run_label(&self.view(), title).is_none() {
                return Err(format!("Help menu entry {title:?} is missing"));
            }
            uiprobe::view::press(self.ui.window(), "Down")
                .map_err(|e| format!("Help menu Down: {e:?}"))?;
            self.hold(Duration::from_millis(180))?;
        }
        uiprobe::view::press(self.ui.window(), "Return")
            .map_err(|e| format!("Help menu Return: {e:?}"))?;
        pump(60);
        Ok(())
    }

    fn extra_combo_next(&mut self, label: &str, current: i32, count: usize) -> Result<(), String> {
        if count < 2 {
            return Err(format!("{label} has no alternate choice"));
        }
        self.extra_click(label)?;
        let key = if i32::try_from(count).is_ok_and(|total| current + 1 < total) {
            "Down"
        } else {
            "Up"
        };
        uiprobe::view::press(self.ui.window(), key).map_err(|e| format!("{label} {key}: {e:?}"))?;
        uiprobe::view::press(self.ui.window(), "Return")
            .map_err(|e| format!("{label} Return: {e:?}"))?;
        Ok(())
    }

    pub(crate) fn drive_extra_event(&mut self, n: usize, name: &str) -> Result<(), String> {
        match name {
            "lighting_preset_chosen" => {
                let model = self.ui.global::<ViewportModel>();
                let before = model.get_selected_lighting_index();
                self.extra_combo_next(
                    "Lighting",
                    before,
                    model.get_lighting_options().row_count(),
                )?;
                if model.get_selected_lighting_index() == before {
                    return Err("Lighting combo did not select a new rig".into());
                }
            }
            "design_lighting_saved" | "design_lighting_forgotten" => {
                let model = self.ui.global::<DesignLightingModel>();
                let panel = self.ui.global::<ParametersModel>();
                if !panel.get_open() || panel.get_active_tab() != 1 {
                    self.extra_click("Lighting and Environment")?;
                }
                if !panel.get_open() || panel.get_active_tab() != 1 {
                    return Err("Lighting and Environment panel did not open".into());
                }
                let expected = name == "design_lighting_saved";
                let label = if expected {
                    "Use This Lighting for This Design"
                } else {
                    "Forget for This Design"
                };
                self.extra_click_panel(label)?;
                if model.get_has_saved() != expected {
                    return Err(format!("{label} did not change the design lighting state"));
                }
            }
            "custom_material_saved" | "coefficient_material_saved" => {
                self.film_material_save(name == "coefficient_material_saved")?;
            }
            "cutting_step_marked" => {
                if !self.ui.global::<CuttingModeModel>().get_open() {
                    let spot = targets::resolve(&self.view(), "cutting_mode_button");
                    let targets::Resolved::Found(spot) = spot else {
                        return Err("Cut Mode button is missing".into());
                    };
                    self.click_and_invoke(&spot)?;
                }
                let model = self.ui.global::<CuttingModeModel>();
                if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
                    model.get_open() && !model.get_busy()
                }) {
                    return Err("Cutting Mode did not become ready".into());
                }
                if model.get_step_state() == 1 {
                    return Err("cutting step is already marked done".into());
                }
                self.extra_click("Mark Step Done")?;
                if model.get_step_state() != 1 || model.get_done_count() < 1 {
                    return Err("Mark Step Done did not update cutting progress".into());
                }
            }
            "cut_slider_rough" | "cut_slider_step" | "cut_slider_finished" => {
                self.film_cut_slider(name)?;
            }
            "rough_planner_opened" => self.film_rough_open()?,
            "rough_plan_finished" => self.film_rough_finish()?,
            "glossary_opened" => {
                self.extra_menu("Help", "Glossary")?;
                if !self.ui.global::<HelpModel>().get_glossary_open() {
                    return Err("Help > Glossary did not open the glossary".into());
                }
                let field = targets::text_input(&self.view(), "Search the Terms")
                    .ok_or("glossary search field is missing")?;
                self.click_and_type(&field, "mast")?;
                if self.ui.global::<HelpModel>().get_glossary_query() != "mast" {
                    return Err("glossary search did not read back mast".into());
                }
                self.hold(pacing::RESULT_HOLD)?;
                uiprobe::view::press(self.ui.window(), "Escape")
                    .map_err(|e| format!("close glossary: {e:?}"))?;
            }
            "shortcuts_opened" => {
                self.extra_menu("Help", "Keyboard Shortcuts")?;
                if !self.ui.global::<ShortcutsModel>().get_open() {
                    return Err("Help > Keyboard Shortcuts did not open".into());
                }
                self.hold(pacing::RESULT_HOLD)?;
                uiprobe::view::press(self.ui.window(), "Escape")
                    .map_err(|e| format!("close shortcuts: {e:?}"))?;
            }
            "help_opened" => {
                if n == 1 {
                    self.extra_menu("Help", "User Manual")?;
                } else {
                    uiprobe::view::press(self.ui.window(), "F1")
                        .map_err(|e| format!("context help F1: {e:?}"))?;
                }
                self.attach_help_window()?;
            }
            _ => return Err(format!("unrecognized extra UI event {name:?}")),
        }
        Ok(())
    }

    fn film_material_save(&mut self, coefficients: bool) -> Result<(), String> {
        let panel = self.ui.global::<ParametersModel>();
        if !panel.get_open() || panel.get_active_tab() != 0 {
            self.extra_click("Stone Properties")?;
        }
        if !panel.get_open() || panel.get_active_tab() != 0 {
            return Err("Stone panel did not open".into());
        }
        self.extra_click_panel("New/Edit Material...")?;
        let model = self.ui.global::<ViewportModel>();
        if !model.get_material_editor_open() {
            return Err("Material Editor did not open".into());
        }
        if coefficients {
            self.extra_click("Coefficients")?;
            let combo = targets::run_label(&self.view(), "Dispersion Model")
                .ok_or("Dispersion Model combo is missing")?;
            self.click_and_invoke(&combo)?;
            uiprobe::view::press(self.ui.window(), "Up")
                .map_err(|e| format!("Sellmeier choice: {e:?}"))?;
            uiprobe::view::press(self.ui.window(), "Return")
                .map_err(|e| format!("Sellmeier Return: {e:?}"))?;
            let options = self
                .ui
                .global::<DispersionEditorModel>()
                .invoke_copy_options(1);
            let glass = options
                .iter()
                .position(|name| name.as_str() == "Glass (N-BK7)")
                .ok_or("Glass (N-BK7) is not offered for Sellmeier, 3 Terms")?;
            let copy =
                targets::run_label(&self.view(), "Copy Coefficients from a Built-In Material")
                    .ok_or("coefficient built-in copy combo is missing")?;
            self.click_and_invoke(&copy)?;
            for _ in 0..glass {
                uiprobe::view::press(self.ui.window(), "Down")
                    .map_err(|e| format!("copy glass choice: {e:?}"))?;
            }
            uiprobe::view::press(self.ui.window(), "Return")
                .map_err(|e| format!("copy glass Return: {e:?}"))?;
            let coefficient = targets::text_input_start(&self.view(), "Coefficient B1")
                .ok_or("Sellmeier B1 coefficient field is missing")?;
            let old = coefficient
                .handle
                .as_ref()
                .and_then(uiprobe::ElementHandle::accessible_value)
                .ok_or("Sellmeier B1 has no readable value")?;
            let old_number: f64 = old
                .parse()
                .map_err(|_| format!("invalid copied B1 value {old:?}"))?;
            self.click_and_type(&coefficient, &format!("{:.7}", old_number + 0.001))?;
        }
        let name = if coefficients {
            "Tutorial coefficients"
        } else {
            "Tutorial glass"
        };
        let field = targets::text_input(&self.view(), "Material Name")
            .ok_or("Material Name field is missing")?;
        self.click_and_type(&field, name)?;
        self.extra_click("Save")?;
        if model.get_material_editor_open() {
            return Err("Material Save did not close the editor".into());
        }
        Ok(())
    }

    fn film_cut_slider(&mut self, name: &str) -> Result<(), String> {
        let model = self.ui.global::<SolidPreviewModel>();
        let steps = model.get_cut_step_count();
        if steps < 2 {
            return Err(format!("Cut Step has only {steps} steps"));
        }
        let slider =
            targets::run_label(&self.view(), "Cut Step").ok_or("Cut Step slider is missing")?;
        let (x, y) = slider.center();
        let width = slider.w;
        let ratio = match name {
            "cut_slider_rough" => 0.04,
            "cut_slider_step" => 1.0 / steps as f32,
            _ => 0.96,
        };
        let end = LogicalPosition::new(width.mul_add(ratio, width.mul_add(-0.5, x)), y);
        self.glide_to((end.x, end.y))?;
        let window = self.ui.window();
        window.dispatch_event(WindowEvent::PointerMoved { position: end });
        window.dispatch_event(WindowEvent::PointerPressed {
            position: end,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position: end,
            button: PointerEventButton::Left,
        });
        self.rec.click()?;
        pump(60);
        let actual = model.get_tier_cutoff();
        let okay = match name {
            "cut_slider_rough" => actual == -2,
            "cut_slider_step" => actual >= 0,
            _ => actual == -1,
        };
        if !okay {
            return Err(format!(
                "Cut Step pointer input produced cutoff {actual}, expected {name}"
            ));
        }
        Ok(())
    }

    fn film_rough_open(&mut self) -> Result<(), String> {
        self.extra_menu("Library", "Plan Rough...")?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            harness_support::rough_planner_window().is_some()
        }) {
            return Err("Library > Plan Rough did not open its window".into());
        }
        let rough = harness_support::rough_planner_window().ok_or("Rough Planner disappeared")?;
        screenshot::set_window_size(rough.window(), (1180, 780), crate::config::SCALE);
        for text in routes::visible_texts(&WindowView::of("RoughPlannerWindow", &rough)) {
            if let Some(word) = self.cfg.private_hit(&text) {
                return Err(format!(
                    "private text in Rough Planner ({word:?}): {text:?}"
                ));
            }
        }
        if let Some(other) = self.attached_window {
            return Err(format!(
                "the {other} window is still attached to the video; the Rough Planner cannot be attached too"
            ));
        }
        let origin = self.rec.place_window(rough.window());
        self.rec.attach_window(&rough, origin);
        self.attached_window = Some("rough planner");
        self.rec.chapter("Rough Planner");
        self.rec.caption("Model the rough and plan a layout.");
        self.hold(pacing::RESULT_HOLD)?;
        Ok(())
    }

    fn film_rough_finish(&mut self) -> Result<(), String> {
        let rough =
            harness_support::rough_planner_window().ok_or("Rough Planner window is missing")?;
        let origin = self
            .rec
            .attached_origin()
            .ok_or("Rough Planner is not attached to video")?;
        let model = rough.global::<RoughPlanModel>();
        for (index, label, text) in [
            (0, "Rough X (mm)", "18.4"),
            (1, "Rough Y (mm)", "11.0"),
            (2, "Rough Z (mm)", "9.6"),
        ] {
            let field = targets::run_label(&WindowView::of("RoughPlannerWindow", &rough), label)
                .ok_or_else(|| format!("Rough Planner field {label:?} is missing"))?;
            self.type_in_secondary(rough.window(), origin, &field, text)?;
            let actual = match index {
                0 => model.get_rough_x(),
                1 => model.get_rough_y(),
                _ => model.get_rough_z(),
            };
            if actual.as_str() != text {
                return Err(format!("{label} readback mismatch: {actual:?}"));
            }
        }
        let plan = targets::run_label(&WindowView::of("RoughPlannerWindow", &rough), "Plan")
            .ok_or("Rough Planner Plan button is missing")?;
        self.click_secondary(rough.window(), origin, &plan)?;
        if !pacing::wait_lapse(&mut self.rec, Duration::from_secs(180), || {
            !model.get_running()
                && (model.get_results().row_count() > 0 || !model.get_error_text().is_empty())
        })? {
            return Err("Rough Planner did not finish within 180 seconds".into());
        }
        if model.get_results().row_count() == 0 {
            return Err(format!(
                "Rough Planner returned no layouts: {}",
                model.get_error_text()
            ));
        }
        for text in routes::visible_texts(&WindowView::of("RoughPlannerWindow", &rough)) {
            if let Some(word) = self.cfg.private_hit(&text) {
                return Err(format!(
                    "private text in Rough Planner results ({word:?}): {text:?}"
                ));
            }
        }
        self.hold(pacing::RESULT_HOLD)?;
        Ok(())
    }

    fn click_secondary(
        &mut self,
        window: &slint::Window,
        origin: (f32, f32),
        spot: &targets::Spot,
    ) -> Result<(), String> {
        let (x, y) = spot.center();
        self.glide_to((origin.0 + x, origin.1 + y))?;
        self.rec.click()?;
        let p = LogicalPosition::new(x, y);
        window.dispatch_event(WindowEvent::PointerMoved { position: p });
        window.dispatch_event(WindowEvent::PointerPressed {
            position: p,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position: p,
            button: PointerEventButton::Left,
        });
        pump(60);
        Ok(())
    }

    fn type_in_secondary(
        &mut self,
        window: &slint::Window,
        origin: (f32, f32),
        spot: &targets::Spot,
        text: &str,
    ) -> Result<(), String> {
        self.click_secondary(window, origin, spot)?;
        uiprobe::view::press(window, "Ctrl+A").map_err(|e| format!("rough field Ctrl+A: {e:?}"))?;
        uiprobe::view::press(window, "Backspace")
            .map_err(|e| format!("rough field Backspace: {e:?}"))?;
        for ch in text.chars() {
            let key: SharedString = ch.to_string().into();
            window.dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
            window.dispatch_event(WindowEvent::KeyReleased { text: key });
            pump(15);
            self.hold(Duration::from_millis(60))?;
        }
        uiprobe::view::press(window, "Return").map_err(|e| format!("rough field Return: {e:?}"))?;
        Ok(())
    }
}
