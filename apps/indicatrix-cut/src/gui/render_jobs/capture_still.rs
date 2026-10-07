//! "Add to Queue" in the export dialog: the same steps as "Start Export", up to the point
//! where it would render, then one render job per picture instead.
//!
//! The steps mirror `gui::render::render_export::wiring` (validate, export folder, finished
//! stone, scene capture, preset fan-out, file names) and reuse its pieces, so a queued
//! picture is named and lit exactly like one exported directly. The pure helpers
//! ([`plan_still_outputs`], [`still_label`], [`still_summary`]) are tested without a window.

use super::{controller::unix_now, wiring::Deps};
use crate::{
    ExportModel, MainWindow,
    bridge::export_thread::{
        self, ExportParams, RemoteSelection, SceneSnapshot, filename_template::resolve_export_name,
    },
    gui::render::render_export::{
        queue::{
            DesignNames, ExportJob, ExportSettingsView, apply_export_bounce_cap, colorspace_label,
            compute_target_from_index, template_context,
        },
        wiring::{design_naming, fan_out_jobs, resolve_export_dir_then},
    },
    settings::ExportTransfer,
};
use indicatrix::color::ColorSpace;
use indicatrix_render_jobs::{
    codec,
    job::{DesignInfo, JOB_FORMAT_VERSION},
    paths::reserve_unique_file,
};
use indicatrix_vault::model::render_job::NewRenderJob;
use slint::ComponentHandle;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};

/// The values of the export dialog's "Add to Queue" click, as Slint sends them.
#[derive(Debug, Clone, Copy)]
pub(super) struct StillArgs {
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) samples: i32,
    pub(super) color_space_index: i32,
    pub(super) compute_target_index: i32,
    pub(super) max_bounces: i32,
}

/// Where each picture of one "Add to Queue" goes: `export_dir` plus its file name, made
/// unique against the disk, against the output of every job already in the queue (`taken`)
/// and against the earlier names of this batch.
pub fn plan_still_outputs(
    export_dir: &Path,
    names: &[String],
    taken: &[PathBuf],
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let mut reserved: Vec<PathBuf> = taken.to_vec();
    let mut planned = Vec::with_capacity(names.len());
    for name in names {
        let path = reserve_unique_file(&export_dir.join(name), &reserved, exists);
        reserved.push(path.clone());
        planned.push(path);
    }
    planned
}

/// The name a still job has in the list: `Design · Preset · 1920×1080`, with "Current view"
/// for the picture that has no preset.
pub fn still_label(design: &str, preset: &str, width: u32, height: u32) -> String {
    let view = if preset.is_empty() {
        "Current view"
    } else {
        preset
    };
    format!("{design} \u{b7} {view} \u{b7} {width}\u{d7}{height}")
}

/// The detail line of a still job: `1920 × 1080 · 1024 samples · 12 bounces · sRGB · Daylight`.
pub fn still_summary(params: &ExportParams, color_space: &str, lighting: &str) -> String {
    format!(
        "{} \u{d7} {} \u{b7} {} samples \u{b7} {} bounces \u{b7} {color_space} \u{b7} {lighting}",
        params.width, params.height, params.samples_per_pixel, params.max_bounces
    )
}

/// A refusal: shown in the export dialog, red, as the export itself does.
fn refuse(ui: &MainWindow, message: &str) {
    let model = ui.global::<ExportModel>();
    model.set_has_error(true);
    model.set_status_message(message.into());
}

/// "Add to Queue": validates the settings, finds the export folder and the finished stone,
/// and adds one job per picture. Nothing is added when anything is refused.
pub(super) fn add_still_jobs(ui: &MainWindow, deps: &Deps, args: StillArgs) {
    let params = match export_thread::validate_export_params(
        args.width,
        args.height,
        args.samples,
        args.max_bounces,
    ) {
        Ok(params) => params,
        Err(message) => {
            refuse(ui, &message);
            return;
        }
    };
    let color_space = crate::gui::color_space_from_index(args.color_space_index);
    let snapshot = deps.settings_store.snapshot();
    let remote = RemoteSelection {
        compute_target: compute_target_from_index(args.compute_target_index),
        worker: snapshot.settings.remote_worker(),
        transfer: ExportTransfer::from_index(ui.global::<ExportModel>().get_transfer_index()),
        contribute_local: snapshot.settings.contribute_to_final_picture,
    };
    let deps = deps.clone();
    let settings_store = deps.settings_store.clone();
    resolve_export_dir_then(ui, &settings_store, "Nothing was added.", move |ui, dir| {
        // A closed picker already wrote "Nothing was added.".
        let Some(export_dir) = dir else {
            return;
        };
        let render_ctx = deps.render_ctx.clone();
        crate::gui::editor::finished_stone_then(
            ui,
            &render_ctx,
            |ui| {
                let model = ui.global::<ExportModel>();
                model.set_has_error(false);
                model.set_status_message("Preparing the finished stone...".into());
            },
            move |ui, finished| match finished {
                Ok(finished) => {
                    let request = AddRequest {
                        params,
                        color_space,
                        remote,
                        export_dir,
                    };
                    continue_add(ui, &deps, &request, finished.as_ref());
                }
                // The design does not solve, or changed while its stone was prepared.
                Err(withheld) => refuse(
                    ui,
                    &withheld.export_message().replacen(
                        "Nothing was exported",
                        "Nothing was added",
                        1,
                    ),
                ),
            },
        );
    });
}

/// What the second half of "Add to Queue" needs beyond the window and the dependencies.
struct AddRequest {
    params: ExportParams,
    color_space: ColorSpace,
    remote: RemoteSelection,
    export_dir: PathBuf,
}

/// One picture to queue: its job, label, summary and output path, ready to store.
struct Prepared {
    label: String,
    summary: String,
    output: PathBuf,
    text: String,
}

/// The design fields and file name template every picture of one "Add to Queue" shares.
struct Naming {
    design: String,
    designer: String,
    shape: String,
    ri: String,
    template: String,
}

/// The second half, once the finished stone is known: captures the scene, fans the presets
/// out, names the files, builds and stores the jobs.
fn continue_add(
    ui: &MainWindow,
    deps: &Deps,
    request: &AddRequest,
    finished: Option<&indicatrix_solid::preview::StoneGeometryBuf>,
) {
    let base_scene = match SceneSnapshot::capture_finished(&deps.render_ctx, finished) {
        Ok(scene) => apply_export_bounce_cap(scene, request.params.max_bounces),
        Err(reason) => {
            refuse(ui, &reason);
            return;
        }
    };
    let jobs = fan_out_jobs(
        ui,
        &deps.settings_store,
        &base_scene,
        &deps.mesh_bounding_radius,
    );
    let (design, designer, shape, ri) = design_naming(ui, &deps.render_ctx);
    let naming = Naming {
        design,
        designer,
        shape,
        ri,
        template: deps
            .settings_store
            .snapshot()
            .settings
            .export_filename_template,
    };
    let listed = deps
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .render_job_output_paths();
    let taken = match listed {
        Ok(paths) => paths.into_iter().map(PathBuf::from).collect::<Vec<_>>(),
        Err(error) => {
            refuse(
                ui,
                &format!("The render job list could not be read: {error}"),
            );
            return;
        }
    };

    // Build every job before storing any, so a refusal adds nothing.
    let now = unix_now();
    let prepared = match prepare_jobs(request, &jobs, &naming, &taken, now) {
        Ok(prepared) => prepared,
        Err(message) => {
            refuse(ui, &message);
            return;
        }
    };

    let total = prepared.len();
    let stored = store(deps, &prepared, now);
    super::controller::with_controller(super::controller::JobController::refresh);
    match stored {
        Ok(()) => {
            let message = format!(
                "Added {total} render {} to the queue. Start {} from File > Render Jobs...",
                if total == 1 { "job" } else { "jobs" },
                if total == 1 { "it" } else { "them" },
            );
            let model = ui.global::<ExportModel>();
            model.set_has_error(false);
            model.set_status_message(message.as_str().into());
            crate::gui::show_toast(ui, &message, "info");
        }
        Err((stored, message)) => refuse(
            ui,
            &format!("{message} ({stored} of {total} jobs were added.)"),
        ),
    }
}

/// One job per picture of `jobs`: named through the filename template (unique against the
/// disk, the queue's `taken` outputs and each other), frozen and encoded.
fn prepare_jobs(
    request: &AddRequest,
    jobs: &VecDeque<ExportJob>,
    naming: &Naming,
    taken: &[PathBuf],
    now: i64,
) -> Result<Vec<Prepared>, String> {
    let params = request.params;
    let colorspace = colorspace_label(request.color_space);
    let names: Vec<String> = jobs
        .iter()
        .map(|job| {
            let context = template_context(
                DesignNames {
                    design: &naming.design,
                    designer: &naming.designer,
                    shape: &naming.shape,
                    ri: &naming.ri,
                },
                ExportSettingsView {
                    width: params.width,
                    height: params.height,
                    spp: params.samples_per_pixel,
                    bounces: params.max_bounces,
                    colorspace,
                },
                job,
            );
            resolve_export_name(&naming.template, &context)
        })
        .collect();
    let outputs = plan_still_outputs(&request.export_dir, &names, taken, &|path| path.exists());

    let mut prepared = Vec::with_capacity(jobs.len());
    for (job, output) in jobs.iter().zip(outputs) {
        let label = still_label(
            &naming.design,
            &job.preset_label,
            params.width,
            params.height,
        );
        let file = super::convert::build_still_job(
            &super::convert::StillJobInputs {
                scene: &job.scene,
                params,
                color_space: request.color_space,
                remote: &request.remote,
                output: &output,
                label: label.clone(),
                design: DesignInfo {
                    title: naming.design.clone(),
                    designer: naming.designer.clone(),
                    shape: naming.shape.clone(),
                    ri: naming.ri.clone(),
                    material: job.scene.material.name.clone(),
                },
                preset_label: job.preset_label.clone(),
            },
            now,
        )?;
        let text = codec::to_text(&file).map_err(|error| error.to_string())?;
        prepared.push(Prepared {
            label,
            summary: still_summary(&params, colorspace, job.scene.lighting_preset.label()),
            output,
            text,
        });
    }
    Ok(prepared)
}

/// Stores the prepared jobs in order. On a database error, how many were stored before it
/// and what went wrong.
fn store(deps: &Deps, prepared: &[Prepared], now: i64) -> Result<(), (usize, String)> {
    let db = deps
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for (stored, job) in prepared.iter().enumerate() {
        let output = job.output.to_string_lossy();
        db.add_render_job(
            &NewRenderJob {
                kind: "still",
                label: &job.label,
                summary: &job.summary,
                frames_total: 1,
                output_path: &output,
                snapshot_version: JOB_FORMAT_VERSION,
                snapshot: &job.text,
            },
            now,
        )
        .map_err(|error| (stored, format!("The job could not be saved: {error}")))?;
    }
    drop(db);
    Ok(())
}
