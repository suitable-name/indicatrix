//! `render` and `tilt-video`: render a job file written by the desktop app's render queue.
//!
//! The job is rendered by the very same executor the app runs
//! (`indicatrix_cut::gui::render_jobs::headless::run_job`), so a picture made from a script
//! is the one the app would have made, and a remote worker is used through the app's own
//! client. These two commands are the exception to "a command never prints or writes": they
//! write the picture or the frames themselves, and stream their progress to the writer they
//! are given (standard error for [`crate::run`], a buffer for [`crate::run_command_line`]).
//!
//! # Output
//!
//! Standard output is one line: the path written (the PNG; the MP4 or GIF; or the frame
//! folder when no video could be made). The rest goes through the progress writer:
//!
//! | Where | What |
//! |---|---|
//! | terminal | one line rewritten in place, and a line feed at the end |
//! | not a terminal | a still: one line each time the percentage crosses a multiple of 10; a video: one line per finished frame |
//! | always | `note: ...` lines, each distinct note once |
//!
//! `--quiet` turns the progress lines off, never the notes.

use crate::{
    args::{JobCommandKind, RenderJobArgs},
    outcome::{CliError, CommandResult, EXIT_DESIGN, EXIT_IO, EXIT_RENDER, Outcome},
};
use indicatrix_cut::gui::render_jobs::headless::{
    HeadlessOptions, RemoteEndpoint, gpu_compiled_in, run_job,
};
use indicatrix_render_jobs::{
    FailureKind, JobKind, JobOutcome, JobProgress, JobSink, LocalEngines, RenderJobFile, codec,
    run::format_progress_line,
};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

/// Streams progress lines and notes to a writer, in the layout `render` and `tilt-video` use.
struct StreamSink<'a> {
    out: &'a mut dyn Write,
    word: &'static str,
    terminal: bool,
    quiet: bool,
    /// The width of the line a terminal is showing now, 0 when no line is open.
    open_len: usize,
    /// The tens of percent a plain still has already reported.
    last_decile: u32,
    /// The latest estimate, which a plain video line repeats.
    last_eta: Option<f64>,
    notes: Vec<String>,
}

impl<'a> StreamSink<'a> {
    fn new(out: &'a mut dyn Write, word: &'static str, terminal: bool, quiet: bool) -> Self {
        Self {
            out,
            word,
            terminal,
            quiet,
            open_len: 0,
            last_decile: 0,
            last_eta: None,
            notes: Vec::new(),
        }
    }

    /// Ends the line a terminal is rewriting, so the next text starts on a fresh one.
    fn end_line(&mut self) {
        if self.open_len > 0 {
            let _ = writeln!(self.out);
            self.open_len = 0;
        }
    }

    /// Prints `note: {note}` on its own line, unless this note was printed already.
    fn note(&mut self, note: &str) {
        if self.notes.iter().any(|seen| seen == note) {
            return;
        }
        self.end_line();
        let _ = writeln!(self.out, "note: {note}");
        self.notes.push(note.to_string());
    }

    /// Ends the open line and flushes.
    fn finish(&mut self) {
        self.end_line();
        let _ = self.out.flush();
    }
}

impl JobSink for StreamSink<'_> {
    fn progress(&mut self, progress: &JobProgress) {
        if let Some(note) = &progress.note {
            self.note(note);
        }
        self.last_eta = progress.eta_secs;
        if self.quiet {
            return;
        }
        if self.terminal {
            let line = format_progress_line(self.word, progress, true);
            let width = self.open_len;
            let _ = write!(self.out, "\r{line:<width$}");
            let _ = self.out.flush();
            self.open_len = line.chars().count();
        } else if progress.frames_total <= 1 {
            let decile = ((progress.fraction.clamp(0.0, 1.0) * 100.0).round() as u32) / 10;
            if decile > self.last_decile {
                self.last_decile = decile;
                let _ = writeln!(
                    self.out,
                    "{}",
                    format_progress_line(self.word, progress, false)
                );
            }
        }
    }

    fn frame_finished(&mut self, frames_done: u32, frames_total: u32) {
        if self.quiet || self.terminal || frames_total <= 1 {
            return;
        }
        let progress = JobProgress {
            frames_done,
            frames_total,
            fraction: frames_done as f32 / frames_total as f32,
            eta_secs: self.last_eta,
            note: None,
        };
        let _ = writeln!(
            self.out,
            "{}",
            format_progress_line(self.word, &progress, false)
        );
    }
}

/// Refuses a job file of the other kind, naming the command that runs it.
fn check_kind(kind: JobCommandKind, job: &RenderJobFile) -> Result<(), CliError> {
    match (kind, &job.kind) {
        (JobCommandKind::Render, JobKind::TiltVideo(_)) => Err(CliError::usage(
            "This job file holds a tilt video; run it with: indicatrix-cli tilt-video FILE",
        )),
        (JobCommandKind::TiltVideo, JobKind::Still(_)) => Err(CliError::usage(
            "This job file holds a still picture; run it with: indicatrix-cli render FILE",
        )),
        _ => Ok(()),
    }
}

/// The folder of the job file: relative paths in the job resolve against it.
fn job_folder(job: &Path) -> PathBuf {
    job.parent()
        .filter(|folder| !folder.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// The exit code a failed run ends the process with.
const fn failure_code(kind: FailureKind) -> i32 {
    match kind {
        FailureKind::Input => EXIT_DESIGN,
        FailureKind::Output => EXIT_IO,
        FailureKind::Render => EXIT_RENDER,
    }
}

/// Renders the job file `args.job`, streaming progress to `progress`.
///
/// `terminal` says `progress` is a terminal, which rewrites one line instead of printing a
/// line per step.
///
/// # Errors
///
/// - [`CliError::io`] when the job file cannot be read;
/// - [`CliError::design`] when it is not a usable job file;
/// - [`CliError::usage`] when it holds the other kind of job;
/// - a [`EXIT_RENDER`] error when the render was stopped, and the exit code of the failure
///   (`EXIT_DESIGN`, `EXIT_IO` or `EXIT_RENDER`) when it failed.
pub fn run(args: &RenderJobArgs, progress: &mut dyn Write, terminal: bool) -> CommandResult {
    let text = std::fs::read_to_string(&args.job)
        .map_err(|e| CliError::io(format!("cannot read {}: {e}", args.job.display())))?;
    let job = codec::from_text(&text).map_err(|e| CliError::design(e.to_string()))?;
    check_kind(args.kind, &job)?;

    let remote = match (&args.remote, &args.cert_dir) {
        (Some(address), Some(cert_dir)) => Some(RemoteEndpoint {
            address: address.clone(),
            cert_dir: cert_dir.clone(),
        }),
        _ => None,
    };
    let options = HeadlessOptions {
        local: args.local,
        remote,
        compute: args.compute,
        transfer: args.transfer,
        contribute_local: args.contribute_local.then_some(true),
        output: args.out.clone(),
        restart_frames: args.restart,
    };
    // Ctrl+C is not trapped: a video's frames are written atomically, so the next run
    // continues from them.
    let cancel = AtomicBool::new(false);
    let mut sink = StreamSink::new(progress, args.kind.word(), terminal, args.quiet);
    if args.local != LocalEngines::Cpu && !gpu_compiled_in() {
        sink.note("this build has no GPU support; rendering on the CPU.");
    }
    let ended = run_job(&job, &job_folder(&args.job), &options, &cancel, &mut sink);
    sink.finish();

    match ended {
        JobOutcome::Done { result, note } => {
            let mut outcome = Outcome::text(format!("{}\n", result.display()));
            if let Some(note) = note {
                outcome = outcome.with_note(&format!("note: {note}"));
            }
            Ok(outcome.with_note(&format!("wrote {}", result.display())))
        }
        JobOutcome::Stopped { .. } => Err(CliError {
            code: EXIT_RENDER,
            message: "the render was stopped".to_string(),
        }),
        JobOutcome::Failed { kind, message, .. } => Err(CliError {
            code: failure_code(kind),
            message,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        outcome::{EXIT_OK, EXIT_USAGE},
        run_command_line, testing,
    };
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };
    use indicatrix_net::{SceneState, scene::SceneEnvironment};
    use indicatrix_render_jobs::{
        ComputeChoice, JobColorSpace, StillJob, TiltVideoJob, TransferChoice,
        job::{DesignInfo, JOB_FORMAT_VERSION, JobCompute, OverlaySelection},
    };

    /// The smallest scene a job accepts: a round brilliant, 16 by 16, four bounces.
    fn tiny_scene() -> SceneState {
        SceneState {
            width: 16,
            height: 16,
            yaw: 0.4,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
            backdrop: 0.0,
            environment: SceneEnvironment::Studio,
            surface_glare: 1.0,
            tools: Vec::new(),
            fluorescence: indicatrix::optics::fluorescence::Fluorescence::default(),
            head_shadow_deg: 16.0,
        }
    }

    fn job_with(output: &Path, kind: JobKind) -> RenderJobFile {
        RenderJobFile {
            format: JOB_FORMAT_VERSION,
            token: "9f2c4a7d1e0b48c3a65d2f71c8e4b903".to_string(),
            label: "Test job".to_string(),
            created_at: 1_791_302_580,
            design: DesignInfo::default(),
            scene: tiny_scene(),
            hdr: None,
            compute: JobCompute {
                target: ComputeChoice::Local,
                transfer: TransferChoice::FullData,
                contribute_local: false,
            },
            output: output.to_string_lossy().into_owned(),
            kind,
        }
    }

    fn still_kind() -> JobKind {
        JobKind::Still(StillJob {
            samples_per_pixel: 2,
            color_space: JobColorSpace::Srgb,
            preset_label: String::new(),
        })
    }

    fn video_kind() -> JobKind {
        JobKind::TiltVideo(TiltVideoJob {
            axis_index: 0,
            start_deg: 0.0,
            end_deg: 1.0,
            step_deg: 1.0,
            total_frames: 2,
            fps: 2,
            samples_per_pixel: 1,
            color_space: JobColorSpace::Srgb,
            overlay: OverlaySelection::default(),
            curves: None,
            keep_frames: true,
            video_name: "tiny tilt".to_string(),
        })
    }

    /// A fresh, empty folder for one test.
    fn folder(name: &str) -> PathBuf {
        let folder = testing::temp_path(name);
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("the test folder can be made");
        folder
    }

    fn write_job(folder: &Path, job: &RenderJobFile) -> String {
        let path = folder.join("tiny.job.json");
        std::fs::write(&path, codec::to_text(job).expect("the job encodes"))
            .expect("the job file can be written");
        path.to_string_lossy().into_owned()
    }

    fn run_words(parts: &[&str]) -> crate::Outcome {
        let args: Vec<String> = parts.iter().map(|part| (*part).to_string()).collect();
        run_command_line(&args)
    }

    #[test]
    fn a_still_job_renders_a_png_and_prints_its_path() {
        let folder = folder("render-job-still");
        let png = folder.join("out.png");
        let job = write_job(&folder, &job_with(&png, still_kind()));
        let outcome = run_words(&["render", &job, "--local", "cpu"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert!(png.is_file(), "the picture is on disk");
        assert_eq!(outcome.stdout, format!("{}\n", png.display()));
        assert!(
            outcome.stderr.contains(&format!("wrote {}", png.display())),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn quiet_leaves_out_the_progress_lines() {
        let folder = folder("render-job-quiet");
        let png = folder.join("out.png");
        let job = write_job(&folder, &job_with(&png, still_kind()));
        let outcome = run_words(&["render", &job, "--local", "cpu", "--quiet"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert!(!outcome.stderr.contains("render:"), "{}", outcome.stderr);
        assert!(png.is_file());
    }

    #[test]
    fn a_still_file_under_tilt_video_is_a_command_line_mistake() {
        let folder = folder("render-job-wrong-kind");
        let job = write_job(&folder, &job_with(&folder.join("out.png"), still_kind()));
        let outcome = run_words(&["tilt-video", &job, "--local", "cpu"]);
        assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
        assert!(
            outcome.stderr.contains("indicatrix-cli render FILE"),
            "{}",
            outcome.stderr
        );

        let video = write_job(&folder, &job_with(&folder.join("frames"), video_kind()));
        let outcome = run_words(&["render", &video, "--local", "cpu"]);
        assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
        assert!(
            outcome.stderr.contains("indicatrix-cli tilt-video FILE"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_missing_job_file_is_exit_3() {
        let missing = testing::temp_path("render-job-no-such-folder").join("none.job.json");
        let outcome = run_words(&["render", &missing.to_string_lossy(), "--local", "cpu"]);
        assert_eq!(outcome.exit, EXIT_IO, "{}", outcome.stderr);
        assert!(
            outcome.stderr.starts_with("error: cannot read"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_job_file_of_a_newer_format_is_exit_2() {
        let folder = folder("render-job-newer");
        let job = job_with(&folder.join("out.png"), still_kind());
        let mut value: serde_json::Value =
            serde_json::from_str(&codec::to_text(&job).expect("the job encodes"))
                .expect("the text is JSON");
        value["format"] = serde_json::json!(99);
        let path = folder.join("newer.job.json");
        std::fs::write(&path, value.to_string()).expect("the job file can be written");
        let outcome = run_words(&["render", &path.to_string_lossy(), "--local", "cpu"]);
        assert_eq!(outcome.exit, EXIT_DESIGN, "{}", outcome.stderr);
        assert!(!folder.join("out.png").exists());
    }

    #[test]
    fn a_tilt_video_resumes_from_its_frames_and_restart_renders_them_again() {
        let folder = folder("render-job-video");
        let frames = folder.join("frames");
        let job = write_job(&folder, &job_with(&frames, video_kind()));

        let first = run_words(&["tilt-video", &job, "--local", "cpu"]);
        assert_eq!(first.exit, EXIT_OK, "{}", first.stderr);
        let video = first.stdout.trim().to_string();
        let extension = Path::new(&video)
            .extension()
            .map(std::ffi::OsStr::to_ascii_lowercase);
        assert!(
            extension.is_some_and(|extension| extension == "mp4" || extension == "gif"),
            "{}",
            first.stdout
        );
        assert!(
            first.stderr.contains("frame 1 of 2 done"),
            "{}",
            first.stderr
        );
        assert!(
            first.stderr.contains("frame 2 of 2 done"),
            "{}",
            first.stderr
        );

        std::fs::remove_file(&video).expect("the video was written");
        let second = run_words(&["tilt-video", &job, "--local", "cpu"]);
        assert_eq!(second.exit, EXIT_OK, "{}", second.stderr);
        assert!(
            !second.stderr.contains("frame 1 of 2 done"),
            "frame 1 is reused: {}",
            second.stderr
        );
        assert!(Path::new(&video).is_file(), "the video is made again");

        let third = run_words(&["tilt-video", &job, "--local", "cpu", "--restart"]);
        assert_eq!(third.exit, EXIT_OK, "{}", third.stderr);
        assert!(
            third.stderr.contains("frame 1 of 2 done"),
            "{}",
            third.stderr
        );
        assert!(
            third.stderr.contains("frame 2 of 2 done"),
            "{}",
            third.stderr
        );
    }

    #[test]
    fn the_sink_prints_plain_lines_per_ten_percent_and_each_note_once() {
        let mut buffer: Vec<u8> = Vec::new();
        {
            let mut sink = StreamSink::new(&mut buffer, "render", false, false);
            let at = |fraction: f32, note: Option<&str>| JobProgress {
                frames_done: 0,
                frames_total: 1,
                fraction,
                eta_secs: None,
                note: note.map(str::to_string),
            };
            sink.progress(&at(0.04, None));
            sink.progress(&at(0.12, Some("slow")));
            sink.progress(&at(0.15, Some("slow")));
            sink.progress(&at(0.4, None));
            sink.finish();
        }
        let text = String::from_utf8(buffer).expect("text");
        assert_eq!(text, "note: slow\nrender: 12%\nrender: 40%\n");
    }

    #[test]
    fn a_terminal_sink_rewrites_one_line_and_ends_it() {
        let mut buffer: Vec<u8> = Vec::new();
        {
            let mut sink = StreamSink::new(&mut buffer, "render", true, false);
            let at = |fraction: f32| JobProgress {
                frames_done: 0,
                frames_total: 1,
                fraction,
                eta_secs: None,
                note: None,
            };
            sink.progress(&at(0.5));
            sink.progress(&at(1.0));
            sink.finish();
        }
        let text = String::from_utf8(buffer).expect("text");
        assert_eq!(text, "\rrender  50%\rrender  100%\n");
    }
}
