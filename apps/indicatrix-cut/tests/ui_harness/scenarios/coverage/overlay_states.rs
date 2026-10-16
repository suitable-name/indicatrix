//! The overlay and panel states of the click-through passes (`overlays.rs`): how each one is
//! reached from a fresh app, which component its controls live in, and which controls the pass
//! must not press.
//!
//! A state's `open` function uses the route the app itself uses (a model flag or callback that the
//! normal route ends in, or the header button for the popups). `scope` is the component the
//! pass is limited to (the dialog, the panel); when the component is not found the pass falls
//! back to the whole window and prints a GAP line.
//!
//! Deny rules: the pass runs in a throw-away profile that is rebuilt after every visible change,
//! so destroying data is harmless and is NOT denied here (unlike the generic smoke list). Only
//! what leaves the profile or runs long is denied: network, real renders, exports, quitting.

use super::super::main_window;
use crate::support::{
    HarnessApp,
    app::{pump, wait_until},
    query::is_clickable_role,
    smoke::{DenyMatch, DenyRule},
};
use indicatrix_cut::{
    CommandPaletteModel, EditorModel, ExportModel, GuideModel, HelpModel, LibraryModel,
    ParametersModel, PreferencesModel, RawTextModel, RemoteWorkerModel, RenderJobsModel,
    RetargetModel, ShortcutsModel, SweepModel, TiltModel, TiltVideoExportModel, TutorialsModel,
    UpdateModel, VariantsModel, ViewportModel, harness_support,
};
use slint::{ComponentHandle, Model};
use std::time::Duration;

/// What the app holds before the state's `open` function runs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Base {
    /// The freshly started app.
    Startup,
    /// The four fixture designs in the library, the first one selected.
    Library,
    /// A template design in the editor.
    Design,
    /// A template design in the editor with one undoable edit made.
    Edited,
}

/// One state of the click-through passes.
pub struct State {
    /// Name used in findings.
    pub name: &'static str,
    /// What the app holds first.
    pub base: Base,
    /// The component (type name) whose controls are pressed; `None`: the whole window.
    pub scope: Option<&'static str>,
    /// Controls (by label) the pass leaves alone, on top of [`OVERLAY_DENY`].
    pub deny: &'static [DenyRule],
    /// Reaches the state.
    pub open: fn(&HarnessApp),
}

pub(super) const fn deny(mode: DenyMatch, pattern: &'static str, reason: &'static str) -> DenyRule {
    DenyRule::label(mode, pattern, reason)
}

/// What every pass leaves alone: whatever ends the app, leaves the profile, reaches the network or
/// starts a long render or export. (Destructive controls are pressed: the profile is rebuilt.)
pub const OVERLAY_DENY: &[DenyRule] = &[
    deny(DenyMatch::Contains, "quit", "ends the app"),
    deny(DenyMatch::Exact, "exit", "ends the app"),
    deny(
        DenyMatch::Exact,
        "export",
        "runs an export (driven by coverage_export_menu)",
    ),
    deny(DenyMatch::Contains, "start export", "runs an export"),
    deny(DenyMatch::Contains, "run export", "runs an export"),
    deny(DenyMatch::Contains, "regenerate all", "long batch job"),
    deny(DenyMatch::Contains, "start render", "long render"),
    deny(DenyMatch::Contains, "test connection", "network"),
    deny(DenyMatch::Contains, "claim", "network"),
    deny(DenyMatch::Contains, "enroll", "network"),
    deny(DenyMatch::Contains, "sync", "network"),
    deny(DenyMatch::Contains, "download", "network"),
    deny(DenyMatch::Contains, "check for update", "network"),
    deny(DenyMatch::Contains, "upload", "network"),
    deny(
        DenyMatch::Contains,
        "check now",
        "network (GitHub release lookup)",
    ),
    deny(
        DenyMatch::Contains,
        "check daily",
        "network (GitHub release lookup)",
    ),
    deny(
        DenyMatch::Contains,
        "close window",
        "closes the window under test",
    ),
];

pub(super) const NONE: &[DenyRule] = &[];
const DENY_PREFS: &[DenyRule] = &[deny(
    DenyMatch::Contains,
    "check for new versions",
    "network: switching it on starts an update check (UpdateModel.enabled_changed)",
)];
pub(super) const DENY_JOBS: &[DenyRule] = &[
    deny(
        DenyMatch::Contains,
        "start queue",
        "long job: starts the queue worker",
    ),
    deny(
        DenyMatch::Contains,
        "run next",
        "long job: renders the next queued job",
    ),
];
pub(super) const DENY_REMOTE: &[DenyRule] = &[
    deny(
        DenyMatch::Contains,
        "save",
        "network: saving an endpoint makes later renders connect",
    ),
    deny(DenyMatch::Contains, "test", "network"),
    deny(DenyMatch::Contains, "mirror", "network"),
    deny(DenyMatch::Contains, "switch", "network"),
];
pub(super) const DENY_FILTERS: &[DenyRule] = &[deny(
    DenyMatch::Contains,
    "compute missing",
    "long job: renders a tilt curve for every design that lacks one",
)];
const DENY_PREVIEW_BATCH: &[DenyRule] = &[
    deny(
        DenyMatch::Contains,
        "quick (solid)",
        "long job: renders solid previews",
    ),
    deny(
        DenyMatch::Contains,
        "generate",
        "long job: renders previews",
    ),
];
const DENY_TILT_BATCH: &[DenyRule] = &[deny(
    DenyMatch::Exact,
    "compute",
    "long job: renders tilt curves for the whole set",
)];
pub(super) const DENY_PERFORMANCE: &[DenyRule] = &[deny(
    DenyMatch::Contains,
    "re-render",
    "long job: renders the tilt curve on the GPU",
)];
const DENY_SWEEP: &[DenyRule] = &[deny(
    DenyMatch::Contains,
    "run sweep",
    "long job: solves the design at every angle of the range",
)];

// ---------------------------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------------------------

/// Presses the first clickable element labelled `label` (whose element type is `kind`, when
/// given) and lets the app react; `false` when there is none.
pub(super) fn press(app: &HarnessApp, label: &str, kind: Option<&str>) -> bool {
    let view = app.main_view();
    let target = view.by_label(label).into_iter().find(|e| {
        e.accessible_role().is_some_and(is_clickable_role)
            && kind.is_none_or(|k| e.type_name().is_some_and(|t| t.as_str() == k))
    });
    let Some(element) = target else {
        return false;
    };
    crate::support::coverage::record_activation(&element);
    crate::support::coverage::record_wrappers(&view.root, &element, false);
    element.invoke_accessible_default_action();
    pump(200);
    true
}

/// Top tab 3D, Edit view.
pub(super) fn edit_tab(app: &HarnessApp) {
    main_window::select_top_tab_model(app, 0);
    main_window::select_render_view(app, 1);
}

/// Top tab 3D, Live Render view.
pub(super) fn live_tab(app: &HarnessApp) {
    main_window::select_top_tab_model(app, 0);
    main_window::select_render_view(app, 0);
}

/// The Edit view with the inspector on `tab` and tier 0 selected.
pub(super) fn inspector(app: &HarnessApp, tab: i32) {
    edit_tab(app);
    let editor = app.ui().global::<EditorModel>();
    editor.set_selected_tier_index(0);
    editor.set_inspector_tab(tab);
    // The stored inspector height is also its maximum: raise it so the tab body (a ScrollView
    // whose scrolled-out controls are not enumerated) shows whole in the tall window.
    editor.set_inspector_height(2400.0);
    pump(250);
}

/// The Live Render view with the Parameters dock open on `tab`.
pub(super) fn parameters(app: &HarnessApp, tab: i32) {
    live_tab(app);
    let dock = app.ui().global::<ParametersModel>();
    dock.set_open(true);
    dock.invoke_show_tab(tab);
    pump(250);
}

/// Two queued still-render jobs (nothing starts them), the Render Jobs panel open.
///
/// "Add to Queue" first needs an export folder (a folder dialog while none is configured), so the
/// dialog is answered with a folder of the profile's working directory (the first answer is stored
/// as the export folder; a second one is queued in case it is not), and the finished stone is
/// prepared on a worker thread, so each call is waited for. Unconsumed answers are dropped.
pub(super) fn jobs_with_rows(app: &HarnessApp) {
    let jobs = app.ui().global::<RenderJobsModel>();
    if let Ok(dir) = std::env::current_dir().map(|d| d.join("harness_queue_exports")) {
        let _ = std::fs::create_dir_all(&dir);
        harness_support::queue_picker_path(dir.clone());
        harness_support::queue_picker_path(dir);
    }
    jobs.invoke_add_still_jobs(1920, 1080, 64, 0, 0, 2);
    let _ = wait_until(Duration::from_secs(20), || jobs.get_rows().row_count() >= 1);
    jobs.invoke_add_still_jobs(1280, 720, 32, 0, 0, 2);
    let _ = wait_until(Duration::from_secs(20), || jobs.get_rows().row_count() >= 2);
    harness_support::clear_picker_answers();
    jobs.set_panel_open(true);
    pump(250);
}

/// Waits (real time, up to 8 s) until the Retarget dialog's validity check has an answer: the
/// check runs on a worker thread, and Apply and Compare... stay disabled while it is running.
/// `validity_state` is 0 ("no verdict") before the check starts as well as in Optimize mode, so
/// waiting for "not 1" returned at once and the pass saw Apply dimmed: the wait is for a verdict
/// (2 to 4), an error, or the Optimize mode, which has none until an option is picked.
pub(super) fn wait_retarget_checked(app: &HarnessApp) {
    let model = app.ui().global::<RetargetModel>();
    // `proposal_changed` only queues a rebuild (drained on a 16 ms tick), and the check starts
    // after its 120 ms debounce: without this, the wait below reads the verdict of the request
    // before and returns at once.
    pump(250);
    let _ = wait_until(Duration::from_secs(8), || {
        !model.get_is_busy()
            && (model.get_validity_state() >= 2
                || !model.get_solve_error().is_empty()
                || model.get_mode_index() == 1)
    });
    pump(150);
}

/// Opens cutting mode and waits (real time, up to 10 s) until Rust has the first page ready: the
/// steps are built on a worker, and the page's buttons and chips only exist after that.
pub(super) fn open_cutting_mode(app: &HarnessApp) {
    let cutting = app.ui().global::<indicatrix_cut::CuttingModeModel>();
    cutting.invoke_open_cutting_mode();
    let _ = wait_until(Duration::from_secs(10), || {
        !cutting.get_busy() && (cutting.get_step_count() > 0 || !cutting.get_error().is_empty())
    });
    pump(200);
}

macro_rules! state {
    ($name:expr, $base:ident, $scope:expr, $deny:expr, $open:expr) => {
        State {
            name: $name,
            base: Base::$base,
            scope: $scope,
            deny: $deny,
            open: $open,
        }
    };
}

// ---------------------------------------------------------------------------------------------
// The lists
// ---------------------------------------------------------------------------------------------

/// Overlays that need no data at all.
pub fn startup_states() -> Vec<State> {
    vec![
        state!("Shortcuts", Startup, Some("ShortcutOverlay"), NONE, |a| {
            a.ui().global::<ShortcutsModel>().set_open(true);
        }),
        state!(
            "Preferences",
            Startup,
            Some("PreferencesDialog"),
            DENY_PREFS,
            |a| {
                a.ui().global::<PreferencesModel>().set_dialog_open(true);
            }
        ),
        state!(
            "Command palette",
            Startup,
            Some("CommandPalette"),
            NONE,
            |a| {
                a.ui().global::<CommandPaletteModel>().invoke_open_palette();
            }
        ),
        state!("Glossary", Startup, Some("GlossaryDialog"), NONE, |a| {
            a.ui().global::<HelpModel>().invoke_open_glossary("".into());
        }),
        state!(
            "Tutorial browser",
            Startup,
            Some("TutorialBrowser"),
            NONE,
            |a| {
                a.ui().global::<TutorialsModel>().invoke_open_browser();
            }
        ),
        state!(
            "Render jobs, empty",
            Startup,
            Some("RenderJobsPanel"),
            DENY_JOBS,
            |a| {
                a.ui().global::<RenderJobsModel>().set_panel_open(true);
            }
        ),
        state!(
            "Render jobs, script export",
            Startup,
            Some("RenderJobsPanel"),
            DENY_JOBS,
            |a| {
                let jobs = a.ui().global::<RenderJobsModel>();
                jobs.set_panel_open(true);
                jobs.set_script_open(true);
            }
        ),
        state!(
            "Remote coordinator",
            Startup,
            Some("RemoteWorkerDialog"),
            DENY_REMOTE,
            |a| {
                a.ui().global::<RemoteWorkerModel>().invoke_open_dialog();
            }
        ),
        state!(
            "Filters popup",
            Startup,
            Some("FilterPanel"),
            DENY_FILTERS,
            |a| {
                press(a, "Filters", None);
            }
        ),
        state!("Import popup", Startup, Some("ImportDialog"), NONE, |a| {
            press(a, "Import", None);
        }),
        state!("Update banner", Startup, Some("UpdateBanner"), NONE, |a| {
            let update = a.ui().global::<UpdateModel>();
            update.set_notice_text("Version 9.9.9 is available.".into());
            update.set_notice_visible(true);
        }),
        state!("Guide panel", Startup, Some("GuidePanel"), NONE, |a| {
            a.ui()
                .global::<GuideModel>()
                .invoke_start_guide("new-design-walkthrough".into());
        }),
    ]
}

/// The first-run Welcome dialog (run with the first-run settings seed).
pub fn welcome_states() -> Vec<State> {
    vec![state!(
        "Welcome",
        Startup,
        Some("WelcomeDialog"),
        NONE,
        |_| {}
    )]
}

/// States that want the library behind them.
pub fn library_states() -> Vec<State> {
    vec![
        state!(
            "Metadata editor",
            Library,
            Some("MetadataEditorDialog"),
            NONE,
            |a| {
                a.ui()
                    .global::<LibraryModel>()
                    .set_metadata_editor_open(true);
            }
        ),
        state!(
            "Preview batch",
            Library,
            Some("PreviewBatchDialog"),
            DENY_PREVIEW_BATCH,
            |a| {
                let batch = a.ui().global::<indicatrix_cut::BatchModel>();
                batch.set_preview_confirming(true);
                batch.set_preview_offer_count(2);
                batch.set_preview_visible(true);
            }
        ),
        state!(
            "Tilt batch",
            Library,
            Some("TiltBatchDialog"),
            DENY_TILT_BATCH,
            |a| {
                let batch = a.ui().global::<indicatrix_cut::BatchModel>();
                batch.set_tilt_confirming(true);
                batch.set_tilt_offer_count(2);
                batch.set_tilt_visible(true);
            }
        ),
        state!(
            "Library with a selected design",
            Library,
            None,
            DENY_FILTERS,
            |_| {}
        ),
    ]
}

/// Dialogs and overlays over a design in the editor.
pub fn design_dialog_states() -> Vec<State> {
    let mut states = design_dialog_states_first();
    states.extend(design_dialog_states_rest());
    states
}

/// The first half of [`design_dialog_states`].
fn design_dialog_states_first() -> Vec<State> {
    vec![
        state!(
            "New Design dialog",
            Design,
            Some("NewDesignDialog"),
            NONE,
            |a| {
                edit_tab(a);
                a.ui().global::<EditorModel>().set_new_dialog_open(true);
            }
        ),
        state!(
            "Retarget dialog",
            Design,
            Some("RetargetDialog"),
            NONE,
            |a| {
                edit_tab(a);
                a.ui().global::<RetargetModel>().invoke_open();
                wait_retarget_checked(a);
            }
        ),
        state!("Gear remap", Design, None, NONE, |a| {
            edit_tab(a);
            a.ui().global::<EditorModel>().set_gear_remap_open(true);
        }),
        state!(
            "Material editor",
            Design,
            Some("MaterialEditorDialog"),
            NONE,
            |a| {
                edit_tab(a);
                a.ui()
                    .global::<ViewportModel>()
                    .set_material_editor_open(true);
            }
        ),
        state!(
            "Performance graph",
            Design,
            Some("PerformanceGraphDialog"),
            DENY_PERFORMANCE,
            |a| {
                edit_tab(a);
                a.ui().global::<TiltModel>().invoke_toggle_dialog();
            }
        ),
        state!(
            "Performance graph, video exporting",
            Design,
            Some("PerformanceGraphDialog"),
            DENY_PERFORMANCE,
            |a| {
                edit_tab(a);
                a.ui().global::<TiltModel>().invoke_toggle_dialog();
                a.ui()
                    .global::<TiltVideoExportModel>()
                    .set_is_exporting(true);
            }
        ),
        state!(
            "Anchor explainer",
            Design,
            Some("AnchorExplainer"),
            NONE,
            |a| {
                edit_tab(a);
                a.ui()
                    .global::<EditorModel>()
                    .set_anchor_explainer_open(true);
            }
        ),
    ]
}

/// The second half of [`design_dialog_states`].
fn design_dialog_states_rest() -> Vec<State> {
    vec![
        state!("Edit as Text", Design, Some("RawTextDialog"), NONE, |a| {
            edit_tab(a);
            a.ui().global::<RawTextModel>().invoke_open();
        }),
        state!(
            "Angle sweep",
            Design,
            Some("SweepDialog"),
            DENY_SWEEP,
            |a| {
                edit_tab(a);
                a.ui().global::<SweepModel>().invoke_open_dialog();
            }
        ),
        state!(
            "Cutting mode",
            Design,
            Some("CuttingModeScreen"),
            NONE,
            |a| {
                edit_tab(a);
                open_cutting_mode(a);
            }
        ),
        state!("Export dialog", Design, Some("ExportDialog"), NONE, |a| {
            live_tab(a);
            a.ui().global::<ExportModel>().set_is_open(true);
        }),
        state!(
            "Export dialog, exporting",
            Design,
            Some("ExportDialog"),
            NONE,
            |a| {
                live_tab(a);
                a.ui().global::<ExportModel>().set_is_open(true);
                a.ui().global::<ExportModel>().set_is_exporting(true);
            }
        ),
        state!(
            "Render jobs with two jobs",
            Design,
            Some("RenderJobsPanel"),
            DENY_JOBS,
            jobs_with_rows
        ),
        state!(
            "Verdict popover",
            Design,
            Some("VerdictPopover"),
            NONE,
            super::states_reach::verdict_with_reasons
        ),
        state!(
            "Unsaved changes question",
            Edited,
            Some("ConfirmActionDialog"),
            NONE,
            |a| {
                edit_tab(a);
                a.ui().global::<EditorModel>().set_unsaved_dialog_open(true);
            }
        ),
    ]
}

/// Panels of the editor and the Live Render view over a design.
pub fn design_panel_states() -> Vec<State> {
    let mut states = design_panel_states_first();
    states.extend(design_panel_states_rest());
    states
}

/// The first half of [`design_panel_states`].
fn design_panel_states_first() -> Vec<State> {
    vec![
        state!(
            "Inspector: Tier",
            Design,
            Some("EditorInspector"),
            NONE,
            |a| {
                inspector(a, 0);
                // "+ Add" (reader label "Add Facet") is enabled only while a position is typed.
                let _ = super::states_reach::type_into(a, "Position of the Facet to Add", "24");
            }
        ),
        state!(
            "Inspector: Preform",
            Design,
            Some("EditorInspector"),
            NONE,
            |a| inspector(a, 1)
        ),
        state!(
            "Inspector: Optimize",
            Design,
            Some("EditorInspector"),
            NONE,
            |a| inspector(a, 2)
        ),
        state!(
            "Inspector: Optimize ranges",
            Design,
            Some("EditorInspector"),
            NONE,
            super::states_reach::optimize_ranges_custom
        ),
        state!(
            "Inspector: Schedule",
            Design,
            Some("EditorInspector"),
            NONE,
            |a| inspector(a, 3)
        ),
        state!(
            "Inspector: History steps",
            Edited,
            Some("EditorInspector"),
            NONE,
            |a| {
                inspector(a, 4);
                a.ui().global::<VariantsModel>().set_view(0);
            }
        ),
        state!(
            "Inspector: History variants",
            Edited,
            Some("EditorInspector"),
            NONE,
            |a| {
                inspector(a, 4);
                a.ui().global::<VariantsModel>().set_view(1);
                pump(200);
            }
        ),
        state!(
            "Inspector: concave tier form",
            Design,
            Some("EditorInspector"),
            NONE,
            |a| {
                edit_tab(a);
                a.ui().global::<EditorModel>().invoke_add_concave_tier();
                a.ui().global::<EditorModel>().set_inspector_height(2400.0);
                pump(250);
            }
        ),
    ]
}

/// The second half of [`design_panel_states`].
fn design_panel_states_rest() -> Vec<State> {
    vec![
        state!(
            "Inspector: Optimize running",
            Design,
            Some("EditorInspector"),
            NONE,
            |a| {
                inspector(a, 2);
                // The running-search controls (Cancel) show while this flag is set; pressing them
                // with no search running is the harmless cancel the direct-invoke row covers.
                a.ui().global::<EditorModel>().set_optimize_running(true);
                pump(200);
            }
        ),
        state!("Edit view, work running", Design, None, NONE, |a| {
            edit_tab(a);
            // Abandon Solve, Cancel Deep Solve, the optimize Cancel and Redo only show or
            // enable while their work or history exists; the flags are set directly. Compare
            // needs a held snapshot, which a flag cannot fake (pressing it then raises the
            // "No snapshot taken yet" toast); the snapshot states cover it.
            let editor = a.ui().global::<EditorModel>();
            // The base design's own first solve must land first: when it does, Rust clears
            // `solve_running` and Abandon Solve disappears before the pass reaches it.
            let _ = wait_until(Duration::from_secs(10), || !editor.get_solve_running());
            // Auto-solve off (budget 0, the combo's "Off"), so the pass's own edits start no
            // solve whose end would clear the flag again.
            editor.set_auto_solve_budget_ms(0);
            pump(200);
            editor.set_solve_running(true);
            editor.set_deep_solve_running(true);
            editor.set_optimize_running(true);
            editor.set_can_redo(true);
            pump(250);
        }),
        state!(
            "Status strip details",
            Design,
            Some("EditorLogPopup"),
            super::states_reach::DENY_PIN,
            super::states_reach::status_details_filled
        ),
        state!(
            "Parameters: Stone",
            Design,
            Some("ParametersPanel"),
            NONE,
            |a| parameters(a, 0)
        ),
        state!(
            "Parameters: Lighting",
            Design,
            Some("ParametersPanel"),
            NONE,
            |a| parameters(a, 1)
        ),
        state!(
            "Parameters: Render and Output",
            Design,
            Some("ParametersPanel"),
            NONE,
            |a| {
                parameters(a, 2);
            }
        ),
        state!("Edit view, edited design", Edited, None, NONE, |a| {
            edit_tab(a);
            // A selected tier shows the inspector's tier form; tier 1 can move both ways.
            a.ui().global::<EditorModel>().set_selected_tier_index(1);
            pump(200);
        }),
        state!("Live Render view", Design, None, NONE, live_tab),
        state!(
            "Edit view, two tiers selected",
            Edited,
            Some("EditorTierTable"),
            NONE,
            |a| {
                edit_tab(a);
                let editor = a.ui().global::<EditorModel>();
                // The form's Mirror acts on the table's keyboard cursor, which follows a
                // selection made outside the table (EditorView's selection mirror): select a
                // row (through -1, so the write is a change) to put the cursor on it. This
                // comes first: a changed selection clears the multi-select group (the
                // "N selected" Clear / Delete / Adopt Sel. / Offset row shows only while the
                // group holds two tiers).
                editor.set_selected_tier_index(-1);
                pump(100);
                editor.set_selected_tier_index(0);
                pump(250);
                editor.invoke_toggle_multi_select(0);
                editor.invoke_toggle_multi_select(1);
                pump(150);
                // The toolbar's collapsed parts: the Steps / Mirror form, the tick box column,
                // and the filter's clear button.
                editor.set_tier_filter_text("1".into());
                press(a, "Steps / Mirror", None);
                press(a, "Select", None);
            }
        ),
    ]
}
