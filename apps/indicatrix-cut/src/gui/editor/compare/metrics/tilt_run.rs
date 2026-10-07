//! The tilt thread of the compare window's metrics strip: the request it runs, the reports it
//! posts to the UI thread and the guard that makes sure the end of a run is always reported.
//!
//! Slint-free, like `super` (which keeps the measurements and the state of a run). The tilt
//! average takes about 3 s (2 x 724 evaluations), so it runs on a thread of its own that the
//! cutter can cancel; `super::host` posts the reports onto the UI thread.

use super::environment;
use indicatrix::{
    color::metrics::{SweepProgress, total_evaluations},
    optics::{LightingPreset, materials::GemMaterial},
};
use indicatrix_editor::{metric_deltas::measure_tilt_average_geom, sweep::TiltAverages};
use indicatrix_solid::preview::StoneGeometryBuf;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

/// A progress report goes to the UI thread every this many evaluations (of 1448), so the
/// bar moves smoothly without flooding the event loop.
const PROGRESS_EVERY: usize = 16;

/// What a tilt run needs, owned so it can move to its thread.
pub(in crate::gui::editor::compare) struct TiltRequest {
    /// The run number the reports carry.
    pub(in crate::gui::editor::compare) run: u64,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) sides: [(StoneGeometryBuf, GemMaterial); 2],
    pub(super) lighting: LightingPreset,
}

/// How a tilt run ended.
pub(in crate::gui::editor::compare) enum TiltOutcome {
    /// Both sides finished.
    Averages {
        /// The "before" side's averages.
        before: TiltAverages,
        /// The "after" side's averages.
        after: TiltAverages,
    },
    /// The run was stopped, or its thread died.
    Stopped,
}

/// What a tilt thread reports.
pub(in crate::gui::editor::compare) enum TiltEvent {
    /// `done` of the evaluations are finished.
    Progress {
        /// Evaluations finished over both sides.
        done: usize,
    },
    /// The run is over.
    Finished(TiltOutcome),
}

/// A tilt thread's report, tagged with the session and run it belongs to.
pub(in crate::gui::editor::compare) struct TiltReport {
    /// The compare session that asked.
    pub(in crate::gui::editor::compare) session_id: u64,
    /// The run within that session.
    pub(in crate::gui::editor::compare) run: u64,
    /// The report itself.
    pub(in crate::gui::editor::compare) event: TiltEvent,
}

/// Posts a tilt thread's end when dropped, unless the end was already sent: a run that is
/// cancelled, or whose thread panics, still tells the UI thread that it is over.
struct EndGuard {
    session_id: u64,
    run: u64,
    post: fn(TiltReport),
    sent: bool,
}

impl EndGuard {
    fn finish(&mut self, outcome: TiltOutcome) {
        self.sent = true;
        (self.post)(TiltReport {
            session_id: self.session_id,
            run: self.run,
            event: TiltEvent::Finished(outcome),
        });
    }
}

impl Drop for EndGuard {
    fn drop(&mut self) {
        if !self.sent {
            (self.post)(TiltReport {
                session_id: self.session_id,
                run: self.run,
                event: TiltEvent::Finished(TiltOutcome::Stopped),
            });
        }
    }
}

/// Starts the tilt thread for `request`. `post` hands each report to the UI thread (see
/// `super::super::host`).
///
/// # Errors
///
/// A sentence for the strip when the operating system refuses the thread.
pub(in crate::gui::editor::compare) fn spawn_tilt(
    session_id: u64,
    request: TiltRequest,
    post: fn(TiltReport),
) -> Result<(), String> {
    thread::Builder::new()
        .name("compare-tilt".to_string())
        .spawn(move || run_tilt(session_id, &request, post))
        .map(|_| ())
        .map_err(|error| format!("The tilt average could not start: {error}."))
}

/// One side's tilt average, reporting progress from `offset` evaluations in; `None` when
/// the run was cancelled.
fn tilt_side(
    session_id: u64,
    request: &TiltRequest,
    side: usize,
    post: fn(TiltReport),
) -> Option<TiltAverages> {
    let (stone, material) = &request.sides[side];
    let offset = side * total_evaluations();
    let mut step = |progress: SweepProgress| {
        if request.cancel.load(Ordering::Relaxed) {
            return false;
        }
        if progress.done.is_multiple_of(PROGRESS_EVERY) {
            post(TiltReport {
                session_id,
                run: request.run,
                event: TiltEvent::Progress {
                    done: offset + progress.done,
                },
            });
        }
        true
    };
    measure_tilt_average_geom(
        stone.as_geometry(),
        material,
        environment(request.lighting),
        &mut step,
    )
}

/// The tilt thread: both sides in turn, then the end report.
fn run_tilt(session_id: u64, request: &TiltRequest, post: fn(TiltReport)) {
    let mut guard = EndGuard {
        session_id,
        run: request.run,
        post,
        sent: false,
    };
    let Some(before) = tilt_side(session_id, request, 0, post) else {
        return;
    };
    let Some(after) = tilt_side(session_id, request, 1, post) else {
        return;
    };
    guard.finish(TiltOutcome::Averages { before, after });
}
