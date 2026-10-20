//! Staging for tutorial videos: places the app in the right view, library panel state,
//! inspector tab, and camera focus for each step.

use crate::{director::Film, pacing, support::app::pump, targets};
use indicatrix_cut::{
    EditorModel, GuideModel, LibraryModel, ManipulateModel, ParametersModel, SolidPreviewModel,
    ViewportModel, harness_support,
};
use indicatrix_editor::guide::{CameraFocus, LibraryPanel, Stage, StageView};
use slint::ComponentHandle;
use slint::{
    LogicalPosition,
    platform::{PointerEventButton, WindowEvent},
};
use std::time::Duration;

/// Mirrored on-screen state tracked by the director across steps.
#[derive(Clone, Debug)]
pub struct StageState {
    pub view: StageView,
    pub library: LibraryPanel,
    pub inspector_tab: Option<i32>,
    pub parameters_tab: Option<String>,
    pub solid_view_mode: Option<i32>,
    pub focus: CameraFocus,
    pub last_fit_radius: Option<f64>,
    pub view_switches: usize,
    pub library_toggles: usize,
    pub camera_moves: usize,
}

impl Default for StageState {
    fn default() -> Self {
        Self {
            view: StageView::LiveRender,
            library: LibraryPanel::Shown,
            inspector_tab: None,
            parameters_tab: None,
            solid_view_mode: None,
            focus: CameraFocus::Keep,
            last_fit_radius: None,
            view_switches: 0,
            library_toggles: 0,
            camera_moves: 0,
        }
    }
}

impl Film<'_> {
    /// Applies the wanted stage to the application, filming transitions if `filmed` is true.
    pub fn stage(&mut self, wanted: &Stage, filmed: bool) -> Result<(), String> {
        self.stage_view(wanted, filmed)?;
        self.stage_library(wanted, filmed)?;
        self.stage_editor_sections(wanted)?;
        self.stage_inspector_tab(wanted, filmed)?;
        self.stage_parameters_tab(wanted, filmed)?;
        self.stage_solid_view_mode(wanted, filmed)?;
        Ok(())
    }

    fn current_screen_view(&self) -> StageView {
        if self.ui.get_active_tab() == 0 {
            if self.ui.get_render_view_tab() == 1 {
                StageView::Edit
            } else {
                StageView::LiveRender
            }
        } else {
            StageView::Either
        }
    }

    fn stage_view(&mut self, wanted: &Stage, filmed: bool) -> Result<(), String> {
        let current = self.current_screen_view();
        self.stage_state.view = current;
        if wanted.view == StageView::Either || wanted.view == current {
            return Ok(());
        }

        let guide_model = self.ui.global::<GuideModel>();
        let allows_tabs = guide_model.invoke_allows_view_tabs();

        match wanted.view {
            StageView::Edit => {
                let pill = targets::edit_pill(&self.view())
                    .ok_or_else(|| "Edit view pill not found on screen".to_owned())?;
                self.click_and_invoke(&pill)?;
                pump(30);
                if self.current_screen_view() != StageView::Edit {
                    return Err("Edit pill did not switch views".into());
                }
                if filmed {
                    self.rec.hold(pacing::VIEW_SWITCH_HOLD)?;
                }
                self.stage_state.view = StageView::Edit;
                self.stage_state.view_switches += 1;
            }
            StageView::LiveRender => {
                if !allows_tabs {
                    return Err("guide lock forbids switching to Live Render".to_owned());
                }
                let pill = targets::live_render_pill(&self.view())
                    .ok_or_else(|| "Live Render view pill not found on screen".to_owned())?;
                self.click_and_invoke(&pill)?;
                pump(30);
                if self.current_screen_view() != StageView::LiveRender {
                    return Err("Live Render pill did not switch views".into());
                }
                if filmed {
                    self.rec.hold(pacing::VIEW_SWITCH_HOLD)?;
                }
                self.stage_state.view = StageView::LiveRender;
                self.stage_state.view_switches += 1;
            }
            StageView::Either => {}
        }

        Ok(())
    }

    fn stage_library(&mut self, wanted: &Stage, filmed: bool) -> Result<(), String> {
        let library_model = self.ui.global::<LibraryModel>();
        let collapsed = library_model.get_panel_collapsed();
        self.stage_state.library = if collapsed {
            LibraryPanel::Hidden
        } else {
            LibraryPanel::Shown
        };

        match wanted.library {
            LibraryPanel::Hidden if !collapsed => {
                let button = targets::hide_library_button(&self.view())
                    .ok_or("Hide Library button is missing")?;
                self.click_and_invoke(&button)?;
                pump(30);
                if !library_model.get_panel_collapsed() {
                    return Err("Hide Library click did not collapse panel".into());
                }
                if filmed {
                    self.rec.hold(pacing::STAGE_HOLD)?;
                }
                self.stage_state.library = LibraryPanel::Hidden;
                self.stage_state.library_toggles += 1;
            }
            LibraryPanel::Shown if collapsed => {
                let button = targets::show_library_button(&self.view())
                    .ok_or("Show Library button is missing")?;
                self.click_and_invoke(&button)?;
                pump(30);
                if library_model.get_panel_collapsed() {
                    return Err("Show Library click did not expand panel".into());
                }
                if filmed {
                    self.rec.hold(pacing::STAGE_HOLD)?;
                }
                self.stage_state.library = LibraryPanel::Shown;
                self.stage_state.library_toggles += 1;
            }
            _ => {}
        }

        Ok(())
    }

    fn stage_editor_sections(&mut self, wanted: &Stage) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        if wanted.expand_tier_table && editor.get_tier_table_collapsed() {
            let header = targets::run_label_start(&self.view(), "TIERS (")
                .ok_or("Tiers section header is missing")?;
            self.click_and_invoke(&header)?;
        }
        if wanted.expand_settings && editor.get_settings_collapsed() {
            let header = targets::run_label(&self.view(), "DESIGN")
                .ok_or("Design Settings header is missing")?;
            self.click_and_invoke(&header)?;
        }
        if editor.get_inspector_collapsed() {
            let header = targets::run_label(&self.view(), "INSPECTOR")
                .ok_or("Inspector section header is missing")?;
            self.click_and_invoke(&header)?;
        }
        pump(10);
        Ok(())
    }

    fn stage_inspector_tab(&mut self, wanted: &Stage, filmed: bool) -> Result<(), String> {
        let Some(t) = wanted.inspector_tab else {
            return Ok(());
        };

        let editor = self.ui.global::<EditorModel>();
        if editor.get_inspector_tab() == t {
            self.stage_state.inspector_tab = Some(t);
            return Ok(());
        }

        let pill =
            targets::inspector_tab_pill(&self.view(), t).ok_or("Inspector tab pill is missing")?;
        self.click_and_invoke(&pill)?;
        pump(30);
        if editor.get_inspector_tab() != t {
            return Err(format!("Inspector tab click did not select {t}"));
        }
        if filmed {
            self.rec.hold(pacing::STAGE_HOLD)?;
        }
        self.stage_state.inspector_tab = Some(t);
        Ok(())
    }

    fn stage_parameters_tab(&mut self, wanted: &Stage, filmed: bool) -> Result<(), String> {
        let Some(tab) = wanted.parameters_tab else {
            return Ok(());
        };

        let tab_idx: i32 = match tab {
            "lighting" => 1,
            "render_output" => 2,
            _ => 0,
        };

        let params = self.ui.global::<ParametersModel>();
        if params.get_open() && params.get_active_tab() == tab_idx {
            self.stage_state.parameters_tab = Some(tab.to_owned());
            return Ok(());
        }

        let button = targets::dock_tab_button(&self.view(), tab)
            .ok_or("Parameters dock tab button is missing")?;
        self.click_and_invoke(&button)?;
        pump(30);
        if !params.get_open() || params.get_active_tab() != tab_idx {
            return Err(format!("Parameters tab click did not select {tab:?}"));
        }
        if filmed {
            self.rec.hold(pacing::STAGE_HOLD)?;
        }
        self.stage_state.parameters_tab = Some(tab.to_owned());
        Ok(())
    }

    fn stage_solid_view_mode(&mut self, wanted: &Stage, filmed: bool) -> Result<(), String> {
        let Some(m) = wanted.solid_view_mode else {
            return Ok(());
        };

        let solid = self.ui.global::<SolidPreviewModel>();
        if solid.get_view_mode() == m {
            self.stage_state.solid_view_mode = Some(m);
            return Ok(());
        }

        let pill = targets::solid_view_mode_pill(&self.view(), m)
            .ok_or("solid view mode pill is missing")?;
        self.click_and_invoke(&pill)?;
        if solid.get_view_mode() != m {
            return Err(format!("solid view mode click did not select {m}"));
        }
        let ui = self.ui;
        let _ = pacing::wait_real(Duration::from_secs(2), || {
            let sp = ui.global::<SolidPreviewModel>();
            if m == 3 {
                sp.get_has_diagram()
            } else {
                sp.get_has_solid()
            }
        });
        pump(30);
        if filmed {
            self.rec.hold(pacing::STAGE_HOLD)?;
        }
        self.stage_state.solid_view_mode = Some(m);
        Ok(())
    }

    /// Why the camera must not be dragged right now, as a note key and a sentence; `None` when
    /// the drag is free. Diagram's drag pans its drawing. Solid/Path-Traced/Ray-Traced/Both in Edit orbit the
    /// shared camera, just like Live Render. Active manipulation owns this gesture, so the camera
    /// is left alone until the tool is finished.
    fn camera_gesture_blocked(&self, screen: StageView) -> Option<(&'static str, &'static str)> {
        let solid = self.ui.global::<SolidPreviewModel>();
        let manipulate = self.ui.global::<ManipulateModel>();
        if !solid.get_has_solid() {
            Some(("no-solid", "there is no solid preview"))
        } else if manipulate.get_dragging() {
            Some(("dragging", "a manipulation drag owns the camera gesture"))
        } else if manipulate.get_slice_mode() {
            Some(("slice-mode", "slice mode owns the camera gesture"))
        } else if screen == StageView::Edit && solid.get_view_mode() == 3 {
            Some(("diagram-in-edit", "the Diagram view drag pans its drawing"))
        } else {
            None
        }
    }

    /// The pointer surface that orbits the camera on `screen`.
    fn camera_surface(&self, screen: StageView) -> Result<targets::Spot, String> {
        if screen == StageView::Edit {
            targets::run(
                &self.view(),
                &targets::Query::Id("SolidViewportView::drag_area"),
            )
            .ok_or_else(|| "active SolidViewport drag area is missing for camera framing".into())
        } else {
            targets::run(
                &self.view(),
                &targets::Query::Id("GemViewportView::touch_area"),
            )
            .ok_or_else(|| "active GemViewport touch area is missing for camera framing".into())
        }
    }

    /// Drags the viewport around `center` by (`dx`, `dy`) logical pixels in twelve moves, the
    /// cursor following when `filmed`.
    fn drag_camera(
        &mut self,
        center: (f32, f32),
        (dx, dy): (f32, f32),
        filmed: bool,
    ) -> Result<(), String> {
        let start = LogicalPosition::new(center.0 - dx / 2.0, center.1 - dy / 2.0);
        let end = LogicalPosition::new(center.0 + dx / 2.0, center.1 + dy / 2.0);
        if filmed {
            self.glide_to((start.x, start.y))?;
        }
        self.ui
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position: start });
        self.ui
            .window()
            .dispatch_event(WindowEvent::PointerPressed {
                position: start,
                button: PointerEventButton::Left,
            });
        for step in 1..=12_u8 {
            let t = f32::from(step) / 12.0;
            let position = LogicalPosition::new(dx.mul_add(t, start.x), dy.mul_add(t, start.y));
            if filmed {
                self.glide_to((position.x, position.y))?;
            }
            self.ui
                .window()
                .dispatch_event(WindowEvent::PointerMoved { position });
            pump(5);
        }
        self.ui
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position: end,
                button: PointerEventButton::Left,
            });
        Ok(())
    }

    /// Frames the stone according to `wanted_focus`.
    pub fn frame(
        &mut self,
        wanted_focus: CameraFocus,
        force: bool,
        filmed: bool,
    ) -> Result<(), String> {
        let effective_focus = if wanted_focus != CameraFocus::Keep {
            wanted_focus
        } else if self.stage_state.focus != CameraFocus::Keep {
            self.stage_state.focus
        } else {
            CameraFocus::Whole
        };
        let focus_changed =
            wanted_focus != CameraFocus::Keep && wanted_focus != self.stage_state.focus;

        // has_solid can still describe the startup placeholder. In Edit the
        // empty-state cards cover it until a tutorial actually creates/loads its
        // design. Never drag those cards or remember a fit for that placeholder.
        let screen = self.current_screen_view();
        let editor = self.ui.global::<EditorModel>();
        let viewport = self.ui.global::<ViewportModel>();
        let has_design_picture = match screen {
            StageView::Edit => editor.get_has_design(),
            StageView::LiveRender if viewport.get_shows_editor_design() => editor.get_has_design(),
            StageView::LiveRender => self.ui.global::<LibraryModel>().get_selected_entry_id() >= 0,
            StageView::Either => false,
        };
        if !has_design_picture {
            self.note_once(
                "frame-skipped:no-design-picture",
                "camera framing skipped: no design picture is on screen yet".to_owned(),
            );
            return Ok(());
        }

        if let Some((key, why)) = self.camera_gesture_blocked(screen) {
            self.note_once(
                &format!("frame-skipped:{key}"),
                format!("camera framing skipped: {why}"),
            );
            return Ok(());
        }
        let spot = self.camera_surface(screen)?;

        let (start_yaw, start_pitch, start_dist) = harness_support::camera_pose();
        let target_pose = effective_focus.pose().unwrap_or((start_yaw, start_pitch));

        let target_dist = harness_support::fit_distance_now() * pacing::FIT_MARGIN;

        // Skip when focus did not change and radius changed by <= 3 % per S-D5.
        if !force
            && !focus_changed
            && let Some(last_dist) = self.stage_state.last_fit_radius
        {
            let delta = (f64::from(target_dist / pacing::FIT_MARGIN) - last_dist).abs()
                / last_dist.max(1e-6);
            if delta <= pacing::REFIT_THRESHOLD {
                return Ok(());
            }
        }

        let already_there = (target_pose.0 - start_yaw).abs() < 1e-3
            && (target_pose.1 - start_pitch).abs() < 1e-3
            && (target_dist - start_dist).abs() / start_dist.max(1e-6) < 1e-3;
        if !force && already_there {
            self.stage_state.last_fit_radius = Some(f64::from(target_dist / pacing::FIT_MARGIN));
            self.stage_state.focus = effective_focus;
            return Ok(());
        }

        // The viewport maps pointer deltas to yaw/pitch at -0.008/+0.008
        // radians per pixel (gem_viewport.slint and camera_lighting.rs).
        let center = spot.center();
        let dx = -(target_pose.0 - start_yaw) / 0.008;
        let dy = (target_pose.1 - start_pitch) / 0.008;
        if dx.abs() > 0.2 || dy.abs() > 0.2 {
            self.drag_camera(center, (dx, dy), filmed)?;
        }
        let (_, _, after_orbit_dist) = harness_support::camera_pose();
        let fit = harness_support::fit_distance_now() * pacing::FIT_MARGIN;
        let wheel = (fit - after_orbit_dist) / -0.002;
        if wheel.abs() > 0.5 {
            self.ui
                .window()
                .dispatch_event(WindowEvent::PointerScrolled {
                    position: LogicalPosition::new(center.0, center.1),
                    delta_x: 0.0,
                    delta_y: wheel,
                });
            pump(30);
        }
        let (yaw, pitch, distance) = harness_support::camera_pose();
        if (yaw - target_pose.0).abs() > 0.04
            || (pitch - target_pose.1).abs() > 0.04
            || (distance - fit).abs() / fit.max(1e-6) > 0.08
        {
            return Err(format!(
                "camera pointer/wheel framing did not read back target: pose ({yaw:.2},{pitch:.2},{distance:.2}), expected ({:.2},{:.2},{fit:.2})",
                target_pose.0, target_pose.1
            ));
        }
        if filmed {
            self.rec.hold(pacing::STAGE_HOLD)?;
        }

        // Only a successful readback establishes the next step's framing baseline.
        self.stage_state.last_fit_radius = Some(f64::from(fit / pacing::FIT_MARGIN));
        self.stage_state.focus = effective_focus;
        self.stage_state.camera_moves += 1;
        Ok(())
    }
}
