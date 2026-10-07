//! The manufacturability pass the PLAN worker computes after it has handed its frame on
//! ([`LateFindings`]): which plans are owed one, how it is found again for a frame the render
//! worker has not drawn yet, and that a real replan delivers it AFTER its frame.

use super::{
    super::{
        controller::SharedWarnings,
        plan_worker::{
            findings_job, findings_of_plan, finish_findings, newer_plan_will_run_a_pass,
            store_findings,
        },
        render::findings_for_frame,
        request::PlanJob,
    },
    *,
};
use crate::{
    bridge::render_thread::RedrawGate,
    gui::{editor::frame_updates_mast_cache, solid_sink::frame_files_masts},
};
use indicatrix::geometry::meet_solver::{MeetConstraint, SolveStrategy, SolvedTier};
use indicatrix_cut_core::{
    ConstraintTier, Design, ManufacturabilityWarning, PreformSpec, ScheduleMeta,
};
use indicatrix_editor::view_model::rows::solved_manufacturability_warnings;
use std::{
    collections::BTreeSet,
    sync::{Arc, Condvar, Mutex, PoisonError},
};

/// The generation the Slice tool stamps on its provisional plans
/// (`gui::editor::manipulate::PROVISIONAL_GENERATION`, private to that module).
const PROVISIONAL: u64 = u64::MAX - 1;

/// What the sink saw of one frame.
#[derive(Clone)]
struct Seen {
    planned: bool,
    generation: u64,
    warnings: Option<Arc<Vec<ManufacturabilityWarning>>>,
    /// Whether the frame carried a mast list.
    has_masts: bool,
}

/// What the sink saw of one findings update.
#[derive(Clone)]
struct LateSeen {
    generation: u64,
    masts: usize,
    warnings: Arc<Vec<ManufacturabilityWarning>>,
    /// Whether a frame had already been drawn when the findings were delivered.
    frame_was_first: bool,
}

#[derive(Default)]
struct Events {
    frames: Vec<Seen>,
    late: Vec<LateSeen>,
}

struct WarningSink {
    events: Mutex<Events>,
    arrived: Condvar,
}

impl WarningSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Events::default()),
            arrived: Condvar::new(),
        })
    }

    /// Blocks until `ready` holds for what the sink has seen so far and returns it.
    fn wait_for_events(
        &self,
        what: &str,
        ready: impl Fn(&Events) -> bool,
    ) -> (Vec<Seen>, Vec<LateSeen>) {
        let (guard, result) = self
            .arrived
            .wait_timeout_while(
                self.events.lock().unwrap_or_else(PoisonError::into_inner),
                DEADLINE,
                |events| !ready(events),
            )
            .unwrap_or_else(PoisonError::into_inner);
        assert!(!result.timed_out(), "timed out waiting for {what}");
        (guard.frames.clone(), guard.late.clone())
    }

    /// Blocks until `ready` holds for the frames seen so far and returns them.
    fn wait_for(&self, what: &str, ready: impl Fn(&[Seen]) -> bool) -> Vec<Seen> {
        self.wait_for_events(what, |events| ready(&events.frames)).0
    }
}

impl PreviewSink for WarningSink {
    fn apply(&self, frame: PreviewFrame) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .frames
            .push(Seen {
                planned: frame.planned,
                generation: frame.generation,
                warnings: frame.warnings,
                has_masts: frame.solved.is_some(),
            });
        self.arrived.notify_all();
    }

    fn apply_findings(&self, findings: LateFindings) {
        // The plan worker submits the frame BEFORE it delivers the findings, so a frame
        // reaches the sink while this call waits for one. Had the worker run the pass first
        // it would be stuck right here, the frame would never be submitted, and the wait
        // would run out.
        let (mut events, result) = self
            .arrived
            .wait_timeout_while(
                self.events.lock().unwrap_or_else(PoisonError::into_inner),
                DEADLINE,
                |events| events.frames.is_empty(),
            )
            .unwrap_or_else(PoisonError::into_inner);
        let frame_was_first = !result.timed_out();
        events.late.push(LateSeen {
            generation: findings.generation,
            masts: findings.masts.len(),
            warnings: findings.warnings,
            frame_was_first,
        });
        drop(events);
        self.arrived.notify_all();
    }
}

fn a_replan_of(design: &Arc<Design>, generation: u64) -> ReplanRequest {
    ReplanRequest {
        design: Arc::clone(design),
        dirty: BTreeSet::new(),
        last_solved: None,
        camera: CAMERA,
        size: (16, 16),
        selected_tier: None,
        n_d: design.effective_refractive_index(),
        view_mode: 0,
        generation,
        show_preform: true,
        enlarged_panel: -1,
    }
}

/// A plan whose masts describe its design is owed exactly the pass the inline code would
/// run; one whose masts do not (unsolvable, another length, none) is owed none, because
/// findings computed from foreign masts would badge the wrong rows. A stale plan is owed none
/// either, for another reason: its masts are the fresh solve and do describe the design, but
/// the frame is one edit behind and the idle replan's fresh plan brings the pass (F4-12).
#[test]
fn only_a_plan_whose_masts_describe_its_design_is_owed_findings() {
    let design = Arc::new(Design::concave_fixture());
    let solved = design.solve().expect("the fixture solves");
    let expected = solved_manufacturability_warnings(&design, &solved);

    let owed = findings_job(7, &design, Some(&solved), false, false).expect("owed the pass");
    assert_eq!(owed.run(), expected);
    assert!(findings_job(7, &design, Some(&solved), true, false).is_none());
    assert!(findings_job(7, &design, Some(&solved), false, true).is_none());
    assert!(findings_job(7, &design, None, false, false).is_none());
    // A mast list left over from before a tier was added or removed: asking for findings
    // from it would index out of range.
    let leftover = vec![SolvedTier {
        mast: 0.5,
        strategy: SolveStrategy::ScaleReference,
        detail: "from before the edit".to_string(),
    }];
    assert_ne!(leftover.len(), design.tiers.len());
    assert!(findings_job(7, &design, Some(&leftover), false, false).is_none());
}

/// F3-12: a Slice-tool provisional plan runs no manufacturability pass at all -- its findings
/// are never shown (the sink files rows for committed frames only), so the pass would only
/// delay the next plan. The same plan under a real generation is owed it.
#[test]
fn a_provisional_plan_is_owed_no_findings() {
    assert!(!frame_updates_mast_cache(PROVISIONAL), "the premise");
    let design = Arc::new(Design::concave_fixture());
    let solved = design.solve().expect("the fixture solves");

    assert!(findings_job(PROVISIONAL, &design, Some(&solved), false, false).is_none());
    assert!(findings_job(7, &design, Some(&solved), false, false).is_some());
}

/// Findings are found by the very design allocation the plan carried -- two designs that
/// merely compare equal are two plans -- and only the newest few are kept.
#[test]
fn findings_are_found_by_design_allocation_and_the_oldest_are_forgotten() {
    let slot = SharedWarnings::default();
    let first = Arc::new(Design::concave_fixture());
    let twin = Arc::new(Design::concave_fixture());
    let finding = ManufacturabilityWarning::ToolEnclosed {
        tier: 0,
        placement: 0,
    };
    store_findings(&slot, &first, vec![finding.clone()]);

    assert_eq!(
        findings_of_plan(&slot, &first).as_deref(),
        Some(&vec![finding])
    );
    assert!(
        findings_of_plan(&slot, &twin).is_none(),
        "an equal design that is not the planned allocation is not the plan"
    );

    let later: Vec<Arc<Design>> = (0..4)
        .map(|_| Arc::new(Design::concave_fixture()))
        .collect();
    for design in &later {
        store_findings(&slot, design, Vec::new());
    }
    assert!(
        findings_of_plan(&slot, &first).is_none(),
        "the oldest entry makes room for the newest"
    );
    assert!(later.iter().all(|d| findings_of_plan(&slot, d).is_some()));
}

/// A frame shows the carried findings only while it still describes that plan.
#[test]
fn a_frame_of_another_generation_shows_no_findings() {
    let findings = Arc::new(vec![ManufacturabilityWarning::ToolsOverlap { a: 0, b: 1 }]);
    let carried = (5, Arc::clone(&findings));

    assert_eq!(
        findings_for_frame(Some(&carried), 5).as_deref(),
        Some(&*findings)
    );
    assert!(findings_for_frame(Some(&carried), 6).is_none());
    assert!(findings_for_frame(None, 5).is_none());
}

/// The whole path: a replan of a design with concave tools hands its frame to the render
/// worker first and delivers the pass of its own masts as a follow-up update. The test sink
/// refuses to take the findings until a frame has been drawn, so a worker that ran the pass
/// before submitting would deadlock the frame and fail here.
#[test]
fn a_replan_hands_its_frame_on_before_its_findings() {
    let sink = WarningSink::new();
    let state = SolidPreviewState::new(sink.clone());
    let design = Arc::new(Design::concave_fixture());
    let solved = design.solve().expect("the fixture solves");
    let expected = solved_manufacturability_warnings(&design, &solved);

    state.request_replan(a_replan_of(&design, 7));
    let (frames, late) = sink.wait_for_events("the frame and its findings", |events| {
        events.frames.iter().any(|frame| frame.planned) && !events.late.is_empty()
    });

    let planned = frames
        .iter()
        .find(|frame| frame.planned)
        .expect("one frame");
    assert_eq!(planned.generation, 7);
    // Whether the frame carried the findings depends on whether the pass beat the render
    // worker; if it did, they are the same ones.
    if let Some(carried) = &planned.warnings {
        assert_eq!(carried.as_ref(), &expected);
    }
    assert_eq!(late.len(), 1, "one follow-up for one plan");
    assert_eq!(late[0].generation, 7);
    assert_eq!(late[0].masts, design.tiers.len());
    assert_eq!(late[0].warnings.as_ref(), &expected);
    assert!(
        late[0].frame_was_first,
        "the frame must not wait for the findings pass"
    );
}

/// F3-12 end to end: a provisional plan delivers its frame and no findings. The plan worker
/// handles plans one at a time, so when the NEXT plan's findings arrive any findings of the
/// provisional plan would have arrived before them.
#[test]
fn a_provisional_replan_runs_no_findings_pass() {
    let sink = WarningSink::new();
    let state = SolidPreviewState::new(sink.clone());
    let provisional = Arc::new(Design::concave_fixture());
    let committed = Arc::new(Design::concave_fixture());

    state.request_replan(a_replan_of(&provisional, PROVISIONAL));
    sink.wait_for("the provisional frame", |frames| {
        frames
            .iter()
            .any(|frame| frame.planned && frame.generation == PROVISIONAL)
    });
    state.request_replan(a_replan_of(&committed, 8));
    let (_, late) =
        sink.wait_for_events("the next plan's findings", |events| !events.late.is_empty());

    assert!(
        late.iter()
            .all(|findings| findings.generation != PROVISIONAL),
        "no findings for the provisional plan"
    );
    assert_eq!(late[0].generation, 8);
}

/// Two tiers that meet nothing: the design does not solve.
fn unsolvable_design() -> Design {
    let tier = |name: &str, angle_deg: f64| ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: vec![0.0],
        constraint: MeetConstraint::MeetExisting,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    };
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gear_teeth: 96,
            ..ScheduleMeta::default()
        },
        vec![tier("C1", 30.0), tier("C2", 40.0)],
    )
}

/// F4-8 end to end, as the sink sees it: a plan that solves, a plan that does not, a camera
/// redraw. The camera redraw repeats the first plan's masts under the second plan's
/// generation (which has no masts of its own), and the unsolvable plan carries none; filing
/// those would let an export find the old masts "for exactly the new generation". With the
/// sink's rule only the solved plan's masts are ever filed, under the generation they were
/// solved for.
#[test]
fn only_the_solved_plan_has_masts_to_file_when_the_next_plan_does_not_solve() {
    let sink = WarningSink::new();
    let state = SolidPreviewState::new(sink.clone());

    let solvable = Arc::new(Design::concave_fixture());
    state.request_replan(a_replan_of(&solvable, 1));
    sink.wait_for("the solved plan", |frames| {
        frames.iter().any(|frame| frame.planned)
    });
    state.request_replan(a_replan_of(&Arc::new(unsolvable_design()), 2));
    sink.wait_for("the unsolvable plan", |frames| {
        frames
            .iter()
            .any(|frame| frame.planned && frame.generation == 2)
    });
    state.request_redraw(&box_planes(0.6), CAMERA, (16, 16), 0);
    let frames = sink.wait_for("the camera redraw", |frames| {
        frames.iter().any(|frame| !frame.planned)
    });

    let summary: Vec<(bool, u64, bool)> = frames
        .iter()
        .map(|frame| (frame.planned, frame.generation, frame.has_masts))
        .collect();
    assert_eq!(
        summary,
        [(true, 1, true), (true, 2, false), (false, 2, true)],
        "the unsolvable plan carries no masts; the camera redraw repeats the first plan's"
    );
    let filed: Vec<u64> = frames
        .iter()
        .filter(|frame| frame.has_masts && frame_files_masts(frame.planned, false))
        .map(|frame| frame.generation)
        .collect();
    assert_eq!(
        filed,
        [1],
        "masts are filed only under the generation they solved"
    );
}

/// A sink that only records which generations it was handed findings for.
#[derive(Default)]
struct FindingsLog {
    generations: Mutex<Vec<u64>>,
}

impl FindingsLog {
    fn delivered(&self) -> Vec<u64> {
        self.generations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl PreviewSink for FindingsLog {
    fn apply(&self, _frame: PreviewFrame) {}

    fn apply_findings(&self, findings: LateFindings) {
        self.generations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(findings.generation);
    }
}

/// A plan job for `design` at `generation`, as the plan gate holds it while it waits.
fn queued_job(design: &Arc<Design>, generation: u64) -> PlanJob {
    PlanJob {
        design: Arc::clone(design),
        dirty: BTreeSet::new(),
        last_solved: None,
        camera: CAMERA,
        size: (16, 16),
        selected_tier: None,
        n_d: design.effective_refractive_index(),
        view_mode: 0,
        generation,
        show_preform: true,
        enlarged_panel: -1,
        tier_cutoff: None,
        cut_steps: None,
    }
}

/// F3-17: only a plan with a plan of its own pass behind it is skipped.
#[test]
fn only_a_queued_committed_plan_makes_a_pass_not_worth_running() {
    assert!(!newer_plan_will_run_a_pass(None), "nothing queued");
    assert!(
        newer_plan_will_run_a_pass(Some(8)),
        "a committed plan waits"
    );
    assert!(
        newer_plan_will_run_a_pass(Some(7)),
        "a plan of the same generation (a tier click) has its own pass"
    );
    assert!(
        !newer_plan_will_run_a_pass(Some(PROVISIONAL)),
        "a Slice-tool plan owes no pass, so the committed plan's pass is the only one"
    );
}

/// F3-17: the pass runs on the plan worker, so a drag on a concave design must not wait for
/// the pass of a plan the next one replaces. With a newer plan queued the pass is skipped (it
/// stores nothing and delivers nothing); the plan nothing waits behind still gets its findings.
#[test]
fn a_plan_with_a_newer_plan_queued_runs_no_pass_and_the_newest_still_gets_its_findings() {
    let design = Arc::new(Design::concave_fixture());
    let solved = design.solve().expect("the fixture solves");
    let owed = |plan: &Arc<Design>, generation: u64| {
        findings_job(generation, plan, Some(&solved), false, false).expect("owed the pass")
    };
    let log = Arc::new(FindingsLog::default());
    let state = SolidPreviewState::new(log.clone());
    let gate: RedrawGate<PlanJob> = RedrawGate::new();

    // A burst: plan 7 is done, plan 8 is already waiting. Plan 7's pass is skipped.
    let first = Arc::new(Design::concave_fixture());
    gate.submit(queued_job(&design, 8));
    finish_findings(&state, &state.warnings, &gate, Some(owed(&first, 7)));
    assert_eq!(
        log.delivered(),
        Vec::<u64>::new(),
        "no pass for the plan overtaken"
    );
    assert!(
        findings_of_plan(&state.warnings, &first).is_none(),
        "and nothing stored for it"
    );

    // Plan 8 is taken and finished; nothing waits behind it, so its findings arrive.
    assert!(gate.take().is_some());
    finish_findings(&state, &state.warnings, &gate, Some(owed(&design, 8)));
    assert_eq!(log.delivered(), [8]);
    assert!(findings_of_plan(&state.warnings, &design).is_some());
}

/// F3-17: a Slice-tool plan waiting behind a committed plan does not take its pass away: it
/// owes none, and nothing else would badge the committed design's rows.
#[test]
fn a_queued_provisional_plan_does_not_cost_the_committed_plan_its_pass() {
    let design = Arc::new(Design::concave_fixture());
    let solved = design.solve().expect("the fixture solves");
    let log = Arc::new(FindingsLog::default());
    let state = SolidPreviewState::new(log.clone());
    let gate: RedrawGate<PlanJob> = RedrawGate::new();

    gate.submit(queued_job(&design, PROVISIONAL));
    let owed = findings_job(7, &design, Some(&solved), false, false).expect("owed the pass");
    finish_findings(&state, &state.warnings, &gate, Some(owed));
    assert_eq!(log.delivered(), [7]);
}

/// A plan that is owed no pass has nothing to skip or run.
#[test]
fn a_plan_owed_no_pass_delivers_nothing() {
    let log = Arc::new(FindingsLog::default());
    let state = SolidPreviewState::new(log.clone());
    let gate: RedrawGate<PlanJob> = RedrawGate::new();
    finish_findings(&state, &state.warnings, &gate, None);
    assert_eq!(log.delivered(), Vec::<u64>::new());
}

/// A redraw before any plan has no findings to show; the sink then falls back.
#[test]
fn a_redraw_before_any_plan_carries_no_findings() {
    let sink = WarningSink::new();
    let state = SolidPreviewState::new(sink.clone());
    state.request_redraw(&box_planes(0.6), CAMERA, (16, 16), 0);
    let seen = sink.wait_for("the frame", |frames| !frames.is_empty());
    assert!(seen[0].warnings.is_none());
}
