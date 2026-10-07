//! The Retarget dialog's validity check and optical comparison, run off the UI thread.
//!
//! Building a plan is cheap and stays synchronous ([`super::proposal::rebuild_and_push`]).
//! Judging it is not: the check solves the original and the candidate stone, builds their
//! meshes and traces three small optical columns (`indicatrix_editor::retarget::check`).
//! [`begin_check`] hands that to a worker thread and shows `Checking...` meanwhile; the
//! result comes back through [`finish_check`], which drops it when a newer request has
//! superseded it.
//!
//! # State
//!
//! The editor crate owns the logic, this module owns the one place the latest result
//! lives: a `thread_local!` [`CHECK`] slot, like [`super::RETARGET_ASYNC`] (Slint's event
//! loop is single-threaded; `EditorState::pending_retarget` keeps its older type for the
//! web app's sake, so the masts the check chose travel here instead). The worker sees only
//! the [`CHECK_SERIAL`] atomic: every request bumps it, and a worker whose serial no longer
//! matches stops at its next step.
//!
//! - [`apply_gate`] decides whether Apply may go ahead, and with which masts.
//! - [`candidate_anchors`] gives the compare view the masts to build its candidate with
//!   (a comparison of an invalid change is allowed; one still being checked is not).
//! - [`reset_check`] forgets the slot, for Optimize mode before an option is picked and for
//!   closing the dialog.
//! - [`set_external_check`] fills the slot with the verdict of an Optimize option the search
//!   already judged, so Apply and the comparison treat it like a Shift result.

use super::{
    proposal_view::{CheckView, push_check_view},
    sync_embedded_comparison,
};
use crate::{MainWindow, RetargetModel};
use indicatrix::{
    geometry::meet_solver::SolvedTier,
    optics::{LightingPreset, materials::GemMaterial},
};
use indicatrix_cut_core::{CANONICAL_LIGHTING_PRESET, Design};
use indicatrix_editor::{
    material_lookup::{EditorMaterialLookup, resolved_gem_material},
    retarget::{
        AnchorChange, GirdleAllowance, RetargetPlan,
        check::{AnalysisResult, CheckInputs, RetargetCheck, run_check},
        metrics::RetargetMetrics,
        validity::{InvalidReason, RetargetValidity},
    },
};
use slint::{ComponentHandle, Timer, TimerMode};
use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
    time::Duration,
};

/// Bumped by every request and every reset. The worker reads it to learn it was superseded.
static CHECK_SERIAL: AtomicU64 = AtomicU64::new(0);

/// Where the latest request stands.
#[derive(Default)]
enum Progress {
    /// No check is relevant: Optimize mode, the dialog is closed, nothing to apply.
    #[default]
    Idle,
    /// A worker is running.
    Checking,
    /// The worker reported.
    Done(Box<RetargetCheck>),
}

/// [`CHECK`]'s payload.
#[derive(Default)]
struct CheckSlot {
    progress: Progress,
    /// The [`CHECK_SERIAL`] value of the request `progress` belongs to.
    serial: u64,
    /// The design generation that request was built against.
    generation: u64,
    /// The analysis of the ORIGINAL stone at one design generation. It does not depend on
    /// the plan, so every slider tick of the same design reuses it.
    analysis: Option<(u64, Arc<AnalysisResult>)>,
}

thread_local! {
    static CHECK: RefCell<CheckSlot> = RefCell::new(CheckSlot::default());
    /// Holds the next worker back until requests stop arriving for [`CHECK_DEBOUNCE`].
    static DEBOUNCE: Timer = Timer::default();
}

/// How long a request waits for a newer one before its worker starts: long enough to skip
/// the ticks of a slider drag, short enough not to be felt after a click.
const CHECK_DEBOUNCE: Duration = Duration::from_millis(120);

/// What the check says about the plan built against `generation`.
pub(super) enum CheckStatus {
    /// No check applies (Optimize mode, a stale generation, nothing requested).
    Idle,
    /// The worker has not reported yet.
    Checking,
    /// The worker reported.
    Done {
        /// The masts to apply with the angles.
        anchors: Vec<AnchorChange>,
        /// `false` for an invalid retarget.
        allows_apply: bool,
        /// The verdict line, for a refusal message.
        headline: String,
    },
}

/// The status of the check for the plan built against `generation`.
pub(super) fn status_for(generation: u64) -> CheckStatus {
    CHECK.with(|cell| {
        let slot = cell.borrow();
        match &slot.progress {
            Progress::Checking if slot.generation == generation => CheckStatus::Checking,
            Progress::Done(check) if slot.generation == generation => CheckStatus::Done {
                anchors: check.anchors.clone(),
                allows_apply: check.allows_apply(),
                headline: check.validity.headline(),
            },
            Progress::Idle | Progress::Checking | Progress::Done(_) => CheckStatus::Idle,
        }
    })
}

/// The solved masts of the ORIGINAL stone, from the check run's cached analysis, when that
/// analysis belongs to `generation` and succeeded. Never reads the viewport's solve slot,
/// which can hold the candidate's ghost while the dialog is open.
pub(super) fn cached_original_solved(generation: u64) -> Option<Vec<SolvedTier>> {
    CHECK.with(|cell| {
        cell.borrow()
            .analysis
            .as_ref()
            .filter(|(cached_generation, _)| *cached_generation == generation)
            .and_then(|(_, analysis)| Result::as_ref(analysis).ok().map(|a| a.solved.clone()))
    })
}

/// Whether Apply may commit the plan built against `generation`.
pub(super) enum ApplyGate {
    /// Go ahead with these masts (none when no check applies, or the check could not run).
    Open(Vec<AnchorChange>),
    /// The check is still running.
    Checking,
    /// The check found the retarget invalid; the string is the verdict line.
    Blocked(String),
}

/// [`ApplyGate`] for the plan built against `generation`.
pub(super) fn apply_gate(generation: u64) -> ApplyGate {
    match status_for(generation) {
        CheckStatus::Idle => ApplyGate::Open(Vec::new()),
        CheckStatus::Checking => ApplyGate::Checking,
        CheckStatus::Done {
            anchors,
            allows_apply,
            headline,
        } => {
            if allows_apply {
                ApplyGate::Open(anchors)
            } else {
                ApplyGate::Blocked(headline)
            }
        }
    }
}

/// The masts the compare view builds its candidate with, for the plan built against
/// `generation`. An invalid change is still compared (that is how the cutter sees what is
/// wrong with it); one still being checked is not.
///
/// # Errors
///
/// A cutter-facing sentence while the check is running.
pub(super) fn candidate_anchors(generation: u64) -> Result<Vec<AnchorChange>, String> {
    match status_for(generation) {
        CheckStatus::Idle => Ok(Vec::new()),
        CheckStatus::Checking => {
            Err("Checking this change -- the comparison appears when it is done.".to_string())
        }
        CheckStatus::Done { anchors, .. } => Ok(anchors),
    }
}

/// Whether the check for the plan built against `generation` found it invalid.
pub(super) fn is_invalid(generation: u64) -> bool {
    matches!(
        status_for(generation),
        CheckStatus::Done {
            allows_apply: false,
            ..
        }
    )
}

/// The masts of the latest finished check, whatever generation it belongs to. For the
/// legacy viewport ghost, which has no generation at hand; empty when nothing finished.
pub(super) fn latest_anchors() -> Vec<AnchorChange> {
    CHECK.with(|cell| match &cell.borrow().progress {
        Progress::Done(check) => check.anchors.clone(),
        Progress::Idle | Progress::Checking => Vec::new(),
    })
}

/// Forgets the slot and tells a running worker to stop. Shows no validity block.
pub(super) fn reset_check(ui: &MainWindow) {
    CHECK_SERIAL.fetch_add(1, AtomicOrdering::SeqCst);
    DEBOUNCE.with(Timer::stop);
    CHECK.with(|cell| {
        let mut slot = cell.borrow_mut();
        slot.progress = Progress::Idle;
        slot.serial = 0;
    });
    push_check_view(ui, &CheckView::default());
}

/// Shows `check` as the verdict for the plan built against `generation`, without running a
/// worker: the Optimize search already judged each option it offers, so picking one just
/// copies its verdict, masts and optical table into the slot Apply, the comparison and the
/// compare window read. Supersedes any check still running.
pub(super) fn set_external_check(ui: &MainWindow, generation: u64, check: RetargetCheck) {
    let serial = CHECK_SERIAL.fetch_add(1, AtomicOrdering::SeqCst) + 1;
    DEBOUNCE.with(Timer::stop);
    let view = CheckView::from_check(&check);
    CHECK.with(|cell| {
        let mut slot = cell.borrow_mut();
        slot.progress = Progress::Done(Box::new(check));
        slot.serial = serial;
        slot.generation = generation;
    });
    push_check_view(ui, &view);
}

/// The design's current material as the tracer sees it, when the design names one.
pub(super) fn current_gem(design: &Design, custom: &[GemMaterial]) -> Option<GemMaterial> {
    let material = &design.material;
    (material.name.is_some() || material.refractive_index_override.is_some())
        .then(|| resolved_gem_material(material, &EditorMaterialLookup::new(custom)))
}

/// The verdict for a check that could not run to the end (the worker panicked): the plain
/// angle edit, as before the check existed.
fn failed_check() -> RetargetCheck {
    RetargetCheck {
        validity: RetargetValidity::unchecked(&InvalidReason::DoesNotSolve(
            "the check stopped unexpectedly".to_string(),
        )),
        anchors: Vec::new(),
        metrics: RetargetMetrics::default(),
    }
}

/// Everything a worker needs, bundled so the debounce timer can hold it until it fires.
struct CheckJob {
    design: Design,
    plan: RetargetPlan,
    custom: Vec<GemMaterial>,
    lighting: LightingPreset,
    girdle_allowance: bool,
    serial: u64,
    generation: u64,
    ui_weak: slint::Weak<MainWindow>,
}

/// Starts checking `plan` against `design` (built at `generation`).
///
/// Supersedes any check still running. Shows `Checking...` at once; Apply stays disabled
/// until [`finish_check`] reports. The worker starts only after [`CHECK_DEBOUNCE`] without
/// a newer request, so dragging the crown slider does not start a solve per tick.
pub(super) fn begin_check(
    ui: &MainWindow,
    design: &Design,
    plan: RetargetPlan,
    custom: &[GemMaterial],
    generation: u64,
) {
    let serial = CHECK_SERIAL.fetch_add(1, AtomicOrdering::SeqCst) + 1;
    CHECK.with(|cell| {
        let mut slot = cell.borrow_mut();
        slot.progress = Progress::Checking;
        slot.serial = serial;
        slot.generation = generation;
    });
    push_check_view(ui, &CheckView::checking());

    let job = RefCell::new(Some(CheckJob {
        design: design.clone(),
        plan,
        custom: custom.to_vec(),
        lighting: CANONICAL_LIGHTING_PRESET,
        girdle_allowance: ui.global::<RetargetModel>().get_girdle_allowance(),
        serial,
        generation,
        ui_weak: ui.as_weak(),
    }));
    DEBOUNCE.with(|timer| {
        timer.start(TimerMode::SingleShot, CHECK_DEBOUNCE, move || {
            let pending = job.borrow_mut().take();
            if let Some(job) = pending {
                spawn_worker(job);
            }
        });
    });
}

/// Runs one job on a worker thread; its result comes back through [`finish_check`].
fn spawn_worker(job: CheckJob) {
    // Read when the timer fires, so a check that finished meanwhile has left its analysis.
    let cached = CHECK.with(|cell| {
        cell.borrow()
            .analysis
            .as_ref()
            .filter(|(cached_generation, _)| *cached_generation == job.generation)
            .map(|(_, analysis)| Arc::clone(analysis))
    });
    std::thread::spawn(move || {
        let CheckJob {
            design,
            plan,
            custom,
            lighting,
            girdle_allowance,
            serial,
            generation,
            ui_weak,
        } = job;
        let stop = move || CHECK_SERIAL.load(AtomicOrdering::SeqCst) != serial;
        let current = current_gem(&design, &custom);
        let inputs = CheckInputs {
            design: &design,
            plan: &plan,
            current_gem: current.as_ref(),
            lighting,
            girdle: girdle_allowance.then(GirdleAllowance::standard),
        };
        // A panic inside the check must not leave the dialog on `Checking...` forever.
        let outcome = catch_unwind(AssertUnwindSafe(|| run_check(&inputs, cached, &stop)));
        let (analysis, check) = match outcome {
            Ok(Some((analysis, check))) => (Some(analysis), check),
            // Superseded: the newer request reports instead.
            Ok(None) => return,
            Err(_) => (None, failed_check()),
        };
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            finish_check(&ui, serial, generation, analysis, check);
        });
    });
}

/// A worker's result, back on the UI thread. Dropped when `serial` is no longer current.
fn finish_check(
    ui: &MainWindow,
    serial: u64,
    generation: u64,
    analysis: Option<Arc<AnalysisResult>>,
    check: RetargetCheck,
) {
    // Only Shift mode runs this check; when it is not valid the way out is Optimize.
    let view = CheckView::from_check(&check).with_optimize_hint();
    let current = CHECK.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.serial != serial {
            return false;
        }
        if let Some(analysis) = analysis {
            slot.analysis = Some((generation, analysis));
        }
        slot.progress = Progress::Done(Box::new(check));
        true
    });
    if !current {
        return;
    }
    push_check_view(ui, &view);
    // The pane opened on the plan while it was still being checked and shows only that it
    // is waiting; now that the masts are known it can build the real candidate. No editor
    // state is borrowed here.
    sync_embedded_comparison(ui, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_editor::retarget::validity::ValidityStatus;

    fn ready(valid: bool) -> RetargetCheck {
        let validity = if valid {
            let mut validity = RetargetValidity::unchecked(&InvalidReason::NotClosed);
            validity.status = ValidityStatus::Valid;
            validity.reasons.clear();
            validity
        } else {
            let mut validity = RetargetValidity::unchecked(&InvalidReason::GirdleGone);
            validity.status = ValidityStatus::Invalid;
            validity
        };
        RetargetCheck {
            validity,
            anchors: vec![AnchorChange {
                tier_index: 3,
                old_mast: 0.5,
                new_mast: 0.6,
            }],
            metrics: RetargetMetrics::default(),
        }
    }

    fn set(progress: Progress, generation: u64) {
        CHECK.with(|cell| {
            let mut slot = cell.borrow_mut();
            slot.progress = progress;
            slot.generation = generation;
        });
    }

    #[test]
    fn an_idle_slot_lets_apply_through_without_masts() {
        set(Progress::Idle, 7);
        assert!(matches!(apply_gate(7), ApplyGate::Open(anchors) if anchors.is_empty()));
        assert_eq!(candidate_anchors(7), Ok(Vec::new()));
    }

    #[test]
    fn a_running_check_holds_apply_and_the_comparison() {
        set(Progress::Checking, 7);
        assert!(matches!(apply_gate(7), ApplyGate::Checking));
        assert!(candidate_anchors(7).is_err());
    }

    #[test]
    fn a_valid_check_opens_apply_with_its_masts() {
        set(Progress::Done(Box::new(ready(true))), 7);
        match apply_gate(7) {
            ApplyGate::Open(anchors) => assert_eq!(anchors.len(), 1),
            _ => panic!("a valid check must open Apply"),
        }
        assert!(!is_invalid(7));
    }

    #[test]
    fn an_invalid_check_blocks_apply_but_still_allows_the_comparison() {
        set(Progress::Done(Box::new(ready(false))), 7);
        match apply_gate(7) {
            ApplyGate::Blocked(headline) => {
                assert!(headline.starts_with("Not valid"), "{headline}");
            }
            _ => panic!("an invalid check must block Apply"),
        }
        assert!(is_invalid(7));
        assert_eq!(candidate_anchors(7).expect("not checking").len(), 1);
    }

    #[test]
    fn a_result_for_another_generation_never_leaks_its_masts() {
        set(Progress::Done(Box::new(ready(true))), 7);
        assert!(matches!(apply_gate(8), ApplyGate::Open(anchors) if anchors.is_empty()));
        assert_eq!(candidate_anchors(8), Ok(Vec::new()));
    }

    #[test]
    fn the_latest_masts_are_empty_until_a_check_finishes() {
        set(Progress::Checking, 7);
        assert_eq!(latest_anchors(), Vec::new());
        set(Progress::Done(Box::new(ready(true))), 7);
        assert_eq!(latest_anchors().len(), 1);
    }
}
