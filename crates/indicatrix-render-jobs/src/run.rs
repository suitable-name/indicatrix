//! What a running job reports and how a run ends: the types the desktop executor and the
//! command-line renderer share, and the progress line text.

use crate::state::RunEnd;
use std::path::PathBuf;

/// How far a running job has come.
#[derive(Debug, Clone, PartialEq)]
pub struct JobProgress {
    /// Frames finished so far (0 or 1 for a still).
    pub frames_done: u32,
    /// Frames in the job (1 for a still).
    pub frames_total: u32,
    /// The whole job's progress, `0.0..=1.0`.
    pub fraction: f32,
    /// The estimated time left, in seconds, when there is an estimate.
    pub eta_secs: Option<f64>,
    /// A note for the person, such as a fallback from the remote worker.
    pub note: Option<String>,
}

/// What kind of problem failed a job. The command-line renderer maps it to an exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The job cannot be used: invalid values, a missing or changed HDR map, a frame
    /// folder that belongs to something else.
    Input,
    /// A file or folder could not be read or written.
    Output,
    /// The render itself failed.
    Render,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobOutcome {
    /// Everything was written.
    Done {
        /// The picture or video written, or the frame folder when no video was made.
        result: PathBuf,
        /// Something the person should know, such as why only frames were kept.
        note: Option<String>,
    },
    /// The render stopped when asked to.
    Stopped {
        /// Frames finished before it stopped.
        frames_done: u32,
    },
    /// The render failed.
    Failed {
        /// What kind of problem it was.
        kind: FailureKind,
        /// One plain sentence for the person.
        message: String,
        /// Frames finished before it failed.
        frames_done: u32,
    },
}

impl JobOutcome {
    /// The run end this outcome is, for the state machine.
    #[must_use]
    pub const fn end(&self) -> RunEnd {
        match self {
            Self::Done { .. } => RunEnd::Finished,
            Self::Stopped { .. } => RunEnd::Stopped,
            Self::Failed { .. } => RunEnd::Failed,
        }
    }

    /// Frames finished: all of `frames_total` after a finished run.
    #[must_use]
    pub const fn frames_done(&self, frames_total: u32) -> u32 {
        match self {
            Self::Done { .. } => frames_total,
            Self::Stopped { frames_done } | Self::Failed { frames_done, .. } => *frames_done,
        }
    }
}

/// Receives a job's progress. Called from the thread that renders.
pub trait JobSink {
    /// The job's progress changed.
    fn progress(&mut self, progress: &JobProgress);
    /// A frame is finished and on disk.
    fn frame_finished(&mut self, frames_done: u32, frames_total: u32);
}

/// How long is left, in words: `about 40 s left`, `about 3 min left`,
/// `about 2 h 10 min left`. Fractions of a second are dropped.
#[must_use]
pub fn format_eta(secs: f64) -> String {
    let total = if secs.is_finite() && secs > 0.0 {
        secs as u64
    } else {
        0
    };
    if total < 60 {
        format!("about {} s left", total.max(1))
    } else if total < 3600 {
        format!("about {} min left", total / 60)
    } else {
        let hours = total / 3600;
        let minutes = (total % 3600) / 60;
        if minutes == 0 {
            format!("about {hours} h left")
        } else {
            format!("about {hours} h {minutes} min left")
        }
    }
}

/// One progress line.
///
/// For a terminal (`terminal` true), a line that is rewritten in place:
/// `render  42%  about 3 min left` or
/// `tilt-video  frame 37 of 181  21%  about 12 min left`. Otherwise, for logs:
/// `render: 40%` or `tilt-video: frame 37 of 181 done, about 12 min left`. A job with one
/// frame is a still. The estimate is left out when there is none.
#[must_use]
pub fn format_progress_line(command_word: &str, progress: &JobProgress, terminal: bool) -> String {
    let video = progress.frames_total > 1;
    let percent = (progress.fraction.clamp(0.0, 1.0) * 100.0).round() as u32;
    let eta = progress
        .eta_secs
        .filter(|secs| secs.is_finite())
        .map(format_eta);
    let frame = format!(
        "frame {} of {}",
        progress.frames_done, progress.frames_total
    );
    match (terminal, video) {
        (true, false) => join_parts("  ", format!("{command_word}  {percent}%"), eta),
        (true, true) => join_parts("  ", format!("{command_word}  {frame}  {percent}%"), eta),
        (false, false) => join_parts(", ", format!("{command_word}: {percent}%"), eta),
        (false, true) => join_parts(", ", format!("{command_word}: {frame} done"), eta),
    }
}

fn join_parts(separator: &str, mut line: String, tail: Option<String>) -> String {
    if let Some(tail) = tail {
        line.push_str(separator);
        line.push_str(&tail);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(done: u32, total: u32, fraction: f32, eta: Option<f64>) -> JobProgress {
        JobProgress {
            frames_done: done,
            frames_total: total,
            fraction,
            eta_secs: eta,
            note: None,
        }
    }

    #[test]
    fn eta_boundaries() {
        assert_eq!(format_eta(0.0), "about 1 s left");
        assert_eq!(format_eta(40.4), "about 40 s left");
        assert_eq!(format_eta(59.0), "about 59 s left");
        assert_eq!(format_eta(60.0), "about 1 min left");
        assert_eq!(format_eta(190.0), "about 3 min left");
        assert_eq!(format_eta(3599.0), "about 59 min left");
        assert_eq!(format_eta(3600.0), "about 1 h left");
        assert_eq!(format_eta(7800.0), "about 2 h 10 min left");
        assert_eq!(format_eta(f64::NAN), "about 1 s left");
        assert_eq!(format_eta(-5.0), "about 1 s left");
    }

    #[test]
    fn terminal_lines() {
        assert_eq!(
            format_progress_line("render", &progress(0, 1, 0.42, Some(190.0)), true),
            "render  42%  about 3 min left"
        );
        assert_eq!(
            format_progress_line("render", &progress(0, 1, 0.42, None), true),
            "render  42%"
        );
        assert_eq!(
            format_progress_line("tilt-video", &progress(37, 181, 0.21, Some(720.0)), true),
            "tilt-video  frame 37 of 181  21%  about 12 min left"
        );
        assert_eq!(
            format_progress_line("tilt-video", &progress(37, 181, 0.21, None), true),
            "tilt-video  frame 37 of 181  21%"
        );
    }

    #[test]
    fn plain_lines() {
        assert_eq!(
            format_progress_line("render", &progress(0, 1, 0.4, None), false),
            "render: 40%"
        );
        assert_eq!(
            format_progress_line("render", &progress(0, 1, 0.4, Some(45.0)), false),
            "render: 40%, about 45 s left"
        );
        assert_eq!(
            format_progress_line("tilt-video", &progress(37, 181, 0.2, Some(720.0)), false),
            "tilt-video: frame 37 of 181 done, about 12 min left"
        );
        assert_eq!(
            format_progress_line("tilt-video", &progress(37, 181, 0.2, None), false),
            "tilt-video: frame 37 of 181 done"
        );
    }

    #[test]
    fn the_fraction_is_clamped_and_a_bad_eta_is_left_out() {
        assert_eq!(
            format_progress_line("render", &progress(0, 1, 1.7, None), false),
            "render: 100%"
        );
        assert_eq!(
            format_progress_line("render", &progress(0, 1, -0.2, None), false),
            "render: 0%"
        );
        assert_eq!(
            format_progress_line("render", &progress(0, 1, 0.5, Some(f64::NAN)), false),
            "render: 50%"
        );
    }

    #[test]
    fn outcomes_map_to_run_ends_and_frame_counts() {
        let done = JobOutcome::Done {
            result: PathBuf::from("a.png"),
            note: None,
        };
        assert_eq!(done.end(), RunEnd::Finished);
        assert_eq!(done.frames_done(181), 181);
        let stopped = JobOutcome::Stopped { frames_done: 37 };
        assert_eq!(stopped.end(), RunEnd::Stopped);
        assert_eq!(stopped.frames_done(181), 37);
        let failed = JobOutcome::Failed {
            kind: FailureKind::Render,
            message: "x".to_string(),
            frames_done: 5,
        };
        assert_eq!(failed.end(), RunEnd::Failed);
        assert_eq!(failed.frames_done(181), 5);
    }
}
