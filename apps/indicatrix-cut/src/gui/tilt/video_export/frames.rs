//! The tilt video's frame loop, free of any window: renders every frame of a sweep in
//! order, overlays the selected metrics and writes a numbered PNG sequence.
//!
//! Two callers share it. The direct export (`run::spawn`) reports through a Slint adapter
//! and renders every frame. A render job (`gui::render_jobs::execute`) reports to a job
//! sink and passes `skip_existing`, so a paused or interrupted video continues at its
//! first missing frame instead of starting over.
//!
//! Every frame is written atomically: the PNG goes to `frame_NNN.png.partial` first and is
//! renamed once complete, so a frame that exists on disk is always a whole picture. That
//! is what lets `skip_existing` trust the folder after a crash.

use super::{metrics, overlay, params, render, run::VideoExportRequest};
use crate::{
    bridge::export_thread::{AccumulationCarry, ComputeTarget},
    gui::tilt::tilt_hover_preview,
    settings::LocalComputeTarget,
};
use indicatrix::renderer::gpu_backend::GpuBackend;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

/// The suffix of a frame that is still being written.
const PARTIAL_SUFFIX: &str = ".partial";

/// Receives the frame loop's progress. Called from the thread that renders.
pub trait FrameReporter {
    /// Within-frame progress: `fraction` (0..=1) of frame `index` (0-based) of `total`,
    /// and an optional note about the remote worker.
    fn frame_progress(&mut self, index: usize, total: usize, fraction: f32, note: Option<String>);

    /// Frame `index` (0-based) is on disk. `frames_done` counts every frame now on disk,
    /// including the ones that were already there.
    fn frame_saved(&mut self, index: usize, frames_done: usize, total: usize, frame_secs: f32);
}

/// How a run of the frame loop ended.
pub enum FramesOutcome {
    /// Every frame is on disk. The paths are in frame order.
    Done(Vec<PathBuf>),
    /// `cancel` was observed before every frame was on disk.
    Cancelled {
        /// Frames on disk when it stopped.
        frames_done: usize,
    },
    /// A frame's render or its PNG write failed outright.
    Failed {
        /// One plain sentence naming the frame and the problem.
        message: String,
        /// Frames on disk when it failed.
        frames_done: usize,
    },
}

/// The path of frame `index_zero_based` of `total` inside `dir`.
pub fn frame_path(dir: &Path, index_zero_based: usize, total: usize) -> PathBuf {
    dir.join(params::frame_file_name(index_zero_based + 1, total))
}

/// For each frame of `total`, whether its PNG already exists in `dir`.
pub fn existing_frames(dir: &Path, total: usize) -> Vec<bool> {
    (0..total)
        .map(|index| frame_path(dir, index, total).is_file())
        .collect()
}

/// Whether `dir` holds any `frame_*.png` file, whatever its numbering.
pub fn holds_frames(dir: &Path) -> bool {
    file_names(dir).any(|name| is_frame_name(&name))
}

/// Deletes every `frame_*.png` in `dir`, and every stray partial file. Returns how many
/// frames were deleted.
///
/// Every frame, not only the numbers of the current sweep: a frame left over from a
/// longer sweep would otherwise run on into the video, because the video muxer reads
/// consecutive numbers for as long as the files exist.
///
/// # Errors
///
/// The first file that could not be deleted.
pub fn clear_frames(dir: &Path) -> std::io::Result<usize> {
    let mut deleted = 0;
    for name in file_names(dir) {
        if is_frame_name(&name) {
            std::fs::remove_file(dir.join(&name))?;
            deleted += 1;
        } else if name.ends_with(PARTIAL_SUFFIX) {
            std::fs::remove_file(dir.join(&name))?;
        }
    }
    Ok(deleted)
}

/// Deletes every `*.partial` file in `dir`: the leftovers of a frame that was being
/// written when the run stopped. Best effort.
pub fn remove_partials(dir: &Path) {
    for name in file_names(dir) {
        if name.ends_with(PARTIAL_SUFFIX) {
            let _ = std::fs::remove_file(dir.join(name));
        }
    }
}

fn is_frame_name(name: &str) -> bool {
    name.starts_with("frame_")
        && Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
}

/// The names of the regular files in `dir`; empty when it cannot be read.
fn file_names(dir: &Path) -> impl Iterator<Item = String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
}

/// Writes `rgba` (`width x height`, 8 bits per channel) to `path` as a PNG, through a
/// `.partial` file that is renamed once it is complete.
///
/// # Errors
///
/// The reason the buffer did not match, or the file could not be written or renamed.
pub fn save_png_atomic(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let image = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| "pixel buffer size did not match dimensions".to_string())?;
    let mut partial_name = path.file_name().unwrap_or_default().to_os_string();
    partial_name.push(PARTIAL_SUFFIX);
    let partial = path.with_file_name(partial_name);
    let written = image
        .save_with_format(&partial, image::ImageFormat::Png)
        .map_err(|e| e.to_string())
        .and_then(|()| std::fs::rename(&partial, path).map_err(|e| e.to_string()));
    if written.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    written
}

/// The path to report and the sentence to show once the frames have been encoded.
///
/// The path is the video or GIF written, or `out_dir` when only the frames are left. The
/// sentence is the one the direct export has always shown.
pub fn encode_outcome_message(
    outcome: &super::encode::EncodeOutcome,
    out_dir: &Path,
) -> (PathBuf, String) {
    use super::encode::EncodeOutcome;
    match outcome {
        EncodeOutcome::Mp4(path) => (path.clone(), format!("Exported {}", path.display())),
        EncodeOutcome::Gif {
            path,
            mp4_failure_reason,
        } => {
            let message = mp4_failure_reason.as_ref().map_or_else(
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
            );
            (path.clone(), message)
        }
        EncodeOutcome::FramesOnly { readme } => (
            out_dir.to_path_buf(),
            format!(
                "No video muxer available -- left the frame sequence in {} (see {})",
                out_dir.display(),
                readme.display()
            ),
        ),
    }
}

/// The local engines the frame loop sets up for `compute_target`: the app's own choice,
/// except under `RemoteOnly`, where the owner chose to keep this computer free and no local
/// GPU context or hybrid probe may exist.
const fn frame_local_compute(
    compute_target: ComputeTarget,
    local_compute: LocalComputeTarget,
) -> LocalComputeTarget {
    match compute_target {
        ComputeTarget::RemoteOnly => LocalComputeTarget::Cpu,
        ComputeTarget::LocalOnly | ComputeTarget::Both => local_compute,
    }
}

/// The legend file of a diagnostic video, one for the whole folder.
const LEGEND_FILE: &str = "optical.txt";

/// Said when frames already on disk were rendered under other diagnostic settings.
const LEGEND_MISMATCH: &str = "The frames already in this folder were rendered with different Optical Diagnostics settings. Restart the video to render every frame again.";

/// Checks the diagnostic choice before any frame renders, and writes the folder's one legend
/// file ([`LEGEND_FILE`]). The legend doubles as the resume key: with `frames_done` frames
/// already on disk it must equal the legend those frames were rendered under, or the video
/// would mix pictures with and without (or with other) diagnostics. A fresh run (no frames on
/// disk) with no diagnostics removes a legend left by an earlier run.
fn prepare_legend(request: &VideoExportRequest, frames_done: usize) -> Result<(), String> {
    use crate::bridge::optical_overlay::export::{refusal, sweep_legend};
    if let Some(reason) = refusal(&request.scene) {
        return Err(reason.to_string());
    }
    let path = request.out_dir.join(LEGEND_FILE);
    let sweep = format!(
        "Sweep axis {}; start {}; end {}; step {} degrees; every frame is traced at its exact signed pose.",
        request.axis_index, request.start_deg, request.end_deg, request.step_deg
    );
    let Some(legend) = sweep_legend(&request.scene, request.width, request.height, &sweep) else {
        if frames_done == 0 {
            let _ = std::fs::remove_file(&path);
        } else if path.is_file() {
            return Err(LEGEND_MISMATCH.to_string());
        }
        return Ok(());
    };
    if frames_done > 0 && std::fs::read_to_string(&path).ok().as_deref() != Some(legend.as_str()) {
        return Err(LEGEND_MISMATCH.to_string());
    }
    std::fs::write(&path, legend).map_err(|e| format!("Diagnostic legend: {e}"))
}

/// The row `up` pixels above the bottom edge of a picture `height` tall.
fn row_above_bottom(height: u32, up: u32) -> i32 {
    i32::try_from(height.saturating_sub(up)).unwrap_or(i32::MAX)
}

/// The caption strip at the bottom of a frame that carries a diagnostic overlay.
fn draw_diagnostic_caption(rgba: &mut [u8], width: u32, height: u32) {
    overlay::fill_rect_alpha(
        rgba,
        width,
        height,
        0,
        row_above_bottom(height, 20),
        width,
        20,
        [0, 0, 0],
        210,
    );
    let caption = if width >= 320 {
        "OPTICAL DIAGNOSTICS V1 - GEOMETRIC ESTIMATE"
    } else {
        "OPTICAL V1"
    };
    overlay::draw_text(
        rgba,
        width,
        height,
        4,
        row_above_bottom(height, 17),
        caption,
        1,
        [255, 255, 255],
    );
    if width >= 320 {
        overlay::draw_text(
            rgba,
            width,
            height,
            4,
            row_above_bottom(height, 9),
            "FIRE = DISPERSION POTENTIAL",
            1,
            [255, 255, 255],
        );
    }
}

/// The GPU backend a sweep traces with: acquired only when the local compute target uses
/// the GPU and some frame still has to be rendered.
fn acquire_gpu(local_compute: LocalComputeTarget, needs_render: bool) -> GpuBackend {
    match local_compute {
        LocalComputeTarget::CpuGpu | LocalComputeTarget::Gpu if needs_render => {
            GpuBackend::acquire()
        }
        _ => GpuBackend::disabled(),
    }
}

/// Draws the optical diagnostics (and their caption) over frame `(index, total)`'s `rgba`,
/// when the scene asks for them. They fill the second half of the frame's progress.
fn overlay_diagnostics(
    request: &VideoExportRequest,
    pose: [f32; 2],
    (index, total): (usize, usize),
    rgba: &mut [u8],
    cancel: &AtomicBool,
    reporter: &mut dyn FrameReporter,
) -> Result<(), String> {
    let mut report_diagnostics = |fraction: f32| {
        reporter.frame_progress(index, total, fraction.mul_add(0.5, 0.5), None);
    };
    let applied = crate::bridge::optical_overlay::export::apply(
        &request.scene,
        pose,
        crate::bridge::optical_overlay::export::Picture {
            width: request.width,
            height: request.height,
            color_space: request.color_space,
            rgba: &mut *rgba,
        },
        cancel,
        &mut report_diagnostics,
    )?;
    if applied.is_some() {
        draw_diagnostic_caption(rgba, request.width, request.height);
    }
    Ok(())
}

/// How a sweep ends when the diagnostic pass of a frame stops with `message`: cancelled
/// when `cancel` is raised (the pass reports a stopped job as an error), failed otherwise.
fn diagnostics_stopped(message: String, cancel: &AtomicBool, frames_done: usize) -> FramesOutcome {
    if cancel.load(Ordering::Relaxed) {
        FramesOutcome::Cancelled { frames_done }
    } else {
        FramesOutcome::Failed {
            message,
            frames_done,
        }
    }
}

/// Renders every frame of `request`'s sweep in order, writing each as a numbered PNG in
/// `request.out_dir` (which must exist).
///
/// Acquires ONE [`GpuBackend`] and one [`AccumulationCarry`] for the WHOLE sweep, never
/// once per frame, and reuses both across every frame via [`render::render_frame_rgba`],
/// the tilt video's thin wrapper around the SAME
/// `bridge::export_thread::render_accumulation` core the still-image export uses. With
/// `skip_existing`, a frame whose PNG is already on disk is not rendered again. `cancel`
/// is checked between frames, not mid-frame: a frame's own trace is already parallelised
/// internally and not worth interrupting partway through.
#[expect(
    clippy::too_many_lines,
    reason = "one frame loop whose setup, cancel checks and progress share local state"
)]
pub fn render_frames(
    request: &VideoExportRequest,
    skip_existing: bool,
    cancel: &AtomicBool,
    reporter: &mut dyn FrameReporter,
) -> FramesOutcome {
    // The full angle list, not a per-index `frame_angle_deg` call: at most 18001 `f64`s
    // (~144KB even at the finest required 0.01° step), negligible next to the per-frame
    // RGBA buffers this loop deliberately never holds more than one of at a time.
    let angles = params::frame_angles(request.start_deg, request.end_deg, request.step_deg);
    let total = request.total_frames;
    let mut frame_paths = Vec::with_capacity(total);
    let existing = if skip_existing {
        existing_frames(&request.out_dir, total)
    } else {
        Vec::new()
    };
    let is_on_disk = |index: usize| existing.get(index).copied().unwrap_or(false);
    let mut frames_done = (0..total).filter(|&index| is_on_disk(index)).count();
    if let Err(message) = prepare_legend(request, frames_done) {
        return FramesOutcome::Failed {
            message,
            frames_done,
        };
    }
    let diagnostics = request.scene.optical_diagnostics.is_some();
    // With diagnostics the beauty render fills the first half of a frame's progress and the
    // diagnostic pass the second half.
    let beauty_share = if diagnostics { 0.5 } else { 1.0 };

    // ---- Compute setup, once for the whole video -----------------------------------
    // Adapter acquisition, remote probing, and hybrid/remote calibration are all far
    // too slow to repeat every frame -- see `AccumulationCarry`'s own doc comment for
    // what `carry` seeds forward from frame to frame. A sweep with nothing left to
    // render acquires nothing.
    let needs_render = (0..angles.len()).any(|index| !is_on_disk(index));
    // `Remote only` traces nothing here: `local_compute` becomes the CPU alone (no hybrid
    // calibration probe, which would trace locally) and no GPU adapter is acquired.
    let local_compute = frame_local_compute(request.remote.compute_target, request.local_compute);
    let gpu = acquire_gpu(
        if request.scene.engine.is_ray() {
            LocalComputeTarget::Cpu
        } else {
            local_compute
        },
        needs_render,
    );
    let mut carry = AccumulationCarry::default();
    let config = render::VideoComputeConfig {
        remote: request.remote.clone(),
        local_compute,
    };

    for (index, &tilt_deg) in angles.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return FramesOutcome::Cancelled { frames_done };
        }
        let path = frame_path(&request.out_dir, index, total);
        if is_on_disk(index) {
            frame_paths.push(path);
            continue;
        }
        let started = Instant::now();
        let (cam_yaw, cam_pitch) =
            tilt_hover_preview::camera_pose_for_axis_tilt(request.axis_index, tilt_deg);

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
            |progress| {
                reporter.frame_progress(
                    index,
                    total,
                    progress.fraction * beauty_share,
                    progress.note,
                );
            },
        );
        let mut rgba = match outcome {
            render::FrameOutcome::Rendered(rgba) => rgba,
            render::FrameOutcome::Cancelled => return FramesOutcome::Cancelled { frames_done },
            render::FrameOutcome::Failed(message) => {
                return FramesOutcome::Failed {
                    message: format!("Frame {} of {total}: {message}", index + 1),
                    frames_done,
                };
            }
        };
        if let Err(message) = overlay_diagnostics(
            request,
            [cam_yaw, cam_pitch],
            (index, total),
            &mut rgba,
            cancel,
            reporter,
        ) {
            return diagnostics_stopped(message, cancel, frames_done);
        }
        if request.selection.count() > 0 {
            let readings =
                metrics::readings_for_frame(request.selection, &request.curves, tilt_deg);
            overlay::draw_overlay(&mut rgba, request.width, request.height, &readings);
        }

        if let Err(e) = save_png_atomic(&path, request.width, request.height, &rgba) {
            return FramesOutcome::Failed {
                message: format!("Failed to write {}: {e}", path.display()),
                frames_done,
            };
        }
        frame_paths.push(path);
        frames_done += 1;
        reporter.frame_saved(index, frames_done, total, started.elapsed().as_secs_f32());
    }
    FramesOutcome::Done(frame_paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{
        export_thread::{RemoteSelection, SceneSnapshot},
        render_thread::RenderContext,
    };
    use indicatrix::color::ColorSpace;
    use std::sync::Mutex;

    /// A fresh, empty folder for one test.
    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tilt_frames_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A three-frame, 8 x 8, one-sample CPU sweep into `dir`.
    fn request(dir: &Path) -> VideoExportRequest {
        VideoExportRequest {
            scene: SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
                .expect("Diamond resolves"),
            axis_index: 0,
            start_deg: -2.0,
            end_deg: 2.0,
            step_deg: 2.0,
            total_frames: 3,
            fps: 30,
            width: 8,
            height: 8,
            samples_per_pixel: 1,
            color_space: ColorSpace::Srgb,
            out_dir: dir.to_path_buf(),
            out_name: "test".to_string(),
            selection: metrics::MetricSelection::default(),
            curves: metrics::MetricCurves {
                brilliance: Vec::new(),
                windowing: Vec::new(),
                extinction: Vec::new(),
            },
            keep_frames: true,
            remote: RemoteSelection::local_only(),
            local_compute: LocalComputeTarget::Cpu,
        }
    }

    #[derive(Default)]
    struct Recorder {
        saved: Vec<(usize, usize)>,
        progress_ticks: usize,
    }

    impl FrameReporter for Recorder {
        fn frame_progress(&mut self, _: usize, _: usize, _: f32, _: Option<String>) {
            self.progress_ticks += 1;
        }

        fn frame_saved(&mut self, index: usize, frames_done: usize, _: usize, _: f32) {
            self.saved.push((index, frames_done));
        }
    }

    #[test]
    fn without_skip_existing_every_frame_is_rendered_and_written() {
        let dir = test_dir("all");
        let mut recorder = Recorder::default();
        let outcome = render_frames(
            &request(&dir),
            false,
            &AtomicBool::new(false),
            &mut recorder,
        );
        let FramesOutcome::Done(paths) = outcome else {
            panic!("expected every frame to render");
        };
        assert_eq!(paths.len(), 3);
        assert!(paths.iter().all(|path| path.is_file()));
        assert_eq!(recorder.saved, vec![(0, 1), (1, 2), (2, 3)]);
        assert!(recorder.progress_ticks > 0);
        assert!(!holds_partials(&dir), "no partial file may be left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skip_existing_renders_only_the_missing_frames_and_leaves_the_others_alone() {
        let dir = test_dir("skip");
        let request = request(&dir);
        let first = frame_path(&dir, 0, 3);
        save_png_atomic(&first, 8, 8, &[7; 8 * 8 * 4]).unwrap();
        let mut recorder = Recorder::default();
        let outcome = render_frames(&request, true, &AtomicBool::new(false), &mut recorder);
        assert!(matches!(outcome, FramesOutcome::Done(ref paths) if paths.len() == 3));
        assert_eq!(recorder.saved, vec![(1, 2), (2, 3)]);
        assert_eq!(
            image::open(&first).unwrap().to_rgba8().as_raw()[0],
            7,
            "the frame that was already there must not be rendered again"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancel_between_frames_reports_the_frames_on_disk() {
        struct CancelAfterFirst<'a>(&'a AtomicBool);
        impl FrameReporter for CancelAfterFirst<'_> {
            fn frame_progress(&mut self, _: usize, _: usize, _: f32, _: Option<String>) {}
            fn frame_saved(&mut self, _: usize, _: usize, _: usize, _: f32) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let dir = test_dir("cancel");
        let cancel = AtomicBool::new(false);
        let outcome = render_frames(
            &request(&dir),
            false,
            &cancel,
            &mut CancelAfterFirst(&cancel),
        );
        assert!(matches!(
            outcome,
            FramesOutcome::Cancelled { frames_done: 1 }
        ));
        assert_eq!(existing_frames(&dir, 3), vec![true, false, false]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The video writes one legend beside its frames, and frames already on disk may only be
    /// kept when that legend is unchanged.
    #[test]
    fn the_folders_legend_is_the_resume_key_for_diagnostic_videos() {
        use indicatrix::renderer::optical_overlay::OpticalPresentation;
        let dir = test_dir("legend");
        let mut request = request(&dir);
        request.scene.optical_diagnostics = Some(OpticalPresentation::default());
        prepare_legend(&request, 0).expect("a fresh run writes the legend");
        let legend = dir.join(LEGEND_FILE);
        assert!(legend.is_file());
        assert!(
            !dir.join("frame_001.optical.txt").exists(),
            "no per-frame legend files"
        );
        prepare_legend(&request, 1).expect("the same settings may resume");

        request.scene.optical_diagnostics = Some(OpticalPresentation {
            opacity: 0.9,
            ..OpticalPresentation::default()
        });
        assert_eq!(
            prepare_legend(&request, 1),
            Err(LEGEND_MISMATCH.to_string())
        );
        prepare_legend(&request, 0).expect("a restart replaces the legend");

        request.scene.optical_diagnostics = None;
        assert_eq!(
            prepare_legend(&request, 1),
            Err(LEGEND_MISMATCH.to_string()),
            "frames with diagnostics cannot continue without them"
        );
        prepare_legend(&request, 0).expect("a clean restart is fine");
        assert!(!legend.exists(), "the old legend is removed");
        prepare_legend(&request, 1).expect("a clean video resumes a clean folder");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remote_only_builds_no_local_compute_and_the_other_choices_keep_the_apps() {
        for local in [
            LocalComputeTarget::Cpu,
            LocalComputeTarget::CpuGpu,
            LocalComputeTarget::Gpu,
        ] {
            assert_eq!(
                frame_local_compute(ComputeTarget::RemoteOnly, local),
                LocalComputeTarget::Cpu,
                "Remote only must not set up a GPU or a hybrid probe"
            );
            assert_eq!(frame_local_compute(ComputeTarget::LocalOnly, local), local);
            assert_eq!(frame_local_compute(ComputeTarget::Both, local), local);
        }
    }

    /// Remote only with no reachable remote fails the first frame with a message and traces
    /// nothing locally: no frame file appears.
    #[test]
    fn remote_only_without_a_remote_fails_instead_of_tracing_locally() {
        let dir = test_dir("remote_only");
        let mut request = request(&dir);
        request.remote = RemoteSelection {
            compute_target: ComputeTarget::RemoteOnly,
            ..RemoteSelection::local_only()
        };
        let mut recorder = Recorder::default();
        let outcome = render_frames(&request, false, &AtomicBool::new(false), &mut recorder);
        let FramesOutcome::Failed {
            message,
            frames_done,
        } = outcome
        else {
            panic!("expected the video to fail without a remote");
        };
        assert!(message.starts_with("Frame 1 of 3: "), "{message}");
        assert_eq!(frames_done, 0);
        assert_eq!(recorder.saved.len(), 0);
        assert!(!holds_frames(&dir), "nothing may be traced locally");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn holds_partials(dir: &Path) -> bool {
        file_names(dir).any(|name| name.ends_with(PARTIAL_SUFFIX))
    }

    #[test]
    fn save_png_atomic_round_trips_a_frame_and_leaves_no_partial_file() {
        let dir = test_dir("atomic");
        let path = dir.join("frame_001.png");
        save_png_atomic(&path, 4, 4, &[255; 4 * 4 * 4]).unwrap();
        let decoded = image::open(&path).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (4, 4));
        assert!(!holds_partials(&dir));
        assert!(save_png_atomic(&path, 4, 4, &[0; 3]).is_err());
        assert!(!holds_partials(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_holds_and_clear_see_frames_by_name() {
        let dir = test_dir("clear");
        assert!(!holds_frames(&dir));
        std::fs::write(frame_path(&dir, 1, 3), b"x").unwrap();
        std::fs::write(dir.join("frame_9.png"), b"x").unwrap();
        std::fs::write(dir.join("frame_001.png.partial"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"keep").unwrap();
        assert_eq!(existing_frames(&dir, 3), vec![false, true, false]);
        assert!(holds_frames(&dir));
        assert_eq!(clear_frames(&dir).unwrap(), 2);
        assert!(!holds_frames(&dir));
        assert!(!holds_partials(&dir));
        assert!(dir.join("notes.txt").is_file());
        std::fs::write(dir.join("frame_001.png.partial"), b"x").unwrap();
        remove_partials(&dir);
        assert!(!holds_partials(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn encode_outcome_messages_keep_the_wording_of_the_direct_export() {
        use super::super::encode::EncodeOutcome;
        let dir = Path::new("out");
        let mp4 = EncodeOutcome::Mp4(PathBuf::from("out/v.mp4"));
        let (path, message) = encode_outcome_message(&mp4, dir);
        assert_eq!(path, PathBuf::from("out/v.mp4"));
        assert_eq!(
            message,
            format!("Exported {}", Path::new("out/v.mp4").display())
        );

        let gif = EncodeOutcome::Gif {
            path: PathBuf::from("out/v.gif"),
            mp4_failure_reason: None,
        };
        let (path, message) = encode_outcome_message(&gif, dir);
        assert_eq!(path, PathBuf::from("out/v.gif"));
        assert!(message.starts_with("ffmpeg not found on PATH -- exported an animated GIF"));

        let gif = EncodeOutcome::Gif {
            path: PathBuf::from("out/v.gif"),
            mp4_failure_reason: Some("boom".to_string()),
        };
        assert!(
            encode_outcome_message(&gif, dir)
                .1
                .starts_with("ffmpeg could not produce an MP4 (boom) -- exported")
        );

        let frames = EncodeOutcome::FramesOnly {
            readme: PathBuf::from("out/README.txt"),
        };
        let (path, message) = encode_outcome_message(&frames, dir);
        assert_eq!(path, PathBuf::from("out"));
        assert!(message.starts_with("No video muxer available -- left the frame sequence in"));
        assert!(message.contains("README.txt"));
    }
}
