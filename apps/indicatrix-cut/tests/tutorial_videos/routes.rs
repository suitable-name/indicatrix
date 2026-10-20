//! The routes a learner takes for the steps that have no recipe (a step that waits for a UI event),
//! shown in the video, plus the small app helpers the director shares.
//!
//! Every route is a click or key input on a visible control; native file dialogs are answered
//! with `queue_picker_path` after the filmed click that opens them. An event with no route is
//! never raised through `GuideModel.event`: the video is abandoned instead.

use crate::{
    director::Film, pacing, scripts::pure_tables::output_path, support::WindowView, targets,
};
use indicatrix_cut::{
    ActivityModel, EditorModel, LibraryModel, MainWindow, PreferencesModel, TutorialRowData,
    TutorialsModel, VariantsModel, harness_support,
};
use indicatrix_editor::guide::Goal;
use slint::{ComponentHandle, Model};

pub use crate::scripts::pure_tables::event_only;

/// The gallery card the tutorials that need an open design get one from (the Standard Round
/// Brilliant).
pub const OPEN_DESIGN_CARD: i32 = 1;

/// Whether a "Path tracing" activity is running in `ui`.
#[must_use]
pub fn trace_running(ui: &MainWindow) -> bool {
    ui.global::<ActivityModel>()
        .get_activities()
        .iter()
        .any(|activity| activity.kind.as_str() == "trace")
}

/// The browser row for guide `id`, if the browser lists it.
#[must_use]
pub fn browser_row(ui: &MainWindow, id: &str) -> Option<TutorialRowData> {
    let rows = ui.global::<TutorialsModel>().get_rows();
    (0..rows.row_count())
        .filter_map(|at| rows.row_data(at))
        .find(|row| row.id.as_str() == id)
}

/// Every label, placeholder text and field value in `view` (the privacy scan reads them).
/// Copy of `ui_screens/kit.rs::visible_texts`.
#[must_use]
pub fn visible_texts(view: &WindowView) -> Vec<String> {
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

/// A short name for a goal kind, for the abandon reason.
#[must_use]
pub const fn goal_kind(goal: &Goal) -> &'static str {
    match goal {
        Goal::Manual => "Manual",
        Goal::Event(_) => "Event",
        Goal::TierMatches { .. } => "TierMatches",
        Goal::TierExists(_) => "TierExists",
        Goal::TierCountAtLeast(_) => "TierCountAtLeast",
        Goal::Material(_) => "Material",
        Goal::SolvedClosed => "SolvedClosed",
        Goal::YieldApplied => "YieldApplied",
        Goal::ViewMode(_) => "ViewMode",
        Goal::InspectorTab(_) => "InspectorTab",
        Goal::FreshDesign { .. } => "FreshDesign",
        Goal::ConcaveTiersAtLeast(_) => "ConcaveTiersAtLeast",
        Goal::DesignRebuilt { .. } => "DesignRebuilt",
        Goal::Check { label, .. } => label,
        Goal::All(_) => "All",
        Goal::Any(_) => "Any",
    }
}

impl Film<'_> {
    /// Opens a real File menu item and answers only its native picker. The path is
    /// relative to uiprobe's isolated profile CWD and private to this guide
    /// ([`output_path`]), so success toasts stay neutral and two guides never collide.
    fn filmed_file_action(&mut self, item: &'static str, file: &str) -> Result<(), String> {
        let path = output_path(&self.id, file);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("create tutorial output: {e}"))?;
        }
        if path.exists() {
            return Err(format!(
                "{item}: output already exists before the click: {}",
                path.display()
            ));
        }
        harness_support::queue_picker_path(path.clone());
        let menu = targets::file_button(&self.view()).ok_or("File button is missing")?;
        self.click_and_invoke(&menu)?;
        self.hold(pacing::MENU_HOLD)?;
        let action = targets::menu_item(&self.view(), item)
            .ok_or_else(|| format!("File menu item {item:?} is missing"))?;
        self.click_and_invoke(&action)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || path.exists()) {
            return Err(format!(
                "{item}: native picker/write did not create {}",
                path.display()
            ));
        }
        if harness_support::pending_picker_answers() != 0 {
            return Err(format!(
                "{item}: native picker did not consume its queued path"
            ));
        }
        Ok(())
    }

    /// Dispatch only observed UI routes. State setters and fabricated completion events
    /// cannot stand in for an interaction, including members of composite goals.
    pub(crate) fn drive_event(&mut self, n: usize, name: &str) -> Result<(), String> {
        let routed = self.drive_solve_event(name)?
            || self.drive_file_event(name)?
            || self.drive_library_event(n, name)?
            || self.drive_view_event(n, name)?;
        if !routed {
            return Err(format!(
                "GAP, event {name:?}: no authored input route; synthetic completion is forbidden"
            ));
        }
        self.hold(pacing::RESULT_HOLD)?;
        Ok(())
    }

    /// The events of solving and the snapshot; whether `name` is one of them.
    fn drive_solve_event(&mut self, name: &str) -> Result<bool, String> {
        match name {
            "snapshot_taken" => self.film_command("solve.snapshot")?,
            "solve_requested" => self.film_command(indicatrix_editor::guide::CMD_SOLVE)?,
            "auto_solve_off" => self.film_auto_solve(false)?,
            "auto_solve_on" => self.film_auto_solve(true)?,
            "deep_solve_started" => self.film_deep_solve_start()?,
            "deep_solve_finished" => {
                let editor = self.ui.global::<EditorModel>();
                if !pacing::wait_lapse(&mut self.rec, std::time::Duration::from_secs(90), || {
                    !editor.get_deep_solve_running() && !editor.get_deep_solve_status().is_empty()
                })? {
                    return Err("Deep Solve did not finish with a status".into());
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Opens the Auto-Solve combo and picks Off (`on` false) or the first budget after it.
    fn film_auto_solve(&mut self, on: bool) -> Result<(), String> {
        let combo = targets::resolve(&self.view(), "auto_solve");
        let targets::Resolved::Found(combo) = combo else {
            return Err("Auto-Solve combo is missing".into());
        };
        self.click_and_invoke(&combo)?;
        for _ in 0..4 {
            uiprobe::view::press(self.ui.window(), "Up")
                .map_err(|e| format!("Auto-Solve Up: {e:?}"))?;
        }
        if on {
            uiprobe::view::press(self.ui.window(), "Down")
                .map_err(|e| format!("Auto-Solve Down: {e:?}"))?;
        }
        uiprobe::view::press(self.ui.window(), "Return")
            .map_err(|e| format!("Auto-Solve Return: {e:?}"))?;
        let budget = self.ui.global::<EditorModel>().get_auto_solve_budget_ms();
        if on && budget == 0 {
            return Err("Auto-Solve combo did not turn automatic solving back on".into());
        }
        if !on && budget != 0 {
            return Err("Auto-Solve Off selection did not change the budget to zero".into());
        }
        Ok(())
    }

    /// Clicks Deep Solve and waits for a run, a new status or the step's completion.
    fn film_deep_solve_start(&mut self) -> Result<(), String> {
        let before = self.ui.global::<EditorModel>().get_deep_solve_status();
        let targets::Resolved::Found(button) = targets::resolve(&self.view(), "deep_solve_button")
        else {
            return Err("Deep Solve button is missing".into());
        };
        self.click_and_invoke(&button)?;
        let editor = self.ui.global::<EditorModel>();
        let guide = self.ui.global::<indicatrix_cut::GuideModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            editor.get_deep_solve_running()
                || editor.get_deep_solve_status() != before
                || guide.get_step_done()
        }) {
            return Err("Deep Solve click produced no run or completion readback".into());
        }
        Ok(())
    }

    /// The events of saving, exporting and importing files; whether `name` is one of them.
    fn drive_file_event(&mut self, name: &str) -> Result<bool, String> {
        match name {
            "design_saved" => self.filmed_file_action("Save", "tutorial-design.isdd")?,
            "sheet_exported" => {
                self.filmed_file_action("Export Cutting Sheet (HTML)...", "cutting-sheet.html")?;
            }
            "diagram_exported" => {
                self.filmed_file_action("Export Diagram (PNG)...", "diagram.png")?;
            }
            "asc_exported" => self.filmed_file_action("Export .asc", "edited.asc")?,
            "gcs_exported" => {
                self.filmed_file_action("Export as Gem Cut Studio (.gcs)...", "edited.gcs")?;
            }
            "library_imported" => self.film_library_import()?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Imports the `.asc` an earlier step exported, through the library's Import dialog.
    fn film_library_import(&mut self) -> Result<(), String> {
        let path = output_path(&self.id, "edited.asc");
        if !path.is_file() {
            return Err("exported edited.asc is missing before Import".into());
        }
        let library = self.ui.global::<LibraryModel>();
        let before = library.get_import_done();
        harness_support::queue_picker_path(path);
        let import = targets::resolve(&self.view(), "library_import");
        let targets::Resolved::Found(import) = import else {
            return Err("Import button is missing".into());
        };
        self.click_and_invoke(&import)?;
        self.hold(pacing::MENU_HOLD)?;
        let file = targets::run_label(&self.view(), "Choose File...")
            .ok_or("Import Choose File button is missing")?;
        self.click_and_invoke(&file)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            !library.get_is_busy() && library.get_import_done() > before
        }) {
            return Err(format!(
                "Import did not complete: {}",
                library.get_import_result_text()
            ));
        }
        if harness_support::pending_picker_answers() != 0 {
            return Err("Import picker did not consume edited.asc".into());
        }
        Ok(())
    }

    /// The events of the library list and its search and filters; whether `name` is one of them.
    fn drive_library_event(&mut self, n: usize, name: &str) -> Result<bool, String> {
        match name {
            "library_searched" => {
                let library = self.ui.global::<LibraryModel>();
                let title = library
                    .get_diagram_list()
                    .row_data(0)
                    .ok_or("Library search has no listed design")?
                    .title
                    .to_string();
                let query: String = title.trim().chars().take(3).collect();
                if query.is_empty() {
                    return Err("first library design has an empty title".into());
                }
                let field = targets::text_input(&self.view(), "Search the Library")
                    .ok_or("Library search input is missing")?;
                self.click_and_type(&field, &query)?;
                if library.get_search_text().as_str() != query {
                    return Err("Library search text did not read back as typed".into());
                }
            }
            "library_search_cleared" => {
                let clear = targets::run_label(&self.view(), "Clear Search")
                    .ok_or("Clear Search button is missing")?;
                self.click_and_invoke(&clear)?;
                if !self
                    .ui
                    .global::<LibraryModel>()
                    .get_search_text()
                    .is_empty()
                {
                    return Err("Clear Search did not empty the search box".into());
                }
            }
            "library_design_selected" => self.film_pick_library_row()?,
            "design_replaced" => {
                let edit = targets::edit_pill(&self.view()).ok_or("Edit tab is missing")?;
                self.click_and_invoke(&edit)?;
                let file = targets::file_button(&self.view()).ok_or("File menu is missing")?;
                self.click_and_invoke(&file)?;
                self.hold(pacing::MENU_HOLD)?;
                let load = targets::menu_item(&self.view(), "Load Selected")
                    .ok_or("Load Selected menu item is missing")?;
                self.click_and_invoke(&load)?;
                if self.ui.global::<EditorModel>().get_unsaved_dialog_open() {
                    return Err("Load Selected raised an unsaved-changes question".into());
                }
            }
            "library_filtered" => self.film_library_filter(n)?,
            "library_filters_reset" => self.film_reset_library_filters()?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Scrolls the library list until a design other than the selected one is on screen and
    /// clicks its row.
    fn film_pick_library_row(&mut self) -> Result<(), String> {
        let library = self.ui.global::<LibraryModel>();
        let rows = library.get_diagram_list();
        let (index, row) = rows
            .iter()
            .enumerate()
            .find(|(_, row)| row.id != library.get_selected_entry_id())
            .ok_or("Library has no unselected design row")?;
        self.click_library_row(index, row.title.as_str(), row.id)
    }

    /// Scrolls the library list to its top and then down until row `index` (the design `title`,
    /// library entry `id`) is on screen, clicks it and requires it to read back as the selection.
    pub(crate) fn click_library_row(
        &mut self,
        index: usize,
        title: &str,
        id: i32,
    ) -> Result<(), String> {
        let library = self.ui.global::<LibraryModel>();
        let list = self
            .view()
            .root
            .query_descendants()
            .match_inherits("DiagramListView")
            .find_first()
            .ok_or("library list is missing")?;
        let scroll = list
            .query_descendants()
            .match_inherits("ListView")
            .find_first()
            .ok_or("library ListView is missing")?;
        let viewport = targets::Spot::of(&scroll);
        self.wheel_over(&viewport, 3000.0)?;
        let mut selected = None;
        for _ in 0..16 {
            let view = self.view();
            selected = view
                .elements()
                .into_iter()
                .filter(|element| {
                    element.accessible_role() == Some(uiprobe::AccessibleRole::ListItem)
                        && element.accessible_item_index() == Some(index)
                        && element
                            .accessible_label()
                            .is_some_and(|label| label.starts_with(title))
                })
                .map(|element| targets::Spot::of(&element))
                .find(|spot| {
                    let (_, y) = spot.center();
                    spot.usable(view.size) && y >= viewport.y && y <= viewport.y + viewport.h
                });
            if selected.is_some() {
                break;
            }
            self.wheel_over(&viewport, -250.0)?;
            self.hold(std::time::Duration::from_millis(200))?;
        }
        let spot = selected
            .ok_or_else(|| format!("library row id {id} is not visible after scrolling"))?;
        self.click_and_invoke(&spot)?;
        if library.get_selected_entry_id() != id {
            return Err(format!("library row click did not select entry {id}"));
        }
        Ok(())
    }

    /// The library filter lessons' step `n`: 0 the Shape Filter, 1 the Filters panel's Refractive
    /// Index slider, 2 the Sort Order.
    fn film_library_filter(&mut self, n: usize) -> Result<(), String> {
        let library = self.ui.global::<LibraryModel>();
        match n {
            0 => {
                if library.get_shape_options().row_count() < 2 {
                    return Err("Shape Filter has no non-default option".into());
                }
                let combo = targets::run_label(&self.view(), "Shape Filter")
                    .ok_or("Shape Filter combo is missing")?;
                self.click_and_invoke(&combo)?;
                for _ in 0..library.get_selected_shape_index() {
                    uiprobe::view::press(self.ui.window(), "Up")
                        .map_err(|e| format!("Shape Up: {e:?}"))?;
                }
                uiprobe::view::press(self.ui.window(), "Down")
                    .map_err(|e| format!("Shape Down: {e:?}"))?;
                uiprobe::view::press(self.ui.window(), "Return")
                    .map_err(|e| format!("Shape Return: {e:?}"))?;
                if library.get_selected_shape_index() != 1 {
                    return Err("Shape Filter did not select the first shape".into());
                }
            }
            1 => {
                let filters = targets::run_label_start(&self.view(), "Filters")
                    .ok_or("Filters button is missing")?;
                self.click_and_invoke(&filters)?;
                self.hold(pacing::MENU_HOLD)?;
                let slider =
                    targets::run_label(&self.view(), "Refractive Index, lowest value to keep")
                        .ok_or("Refractive Index lower slider is missing")?;
                let old = library.get_ri_filter_min();
                self.click_and_invoke(&slider)?;
                if (library.get_ri_filter_min() - old).abs() < f32::EPSILON {
                    return Err("Refractive Index slider click did not change the range".into());
                }
                uiprobe::view::press(self.ui.window(), "Escape")
                    .map_err(|e| format!("close Filters: {e:?}"))?;
            }
            2 => {
                let combo = targets::run_label(&self.view(), "Sort Order")
                    .ok_or("Sort Order combo is missing")?;
                self.click_and_invoke(&combo)?;
                let current = library.get_sort_order_index();
                let key = if current > 2 { "Up" } else { "Down" };
                for _ in 0..(2 - current).abs() {
                    uiprobe::view::press(self.ui.window(), key)
                        .map_err(|e| format!("Sort {key}: {e:?}"))?;
                }
                uiprobe::view::press(self.ui.window(), "Return")
                    .map_err(|e| format!("Sort Return: {e:?}"))?;
                if library.get_sort_order_index() != 2 {
                    return Err("Sort Order did not select Newest".into());
                }
            }
            _ => return Err(format!("no library_filtered action for step {n}")),
        }
        Ok(())
    }

    /// Opens the Filters panel, clicks Reset Filters and puts the Shape and Gear combos back on
    /// their first choice.
    fn film_reset_library_filters(&mut self) -> Result<(), String> {
        let filters =
            targets::run_label_start(&self.view(), "Filters").ok_or("Filters button is missing")?;
        self.click_and_invoke(&filters)?;
        self.hold(pacing::MENU_HOLD)?;
        let reset = targets::run_label(&self.view(), "Reset Filters")
            .ok_or("Reset Filters button is missing")?;
        self.click_and_invoke(&reset)?;
        uiprobe::view::press(self.ui.window(), "Escape")
            .map_err(|e| format!("close Filters: {e:?}"))?;
        let library = self.ui.global::<LibraryModel>();
        for (label, index) in [
            ("Shape Filter", library.get_selected_shape_index()),
            ("Gear Filter", library.get_selected_gear_index()),
        ] {
            if index == 0 {
                continue;
            }
            let combo = targets::run_label(&self.view(), label)
                .ok_or_else(|| format!("{label} is missing"))?;
            self.click_and_invoke(&combo)?;
            for _ in 0..index {
                uiprobe::view::press(self.ui.window(), "Up")
                    .map_err(|e| format!("{label} Up: {e:?}"))?;
            }
            uiprobe::view::press(self.ui.window(), "Return")
                .map_err(|e| format!("{label} Return: {e:?}"))?;
        }
        if library.get_selected_shape_index() != 0 || library.get_selected_gear_index() != 0 {
            return Err("Reset Filters left Shape or Gear restricted".into());
        }
        Ok(())
    }

    /// Opens Preferences with Ctrl+, flips the setting `name` names, and closes the dialog.
    fn film_preference(&mut self, name: &str) -> Result<(), String> {
        uiprobe::view::press(self.ui.window(), "Ctrl+,")
            .map_err(|e| format!("open Preferences: {e:?}"))?;
        let prefs = self.ui.global::<PreferencesModel>();
        if !prefs.get_dialog_open() {
            return Err("Ctrl+, did not open Preferences".into());
        }
        match name {
            "high_contrast_changed" => {
                let old = prefs.get_high_contrast();
                let button = targets::run_label_start(&self.view(), "High Contrast.")
                    .ok_or("High Contrast switch is missing")?;
                self.click_and_invoke(&button)?;
                if prefs.get_high_contrast() == old {
                    return Err("High Contrast switch did not change".into());
                }
            }
            "large_handles_changed" => {
                let old = prefs.get_large_handles();
                let button = targets::run_label_start(&self.view(), "Larger Handles.")
                    .ok_or("Larger Handles switch is missing")?;
                self.click_and_invoke(&button)?;
                if prefs.get_large_handles() == old {
                    return Err("Larger Handles switch did not change".into());
                }
            }
            _ => {
                let old = prefs.get_ui_scale_index();
                let count = prefs.get_ui_scale_options().row_count();
                if count < 2 {
                    return Err("Interface Scale has no alternate value".into());
                }
                let combo = targets::run_label(&self.view(), "Interface Scale")
                    .ok_or("Interface Scale combo is missing")?;
                self.click_and_invoke(&combo)?;
                let key = if old == 0 { "Down" } else { "Up" };
                uiprobe::view::press(self.ui.window(), key)
                    .map_err(|e| format!("Interface Scale {key}: {e:?}"))?;
                uiprobe::view::press(self.ui.window(), "Return")
                    .map_err(|e| format!("Interface Scale Return: {e:?}"))?;
                if prefs.get_ui_scale_index() == old {
                    return Err("Interface Scale choice did not change".into());
                }
            }
        }
        uiprobe::view::press(self.ui.window(), "Escape")
            .map_err(|e| format!("close Preferences: {e:?}"))
    }

    /// The events of the viewport, the tier table, the panels and the settings; whether `name` is
    /// one of them.
    fn drive_view_event(&mut self, n: usize, name: &str) -> Result<bool, String> {
        match name {
            "palette_opened" => {
                return Err(
                    "palette_opened needs an authored script: a bare Ctrl+K would leave the palette open"
                        .into(),
                );
            }
            "high_contrast_changed" | "large_handles_changed" | "ui_scale_changed" => {
                self.film_preference(name)?;
            }
            "verdict_opened" => {
                let targets::Resolved::Found(spot) =
                    targets::resolve(&self.view(), "verdict_badge")
                else {
                    return Err("Overall Verdict badge is not visible".into());
                };
                self.click_and_invoke(&spot)?;
            }
            "facet_picked" | "tier_selected" => self.pick_crown_facet()?,
            "snap_toggled" => self.film_command("view.snap")?,
            "slice_started" => self.film_command("view.slice")?,
            "slice_drawn" => self.draw_slice()?,
            "slice_flipped" => self.press_label("Flip")?,
            "slice_cuts_stone" => self.cut_slice_in()?,
            "slice_symmetric_toggled" => self.press_label("Symmetric")?,
            "diagram_panel_enlarged" => {
                self.press_label("Pavilion")?;
                if self
                    .ui
                    .global::<indicatrix_cut::SolidPreviewModel>()
                    .get_diagram_enlarged_panel()
                    != 1
                {
                    return Err("Pavilion pill did not enlarge the diagram panel".into());
                }
            }
            "live_render_opened" => self.film_command("view.live_render")?,
            "tiers_multi_selected" => {
                // The Multi-Select lesson names its two tiers (Crown Main and Pavilion Main): they
                // are ticked by name, not by their row numbers, which depend on the template.
                self.press_label("Select")?;
                for name in ["Crown Main", "Pavilion Main"] {
                    let row = self.tier_row_number(name)?;
                    self.press_label(&format!("Add Tier {row} to the selection"))?;
                }
            }
            "variants_opened" => {
                self.film_inspector_tab(4)?;
                if self.ui.global::<VariantsModel>().get_view() == 1 {
                    self.press_label("Steps")?;
                }
                self.press_label("Variants")?;
            }
            "interface_mode_changed" => {
                let simple = self.ui.global::<PreferencesModel>().get_simple_mode();
                self.press_label(if simple {
                    "Advanced Interface"
                } else {
                    "Simple Interface"
                })?;
                if self.ui.global::<PreferencesModel>().get_simple_mode() == simple {
                    return Err("Interface Mode click did not change the checked mode".into());
                }
            }
            "raw_text_diff_shown" => {
                return Err(
                    "raw text comparison requires its complete edit/validate/compare/apply script"
                        .into(),
                );
            }
            "compare_opened" => self.film_compare_table(false)?,
            "compare_window_opened" => self.film_compare_table(true)?,
            "lighting_preset_chosen"
            | "design_lighting_saved"
            | "design_lighting_forgotten"
            | "custom_material_saved"
            | "coefficient_material_saved"
            | "cutting_step_marked"
            | "rough_planner_opened"
            | "rough_plan_finished"
            | "glossary_opened"
            | "shortcuts_opened"
            | "help_opened"
            | "cut_slider_rough"
            | "cut_slider_step"
            | "cut_slider_finished" => self.drive_extra_event(n, name)?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The 1-based table row of the tier called `name`, as the tick boxes' labels count the rows
    /// ("Add Tier 3 to the selection").
    fn tier_row_number(&self, name: &str) -> Result<i32, String> {
        let wanted = name.trim();
        self.ui
            .global::<EditorModel>()
            .get_tiers()
            .iter()
            .find(|row| row.name.trim().eq_ignore_ascii_case(wanted))
            .map(|row| row.index + 1)
            .ok_or_else(|| format!("tier {wanted:?} is not in the tier table"))
    }

    /// Films the switch to the Advanced interface (a captioned "Setup" chapter) when the app
    /// runs in Simple mode and the tutorial is not about the mode itself: its steps use
    /// advanced-only controls such as the "Exact Scale Value" Meets choice.
    pub(crate) fn advanced_prelude(&mut self, guide_id: &str) -> Result<(), String> {
        if matches!(guide_id, "welcome-tour" | "prefs-interface-mode") {
            return Ok(());
        }
        if !self.ui.global::<PreferencesModel>().get_simple_mode() {
            return Ok(());
        }
        self.rec.chapter("Setup");
        self.rec
            .caption("These steps use the Advanced interface; switch to it first.");
        self.press_label("Advanced Interface")?;
        if self.ui.global::<PreferencesModel>().get_simple_mode() {
            return Err("clicking Advanced Interface did not switch the interface mode".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.rec.caption("");
        Ok(())
    }
}
