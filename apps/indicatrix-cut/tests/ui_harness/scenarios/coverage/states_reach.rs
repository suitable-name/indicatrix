//! States of the click-through passes for the elements run 7 still left uncovered (lane E of the
//! 2026-10-15 findings), plus the helpers the other state tables share for them.
//!
//! Proves: in each state every enabled button, tab, check box and combo box of the scope is
//! pressed once and every field touched (the engine of `overlays.rs`), after the state's `open`
//! function reached the page the way a user does: model flags first, then the opener.
//!
//! What kept the elements unreached, by state:
//!
//! * the library list folds onto its rail at the passes' 1366 px width, so no list row (and none
//!   of its hover menu, tags or the filter bar) existed ([`library_list_open`]);
//! * dialogs whose body is a capped `ScrollView` (the tilt dialog, the material editor, the tier
//!   table sideways): a control scrolled out of the clip is not enumerated ([`scroll_scope`]);
//! * the export dialog reseeds its size and its lighting list every time it opens, so values set
//!   before the opener were gone;
//! * private flags that only a press sets (the Parameters reset question, the cutting-mode reset
//!   question, the video section) are reached by pressing their opener here.

use super::{
    super::{Piece, existing_install, main_window},
    overlay_states::{
        Base, NONE, State, deny, edit_tab, inspector, live_tab, open_cutting_mode, parameters,
        press,
    },
    overlays, state_pieces,
};
use crate::support::{
    HarnessApp,
    app::{pump, wait_until},
    coverage,
    query::is_clickable_role,
    smoke::{DenyMatch, DenyRule},
};
use indicatrix_cut::{
    BodyColorEditorModel, CuttingModeModel, DeepSolveTierRow, EditorModel, ExportModel,
    LibraryModel, LightingPresetItem, OptimizeModel, PerformanceFilterRow, RetargetModel,
    SettingsModel, TiltModel, TiltVideoExportModel, VerdictModel, VerdictReasonData, ViewportModel,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use slint_harness::AccessibleRole;
use std::time::Duration;

/// The pieces of this file: one `overlay:reach-*:<state>` piece per state.
pub fn pieces() -> Vec<Piece> {
    let groups: [super::StateGroup; 3] = [
        ("reach-library", library_states),
        ("reach-dialogs", dialog_states),
        ("reach-panels", panel_states),
    ];
    let mut all = Vec::new();
    for (group, states) in groups {
        all.extend(state_pieces(group, existing_install, states));
    }
    all
}

fn strings(items: &[&str]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .iter()
            .map(|item| SharedString::from(*item))
            .collect::<Vec<_>>(),
    ))
}

// ---------------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------------

/// Types `text` into the first text field of the main window labelled `label`: the field's
/// accessible set-value action (`text = v; edited(v)` in the standard `LineEdit`). `false` when
/// no such field is showing.
pub(super) fn type_into(app: &HarnessApp, label: &str, text: &str) -> bool {
    let view = app.main_view();
    let field = view
        .by_label(label)
        .into_iter()
        .find(|e| e.accessible_role() == Some(AccessibleRole::TextInput));
    let Some(field) = field else {
        return false;
    };
    coverage::record_touch(&field);
    coverage::record_wrappers(&view.root, &field, true);
    field.set_accessible_value(text);
    pump(150);
    true
}

/// Presses the first clickable element labelled `label` inside the component `scope` (a label
/// such as "Reset" also exists elsewhere in the window). `false` when there is none.
fn press_in_scope(app: &HarnessApp, scope: &str, label: &str) -> bool {
    let Some(view) = overlays::find_scope(app, scope) else {
        return false;
    };
    let Some(element) = view
        .by_label(label)
        .into_iter()
        .find(|e| e.accessible_role().is_some_and(is_clickable_role))
    else {
        return false;
    };
    coverage::record_activation(&element);
    coverage::record_wrappers(&view.root, &element, false);
    element.invoke_accessible_default_action();
    pump(250);
    true
}

/// Unfolds the library list when the window put it on its rail. At the passes' 1366 px width the
/// Edit view's auto-collapse rule (app.slint `library_wants_auto_collapse`) folds the library, and
/// `DiagramListView` (rows, hover menu, tags, filter bar) is then not instantiated at all. The
/// rail's own button is pressed (the property is the fallback), then the layout is nudged so the
/// list's rows are instantiated.
pub(super) fn library_list_open(app: &HarnessApp) {
    let library = app.ui().global::<LibraryModel>();
    if library.get_panel_collapsed() && !press(app, "Show the Library List", None) {
        library.set_panel_collapsed(false);
    }
    let window = app.ui().window();
    let size = window.size().to_logical(window.scale_factor());
    window.set_size(slint::LogicalSize::new(size.width, size.height + 1.0));
    pump(30);
    window.set_size(size);
    pump(250);
}

/// Scrolls the largest `ScrollView` inside the component `scope` by `(dx, dy)` with the mouse
/// wheel (negative: towards the end), so controls below or right of the clip are enumerated.
pub(super) fn scroll_scope(app: &HarnessApp, scope: &str, dx: f32, dy: f32) {
    let Some(view) = overlays::find_scope(app, scope) else {
        return;
    };
    let area = view
        .root
        .query_descendants()
        .match_inherits("ScrollView")
        .find_all()
        .into_iter()
        .max_by(|a, b| {
            let (sa, sb) = (a.size(), b.size());
            (sa.width * sa.height).total_cmp(&(sb.width * sb.height))
        });
    area.as_ref().unwrap_or(&view.root).scroll(dx, dy);
    pump(250);
}

/// Waits (real time, up to 8 s) until no solve runs, so a late refresh does not overwrite what a
/// state sets on the models afterwards.
fn wait_solved(app: &HarnessApp) {
    let editor = app.ui().global::<EditorModel>();
    let _ = wait_until(Duration::from_secs(8), || !editor.get_solve_running());
    pump(300);
}

// ---------------------------------------------------------------------------------------------
// Opens used by `overlay_states.rs`
// ---------------------------------------------------------------------------------------------

/// The Optimize tab with the custom objective (the last preset: the three weight boxes and the
/// Yield and Tone sliders), the angle ranges open, and a studio rig selected ("Change the
/// Lighting Rig..." shows under a studio rig only). The ranges table is built by Rust in
/// `ranges_toggled`, which the button calls after flipping `ranges_open`; setting the flag alone
/// left the table empty, so the Min and Max fields never existed.
pub(super) fn optimize_ranges_custom(a: &HarnessApp) {
    let viewport = a.ui().global::<ViewportModel>();
    if let Some(studio) = viewport
        .get_lighting_is_studio()
        .iter()
        .position(|studio| studio)
        .and_then(|index| i32::try_from(index).ok())
    {
        viewport.set_selected_lighting_index(studio);
    }
    inspector(a, 2);
    let optimize = a.ui().global::<OptimizeModel>();
    let last = i32::try_from(optimize.get_preset_labels().row_count()).unwrap_or(0) - 1;
    if last >= 0 {
        optimize.set_preset_index(last);
        optimize.invoke_preset_changed(last);
    }
    optimize.set_ranges_open(true);
    optimize.invoke_ranges_toggled(true);
    pump(250);
}

/// The verdict popover with a reason about a tier (its "Show") and a reason with a Fix. The badge
/// exists only once the first verdict landed (`VerdictModel.level >= 0`), which the old state did
/// not wait for, so it pressed nothing. The template's own reasons are kept; the two kinds of row
/// are added when the template has none (a Fix of a reason Rust does not hold answers with an
/// info toast, "There is no such reason.").
pub(super) fn verdict_with_reasons(a: &HarnessApp) {
    edit_tab(a);
    wait_solved(a);
    let verdict = a.ui().global::<VerdictModel>();
    let _ = wait_until(Duration::from_secs(10), || {
        verdict.get_level() >= 0 && !verdict.get_busy()
    });
    let mut reasons: Vec<VerdictReasonData> = verdict.get_reasons().iter().collect();
    if !reasons.iter().any(|r| r.tier >= 0) {
        reasons.push(VerdictReasonData {
            level: 1,
            text: "Tier 1 meets its neighbour at a shallow angle.".into(),
            tier: 0,
            fix_label: "".into(),
            fix_hint: "".into(),
        });
    }
    if !reasons.iter().any(|r| !r.fix_label.is_empty()) {
        reasons.push(VerdictReasonData {
            level: 1,
            text: "Tier 1 is not on a whole tooth.".into(),
            tier: 0,
            fix_label: "Snap to Teeth".into(),
            fix_hint: "Move the tier to the nearest whole tooth.".into(),
        });
    }
    verdict.set_reasons(ModelRc::new(VecModel::from(reasons)));
    if verdict.get_level() < 0 {
        verdict.set_level(1);
        verdict.set_word("Check".into());
    }
    verdict.set_stale(false);
    pump(200);
    let word = verdict.get_word();
    if press(a, &format!("Overall Verdict: {word}"), None) {
        coverage::record_site_id("EditorStatusStrip::verdict-badge-l628");
    }
}

/// "Pin" of the Deep Solve table: there is no Deep Solve run (a long job), so the press answers
/// "No verified mast recorded" as an error toast; it is made once by the open function instead.
pub(super) const DENY_PIN: &[DenyRule] = &[deny(
    DenyMatch::StartsWith,
    "pin tier",
    "needs a finished Deep Solve run (long job); pressed once by the state's open function",
)];

/// The status strip's Details popup with every section it can show filled: the Deep Solve
/// per-tier table (Pin), a manufacturability warning about tier 1 (a button that selects it), a
/// material suggestion (Set Material, No Thanks), and a saved file in the strip (its chip reveals
/// the folder; the harness build captures the opener). Rust fills these after a Deep Solve, a
/// solve with warnings, a catalogue load and a save; they are written directly here.
pub(super) fn status_details_filled(a: &HarnessApp) {
    edit_tab(a);
    wait_solved(a);
    let editor = a.ui().global::<EditorModel>();
    editor.set_deep_solve_tier_rows(ModelRc::new(VecModel::from(vec![DeepSolveTierRow {
        tier_number: "#1".into(),
        name: "".into(),
        before_mast: "1.0000".into(),
        after_mast: "1.0213".into(),
        delta: "+0.0213".into(),
    }])));
    editor.set_manufacturability_warnings(strings(&["Tier 1 has a facet narrower than 0.1 mm."]));
    editor.set_manufacturability_warning_tiers(ModelRc::new(VecModel::from(vec![0])));
    let material = a
        .ui()
        .global::<ViewportModel>()
        .get_material_options()
        .row_data(1)
        .unwrap_or_else(|| "Sapphire".into());
    editor.set_material_suggestion_name(material.clone());
    editor.set_material_suggestion_text(format!("The catalogue says {material}.").into());
    if let Ok(dir) = std::env::current_dir() {
        let saved = dir.join("harness_saved.isdd");
        editor.set_last_saved_path(saved.to_string_lossy().as_ref().into());
        editor.set_last_saved_label("harness_saved.isdd".into());
    }
    pump(250);
    press(a, "Details", None);
    if press(a, "Pin Tier #1", None) {
        a.ui().set_toast_visible(false);
    }
}

// ---------------------------------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------------------------------

fn library_states() -> Vec<State> {
    vec![
        State {
            name: "Files tab, attachments of the selected design",
            base: Base::Library,
            scope: Some("AttachmentsView"),
            deny: NONE,
            open: |a| {
                // An imported .asc keeps the file as an attachment: Save / Export writes it to
                // ./exports/ (the profile's working directory), Copy URL goes to the captured
                // clipboard.
                main_window::select_top_tab_model(a, 2);
                let library = a.ui().global::<LibraryModel>();
                let _ = wait_until(Duration::from_secs(10), || {
                    library.get_current_files().row_count() > 0
                });
                pump(250);
            },
        },
        State {
            name: "Filters popup, RI match, a tag and a tilt filter",
            base: Base::Library,
            scope: Some("FilterPanel"),
            deny: super::overlay_states::DENY_FILTERS,
            open: filters_filled,
        },
    ]
}

/// The Advanced Filters popup with the RI match on (its sliders and "Use Current Material"), a
/// tag in the vocabulary (its chip) and one tilt-performance filter (its remove button). The
/// header button says "Filters (A Filter Is On)" once a filter is on, which is why the old route
/// (the label "Filters") could not open it in this state.
fn filters_filled(a: &HarnessApp) {
    let library = a.ui().global::<LibraryModel>();
    if let Some(row) = library.get_diagram_list().row_data(0) {
        library.invoke_add_tag_to_entry(row.id, "harness".into());
        let _ = wait_until(Duration::from_secs(8), || {
            library.get_all_tags().row_count() > 0
        });
    }
    library.invoke_performance_filter_add(PerformanceFilterRow {
        metric_index: 2,
        bound_index: 0,
        threshold_pct: 20.0,
        tilt_radius_deg: 45.0,
        aggregate_index: 0,
    });
    library.set_ri_tolerance_enabled(true);
    library.invoke_filters_changed();
    let _ = wait_until(Duration::from_secs(5), || {
        library.get_performance_filters().row_count() > 0
    });
    pump(250);
    if !press(a, "Filters (A Filter Is On)", None) {
        press(a, "Filters", None);
    }
}

// ---------------------------------------------------------------------------------------------
// Dialogs
// ---------------------------------------------------------------------------------------------

const DENY_RECOVER: &[DenyRule] = &[deny(
    DenyMatch::Exact,
    "recover",
    "opens the recovery file, which is no design here (Recover is driven by startup_restore_prompt)",
)];

const DENY_COMPARE_WINDOW: &[DenyRule] = &[deny(
    DenyMatch::Contains,
    "compare visually",
    "GPU render: opens the Compare window, which renders both designs",
)];

fn dialog_states() -> Vec<State> {
    vec![
        State {
            name: "Write question with Don't Ask Again",
            base: Base::Design,
            scope: Some("ConfirmActionDialog"),
            deny: NONE,
            open: |a| {
                // The "not a closed solid, write anyway?" kind of question; nothing waits on
                // it, so Write Anyway and Cancel only close it.
                edit_tab(a);
                let editor = a.ui().global::<EditorModel>();
                editor.set_write_confirm_heading("Write Anyway?".into());
                editor.set_write_confirm_message("The stone is not a closed solid.".into());
                editor.set_write_confirm_primary_label("Write Anyway".into());
                editor.set_write_confirm_show_dont_ask(true);
                editor.set_write_confirm_open(true);
                pump(250);
            },
        },
        State {
            name: "Startup question about a leftover autosave",
            base: Base::Startup,
            scope: Some("ConfirmActionDialog"),
            deny: DENY_RECOVER,
            open: |a| {
                // Only a leftover AUTOSAVE offers Delete; the file is made in the profile.
                let Ok(dir) = std::env::current_dir() else {
                    return;
                };
                let path = dir.join("harness_autosave.isdd");
                let _ = std::fs::write(&path, "harness autosave");
                let editor = a.ui().global::<EditorModel>();
                editor.set_startup_restore_is_autosave(true);
                editor.set_startup_restore_path(path.to_string_lossy().as_ref().into());
                pump(250);
            },
        },
        State {
            name: "Cutting mode, second step after the reset question",
            base: Base::Design,
            scope: Some("CuttingModeScreen"),
            deny: NONE,
            open: cutting_second_step,
        },
        State {
            name: "Export dialog, custom size and a saved lighting",
            base: Base::Design,
            scope: Some("ExportDialog"),
            deny: NONE,
            open: export_custom_size,
        },
        State {
            name: "Compare to Snapshot",
            base: Base::Design,
            scope: Some("RetargetDialog"),
            deny: DENY_COMPARE_WINDOW,
            open: compare_to_snapshot,
        },
        State {
            name: "Material editor, color editor solved",
            base: Base::Design,
            scope: Some("MaterialEditorDialog"),
            deny: NONE,
            open: color_editor_solved,
        },
    ]
}

/// Cutting mode on its second step (Previous is enabled from there). The reset question replaces
/// the step buttons while it is open, so its two answers are pressed here first, the way a user
/// answers it: "Keep My Marks", then the question again and "Reset".
fn cutting_second_step(a: &HarnessApp) {
    edit_tab(a);
    open_cutting_mode(a);
    let cutting = a.ui().global::<CuttingModeModel>();
    cutting.set_confirm_reset(true);
    pump(200);
    press_in_scope(a, "CuttingModeScreen", "Keep My Marks");
    cutting.set_confirm_reset(true);
    pump(200);
    press_in_scope(a, "CuttingModeScreen", "Reset");
    cutting.invoke_next();
    let _ = wait_until(Duration::from_secs(5), || cutting.get_has_previous());
    pump(200);
}

/// The still export dialog on the Custom size (the Width and Height boxes) with a saved lighting
/// in its "Also Render With Saved Lighting" list. Opening the dialog reseeds the size and the
/// lighting list from the settings (`ExportModel.is_open` -> `populate_fanout_presets`), so both
/// are set after the opener: the size through its "Custom" pill.
fn export_custom_size(a: &HarnessApp) {
    let viewport = a.ui().global::<ViewportModel>();
    viewport.invoke_save_lighting_preset("Harness lighting".into());
    let _ = wait_until(Duration::from_secs(5), || {
        viewport
            .get_lighting_presets()
            .iter()
            .any(|preset| !preset.built_in)
    });
    live_tab(a);
    let export = a.ui().global::<ExportModel>();
    export.set_is_open(true);
    pump(300);
    if !press_in_scope(a, "ExportDialog", "Custom") {
        export.set_selected_size_preset(2);
    }
    if export.get_fanout_presets().row_count() == 0 {
        export.set_fanout_presets(ModelRc::new(VecModel::from(vec![LightingPresetItem {
            name: "Harness lighting".into(),
            built_in: false,
            export_usable: true,
            has_env_map: false,
            selected: false,
        }])));
    }
    pump(250);
}

/// The "Compare to Snapshot" panel (the Retarget dialog's other face): a snapshot taken, one
/// tier nudged, then the command bar's Compare to Snapshot. The snapshot is stored after a
/// background solve, so each step is waited for.
fn compare_to_snapshot(a: &HarnessApp) {
    edit_tab(a);
    wait_solved(a);
    let editor = a.ui().global::<EditorModel>();
    editor.invoke_snapshot_design();
    if !wait_until(Duration::from_secs(30), || editor.get_has_snapshot()) {
        return;
    }
    let _ = super::super::dialogs::nudge_first_tier(a.ui());
    wait_solved(a);
    let retarget = a.ui().global::<RetargetModel>();
    open_comparison(a);
    // The panel's two header buttons: the scope is found by difference here, and the sweep did
    // not credit them, so they are pressed the way a user does. The help button's accessible
    // label is "Help: " plus its hint. Closing ends the panel, so it is opened again to leave it
    // open for the sweep.
    if retarget.get_compare_open() {
        press(
            a,
            "Help: Open the manual page on comparing a design with its snapshot",
            None,
        );
        pump(300);
        if !retarget.get_compare_open() {
            open_comparison(a);
        }
        if press(a, "Close the snapshot comparison", None) {
            open_comparison(a);
        }
    }
}

/// Opens the snapshot comparison by pressing the command bar's Compare button (enabled only with
/// a held snapshot), then waits for the panel; the model command stands in when the button is not
/// showing (the Simple interface leaves it out).
fn open_comparison(a: &HarnessApp) {
    let retarget = a.ui().global::<RetargetModel>();
    if retarget.get_compare_open() {
        return;
    }
    if !press(a, "Compare", None) {
        a.ui().global::<EditorModel>().invoke_compare_to_snapshot();
    }
    let _ = wait_until(Duration::from_secs(15), || retarget.get_compare_open());
    pump(250);
}

/// The material editor's color editor ("Pick...") once its first solve landed, scrolled to its
/// end: Apply Color is enabled only with a solution, and it sits at the bottom of the dialog's
/// capped scroll area.
fn color_editor_solved(a: &HarnessApp) {
    edit_tab(a);
    a.ui()
        .global::<ViewportModel>()
        .set_material_editor_open(true);
    pump(300);
    press(a, "Pick\u{2026}", None);
    let colour = a.ui().global::<BodyColorEditorModel>();
    let _ = wait_until(Duration::from_secs(30), || {
        colour.get_has_solution() && !colour.get_busy()
    });
    scroll_scope(a, "MaterialEditorDialog", 0.0, -4000.0);
}

// ---------------------------------------------------------------------------------------------
// Panels
// ---------------------------------------------------------------------------------------------

const DENY_VIDEO: &[DenyRule] = &[
    deny(
        DenyMatch::Contains,
        "start video export",
        "long job: renders every frame of the sweep",
    ),
    deny(
        DenyMatch::Contains,
        "add to queue",
        "queues a long video job",
    ),
    deny(
        DenyMatch::Contains,
        "run in background",
        "long job: the running video export",
    ),
    deny(
        DenyMatch::Contains,
        "re-render",
        "long job: renders the tilt curve on the GPU",
    ),
];

fn panel_states() -> Vec<State> {
    vec![
        State {
            name: "Parameters: Lighting, reset question",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| {
                // Surface glare under 100 % and the head shadow off 16 degrees show the two
                // sliders' "Reset to ..." links; "Reset This Tab" opens Confirm and Cancel, and
                // "Save View as Saved Lighting..." the name row (SaveNameRow's field).
                let settings = a.ui().global::<SettingsModel>();
                settings.set_surface_glare_pct(50.0);
                settings.set_head_shadow_deg(8.0);
                parameters(a, 1);
                press_in_scope(a, "ParametersPanel", "Reset This Tab");
                press_in_scope(a, "ParametersPanel", "Save View as Saved Lighting...");
            },
        },
        State {
            name: "Parameters: Stone, previewing another material",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| {
                // A viewport material that is not the design's is a preview: the amber badge
                // with "Back to Design" and the preview's own color control. The body color on
                // its last entry (Custom) shows "Choose Color...". The crystal-axis section is
                // disabled for an isotropic material (the template's); Rust clears
                // `c_axis_override_available` for those, so it is set for a birefringent one.
                parameters(a, 0);
                let settings = a.ui().global::<SettingsModel>();
                settings.set_c_axis_override_available(true);
                settings.set_c_axis_override_enabled(true);
                a.ui()
                    .global::<ViewportModel>()
                    .set_viewport_material_linked(false);
                let editor = a.ui().global::<EditorModel>();
                let custom =
                    i32::try_from(editor.get_body_color_options().row_count()).unwrap_or(1) - 1;
                editor.set_body_color_index(custom.max(0));
                pump(250);
            },
        },
        State {
            name: "Parameters: Stone, library design on the viewport",
            base: Base::Library,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| {
                // The Stone tab's library branch ("Show the Library Design As", its color) is
                // shown while the viewport shows a library design; Rust clears the flag when the
                // library preview is drawn, which the passes never wait for, so it is set here.
                parameters(a, 0);
                a.ui()
                    .global::<ViewportModel>()
                    .set_shows_editor_design(false);
                pump(250);
            },
        },
        State {
            name: "Performance graph, video settings, middle",
            base: Base::Design,
            scope: Some("PerformanceGraphDialog"),
            deny: DENY_VIDEO,
            open: |a| video_section_scrolled(a, -450.0),
        },
        State {
            name: "Performance graph, video settings, end",
            base: Base::Design,
            scope: Some("PerformanceGraphDialog"),
            deny: DENY_VIDEO,
            open: |a| video_section_scrolled(a, -4000.0),
        },
    ]
}

/// The tilt dialog with "Export Tilt Video" expanded on the Custom resolution with the values
/// overlay on, its body scrolled by `dy`. The dialog is capped at 720 px and its body scrolls:
/// the start, end and step fields were reached by the older states, everything below them was
/// outside the clip. The settings are written after the dialog opened.
fn video_section_scrolled(a: &HarnessApp, dy: f32) {
    edit_tab(a);
    a.ui().global::<TiltModel>().invoke_toggle_dialog();
    pump(300);
    let video = a.ui().global::<TiltVideoExportModel>();
    video.set_resolution_preset(video.invoke_custom_resolution_index());
    video.set_show_values(true);
    press_in_scope(a, "PerformanceGraphDialog", "Export Tilt Video");
    pump(250);
    scroll_scope(a, "PerformanceGraphDialog", 0.0, dy);
}
