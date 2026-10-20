//! More main-window states of the click-through passes (run 6 left 399 elements uncovered; this
//! file is the part of the answer that sets model properties and presses openers).
//!
//! A control that only exists after another control was pressed (an edit form, a menu, a hint
//! bar) is not enumerated by the engine unless the state's `open` function has pressed the
//! opener, so each state here reaches the page the way a user would: model flags first, then the
//! opener button. Every property set below is an `in` / `in-out` property of the generated
//! global in `ui/models/*.slint`.

use super::overlay_states::{
    Base, DENY_FILTERS, DENY_JOBS, NONE, State, deny, edit_tab, inspector, jobs_with_rows,
    parameters, press, wait_retarget_checked,
};
use crate::support::{
    HarnessApp,
    app::{pump, wait_until},
    smoke::{DenyMatch, DenyRule},
};
use indicatrix_cut::{
    EditorModel, LibraryModel, RawTextDiffRow, RemoteAvailabilityItem, RemoteWorkerModel,
    RenderJobsModel, RetargetModel, TiltVideoExportModel, VariantsModel, WorkerItem,
    harness_support,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::time::Duration;

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
const DENY_RETARGET: &[DenyRule] = &[
    deny(
        DenyMatch::Contains,
        "compare",
        "opens the Compare window, which renders both designs on the GPU",
    ),
    deny(
        DenyMatch::Contains,
        "search",
        "long job: RetargetModel.search runs the Optimize search on a worker",
    ),
];
const DENY_STALE: &[DenyRule] = &[deny(
    DenyMatch::Contains,
    "recompute",
    "long job: starts a Deep Solve or an Optimize search",
)];
/// `DENY_REMOTE` plus the Library "Remote" pill. The configured states put the endpoint in the
/// model only (the settings store keeps none, so nothing can reach the network); the pill's
/// `switch_library_source` reads the settings store and would otherwise probe the coordinator,
/// so pressing it here can only ever answer "No remote coordinator is configured.".
const DENY_REMOTE_CONFIGURED: &[DenyRule] = &[
    deny(
        DenyMatch::Contains,
        "save",
        "network: saving an endpoint makes later renders connect",
    ),
    deny(DenyMatch::Contains, "test", "network"),
    deny(DenyMatch::Contains, "mirror", "network"),
    deny(DenyMatch::Contains, "switch", "network"),
    deny(
        DenyMatch::Exact,
        "remote",
        "network: browsing the remote library probes the coordinator",
    ),
];
const DENY_VARIANT_PICTURES: &[DenyRule] = &[deny(
    DenyMatch::Contains,
    "compare pictures",
    "opens the Compare window, which renders both variants on the GPU",
)];

fn state(
    name: &'static str,
    base: Base,
    scope: Option<&'static str>,
    deny: &'static [DenyRule],
    open: fn(&HarnessApp),
) -> State {
    State {
        name,
        base,
        scope,
        deny,
        open,
    }
}

/// Runs the states of this file (called by the scenarios of `overlays_more`).
pub fn dialog_states() -> Vec<State> {
    let mut states = dialog_states_first();
    states.extend(dialog_states_rest());
    states
}

/// The first half of [`dialog_states`].
fn dialog_states_first() -> Vec<State> {
    vec![
        state(
            "Performance graph, video settings open",
            Base::Design,
            Some("PerformanceGraphDialog"),
            DENY_VIDEO,
            video_section,
        ),
        state(
            "New Design dialog, empty design with custom gear",
            Base::Design,
            Some("NewDesignDialog"),
            NONE,
            |a| {
                edit_tab(a);
                let editor = a.ui().global::<EditorModel>();
                editor.set_new_template_index(0);
                editor.set_new_gear_index(6);
                editor.set_new_dialog_open(true);
                pump(250);
            },
        ),
        state(
            "Retarget dialog, new material",
            Base::Design,
            Some("RetargetDialog"),
            DENY_RETARGET,
            |a| retarget(a, false),
        ),
        state(
            "Retarget dialog, valid proposal",
            Base::Design,
            Some("RetargetDialog"),
            DENY_RETARGET,
            retarget_valid,
        ),
        state(
            "Retarget dialog, Optimize mode",
            Base::Design,
            Some("RetargetDialog"),
            DENY_RETARGET,
            |a| retarget(a, true),
        ),
    ]
}

/// The second half of [`dialog_states`].
fn dialog_states_rest() -> Vec<State> {
    vec![
        state(
            "Edit as Text, edited",
            Base::Design,
            Some("RawTextDialog"),
            NONE,
            |a| raw_text_edited(a, 0),
        ),
        state(
            "Edit as Text, discard question",
            Base::Design,
            Some("RawTextDialog"),
            NONE,
            |a| raw_text_edited(a, 1),
        ),
        state(
            "Edit as Text, compare menu",
            Base::Design,
            Some("RawTextDialog"),
            NONE,
            |a| raw_text_edited(a, 2),
        ),
        state(
            "Edit as Text, comparison shown",
            Base::Design,
            Some("RawTextDialog"),
            NONE,
            |a| raw_text_edited(a, 3),
        ),
        state(
            "Remote coordinator, configured",
            Base::Startup,
            Some("RemoteWorkerDialog"),
            DENY_REMOTE_CONFIGURED,
            |a| remote_configured(a, false),
        ),
        state(
            "Remote coordinator, edit form",
            Base::Startup,
            Some("RemoteWorkerDialog"),
            DENY_REMOTE_CONFIGURED,
            |a| remote_configured(a, true),
        ),
        state(
            "Render jobs, every row button",
            Base::Design,
            Some("RenderJobsPanel"),
            DENY_JOBS,
            jobs_all_buttons,
        ),
        state(
            "Render jobs, script export with jobs",
            Base::Design,
            Some("RenderJobsPanel"),
            DENY_JOBS,
            jobs_script_export,
        ),
        state("Render jobs chip", Base::Design, None, DENY_JOBS, |a| {
            edit_tab(a);
            let jobs = a.ui().global::<RenderJobsModel>();
            jobs.set_chip_visible(true);
            jobs.set_chip_text("1 job".into());
            jobs.set_chip_progress(0.4);
            pump(250);
        }),
    ]
}

/// Library states: a search typed in and an id filter active, and the toast.
pub fn library_states() -> Vec<State> {
    vec![
        state(
            "Library with a search and a filter bar",
            Base::Library,
            None,
            DENY_FILTERS,
            |a| {
                // The bar shows while the just-imported filter holds ids; the list itself exists
                // only once the library is unfolded.
                let library = a.ui().global::<LibraryModel>();
                let mut ids: Vec<i32> = library
                    .get_diagram_list()
                    .iter()
                    .map(|row| row.id)
                    .collect();
                if ids.is_empty() {
                    ids.push(0);
                }
                library.set_recent_import_filter(ModelRc::new(VecModel::from(ids)));
                library.set_search_text("quartz".into());
                library.set_id_filter_label("Showing a harness filter.".into());
                super::states_reach::library_list_open(a);
                pump(400);
            },
        ),
        state("Toast, dismissed", Base::Startup, None, NONE, |a| {
            let ui = a.ui();
            ui.set_toast_type("info".into());
            ui.set_toast_message("A note from the harness.".into());
            ui.set_toast_visible(true);
            pump(200);
            // The engine hides the toast before it enumerates, so the Dismiss press is made here.
            press(a, "Dismiss this message", None);
        }),
    ]
}

/// Panels of the editor over a design with variants saved (real saves, so the row keys are real).
pub fn panel_states() -> Vec<State> {
    let mut states = panel_states_first();
    states.extend(panel_states_rest());
    states
}

/// The first half of [`panel_states`].
fn panel_states_first() -> Vec<State> {
    vec![
        state(
            "Inspector: History variants, two saved",
            Base::Edited,
            Some("EditorInspector"),
            DENY_VARIANT_PICTURES,
            |a| variants_saved(a, 0),
        ),
        state(
            "Inspector: History variants, save form",
            Base::Edited,
            Some("EditorInspector"),
            DENY_VARIANT_PICTURES,
            |a| variants_saved(a, 1),
        ),
        state(
            "Inspector: History variants, note form",
            Base::Edited,
            Some("EditorInspector"),
            DENY_VARIANT_PICTURES,
            |a| variants_saved(a, 2),
        ),
        state(
            "Inspector: History variants, text comparison",
            Base::Edited,
            Some("EditorInspector"),
            DENY_VARIANT_PICTURES,
            |a| variants_saved(a, 3),
        ),
        state(
            "Inspector: Optimize, ranges and weights",
            Base::Design,
            Some("EditorInspector"),
            NONE,
            |a| {
                inspector(a, 2);
                a.ui()
                    .global::<indicatrix_cut::OptimizeModel>()
                    .set_ranges_open(true);
                pump(250);
            },
        ),
        state(
            "Edit view, out-of-date results",
            Base::Design,
            None,
            DENY_STALE,
            |a| {
                edit_tab(a);
                let editor = a.ui().global::<EditorModel>();
                editor.set_deep_solve_stale(true);
                editor.set_optimize_stale(true);
                pump(250);
            },
        ),
    ]
}

/// The second half of [`panel_states`].
fn panel_states_rest() -> Vec<State> {
    vec![
        state(
            "Parameters: Stone, crystal axis override",
            Base::Design,
            Some("ParametersPanel"),
            NONE,
            |a| {
                a.ui()
                    .global::<indicatrix_cut::SettingsModel>()
                    .set_c_axis_override_enabled(true);
                parameters(a, 0);
            },
        ),
        state(
            "Parameters: Render and Output, remote configured",
            Base::Design,
            Some("ParametersPanel"),
            NONE,
            |a| {
                a.ui()
                    .global::<RemoteWorkerModel>()
                    .set_availability(RemoteAvailabilityItem {
                        configured: true,
                        reachable_render_capable: true,
                    });
                parameters(a, 2);
            },
        ),
        state(
            "Parameters: Lighting, environment map loaded",
            Base::Design,
            Some("ParametersPanel"),
            NONE,
            |a| {
                // Clear shows only while a map is loaded; the flag is Rust-owned, set directly.
                a.ui()
                    .global::<indicatrix_cut::SettingsModel>()
                    .set_env_map_loaded(true);
                parameters(a, 1);
            },
        ),
    ]
}

// ---------------------------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------------------------

/// The tilt dialog with the "Export Tilt Video" section expanded on the Custom resolution (Width
/// and Height boxes), the values chips and, with a remote configured, the Compute and Transfer
/// rows. The reach states cover the scrolled sections but configure no remote.
fn video_section(a: &HarnessApp) {
    edit_tab(a);
    let remote = a.ui().global::<RemoteWorkerModel>();
    remote.set_availability(RemoteAvailabilityItem {
        configured: true,
        reachable_render_capable: true,
    });
    let video = a.ui().global::<TiltVideoExportModel>();
    video.set_resolution_preset(video.invoke_custom_resolution_index());
    video.set_show_values(true);
    video.set_transfer_index(1);
    video.set_compute_target_index(2);
    a.ui()
        .global::<indicatrix_cut::TiltModel>()
        .invoke_toggle_dialog();
    pump(300);
    press(a, "Export Tilt Video", None);
    pump(300);
}

/// The Retarget dialog with a proposal (a target index of refraction typed in) and the validity
/// check finished; `optimize` switches to the Optimize mode with the crown fraction free.
fn retarget(a: &HarnessApp, optimize: bool) {
    edit_tab(a);
    let model = a.ui().global::<RetargetModel>();
    model.invoke_open();
    model.set_target_ri_override_text("2.4".into());
    model.set_crown_follows_pavilion(false);
    if optimize {
        model.set_mode_index(1);
    }
    model.invoke_proposal_changed();
    wait_retarget_checked(a);
}

/// The Retarget dialog on the proposal `open` itself builds: the design's own material as the
/// target (no typed index of refraction, so `target_ri_override_text` is not read), which moves
/// no mast and is the retarget the check tests pin as valid
/// (`a_same_material_retarget_is_valid_and_changes_no_mast`). A real index change on this
/// template (a shallow-pavilion quartz design) is refused as "invalid" (3), which leaves Apply
/// dimmed. `open` builds the plan synchronously and starts the one check after its 120 ms
/// debounce, so the single wait is for the dialog's own `can_apply`.
fn retarget_valid(a: &HarnessApp) {
    edit_tab(a);
    let model = a.ui().global::<RetargetModel>();
    model.invoke_open();
    pump(250);
    // The dialog's own `can_apply`: a proposal (rows, no anchored-tier error, no solve error, not
    // busy) whose check is finished and came out valid (2). "Invalid" (3) and "could not be
    // checked" (4) dim Apply too: an unchecked change has no candidate stone, and the engine's
    // gate refuses it.
    let applicable = wait_until(Duration::from_secs(20), || {
        model.get_rows().row_count() > 0
            && model.get_anchored_tier_errors().row_count() == 0
            && model.get_solve_error().is_empty()
            && !model.get_is_busy()
            && model.get_mode_index() == 0
            && !matches!(model.get_validity_state(), 0 | 1 | 3 | 4)
    });
    if !applicable {
        println!(
            "ui-harness:   retarget, valid proposal: Apply stays dimmed: {} row(s), validity {}, busy {}, solve error {:?}",
            model.get_rows().row_count(),
            model.get_validity_state(),
            model.get_is_busy(),
            model.get_solve_error().as_str()
        );
    }
    pump(300);
}

/// The Edit as Text dialog with one extra line typed in. `step`: 0 the edited page, 1 with the
/// discard question, 2 with the Compare With... menu open, 3 with a comparison showing.
fn raw_text_edited(a: &HarnessApp, step: u8) {
    use indicatrix_cut::RawTextModel;
    edit_tab(a);
    let raw = a.ui().global::<RawTextModel>();
    raw.invoke_open();
    let _ = wait_until(Duration::from_secs(8), || {
        !raw.get_busy() && !raw.get_text().is_empty()
    });
    let edited = format!("{}\n", raw.get_text());
    raw.set_text(edited.as_str().into());
    raw.invoke_text_edited(edited.as_str().into());
    let _ = wait_until(Duration::from_secs(5), || raw.get_text_differs());
    pump(200);
    match step {
        1 => raw.set_confirm_discard(true),
        2 => {
            press(a, "Compare With...", None);
        }
        3 => {
            raw.set_compare_title("Your text against the saved file".into());
            raw.set_compare_summary("One line differs.".into());
            raw.set_diff_rows(ModelRc::new(VecModel::from(vec![
                RawTextDiffRow {
                    kind: 0,
                    old_line: "a".into(),
                    new_line: "a".into(),
                    text: "a".into(),
                },
                RawTextDiffRow {
                    kind: 1,
                    old_line: "b".into(),
                    new_line: "c".into(),
                    text: "c".into(),
                },
            ])));
            raw.set_compare_open(true);
        }
        _ => {}
    }
    pump(250);
}

/// The Remote coordinator dialog with an endpoint saved in the model (no network is touched:
/// only the Test, Save, Mirror and Switch buttons would, and they are denied); `edit` presses the
/// pencil so the form (name, address, certificates, enrollment, transfer) is showing.
fn remote_configured(a: &HarnessApp, edit: bool) {
    let remote = a.ui().global::<RemoteWorkerModel>();
    remote.invoke_open_dialog();
    pump(150);
    remote.set_endpoint(WorkerItem {
        name: "Harness coordinator".into(),
        address: "127.0.0.1:9".into(),
        cert_dir: "".into(),
        transfer_mode_index: 0,
        cadence_ms: 500,
        preview_scale_index: 3,
        preview_scale_percent: 50,
        export_transfer_index: 0,
    });
    remote.set_configured(true);
    remote.set_availability(RemoteAvailabilityItem {
        configured: true,
        reachable_render_capable: true,
    });
    pump(250);
    if edit {
        // Browse... opens a folder dialog: answer it with a cancel.
        harness_support::queue_picker_cancel();
        harness_support::queue_picker_cancel();
        press(a, "Edit this remote coordinator", None);
        pump(250);
    }
}

/// Two queued jobs whose rows offer every action the row has, the queue shown as running.
fn jobs_all_buttons(a: &HarnessApp) {
    jobs_with_rows(a);
    let jobs = a.ui().global::<RenderJobsModel>();
    let rows: Vec<_> = jobs
        .get_rows()
        .iter()
        .map(|mut row| {
            row.can_pause = true;
            row.can_resume = true;
            row.can_restart = true;
            row.can_cancel = true;
            row.can_delete = true;
            row.can_move_up = true;
            row.can_move_down = true;
            row.can_show = false;
            row
        })
        .collect();
    jobs.set_rows(ModelRc::new(VecModel::from(rows)));
    jobs.set_queue_running(true);
    pump(250);
}

/// The script-export page of the jobs panel with jobs to export; the folder dialog is answered
/// with a folder of the profile's working directory.
fn jobs_script_export(a: &HarnessApp) {
    jobs_with_rows(a);
    let jobs = a.ui().global::<RenderJobsModel>();
    jobs.set_script_open(true);
    jobs.set_script_job_count(2);
    if let Ok(dir) = std::env::current_dir().map(|d| d.join("harness_script_exports")) {
        let _ = std::fs::create_dir_all(&dir);
        harness_support::queue_picker_path(dir.clone());
        harness_support::queue_picker_path(dir);
    }
    pump(250);
}

/// The History tab on the variants page with variants saved through the form (real saves: two
/// for steps 0 and 3, one for the form steps).
/// `step`: 0 the list, 1 the save form open, 2 the note form of the newest variant, 3 the text
/// comparison of the newest against the older one.
fn variants_saved(a: &HarnessApp, step: u8) {
    inspector(a, 4);
    let variants = a.ui().global::<VariantsModel>();
    variants.set_view(1);
    pump(300);
    // Each save costs about two seconds of real time and the pass restores the state a dozen
    // times, so only the steps that need two variants (the list, the comparison) save two.
    let saves = if matches!(step, 0 | 3) { 2 } else { 1 };
    for (index, name) in ["Harness A", "Harness B"]
        .into_iter()
        .take(saves)
        .enumerate()
    {
        variants.invoke_begin_save(-1);
        pump(150);
        variants.set_form_name(name.into());
        variants.invoke_submit_form();
        let wanted = index + 1;
        let _ = wait_until(Duration::from_secs(10), || {
            variants.get_rows().row_count() >= wanted
        });
        pump(250);
    }
    match step {
        1 => variants.invoke_begin_save(-1),
        2 => {
            if let Some(row) = variants.get_rows().row_data(0) {
                variants.invoke_begin_note(row.key);
            }
        }
        3 => {
            variants.invoke_compare_text();
            let _ = wait_until(Duration::from_secs(10), || {
                variants.get_diff_open() && !variants.get_diff_busy()
            });
        }
        _ => {}
    }
    pump(300);
}
