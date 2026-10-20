//! Filmed routes of the Optimize, Retarget and Angle Sweep lessons, kept out of the registry in
//! `scripts.rs` so that its match stays a list of arms.

use crate::{director::Film, pacing, support::app::pump, targets};
use indicatrix_cut::{EditorModel, OptimizeModel, RetargetModel, SweepModel};
use slint::{ComponentHandle, Model};
use std::time::Duration;

impl Film<'_> {
    /// The Optimize tab: opens the Optimize Objective combo and chooses Low Windowing.
    pub(super) fn optimize_choose_objective(&mut self) -> Result<(), String> {
        // The previous step opened the Optimize tab; do not click it a second time.
        if self.ui.global::<EditorModel>().get_inspector_tab() != 2 {
            self.film_inspector_tab(2)?;
        }
        self.press_label("Optimize Objective")?;
        let optimize = self.ui.global::<OptimizeModel>();
        let labels = optimize.get_preset_labels();
        let idx = labels
            .iter()
            .position(|l| l.as_str() == "Low Windowing")
            .and_then(|at| i32::try_from(at).ok())
            .ok_or("Low Windowing objective is unavailable")?;
        let current = optimize.get_preset_index();
        self.select_open_combo(current, idx)?;
        if optimize.get_preset_index() != idx {
            return Err("Optimize Objective did not select Low Windowing".into());
        }
        pump(60);
        self.hold(pacing::MENU_HOLD)
    }

    /// The Optimize tab: opens the Angle Ranges panel.
    pub(super) fn optimize_open_ranges(&mut self) -> Result<(), String> {
        self.press_label("Angle Ranges...")?;
        if !self.ui.global::<OptimizeModel>().get_ranges_open() {
            return Err("Angle Ranges did not open".into());
        }
        self.hold(pacing::MENU_HOLD)
    }

    /// The Optimize tab: types a short budget and two candidates to keep, clicks Optimize and
    /// waits for the results.
    pub(super) fn optimize_run_search(&mut self) -> Result<(), String> {
        let budget_field = targets::text_input(&self.view(), "Search Budget")
            .ok_or("Search Budget field is not visible")?;
        self.click_and_type(&budget_field, "100")?;
        let optimize = self.ui.global::<OptimizeModel>();
        let candidates = targets::text_input(&self.view(), "Number of Candidates to Keep")
            .ok_or("Number of Candidates to Keep is not visible")?;
        self.click_and_type(&candidates, "2")?;
        if optimize.get_candidates() != 2 {
            return Err("Optimize candidates did not read back as 2".into());
        }
        let button = targets::run(
            &self.view(),
            &targets::Query::Id("OptimizeTab::tip-button-optimize"),
        )
        .ok_or("Optimize button is not visible")?;
        self.click_and_invoke(&button)?;
        let editor = self.ui.global::<EditorModel>();
        if !pacing::wait_real(Duration::from_secs(30), || {
            !editor.get_optimize_running() && optimize.get_has_results()
        }) {
            return Err("Optimize search did not finish in time".into());
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// The Optimize tab: clicks the first candidate.
    pub(super) fn optimize_pick_candidate(&mut self) -> Result<(), String> {
        let optimize = self.ui.global::<OptimizeModel>();
        let candidate = targets::run_label_start(&self.view(), "Candidate 1")
            .ok_or("Candidate 1 is not visible")?;
        self.click_and_invoke(&candidate)?;
        pump(60);
        if optimize.get_selected_candidate() < 0 {
            return Err("Candidate 1 was not selected".into());
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// The Optimize tab: clicks Apply This Candidate and waits for it to finish.
    pub(super) fn optimize_apply_candidate(&mut self) -> Result<(), String> {
        self.press_label("Apply This Candidate")?;
        let editor = self.ui.global::<EditorModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || !editor.get_optimize_running()) {
            return Err("Apply This Candidate did not finish".into());
        }
        Ok(())
    }

    /// Opens the Retarget dialog and chooses `material` as the target material.
    fn retarget_choose_material(&mut self, material: &str) -> Result<(), String> {
        self.press_label("Retarget...")?;
        let retarget = self.ui.global::<RetargetModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || retarget.get_is_open()) {
            return Err("Retarget dialog did not open".into());
        }
        let index = retarget
            .get_material_options()
            .iter()
            .position(|option| option.as_str() == material)
            .and_then(|at| i32::try_from(at).ok())
            .ok_or_else(|| format!("{material} is not in Retarget material options"))?;
        let combo = targets::run_label(&self.view(), "Target Material")
            .ok_or("Target Material combo is missing")?;
        let current = retarget.get_target_material_index();
        self.click_and_invoke(&combo)?;
        self.select_open_combo(current, index)?;
        if retarget.get_target_material_index() != index {
            return Err(format!("Target Material did not select {material}"));
        }
        Ok(())
    }

    /// Clicks Apply in the Retarget dialog and waits for it to close.
    fn retarget_apply(&mut self) -> Result<(), String> {
        let retarget = self.ui.global::<RetargetModel>();
        let apply = self.scoped_button("RetargetDialog", "Apply")?;
        self.click_and_invoke(&apply)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || !retarget.get_is_open()) {
            return Err("Retarget Apply did not close dialog".into());
        }
        Ok(())
    }

    /// Retarget in Shift mode to topaz: waits for Valid, then applies.
    pub(super) fn retarget_shift_to_topaz(&mut self) -> Result<(), String> {
        self.retarget_choose_material("Topaz")?;
        let retarget = self.ui.global::<RetargetModel>();
        if retarget.get_mode_index() != 0 {
            let shift = self.scoped_button("RetargetDialog", "Shift")?;
            self.click_and_invoke(&shift)?;
        }
        if retarget.get_mode_index() != 0 {
            return Err("Retarget Shift mode is not active".into());
        }
        pump(60);
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || retarget.get_validity_state() == 2) {
            return Err(format!(
                "Retarget to Topaz did not become Valid, state={}",
                retarget.get_validity_state()
            ));
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.retarget_apply()
    }

    /// Retarget in Optimize mode to sapphire: searches, picks the first candidate, applies.
    pub(super) fn retarget_optimize_to_sapphire(&mut self) -> Result<(), String> {
        self.retarget_choose_material("Sapphire")?;
        let retarget = self.ui.global::<RetargetModel>();
        if retarget.get_mode_index() != 1 {
            // The inspector has an "Optimize" tab behind the dialog: scope to the dialog.
            let optimize = self.scoped_button("RetargetDialog", "Optimize")?;
            self.click_and_invoke(&optimize)?;
        }
        if retarget.get_mode_index() != 1 {
            return Err("Retarget Optimize mode is not active".into());
        }
        pump(60);
        self.press_label("Search for Better Angles")?;
        if !pacing::wait_real(Duration::from_secs(30), || {
            !retarget.get_is_busy() && retarget.get_candidates().row_count() > 0
        }) {
            return Err("Retarget Optimize search did not complete with candidates".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        let candidate = targets::run_label_start(&self.view(), "1. ")
            .ok_or("Retarget Candidate 1 is not visible")?;
        self.click_and_invoke(&candidate)?;
        if retarget.get_selected_candidate() != 0 {
            return Err("Retarget Candidate 1 was not selected".into());
        }
        self.retarget_apply()
    }

    /// The Angle Sweep dialog: sweeps Pavilion Main from 38 to 42 degrees in steps of 1, picks the
    /// 42 degree row, uses it and closes the dialog.
    pub(super) fn sweep_pavilion_and_use(&mut self) -> Result<(), String> {
        self.open_sweep()?;
        let sweep = self.ui.global::<SweepModel>();
        let tier_idx = sweep
            .get_tier_names()
            .iter()
            .position(|name| name.contains("Pavilion Main"))
            .and_then(|at| i32::try_from(at).ok())
            .ok_or("Pavilion Main is not offered for Angle Sweep")?;
        let tier_combo = targets::run_label(&self.view(), "Tier to Sweep")
            .ok_or("Tier to Sweep combo is missing")?;
        let current = sweep.get_tier_index();
        self.click_and_invoke(&tier_combo)?;
        self.select_open_combo(current, tier_idx)?;
        if sweep.get_tier_index() != tier_idx {
            return Err("Angle Sweep tier did not select Pavilion Main".into());
        }
        for (label, value) in [
            ("From (degrees)", "38"),
            ("To (degrees)", "42"),
            ("Step (degrees)", "1"),
        ] {
            let field = targets::text_input(&self.view(), label)
                .ok_or_else(|| format!("Angle Sweep {label} field is missing"))?;
            self.click_and_type(&field, value)?;
        }
        if sweep.get_from_text() != "38"
            || sweep.get_to_text() != "42"
            || sweep.get_step_text() != "1"
        {
            return Err("Angle Sweep range did not read back as 38-42 by 1".into());
        }
        pump(60);
        self.press_label("Run Sweep")?;
        if !pacing::wait_real(Duration::from_secs(30), || {
            !sweep.get_running() && sweep.get_has_results()
        }) {
            return Err("Angle sweep did not complete in time".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        let row_idx = sweep
            .get_rows()
            .iter()
            .position(|r| r.angle.as_str() == "42.00")
            .and_then(|at| i32::try_from(at).ok())
            .ok_or("Angle 42.00 row not found in sweep results")?;
        let selected = targets::run_label_start(&self.view(), "Angle 42.00")
            .ok_or("Angle 42.00 result row is not visible")?;
        self.click_and_invoke(&selected)?;
        if sweep.get_selected_row() != row_idx {
            return Err("Angle Sweep row click did not select 42.00".into());
        }
        let use_label = sweep.get_use_label().to_string();
        let use_btn = self.scoped_button("SweepDialog", &use_label)?;
        self.click_and_invoke(&use_btn)?;
        pump(60);
        let close = self.scoped_button("SweepDialog", "Close")?;
        self.click_and_invoke(&close)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || !sweep.get_dialog_open()) {
            return Err("Angle Sweep Close click did not close the dialog".into());
        }
        Ok(())
    }
}
