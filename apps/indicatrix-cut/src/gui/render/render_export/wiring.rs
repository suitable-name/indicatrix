//! Slint callback wiring for the export dialog: kicking off/cancelling a fan-out
//! export ([`setup_render_export_callbacks`]), and the dialog's smaller supporting
//! callbacks (remote-availability probe, preset-fanout list, output-location fields,
//! per-preset checkbox toggle).

use super::queue::{
    ExportJob, ExportQueue, apply_export_bounce_cap, apply_preset_to_scene, colorspace_label,
    compute_target_from_index, start_next_export_job,
};
use crate::{
    ExportModel, LibraryModel, LightingPresetItem, MainWindow,
    bridge::export_thread::{self, ComputeTarget, ExportParams, SceneSnapshot},
    settings::{LightingPreset as SavedLightingPreset, SettingsPersister, WorkerSettings},
};
use indicatrix::color::ColorSpace;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::RefCell,
    collections::VecDeque,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
};

// NOTE: this function has no doc comment of its own. A 9-paragraph doc comment
// describing "the high-resolution export flow" sits immediately above
// `color_space_from_index` instead (no blank line between them), so rustdoc
// actually attaches it to THAT function -- see `color_space_from_index` in
// `gui/startup_settings.rs`, which carries it verbatim.
//
// The preset-fan-out/queue-assembly/spawn tail lives in its own function
// ([`finish_start_export`]), which lets the export-directory picker run off the
// UI thread and keeps this function under clippy's function-length lint. Left
// unmarked rather than adding a speculative `#[expect(clippy::too_many_lines)]`:
// if a future change grows this back past the limit, clippy's own warning is
// the right signal to split again, not a standing `#[expect]` nobody is
// watching.
pub(in crate::gui) fn setup_render_export_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<crate::bridge::render_thread::RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    setup_check_remote_availability_callback(ui, settings_store);
    setup_populate_export_fanout_presets_callback(ui, settings_store);
    setup_toggle_export_preset_selected_callback(ui);
    setup_export_output_location_callbacks(ui, settings_store);

    // The queue currently running, if any -- plain UI-thread-only state (both
    // callbacks below only ever run on the Slint event loop), so `Rc<RefCell<_>>>` is
    // the right container for THIS handle even though `ExportQueue` itself, one layer
    // in, has to be `Arc<Mutex<_>>>` -- see this group's own `mod.rs` doc comment.
    let export_queue: Rc<RefCell<Option<Arc<Mutex<ExportQueue>>>>> = Rc::new(RefCell::new(None));

    let render_ctx_start = render_ctx.clone();
    let settings_store_start = settings_store.clone();
    let export_queue_start = export_queue.clone();
    let ui_weak_start = ui.as_weak();
    ui.global::<ExportModel>().on_start_export(
        move |width: i32,
              height: i32,
              samples: i32,
              color_space_index: i32,
              compute_target_index: i32,
              max_bounces: i32| {
            let Some(ui) = ui_weak_start.upgrade() else {
                return;
            };

            let params =
                match export_thread::validate_export_params(width, height, samples, max_bounces) {
                    Ok(params) => params,
                    Err(err) => {
                        ui.global::<ExportModel>().set_has_error(true);
                        ui.global::<ExportModel>().set_status_message(err.into());
                        return;
                    }
                };
            let color_space = crate::gui::color_space_from_index(color_space_index);
            let compute_target = compute_target_from_index(compute_target_index);
            // A snapshot of whatever workers are configured RIGHT NOW -- `run_export`
            // re-probes the first one itself (see its own doc comment on why this
            // isn't trusted from whatever the dialog observed when it opened), so this
            // is just the current list, not a cached capability.
            let workers = settings_store_start.snapshot().settings.remote_workers;

            // ---- Export directory, prompted ONCE ---------------------------------------
            // A native FOLDER picker (not Save-As -- the template below owns the
            // filename now), shown only when no directory has been chosen yet. Once
            // chosen, it's persisted immediately so every future export -- in this
            // session and every one after -- never prompts again.
            //
            // The picker itself runs off the UI thread via `gui::pickers::pick`;
            // everything that runs after it (preset fan-out, queue assembly, spawn)
            // lives in [`finish_start_export`], its own continuation, bundled via
            // [`StartExportContext`] purely to keep both functions under clippy's
            // argument-count lint.
            let configured = settings_store_start.snapshot().settings.export_directory;
            let context = StartExportContext {
                render_ctx: render_ctx_start.clone(),
                settings_store: settings_store_start.clone(),
                export_queue: export_queue_start.clone(),
                params,
                color_space,
                compute_target,
                workers,
            };
            if configured.is_empty() {
                crate::gui::pickers::pick(
                    &ui,
                    crate::gui::pickers::PickerRequest {
                        kind: crate::gui::pickers::PickerKind::PickFolder,
                        title: Some("Choose an export folder".to_string()),
                        filters: Vec::new(),
                        default_file_name: None,
                        starting_dir: None,
                    },
                    move |ui, dir| {
                        let Some(dir) = dir else {
                            ui.global::<ExportModel>().set_has_error(false);
                            ui.global::<ExportModel>()
                                .set_status_message("Export cancelled.".into());
                            return;
                        };
                        context.settings_store.update(|s| {
                            s.settings.export_directory = dir.to_string_lossy().into_owned();
                        });
                        finish_start_export(ui, dir, context);
                    },
                );
            } else {
                finish_start_export(&ui, PathBuf::from(configured), context);
            }
        },
    );

    let export_queue_cancel = export_queue;
    ui.global::<ExportModel>().on_cancel_export(move || {
        if let Some(queue) = export_queue_cancel.borrow().as_ref() {
            let mut q = queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            q.cancelled = true;
            if let Some(handle) = &q.current_handle {
                handle.cancel();
            }
        }
    });
}

/// Everything [`finish_start_export`] needs beyond `ui`/the resolved export
/// directory -- bundled into one struct (rather than six more parameters)
/// purely to keep both it and [`setup_render_export_callbacks`] under
/// clippy's argument-count lint, the same reasoning every other multi-field
/// bundle in this crate uses (e.g. `gui::editor::callbacks::solve_actions::
/// RunProvenance`).
struct StartExportContext {
    render_ctx: Arc<Mutex<crate::bridge::render_thread::RenderContext>>,
    settings_store: Arc<SettingsPersister>,
    export_queue: Rc<RefCell<Option<Arc<Mutex<ExportQueue>>>>>,
    params: ExportParams,
    color_space: ColorSpace,
    compute_target: ComputeTarget,
    workers: Vec<WorkerSettings>,
}

/// The rest of "Start Export", once the export directory is known -- split out
/// of [`setup_render_export_callbacks`]'s `on_start_export` handler so the SAME
/// preset-fan-out/queue-assembly/spawn logic runs whether the directory was
/// already configured (synchronously) or just picked (from `gui::pickers::pick`'s
/// own continuation, off the UI thread for the dialog itself).
fn finish_start_export(ui: &MainWindow, export_dir: PathBuf, context: StartExportContext) {
    let StartExportContext {
        render_ctx,
        settings_store,
        export_queue,
        params,
        color_space,
        compute_target,
        workers,
    } = context;

    let template = settings_store.snapshot().settings.export_filename_template;

    // The export's OWN (already-validated) bounce cap, not whatever the live
    // viewport is set to -- see `apply_export_bounce_cap`'s own doc comment.
    let base_scene =
        apply_export_bounce_cap(SceneSnapshot::capture(&render_ctx), params.max_bounces);

    // ---- Preset fan-out ---------------------------------------------------------
    // Only presets BOTH marked `export_usable` (the settings dialog's checkbox)
    // AND checked in this dialog's own fan-out list -- see
    // `GemViewportView.export_fanout_presets`'s own doc comment for why that's a
    // separate, dialog-scoped list rather than a filtered view of the settings
    // dialog's.
    let selected_presets: Vec<SavedLightingPreset> = {
        let snapshot = settings_store.snapshot();
        ui.global::<ExportModel>()
            .get_fanout_presets()
            .iter()
            .filter(|item| item.selected)
            .filter_map(|item| {
                snapshot
                    .presets
                    .iter()
                    .find(|p| p.name == item.name.as_str())
                    .cloned()
            })
            .collect()
    };
    // An HDR environment map does NOT force this export onto a
    // slower CPU-only path -- the GPU megakernel has its own `env_mode` for
    // `HdrMap` and renders it directly (see `render_thread::mod`'s module doc
    // comment), so there is nothing to warn the user about here.
    let mut jobs = VecDeque::with_capacity(1 + selected_presets.len());
    jobs.push_back(ExportJob {
        scene: base_scene.clone(),
        preset_label: String::new(),
    });
    for preset in &selected_presets {
        jobs.push_back(ExportJob {
            scene: apply_preset_to_scene(base_scene.clone(), preset),
            preset_label: preset.name.clone(),
        });
    }
    let total = jobs.len();

    let detail = ui.global::<LibraryModel>().get_current_detail();

    // Pause live-viewport tracing for the duration of the WHOLE queue -- see
    // `RenderContext::export_active`'s own doc comment. Read the local CPU/GPU
    // choice from the SAME short lock, so every job in this queue traces with
    // the setting in force at the instant the export started (see the
    // pre-fan-out version of this comment for why re-reading it mid-run would
    // be wrong).
    let local_compute = {
        // A COUNT, not a bool -- see
        // `RenderContext::export_active_count`'s doc comment.
        let mut guard = crate::bridge::render_thread::RenderContext::lock(&render_ctx);
        guard.export_active_count += 1;
        guard.local_compute_target
    };

    let queue = Arc::new(Mutex::new(ExportQueue {
        jobs,
        total,
        current_index: 0,
        completed: Vec::new(),
        failures: Vec::new(),
        cancelled: false,
        current_handle: None,
        export_dir,
        template,
        design: detail.title.to_string(),
        designer: detail.designer.to_string(),
        shape: detail.shape.to_string(),
        ri: detail.ri.to_string(),
        width: params.width,
        height: params.height,
        spp: params.samples_per_pixel,
        bounces: params.max_bounces,
        colorspace: colorspace_label(color_space).to_string(),
        params,
        color_space,
        compute_target,
        workers,
        local_compute,
    }));

    ui.global::<ExportModel>().set_is_exporting(true);
    ui.global::<ExportModel>().set_has_error(false);
    ui.global::<ExportModel>()
        .set_status_message(String::new().into());

    *export_queue.borrow_mut() = Some(queue.clone());
    start_next_export_job(ui, &render_ctx, &queue);
}

/// Wires `check_remote_availability` (fired by `gem_viewport.slint`'s `changed
/// export_open` handler every time the export dialog opens -- see that file's own
/// comment) to a background probe of the first configured worker, reporting the result
/// into `export_remote_available`/`export_remote_unavailable_reason` so
/// `export_dialog.slint`'s Compute pill can grey out Remote/Local+Remote WITH a reason
/// rather than leaving them silently missing. Follows `on_test_worker_connection`'s
/// exact shape (`gui::remote::worker_callbacks`): a blocking probe on its own
/// `std::thread::spawn` thread, result marshalled back via `upgrade_in_event_loop`.
fn setup_check_remote_availability_callback(
    ui: &MainWindow,
    settings_store: &Arc<SettingsPersister>,
) {
    let settings_store = settings_store.clone();
    let ui_weak = ui.as_weak();
    ui.global::<ExportModel>()
        .on_check_remote_availability(move || {
            let workers = settings_store.snapshot().settings.remote_workers;
            let ui_weak_result = ui_weak.clone();
            std::thread::spawn(move || {
                let result = export_thread::probe_remote(&workers);
                let _ = ui_weak_result.upgrade_in_event_loop(move |ui| match result {
                    Ok(_capability) => {
                        ui.global::<ExportModel>().set_remote_available(true);
                        ui.global::<ExportModel>()
                            .set_remote_unavailable_reason(String::new().into());
                    }
                    Err(reason) => {
                        ui.global::<ExportModel>().set_remote_available(false);
                        ui.global::<ExportModel>()
                            .set_remote_unavailable_reason(reason.message().into());
                    }
                });
            });
        });
}

/// Rebuilds `export_fanout_presets` -- the export dialog's own preset-fan-out list --
/// from every CURRENTLY `export_usable` preset (see `LightingPresetItem.export_usable`'s
/// own doc comment), each starting unchecked. Fired every time the export dialog opens
/// (`gem_viewport.slint`'s `changed export_open`, alongside `check_remote_availability`),
/// so a preset marked/unmarked usable since the dialog last opened is picked up, and a
/// checkbox left checked from a previous export doesn't carry over into a fresh one.
fn setup_populate_export_fanout_presets_callback(
    ui: &MainWindow,
    settings_store: &Arc<SettingsPersister>,
) {
    let settings_store = settings_store.clone();
    let ui_weak = ui.as_weak();
    ui.global::<ExportModel>()
        .on_populate_fanout_presets(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let snapshot = settings_store.snapshot();
            let items: Vec<LightingPresetItem> = snapshot
                .presets
                .into_iter()
                .filter(|p| p.export_usable)
                .map(|p| LightingPresetItem {
                    name: p.name.into(),
                    built_in: p.built_in,
                    export_usable: true,
                    has_env_map: p.env_map_path.is_some(),
                    selected: false,
                })
                .collect();
            ui.global::<ExportModel>()
                .set_fanout_presets(ModelRc::new(VecModel::from(items)));
            // Output location -- refreshed every time the dialog opens (same trigger as
            // the preset list above) so an external edit to the settings file, or a
            // directory changed from a different code path, is always reflected rather
            // than whatever was last shown.
            ui.global::<ExportModel>()
                .set_directory(snapshot.settings.export_directory.into());
            ui.global::<ExportModel>()
                .set_filename_template(snapshot.settings.export_filename_template.into());
        });
}

/// Wires the "Change..." export-directory button and the filename-template text
/// field's live-edit persistence -- see `export_dialog.slint`'s "Output Location"
/// section for both controls.
fn setup_export_output_location_callbacks(
    ui: &MainWindow,
    settings_store: &Arc<SettingsPersister>,
) {
    let settings_store_dir = settings_store.clone();
    let ui_weak_dir = ui.as_weak();
    ui.global::<ExportModel>().on_change_directory(move || {
        let Some(ui) = ui_weak_dir.upgrade() else {
            return;
        };
        let current = settings_store_dir.snapshot().settings.export_directory;
        let settings_store_dir = settings_store_dir.clone();
        // The picker runs off the UI thread via `gui::pickers::pick`. Cancelling
        // leaves the previously configured directory untouched -- same "declining
        // changes nothing" convention this dialog's other pickers follow.
        crate::gui::pickers::pick(
            &ui,
            crate::gui::pickers::PickerRequest {
                kind: crate::gui::pickers::PickerKind::PickFolder,
                title: Some("Choose an export folder".to_string()),
                filters: Vec::new(),
                default_file_name: None,
                starting_dir: (!current.is_empty()).then(|| PathBuf::from(current)),
            },
            move |ui, dir| {
                let Some(dir) = dir else {
                    return;
                };
                let dir_string = dir.to_string_lossy().into_owned();
                settings_store_dir.update(|s| s.settings.export_directory.clone_from(&dir_string));
                ui.global::<ExportModel>().set_directory(dir_string.into());
            },
        );
    });

    let settings_store_template = settings_store.clone();
    ui.global::<ExportModel>()
        .on_filename_template_changed(move |text| {
            settings_store_template
                .update(|s| s.settings.export_filename_template = text.to_string());
        });

    // The filename template field needs a live preview and a way to flag a
    // typo'd placeholder before actually exporting. Both callbacks
    // render against one fixed, clearly-labelled EXAMPLE `TemplateContext` (not the
    // live selected design) -- reusing `filename_template::render` here would mean
    // threading the real selected design/material/camera state down through
    // `gem_viewport.slint`/`export_dialog.slint` for a "preview" that is, by
    // definition, only ever seen while the dialog is open and something is already
    // loaded; a representative example demonstrates the mechanism (and catches typos)
    // without that extra wiring, at the cost of not literally matching the current
    // design's own title.
    ui.global::<ExportModel>().on_preview_filename_template(
        move |template: slint::SharedString| {
            let rendered = export_thread::filename_template::render(
                &template,
                &example_template_context(),
                std::time::SystemTime::now(),
            );
            format!("{rendered}.png").into()
        },
    );
    ui.global::<ExportModel>()
        .on_filename_template_unknown_placeholders(move |template: slint::SharedString| {
            unknown_placeholders(&template).into()
        });
}

/// A representative, clearly-fictional set of template values for
/// `on_preview_filename_template`'s live "resolves to" preview -- see that callback's
/// own doc comment for why this is a fixed example rather than the live selected
/// design.
fn example_template_context() -> export_thread::TemplateContext {
    export_thread::TemplateContext {
        design: "Example Design".to_string(),
        designer: "Jane Doe".to_string(),
        shape: "Round Brilliant".to_string(),
        material: "Diamond".to_string(),
        ri: "2.417".to_string(),
        width: 1920,
        height: 1080,
        spp: 512,
        bounces: 12,
        colorspace: "sRGB".to_string(),
        preset: "Studio".to_string(),
        lighting: "D65 Daylight".to_string(),
        yaw_deg: 48.0,
        pitch_deg: 54.0,
        distance: 3.5,
        exposure: 1.0,
    }
}

/// Which `{name}` placeholders in `template` `filename_template::render` does NOT
/// recognise -- reported as a comma-separated list (empty string if none), for
/// `export_dialog.slint`'s own "flag unknown placeholders" legend.
///
/// Deliberately does not duplicate `filename_template::substitute`'s own match arms
/// as a second, hand-maintained list of known names (a `mod.rs`/`gui` copy would
/// silently drift the moment one side gained a variable the other didn't): every
/// unrecognised `{name}` is emitted back literally by `render` (see that function's
/// own doc comment), so re-scanning the rendered output for surviving `{...}` tokens
/// is exactly the set this function wants, with no second list to keep in sync.
fn unknown_placeholders(template: &str) -> String {
    let rendered = export_thread::filename_template::render(
        template,
        &example_template_context(),
        std::time::SystemTime::now(),
    );
    let mut found = Vec::new();
    let mut rest = rendered.as_str();
    while let Some(open) = rest.find('{') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('}') else {
            break;
        };
        let name = &after_open[..close];
        if !found.iter().any(|f: &String| f == name) {
            found.push(name.to_string());
        }
        rest = &after_open[close + 1..];
    }
    found.join(", ")
}

/// Flips one row's checkbox in `export_fanout_presets` in place -- `idx` is this list's
/// OWN index (rebuilt fresh by `setup_populate_export_fanout_presets_callback` above
/// every time the dialog opens), not the settings dialog's full preset-list index.
fn setup_toggle_export_preset_selected_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<ExportModel>()
        .on_toggle_preset_selected(move |idx: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(idx) = usize::try_from(idx) else {
                return;
            };
            let model = ui.global::<ExportModel>().get_fanout_presets();
            if let Some(mut item) = model.row_data(idx) {
                item.selected = !item.selected;
                model.set_row_data(idx, item);
            }
        });
}
