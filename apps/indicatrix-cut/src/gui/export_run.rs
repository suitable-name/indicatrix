//! "Is an export running?" for the Rust side, the header export indicator's wording, and the
//! close guard's sentence for an export started from a dialog.
//!
//! Three things can export: the still-image export, the tilt video export (both started from a
//! dialog, the "direct" exports) and a render-queue job. Only one runs at a time. The Slint side
//! derives the same answer for the controls it disables (`ExportRunModel.running`,
//! `ui/models/export_run.slint`); [`ExportActivity`] is the Rust mirror over the same three
//! flags, used by every start guard. The progress itself stays on `ExportModel` and
//! `TiltVideoExportModel`, which outlive their popups, so "Run in background" is only closing the
//! popup.

use crate::{ExportModel, ExportRunModel, MainWindow, RenderJobsModel, TiltVideoExportModel};
use slint::ComponentHandle;

/// The refusal shown when a start is attempted while an export runs. Kept in step with
/// `ExportRunModel.busy_hint`.
pub const BUSY_MESSAGE: &str = "An export is running -- wait for it or cancel it.";

/// Which exports run right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportActivity {
    /// The still-image export.
    pub still: bool,
    /// The tilt video export.
    pub video: bool,
    /// A render-queue job.
    pub queue_job: bool,
}

impl ExportActivity {
    /// Reads the three flags off the window's globals.
    pub(crate) fn read(ui: &MainWindow) -> Self {
        Self {
            still: ui.global::<ExportModel>().get_is_exporting(),
            video: ui.global::<TiltVideoExportModel>().get_is_exporting(),
            queue_job: ui.global::<RenderJobsModel>().get_job_running(),
        }
    }

    /// Any export at all: nothing else may start.
    pub(crate) const fn running(self) -> bool {
        self.still || self.video || self.queue_job
    }

    /// An export started from a dialog: the queue may not start a job.
    pub(crate) const fn direct(self) -> bool {
        self.still || self.video
    }
}

/// `Some(BUSY_MESSAGE)` while any export runs, else `None`: what every start guard asks.
pub fn busy_refusal(ui: &MainWindow) -> Option<&'static str> {
    ExportActivity::read(ui).running().then_some(BUSY_MESSAGE)
}

/// The header indicator's text for a tilt video: `done` of `total` frames finished, `eta` the
/// estimator's sentence (empty while there is no estimate). The frame shown is the one being
/// rendered.
pub fn video_indicator_text(done: i32, total: i32, eta: &str) -> String {
    let mut text = if total > 0 {
        let shown = (done.max(0) + 1).min(total);
        format!("Exporting video -- frame {shown} of {total}")
    } else {
        "Exporting video".to_string()
    };
    push_eta(&mut text, eta);
    text
}

/// The header indicator's text for a still export: `progress` is `0..=1` over the whole
/// export; with `preset_total` above one the picture being made is named too.
pub fn still_indicator_text(
    progress: f32,
    preset_index: i32,
    preset_total: i32,
    eta: &str,
) -> String {
    let percent = (progress.clamp(0.0, 1.0) * 100.0).round() as i32;
    let mut text = if preset_total > 1 {
        let shown = (preset_index.max(0) + 1).min(preset_total);
        format!("Exporting image {shown} of {preset_total} -- {percent}%")
    } else {
        format!("Exporting image -- {percent}%")
    };
    push_eta(&mut text, eta);
    text
}

fn push_eta(text: &mut String, eta: &str) {
    if !eta.is_empty() {
        text.push_str(" \u{b7} ");
        text.push_str(eta);
    }
}

/// The close guard's sentence while an export started from a dialog runs, else `None`.
pub fn work_at_risk_text(activity: ExportActivity) -> Option<String> {
    let mut parts = Vec::new();
    if activity.still {
        parts.push(
            "An image export is running. Closing now stops it, and the pictures it has not \
             written yet are lost.",
        );
    }
    if activity.video {
        parts.push(
            "A tilt video export is running. Closing now stops it; the frames it has written \
             stay in its folder.",
        );
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// [`work_at_risk_text`] for the window as it is now.
pub fn work_at_risk(ui: &MainWindow) -> Option<String> {
    work_at_risk_text(ExportActivity::read(ui))
}

/// Answers the two text callbacks of `ExportRunModel`.
pub(super) fn setup_export_run_callbacks(ui: &MainWindow) {
    let model = ui.global::<ExportRunModel>();
    model.on_video_text(|done, total, eta| video_indicator_text(done, total, &eta).into());
    model.on_still_text(|progress, index, total, eta| {
        still_indicator_text(progress, index, total, &eta).into()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn activity(still: bool, video: bool, queue_job: bool) -> ExportActivity {
        ExportActivity {
            still,
            video,
            queue_job,
        }
    }

    #[test]
    fn an_export_is_running_when_any_of_the_three_is() {
        assert!(!activity(false, false, false).running());
        assert!(activity(true, false, false).running());
        assert!(activity(false, true, false).running());
        assert!(activity(false, false, true).running());
        assert!(activity(true, true, true).running());
    }

    #[test]
    fn only_a_dialog_export_is_direct() {
        assert!(!activity(false, false, true).direct());
        assert!(activity(true, false, false).direct());
        assert!(activity(false, true, true).direct());
        assert!(!activity(false, false, false).direct());
    }

    #[test]
    fn the_video_text_names_the_frame_and_the_estimate() {
        assert_eq!(
            video_indicator_text(36, 181, "about 2 min left"),
            "Exporting video -- frame 37 of 181 \u{b7} about 2 min left"
        );
        assert_eq!(
            video_indicator_text(0, 181, ""),
            "Exporting video -- frame 1 of 181"
        );
        // Every frame is done and the video is being joined: never "182 of 181".
        assert_eq!(
            video_indicator_text(181, 181, ""),
            "Exporting video -- frame 181 of 181"
        );
        assert_eq!(video_indicator_text(0, 0, ""), "Exporting video");
    }

    #[test]
    fn the_still_text_gives_the_percentage_and_the_picture_of_a_fan_out() {
        assert_eq!(
            still_indicator_text(0.424, 0, 1, ""),
            "Exporting image -- 42%"
        );
        assert_eq!(
            still_indicator_text(0.5, 1, 5, "about 40 s left"),
            "Exporting image 2 of 5 -- 50% \u{b7} about 40 s left"
        );
        assert_eq!(
            still_indicator_text(7.0, 0, 1, ""),
            "Exporting image -- 100%"
        );
        assert_eq!(
            still_indicator_text(-1.0, 0, 1, ""),
            "Exporting image -- 0%"
        );
    }

    #[test]
    fn the_close_guard_names_the_export_that_would_stop() {
        assert_eq!(work_at_risk_text(activity(false, false, false)), None);
        // A queue job has its own sentence (`render_jobs::work_at_risk`).
        assert_eq!(work_at_risk_text(activity(false, false, true)), None);
        let image = work_at_risk_text(activity(true, false, false)).unwrap();
        assert!(image.starts_with("An image export is running."));
        let video = work_at_risk_text(activity(false, true, false)).unwrap();
        assert!(video.starts_with("A tilt video export is running."));
    }
}
