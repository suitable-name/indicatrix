//! Filmed routes that open a design: an imported design from the library (the Adopt Imported
//! Meets lesson's first step and the Deep Solve lesson's Setup) and the design file the Open
//! lesson has just saved.
//!
//! Every click is on a visible control. The native file dialog of File > Open is answered with
//! `queue_picker_path` after the filmed click that opens it, as everywhere in the harness.

use crate::{
    director::Film,
    pacing,
    scripts::pure_tables::output_path,
    support::app::pump,
    targets::{self, Spot},
};
use indicatrix_cut::{EditorModel, LibraryModel, harness_support};
use slint::{ComponentHandle, Model};
use std::time::Duration;

/// How many designs of the library list are tried, from the top, before the video is abandoned.
/// A design that is not an import (a native file, an angle table) has no imported meet.
const MAX_DESIGNS: usize = 4;

/// How long one load may take to show its tiers (real time, shown as a time-lapse).
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

/// The file name the Save step of a lesson writes (`routes.rs`, the `design_saved` route).
const SAVED_DESIGN: &str = "tutorial-design.isdd";

/// The component of the tier table.
const TIER_TABLE: &str = "EditorTierTable";

/// The label of the Adopt button every imported row of the tier table carries.
const ADOPT_BUTTON: &str = "Adopt";

impl Film<'_> {
    /// Films the step "Open an imported design": the library list is shown, a design is picked in
    /// it and File > Load Selected opens it, until the design has a meet its file stated.
    ///
    /// The lesson starts from a design built in the editor, which has nothing to adopt. Replacing
    /// it makes the editor ask about its unsaved changes, and the question is answered with its
    /// own Discard button, as a learner would.
    pub(super) fn open_imported_library_design(&mut self) -> Result<(), String> {
        let library = self.ui.global::<LibraryModel>();
        let listed = library.get_diagram_list().row_count();
        if listed == 0 {
            return Err("the library lists no design to open".to_owned());
        }
        let tries = listed.min(MAX_DESIGNS);
        for at in 0..tries {
            self.show_library_list()?;
            let row = library
                .get_diagram_list()
                .row_data(at)
                .ok_or_else(|| format!("library row {at} disappeared"))?;
            self.click_library_row(at, row.title.as_str(), row.id)?;
            self.load_selected_design()?;
            if self.wait_for_imported_meet()? {
                return self.hold(pacing::RESULT_HOLD);
            }
        }
        Err(format!(
            "none of the first {tries} designs of the library has an imported meet to adopt; \
             INDICATRIX_HARNESS_LIBRARY must name a library of imported designs"
        ))
    }

    /// Films the first part of the Deep Solve lesson's Setup: a catalogue design is loaded from
    /// the library, the way [`Self::open_imported_library_design`] films it.
    ///
    /// The lesson's first step says Deep Solve stays dim until a design with printed proportions
    /// is loaded from the library and a tier adopted, and a template design (see
    /// `Film::film_design_setup`) has neither. So, as the step says, the learner does both before
    /// starting the lesson.
    pub(crate) fn film_catalogue_setup(&mut self) -> Result<(), String> {
        self.rec.chapter("Setup");
        self.rec.caption(
            "Deep Solve needs a catalogue design: load one from the library, then adopt a tier.",
        );
        self.open_imported_library_design()
    }

    /// Films the second part of the Deep Solve lesson's Setup: Adopt is clicked on one tier of the
    /// loaded design, and Deep Solve must then be enabled.
    pub(crate) fn adopt_for_deep_solve(&mut self) -> Result<(), String> {
        self.adopt_one_imported_tier()?;
        pump(100);
        let editor = self.ui.global::<EditorModel>();
        if !pacing::wait_lapse(&mut self.rec, LOAD_TIMEOUT, || {
            editor.get_deep_solve_available() && !editor.get_solve_running()
        })? {
            return Err(format!(
                "Deep Solve stays dim after loading a library design and adopting a tier: {}",
                editor.get_deep_solve_hint()
            ));
        }
        self.rec.caption("");
        self.hold(pacing::RESULT_HOLD)
    }

    /// Clicks the Adopt button of one imported row of the tier table (any row's button will do:
    /// the table repeats the label on every imported row), then turns the table back to where it
    /// was.
    pub(super) fn adopt_one_imported_tier(&mut self) -> Result<(), String> {
        let (adopt, turns) = self.first_adopt_button()?;
        self.click_and_invoke(&adopt)?;
        self.unturn_table(turns)
    }

    /// The Adopt button of the first imported row that is on view in the tier table, with the
    /// number of sideways turns it took to bring it there.
    ///
    /// The Adopt column is the 12th of 18, about 610 px along a row that is about 900 px wide,
    /// while the table of the default dock (640 px, less its paddings and scroll bar) shows about
    /// 590 px of it. So the column starts out beyond the right edge of the table: its buttons
    /// still have a place in the window (over whatever lies beside the table), but a click there
    /// would land on that neighbour. Only a button that lies wholly inside the table's scrolling
    /// area counts ([`Spot::in_view_of`]); the wheel, turned sideways over the table as a learner
    /// would, brings one in. When no imported row is level with the table (it was scrolled down
    /// away from them) the wheel brings the table to its top first.
    fn first_adopt_button(&mut self) -> Result<(Spot, usize), String> {
        let viewport = self.tier_table_viewport()?;
        let level = self
            .scoped_spots(TIER_TABLE, ADOPT_BUTTON)
            .iter()
            .any(|button| button.level_with(&viewport));
        if !level {
            self.wheel_over(&viewport, 3000.0)?;
            pump(60);
        }
        self.reveal_table_button(|film, area| {
            film.scoped_spots(TIER_TABLE, ADOPT_BUTTON)
                .into_iter()
                .find(|button| button.in_view_of(area))
        })
        .map_err(|why| format!("the Adopt button of an imported tier: {why}"))
    }

    /// Films the step "Open it" of the Open lesson: File > Open on the file the lesson's Save step
    /// wrote, which the picker answers with.
    pub(super) fn open_saved_design(&mut self) -> Result<(), String> {
        let path = output_path(&self.id, SAVED_DESIGN);
        if !path.is_file() {
            return Err(format!(
                "File > Open: the saved design {} is missing",
                path.display()
            ));
        }
        harness_support::queue_picker_path(path);
        let file = targets::file_button(&self.view()).ok_or("File button is missing")?;
        self.click_and_invoke(&file)?;
        self.hold(pacing::MENU_HOLD)?;
        let open = targets::menu_item(&self.view(), "Open").ok_or("File > Open is missing")?;
        self.click_and_invoke(&open)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            harness_support::pending_picker_answers() == 0
        }) {
            return Err("File > Open did not consume its picker path".to_owned());
        }
        Ok(())
    }

    /// Expands the library panel when the stage collapsed it, by clicking its rail button.
    fn show_library_list(&mut self) -> Result<(), String> {
        let library = self.ui.global::<LibraryModel>();
        if !library.get_panel_collapsed() {
            return Ok(());
        }
        let button = targets::show_library_button(&self.view())
            .ok_or("the Show Library button is missing")?;
        self.click_and_invoke(&button)?;
        pump(30);
        if library.get_panel_collapsed() {
            return Err("the Show Library click did not expand the panel".to_owned());
        }
        self.hold(pacing::STAGE_HOLD)
    }

    /// Clicks File, then Load Selected, and answers the unsaved-changes question when the editor
    /// asks it.
    fn load_selected_design(&mut self) -> Result<(), String> {
        // The File menu is in the Edit view's command bar; the app starts on Live Render.
        self.ensure_edit_view()?;
        let file = targets::file_button(&self.view()).ok_or("File button is missing")?;
        self.click_and_invoke(&file)?;
        self.hold(pacing::MENU_HOLD)?;
        let load = targets::menu_item(&self.view(), "Load Selected")
            .ok_or("File > Load Selected is missing")?;
        self.click_and_invoke(&load)?;
        pump(150);
        if self.ui.global::<EditorModel>().get_unsaved_dialog_open() {
            self.hold(pacing::RESULT_HOLD)?;
            let discard = self.scoped_button("ConfirmActionDialog", "Discard")?;
            self.click_and_invoke(&discard)?;
            pump(100);
        }
        Ok(())
    }

    /// Waits for the loaded design to show a tier with a meet its file stated; whether it did.
    fn wait_for_imported_meet(&mut self) -> Result<bool, String> {
        let editor = self.ui.global::<EditorModel>();
        pacing::wait_lapse(&mut self.rec, LOAD_TIMEOUT, || {
            editor.get_has_design()
                && !editor.get_solve_running()
                && editor
                    .get_tiers()
                    .iter()
                    .any(|row| !row.imported_meet_text.is_empty())
        })
    }
}
