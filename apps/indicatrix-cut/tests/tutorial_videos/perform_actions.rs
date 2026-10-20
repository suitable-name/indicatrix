//! Secondary filmed widget routes for `Perform` recipes: steps ladder, mirroring, tier notes,
//! material selection, yield, solid view modes, inspector tabs, and toolbar commands.

use crate::{
    director::Film,
    pacing,
    scripts::pure_tables::PALETTE_COMMANDS,
    support::app::pump,
    targets::{self, Query, Resolved, Spot},
};
use indicatrix_cut::{CommandPaletteModel, EditorModel, ParametersModel, SolidPreviewModel};
use indicatrix_editor::guide::{
    CMD_ADOPT_ALL, CMD_COMPARE, CMD_DELETE, CMD_SOLVE, CMD_UNDO, StepSeries,
};
use slint::{ComponentHandle, Model};
use std::time::Duration;

const FIELD_PAUSE: Duration = Duration::from_millis(200);

/// The component that holds the Steps / Mirror panel. The Tier form in the inspector carries
/// fields with the same labels ("Index Positions"), so every lookup is scoped to it.
const TOOLBAR_COMPONENT: &str = "TierTableToolbar";

/// What the accessible label of the trash button of a tier row starts with (`TierTableRow`, the
/// `CompactIcon` whose hint reads "Remove this tier (Delete).").
const REMOVE_BUTTON: &str = "Remove this tier";

impl Film<'_> {
    /// The visible text input labelled `label` inside the Steps / Mirror toolbar panel.
    fn toolbar_input(&self, label: &str) -> Option<Spot> {
        let view = self.view();
        view.root
            .query_descendants()
            .match_inherits(TOOLBAR_COMPONENT)
            .find_first()?
            .query_descendants()
            .match_accessible_role(uiprobe::AccessibleRole::TextInput)
            .find_all()
            .into_iter()
            .filter(|element| element.accessible_label().is_some_and(|name| name == label))
            .map(|element| Spot::of(&element))
            .find(|spot| spot.usable(view.size))
    }

    /// The visible element labelled exactly `label` inside the Steps / Mirror toolbar panel.
    fn toolbar_label(&self, label: &str) -> Option<Spot> {
        let view = self.view();
        view.root
            .query_descendants()
            .match_inherits(TOOLBAR_COMPONENT)
            .find_first()?
            .query_descendants()
            .find_all()
            .into_iter()
            .filter(|element| element.accessible_label().is_some_and(|name| name == label))
            .map(|element| Spot::of(&element))
            .find(|spot| spot.usable(view.size))
    }

    /// Makes the Keep Linked tick box of the Steps / Mirror panel read `linked`, clicking it only
    /// when it reads the other way.
    fn set_keep_linked(&mut self, linked: bool) -> Result<(), String> {
        let checkbox = self
            .view()
            .root
            .query_descendants()
            .match_inherits(TOOLBAR_COMPONENT)
            .find_first()
            .ok_or("Steps / Mirror: the tier table toolbar is missing")?
            .query_descendants()
            .match_accessible_role(uiprobe::AccessibleRole::Checkbox)
            .find_all()
            .into_iter()
            .find(|element| {
                element
                    .accessible_label()
                    .is_some_and(|name| name == "Keep Linked")
            })
            .ok_or_else(|| "Steps / Mirror: Keep Linked checkbox is missing".to_owned())?;
        let checked = checkbox
            .accessible_checked()
            .ok_or_else(|| "Keep Linked has no checked-state readback".to_owned())?;
        if checked != linked {
            self.click_and_invoke(&targets::Spot::of(&checkbox))?;
        }
        if checkbox.accessible_checked() != Some(linked) {
            return Err(format!(
                "Keep Linked readback: wanted {linked}, got {:?}",
                checkbox.accessible_checked()
            ));
        }
        Ok(())
    }

    /// Films opening the Steps / Mirror toolbar panel, typing each step series field, and clicking Generate.
    pub(crate) fn film_steps(&mut self, series: &StepSeries) -> Result<(), String> {
        if self.ui.global::<EditorModel>().get_tier_table_collapsed() {
            let header = targets::run_label_start(&self.view(), "TIERS (")
                .ok_or("Tiers header is missing")?;
            self.click_and_invoke(&header)?;
        }
        pump(50);
        if self.toolbar_input("Ladder Name").is_none()
            && let Some(toggle) = targets::run_label(&self.view(), "Steps / Mirror")
        {
            self.click_and_invoke(&toggle)?;
            self.hold(FIELD_PAUSE)?;
        }
        let count_str = series.count.to_string();
        for (label, val) in [
            ("Ladder Name", series.name.as_str()),
            ("Start Angle in Degrees", series.start.as_str()),
            ("Step in Degrees", series.step.as_str()),
            ("Number of Tiers", count_str.as_str()),
            ("Index Positions", series.indices.as_str()),
            ("Anchor Scale Value", series.anchor.as_str()),
        ] {
            let spot = self
                .toolbar_input(label)
                .ok_or_else(|| format!("Steps / Mirror: missing field {label:?} in the toolbar"))?;
            self.click_and_type(&spot, val)?;
        }
        self.set_keep_linked(series.linked)?;
        let editor = self.ui.global::<EditorModel>();
        let before = editor.get_tiers().row_count();
        let generate = self
            .toolbar_label("Generate")
            .ok_or_else(|| "Steps / Mirror: Generate is missing in the toolbar".to_owned())?;
        self.click_and_invoke(&generate)?;
        pump(80);
        let tiers = editor.get_tiers();
        if tiers.row_count()
            != before + usize::try_from(series.count).map_err(|_| "invalid series count")?
        {
            return Err("Generate did not add exactly the requested tiers".into());
        }
        let start = series
            .start
            .parse::<f64>()
            .map_err(|_| "series start is not numeric")?;
        let increment = series
            .step
            .parse::<f64>()
            .map_err(|_| "series increment is not numeric")?;
        for rung in 0..series.count {
            let name = format!("{}{}", series.name, rung + 1);
            let rows: Vec<_> = tiers
                .iter()
                .filter(|row| row.name.as_str() == name)
                .collect();
            if rows.len() != 1 {
                return Err(format!(
                    "Generate: expected unique {name:?}, got {} rows",
                    rows.len()
                ));
            }
            let row = &rows[0];
            let angle = row
                .angle_full
                .parse::<f64>()
                .map_err(|_| format!("{name}: invalid angle"))?;
            let expected = f64::from(rung).mul_add(increment, start);
            if (angle - expected).abs() > 1e-8
                || (rung == 0 && !row.relation_text.is_empty())
                || (rung > 0 && series.linked == row.relation_text.is_empty())
            {
                return Err(format!(
                    "{name}: angle={}, expected={expected}, relation={:?}, Keep Linked={}",
                    row.angle_full, row.relation_text, series.linked
                ));
            }
        }
        Ok(())
    }

    /// Films mirroring a tier to the other block via the Steps / Mirror toolbar panel.
    pub(crate) fn film_mirror_tier(&mut self, tier: &str, suffix: &str) -> Result<(), String> {
        self.film_select_tier(tier)?;
        if self
            .toolbar_input("Name Suffix for the Mirrored Tier")
            .is_none()
            && let Some(toggle) = targets::run_label(&self.view(), "Steps / Mirror")
        {
            self.click_and_invoke(&toggle)?;
            self.hold(FIELD_PAUSE)?;
        }
        let spot = self
            .toolbar_input("Name Suffix for the Mirrored Tier")
            .ok_or("Mirror suffix input is missing in the toolbar")?;
        self.click_and_type(&spot, suffix)?;
        let btn = self
            .toolbar_label("Mirror")
            .ok_or("Mirror button is missing in the toolbar")?;
        self.click_and_invoke(&btn)?;
        pump(80);
        Ok(())
    }

    /// Films clicking "Remove Relation" on a tier.
    pub(crate) fn film_clear_relation(&mut self, tier: &str) -> Result<(), String> {
        let row = self.film_select_tier(tier)?;
        self.press_label("Remove Relation")?;
        pump(80);
        if self
            .ui
            .global::<EditorModel>()
            .get_tiers()
            .row_data(usize::try_from(row).map_err(|_| format!("{tier}: no row index"))?)
            .is_none_or(|row| !row.relation_text.is_empty())
        {
            return Err(format!("{tier}: Remove Relation did not free the tier"));
        }
        Ok(())
    }

    /// Films typing a tier note and clicking "Set Note".
    pub(crate) fn film_tier_note(&mut self, tier: &str, text: &str) -> Result<(), String> {
        self.film_select_tier(tier)?;
        let spot = self
            .input_spot("Note for This Tier")?
            .ok_or("tier note input is missing")?;
        self.click_and_type(&spot, text)?;
        let btn =
            targets::run_label(&self.view(), "Set Note").ok_or("Set Note button is missing")?;
        self.click_and_invoke(&btn)?;
        pump(80);
        Ok(())
    }

    /// Films typing a cheater offset and clicking "Set Cheater Offset".
    pub(crate) fn film_cheater_offset(&mut self, tier: &str, text: &str) -> Result<(), String> {
        self.film_select_tier(tier)?;
        let spot = self
            .input_spot("Cheater Offset in Degrees")?
            .ok_or("cheater offset input is missing")?;
        self.click_and_type(&spot, text)?;
        let btn = targets::run_label(&self.view(), "Set Cheater Offset")
            .ok_or("Set Cheater Offset button is missing")?;
        self.click_and_invoke(&btn)?;
        pump(80);
        Ok(())
    }

    /// Films choosing a material in the Stone tab of the Parameters dock.
    pub(crate) fn film_material(&mut self, name: &str) -> Result<(), String> {
        let params = self.ui.global::<ParametersModel>();
        if !params.get_open() || params.get_active_tab() != 0 {
            let btn = targets::dock_tab_button(&self.view(), "stone")
                .or_else(|| targets::run_label(&self.view(), "Edit in Stone Tab"))
                .ok_or("Stone tab button is missing")?;
            self.click_and_invoke(&btn)?;
            pump(100);
            if !params.get_open() || params.get_active_tab() != 0 {
                return Err("Stone tab click did not open the tab".into());
            }
            self.hold(pacing::STAGE_HOLD)?;
        }
        let combo =
            targets::run_label(&self.view(), "Material").ok_or("Material combo is missing")?;
        let editor = self.ui.global::<EditorModel>();
        let options = editor.get_material_combo_options();
        let wanted = name.trim();
        let index = (0..options.row_count())
            .find(|&at| {
                options
                    .row_data(at)
                    .is_some_and(|opt| opt.trim().eq_ignore_ascii_case(wanted))
            })
            .and_then(|at| i32::try_from(at).ok())
            .ok_or_else(|| format!("'{wanted}' is not in the Material list."))?;
        self.click_and_invoke(&combo)?;
        let current = editor.get_material_combo_index();
        let key = if index >= current { "Down" } else { "Up" };
        for _ in 0..(index - current).abs() {
            uiprobe::view::press(self.ui.window(), key)
                .map_err(|e| format!("Material {key}: {e:?}"))?;
        }
        uiprobe::view::press(self.ui.window(), "Return")
            .map_err(|e| format!("Material Return: {e:?}"))?;
        pump(100);
        if editor.get_material_combo_index() != index {
            return Err(format!(
                "Material combo selected {}, expected {index}",
                editor.get_material_combo_index()
            ));
        }
        self.hold(FIELD_PAUSE)
    }

    /// Films typing the Girdle Diameter in the Preform tab and clicking "Apply Yield Inputs".
    pub(crate) fn film_yield(&mut self, girdle_diameter_mm: f64) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        if editor.get_inspector_tab() != 1 || editor.get_inspector_collapsed() {
            let pill =
                targets::inspector_tab_pill(&self.view(), 1).ok_or("Preform tab is missing")?;
            self.click_and_invoke(&pill)?;
            pump(80);
            if editor.get_inspector_tab() != 1 || editor.get_inspector_collapsed() {
                return Err("Preform tab click did not open the inspector".into());
            }
            self.hold(pacing::STAGE_HOLD)?;
        }
        let text = girdle_diameter_mm.to_string();
        let spot = self
            .input_spot("Girdle Diameter (mm)")?
            .ok_or("Girdle Diameter input is missing")?;
        self.click_and_type(&spot, &text)?;
        let apply_spot = targets::run(
            &self.view(),
            &Query::Id("PreformTab::tip-button-yield-pending"),
        )
        .or_else(|| targets::run_label_start(&self.view(), "Apply Yield Inputs"));
        let spot = apply_spot.ok_or("Apply Yield Inputs button is missing")?;
        self.click_and_invoke(&spot)?;
        pump(100);
        Ok(())
    }

    /// Films clicking a solid preview mode pill (0 Solid, 1 Path-Traced, 2 Both, 3 Diagram, 4 Ray-Traced).
    pub(crate) fn film_view_mode(&mut self, mode: i32) -> Result<(), String> {
        let pill =
            targets::solid_view_mode_pill(&self.view(), mode).ok_or("view-mode pill is missing")?;
        self.click_and_invoke(&pill)?;
        if self.ui.global::<SolidPreviewModel>().get_view_mode() != mode {
            return Err(format!("view-mode pill did not select {mode}"));
        }
        Ok(())
    }

    /// Selects the actual tab and requires its state to change through that input.
    pub(crate) fn film_inspector_tab(&mut self, tab: i32) -> Result<(), String> {
        let pill =
            targets::inspector_tab_pill(&self.view(), tab).ok_or("inspector tab is missing")?;
        self.click_and_invoke(&pill)?;
        if self.ui.global::<EditorModel>().get_inspector_tab() != tab {
            return Err(format!("inspector tab did not select {tab}"));
        }
        Ok(())
    }

    /// Films clicking the visible toolbar/row button for a command and executing it.
    pub(crate) fn film_command(&mut self, id: &str) -> Result<(), String> {
        let palette_title = PALETTE_COMMANDS
            .iter()
            .find(|(command, _)| *command == id)
            .map(|&(_, title)| title);
        if let Some(title) = palette_title {
            let palette_model = self.ui.global::<CommandPaletteModel>();
            if !palette_model.get_open() {
                uiprobe::view::press(self.ui.window(), "Ctrl+K")
                    .map_err(|e| format!("open command palette: {e:?}"))?;
            }
            if !pacing::wait_real(pacing::START_TIMEOUT, || palette_model.get_open()) {
                return Err("Ctrl+K did not open the command palette".into());
            }
            let search = targets::text_input(&self.view(), "Search Commands")
                .ok_or("command palette search input is missing")?;
            self.click_and_type(&search, title)?;
            if !pacing::wait_real(pacing::START_TIMEOUT, || {
                palette_model.get_query() == title
                    && palette_model
                        .get_rows()
                        .iter()
                        .any(|row| row.title == title && row.reason.is_empty())
            }) {
                return Err(format!(
                    "palette command {title:?} did not become available"
                ));
            }
            // Scope the result to the palette: a title can also occur in a
            // menu or elsewhere behind its modal backdrop.
            let palette = self
                .view()
                .root
                .query_descendants()
                .match_inherits("CommandPalette")
                .find_first()
                .ok_or("command palette is not visible")?;
            let command = palette
                .query_descendants()
                .match_inherits("PaletteRowView")
                .find_all()
                .into_iter()
                .filter(|row| row.accessible_label().is_some_and(|label| label == title))
                .map(|row| Spot::of(&row))
                .find(|spot| spot.usable(self.view().size))
                .ok_or_else(|| format!("palette command {title:?} is not visible"))?;
            self.click_and_invoke(&command)?;
            if palette_model.get_open() {
                return Err(format!(
                    "clicking palette command {title:?} did not activate it"
                ));
            }
            self.hold(Duration::from_millis(500))?;
            if id == "help.manual" {
                self.attach_help_window()?;
            }
            return Ok(());
        }
        if id == CMD_SOLVE {
            let ui = self.ui;
            let _ = pacing::wait_real(Duration::from_secs(10), || {
                !ui.global::<EditorModel>().get_solve_running()
            });
        }
        if id == CMD_DELETE {
            return self.click_selected_row_remove();
        }
        let view = self.view();
        let resolved = |t: &str| match targets::resolve(&view, t) {
            Resolved::Found(s) => Some(s),
            _ => None,
        };
        let spot = match id {
            CMD_SOLVE => resolved("solve_button"),
            CMD_UNDO => targets::run_label(&view, "Undo"),
            CMD_ADOPT_ALL => targets::run_label(&view, "Adopt All"),
            "solve.snapshot" => resolved("snapshot_button"),
            CMD_COMPARE => resolved("compare_button"),
            "view.live_render" => targets::live_render_pill(&view),
            "view.snap" => resolved("snap_pill"),
            "view.slice" => resolved("slice_pill"),
            _ => None,
        };
        drop(view);
        let spot =
            spot.ok_or_else(|| format!("command {id:?}: required visible control missing"))?;
        self.click_and_invoke(&spot)?;
        pump(60);
        self.hold(Duration::from_millis(500))?;
        Ok(())
    }

    /// Films clicking the trash button of the selected tier's row, the last column of the table.
    ///
    /// The column is far beyond the right edge of the table of the default dock, so the table is
    /// turned sideways with the wheel until the button is in view ([`Self::reveal_table_button`]),
    /// the button is clicked, and the table is turned back. The row must be on screen and the
    /// button must sit on it (within one row height, and wholly inside the table's scrolling
    /// area); anything else fails, so a click can never land on another tier's button or beside
    /// the table. The goal of the step (the tier is gone) is checked by the director afterwards.
    fn click_selected_row_remove(&mut self) -> Result<(), String> {
        let selected = self.ui.global::<EditorModel>().get_selected_tier_index();
        if selected < 0 {
            return Err(format!("{REMOVE_BUTTON:?}: no tier row is selected"));
        }
        let row_prefix = format!("Tier {},", selected + 1);
        let row = targets::run_label_start(&self.view(), &row_prefix).ok_or_else(|| {
            format!("{REMOVE_BUTTON:?}: the selected tier row {row_prefix:?} is not on screen")
        })?;
        let (button, turns) = self
            .reveal_table_button(|film, area| {
                targets::label_start_near_y(&film.view(), REMOVE_BUTTON, row.center().1, row.h)
                    .filter(|spot| spot.in_view_of(area))
            })
            .map_err(|why| format!("{REMOVE_BUTTON:?} on the selected tier row: {why}"))?;
        self.click_and_invoke(&button)?;
        pump(60);
        self.hold(Duration::from_millis(500))?;
        self.unturn_table(turns)
    }
}
