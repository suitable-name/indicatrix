//! Filmed routes of single lesson steps that take more than one gesture, kept out of the
//! registry in `scripts.rs` so that its match stays a list of arms.

use crate::{
    director::Film,
    pacing,
    targets::{self, Query},
};
use indicatrix_cut::{EditorModel, GuideModel, ManipulateModel, VariantsModel};
use indicatrix_editor::guide::TierEntry;
use slint::{ComponentHandle, Model};

impl Film<'_> {
    /// The Preform tab: types a half-width of 1.8 for the rough and clicks Apply Preform.
    pub(super) fn film_preform_width(&mut self) -> Result<(), String> {
        if self.ui.global::<EditorModel>().get_inspector_tab() != 1 {
            self.film_inspector_tab(1)?;
        }
        let field = self
            .input_spot("Half-Width (Model Units, Girdle Half-Width = 1)")?
            .ok_or("Preform Half-Width input is missing")?;
        self.click_and_type(&field, "1.8")?;
        let apply = targets::run_label_start(&self.view(), "Apply Preform")
            .ok_or("Apply Preform button is missing")?;
        self.click_and_invoke(&apply)?;
        if self.ui.global::<EditorModel>().get_preform_half_width() != "1.8" {
            return Err("Apply Preform did not keep Half-Width at 1.8".into());
        }
        Ok(())
    }

    /// Crown Main: turns its index 12 into 12.5, a position between two teeth, in the Tier form.
    pub(super) fn film_index_between_teeth(&mut self) -> Result<(), String> {
        let index = self.film_select_tier("Crown Main")?;
        let row = self
            .ui
            .global::<EditorModel>()
            .get_tiers()
            .row_data(usize::try_from(index).map_err(|_| "Crown Main has no row")?)
            .ok_or_else(|| "Crown Main disappeared".to_owned())?;
        let mut changed = false;
        let indices = row
            .indices_full
            .split(',')
            .map(|part| {
                if part.trim() == "12" {
                    changed = true;
                    "12.5".to_owned()
                } else {
                    part.trim().to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        if !changed {
            return Err(format!(
                "Crown Main has no exact 12 token: {:?}",
                row.indices_full
            ));
        }
        self.film_tier(&TierEntry::of("Crown Main").indices(&indices))
    }

    /// The History tab: opens its Variants view (through the Steps view's tab when that one is
    /// showing) and waits for the step's goal to hold.
    pub(super) fn open_variants_view(&mut self) -> Result<(), String> {
        self.film_inspector_tab(4)?;
        if self.ui.global::<VariantsModel>().get_view() == 1 {
            let steps = targets::run(&self.view(), &Query::Tab("Steps"))
                .ok_or("History Steps tab is missing")?;
            self.click_and_invoke(&steps)?;
        }
        let variants = targets::run(&self.view(), &Query::Tab("Variants"))
            .ok_or("History Variants tab is missing")?;
        self.click_and_invoke(&variants)?;
        if self.ui.global::<VariantsModel>().get_view() != 1 {
            return Err("Variants view did not open".into());
        }
        if !self.wait_done(0, pacing::STEP_TIMEOUT)? {
            return Err(format!(
                "Variants tab click left the composite goal false: {}",
                self.ui.global::<GuideModel>().get_current().waiting
            ));
        }
        Ok(())
    }

    /// Slice mode: clicks Symmetric, holds the changed state on screen and clicks it back.
    pub(super) fn film_symmetric_toggle(&mut self) -> Result<(), String> {
        let before = self.ui.global::<ManipulateModel>().get_slice_symmetric();
        self.press_label("Symmetric")?;
        if self.ui.global::<ManipulateModel>().get_slice_symmetric() == before {
            return Err("Symmetric did not toggle".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.press_label("Symmetric")?;
        if self.ui.global::<ManipulateModel>().get_slice_symmetric() != before {
            return Err("Symmetric did not return to its original state".into());
        }
        Ok(())
    }

    /// The Diagram view: drags the angle handle until the step's goal holds.
    pub(super) fn drag_diagram_angle_handle(&mut self) -> Result<(), String> {
        for distance in [55.0, -110.0] {
            self.drag_handle(0, distance, false)?;
            if self.ui.global::<GuideModel>().get_step_done() {
                break;
            }
        }
        if !self.ui.global::<GuideModel>().get_step_done() {
            return Err(format!(
                "Diagram A-handle drag did not change a tier angle: {}",
                self.ui.global::<ManipulateModel>().get_hint_text()
            ));
        }
        Ok(())
    }
}
