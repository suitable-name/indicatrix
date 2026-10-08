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
//!
//! A `RemoteOnly` video claims no pause at all ([`pauses_viewport`]): it acquires no GPU
//! adapter and runs no local lane, so the live viewport stays usable for the whole video.

use super::{
    encode,
    frames::{self, FrameReporter, FramesOutcome},
    metrics, params,
};
use crate::{
    ActivityModel, MainWindow, TiltModel, TiltVideoExportModel,
    bridge::{
        export_thread::{RemoteSelection, SceneSnapshot},
        render_thread::RenderContext,
    },
    gui::progress_eta::{EtaEstimator, format_eta},
    settings::LocalComputeTarget,
};
use indicatrix::color::ColorSpace;
use slint::ComponentHandle;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::Instant,
};

/// Everything one video export run needs, assembled once on the UI thread before the
/// background thread takes over. `Send`: every field is already `Send` (a
/// `SceneSnapshot`, plain numbers/strings, `Vec<f32>` curves, and a `RemoteSelection`),
/// the same bar `bridge::export_thread::spawn_export` holds its own request state to.
///
/// Public inside this group: a render job builds the same request (`gui::render_jobs::convert`) and
/// runs it through [`frames::render_frames`].
pub struct VideoExportRequest {
    pub scene: SceneSnapshot,
    pub axis_index: usize,
    pub start_deg: f64,
    pub end_deg: f64,
    pub step_deg: f64,
    pub total_frames: usize,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub samples_per_pixel: u32,
    pub color_space: ColorSpace,
    pub out_dir: PathBuf,
    pub out_name: String,
    pub selection: metrics::MetricSelection,
    pub curves: metrics::MetricCurves,
    pub keep_frames: bool,
    /// The remote side: the section's "Compute" choice (`Both` by default; `RemoteOnly`
    /// traces nothing on this computer, see this group's own `mod.rs` doc comment), the
    /// configured remote endpoint (`AppSettings::remote`, read from the SAME settings
    /// store the still-image export reads), and the section's "Transfer" choice.
    pub remote: RemoteSelection,
    /// `RenderContext::local_compute_target`, read from the SAME persisted setting the
    /// still-image export and the live viewport both use.
    pub local_compute: LocalComputeTarget,
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
    let pausing = pauses_viewport(&request);
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
                pausing,
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
///
/// `pausing` is [`pauses_viewport`] for this run: a Remote-only video never incremented
/// the count, so it must not decrement it either.
fn finish_video_export(render_ctx: &Arc<Mutex<RenderContext>>, pausing: bool) {
    if pausing {
        RenderContext::lock(render_ctx).export_active_count -= 1;
    }
}

/// Whether `request`'s run holds the live-viewport pause for its duration -- false only
/// under `RemoteOnly`, which traces nothing on this computer. The caller
/// (`mod.rs::handle_start_video_export`) increments `export_active_count` exactly when
/// this is true, and [`finish_video_export`] decrements exactly when it is.
pub(super) const fn pauses_viewport(request: &VideoExportRequest) -> bool {
    request.remote.compute_target.pauses_live_viewport()
}

fn run(
    ui_weak: &slint::Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    request: &VideoExportRequest,
    cancel: &AtomicBool,
    activity_id: i32,
) {
    let digits = params::frame_number_digits(request.total_frames);
    let pausing = pauses_viewport(request);
    let frame_paths = match render_all_frames(ui_weak, request, cancel, activity_id) {
        FramesOutcome::Done(paths) => paths,
        FramesOutcome::Cancelled { .. } => {
            report_cancelled(ui_weak, render_ctx, pausing, activity_id);
            return;
        }
        FramesOutcome::Failed { message, .. } => {
            report_failure(ui_weak, render_ctx, pausing, &message, activity_id);
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
    report_done(
        ui_weak,
        render_ctx,
        pausing,
        &request.out_dir,
        &outcome,
        activity_id,
    );
}

/// Renders every frame of `request`'s sweep through [`frames::render_frames`] (every
/// frame, never skipping one: the direct export always starts a fresh folder), reporting
/// into the dialog and the status strip through a [`SlintFrameReporter`]. See
/// [`frames::render_frames`] for the compute setup and this group's own `mod.rs` doc
/// comment for where `request`'s compute configuration came from.
fn render_all_frames(
    ui_weak: &slint::Weak<MainWindow>,
    request: &VideoExportRequest,
    cancel: &AtomicBool,
    activity_id: i32,
) -> FramesOutcome {
    // The whole sweep's own time-remaining estimator, fed once per completed frame
    // (`frames_done / frames_total`) -- fresh for every call to this function, since a
    // new run's render rate has nothing to do with a previous run's. See
    // `gui::progress_eta`'s own module doc comment for why a rolling regression rather
    // than "seconds since last frame".
    let mut reporter = SlintFrameReporter {
        ui_weak: ui_weak.clone(),
        activity_id,
        eta: EtaEstimator::default(),
    };
    frames::render_frames(request, false, cancel, &mut reporter)
}

/// Writes the frame loop's progress into `TiltVideoExportModel` and the status strip's
/// `ActivityChip` -- exactly the two reports the loop made before it moved to `frames`.
struct SlintFrameReporter {
    ui_weak: slint::Weak<MainWindow>,
    activity_id: i32,
    eta: EtaEstimator,
}

impl FrameReporter for SlintFrameReporter {
    fn frame_progress(&mut self, index: usize, total: usize, fraction: f32, note: Option<String>) {
        report_frame_progress(
            &self.ui_weak,
            index,
            total,
            fraction,
            note,
            self.activity_id,
        );
    }

    fn frame_saved(&mut self, _index: usize, frames_done: usize, total: usize, frame_secs: f32) {
        let now = Instant::now();
        self.eta
            .observe(now, frames_done as f64 / total.max(1) as f64);
        report_progress(
            &self.ui_weak,
            frames_done,
            total,
            frame_secs,
            format_eta(self.eta.eta(now)),
            self.activity_id,
        );
    }
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
    pausing: bool,
    activity_id: i32,
) {
    finish_video_export(render_ctx, pausing);
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_is_exporting(false);
        model.set_has_error(false);
        model.set_status_message("Video export cancelled.".into());
        model.set_eta_text(String::new().into());
        // Run in the background, nothing on screen would say so.
        if !ui.global::<TiltModel>().get_dialog_open() {
            crate::gui::show_toast(&ui, "Video export cancelled.", "info");
        }
        ui.global::<ActivityModel>()
            .invoke_finish_external(activity_id);
    });
}

fn report_failure(
    ui_weak: &slint::Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    pausing: bool,
    message: &str,
    activity_id: i32,
) {
    finish_video_export(render_ctx, pausing);
    let message = message.to_string();
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_is_exporting(false);
        model.set_has_error(true);
        // A toast too: the popup may be closed (run in the background), and an error stays
        // on screen until dismissed.
        crate::gui::show_toast(&ui, &message, "error");
        model.set_status_message(message.into());
        model.set_eta_text(String::new().into());
        ui.global::<ActivityModel>()
            .invoke_finish_external(activity_id);
    });
}

fn report_done(
    ui_weak: &slint::Weak<MainWindow>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    pausing: bool,
    out_dir: &Path,
    outcome: &encode::EncodeOutcome,
    activity_id: i32,
) {
    finish_video_export(render_ctx, pausing);
    let (_, message) = frames::encode_outcome_message(outcome, out_dir);
    let toast_kind = if matches!(outcome, encode::EncodeOutcome::Mp4(_)) {
        "success"
    } else {
        "info"
    };
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<TiltVideoExportModel>();
        model.set_is_exporting(false);
        model.set_has_error(false);
        model.set_progress(1.0);
        // A toast too: the popup may be closed (run in the background).
        crate::gui::show_toast(&ui, &message, toast_kind);
        model.set_status_message(message.into());
        model.set_eta_text(String::new().into());
        ui.global::<ActivityModel>()
            .invoke_finish_external(activity_id);
    });
}

#[cfg(test)]
mod tests {
    use super::{super::render, *};
    use crate::bridge::export_thread::AccumulationCarry;
    use indicatrix::renderer::gpu_backend::GpuBackend;

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
        frames::save_png_atomic(&path, 8, 8, &rgba).unwrap();

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
        frames::save_png_atomic(&path, 4, 4, &rgba).unwrap();
        let decoded = image::open(&path).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (4, 4));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
