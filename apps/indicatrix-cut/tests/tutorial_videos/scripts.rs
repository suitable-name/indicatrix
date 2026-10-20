//! Explicit presentation actions, keyed by stable tutorial identity and step index.
//! Goals observe these actions; they are never decomposed into substitute edits.
//!
//! The registry is a list of `("guide-id", step) => ...` arms, split by lesson family into the
//! `script_*` methods below. The arms are read line by line by `pure_tables::scripted_keys`, so
//! each one starts its own line with its key; a step whose action takes more than a few lines
//! calls a method of one of the route modules instead.
use crate::{director::Film, pacing};
use indicatrix_editor::guide::{CMD_ADOPT_ALL, CMD_SOLVE, CMD_UNDO, TierEntry};

#[path = "scripts_actions.rs"]
mod actions;
#[path = "batch_routes.rs"]
mod batch_routes;
#[path = "chip_routes.rs"]
mod chip_routes;
#[path = "key_routes.rs"]
mod key_routes;
#[path = "lesson_routes.rs"]
mod lesson_routes;
#[path = "open_routes.rs"]
mod open_routes;
#[path = "optical_routes.rs"]
mod optical_routes;
#[path = "optimizing_routes.rs"]
mod optimizing_routes;
#[path = "pick_routes.rs"]
mod pick_routes;
/// The pure tables that pin the arms below to the catalogue (`pure_tables::SCRIPTED`).
#[path = "pure_tables.rs"]
pub mod pure_tables;
#[path = "raw_text_routes.rs"]
mod raw_text_routes;
#[path = "setup_routes.rs"]
mod setup_routes;
#[path = "table_routes.rs"]
mod table_routes;

impl Film<'_> {
    /// Returns false only when this step has no explicit script in this registry. Every arm of
    /// the `script_*` matches is listed, with its step title, in `pure_tables::SCRIPTED`, and the
    /// director checks both against the catalogue before it films: add an arm and its table row
    /// together.
    pub(crate) fn explicit_script(&mut self, n: usize) -> Result<bool, String> {
        if self.id == "viewing-optical-diagnostics" {
            optical_routes::drive(self, n)?;
            return Ok(true);
        }
        Ok(self.script_tiers(n)?
            || self.script_optimizing(n)?
            || self.script_solving(n)?
            || self.script_viewing(n)?)
    }

    /// The scripts of the tiers lessons, the command palette, the preform and the yield steps.
    fn script_tiers(&mut self, n: usize) -> Result<bool, String> {
        match (self.id.as_str(), n) {
            ("app-command-palette", 1) => self.palette_choose_view("diag", "Diagram View", 3)?,
            ("new-design-walkthrough", 8) => self.film_yield(6.5)?,
            ("solving-yield-carat", 2) => self.film_yield(8.0)?,
            // The design to adopt from is loaded from the library, as the step's actions offer.
            ("tiers-adopt-imported-meets", 0) => self.open_imported_library_design()?,
            // Any one row's Adopt button that is on view in the table; it repeats the label on
            // every imported row, and the table is turned sideways until one is in view
            // (`open_routes.rs`).
            ("tiers-adopt-imported-meets", 1) => self.adopt_one_imported_tier()?,
            ("tiers-adopt-imported-meets", 2) => self.film_command(CMD_ADOPT_ALL)?,
            ("solving-preform", 2) => self.film_preform_width()?,
            ("tiers-pin-to-mast", 1) => self.pin_solved_depth("Upper Girdle")?,
            ("tiers-pin-to-mast", 2) => self.film_command(CMD_UNDO)?,
            ("tiers-linked-series", 1) => self.inline_angle("Step1", "32")?,
            ("tiers-linked-series", 3) => {
                self.film_tier(&TierEntry::of("Step3").angle("=Step1+2"))?;
            }
            ("tiers-arithmetic", 1) => {
                // film_tier picks the Table row itself.
                self.film_tier(&TierEntry::of("Table").meets(2, "0.5-0.15"))?;
            }
            ("tiers-arithmetic", 2) => {
                self.film_select_tier("Crown Main")?;
                let field = self
                    .input_spot("Position of the Facet to Add")?
                    .ok_or_else(|| "facet-position input is not visible".to_owned())?;
                self.click_and_type(&field, "96/16")?;
                self.press_label("Add Facet")?;
            }
            ("tiers-arithmetic", 3) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            // The chips under Facets: the lesson teaches these controls, not a Tier form edit
            // that lands on the same indices (`chip_routes.rs`).
            ("tiers-facet-chips", 0) => self.add_chip_facet("Crown Main", 6.0, "6")?,
            ("tiers-facet-chips", 1) => self.remove_chip("Crown Main", 6.0)?,
            ("tiers-facet-chips", 2) => self.detach_chip("Crown Main", 84.0)?,
            ("tiers-facet-chips", 3) => self.remove_chip("Crown Main", 84.0)?,
            ("tiers-facet-chips", 4) => self.rotate_chips_up("Crown Main", "6")?,
            ("tiers-facet-chips", 5) => self.mirror_chips("Crown Main")?,
            // The Quick Add buttons of the bar above the table, not a Tier form that types the
            // same values (`table_routes.rs`).
            ("tiers-quick-add", 0) => self.quick_add_tier("Girdle")?,
            ("tiers-quick-add", 1) => self.quick_add_tier("Table")?,
            ("tiers-quick-add", 2) => self.quick_add_tier("Culet")?,
            // The ANGLE cell of the table itself: F2 and typing, the Up key, Ctrl and the wheel.
            ("tiers-inline-angle", 0) => self.inline_angle("Pavilion Main", "41")?,
            ("tiers-inline-angle", 1) => self.nudge_angle_with_keys("Crown Main", 2)?,
            ("tiers-inline-angle", 2) => self.nudge_angle_with_wheel("Crown Main", -2)?,
            ("tiers-step-series", 2) => self.inline_angle("Step2", "35")?,
            ("tiers-duplicate", 1) => self.inline_angle("Crown Main (2)", "30")?,
            ("tiers-relations", 2) => self.inline_angle("Crown Main", "36")?,
            // The Offset and Delete buttons of the bar above the table act on the whole ticked
            // group.
            ("tiers-multi-select", 1) => self.offset_ticked_tiers("1")?,
            ("tiers-multi-select", 2) => self.delete_ticked_tiers()?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The scripts of the Optimize, Retarget and Angle Sweep lessons.
    #[expect(
        clippy::match_same_arms,
        reason = "one arm per scripted step: pure_tables::scripted_keys reads the arms line by line"
    )]
    fn script_optimizing(&mut self, n: usize) -> Result<bool, String> {
        match (self.id.as_str(), n) {
            ("optimizing-optimize", 2) => self.optimize_choose_objective()?,
            ("optimizing-optimize", 3) => self.optimize_open_ranges()?,
            ("optimizing-optimize", 4) => self.optimize_run_search()?,
            ("optimizing-optimize", 5) => self.optimize_pick_candidate()?,
            ("optimizing-optimize", 6) => self.optimize_apply_candidate()?,
            ("optimizing-optimize", 7) => {
                self.film_command(CMD_SOLVE)?;
                self.hold(pacing::RESULT_HOLD)?;
                self.film_command(CMD_UNDO)?;
            }
            ("optimizing-retarget-shift", 1) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            ("optimizing-retarget-shift", 2) => self.retarget_shift_to_topaz()?,
            ("optimizing-retarget-shift", 3) => self.film_command(CMD_UNDO)?,
            ("optimizing-retarget-shift", 4) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            ("optimizing-retarget-optimize", 1) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            ("optimizing-retarget-optimize", 2) => self.retarget_optimize_to_sapphire()?,
            ("optimizing-retarget-optimize", 3) => self.film_command(CMD_UNDO)?,
            ("optimizing-retarget-optimize", 4) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            ("optimizing-sweep", 0) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            ("optimizing-sweep", 1) => self.sweep_pavilion_and_use()?,
            ("optimizing-sweep", 2) => self.film_command(CMD_UNDO)?,
            ("optimizing-sweep", 3) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The scripts of the history, variants, raw text, snapshot and verdict lessons.
    #[expect(
        clippy::match_same_arms,
        reason = "one arm per scripted step: pure_tables::scripted_keys reads the arms line by line"
    )]
    fn script_solving(&mut self, n: usize) -> Result<bool, String> {
        match (self.id.as_str(), n) {
            ("solving-raw-text", 3) => {
                self.open_raw_text()?;
                self.hold(pacing::RESULT_HOLD)?;
                self.dialog_button("Close")?;
            }
            ("solving-snapshot-compare", 1) => self.inline_angle("Crown Main", "36")?,
            ("solving-snapshot-compare", 4) => self.film_compare_read_close()?,
            ("solving-history", 1) => self.inline_angle("Crown Main", "36")?,
            ("solving-history", 2) => self.inline_angle("Pavilion Main", "41")?,
            ("solving-history", 3) => self.history_jump(1)?,
            ("solving-history", 4) => self.history_jump(0)?,
            ("solving-history", 5) => self.history_jump(2)?,
            ("solving-history", 6) => self.film_command(CMD_UNDO)?,
            ("solving-history", 7) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            // The previous step solved; this one only opens the verdict.
            ("solving-verdict-fixes", 3) => self.verdict_fix("Add a Table", false)?,
            ("solving-verdict-fixes", 4) => self.film_index_between_teeth()?,
            // The step's first action is "Click Solve", so the verdict is current.
            ("solving-verdict-fixes", 5) => self.verdict_fix("Snap to Teeth", true)?,
            ("solving-verdict-fixes", 6) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            ("solving-variants", 0) => self.open_variants_view()?,
            ("solving-variants", 1) => self.save_variant("Variant 1")?,
            ("solving-variants", 2) => self.inline_angle("Crown Main", "38")?,
            ("solving-variants", 3) => self.save_variant("Steeper crown")?,
            ("solving-variants", 4) => self.open_variant("Variant 1")?,
            ("solving-variants", 5) => self.film_command(CMD_UNDO)?,
            ("solving-variants", 6) => self.compare_variants()?,
            ("solving-variants", 7) => {
                self.delete_variant("Steeper crown")?;
                self.delete_variant("Variant 1")?;
            }
            ("solving-raw-text", 0) => self.film_command("solve.snapshot")?,
            ("solving-raw-text", 1) => self.edit_raw_text()?,
            ("solving-raw-text", 2) => self.film_command(CMD_UNDO)?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The scripts of the keyboard, picking, diagram, drag handle and Slice lessons, and of
    /// opening a file.
    #[expect(
        clippy::match_same_arms,
        reason = "one arm per scripted step: pure_tables::scripted_keys reads the arms line by line"
    )]
    fn script_viewing(&mut self, n: usize) -> Result<bool, String> {
        match (self.id.as_str(), n) {
            // The lesson teaches the keys themselves (`key_routes.rs`), not the pills, the palette
            // command or the Help menu that reach the same views.
            ("app-keyboard-shortcuts", 1) => self.press_view_key("4", 3)?,
            ("app-keyboard-shortcuts", 2) => self.press_view_key("1", 0)?,
            ("app-keyboard-shortcuts", 3) => self.swap_tabs_with_keys()?,
            ("app-keyboard-shortcuts", 4) => self.open_shortcuts_with_key()?,
            // The step says to click a facet of a different tier than the one step 2 picked (Crown
            // Main), which the event route's Crown Main facet would not be.
            ("viewing-solid-picking", 4) => self.pick_other_tier_facet("Crown Main")?,
            // The step says to click a row of the tier list; a click on a facet raises the same
            // event, but it is the other route.
            ("viewing-solid-picking", 5) => {
                self.film_select_tier("Pavilion Main")?;
            }
            // The step's own action: File > Open on the file the lesson saved, not Load Selected.
            ("output-open", 1) => self.open_saved_design()?,
            ("viewing-diagram", 3) => self.drag_diagram_angle_handle()?,
            ("viewing-drag-handles", 0) => self.pick_crown_facet()?,
            ("viewing-drag-handles", 1) => self.drag_handle(0, 70.0, false)?,
            ("viewing-drag-handles", 2) => self.film_command(CMD_UNDO)?,
            ("viewing-drag-handles", 3) => self.drag_handle(1, -70.0, false)?,
            ("viewing-drag-handles", 4) => self.drag_handle(2, 80.0, false)?,
            ("viewing-drag-handles", 5) => {
                self.film_command("view.snap")?;
                self.drag_handle_fine(0, 30.0)?;
            }
            ("viewing-drag-handles", 6) => self.film_command("view.snap")?,
            ("viewing-drag-handles", 7) => {
                self.drag_handle(0, 40.0, true)?;
                // The drag was cancelled with Escape inside `gesture`. This second Escape is the
                // lesson's "Escape with nothing being dragged clears the selection".
                self.key("Escape")?;
                for _ in 0..3 {
                    self.film_command(CMD_UNDO)?;
                }
            }
            ("viewing-slice", 0) => {
                self.film_view_mode(0)?;
                self.film_command("view.slice")?;
            }
            ("viewing-slice", 1) => self.draw_slice()?,
            ("viewing-slice", 2) => {
                self.press_label("Flip")?;
                self.hold(pacing::RESULT_HOLD)?;
                self.press_label("Flip")?;
            }
            ("viewing-slice", 3) => self.cut_slice_in()?,
            ("viewing-slice", 4) => self.film_symmetric_toggle()?,
            ("viewing-slice", 5) => self.keep_slice()?,
            ("viewing-slice", 6) => self.film_command(CMD_UNDO)?,
            ("viewing-slice", 7) => {
                self.hold(pacing::RESULT_HOLD)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
