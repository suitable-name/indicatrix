//! The Jobs window's rows as plain data: every text and every button flag of a job row,
//! worked out from the database row and the live progress, without a window.
//!
//! The Slint side only lays these out (`RenderJobRow` in `ui/models/render_jobs.slint`), so
//! each sentence is tested here. The buttons come from the state machine
//! (`indicatrix_render_jobs::state::row_actions`), so a button shows exactly when its command
//! applies.

use indicatrix_render_jobs::{
    run::{JobProgress, format_eta},
    state::{JobState, row_actions},
};
use indicatrix_vault::model::render_job::RenderJobMeta;
use std::path::Path;

/// One job row, as the Jobs window shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct RowView {
    pub id: i32,
    pub number_text: String,
    pub label: String,
    pub kind_text: String,
    pub state_word: String,
    pub state_text: String,
    pub summary: String,
    /// `0.0..=1.0`, or `-1.0` for "no bar".
    pub progress: f32,
    pub progress_text: String,
    pub error_text: String,
    pub can_pause: bool,
    pub can_resume: bool,
    pub can_restart: bool,
    pub can_cancel: bool,
    pub can_delete: bool,
    pub can_run_next: bool,
    pub can_move_up: bool,
    pub can_move_down: bool,
    pub can_show: bool,
}

/// The progress of the job that is rendering now.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveProgress {
    pub job_id: i64,
    pub progress: JobProgress,
}

/// The whole window: the rows, the queue line, and the header chip.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueView {
    pub rows: Vec<RowView>,
    /// Whether a started queue is running (the window shows Pause Queue instead of Start).
    pub queue_running: bool,
    /// Whether Start Queue can do anything: a job is waiting.
    pub can_start_queue: bool,
    /// "3 waiting · 1 done", empty with no jobs.
    pub queue_status: String,
    pub chip_visible: bool,
    pub chip_text: String,
    /// `0.0..=1.0` while a job runs, else `-1.0`.
    pub chip_progress: f32,
}

/// The text of `row` for the database row `meta`, which is `index` of `count` in the list.
/// `live` is the running job's progress, used only when the row is running.
pub fn row_view(
    meta: &RenderJobMeta,
    index: usize,
    count: usize,
    live: Option<&JobProgress>,
) -> RowView {
    let mut view = RowView {
        id: i32::try_from(meta.job_id).unwrap_or(i32::MAX),
        number_text: (index + 1).to_string(),
        label: meta.label.clone(),
        kind_text: if meta.kind == "still" {
            "Still image"
        } else {
            "Tilt video"
        }
        .to_string(),
        state_word: String::new(),
        state_text: String::new(),
        summary: meta.summary.clone(),
        progress: -1.0,
        progress_text: String::new(),
        error_text: String::new(),
        can_pause: false,
        can_resume: false,
        can_restart: false,
        can_cancel: false,
        can_delete: false,
        can_run_next: false,
        can_move_up: false,
        can_move_down: false,
        can_show: false,
    };
    let Some(state) = JobState::parse(&meta.state) else {
        // A state word this version does not know (a newer version wrote it): a neutral row
        // that can only be removed.
        "unknown".clone_into(&mut view.state_word);
        "Unknown".clone_into(&mut view.state_text);
        view.can_delete = true;
        return view;
    };
    view.state_word = state.as_str().to_string();
    view.state_text = state.label().to_string();

    let actions = row_actions(state, index == 0, index + 1 >= count);
    view.can_pause = actions.pause;
    view.can_resume = actions.resume;
    view.can_restart = actions.restart;
    view.can_cancel = actions.cancel;
    view.can_delete = actions.delete;
    view.can_run_next = actions.run_next;
    view.can_move_up = actions.move_up;
    view.can_move_down = actions.move_down;
    view.can_show = state == JobState::Done && meta.result_path.is_some();

    let (progress, text) = progress_of(meta, state, live);
    view.progress = progress;
    view.progress_text = text;
    if state == JobState::Failed {
        view.error_text = meta
            .error_text
            .clone()
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "The job failed.".to_string());
    }
    view
}

/// The progress bar fraction (`-1.0` for none) and the progress words of a row.
fn progress_of(meta: &RenderJobMeta, state: JobState, live: Option<&JobProgress>) -> (f32, String) {
    let total = meta.frames_total.max(1);
    let video = meta.frames_total > 1;
    let done = meta.frames_done.min(total);
    let share = |done: u32| done as f32 / total as f32;
    // The frame a video is on, or goes on with: the first one not finished.
    let frame_text = |done: u32| format!("frame {} of {total}", (done + 1).min(total));
    match state {
        JobState::Running => {
            let fraction = live.map_or(0.0, |p| p.fraction.clamp(0.0, 1.0));
            let eta = live
                .and_then(|p| p.eta_secs)
                .filter(|secs| secs.is_finite())
                .map(format_eta);
            let head = match (video, live) {
                (true, Some(p)) => capitalise(&frame_text(p.frames_done)),
                (true, None) => capitalise(&frame_text(done)),
                (false, Some(_)) => format!("{}%", percent(fraction)),
                (false, None) => "Starting".to_string(),
            };
            (fraction, join(head, eta))
        }
        JobState::Queued => {
            if video && done > 0 {
                (
                    share(done),
                    format!("Waiting \u{b7} {done} of {total} frames done"),
                )
            } else {
                (-1.0, "Waiting".to_string())
            }
        }
        JobState::Paused => {
            let head = if video {
                format!("Paused at {}", frame_text(done))
            } else {
                "Paused \u{b7} starts again from the beginning".to_string()
            };
            let note = meta.error_text.clone().filter(|text| !text.is_empty());
            let bar = if video && done > 0 { share(done) } else { -1.0 };
            (bar, join(head, note))
        }
        JobState::Done => (
            1.0,
            format!("Saved: {}", result_name(meta).unwrap_or_default()),
        ),
        JobState::Failed => {
            if video && done > 0 {
                (share(done), format!("{done} of {total} frames done"))
            } else {
                (-1.0, String::new())
            }
        }
        JobState::Cancelled => (-1.0, "Cancelled".to_string()),
    }
}

/// The file or folder name of what a finished job wrote.
fn result_name(meta: &RenderJobMeta) -> Option<String> {
    let path = meta.result_path.as_deref().unwrap_or(&meta.output_path);
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

fn percent(fraction: f32) -> u32 {
    (fraction.clamp(0.0, 1.0) * 100.0).round() as u32
}

fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

/// `head`, then ` · tail` when there is one.
fn join(head: String, tail: Option<String>) -> String {
    match tail {
        Some(tail) => format!("{head} \u{b7} {tail}"),
        None => head,
    }
}

/// The whole window for the database rows `metas` (in queue order). `live` belongs to the
/// running job and is ignored for any other row.
pub fn queue_view(
    metas: &[RenderJobMeta],
    queue_running: bool,
    live: Option<&LiveProgress>,
) -> QueueView {
    let count = metas.len();
    let states: Vec<Option<JobState>> = metas.iter().map(|m| JobState::parse(&m.state)).collect();
    let rows = metas
        .iter()
        .enumerate()
        .map(|(index, meta)| {
            let own_live = live
                .filter(|l| l.job_id == meta.job_id)
                .map(|l| &l.progress);
            row_view(meta, index, count, own_live)
        })
        .collect();

    let number_in = |wanted: JobState| states.iter().filter(|s| **s == Some(wanted)).count();
    let waiting = number_in(JobState::Queued);
    let running = number_in(JobState::Running);
    let status_parts = [
        (running, "running"),
        (waiting, "waiting"),
        (number_in(JobState::Paused), "paused"),
        (number_in(JobState::Failed), "failed"),
        (number_in(JobState::Done), "done"),
    ];
    let queue_status = status_parts
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, word)| format!("{n} {word}"))
        .collect::<Vec<_>>()
        .join(" \u{b7} ");

    let running_index = states.iter().position(|s| *s == Some(JobState::Running));
    let (chip_text, chip_progress) = running_index.map_or_else(
        || {
            // The chip also shows while a started queue has nothing left but the job that
            // just finished.
            let text = if waiting == 0 {
                "Jobs".to_string()
            } else {
                format!("Jobs: {waiting} waiting")
            };
            (text, -1.0)
        },
        |at| {
            // "2 of 5": the running job's place among the jobs that are waiting, running or
            // done (the ones a queue run goes through).
            let in_run = |s: &Option<JobState>| {
                matches!(
                    s,
                    Some(JobState::Queued | JobState::Running | JobState::Done)
                )
            };
            let total = states.iter().filter(|s| in_run(s)).count();
            let place = states[..=at].iter().filter(|s| in_run(s)).count();
            let fraction = live
                .filter(|l| l.job_id == metas[at].job_id)
                .map_or(0.0, |l| l.progress.fraction.clamp(0.0, 1.0));
            (
                format!("Rendering {place} of {total} \u{b7} {}%", percent(fraction)),
                fraction,
            )
        },
    );
    QueueView {
        rows,
        queue_running,
        can_start_queue: waiting > 0,
        queue_status,
        chip_visible: running > 0 || waiting > 0 || queue_running,
        chip_text,
        chip_progress,
    }
}
