//! "Add to Queue" in the tilt video section: the same request the direct "Start Video
//! Export" builds, frozen as a render job instead of started.
//!
//! `gui::tilt::video_export::prepare_queued_video` resolves the export folder and the
//! finished stone and builds the request without creating or starting anything; this
//! module reserves the frame folder against the disk and the queue, freezes the request
//! and stores it. The folder itself is created by the executor when the job first runs.

use super::{controller::unix_now, wiring::Deps};
use crate::{
    MainWindow, TiltVideoExportModel,
    gui::{render::render_export::wiring::design_naming, tilt::video_export},
};
use indicatrix::color::metrics::PROFILE_AZIMUTHS_DEG;
use indicatrix_render_jobs::{
    codec,
    job::{DesignInfo, JOB_FORMAT_VERSION},
    paths::reserve_unique_folder,
};
use indicatrix_vault::model::render_job::NewRenderJob;
use slint::ComponentHandle;
use std::path::PathBuf;

/// What a tilt video job's detail line says about the sweep and the picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoFacts {
    pub start_deg: f64,
    pub end_deg: f64,
    pub step_deg: f64,
    pub frames: usize,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub samples: u32,
}

/// The name a tilt video job has in the list: `Design · tilt video · axis 45°`.
pub fn video_label(design: &str, axis_deg: f32) -> String {
    format!("{design} \u{b7} tilt video \u{b7} axis {axis_deg:.0}\u{b0}")
}

/// An angle in the shortest form: `-90`, `0.5`.
fn degrees(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The detail line of a tilt video job:
/// `-90° to 90° in 1° steps · 181 frames · 30 fps · 1280 × 720 · 256 samples`.
pub fn video_summary(facts: &VideoFacts) -> String {
    format!(
        "{}\u{b0} to {}\u{b0} in {}\u{b0} steps \u{b7} {} frames \u{b7} {} fps \u{b7} {} \u{d7} {} \u{b7} {} samples",
        degrees(facts.start_deg),
        degrees(facts.end_deg),
        degrees(facts.step_deg),
        facts.frames,
        facts.fps,
        facts.width,
        facts.height,
        facts.samples,
    )
}

fn refuse(ui: &MainWindow, message: &str) {
    let model = ui.global::<TiltVideoExportModel>();
    model.set_has_error(true);
    model.set_status_message(message.into());
}

/// "Add to Queue" for the tilt video of `axis_index`.
pub(super) fn add_tilt_video_job(ui: &MainWindow, deps: &Deps, axis_index: i32) {
    let render_ctx = deps.render_ctx.clone();
    let settings_store = deps.settings_store.clone();
    let deps = deps.clone();
    video_export::prepare_queued_video(
        ui,
        &render_ctx,
        &settings_store,
        axis_index,
        move |ui, prepared| match prepared {
            // The person closed the folder picker.
            Ok(None) => {
                let model = ui.global::<TiltVideoExportModel>();
                model.set_has_error(false);
                model.set_status_message("Nothing was added.".into());
            }
            Ok(Some(request)) => queue_request(ui, &deps, request, axis_index),
            Err(message) => refuse(
                ui,
                &message.replacen("Nothing was exported", "Nothing was added", 1),
            ),
        },
    );
}

/// Freezes `request` and stores it as a job.
fn queue_request(
    ui: &MainWindow,
    deps: &Deps,
    mut request: video_export::run::VideoExportRequest,
    axis_index: i32,
) {
    let taken = {
        let db = deps
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        db.render_job_output_paths()
    };
    let taken: Vec<PathBuf> = match taken {
        Ok(paths) => paths.into_iter().map(PathBuf::from).collect(),
        Err(error) => {
            refuse(
                ui,
                &format!("The render job list could not be read: {error}"),
            );
            return;
        }
    };
    // Not created here: the executor creates the frame folder when the job first runs.
    request.out_dir = reserve_unique_folder(&request.out_dir, &taken, &|path| path.exists());

    let (design, designer, shape, ri) = design_naming(ui, &deps.render_ctx);
    let axis_deg = usize::try_from(axis_index)
        .ok()
        .and_then(|index| PROFILE_AZIMUTHS_DEG.get(index).copied())
        .unwrap_or(0.0);
    let label = video_label(&design, axis_deg);
    let summary = video_summary(&VideoFacts {
        start_deg: request.start_deg,
        end_deg: request.end_deg,
        step_deg: request.step_deg,
        frames: request.total_frames,
        fps: request.fps,
        width: request.width,
        height: request.height,
        samples: request.samples_per_pixel,
    });
    let now = unix_now();
    let design_info = DesignInfo {
        title: design,
        designer,
        shape,
        ri,
        material: request.scene.material.name.clone(),
    };
    let video_name = request.out_name.clone();
    let text =
        super::convert::build_video_job(&request, label.clone(), design_info, video_name, now)
            .and_then(|job| codec::to_text(&job).map_err(|e| e.to_string()));
    let text = match text {
        Ok(text) => text,
        Err(message) => {
            refuse(ui, &message);
            return;
        }
    };

    let output = request.out_dir.to_string_lossy().into_owned();
    let stored = {
        let db = deps
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        db.add_render_job(
            &NewRenderJob {
                kind: "tilt_video",
                label: &label,
                summary: &summary,
                frames_total: u32::try_from(request.total_frames).unwrap_or(u32::MAX),
                output_path: &output,
                snapshot_version: JOB_FORMAT_VERSION,
                snapshot: &text,
            },
            now,
        )
    };
    // Zoning builds: a zoned stone's zones travel beside the job (the frozen scene carries none).
    // If they cannot be stored the job is removed again rather than queued without its colour.
    #[cfg(feature = "zoning")]
    let stored = stored.and_then(|job_id| {
        let db = deps
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::gui::rough_colour::store::save_job_zoning(&db, job_id, &request.scene.material)
            .inspect_err(|_| {
                let _ = db.delete_render_job(job_id);
            })
            .map(|_| job_id)
    });
    match stored {
        Ok(_) => {
            super::controller::with_controller(super::controller::JobController::refresh);
            let message = "Added 1 render job to the queue. Start it from File > Render Jobs...";
            let model = ui.global::<TiltVideoExportModel>();
            model.set_has_error(false);
            model.set_status_message(message.into());
            crate::gui::show_toast(ui, message, "info");
        }
        Err(error) => refuse(ui, &format!("The job could not be saved: {error}")),
    }
}
