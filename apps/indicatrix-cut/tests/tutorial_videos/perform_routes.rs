//! Filmed widget routes for `Perform` recipes: instead of pressing the Guide Panel's Next button
//! (which silently commits in Rust), the cursor moves to each control, focuses fields with a
//! click, types text character-by-character, and clicks the real commit buttons.

use crate::{
    director::{Film, snap},
    pacing,
    perform_inputs::FIELD_PAUSE,
    routes::trace_running,
    support::app::pump,
    targets::{self, Query, Spot},
};
use indicatrix_cut::{EditorModel, GuideModel, MainWindow, TemplateGalleryModel};
use indicatrix_cut_core::{FreshDesignSpec, PreformShape, design::ConcaveTier};
use indicatrix_editor::{
    guide::{Perform, TierEntry},
    loading::{ConcaveTierFormFields, concave_tier_form_fields},
    material::gear_index_from_teeth,
};
use slint::{ComponentHandle, Model};
use std::time::Instant;

/// What a tier row shows: name, angle, indices, Meets kind and its text.
type TierRowValues = (String, String, String, i32, String);

/// The values of tier row `at` of the tier table.
fn tier_row_values(ui: &MainWindow, at: i32) -> Option<TierRowValues> {
    let at = usize::try_from(at).ok()?;
    ui.global::<EditorModel>()
        .get_tiers()
        .row_data(at)
        .map(|row| {
            (
                row.name.to_string(),
                row.angle_full.to_string(),
                row.indices_full.to_string(),
                row.constraint_kind,
                row.constraint_text.to_string(),
            )
        })
}

impl Film<'_> {
    /// Runs `perform` visibly through the application's widgets, waits for step `n` to complete,
    /// holds the result on screen and advances.
    pub(crate) fn perform_step(
        &mut self,
        n: usize,
        perform: Option<&Perform>,
        label: &str,
    ) -> Result<(), String> {
        self.perform_n = n;
        if let Some(recipe) = perform {
            self.drive_perform(recipe)?;
        } else {
            return Err(format!(
                "{label}: no authored filmed action; GuideModel.go_on is forbidden"
            ));
        }
        self.wait_after_perform(n, label)
    }

    fn wait_after_perform(&mut self, n: usize, label: &str) -> Result<(), String> {
        let ui = self.ui;
        pump(40);
        let mut gave_up_at: Option<Instant> = None;
        let mut question = false;
        let met = pacing::wait_lapse(&mut self.rec, pacing::STEP_TIMEOUT, || {
            let s = snap(ui);
            if !s.at(n) || s.done {
                return true;
            }
            if ui.global::<EditorModel>().get_unsaved_dialog_open() {
                question = true;
                return true;
            }
            if s.failed && !s.busy {
                let since = *gave_up_at.get_or_insert_with(Instant::now);
                return since.elapsed() >= pacing::GIVE_UP_GRACE;
            }
            false
        })?;
        if question {
            return Err(format!("{label}: the unsaved-changes question appeared"));
        }
        let end = snap(ui);
        if !met || (end.at(n) && !end.done) {
            let err = ui.global::<EditorModel>().get_tier_form_error();
            let tiers = ui.global::<EditorModel>().get_tiers();
            let count = tiers.row_count();
            let first = tiers.row_data(0).map(|r| {
                format!(
                    "name={:?},angle={:?},idx={:?},kind={},ctext={:?}",
                    r.name.as_str(),
                    r.angle_full.as_str(),
                    r.indices_full.as_str(),
                    r.constraint_kind,
                    r.constraint_text.as_str()
                )
            });
            let why = if gave_up_at.is_some() {
                "the recipe ran, gave up and the goal was never met"
            } else {
                "the goal was not met in time"
            };
            return Err(format!(
                "{label}: {why} (tiers={count}, row0={first:?}, form_err={err:?}, waiting={:?})",
                ui.global::<GuideModel>().get_current().waiting
            ));
        }
        let hold = if trace_running(ui) {
            let _ = pacing::wait_real(pacing::RENDER_CAP, || !trace_running(ui));
            pump(300);
            pacing::RENDER_HOLD
        } else {
            pacing::RESULT_HOLD
        };
        if snap(ui).at(n) {
            self.rec.hold(hold)?;
        }
        self.advance_step(n)
    }

    /// Dispatches one `Perform` variant to its filmed widget route.
    fn drive_perform(&mut self, perform: &Perform) -> Result<(), String> {
        match perform {
            Perform::NewDesign {
                template_index,
                spec,
            } => self.film_new_design(*template_index, spec),
            Perform::Tier(entry) => self.film_tier(entry),
            Perform::ConcaveTier { edit, tier } => self.film_concave_tier(edit.as_deref(), tier),
            Perform::SelectTier(name) => self.film_select_tier(name).map(|_| ()),
            Perform::Steps(series) => self.film_steps(series),
            Perform::MirrorTier { tier, suffix } => self.film_mirror_tier(tier, suffix),
            Perform::ClearRelation(tier) => self.film_clear_relation(tier),
            Perform::TierNote { tier, text } => self.film_tier_note(tier, text),
            Perform::CheaterOffset { tier, text } => self.film_cheater_offset(tier, text),
            Perform::Material(name) => self.film_material(name),
            Perform::Yield { girdle_diameter_mm } => self.film_yield(*girdle_diameter_mm),
            Perform::ViewMode(mode) => self.film_view_mode(*mode),
            Perform::InspectorTab(tab) => self.film_inspector_tab(*tab),
            Perform::Command(id) => self.film_command(id),
            Perform::Sequence(parts) => {
                for part in parts {
                    self.drive_perform(part)?;
                    pump(30);
                }
                Ok(())
            }
        }
    }

    /// Films opening the New Design dialog, filling the Empty or Template form, and clicking Create.
    #[expect(
        clippy::too_many_lines,
        reason = "one linear route through the New Design dialog"
    )]
    fn film_new_design(
        &mut self,
        template_index: i32,
        spec: &FreshDesignSpec,
    ) -> Result<(), String> {
        if !self.ui.global::<EditorModel>().get_new_dialog_open() {
            let file = targets::file_button(&self.view()).ok_or("File menu is missing")?;
            self.click_and_invoke(&file)?;
            pump(100);
            self.rec.hold(pacing::MENU_HOLD)?;
            let item = targets::menu_item(&self.view(), "New Design...")
                .ok_or("New Design menu item is missing")?;
            self.click_and_invoke(&item)?;
            pump(150);
            if !self.ui.global::<EditorModel>().get_new_dialog_open() {
                return Err("New Design menu click did not open the dialog".into());
            }
            self.rec.hold(pacing::STAGE_HOLD)?;
        }

        if template_index == 0 {
            let empty_card = targets::run_label_start(&self.view(), "Empty")
                .or_else(|| targets::run_label_start(&self.view(), "Blank"))
                .ok_or("Empty design card is missing")?;
            self.click_and_invoke(&empty_card)?;
            if self.ui.global::<EditorModel>().get_new_template_index() != 0 {
                return Err("Empty card did not select the empty template".into());
            }
            pump(80);
            self.rec.hold(FIELD_PAUSE)?;

            let shape_index = match spec.preform.shape {
                PreformShape::Block | PreformShape::Planes(_) => 0,
                PreformShape::Cylinder { .. } => 1,
            };
            let current_shape_index = self
                .ui
                .global::<EditorModel>()
                .get_new_preform_shape_index();
            if current_shape_index != shape_index {
                let shape_combo = targets::run_label(&self.view(), "Preform Shape")
                    .ok_or("Preform Shape combo is missing")?;
                self.click_and_invoke(&shape_combo)?;
                let key = if shape_index > current_shape_index {
                    "Down"
                } else {
                    "Up"
                };
                uiprobe::view::press(self.ui.window(), key)
                    .map_err(|e| format!("Preform Shape {key}: {e:?}"))?;
                uiprobe::view::press(self.ui.window(), "Return")
                    .map_err(|e| format!("Preform Shape Return: {e:?}"))?;
            }
            pump(60);
            self.rec.hold(FIELD_PAUSE)?;

            let ed = self.ui.global::<EditorModel>();
            for (cur, val, label) in [
                (
                    ed.get_new_preform_half_width().to_string(),
                    spec.preform.half_width.to_string(),
                    "Half-Width of the Rough",
                ),
                (
                    ed.get_new_preform_length_over_width().to_string(),
                    spec.preform.length_over_width.to_string(),
                    "Length over Width of the Rough",
                ),
                (
                    ed.get_new_preform_depth().to_string(),
                    spec.preform.depth.to_string(),
                    "Depth of the Rough",
                ),
            ] {
                if cur != val {
                    let spot = targets::text_input(&self.view(), label)
                        .ok_or_else(|| format!("New Design field {label:?} is missing"))?;
                    self.click_and_type(&spot, &val)?;
                }
            }
            let gear_index = gear_index_from_teeth(spec.gear_teeth);
            if ed.get_new_gear_index() != gear_index {
                let combo = targets::run_label(&self.view(), "Index Gear")
                    .ok_or("Index Gear combo is missing")?;
                self.click_and_invoke(&combo)?;
                let current = ed.get_new_gear_index();
                let key = if gear_index >= current { "Down" } else { "Up" };
                for _ in 0..(gear_index - current).abs() {
                    uiprobe::view::press(self.ui.window(), key)
                        .map_err(|e| format!("Index Gear {key}: {e:?}"))?;
                }
                uiprobe::view::press(self.ui.window(), "Return")
                    .map_err(|e| format!("Index Gear Return: {e:?}"))?;
            }
            if gear_index == 6 {
                let custom = spec.gear_teeth.to_string();
                let spot = targets::text_input(&self.view(), "Teeth on the Custom Gear")
                    .ok_or("Custom Gear teeth input is missing")?;
                self.click_and_type(&spot, &custom)?;
            }
            let sym = spec.symmetry_order.to_string();
            if self
                .ui
                .global::<EditorModel>()
                .get_new_symmetry_order_text()
                .as_str()
                != sym
            {
                let spot = targets::text_input(&self.view(), "Symmetry Order")
                    .ok_or("Symmetry Order input is missing")?;
                self.click_and_type(&spot, &sym)?;
            }
            if ed.get_new_mirror() != spec.mirror {
                let mirror = targets::run_label(&self.view(), "Mirror")
                    .ok_or("Mirror checkbox is missing")?;
                self.click_and_invoke(&mirror)?;
            }
            pump(60);
        } else {
            let gallery = self.ui.global::<TemplateGalleryModel>();
            let card = gallery
                .get_shapes()
                .iter()
                .chain(gallery.get_variants().iter())
                .chain(gallery.get_blank().iter())
                .find(|card| card.template_index == template_index)
                .ok_or_else(|| format!("template card {template_index} is not in the gallery"))?;
            let label = if card.shape.is_empty() {
                card.name.to_string()
            } else {
                format!("{}, {}", card.name, card.shape)
            };
            let spot = targets::run_label(&self.view(), &label)
                .ok_or_else(|| format!("template card {label:?} is not visible"))?;
            self.click_and_invoke(&spot)?;
            if self.ui.global::<EditorModel>().get_new_template_index() != template_index {
                return Err(format!(
                    "template card click did not select {template_index}"
                ));
            }
            pump(80);
            self.rec.hold(FIELD_PAUSE)?;
        }

        let create_btn =
            targets::run_label(&self.view(), "Create").ok_or("Create button is missing")?;
        self.click_and_invoke(&create_btn)?;
        pump(100);
        if self.ui.global::<EditorModel>().get_new_dialog_open() {
            return Err("Create click left the New Design dialog open".into());
        }
        Ok(())
    }

    /// Films adding or editing a flat tier in the Tier inspector tab:
    /// clicks `+ Add Tier` (or the tier row in the table), types each authored field
    /// character-by-character, and clicks `Add Tier` / `Save Tier`.
    pub(crate) fn film_tier(&mut self, entry: &TierEntry) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        if editor.get_inspector_collapsed() {
            let header = targets::run_label(&self.view(), "INSPECTOR")
                .ok_or("Inspector header is missing")?;
            self.click_and_invoke(&header)?;
        }
        if editor.get_inspector_tab() != 0 {
            self.film_inspector_tab(0)?;
        }
        if editor.get_tier_table_collapsed() {
            let header = targets::run_label_start(&self.view(), "TIERS (")
                .ok_or("Tiers header is missing")?;
            self.click_and_invoke(&header)?;
        }
        pump(60);
        self.scroll_inspector(3000.0)?;

        let mut edit_pos: Option<i32> = None;
        if let Some(target_name) = &entry.edit {
            let pos = self.film_select_tier(target_name)?;
            edit_pos = Some(pos);
            if let Some(discard_btn) = targets::run_label_start(&self.view(), "Discard Draft") {
                self.click_and_invoke(&discard_btn)?;
                pump(40);
            }
        } else {
            let add = targets::run_label(&self.view(), "+ Add Tier")
                .ok_or_else(|| "+ Add Tier is not visible".to_owned())?;
            self.click_and_invoke(&add)?;
            if editor.get_selected_tier_index() != -1 {
                return Err("+ Add Tier did not open a fresh form".into());
            }
            pump(80);
            self.rec.hold(FIELD_PAUSE)?;
        }

        if let Some(angle) = &entry.angle {
            let spot = self
                .input_spot("Angle in Degrees")?
                .ok_or_else(|| "required Angle in Degrees field is missing".to_owned())?;
            self.click_and_type(&spot, angle)?;
        }

        if let Some((kind, text)) = &entry.meets {
            self.film_tier_meets(*kind, text)?;
        }

        if let Some(name) = &entry.name {
            let spot = self
                .input_spot("Facet Name")?
                .ok_or("Facet Name input is missing")?;
            self.click_and_type(&spot, name)?;
        }

        if let Some(indices) = &entry.indices {
            let spot = self
                .input_spot("Index Positions")?
                .ok_or_else(|| "required Index Positions field is missing".to_owned())?;
            self.click_and_type(&spot, indices)?;
        }

        let tiers_before = editor.get_tiers().row_count();
        let row_before = edit_pos.and_then(|at| tier_row_values(self.ui, at));
        let label = if edit_pos.is_some() {
            "Save Tier"
        } else {
            "Add Tier"
        };
        let commit = targets::run_label(&self.view(), label)
            .ok_or_else(|| format!("tier form commit {label:?} is not visible"))?;
        self.click_and_invoke(&commit)?;
        pump(80);
        if editor.get_tiers().row_count() != tiers_before + usize::from(edit_pos.is_none()) {
            return Err(format!(
                "{label}: unexpected tier count after the real form commit"
            ));
        }
        if let Some(pos) = edit_pos {
            let after = tier_row_values(self.ui, pos)
                .ok_or_else(|| format!("{label}: edited row {pos} disappeared"))?;
            if let Some(before) = row_before
                && ((entry.name.is_none() && before.0 != after.0)
                    || (entry.angle.is_none() && before.1 != after.1)
                    || (entry.indices.is_none() && before.2 != after.2)
                    || (entry.meets.is_none() && (before.3 != after.3 || before.4 != after.4)))
            {
                return Err(format!(
                    "{label}: an omitted field changed: before={before:?}, after={after:?}"
                ));
            }
        }
        if !editor.get_tier_form_error().is_empty() {
            return Err(format!("{label}: {}", editor.get_tier_form_error()));
        }
        Ok(())
    }

    /// Films choosing the Meets kind `kind` in the Tier form and typing its value `text`.
    fn film_tier_meets(&mut self, kind: i32, text: &str) -> Result<(), String> {
        let meets_combo = targets::run(&self.view(), &targets::Query::Inherits("MeetsCombo"))
            .or_else(|| targets::run_label(&self.view(), "Meets"))
            .ok_or_else(|| "Meets selector is not visible".to_owned())?;
        self.click_and_invoke(&meets_combo)?;
        self.rec.hold(pacing::MENU_HOLD)?;
        let label = match kind {
            0 => "Unspecified Vertex",
            1 => "Named Facet(s)",
            2 => "Exact Scale Value",
            3 => "Cut to Depth (mm)",
            4 => "Girdle Thickness (mm)",
            5 => "Table Width (mm)",
            _ => return Err(format!("unknown Meets option {kind}")),
        };
        // The Slint testing backend does not expose popup-aware coordinates.
        // With the combo focused, keyboard navigation is the same visible user input.
        for _ in 0..6 {
            uiprobe::view::press(self.ui.window(), "Up").map_err(|e| format!("Meets Up: {e:?}"))?;
        }
        for _ in 0..kind {
            uiprobe::view::press(self.ui.window(), "Down")
                .map_err(|e| format!("Meets Down: {e:?}"))?;
        }
        uiprobe::view::press(self.ui.window(), "Return")
            .map_err(|e| format!("Meets Return: {e:?}"))?;
        if let Some(value) = meets_combo
            .handle
            .as_ref()
            .and_then(uiprobe::ElementHandle::accessible_value)
            && !value.contains(label)
        {
            return Err(format!(
                "Meets selection expected {label:?}, read back {value:?}"
            ));
        }
        pump(80);
        self.rec.hold(FIELD_PAUSE)?;

        if kind != 0 && !text.is_empty() {
            let mut value_spot = targets::run(
                &self.view(),
                &Query::Id("TierFormTab::line-edit-constraint-reader-label"),
            );
            for label in [
                "Facet Names This Facet Meets",
                "Exact Scale Value",
                "Value in Millimetres",
            ] {
                if value_spot.is_none() {
                    value_spot = self.input_spot(label)?;
                }
            }
            let spot =
                value_spot.ok_or_else(|| format!("Meets kind {kind}: value field is missing"))?;
            self.click_and_type(&spot, text)?;
        }
        Ok(())
    }

    /// Films selecting a tier row in the tier table by its name.
    pub(crate) fn film_select_tier(&mut self, name: &str) -> Result<i32, String> {
        let editor = self.ui.global::<EditorModel>();
        if editor.get_tier_table_collapsed() {
            let header = targets::run_label_start(&self.view(), "TIERS (")
                .ok_or("collapsed tier-table header is not visible")?;
            self.click_and_invoke(&header)?;
            pump(50);
        }
        let wanted = name.trim();
        let tiers = editor.get_tiers();
        let mut found_idx: Option<i32> = None;
        for i in 0..tiers.row_count() {
            if let Some(row) = tiers.row_data(i) {
                let matches = row.name.trim().eq_ignore_ascii_case(wanted)
                    || row
                        .name
                        .split('/')
                        .any(|part| part.trim().eq_ignore_ascii_case(wanted))
                    || row.code.trim().eq_ignore_ascii_case(wanted);
                if matches {
                    found_idx = i32::try_from(i).ok();
                    break;
                }
            }
        }
        let idx = found_idx.ok_or_else(|| format!("tier {wanted:?} not found in tier table"))?;
        let prefix = format!("Tier {},", idx + 1);
        let table = self
            .view()
            .root
            .query_descendants()
            .match_inherits("EditorTierTable")
            .find_first()
            .ok_or("tier table is not visible for scrolling")?;
        let scroll = table
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .ok_or("tier table has no ScrollView")?;
        let viewport = Spot::of(&scroll);
        let in_viewport = |spot: &Spot| {
            let (x, y) = spot.center();
            x >= viewport.x
                && x <= viewport.x + viewport.w
                && y >= viewport.y
                && y <= viewport.y + viewport.h
        };
        self.wheel_over(&viewport, 3000.0)?;
        self.rec.hold(FIELD_PAUSE)?;
        pump(60);
        let mut row_spot = targets::run_label_start(&self.view(), &prefix).filter(in_viewport);
        for _ in 0..12 {
            if row_spot.is_some() {
                break;
            }
            self.wheel_over(&viewport, -300.0)?;
            self.rec.hold(FIELD_PAUSE)?;
            pump(60);
            row_spot = targets::run_label_start(&self.view(), &prefix).filter(in_viewport);
        }
        let row_spot = row_spot.ok_or_else(|| {
            format!("tier {wanted:?}, row {idx}: row control is not visible after 12 scrolls")
        })?;
        self.click_and_invoke(&row_spot)?;
        pump(80);
        if editor.get_selected_tier_index() != idx {
            return Err(format!(
                "tier {wanted:?}: selection did not change to row {idx}"
            ));
        }
        self.rec.hold(FIELD_PAUSE)?;
        Ok(idx)
    }

    /// Opens the Tool Shape combo of the concave tier form and picks the tool coded `tool`
    /// (CYL, CON, CIR, DSC or SPH) with the keys.
    fn choose_concave_tool(&mut self, tool: &str) -> Result<(), String> {
        let tool_index = match tool {
            "CYL" => 0,
            "CON" => 1,
            "CIR" => 2,
            "DSC" => 3,
            "SPH" => 4,
            _ => return Err(format!("unknown concave tool {tool:?}")),
        };
        let shape = targets::run_label(&self.view(), "Tool Shape")
            .ok_or("concave Tool Shape combo is missing")?;
        self.click_and_invoke(&shape)?;
        for _ in 0..4 {
            uiprobe::view::press(self.ui.window(), "Up")
                .map_err(|e| format!("Tool Shape Up: {e:?}"))?;
        }
        for _ in 0..tool_index {
            uiprobe::view::press(self.ui.window(), "Down")
                .map_err(|e| format!("Tool Shape Down: {e:?}"))?;
        }
        uiprobe::view::press(self.ui.window(), "Return")
            .map_err(|e| format!("Tool Shape Return: {e:?}"))
    }

    /// Films adding or editing a concave tier in `ConcaveFormTab`.
    fn film_concave_tier(&mut self, edit: Option<&str>, tier: &ConcaveTier) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        if editor.get_inspector_collapsed() {
            let header = targets::run_label(&self.view(), "INSPECTOR")
                .ok_or("Inspector header is missing")?;
            self.click_and_invoke(&header)?;
        }
        if editor.get_inspector_tab() != 0 {
            self.film_inspector_tab(0)?;
        }
        if editor.get_tier_table_collapsed() {
            let header = targets::run_label_start(&self.view(), "TIERS (")
                .ok_or("Tiers header is missing")?;
            self.click_and_invoke(&header)?;
        }
        pump(60);

        if let Some(name) = edit {
            self.film_select_tier(name)?;
        } else {
            let btn = targets::run_label(&self.view(), "+ Add Concave Tier")
                .ok_or("+ Add Concave Tier button is missing")?;
            self.click_and_invoke(&btn)?;
            pump(80);
            self.rec.hold(FIELD_PAUSE)?;
        }

        let ConcaveTierFormFields {
            name,
            angle_deg,
            indices,
            instructions,
            tool,
            tool_azimuth_deg,
            x,
            y,
            z,
            diameter_ratio,
            tool_angle_deg,
            reciprocating,
        } = concave_tier_form_fields(tier);

        self.choose_concave_tool(&tool)?;

        for (label, val) in [
            ("Facet Name", name.as_str()),
            ("Angle in Degrees", angle_deg.as_str()),
            ("Index Positions", indices.as_str()),
            ("Cutting Instructions", instructions.as_str()),
            ("Tool Azimuth in Degrees", tool_azimuth_deg.as_str()),
            ("Tool Displacement X", x.as_str()),
            ("Tool Displacement Y", y.as_str()),
            ("Tool Displacement Z", z.as_str()),
            ("Tool Diameter over Stone Width", diameter_ratio.as_str()),
            ("Tool Included Angle in Degrees", tool_angle_deg.as_str()),
        ] {
            if !val.is_empty() {
                let spot = self
                    .input_spot(label)?
                    .ok_or_else(|| format!("concave field {label:?} is missing"))?;
                self.click_and_type(&spot, val)?;
            }
        }

        let motion = targets::run_label(&self.view(), "Reciprocating (Stroked Back and Forth)")
            .ok_or("Reciprocating checkbox is missing")?;
        let checked = motion
            .handle
            .as_ref()
            .and_then(uiprobe::ElementHandle::accessible_checked)
            .ok_or("Reciprocating checkbox has no checked-state readback")?;
        if checked != reciprocating {
            self.click_and_invoke(&motion)?;
        }
        if motion
            .handle
            .as_ref()
            .and_then(uiprobe::ElementHandle::accessible_checked)
            != Some(reciprocating)
        {
            return Err("Reciprocating click did not set the requested motion".into());
        }

        let commit = targets::run(
            &self.view(),
            &Query::Id("EditorInspector::tip-button-concave-loaded"),
        )
        .or_else(|| targets::run_label(&self.view(), "Add Concave Tier"))
        .or_else(|| targets::run_label(&self.view(), "Save Concave Tier"))
        .ok_or("concave form commit button is missing")?;

        self.click_and_invoke(&commit)?;
        pump(80);
        Ok(())
    }
}
