//! Scenarios: the Edit tab -- New Design from a template, tier edits with undo and redo, Solve,
//! Edit as Text, the unsaved-changes guard and click-everything over the editor.
//!
//! The design-creation helpers here (`create_template`, `start_template`, `tier_signature`) are
//! shared with `files.rs`. They take the app or the window, not the scenario context, so the
//! click-everything engine can call them from its `prepare` closure.

use super::{
    Scenario, existing_install,
    main_window::{click, select_render_view},
};
use crate::support::{
    Ctx, HarnessApp,
    app::{pump, wait_until},
};
use indicatrix_cut::{EditorModel, HistoryModel, MainWindow, RawTextModel, TemplateGalleryModel};
use slint::{ComponentHandle, Model};
use slint_harness::AccessibleRole;
use std::time::Duration;

/// Which gallery card of the "Shapes" section to pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Card {
    /// The first card (the round brilliant).
    First,
    /// The last card of the section (a different outline).
    Last,
}

/// Shows the 3D tab's Edit sub-view.
pub(super) fn show_edit_tab(app: &HarnessApp) {
    select_render_view(app, 1);
}

/// Number of rows in the tier table.
pub(super) fn tier_count(ui: &MainWindow) -> usize {
    ui.global::<EditorModel>().get_tiers().row_count()
}

/// The unrounded angle text of tier row `index`, empty when there is no such row.
pub(super) fn tier_angle(ui: &MainWindow, index: usize) -> String {
    ui.global::<EditorModel>()
        .get_tiers()
        .row_data(index)
        .map(|row| row.angle_full.to_string())
        .unwrap_or_default()
}

/// Name, angle and indices of every tier row, joined: two designs with the same signature have
/// the same tier table.
pub(super) fn tier_signature(ui: &MainWindow) -> String {
    let tiers = ui.global::<EditorModel>().get_tiers();
    (0..tiers.row_count())
        .filter_map(|i| tiers.row_data(i))
        .map(|row| format!("{}|{}|{}", row.name, row.angle_full, row.indices_full))
        .collect::<Vec<_>>()
        .join(";")
}

/// Opens the New Design dialog (clicking "New Design...") and waits until the template gallery
/// has its cards, gears and materials. Returns whether it got that far.
pub(super) fn open_new_design_dialog(app: &HarnessApp) -> bool {
    let ui = app.ui();
    let editor = ui.global::<EditorModel>();
    if !editor.get_new_dialog_open() && !click(app, "New Design...") {
        // GAP: the command bar button is not reachable by label; the flag it sets is driven.
        editor.set_new_dialog_open(true);
        pump(100);
    }
    let gallery = ui.global::<TemplateGalleryModel>();
    wait_until(Duration::from_secs(8), || {
        gallery.get_shapes().row_count() > 0
            && gallery.get_gear_options().row_count() > 0
            && gallery.get_material_options().row_count() > 0
    })
}

/// Picks a card of the "Shapes" section by clicking it (its radio button is labelled
/// "<name>, <shape>"); drives the model when the card cannot be found. Returns whether a card
/// was picked.
fn pick_card(app: &HarnessApp, card: Card) -> bool {
    let ui = app.ui();
    let gallery = ui.global::<TemplateGalleryModel>();
    let shapes = gallery.get_shapes();
    let count = shapes.row_count();
    if count == 0 {
        return false;
    }
    let index = match card {
        Card::First => 0,
        Card::Last => count - 1,
    };
    let Some(item) = shapes.row_data(index) else {
        return false;
    };
    let label = if item.shape.is_empty() {
        item.name.to_string()
    } else {
        format!("{}, {}", item.name, item.shape)
    };
    let radio = app
        .main_view()
        .by_label(&label)
        .into_iter()
        .find(|e| e.accessible_role() == Some(AccessibleRole::RadioButton));
    if let Some(element) = radio {
        element.invoke_accessible_default_action();
    } else {
        // GAP: the card is not reachable by label; the two calls the card makes are driven.
        ui.global::<EditorModel>()
            .set_new_template_index(item.template_index);
        gallery.invoke_template_chosen(item.template_index);
    }
    pump(100);
    true
}

/// Opens the New Design dialog, picks `card` and clicks Create, without waiting for the result
/// (the unsaved-changes guard may answer instead of a new design). Returns whether Create was
/// clicked.
pub(super) fn start_template(app: &HarnessApp, card: Card) -> bool {
    open_new_design_dialog(app) && pick_card(app, card) && click(app, "Create")
}

/// [`start_template`] and waits for the new design: the dialog closed, a design installed with at
/// least one tier. Returns false when the guard asked about unsaved changes, or on timeout.
pub(super) fn create_template(app: &HarnessApp, card: Card) -> bool {
    if !start_template(app, card) {
        return false;
    }
    let ui = app.ui();
    let editor = ui.global::<EditorModel>();
    let _ = wait_until(Duration::from_secs(15), || {
        editor.get_unsaved_dialog_open()
            || (!editor.get_new_dialog_open()
                && editor.get_has_design()
                && editor.get_tiers().row_count() > 0)
    });
    pump(300);
    !editor.get_unsaved_dialog_open()
        && !editor.get_new_dialog_open()
        && editor.get_has_design()
        && editor.get_tiers().row_count() > 0
}

/// Selects tier row 0 and adds a tier next to it with the command bar's Duplicate button.
pub(super) fn duplicate_first_tier(app: &HarnessApp) {
    let ui = app.ui();
    let editor = ui.global::<EditorModel>();
    editor.set_selected_tier_index(0);
    pump(80);
    if !click(app, "Duplicate") {
        // GAP: Duplicate is not reachable by label; the callback it fires is driven.
        editor.invoke_duplicate_tier(0);
        pump(100);
    }
}

/// `EditorModel.inspector_tab` of the History tab.
const HISTORY_TAB: i32 = 4;

/// Makes the Inspector's History tab the showing one (its pill, then the model when the click did
/// not take) and waits until its list holds the "Start" row. The list is only filled while the
/// tab is on screen (`gui::editor::history_panel::refresh`), so every History check opens the
/// tab first. Returns whether the list got a row.
fn open_history_tab(app: &HarnessApp) -> bool {
    let ui = app.ui();
    let editor = ui.global::<EditorModel>();
    click(app, "History");
    if editor.get_inspector_tab() != HISTORY_TAB || editor.get_inspector_collapsed() {
        // GAP: the History pill did not select the tab; the two properties it sets are driven.
        editor.set_inspector_collapsed(false);
        editor.set_inspector_tab(HISTORY_TAB);
        pump(100);
    }
    let history = ui.global::<HistoryModel>();
    wait_until(Duration::from_secs(3), || {
        history.get_rows().row_count() >= 1
    })
}

/// Clicks Undo (or fires the callback when the button cannot be found).
fn undo(app: &HarnessApp) {
    if !click(app, "Undo") {
        app.ui().global::<EditorModel>().invoke_undo();
        pump(100);
    }
}

/// Clicks Redo (or fires the callback when the button cannot be found).
fn redo(app: &HarnessApp) {
    if !click(app, "Redo") {
        app.ui().global::<EditorModel>().invoke_redo();
        pump(100);
    }
}

/// The scenarios of this file.
pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "new_design_from_template",
            seed: existing_install,
            run: new_design_from_template,
        },
        Scenario {
            name: "edit_tiers_undo_redo",
            seed: existing_install,
            run: edit_tiers_undo_redo,
        },
        Scenario {
            name: "solve_and_verdict",
            seed: existing_install,
            run: solve_and_verdict,
        },
        Scenario {
            name: "edit_as_text_round_trip",
            seed: existing_install,
            run: edit_as_text_round_trip,
        },
        Scenario {
            name: "unsaved_changes_guard",
            seed: existing_install,
            run: unsaved_changes_guard,
        },
        Scenario {
            name: "editor_click_everything",
            seed: existing_install,
            run: editor_click_everything,
        },
    ]
}

/// Opens New Design, picks the first template and creates it. Proves the editor ends up holding
/// a design with at least one tier and the dialog closes; sweeps the dialog and the result.
fn new_design_from_template(ctx: &mut Ctx<'_>) {
    show_edit_tab(ctx.app);
    let opened = open_new_design_dialog(ctx.app);
    ctx.expect(
        opened,
        "new-design-dialog",
        "the New Design dialog did not open with template cards, gears and materials",
    );
    pump(300);
    ctx.check_health("New Design dialog open");
    ctx.sweep_main_at_sizes("New Design overlay");

    let created = create_template(ctx.app, Card::First);
    ctx.expect(
        created,
        "template-created",
        "Create from the first template did not install a design",
    );
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    ctx.expect(
        editor.get_has_design() && editor.get_tiers().row_count() >= 1,
        "template-tiers",
        format!(
            "has_design={}, tiers={}",
            editor.get_has_design(),
            editor.get_tiers().row_count()
        ),
    );
    ctx.expect(
        !editor.get_new_dialog_open(),
        "new-design-closed",
        "the New Design dialog is still open after Create",
    );
    // A design made from a template is unsaved until it is saved.
    ctx.expect(
        editor.get_is_dirty(),
        "new-design-dirty",
        "a design just created from a template is not marked as unsaved",
    );
    ctx.check_health("design created from a template");
    ctx.sweep_main_at_sizes("Edit tab with a template design");
}

/// Adds a tier, nudges an angle, then undoes twice and redoes once, watching the History tab.
/// Proves the tier count, the angle, the undo/redo flags and the History position follow.
fn edit_tiers_undo_redo(ctx: &mut Ctx<'_>) {
    show_edit_tab(ctx.app);
    let created = create_template(ctx.app, Card::First);
    if !ctx.expect(
        created,
        "template-design",
        "no design could be created from a template",
    ) {
        return;
    }
    let shown = open_history_tab(ctx.app);
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    let history = ui.global::<HistoryModel>();
    let tiers0 = tier_count(&ui);
    let angle0 = tier_angle(&ui, 0);
    let rows0 = history.get_rows().row_count();
    if !shown {
        ctx.report.note(format!(
            "edit_tiers_undo_redo: History tab selected = {}, inspector collapsed = {}, rows = {rows0}",
            editor.get_inspector_tab() == HISTORY_TAB,
            editor.get_inspector_collapsed()
        ));
    }
    ctx.expect(
        rows0 >= 1 && history.get_current_position() == 0,
        "history-start",
        format!(
            "fresh design: {rows0} history row(s), position {}",
            history.get_current_position()
        ),
    );

    duplicate_first_tier(ctx.app);
    ctx.wait_for("a tier is added", Duration::from_secs(5), || {
        tier_count(&ui) == tiers0 + 1
    });
    ctx.expect(
        editor.get_can_undo(),
        "undo-enabled",
        "Undo is not available after adding a tier",
    );
    editor.invoke_nudge_angle(0, 0.5);
    if ctx.wait_for("the angle changes", Duration::from_secs(5), || {
        tier_angle(&ui, 0) != angle0
    }) {
        crate::support::coverage::record_callback("EditorModel.nudge_angle");
    }
    ctx.wait_for("History reaches step 2", Duration::from_secs(5), || {
        history.get_current_position() == 2
    });
    ctx.expect(
        history.get_rows().row_count() >= rows0 + 2,
        "history-rows",
        format!(
            "two edits made, History has {} row(s) (started with {rows0})",
            history.get_rows().row_count()
        ),
    );
    ctx.check_health("two edits");
    ctx.sweep_main_at_sizes("History tab after two edits");

    undo(ctx.app);
    ctx.wait_for("undo restores the angle", Duration::from_secs(5), || {
        tier_angle(&ui, 0) == angle0
    });
    undo(ctx.app);
    ctx.wait_for(
        "undo removes the added tier",
        Duration::from_secs(5),
        || tier_count(&ui) == tiers0,
    );
    ctx.wait_for(
        "History is back at the start",
        Duration::from_secs(5),
        || history.get_current_position() == 0,
    );
    ctx.expect(
        editor.get_can_redo(),
        "redo-enabled",
        "Redo is not available after two undos",
    );
    redo(ctx.app);
    ctx.wait_for("redo brings the tier back", Duration::from_secs(5), || {
        tier_count(&ui) == tiers0 + 1
    });
    ctx.wait_for("History is at step 1", Duration::from_secs(5), || {
        history.get_current_position() == 1
    });
    ctx.check_health("undo and redo");
    ctx.sweep_main_at_sizes("History tab after undo and redo");
}

/// Makes the design stale (auto-solve off, a small angle nudge), presses Solve and waits for the
/// result. Proves the solve finishes in the "solved" state with facets and raises no error toast.
fn solve_and_verdict(ctx: &mut Ctx<'_>) {
    show_edit_tab(ctx.app);
    let created = create_template(ctx.app, Card::First);
    if !ctx.expect(
        created,
        "template-design",
        "no design could be created from a template",
    ) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    editor.set_auto_solve_budget_ms(0);
    pump(100);
    editor.invoke_nudge_angle(0, 0.1);
    pump(300);
    ctx.report.note(format!(
        "solve_and_verdict: state before Solve = {:?}",
        editor.get_solve_state().to_string()
    ));
    ctx.check_health("edit before Solve");

    if !click(ctx.app, "Solve") {
        // GAP: the Solve button is not reachable by label; the callback F5 fires is driven.
        editor.invoke_solve();
    }
    ctx.wait_for("Solve finishes", Duration::from_secs(30), || {
        !editor.get_solve_running() && editor.get_solve_state().as_str() == "solved"
    });
    let state = editor.get_solve_state().to_string();
    ctx.expect(
        state == "solved",
        "solve-verdict",
        format!(
            "solve_state is {state:?} after Solve; status: {}",
            editor.get_status_text()
        ),
    );
    ctx.expect(
        editor.get_facet_count() > 0,
        "solve-facets",
        format!("facet_count is {} after Solve", editor.get_facet_count()),
    );
    ctx.check_health("Solve");
    ctx.sweep_main_at_sizes("Edit tab after Solve");
}

/// Edit as Text: opens it, applies the unchanged text, then a broken text. Proves the unchanged
/// apply changes nothing and raises no error, the broken text is reported inside the dialog and
/// refused, and closing asks before dropping the edit.
fn edit_as_text_round_trip(ctx: &mut Ctx<'_>) {
    show_edit_tab(ctx.app);
    let created = create_template(ctx.app, Card::First);
    if !ctx.expect(
        created,
        "template-design",
        "no design could be created from a template",
    ) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let raw = ui.global::<RawTextModel>();
    let signature = tier_signature(&ui);
    raw.invoke_open();
    // The text is written after a background solve of the design (`native_io::resolve_solved_then`),
    // so the dialog is open and busy first. A load error ends the wait too: it is the finding.
    let ready = ctx.wait_for(
        "Edit as Text shows the text",
        Duration::from_secs(30),
        || {
            raw.get_is_open()
                && !raw.get_busy()
                && (!raw.get_text().is_empty() || !raw.get_load_error().is_empty())
        },
    );
    if !ready {
        ctx.report.note(format!(
            "edit_as_text_round_trip: is_open={}, busy={}, load_error={:?}",
            raw.get_is_open(),
            raw.get_busy(),
            raw.get_load_error()
        ));
        return;
    }
    crate::support::coverage::record_callback("RawTextModel.open");
    if !ctx.expect(
        raw.get_load_error().is_empty(),
        "raw-text-load",
        format!("the text could not be shown: {}", raw.get_load_error()),
    ) {
        return;
    }
    ctx.check_health("Edit as Text open");
    ctx.sweep_main_at_sizes("Edit as Text dialog");

    raw.invoke_apply();
    pump(300);
    ctx.expect(
        tier_signature(&ui) == signature,
        "raw-text-unchanged",
        "applying the unchanged text changed the design",
    );
    ctx.check_health("apply unchanged text");

    let broken = "this is not a cutting design\n";
    raw.set_text(broken.into());
    raw.invoke_text_edited(broken.into());
    if ctx.wait_for("the broken text is flagged", Duration::from_secs(8), || {
        raw.get_status_kind() == 2
    }) {
        crate::support::coverage::record_callback("RawTextModel.text_edited");
    }
    ctx.expect(
        !raw.get_can_apply() && !raw.get_status().is_empty(),
        "raw-text-problem",
        format!(
            "broken text: can_apply={}, status {:?}",
            raw.get_can_apply(),
            raw.get_status()
        ),
    );
    ctx.sweep_main_at_sizes("Edit as Text dialog with a problem");
    raw.invoke_apply();
    pump(300);
    ctx.expect(
        raw.get_is_open() && tier_signature(&ui) == signature,
        "raw-text-refused",
        "applying a broken text closed the dialog or changed the design",
    );

    // Closing with an unapplied edit asks first; "Discard Edits" then closes.
    raw.invoke_request_close();
    pump(100);
    ctx.expect(
        raw.get_confirm_discard(),
        "raw-text-close-asks",
        "closing with an unapplied edit did not ask",
    );
    if !click(ctx.app, "Discard Edits") {
        // GAP: the dialog's Discard Edits button is not reachable by label.
        raw.set_confirm_discard(false);
        raw.invoke_close();
        pump(100);
    }
    ctx.expect(
        !raw.get_is_open(),
        "raw-text-closed",
        "Edit as Text is still open after Discard Edits",
    );
    ctx.check_health("Edit as Text closed");
}

/// Makes the design dirty, then starts New Design. Proves the unsaved-changes confirm appears,
/// Cancel keeps the design untouched and Discard replaces it.
fn unsaved_changes_guard(ctx: &mut Ctx<'_>) {
    show_edit_tab(ctx.app);
    let created = create_template(ctx.app, Card::First);
    if !ctx.expect(
        created,
        "template-design",
        "no design could be created from a template",
    ) {
        return;
    }
    duplicate_first_tier(ctx.app);
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    pump(300);
    let edited = tier_signature(&ui);
    if !ctx.expect(
        editor.get_is_dirty(),
        "dirty-after-edit",
        "the design is not dirty after adding a tier",
    ) {
        return;
    }

    // Cancel keeps the design.
    start_template(ctx.app, Card::Last);
    let asked = ctx.wait_for(
        "the unsaved-changes confirm",
        Duration::from_secs(5),
        || editor.get_unsaved_dialog_open(),
    );
    if !asked {
        return;
    }
    ctx.sweep_main_at_sizes("Unsaved changes confirm");
    editor.set_unsaved_dialog_open(false);
    editor.invoke_unsaved_dialog_cancel();
    pump(300);
    ctx.expect(
        tier_signature(&ui) == edited && editor.get_is_dirty(),
        "guard-cancel-keeps",
        "Cancel on the unsaved-changes confirm changed or cleaned the design",
    );
    ctx.check_health("guard cancelled");

    // Discard replaces it.
    editor.set_new_dialog_open(false);
    pump(100);
    start_template(ctx.app, Card::Last);
    ctx.wait_for("the confirm again", Duration::from_secs(5), || {
        editor.get_unsaved_dialog_open()
    });
    if !click(ctx.app, "Discard") {
        // GAP: the confirm's Discard button is not reachable by label.
        editor.invoke_unsaved_dialog_discard();
    }
    ctx.wait_for("the confirm closes", Duration::from_secs(10), || {
        !editor.get_unsaved_dialog_open() && !editor.get_new_dialog_open()
    });
    pump(300);
    ctx.expect(
        tier_signature(&ui) != edited,
        "guard-discard-replaces",
        "Discard did not replace the edited design with the new template",
    );
    ctx.check_health("guard discarded");
    ctx.sweep_main_at_sizes("Edit tab after Discard");
}

/// The click-everything pass over the Edit tab holding a template design.
fn editor_click_everything(ctx: &mut Ctx<'_>) {
    let _stats = ctx.smoke_main(
        "Edit tab with a template design",
        &mut |app: &mut HarnessApp| {
            show_edit_tab(app);
            let _ = create_template(app, Card::First);
        },
    );
    ctx.check_health("editor click-everything");
}
