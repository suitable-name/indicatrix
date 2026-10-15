//! Scenarios: the main window's keyboard shortcuts (`global_shortcuts` in `ui/app.slint`).
//!
//! `shortcut_chords` drives every chord from a state where it applies and checks its effect;
//! `shortcut_modal_and_text_guards` proves the two things a shortcut must NOT do: act while a
//! modal overlay is open, and steal plain digits from a text field.
//!
//! GAP: Ctrl+Meta variants (the same arms test `event.modifiers.meta`) are not repeated, and the
//! cutting-mode key table (`CuttingModeModel.handle_key`) is left to the cutting-mode lane.

use super::{Scenario, dialogs, existing_install};
use crate::support::{
    Ctx, SecondaryWindow,
    app::{pump, wait_until},
    coverage,
};
use indicatrix_cut::{
    CommandPaletteModel, EditorModel, MainWindow, ParametersModel, PreferencesModel,
    ShortcutsModel, SolidPreviewModel, harness_support,
};
use slint::ComponentHandle;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};

/// The scenarios of this file.
pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "shortcut_chords",
            seed: existing_install,
            run: shortcut_chords,
        },
        Scenario {
            name: "shortcut_modal_and_text_guards",
            seed: existing_install,
            run: shortcut_modal_and_text_guards,
        },
    ]
}

/// One chord and what it must do. The probe is a number read from the UI before and after; the
/// expectation compares the two.
struct Case {
    /// The chord, in `Ctx::press` syntax.
    chord: &'static str,
    /// The effect, in words, for the finding.
    effect: &'static str,
    /// The global callback this proves (recorded for the coverage gate), if any.
    record: Option<&'static str>,
    /// Brings the app to a state where the chord applies; `false`: the setup failed (already a
    /// finding) and the chord is skipped.
    setup: fn(&mut Ctx<'_>) -> bool,
    /// The number that shows the effect.
    probe: fn(&MainWindow) -> i64,
    /// Whether (before, after) shows the effect.
    expect: fn(i64, i64) -> bool,
}

const fn nothing(_: &mut Ctx<'_>) -> bool {
    true
}

fn flag(value: bool) -> i64 {
    i64::from(value)
}

fn design_hash(ui: &MainWindow) -> i64 {
    let mut hasher = DefaultHasher::new();
    dialogs::design_signature(ui).hash(&mut hasher);
    // Wrap-around is fine: only equality of two hashes matters.
    hasher.finish().cast_signed()
}

fn tab_to(ctx: &Ctx<'_>, tab: i32) -> bool {
    ctx.app.ui().set_active_tab(tab);
    true
}

fn view_mode_to(ctx: &Ctx<'_>, mode: i32) -> bool {
    ctx.app
        .ui()
        .global::<SolidPreviewModel>()
        .set_view_mode(mode);
    true
}

fn picker_cancel(_: &mut Ctx<'_>) -> bool {
    harness_support::queue_picker_cancel();
    true
}

fn after_edit(ctx: &mut Ctx<'_>) -> bool {
    dialogs::edit_design(ctx)
}

/// An edit, then one undo: the state in which a redo applies.
fn after_edit_and_undo(ctx: &mut Ctx<'_>) -> bool {
    if !dialogs::edit_design(ctx) {
        return false;
    }
    let ui = ctx.app.ui().clone_strong();
    let edited = dialogs::design_signature(&ui);
    ui.global::<EditorModel>().invoke_undo();
    ctx.wait_for(
        "the setup undo takes effect",
        Duration::from_secs(5),
        || dialogs::design_signature(&ui) != edited,
    )
}

fn open_preferences(ctx: &mut Ctx<'_>) -> bool {
    ctx.app
        .ui()
        .global::<PreferencesModel>()
        .set_dialog_open(true);
    pump(250);
    true
}

fn open_shortcuts(ctx: &mut Ctx<'_>) -> bool {
    ctx.app.ui().global::<ShortcutsModel>().set_open(true);
    pump(250);
    true
}

fn select_first_tier(ctx: &mut Ctx<'_>) -> bool {
    ctx.app
        .ui()
        .global::<EditorModel>()
        .set_selected_tier_index(0);
    true
}

/// How "solved" the design is: stale 0, solving 1, solved 2, failed 3.
fn solve_rank(ui: &MainWindow) -> i64 {
    match ui.global::<EditorModel>().get_solve_state().as_str() {
        "stale" => 0,
        "solving" => 1,
        "solved" => 2,
        _ => 3,
    }
}

fn parameters_state(ui: &MainWindow) -> i64 {
    let params = ui.global::<ParametersModel>();
    flag(params.get_open()) * 10 + i64::from(params.get_active_tab())
}

/// The whole table, one row per chord of `global_shortcuts`.
const CASES: &[Case] = &[
    Case {
        chord: "Ctrl+2",
        effect: "switches to the Cutting Table tab (1)",
        record: None,
        setup: |ctx| tab_to(ctx, 0),
        probe: |ui| i64::from(ui.get_active_tab()),
        expect: |_, after| after == 1,
    },
    Case {
        chord: "Ctrl+3",
        effect: "switches to the Attachments tab (2)",
        record: None,
        setup: |ctx| tab_to(ctx, 0),
        probe: |ui| i64::from(ui.get_active_tab()),
        expect: |_, after| after == 2,
    },
    Case {
        chord: "Ctrl+1",
        effect: "switches back to the 3D Gem tab (0)",
        record: None,
        setup: |ctx| tab_to(ctx, 2),
        probe: |ui| i64::from(ui.get_active_tab()),
        expect: |_, after| after == 0,
    },
    Case {
        chord: "Ctrl+K",
        effect: "opens the command palette",
        record: Some("CommandPaletteModel.opened"),
        setup: nothing,
        probe: |ui| flag(ui.global::<CommandPaletteModel>().get_open()),
        expect: |before, after| before == 0 && after == 1,
    },
    Case {
        chord: "Ctrl+Shift+P",
        effect: "opens the command palette",
        record: None,
        setup: nothing,
        probe: |ui| flag(ui.global::<CommandPaletteModel>().get_open()),
        expect: |before, after| before == 0 && after == 1,
    },
    Case {
        chord: "Ctrl+,",
        effect: "opens Preferences",
        record: None,
        setup: nothing,
        probe: |ui| flag(ui.global::<PreferencesModel>().get_dialog_open()),
        expect: |before, after| before == 0 && after == 1,
    },
    Case {
        chord: "F1",
        effect: "opens the Help window",
        record: Some("HelpModel.open_context_help"),
        setup: nothing,
        probe: |_| flag(harness_support::is_window_open(SecondaryWindow::Help)),
        expect: |before, after| before == 0 && after == 1,
    },
    Case {
        chord: "?",
        effect: "opens the keyboard shortcuts overlay",
        record: None,
        setup: nothing,
        probe: |ui| flag(ui.global::<ShortcutsModel>().get_open()),
        expect: |before, after| before == 0 && after == 1,
    },
    Case {
        chord: "Ctrl+E",
        effect: "toggles the 3D Gem tab between Live Render and Edit",
        record: None,
        setup: nothing,
        probe: |ui| i64::from(ui.get_render_view_tab()),
        expect: |before, after| after != before,
    },
    Case {
        chord: "Ctrl+Alt+1",
        effect: "shows the Parameters dock's Stone tab",
        record: None,
        setup: nothing,
        probe: parameters_state,
        expect: |_, after| after == 10,
    },
    Case {
        chord: "Ctrl+Alt+2",
        effect: "shows the Parameters dock's Lighting & Environment tab",
        record: None,
        setup: nothing,
        probe: parameters_state,
        expect: |_, after| after == 11,
    },
    Case {
        chord: "Ctrl+Alt+3",
        effect: "shows the Parameters dock's Render & Output tab",
        record: None,
        setup: nothing,
        probe: parameters_state,
        expect: |_, after| after == 12,
    },
    Case {
        chord: "Ctrl+N",
        effect: "opens the New Design dialog",
        record: None,
        setup: nothing,
        probe: |ui| flag(ui.global::<EditorModel>().get_new_dialog_open()),
        expect: |before, after| before == 0 && after == 1,
    },
    Case {
        chord: "Ctrl+F",
        effect: "pulses the tier filter's focus request on the Edit tab",
        record: None,
        setup: nothing,
        probe: |ui| i64::from(ui.global::<EditorModel>().get_filter_focus_pulse()),
        expect: |before, after| after > before,
    },
    Case {
        chord: "Ctrl+S",
        effect: "asks the file picker (a never-saved design: Save As)",
        record: Some("EditorModel.save_native"),
        setup: picker_cancel,
        probe: |_| i64::try_from(harness_support::pending_picker_answers()).unwrap_or(i64::MAX),
        expect: |before, after| before == 1 && after == 0,
    },
    Case {
        chord: "Ctrl+Shift+S",
        effect: "asks the file picker (Save As)",
        record: Some("EditorModel.save_native_as"),
        setup: picker_cancel,
        probe: |_| i64::try_from(harness_support::pending_picker_answers()).unwrap_or(i64::MAX),
        expect: |before, after| before == 1 && after == 0,
    },
    // Ctrl+O is not in the table: with an unsaved design it asks "discard the changes?" first
    // (`native_io::open_native`), and only Discard reaches the picker -- see `ctrl_o_opens_picker`.
    Case {
        chord: "Ctrl+Z",
        effect: "undoes the last edit (the design changes)",
        record: Some("EditorModel.undo"),
        setup: after_edit,
        probe: design_hash,
        expect: |before, after| after != before,
    },
    Case {
        chord: "Ctrl+Y",
        effect: "redoes the undone edit (the design changes)",
        record: Some("EditorModel.redo"),
        setup: after_edit_and_undo,
        probe: design_hash,
        expect: |before, after| after != before,
    },
    Case {
        chord: "Ctrl+Shift+Z",
        effect: "redoes the undone edit (the design changes)",
        record: None,
        setup: after_edit_and_undo,
        probe: design_hash,
        expect: |before, after| after != before,
    },
    Case {
        chord: "F5",
        effect: "solves the design (stale -> solved)",
        record: Some("EditorModel.solve"),
        setup: after_edit,
        probe: solve_rank,
        expect: |before, after| before != 2 && after == 2,
    },
    Case {
        chord: "Escape",
        effect: "closes the Preferences dialog",
        record: None,
        setup: open_preferences,
        probe: |ui| flag(ui.global::<PreferencesModel>().get_dialog_open()),
        expect: |before, after| before == 1 && after == 0,
    },
    Case {
        chord: "Escape",
        effect: "closes the keyboard shortcuts overlay",
        record: None,
        setup: open_shortcuts,
        probe: |ui| flag(ui.global::<ShortcutsModel>().get_open()),
        expect: |before, after| before == 1 && after == 0,
    },
    Case {
        chord: "Escape",
        effect: "clears the tier selection",
        record: None,
        setup: select_first_tier,
        probe: |ui| i64::from(ui.global::<EditorModel>().get_selected_tier_index()),
        expect: |_, after| after == -1,
    },
    Case {
        chord: "1",
        effect: "switches the Solid view to mode 0 (Edit tab only)",
        record: None,
        setup: |ctx| view_mode_to(ctx, 3),
        probe: |ui| i64::from(ui.global::<SolidPreviewModel>().get_view_mode()),
        expect: |_, after| after == 0,
    },
    Case {
        chord: "2",
        effect: "switches the Solid view to mode 1",
        record: None,
        setup: |ctx| view_mode_to(ctx, 0),
        probe: |ui| i64::from(ui.global::<SolidPreviewModel>().get_view_mode()),
        expect: |_, after| after == 1,
    },
    Case {
        chord: "3",
        effect: "switches the Solid view to mode 2",
        record: None,
        setup: |ctx| view_mode_to(ctx, 0),
        probe: |ui| i64::from(ui.global::<SolidPreviewModel>().get_view_mode()),
        expect: |_, after| after == 2,
    },
    Case {
        chord: "4",
        effect: "switches the Solid view to mode 3",
        record: None,
        setup: |ctx| view_mode_to(ctx, 0),
        probe: |ui| i64::from(ui.global::<SolidPreviewModel>().get_view_mode()),
        expect: |_, after| after == 3,
    },
];

/// Brings the window to the neutral baseline every chord starts from: no overlay, the 3D Gem tab
/// on its Edit view, Solid mode 0, the Parameters dock folded, no queued picker answer, the Help
/// window closed, and the keyboard back on the main scope.
fn reset(ctx: &Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    ui.global::<PreferencesModel>().set_dialog_open(false);
    ui.global::<ShortcutsModel>().set_open(false);
    let editor = ui.global::<EditorModel>();
    editor.set_new_dialog_open(false);
    editor.set_text_focus(false);
    editor.set_selected_tier_index(-1);
    let params = ui.global::<ParametersModel>();
    params.set_open(false);
    params.set_active_tab(0);
    ui.global::<SolidPreviewModel>().set_view_mode(0);
    ui.set_active_tab(0);
    indicatrix_cut::gui::help::close_help_window();
    harness_support::clear_picker_answers();
    pump(100);
    if ui.get_render_view_tab() != 1 {
        ui.set_render_view_tab(1);
        ui.invoke_render_view_tab_changed(1);
        pump(150);
    }
    // Closing the palette bumps the pulse that hands the keyboard back to the main scope.
    ui.global::<CommandPaletteModel>().invoke_close_palette();
    pump(150);
}

fn run_case(ctx: &mut Ctx<'_>, case: &Case) {
    reset(ctx);
    if !(case.setup)(ctx) {
        return;
    }
    pump(150);
    let ui = ctx.app.ui().clone_strong();
    let before = (case.probe)(&ui);
    ctx.press(case.chord);
    let held = wait_until(Duration::from_secs(3), || {
        (case.expect)(before, (case.probe)(&ui))
    });
    let after = (case.probe)(&ui);
    ctx.expect(
        held,
        "shortcut-effect",
        format!(
            "{} should {} (probe before {before}, after {after})",
            case.chord, case.effect
        ),
    );
    if held && let Some(key) = case.record {
        coverage::record_callback(key);
    }
    ctx.check_health(&format!("shortcut {}", case.chord));
}

/// Every chord of `global_shortcuts`, table-driven, from a template design on the Edit tab: the
/// tab, palette, preferences, help, shortcut-overlay, parameters, new-design and filter chords
/// open or switch what they name; Ctrl+S, Ctrl+Shift+S and Ctrl+O ask the file picker (answered
/// cancel; the queue drains); undo, redo and F5 change the design / solve it; Esc closes the top
/// overlay or clears the selection; the digits 1-4 switch the Solid view on the Edit tab.
fn shortcut_chords(ctx: &mut Ctx<'_>) {
    if !dialogs::create_template_design(ctx) {
        return;
    }
    ctx.check_health("template design for the shortcuts");
    for case in CASES {
        run_case(ctx, case);
    }
    ctrl_o_opens_picker(ctx);
    reset(ctx);
}

/// Ctrl+O on the (unsaved) template design: the "discard the unsaved changes?" confirm appears and
/// the picker is not asked yet; Discard then asks the file picker (answered cancel).
fn ctrl_o_opens_picker(ctx: &mut Ctx<'_>) {
    reset(ctx);
    harness_support::queue_picker_cancel();
    pump(100);
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    ctx.press("Ctrl+O");
    let asked = wait_until(Duration::from_secs(3), || editor.get_unsaved_dialog_open());
    ctx.expect(
        asked,
        "shortcut-effect",
        "Ctrl+O should ask whether to discard the unsaved design before opening a file",
    );
    if asked {
        editor.invoke_unsaved_dialog_discard();
    }
    let picked = wait_until(Duration::from_secs(3), || {
        harness_support::pending_picker_answers() == 0
    });
    ctx.expect(
        picked,
        "shortcut-effect",
        "Ctrl+O (after Discard) should ask the file picker (Open)",
    );
    if asked && picked {
        coverage::record_callback("EditorModel.open_native");
    }
    harness_support::clear_picker_answers();
    editor.set_unsaved_dialog_open(false);
    pump(150);
    ctx.check_health("shortcut Ctrl+O");
}

/// What a modal overlay swallows: with `open` showing, the main chords do nothing; Esc closes it.
fn modal_guard(
    ctx: &mut Ctx<'_>,
    name: &str,
    open: fn(&MainWindow),
    is_open: fn(&MainWindow) -> bool,
) {
    reset(ctx);
    let ui = ctx.app.ui().clone_strong();
    open(&ui);
    pump(250);
    ctx.expect(is_open(&ui), "modal-opens", format!("{name}: did not open"));
    ctx.press("Ctrl+2");
    ctx.press("Ctrl+N");
    ctx.press("Ctrl+K");
    ctx.press("3");
    let leaked = ui.get_active_tab() != 0
        || ui.global::<EditorModel>().get_new_dialog_open()
        || ui.global::<CommandPaletteModel>().get_open()
        || ui.global::<SolidPreviewModel>().get_view_mode() != 0;
    ctx.expect(
        !leaked,
        "modal-swallows-chords",
        format!(
            "{name} is open, yet Ctrl+2 / Ctrl+N / Ctrl+K / 3 reached the main window's shortcuts"
        ),
    );
    ctx.press("Escape");
    pump(150);
    ctx.expect(
        !is_open(&ui),
        "modal-closes",
        format!("{name}: Esc did not close it"),
    );
    ctx.check_health(&format!("{name} swallows main chords"));
}

/// A text field that has the keyboard keeps plain digits and "?" (they are typed, not
/// shortcuts).
fn text_field_guard(ctx: &mut Ctx<'_>) {
    reset(ctx);
    let ui = ctx.app.ui().clone_strong();
    let view = ctx.app.main_view();
    let Some(field) = view.try_find("Search the Library") else {
        ctx.expect(false, "text-field", "no library search field to type into");
        return;
    };
    dialogs::click_center(&ui, &field);
    ctx.press("3");
    ctx.press("?");
    pump(200);
    let typed = field
        .accessible_value()
        .map(|v| v.to_string())
        .unwrap_or_default();
    let mode = ui.global::<SolidPreviewModel>().get_view_mode();
    let overlay = ui.global::<ShortcutsModel>().get_open();
    ctx.expect(
        mode == 0 && !overlay,
        "text-field-keeps-keys",
        format!("typing 3 and ? in the library search switched the Solid view to {mode} or opened the shortcuts ({overlay})"),
    );
    ctx.expect(
        typed.contains('3'),
        "text-field-types",
        format!("the library search holds {typed:?} after typing 3 and ?"),
    );
    field.set_accessible_value("");
    pump(300);
    ui.global::<ShortcutsModel>().set_open(false);
    ctx.check_health("text field keeps digits");
}

/// Two guards. A modal overlay (Preferences, the shortcuts overlay) swallows the main chords
/// (Ctrl+2, Ctrl+N, Ctrl+K, a digit) and closes on Esc. A focused text field (the library
/// search, focused with a pointer click) keeps "3" and "?" as typed text.
fn shortcut_modal_and_text_guards(ctx: &mut Ctx<'_>) {
    if !dialogs::create_template_design(ctx) {
        return;
    }
    modal_guard(
        ctx,
        "Preferences",
        |ui| ui.global::<PreferencesModel>().set_dialog_open(true),
        |ui| ui.global::<PreferencesModel>().get_dialog_open(),
    );
    modal_guard(
        ctx,
        "Keyboard shortcuts overlay",
        |ui| ui.global::<ShortcutsModel>().set_open(true),
        |ui| ui.global::<ShortcutsModel>().get_open(),
    );
    text_field_guard(ctx);
    reset(ctx);
}
