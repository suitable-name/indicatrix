//! The optical figures of the compare window: the data behind its metrics strip.
//!
//! Each solved side is measured table up (brilliance, windowing, extinction, fire and
//! scintillation, [`indicatrix_editor::metric_deltas::measure_table_up_geom`], concave tools included) in its own
//! material and under the viewport's lighting, right after the session is built (about 2
//! ms a side, on the same worker thread as the solve). The strip can then add the tilt
//! averages over four axes and 181 tilts: 2 x 724 evaluations, about 3 s, on a worker
//! thread of their own that the cutter can cancel.
//!
//! Everything here is Slint-free. [`SessionMetrics`] is the state, with the transitions of
//! a tilt run ([`SessionMetrics::begin_tilt`], [`SessionMetrics::cancel_tilt`],
//! [`SessionMetrics::tilt_progress`], [`SessionMetrics::tilt_finished`]) guarded by a run
//! number, so a late report of a cancelled or replaced run changes nothing, and
//! [`SessionMetrics::strip`] is the text the strip shows. `super::host` pushes that text
//! into the model and posts the worker's reports to the UI thread.
//!
//! The tilt thread itself (its request, its reports and the guard that always reports its
//! end) is in [`tilt_run`].

mod tilt_run;

use super::session::{CompareSession, CompareSide, resolve_metrics_material};
use crate::gui::solid_preview::cut_slider::cut_geometry_no_solve;
use indicatrix::{
    color::metrics::total_evaluations,
    geometry::{
        meet_solver::SolvedTier,
        stone_metrics::{SolidStatus, build_solid_mesh},
    },
    optics::{LightingPreset, materials::GemMaterial, raytracer::EnvironmentSource},
};
use indicatrix_cut_core::{
    Design,
    optimize::{CANONICAL_LIGHT_PITCH, CANONICAL_LIGHT_YAW},
};
use indicatrix_editor::{
    metric_deltas::{
        MetricRow, OpticalFigures, describe_metric_deltas, describe_metric_deltas_for,
        measure_table_up_geom, metric_rows,
    },
    sweep::TiltAverages,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tilt_run::{TiltOutcome, TiltRequest};
// What `super::host` names: the reports a tilt thread posts and the call that starts one.
pub(super) use tilt_run::{TiltEvent, TiltReport, spawn_tilt};

/// The subject of the sentence in the snapshot dialog: the snapshot is "before", the
/// design on the bench now is the thing that changed.
const SNAPSHOT_SUBJECT: &str = "The current design";

/// What the snapshot dialog says when one of the two designs has no figures.
const SNAPSHOT_NO_FIGURES: &str =
    "No optical comparison: one of the two designs does not close into a stone.";

/// The studio the figures are measured under: the viewport's lighting preset at the
/// canonical light pose Optimize and the angle sweep score under.
const fn environment(lighting: LightingPreset) -> EnvironmentSource<'static> {
    lighting.studio(1.0, CANONICAL_LIGHT_YAW, CANONICAL_LIGHT_PITCH)
}

/// The table-up figures of `side` in its own material, concave tools included, or `None`
/// when it does not solve into a closed stone (there is nothing honest to measure).
fn figures_for(side: &CompareSide, lighting: LightingPreset) -> Option<OpticalFigures> {
    if !side.is_solved() || side.bounding_radius.is_none() || side.stone.planes.is_empty() {
        return None;
    }
    Some(measure_table_up_geom(
        side.stone.as_geometry(),
        &side.metrics_material,
        environment(lighting),
    ))
}

/// Stops its run when dropped: a tilt run whose session is replaced or closed ends with
/// the session instead of burning a core for seconds.
#[derive(Debug)]
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Where the tilt average of a session stands.
#[derive(Debug)]
enum TiltState {
    /// Not asked for yet, or cancelled.
    NotRun,
    /// Running on its worker thread.
    Running {
        /// The run this state belongs to; reports of any other run are stale.
        run: u64,
        /// Stops the run when the state goes away.
        _stop: CancelOnDrop,
        /// Evaluations finished, over both sides.
        done: usize,
        /// Evaluations in all.
        total: usize,
    },
    /// Both averages are in.
    Done {
        /// The "before" side's averages.
        before: TiltAverages,
        /// The "after" side's averages.
        after: TiltAverages,
    },
    /// The run stopped without a result (an internal error, or the thread would not
    /// start); the sentence says so.
    Failed(String),
}

/// The measurements of one session and the state of its tilt run.
#[derive(Debug)]
pub(super) struct SessionMetrics {
    lighting: LightingPreset,
    before: Option<OpticalFigures>,
    after: Option<OpticalFigures>,
    tilt: TiltState,
    runs: u64,
}

/// The text of the strip, ready to push into the model.
#[derive(Debug, Default, Clone, PartialEq)]
pub(super) struct StripText {
    /// The table rows; empty when there are no figures.
    pub(super) rows: Vec<MetricRow>,
    /// The sentences, one per line.
    pub(super) summary: String,
    /// A dimmed line: why there are no figures, or why the tilt run failed.
    pub(super) note: String,
    /// Whether the "Tilt average" button works: both sides have figures and no run is going
    /// or finished.
    pub(super) tilt_available: bool,
    /// Whether a tilt run is going.
    pub(super) tilt_running: bool,
    /// Whether the tilt averages are in the table.
    pub(super) tilt_done: bool,
    /// How far the tilt run is, 0 to 1.
    pub(super) tilt_progress: f32,
}

impl SessionMetrics {
    /// Measures both sides of `session` under `lighting`.
    #[must_use]
    pub(super) fn measure(session: &CompareSession, lighting: LightingPreset) -> Self {
        Self {
            lighting,
            before: figures_for(&session.before, lighting),
            after: figures_for(&session.after, lighting),
            tilt: TiltState::NotRun,
            runs: 0,
        }
    }

    /// Whether both sides have figures, so a tilt average can be asked for.
    const fn both_measured(&self) -> bool {
        self.before.is_some() && self.after.is_some()
    }

    /// Whether a tilt run can be started now: both sides were measured and no run is going
    /// or finished (a cancelled or failed run can be asked for again). The strip's
    /// `tilt_available` and [`SessionMetrics::begin_tilt`] both ask this, so the button is
    /// never offered for a request that would be refused.
    const fn can_begin_tilt(&self) -> bool {
        self.both_measured()
            && !matches!(
                self.tilt,
                TiltState::Running { .. } | TiltState::Done { .. }
            )
    }

    /// Starts a tilt run: `None` unless [`SessionMetrics::can_begin_tilt`]. The returned
    /// request goes to [`spawn_tilt`].
    pub(super) fn begin_tilt(&mut self, session: &CompareSession) -> Option<TiltRequest> {
        if !self.can_begin_tilt() {
            return None;
        }
        self.runs += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.tilt = TiltState::Running {
            run: self.runs,
            _stop: CancelOnDrop(Arc::clone(&cancel)),
            done: 0,
            total: 2 * total_evaluations(),
        };
        Some(TiltRequest {
            run: self.runs,
            cancel,
            sides: [
                (
                    session.before.stone.clone(),
                    session.before.metrics_material.clone(),
                ),
                (
                    session.after.stone.clone(),
                    session.after.metrics_material.clone(),
                ),
            ],
            lighting: self.lighting,
        })
    }

    /// Cancels a running tilt run; `false` when none is running. The run's late reports
    /// are ignored from here on.
    pub(super) fn cancel_tilt(&mut self) -> bool {
        if matches!(self.tilt, TiltState::Running { .. }) {
            // Dropping the state stops the thread.
            self.tilt = TiltState::NotRun;
            true
        } else {
            false
        }
    }

    /// Records a progress report of `run`; `false` when it is not the running run.
    pub(super) const fn tilt_progress(&mut self, run: u64, finished: usize) -> bool {
        match &mut self.tilt {
            TiltState::Running {
                run: current, done, ..
            } if *current == run => {
                *done = finished;
                true
            }
            _ => false,
        }
    }

    /// Records the end of `run`; `false` when it is not the running run (cancelled or
    /// replaced meanwhile). A stop nobody asked for becomes a visible failure.
    pub(super) fn tilt_finished(&mut self, run: u64, outcome: &TiltOutcome) -> bool {
        if !matches!(&self.tilt, TiltState::Running { run: current, .. } if *current == run) {
            return false;
        }
        self.tilt = match *outcome {
            TiltOutcome::Averages { before, after } => TiltState::Done { before, after },
            TiltOutcome::Stopped => TiltState::Failed(
                "The tilt average stopped after an internal error. Press Tilt average to try again."
                    .to_owned(),
            ),
        };
        true
    }

    /// Records that the tilt thread of `run` could not be started.
    pub(super) fn tilt_failed_to_start(&mut self, run: u64, reason: String) -> bool {
        if !matches!(&self.tilt, TiltState::Running { run: current, .. } if *current == run) {
            return false;
        }
        self.tilt = TiltState::Failed(reason);
        true
    }

    /// How far the running tilt run is, 0 to 1 (0 when none is running).
    #[must_use]
    pub(super) fn progress_fraction(&self) -> f32 {
        match &self.tilt {
            TiltState::Running { done, total, .. } => *done as f32 / (*total).max(1) as f32,
            _ => 0.0,
        }
    }

    /// The tilt averages, once both are in.
    const fn tilt_pair(&self) -> Option<(&TiltAverages, &TiltAverages)> {
        match &self.tilt {
            TiltState::Done { before, after } => Some((before, after)),
            _ => None,
        }
    }

    /// What the strip shows now.
    #[must_use]
    pub(super) fn strip(&self) -> StripText {
        let running = matches!(self.tilt, TiltState::Running { .. });
        let mut text = StripText {
            tilt_available: self.can_begin_tilt(),
            tilt_running: running,
            tilt_done: self.tilt_pair().is_some(),
            tilt_progress: self.progress_fraction(),
            ..StripText::default()
        };
        if let (Some(before), Some(after)) = (&self.before, &self.after) {
            let tilt = self.tilt_pair();
            text.rows = metric_rows(before, after, tilt);
            text.summary = describe_metric_deltas(before, after, tilt).join("\n");
            if let TiltState::Failed(reason) = &self.tilt {
                text.note.clone_from(reason);
            }
        } else {
            text.note = missing_figures_note(self.before.is_some(), self.after.is_some());
        }
        text
    }
}

/// Why there are no figures.
fn missing_figures_note(before_measured: bool, after_measured: bool) -> String {
    let text = match (before_measured, after_measured) {
        (false, false) => {
            "Neither side solves into a closed stone, so there are no optical figures."
        }
        (false, true) => {
            "The before side does not solve into a closed stone, so there are no optical figures to compare."
        }
        _ => {
            "The after side does not solve into a closed stone, so there are no optical figures to compare."
        }
    };
    text.to_owned()
}

/// The table-up figures of `design`, whose masts are `solved`, in its own material; `None`
/// when it has no masts or its facets do not close into a stone.
fn design_figures(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    custom: &[GemMaterial],
    lighting: LightingPreset,
) -> Option<OpticalFigures> {
    let solved = solved?;
    let halfspaces = design.planes_from_solved(solved);
    if !matches!(build_solid_mesh(&halfspaces), SolidStatus::Closed(_)) {
        return None;
    }
    // Never solves (the masts are in hand) and, without concave tiers, holds exactly the
    // planes `design_to_gpu_planes_from_solved` gives.
    let stone = cut_geometry_no_solve(design, Some(solved), None);
    let material = resolve_metrics_material(design, custom);
    Some(measure_table_up_geom(
        stone.as_geometry(),
        &material,
        environment(lighting),
    ))
}

/// The sentence for the snapshot dialog: how the design on the bench now differs optically
/// from the snapshot, on one line. `custom` is the custom materials catalogue the designs
/// may name. The measuring takes a few milliseconds a design, so it runs on the UI thread.
#[must_use]
pub(in crate::gui::editor) fn snapshot_summary(
    snapshot: &Design,
    snapshot_solved: Option<&[SolvedTier]>,
    current: &Design,
    current_solved: Option<&[SolvedTier]>,
    custom: &[GemMaterial],
    lighting: LightingPreset,
) -> String {
    let before = design_figures(snapshot, snapshot_solved, custom, lighting);
    let after = design_figures(current, current_solved, custom, lighting);
    match (before, after) {
        (Some(before), Some(after)) => {
            describe_metric_deltas_for(SNAPSHOT_SUBJECT, &before, &after, None).join(" ")
        }
        _ => SNAPSHOT_NO_FIGURES.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::{
        editor::compare::session::{CompareOrigin, SideInput, resolve_side_material},
        solid_preview::preview_state::CameraPose,
    };
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{ConstraintTier, MaterialSelection, PreformSpec, ScheduleMeta};
    use std::{
        sync::Mutex,
        thread,
        time::{Duration, Instant},
    };

    const POSE: CameraPose = CameraPose {
        yaw: 0.0,
        pitch: 0.0,
        distance: 3.0,
    };

    fn round_brilliant() -> Design {
        let mut design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        design.material = MaterialSelection {
            name: Some("Diamond".to_string()),
            ..MaterialSelection::default()
        };
        design
    }

    fn steeper_crown() -> Design {
        let mut design = round_brilliant();
        let main = design
            .tiers
            .iter_mut()
            .find(|tier| tier.name == "Crown Main")
            .expect("the standard round brilliant has a Crown Main tier");
        main.angle_deg = 37.0;
        design
    }

    /// A design with no scale-reference anchor at all: it cannot solve.
    fn unsolvable() -> Design {
        let mut design = Design::fresh(PreformSpec::block(2.0, 1.0, 2.0), 96, 4, 1.62);
        design.tiers.push(ConstraintTier {
            angle_deg: 30.0,
            name: "A".to_string(),
            indices: vec![0.0, 24.0, 48.0, 72.0],
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        });
        design
    }

    fn input(design: Design) -> SideInput {
        let material = resolve_side_material(&design, &[]);
        let metrics_material = resolve_metrics_material(&design, &[]);
        SideInput {
            design,
            label: "side".to_string(),
            material,
            metrics_material,
        }
    }

    fn session(before: Design, after: Design) -> CompareSession {
        CompareSession::build(input(before), input(after), CompareOrigin::Optimize, POSE)
    }

    fn lighting() -> LightingPreset {
        LightingPreset::RingLights
    }

    fn averages(brilliance: f32) -> TiltAverages {
        TiltAverages {
            brilliance_pct: brilliance,
            windowing_pct: 10.0,
            extinction_pct: 20.0,
        }
    }

    #[test]
    fn two_solving_sides_are_measured_and_the_same_stone_twice_is_no_difference() {
        let session = session(round_brilliant(), round_brilliant());
        let metrics = SessionMetrics::measure(&session, lighting());
        let strip = metrics.strip();
        assert_eq!(strip.rows.len(), 5);
        assert_eq!(strip.summary, "No clear optical difference.");
        assert_eq!(strip.note, "");
        assert!(strip.tilt_available && !strip.tilt_running && !strip.tilt_done);
        for row in &strip.rows {
            assert_eq!(
                row.before, row.after,
                "{}: the same planes, the same figures",
                row.label
            );
        }
    }

    #[test]
    fn a_changed_crown_angle_gives_a_different_row_set_and_some_words() {
        let session = session(round_brilliant(), steeper_crown());
        let strip = SessionMetrics::measure(&session, lighting()).strip();
        assert_eq!(strip.rows.len(), 5);
        assert!(
            strip.rows.iter().any(|row| row.before != row.after),
            "a steeper crown must move at least one figure"
        );
        assert_ne!(strip.summary, "", "the change is put into words");
    }

    #[test]
    fn a_side_that_does_not_solve_has_no_figures_and_the_note_names_it() {
        let after_broken =
            SessionMetrics::measure(&session(round_brilliant(), unsolvable()), lighting());
        let strip = after_broken.strip();
        assert!(strip.rows.is_empty() && strip.summary.is_empty());
        assert_eq!(
            strip.note,
            "The after side does not solve into a closed stone, so there are no optical figures to compare."
        );
        assert!(!strip.tilt_available);

        let before_broken =
            SessionMetrics::measure(&session(unsolvable(), round_brilliant()), lighting());
        assert!(
            before_broken
                .strip()
                .note
                .starts_with("The before side does not solve")
        );

        let both = SessionMetrics::measure(&session(unsolvable(), unsolvable()), lighting());
        assert!(both.strip().note.starts_with("Neither side solves"));
    }

    #[test]
    fn a_tilt_run_starts_once_and_is_tracked_by_its_run_number() {
        let session = session(round_brilliant(), steeper_crown());
        let mut metrics = SessionMetrics::measure(&session, lighting());
        let request = metrics
            .begin_tilt(&session)
            .expect("both sides are measured");
        assert_eq!(request.run, 1);
        assert!(metrics.begin_tilt(&session).is_none(), "one run at a time");
        let strip = metrics.strip();
        assert!(strip.tilt_running && !strip.tilt_available && !strip.tilt_done);

        assert!(metrics.tilt_progress(1, 724));
        assert!((metrics.progress_fraction() - 0.5).abs() < 1e-6);
        assert!(
            !metrics.tilt_progress(2, 800),
            "another run's report is stale"
        );

        assert!(metrics.tilt_finished(
            1,
            &TiltOutcome::Averages {
                before: averages(50.0),
                after: averages(53.0),
            }
        ));
        let strip = metrics.strip();
        assert!(strip.tilt_done && !strip.tilt_running && !strip.tilt_available);
        assert_eq!(strip.rows.len(), 8);
        assert_eq!(strip.rows[5].label, "Tilt brilliance");
        assert!(
            strip
                .summary
                .lines()
                .last()
                .is_some_and(|line| line.starts_with(
                    "Averaged over all tilts, the after design returns more light (+3 %)"
                )),
            "{}",
            strip.summary
        );
        assert!(
            metrics.begin_tilt(&session).is_none(),
            "the averages are in: nothing to ask for"
        );
        assert!(
            !metrics.tilt_finished(1, &TiltOutcome::Stopped),
            "the run is over"
        );
    }

    #[test]
    fn cancelling_stops_the_thread_and_makes_late_reports_stale() {
        let session = session(round_brilliant(), steeper_crown());
        let mut metrics = SessionMetrics::measure(&session, lighting());
        let request = metrics.begin_tilt(&session).expect("started");
        assert!(!request.cancel.load(Ordering::Relaxed));
        assert!(metrics.cancel_tilt());
        assert!(
            request.cancel.load(Ordering::Relaxed),
            "cancel flag is raised"
        );
        assert!(!metrics.cancel_tilt(), "nothing left to cancel");
        assert!(!metrics.tilt_progress(1, 10));
        assert!(!metrics.tilt_finished(1, &TiltOutcome::Stopped));
        let strip = metrics.strip();
        assert!(strip.tilt_available && !strip.tilt_running);
        assert_eq!(strip.note, "", "a cancel is not a failure");
        // A new run can be started and gets a new number.
        let second = metrics.begin_tilt(&session).expect("again");
        assert_eq!(second.run, 2);
    }

    #[test]
    fn dropping_the_session_metrics_stops_a_running_tilt() {
        let session = session(round_brilliant(), steeper_crown());
        let mut metrics = SessionMetrics::measure(&session, lighting());
        let request = metrics.begin_tilt(&session).expect("started");
        drop(metrics);
        assert!(request.cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn a_stop_nobody_asked_for_shows_as_a_failure_and_can_be_retried() {
        let session = session(round_brilliant(), steeper_crown());
        let mut metrics = SessionMetrics::measure(&session, lighting());
        let _request = metrics.begin_tilt(&session).expect("started");
        assert!(metrics.tilt_finished(1, &TiltOutcome::Stopped));
        let strip = metrics.strip();
        assert!(
            strip
                .note
                .starts_with("The tilt average stopped after an internal error")
        );
        assert!(strip.tilt_available, "Failed allows another try");
        assert_eq!(strip.rows.len(), 5);
        assert!(metrics.begin_tilt(&session).is_some());
    }

    #[test]
    fn a_thread_that_would_not_start_is_reported_and_retryable() {
        let session = session(round_brilliant(), steeper_crown());
        let mut metrics = SessionMetrics::measure(&session, lighting());
        let _request = metrics.begin_tilt(&session).expect("started");
        assert!(
            metrics
                .tilt_failed_to_start(1, "The tilt average could not start: no thread.".to_owned())
        );
        assert_eq!(
            metrics.strip().note,
            "The tilt average could not start: no thread."
        );
        assert!(
            !metrics.tilt_failed_to_start(1, "again".to_owned()),
            "no longer running"
        );
    }

    static REPORTS: Mutex<Vec<(u64, u64, bool)>> = Mutex::new(Vec::new());

    fn collect(report: TiltReport) {
        let TiltReport {
            session_id,
            run,
            event,
        } = report;
        let stopped = matches!(event, TiltEvent::Finished(TiltOutcome::Stopped));
        REPORTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((session_id, run, stopped));
    }

    #[test]
    fn a_cancelled_tilt_thread_still_reports_that_it_is_over() {
        let session = session(round_brilliant(), steeper_crown());
        let mut metrics = SessionMetrics::measure(&session, lighting());
        let request = metrics.begin_tilt(&session).expect("started");
        // Cancelled before the thread looks: its first step answers no.
        request.cancel.store(true, Ordering::Relaxed);
        spawn_tilt(77, request, collect).expect("the thread starts");
        let give_up = Instant::now() + Duration::from_secs(60);
        loop {
            let seen = REPORTS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .any(|&(session_id, run, stopped)| session_id == 77 && run == 1 && stopped);
            if seen {
                break;
            }
            assert!(
                Instant::now() < give_up,
                "the thread never reported its end"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn the_snapshot_sentence_names_the_current_design_and_needs_both_to_close() {
        let snapshot = round_brilliant();
        let steeper = steeper_crown();
        let solved_a = snapshot.solve().expect("solves");
        let solved_b = steeper.solve().expect("solves");
        let same = snapshot_summary(
            &snapshot,
            Some(&solved_a),
            &snapshot,
            Some(&solved_a),
            &[],
            lighting(),
        );
        assert_eq!(same, "No clear optical difference.");
        let moved = snapshot_summary(
            &snapshot,
            Some(&solved_a),
            &steeper,
            Some(&solved_b),
            &[],
            lighting(),
        );
        assert!(!moved.contains('\n'), "one line");
        assert!(
            moved == "No clear optical difference." || moved.starts_with("The current design"),
            "{moved}"
        );
        let missing = snapshot_summary(&snapshot, None, &steeper, Some(&solved_b), &[], lighting());
        assert_eq!(missing, SNAPSHOT_NO_FIGURES);
    }
}
