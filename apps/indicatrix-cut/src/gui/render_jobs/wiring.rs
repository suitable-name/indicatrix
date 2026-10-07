//! The Slint side of the render queue: the callbacks of `RenderJobsModel`, and writing the
//! rows, the queue line, the chip and the script facts into it.
//!
//! The callbacks only forward to the controller or to the "Add to Queue" and script
//! export modules; every decision lives in plain Rust beside them (see [`super::rows`] and
//! [`super::controller`]).

use super::{
    capture_still::{self, StillArgs},
    capture_video,
    controller::{JobController, install, unix_now, with_controller},
    rows::{QueueView, RowView},
    script_export,
};
use crate::{
    MainWindow, RenderJobRow, RenderJobsModel,
    bridge::render_thread::RenderContext,
    settings::{SettingsPersister, WorkerSettings},
};
use indicatrix_render_jobs::state::{INTERRUPTED_NOTE, JobCommand};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    rc::Rc,
    sync::{Arc, Mutex},
};

/// What "Add to Queue" and the script export need from the app: shared handles, cloned
/// into each callback.
#[derive(Clone)]
pub(super) struct Deps {
    pub(super) db: Arc<Mutex<Database>>,
    pub(super) render_ctx: Arc<Mutex<RenderContext>>,
    pub(super) settings_store: Arc<SettingsPersister>,
    /// The live mesh radius a preset's camera distance is clamped against (see
    /// `render_export::wiring::fan_out_jobs`).
    pub(super) mesh_bounding_radius: Arc<Mutex<f64>>,
}

/// Sets up the render queue: marks a job the last session left running as paused, creates
/// the controller, wires `RenderJobsModel`, and shows the waiting jobs in the header chip.
pub fn setup_render_jobs(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    mesh_bounding_radius: &Arc<Mutex<f64>>,
) {
    let interrupted = db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .interrupt_running_render_jobs(INTERRUPTED_NOTE, unix_now());
    match interrupted {
        Ok(0) => {}
        Ok(count) => tracing::info!("{count} render job(s) were running when the app closed"),
        Err(error) => tracing::warn!("Render jobs left running could not be paused: {error}"),
    }

    install(Rc::new(JobController::new(
        ui.as_weak(),
        Arc::clone(db),
        Arc::clone(render_ctx),
        Arc::clone(settings_store),
    )));
    let deps = Deps {
        db: Arc::clone(db),
        render_ctx: Arc::clone(render_ctx),
        settings_store: Arc::clone(settings_store),
        mesh_bounding_radius: Arc::clone(mesh_bounding_radius),
    };

    let model = ui.global::<RenderJobsModel>();
    model.on_refresh(|| {
        with_controller(JobController::refresh);
    });
    model.on_start_queue(|| {
        with_controller(JobController::start_queue);
    });
    model.on_pause_queue(|| {
        with_controller(JobController::pause_queue);
    });
    model.on_clear_finished(|| {
        with_controller(JobController::clear_finished);
    });
    model.on_show_result(|id| {
        with_controller(|controller| controller.show_result(i64::from(id)));
    });
    // The row buttons: one callback each, one command each.
    let command = |command: JobCommand| {
        move |id: i32| {
            with_controller(|controller| controller.command(i64::from(id), command));
        }
    };
    model.on_pause_job(command(JobCommand::Pause));
    model.on_resume_job(command(JobCommand::Resume));
    model.on_restart_job(command(JobCommand::Restart));
    model.on_cancel_job(command(JobCommand::Cancel));
    model.on_delete_job(command(JobCommand::Delete));
    model.on_run_next(command(JobCommand::RunNext));
    model.on_move_job(|id, places| {
        let command = if places < 0 {
            JobCommand::MoveUp
        } else {
            JobCommand::MoveDown
        };
        with_controller(|controller| controller.command(i64::from(id), command));
    });

    let ui_weak = ui.as_weak();
    let deps_script = deps.clone();
    model.on_export_script(move || {
        if let Some(ui) = ui_weak.upgrade() {
            script_export::export_script(&ui, &deps_script);
        }
    });

    let ui_weak = ui.as_weak();
    let deps_still = deps.clone();
    model.on_add_still_jobs(
        move |width, height, samples, color_space_index, compute_target_index, max_bounces| {
            if let Some(ui) = ui_weak.upgrade() {
                capture_still::add_still_jobs(
                    &ui,
                    &deps_still,
                    StillArgs {
                        width,
                        height,
                        samples,
                        color_space_index,
                        compute_target_index,
                        max_bounces,
                    },
                );
            }
        },
    );

    let ui_weak = ui.as_weak();
    model.on_add_tilt_video_job(move |axis_index| {
        if let Some(ui) = ui_weak.upgrade() {
            capture_video::add_tilt_video_job(&ui, &deps, axis_index);
        }
    });

    // The header chip shows waiting jobs from the start.
    with_controller(JobController::refresh);
}

/// The Slint row for `row`.
fn to_slint(row: &RowView) -> RenderJobRow {
    RenderJobRow {
        id: row.id,
        number_text: row.number_text.as_str().into(),
        label: row.label.as_str().into(),
        kind_text: row.kind_text.as_str().into(),
        state_word: row.state_word.as_str().into(),
        state_text: row.state_text.as_str().into(),
        summary: row.summary.as_str().into(),
        progress: row.progress,
        progress_text: row.progress_text.as_str().into(),
        error_text: row.error_text.as_str().into(),
        can_pause: row.can_pause,
        can_resume: row.can_resume,
        can_restart: row.can_restart,
        can_cancel: row.can_cancel,
        can_delete: row.can_delete,
        can_run_next: row.can_run_next,
        can_move_up: row.can_move_up,
        can_move_down: row.can_move_down,
        can_show: row.can_show,
    }
}

/// Shows `view`: the rows (changed rows only, so the list keeps its scroll position and a
/// progress tick does not rebuild it), the queue line and the header chip.
pub(super) fn publish(ui: &MainWindow, view: &QueueView) {
    let model = ui.global::<RenderJobsModel>();
    let rows: Vec<RenderJobRow> = view.rows.iter().map(to_slint).collect();
    let current = model.get_rows();
    if let Some(vec_model) = current.as_any().downcast_ref::<VecModel<RenderJobRow>>() {
        if vec_model.row_count() == rows.len() {
            for (index, row) in rows.into_iter().enumerate() {
                if vec_model.row_data(index).is_none_or(|old| old != row) {
                    vec_model.set_row_data(index, row);
                }
            }
        } else {
            vec_model.set_vec(rows);
        }
    } else {
        model.set_rows(ModelRc::new(VecModel::from(rows)));
    }
    model.set_queue_running(view.queue_running);
    model.set_can_start_queue(view.can_start_queue);
    model.set_queue_status(view.queue_status.as_str().into());
    model.set_chip_visible(view.chip_visible);
    model.set_chip_text(view.chip_text.as_str().into());
    model.set_chip_progress(view.chip_progress);
}

/// Shows what the Export Script section needs: how many jobs it would write, and the remote
/// worker it can render on (none when Settings has no worker).
pub(super) fn publish_script_facts(
    ui: &MainWindow,
    unfinished: usize,
    worker: Option<&WorkerSettings>,
) {
    let model = ui.global::<RenderJobsModel>();
    model.set_script_job_count(i32::try_from(unfinished).unwrap_or(i32::MAX));
    model.set_script_remote_available(worker.is_some());
    model.set_script_remote_label(worker.map_or("", |w| w.address.as_str()).into());
    // The choice cannot stay on a worker that is no longer set up.
    if worker.is_none() && model.get_script_target_index() == 1 {
        model.set_script_target_index(0);
    }
}
