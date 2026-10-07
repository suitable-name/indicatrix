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
    bridge::export_thread::AccumulationCarry, gui::tilt::tilt_hover_preview,
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

    // ---- Compute setup, once for the whole video -----------------------------------
    // Adapter acquisition, remote probing, and hybrid/remote calibration are all far
    // too slow to repeat every frame -- see `AccumulationCarry`'s own doc comment for
    // what `carry` seeds forward from frame to frame. A sweep with nothing left to
    // render acquires nothing.
    let needs_render = (0..angles.len()).any(|index| !is_on_disk(index));
    let gpu = match request.local_compute {
        LocalComputeTarget::CpuGpu | LocalComputeTarget::Gpu if needs_render => {
            GpuBackend::acquire()
        }
        _ => GpuBackend::disabled(),
    };
    let mut carry = AccumulationCarry::default();
    let config = render::VideoComputeConfig {
        remote: request.remote.clone(),
        local_compute: request.local_compute,
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
            |progress| reporter.frame_progress(index, total, progress.fraction, progress.note),
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
