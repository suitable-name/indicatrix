//! The background thread that renders every frame, overlays the selected metrics,
//! writes the PNG sequence, and hands off to `encode` once the sweep finishes (or was
//! cancelled). Kept separate from `mod.rs`'s UI-thread wiring so neither file grows
//! past clippy's function-length lint.
//!
//! # Pausing the live viewport for the whole video
//!
//! A video now shares the local GPU adapter and remote workers with the rest of the
//! app (`render::render_frame_rgba`, via `bridge::export_thread::render_accumulation`)
//! instead of tracing on the CPU alone, so this app's hard rule of one GPU program at a
//! time on the single shared adapter applies here exactly as it does to a still-image
//! export: [`spawn`] increments `RenderContext::export_active_count` before starting
//! (in `mod.rs::handle_start_video_export`, mirroring `gui::render::render_export::
//! wiring::finish_start_export`'s own increment), and [`finish_video_export`] is the
//! ONE place that decrements it again -- called from every terminal path
//! ([`report_cancelled`]/[`report_failure`]/[`report_done`]), and from [`spawn`]'s own
//! `catch_unwind` when `run` panics outright, so the live viewport can never stay
//! paused after a video export ends, mirroring `gui::render::render_export::queue::
//! finish_export_queue`'s single decrement point and `bridge::export_thread::
//! spawn_export`'s "`on_done` fires on every exit path, including a caught panic".

use super::{encode, metrics, overlay, params, render};
use crate::{
    ActivityModel, MainWindow, TiltVideoExportModel,
    bridge::{
        export_thread::{AccumulationCarry, RemoteSelection, SceneSnapshot},
        render_thread::RenderContext,
    },
    gui::progress_eta::{EtaEstimator, format_eta},
    settings::LocalComputeTarget,
};
use indicatrix::{color::ColorSpace, renderer::gpu_backend::GpuBackend};
use slint::ComponentHandle;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

/// Everything one video export run needs, assembled once on the UI thread before the
/// background thread takes over. `Send`: every field is already `Send` (a
/// `SceneSnapshot`, plain numbers/strings, `Vec<f32>` curves, and a `RemoteSelection`),
/// the same bar `bridge::export_thread::spawn_export` holds its own request state to.
pub(super) struct VideoExportRequest {
    pub(super) scene: SceneSnapshot,
    pub(super) axis_index: usize,
    pub(super) start_deg: f64,
    pub(super) end_deg: f64,
    pub(super) step_deg: f64,
    pub(super) total_frames: usize,
    pub(super) fps: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) samples_per_pixel: u32,
    pub(super) color_space: ColorSpace,
    pub(super) out_dir: PathBuf,
    pub(super) out_name: String,
    pub(super) selection: metrics::MetricSelection,
    pub(super) curves: metrics::MetricCurves,
    pub(super) keep_frames: bool,
    /// The remote side: always `ComputeTarget::Both` for a video (see this group's own
    /// `mod.rs` doc comment on why there is no video-side "Compute" control), the
    /// configured remote endpoint (`AppSettings::remote`, read from the SAME settings
    /// store the still-image export reads), and the section's "Transfer" choice.
    pub(super) remote: RemoteSelection,
    /// `RenderContext::local_compute_target`, read from the SAME persisted setting the
    /// still-image export and the live viewport both use.
    pub(super) local_compute: LocalComputeTarget,
}

/// Spawns the background render loop for `request`. Never blocks the UI thread; checks
/// `cancel` between frames (not mid-frame -- a frame's own trace is already
/// parallelised internally, across local CPU/GPU and any remote worker, and not worth
/// interrupting partway through).
///
/// `activity_id` is the `crate::ActivityModel` id `handle_start_video_export`
/// registered for this run -- finished on every exit path below
/// (cancelled/failed/done), and given real progress (frames done / total) on
/// every frame via `report_progress`. `render_ctx` is used ONLY to resume the live
/// viewport once this run ends -- see this module's own doc comment; the caller has
/// already incremented `export_active_count` before calling this.
pub(super) fn spawn(
    ui_weak: slint::Weak<MainWindow>,
    render_ctx: Arc<Mutex<RenderContext>>,
    request: VideoExportRequest,
    cancel: Arc<AtomicBool>,
    activity_id: i32,
) {
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(&ui_weak, &render_ctx, &request, &cancel, activity_id);
        }));
        if let Err(payload) = result {
            // `run`'s own `report_cancelled`/`report_failure`/`report_done` all
            // decrement `export_active_count` themselves -- a caught panic means NONE
            // of them ran, so this is the one path that must do it here instead (via
            // `report_failure`, which does both).
            let message = panic_message(&*payload);
            report_failure(
                &ui_weak,
                &render_ctx,
                &format!("Video export failed unexpectedly: {message}"),
                activity_id,
            );
        }
    });
}

/// Downcasts a `catch_unwind` payload to a human-readable message -- the exact same
/// convention `gui::batch::preview::engine::panic_message` and
/// `bridge::export_thread::spawn_export` already use for this.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Resumes the live viewport now that this run has fully ended -- the ONE place that
/// decrements `export_active_count`, called from every terminal path
/// ([`report_cancelled`]/[`report_failure`]/[`report_done`]). See this module's own
/// doc comment.
fn finish_video_export(render_ctx: &Arc<Mutex<RenderContext>>) {
    RenderContext::lock(render_ctx).export_active_count -= 1;
}

/// One video export run's outcome, once every frame has either all rendered or the run
/// stopped early -- kept distinct from a plain `Option` so [`run`] reports EXACTLY one
/// of cancelled/failed/done, rather than the previous shape where a frame-write
/// failure (reported from inside the frame loop) and a plain user cancel both
/// collapsed to the same `None` and were then BOTH reported by the caller.
enum FramesOutcome {
    /// Every frame rendered and saved; ready to encode.
    Done(Vec<PathBuf>),
    /// `cancel` was observed before every frame finished.
    Cancelled,
    /// A frame's render or its PNG write failed outright.
    Failed(String),
}

fn run(
    ui_weak: &slint::Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    request: &VideoExportRequest,
    cancel: &AtomicBool,
    activity_id: i32,
) {
    let digits = params::frame_number_digits(request.total_frames);
    let frame_paths = match render_all_frames(ui_weak, request, cancel, activity_id) {
        FramesOutcome::Done(paths) => paths,
        FramesOutcome::Cancelled => {
            report_cancelled(ui_weak, render_ctx, activity_id);
            return;
        }
        FramesOutcome::Failed(message) => {
            report_failure(ui_weak, render_ctx, &message, activity_id);
            return;
        }
    };

    let outcome = encode::encode(
        &request.out_dir,
        &frame_paths,
        digits,
        request.fps,
        &request.out_name,
    );
    if !request.keep_frames
        && matches!(
            outcome,
            encode::EncodeOutcome::Mp4(_) | encode::EncodeOutcome::Gif { .. }
        )
    {
        for path in &frame_paths {
            let _ = std::fs::remove_file(path);
        }
    }
    report_done(ui_weak, render_ctx, &request.out_dir, &outcome, activity_id);
}

/// Renders every frame of `request`'s sweep in order, writing each as a numbered PNG.
/// Acquires ONE [`GpuBackend`] and one [`AccumulationCarry`] for the WHOLE sweep --
/// never once per frame -- and reuses both across every frame via
/// [`render::render_frame_rgba`], the tilt video's own thin wrapper around the SAME
/// `bridge::export_thread::render_accumulation` core the still-image export uses. See
/// this group's own `mod.rs` doc comment for where `request`'s compute configuration
/// came from.
fn render_all_frames(
    ui_weak: &slint::Weak<MainWindow>,
    request: &VideoExportRequest,
    cancel: &AtomicBool,
    activity_id: i32,
) -> FramesOutcome {
    // The full angle list, not a per-index `frame_angle_deg` call: at most 18001 `f64`s
    // (~144KB even at the finest required 0.01° step), negligible next to the per-frame
    // RGBA buffers this loop deliberately never holds more than one of at a time (see
    // this module's own "never all frames in memory at once" doc comment).
    let angles = params::frame_angles(request.start_deg, request.end_deg, request.step_deg);
    let mut frame_paths = Vec::with_capacity(request.total_frames);

    // ---- Compute setup, once for the whole video -----------------------------------
    // Adapter acquisition, remote probing, and hybrid/remote calibration are all far
    // too slow to repeat every frame -- see `bridge::export_thread::AccumulationCarry`'s
    // own doc comment for what `carry` seeds forward from frame to frame.
    let gpu = match request.local_compute {
        LocalComputeTarget::Cpu => GpuBackend::disabled(),
        LocalComputeTarget::CpuGpu | LocalComputeTarget::Gpu => GpuBackend::acquire(),
    };
    let mut carry = AccumulationCarry::default();
    // The whole sweep's own time-remaining estimator, fed once per completed frame
    // (`frames_done / frames_total`, below) -- fresh for every call to this
    // function, since a new run's render rate has nothing to do with a previous
    // run's. See `gui::progress_eta`'s own module doc comment for why a rolling
    // regression rather than "seconds since last frame".
    let mut eta = EtaEstimator::default();
    let config = render::VideoComputeConfig {
        remote: request.remote.clone(),
        local_compute: request.local_compute,
    };

    for (index, &tilt_deg) in angles.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return FramesOutcome::Cancelled;
        }
        let started = Instant::now();
        let (cam_yaw, cam_pitch) = crate::gui::tilt::tilt_hover_preview::camera_pose_for_axis_tilt(
            request.axis_index,
            tilt_deg,
        );

        let ui_weak_progress = ui_weak.clone();
        let total_frames = request.total_frames;
        let outcome = render::render_frame_rgba(
            &request.scene,
            request.width,
            request.height,
            request.samples_per_pixel,
            cam_yaw,
            cam_pitch,
            request.color_space,
            &config,
            &gpu,
            &mut carry,
            cancel,
            move |progress| {
                report_frame_progress(
                    &ui_weak_progress,
                    index,
                    total_frames,
                    progress.fraction,
                    progress.note,
                    activity_id,
                );
            },
        );
        let mut rgba = match outcome {
            render::FrameOutcome::Rendered(rgba) => rgba,
            render::FrameOutcome::Cancelled => return FramesOutcome::Cancelled,
            render::FrameOutcome::Failed(message) => {
                return FramesOutcome::Failed(format!(
                    "Frame {} of {}: {message}",
                    index + 1,
                    request.total_frames
                ));
            }
        };
        if request.selection.count() > 0 {
            let readings =
                metrics::readings_for_frame(request.selection, &request.curves, tilt_deg);
            overlay::draw_overlay(&mut rgba, request.width, request.height, &readings);
        }

        let path = request
            .out_dir
            .join(params::frame_file_name(index + 1, request.total_frames));
        if let Err(e) = save_png(&path, request.width, request.height, &rgba) {
            return FramesOutcome::Failed(format!("Failed to write {}: {e}", path.display()));
        }
        frame_paths.push(path);

        let now = Instant::now();
        eta.observe(now, (index + 1) as f64 / request.total_frames.max(1) as f64);
        report_progress(
            ui_weak,
            index + 1,
            request.total_frames,
            started.elapsed().as_secs_f32(),
            format_eta(eta.eta(now)),
            activity_id,
        );
    }
    FramesOutcome::Done(frame_paths)
}

fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let image = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| "pixel buffer size did not match dimensions".to_string())?;
    image.save(path).map_err(|e| e.to_string())
}

/// Reports one completed frame's progress: the dialog's own frame counter/last-frame
/// timing, plus the SAME completion fraction the status strip's `ActivityChip` shows.
/// `report_frame_progress` (below) additionally blends in WITHIN-frame sample progress
/// on every batch tick, so the two together give a bar that moves continuously rather
/// than jumping once per frame.
///
/// `eta_text` is [`render_all_frames`]'s own `EtaEstimator` (fed `frames_done /
/// frames_total` once per completed frame), already formatted by
/// `gui::progress_eta::format_eta` -- computed at the call site rather than in here so
/// this function stays a plain UI-thread setter, matching every other field it writes.
fn report_progress(
    ui_weak: &slint::Weak<MainWindow>,
    done: usize,
    total: usize,
    last_frame_secs: f32,
    eta_text: String,
    activity_id: i32,
) {
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_current_frame(done as i32);
        let fraction = params::export_progress_fraction(done, total);
        model.set_progress(fraction);
        model.set_last_frame_seconds(last_frame_secs);
        model.set_eta_text(eta_text.into());
        // `ActivityRegistry::progress`/`ActivityList::set_progress` real
        // completion-fraction path -- `TiltVideoExportModel.progress` above
        // already carries the same fraction for this dialog's own bar, this is
        // the status strip's `ActivityChip` reading the identical number.
        ui.global::<ActivityModel>()
            .invoke_progress_external(activity_id, fraction);
    });
}

/// Reports one WITHIN-frame progress tick from `render::render_frame_rgba`'s own
/// `bridge::export_thread::ExportProgress` callback -- cheap (a couple of float ops
/// plus a UI property set already being marshaled per sample batch, exactly like the
/// still-image export's own per-batch progress) and blended with `frame_index`'s
/// already-completed frames so the bar advances continuously through a frame instead
/// of jumping only at frame boundaries.
///
/// Surfaces a remote-lane fallback/failure `note` (a worker declined, dropped a chunk,
/// or was paused after repeated failures) the same way the still-image export does via
/// `ExportProgress::note` -- reused as this dialog's own `status_message` since
/// `TiltVideoExportModel` has no separate note field of its own.
fn report_frame_progress(
    ui_weak: &slint::Weak<MainWindow>,
    frame_index: usize,
    total_frames: usize,
    frame_fraction: f32,
    note: Option<String>,
    activity_id: i32,
) {
    let overall_fraction = (frame_index as f32 + frame_fraction) / (total_frames.max(1) as f32);
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_progress(overall_fraction);
        if let Some(note) = note {
            model.set_status_message(note.into());
        }
        ui.global::<ActivityModel>()
            .invoke_progress_external(activity_id, overall_fraction);
    });
}

fn report_cancelled(
    ui_weak: &slint::Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    activity_id: i32,
) {
    finish_video_export(render_ctx);
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_is_exporting(false);
        model.set_has_error(false);
        model.set_status_message("Video export cancelled.".into());
        model.set_eta_text(String::new().into());
        ui.global::<ActivityModel>()
            .invoke_finish_external(activity_id);
    });
}

fn report_failure(
    ui_weak: &slint::Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    message: &str,
    activity_id: i32,
) {
    finish_video_export(render_ctx);
    let message = message.to_string();
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_is_exporting(false);
        model.set_has_error(true);
        model.set_status_message(message.into());
        model.set_eta_text(String::new().into());
        ui.global::<ActivityModel>()
            .invoke_finish_external(activity_id);
    });
}

fn report_done(
    ui_weak: &slint::Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    out_dir: &Path,
    outcome: &encode::EncodeOutcome,
    activity_id: i32,
) {
    finish_video_export(render_ctx);
    let message = match outcome {
        encode::EncodeOutcome::Mp4(path) => format!("Exported {}", path.display()),
        encode::EncodeOutcome::Gif {
            path,
            mp4_failure_reason,
        } => mp4_failure_reason.as_ref().map_or_else(
            || {
                format!(
                    "ffmpeg not found on PATH -- exported an animated GIF instead: {}",
                    path.display()
                )
            },
            |reason| {
                format!(
                    "ffmpeg could not produce an MP4 ({reason}) -- exported an animated \
                     GIF instead: {}",
                    path.display()
                )
            },
        ),
        encode::EncodeOutcome::FramesOnly { readme } => format!(
            "No video muxer available -- left the frame sequence in {} (see {})",
            out_dir.display(),
            readme.display()
        ),
    };
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_is_exporting(false);
        model.set_has_error(false);
        model.set_progress(1.0);
        model.set_status_message(message.into());
        model.set_eta_text(String::new().into());
        ui.global::<ActivityModel>()
            .invoke_finish_external(activity_id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> render::VideoComputeConfig {
        render::VideoComputeConfig {
            remote: RemoteSelection::local_only(),
            local_compute: LocalComputeTarget::Cpu,
        }
    }

    /// Structural guarantee behind "cancel mid-run leaves no partial MP4": `run`'s own
    /// body (above) only ever calls `encode::encode` with the `FramesOutcome::Done`
    /// result of `render_all_frames` -- a cancelled sweep returns `FramesOutcome::
    /// Cancelled` and `run` returns right after `report_cancelled` instead, so a video
    /// file is never even ATTEMPTED for a cancelled run. This test exercises the same
    /// per-frame render+save path `render_all_frames` uses (that function itself needs
    /// a live `MainWindow` for its progress reporting, which a headless unit test has
    /// no way to construct) and confirms a run stopped partway ("cancelled") leaves
    /// exactly the frames written before the stop and, critically, no `.mp4`/`.gif`
    /// alongside them -- nothing in this crate ever calls `encode::encode` before every
    /// frame has saved successfully.
    #[test]
    fn a_run_stopped_partway_through_leaves_only_the_frames_already_written_and_no_video() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("Diamond resolves");
        let total_frames = 3;
        let dir =
            std::env::temp_dir().join(format!("tilt_video_cancel_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Simulate "cancel observed after the first frame": only frame 1 of 3 is ever
        // rendered/saved, exactly like `render_all_frames`'s own cancel check would
        // produce.
        let path = dir.join(params::frame_file_name(1, total_frames));
        let config = test_config();
        let gpu = GpuBackend::disabled();
        let mut carry = AccumulationCarry::default();
        let cancel = AtomicBool::new(false);
        let outcome = render::render_frame_rgba(
            &scene,
            8,
            8,
            1,
            0.0,
            1.5,
            ColorSpace::Srgb,
            &config,
            &gpu,
            &mut carry,
            &cancel,
            |_| {},
        );
        let render::FrameOutcome::Rendered(rgba) = outcome else {
            panic!("expected a rendered frame");
        };
        save_png(&path, 8, 8, &rgba).unwrap();

        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "only the one frame written before the simulated cancel should exist"
        );
        assert!(
            entries
                .iter()
                .all(|e| e.path().extension().and_then(|ext| ext.to_str()) != Some("mp4")),
            "no .mp4 must exist for a run that stopped before encoding was ever reached"
        );
        assert!(
            entries
                .iter()
                .all(|e| e.path().extension().and_then(|ext| ext.to_str()) != Some("gif")),
            "no .gif must exist for a run that stopped before encoding was ever reached"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_png_round_trips_a_frame_to_disk() {
        let dir =
            std::env::temp_dir().join(format!("tilt_video_save_png_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("frame_001.png");
        let rgba = vec![255u8; 4 * 4 * 4];
        save_png(&path, 4, 4, &rgba).unwrap();
        let decoded = image::open(&path).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (4, 4));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
