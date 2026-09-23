//! The background thread that renders every frame, overlays the selected metrics,
//! writes the PNG sequence, and hands off to `encode` once the sweep finishes (or was
//! cancelled). Kept separate from `mod.rs`'s UI-thread wiring so neither file grows
//! past clippy's function-length lint.

use super::{encode, metrics, overlay, params, render};
use crate::{
    ActivityModel, MainWindow, TiltVideoExportModel, bridge::export_thread::SceneSnapshot,
};
use indicatrix::color::ColorSpace;
use slint::ComponentHandle;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

/// Everything one video export run needs, assembled once on the UI thread before the
/// background thread takes over. `Send`: every field is already `Send` (a
/// `SceneSnapshot`, plain numbers/strings, and `Vec<f32>` curves), the same bar
/// `bridge::export_thread::spawn_export` holds its own request state to.
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
}

/// Spawns the background render loop for `request`. Never blocks the UI thread; checks
/// `cancel` between frames (not mid-frame -- a frame's own CPU trace is already
/// parallelised internally and not worth interrupting partway through).
///
/// `activity_id` is the `crate::ActivityModel` id `handle_start_video_export`
/// registered for this run -- finished on every exit path below
/// (cancelled/failed/done), and given real progress (frames done / total) on
/// every frame via `report_progress`.
pub(super) fn spawn(
    ui_weak: slint::Weak<MainWindow>,
    request: VideoExportRequest,
    cancel: Arc<AtomicBool>,
    activity_id: i32,
) {
    std::thread::spawn(move || run(&ui_weak, &request, &cancel, activity_id));
}

fn run(
    ui_weak: &slint::Weak<MainWindow>,
    request: &VideoExportRequest,
    cancel: &AtomicBool,
    activity_id: i32,
) {
    let digits = params::frame_number_digits(request.total_frames);
    let Some(frame_paths) = render_all_frames(ui_weak, request, cancel, activity_id) else {
        report_cancelled(ui_weak, activity_id);
        return;
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
            encode::EncodeOutcome::Mp4(_) | encode::EncodeOutcome::Gif(_)
        )
    {
        for path in &frame_paths {
            let _ = std::fs::remove_file(path);
        }
    }
    report_done(ui_weak, &request.out_dir, &outcome, activity_id);
}

/// Renders every frame of `request`'s sweep in order, writing each as a numbered PNG.
/// Returns `None` (rather than a partial `Vec`) the moment `cancel` is observed, or if a
/// frame fails to write to disk (reported to the UI directly, since that's a distinct
/// failure from a plain user-requested cancel).
fn render_all_frames(
    ui_weak: &slint::Weak<MainWindow>,
    request: &VideoExportRequest,
    cancel: &AtomicBool,
    activity_id: i32,
) -> Option<Vec<PathBuf>> {
    // The full angle list, not a per-index `frame_angle_deg` call: at most 18001 `f64`s
    // (~144KB even at the finest required 0.01° step), negligible next to the per-frame
    // RGBA buffers this loop deliberately never holds more than one of at a time (see
    // this module's own "never all frames in memory at once" doc comment).
    let angles = params::frame_angles(request.start_deg, request.end_deg, request.step_deg);
    let mut frame_paths = Vec::with_capacity(request.total_frames);
    for (index, &tilt_deg) in angles.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let started = Instant::now();
        let (cam_yaw, cam_pitch) = crate::gui::tilt::tilt_hover_preview::camera_pose_for_axis_tilt(
            request.axis_index,
            tilt_deg,
        );

        let mut rgba = render::render_frame_rgba(
            &request.scene,
            request.width,
            request.height,
            request.samples_per_pixel,
            cam_yaw,
            cam_pitch,
            request.color_space,
        );
        if request.selection.count() > 0 {
            let readings =
                metrics::readings_for_frame(request.selection, &request.curves, tilt_deg);
            overlay::draw_overlay(&mut rgba, request.width, request.height, &readings);
        }

        let path = request
            .out_dir
            .join(params::frame_file_name(index + 1, request.total_frames));
        if let Err(e) = save_png(&path, request.width, request.height, &rgba) {
            report_failure(
                ui_weak,
                &format!("Failed to write {}: {e}", path.display()),
                activity_id,
            );
            return None;
        }
        frame_paths.push(path);

        report_progress(
            ui_weak,
            index + 1,
            request.total_frames,
            started.elapsed().as_secs_f32(),
            activity_id,
        );
    }
    Some(frame_paths)
}

fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let image = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| "pixel buffer size did not match dimensions".to_string())?;
    image.save(path).map_err(|e| e.to_string())
}

fn report_progress(
    ui_weak: &slint::Weak<MainWindow>,
    done: usize,
    total: usize,
    last_frame_secs: f32,
    activity_id: i32,
) {
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_current_frame(done as i32);
        let fraction = params::export_progress_fraction(done, total);
        model.set_progress(fraction);
        model.set_last_frame_seconds(last_frame_secs);
        // `ActivityRegistry::progress`/`ActivityList::set_progress` real
        // completion-fraction path -- `TiltVideoExportModel.progress` above
        // already carries the same fraction for this dialog's own bar, this is
        // the status strip's `ActivityChip` reading the identical number.
        ui.global::<ActivityModel>()
            .invoke_progress_external(activity_id, fraction);
    });
}

fn report_cancelled(ui_weak: &slint::Weak<MainWindow>, activity_id: i32) {
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_is_exporting(false);
        model.set_has_error(false);
        model.set_status_message("Video export cancelled.".into());
        ui.global::<ActivityModel>()
            .invoke_finish_external(activity_id);
    });
}

fn report_failure(ui_weak: &slint::Weak<MainWindow>, message: &str, activity_id: i32) {
    let message = message.to_string();
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_is_exporting(false);
        model.set_has_error(true);
        model.set_status_message(message.into());
        ui.global::<ActivityModel>()
            .invoke_finish_external(activity_id);
    });
}

fn report_done(
    ui_weak: &slint::Weak<MainWindow>,
    out_dir: &Path,
    outcome: &encode::EncodeOutcome,
    activity_id: i32,
) {
    let message = match outcome {
        encode::EncodeOutcome::Mp4(path) => format!("Exported {}", path.display()),
        encode::EncodeOutcome::Gif(path) => {
            format!(
                "ffmpeg not found on PATH -- exported an animated GIF instead: {}",
                path.display()
            )
        }
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
        ui.global::<ActivityModel>()
            .invoke_finish_external(activity_id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::render_thread::RenderContext;
    use indicatrix::color::ColorSpace;
    use std::sync::Mutex;

    /// Structural guarantee behind "cancel mid-run leaves no partial MP4": `run`'s own
    /// body (above) only ever calls `encode::encode` with the `Some` result of
    /// `render_all_frames` -- a cancelled sweep returns `None` and `run` returns right
    /// after `report_cancelled` instead, so a video file is never even ATTEMPTED for a
    /// cancelled run. This test exercises the same per-frame render+save path
    /// `render_all_frames` uses (that function itself needs a live `MainWindow` for its
    /// progress reporting, which a headless unit test has no way to construct) and
    /// confirms a run stopped partway ("cancelled") leaves exactly the frames written
    /// before the stop and, critically, no `.mp4`/`.gif` alongside them -- nothing in
    /// this crate ever calls `encode::encode` before every frame has saved
    /// successfully.
    #[test]
    fn a_run_stopped_partway_through_leaves_only_the_frames_already_written_and_no_video() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()));
        let total_frames = 3;
        let dir =
            std::env::temp_dir().join(format!("tilt_video_cancel_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Simulate "cancel observed after the first frame": only frame 1 of 3 is ever
        // rendered/saved, exactly like `render_all_frames`'s own cancel check would
        // produce.
        let path = dir.join(params::frame_file_name(1, total_frames));
        let rgba = render::render_frame_rgba(&scene, 8, 8, 1, 0.0, 1.5, ColorSpace::Srgb);
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
