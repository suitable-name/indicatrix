//! The filmed setup of a lesson that works on "the design you have open": the clicks a learner
//! makes to get one (File, New Design..., a template card, Create), under a "Setup" chapter.
//! Nothing is created through a model callback, off camera or otherwise.

use crate::{director::Film, pacing, routes::OPEN_DESIGN_CARD, support::app::pump, targets};
use indicatrix_cut::{EditorModel, TemplateGalleryModel};
use indicatrix_editor::guide::StageView;
use slint::{ComponentHandle, Model};

impl Film<'_> {
    /// Brings the Edit view up by a click on its pill: the command bar's File menu lives there,
    /// and the app starts on Live Render, which has no File button. Nothing is clicked when the
    /// Edit view is already showing.
    ///
    /// # Errors
    ///
    /// The Edit pill is not on screen, or the click did not switch the view.
    pub(crate) fn ensure_edit_view(&mut self) -> Result<(), String> {
        if self.ui.get_active_tab() == 0 && self.ui.get_render_view_tab() == 1 {
            return Ok(());
        }
        let pill = targets::edit_pill(&self.view()).ok_or("Edit tab is missing")?;
        self.click_and_invoke(&pill)?;
        pump(30);
        if self.ui.get_active_tab() != 0 || self.ui.get_render_view_tab() != 1 {
            return Err("the Edit tab click did not switch the view to Edit".to_owned());
        }
        self.stage_state.view = StageView::Edit;
        self.stage_state.view_switches += 1;
        self.hold(pacing::VIEW_SWITCH_HOLD)
    }

    /// Opens the New Design dialog from the File menu, picks the gallery card
    /// [`OPEN_DESIGN_CARD`] and clicks Create, then waits for the design to be solved.
    pub(crate) fn film_design_setup(&mut self) -> Result<(), String> {
        self.rec.chapter("Setup");
        self.rec
            .caption("Create a design to work on: File, New Design, a template, Create.");
        let editor = self.ui.global::<EditorModel>();
        if !editor.get_new_dialog_open() {
            // The app starts on Live Render; the File menu is in the Edit view's command bar.
            self.ensure_edit_view()?;
            let file = targets::file_button(&self.view()).ok_or("File menu is missing")?;
            self.click_and_invoke(&file)?;
            self.hold(pacing::MENU_HOLD)?;
            let item = targets::menu_item(&self.view(), "New Design...")
                .ok_or("New Design menu item is missing")?;
            self.click_and_invoke(&item)?;
            pump(150);
            if !editor.get_new_dialog_open() {
                return Err("New Design menu click did not open the dialog".into());
            }
            self.hold(pacing::STAGE_HOLD)?;
        }
        let gallery = self.ui.global::<TemplateGalleryModel>();
        let card = gallery
            .get_shapes()
            .iter()
            .chain(gallery.get_variants().iter())
            .chain(gallery.get_blank().iter())
            .find(|card| card.template_index == OPEN_DESIGN_CARD)
            .ok_or_else(|| format!("template card {OPEN_DESIGN_CARD} is not in the gallery"))?;
        let label = if card.shape.is_empty() {
            card.name.to_string()
        } else {
            format!("{}, {}", card.name, card.shape)
        };
        let spot = targets::run_label(&self.view(), &label)
            .ok_or_else(|| format!("template card {label:?} is not visible"))?;
        self.click_and_invoke(&spot)?;
        if editor.get_new_template_index() != OPEN_DESIGN_CARD {
            return Err(format!(
                "template card click did not select {OPEN_DESIGN_CARD}"
            ));
        }
        pump(80);
        self.hold(pacing::STAGE_HOLD)?;
        let create = self.scoped_button("NewDesignDialog", "Create")?;
        self.click_and_invoke(&create)?;
        pump(100);
        if editor.get_new_dialog_open() {
            return Err("Create click left the New Design dialog open".into());
        }
        if !pacing::wait_lapse(&mut self.rec, pacing::PREPARE_TIMEOUT, || {
            editor.get_has_design() && !editor.get_solve_running()
        })? {
            return Err("the design the tutorial needs could not be created".into());
        }
        self.rec.caption("");
        Ok(())
    }
}
