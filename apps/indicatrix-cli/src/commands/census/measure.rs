//! The measurement of one design: its rows, one for each target material and mode.
//!
//! A shift row runs the Retarget dialog's own check (`run_check`, the gate under
//! `PreservationPolicy::RETARGET`) and reads its verdict. The raw shape deltas are measured on
//! the check's candidate when it is valid. When the check refuses, the census reproduces the
//! check's anchored seed with `anchored_candidate_for`, projects it with the report function
//! and measures the projection's result, or its last refused probe, which it also gives to the
//! branch diagnosis. That seed is the plain anchored one: a refit attempt the check may have
//! preferred is not reproduced (the diagnosis is of the refusal of the seed, which is the
//! failure the plan asks about). An optimize row runs the dialog's search and measures its best
//! option. Nothing here writes to the design or to the library.

use super::{
    load::{Subject, design_text},
    rows::{Cell, Row},
};
use crate::{
    args::{CensusArgs, Mode},
    materials::{Catalogue, Scoring, current_gem, resolve_scoring},
    stone::analyze as analyze_stone,
};
use indicatrix::{
    geometry::meet_solver::{MeetConstraint, SolveControl},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    CANONICAL_LIGHTING_PRESET, Design, Edit, History, MaterialSelection, ObjectivePreset,
    OptimizeOptions,
    meet_preservation::{
        BranchDiagnosis, MeetPreservationBaseline, PreservationFacts, PreservationPolicy,
        SearchDiagnostics,
    },
    optimize::project_preserved_design_report,
};
use indicatrix_editor::{
    EditorSession,
    retarget::{
        RetargetPlan,
        anchors::anchored_candidate_for,
        apply_with_anchors, build_plan,
        check::{AnalysisResult, CheckInputs, run_check},
        metrics::{MetricColumn, RetargetMetrics},
        plan::tier_display_names,
        search::{SearchInputs, SearchSettings, run_search},
        validity::{
            InvalidReason, RetargetStrategy, RetargetValidity, StoneAnalysis, ValidityStatus,
            analyze,
        },
    },
};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

/// A target whose index is this close to the design's own gets no row.
const RI_EPS: f64 = 0.005;

/// What the census was asked for.
pub(super) struct Context<'a> {
    pub args: &'a CensusArgs,
    pub catalogue: &'a Catalogue,
}

/// The rows of one design and the stones to write for its Valid rows.
#[derive(Default)]
pub(super) struct Output {
    pub rows: Vec<Row>,
    /// `(file name, text)` of every retargeted stone, when `--out-dir` is given.
    pub files: Vec<(String, String)>,
}

/// Everything one row of one design needs.
struct Job<'a> {
    ctx: &'a Context<'a>,
    subject: &'a Subject,
    design: &'a Design,
    stone: &'a StoneAnalysis,
    analysis: &'a Arc<AnalysisResult>,
    baseline: &'a Arc<MeetPreservationBaseline>,
    plan: &'a RetargetPlan,
    scoring: &'a Scoring,
    current: Option<&'a GemMaterial>,
    capture_ms: f64,
}

fn subject_row(subject: &Subject) -> Row {
    let mut row = Row::new();
    row.set("source", Cell::text(subject.source));
    row.set("key", Cell::text(subject.key.as_str()));
    row.set("title", Cell::text(subject.title.as_str()));
    if let Ok(design) = &subject.design {
        row.set(
            "gemcad_version",
            Cell::text(design.meta.gemcad_version.as_str()),
        );
    }
    if let Some(decimals) = subject.mast_decimals {
        row.set("mast_decimals", Cell::int(decimals));
    }
    row
}

/// The row of a design whose measurement panicked: the panic text is its `capture`.
pub(super) fn panicked(subject: &Subject, payload: &(dyn std::any::Any + Send)) -> Output {
    let why = payload
        .downcast_ref::<&str>()
        .map(ToString::to_string)
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string());
    let mut row = subject_row(subject);
    row.set("capture", Cell::text(format!("panic: {why}")));
    row.set("status", Cell::text("unchecked"));
    Output {
        rows: vec![row],
        files: Vec::new(),
    }
}

fn fill_counts(row: &mut Row, design: &Design) {
    let count = |wanted: fn(&MeetConstraint) -> bool| {
        design
            .tiers
            .iter()
            .filter(|tier| wanted(&tier.constraint))
            .count()
    };
    row.set("tiers", Cell::int(design.tiers.len()));
    row.set(
        "pinned",
        Cell::int(count(|c| matches!(c, MeetConstraint::ScaleReference(_)))),
    );
    row.set(
        "meet_derived",
        Cell::int(count(|c| {
            matches!(
                c,
                MeetConstraint::MeetExisting | MeetConstraint::MeetNamed(_)
            )
        })),
    );
    row.set(
        "named",
        Cell::int(count(|c| matches!(c, MeetConstraint::MeetNamed(_)))),
    );
    row.set("relations", Cell::int(design.tier_relations.len()));
    row.set("targets", Cell::int(design.tier_targets.len()));
    row.set("concave_tiers", Cell::int(design.concave_tiers.len()));
    row.set(
        "placements",
        Cell::int(
            design
                .concave_tiers
                .iter()
                .map(|tier| tier.indices.len())
                .sum(),
        ),
    );
    row.set("gear", Cell::Int(u64::from(design.meta.gear_teeth_abs())));
    row.set(
        "symmetry",
        Cell::text(format!(
            "{}{}",
            design.meta.symmetry_order,
            if design.meta.mirror { "m" } else { "" }
        )),
    );
}

/// The captured network's shape: faces, mean valence `2E/V`, the split ratio `V/(F-2)` and
/// the largest slack in millionths of the stone (see [`super::rows::COLUMNS`]).
fn set_network_shape(row: &mut Row, baseline: &MeetPreservationBaseline) {
    let structure = baseline.structure();
    let count = |n: usize| f64::from(u32::try_from(n).unwrap_or(u32::MAX));
    let (vertices, edges, faces) = (
        count(structure.junctions),
        count(structure.edges),
        count(structure.faces),
    );
    row.set("faces", Cell::int(structure.faces));
    if vertices > 0.0 {
        row.set("valence", Cell::num(2.0 * edges / vertices));
    }
    if faces > 2.0 {
        row.set("split_ratio", Cell::num(vertices / (faces - 2.0)));
    }
    row.set("max_slack_ppm", Cell::num(baseline.max_slack() * 1e6));
}

fn crown_label(args: &CensusArgs) -> String {
    let crown = args.crown;
    if crown.scale_by_ratio {
        "ratio".to_string()
    } else if crown.follow_pavilion {
        "follow".to_string()
    } else if crown.fraction.to_bits() == 0.0_f64.to_bits() {
        "fixed".to_string()
    } else {
        format!("fraction={}", crown.fraction)
    }
}

/// The rows of `subject`: one row when it cannot be measured at all, else one for each target
/// that differs from the design's own index and each mode.
pub(super) fn process(subject: &Subject, ctx: &Context<'_>) -> Output {
    let mut out = Output::default();
    let mut base = subject_row(subject);
    let design = match &subject.design {
        Ok(design) => design,
        Err(why) => {
            base.set("capture", Cell::text(why.as_str()));
            out.rows.push(base);
            return out;
        }
    };
    fill_counts(&mut base, design);
    let started = Instant::now();
    let analysis: Arc<AnalysisResult> = Arc::new(analyze(design, true));
    let capture_ms = started.elapsed().as_secs_f64() * 1000.0;
    let unchecked = |mut row: Row, why: String| {
        row.set("capture", Cell::text(why));
        row.set("status", Cell::text("unchecked"));
        row
    };
    let stone = match analysis.as_ref() {
        Ok(stone) => stone,
        Err(reason) => {
            out.rows.push(unchecked(base, reason.message()));
            return out;
        }
    };
    let Some(baseline) = stone.preservation.clone() else {
        out.rows
            .push(unchecked(base, "no meet baseline was captured".to_string()));
        return out;
    };
    base.set("capture", Cell::text("ok"));
    base.set("planes", Cell::int(stone.planes.len()));
    let structure = baseline.structure();
    base.set("junctions", Cell::int(structure.junctions));
    base.set("edges", Cell::int(structure.edges));
    base.set("live_facets", Cell::int(structure.live_facets));
    base.set(
        "reach_fallbacks_upper_bound",
        Cell::int(structure.reach_fallbacks_possible),
    );
    set_network_shape(&mut base, &baseline);
    let mut produced = false;
    for target in &ctx.args.targets {
        let Ok(scoring) = resolve_scoring(design, ctx.catalogue, Some(target)) else {
            continue;
        };
        let current = current_gem(design, ctx.catalogue);
        let plan = build_plan(
            design,
            &scoring.resolved,
            ctx.args.crown,
            ctx.catalogue.custom(),
        );
        if (plan.n_to - plan.n_from).abs() <= RI_EPS {
            continue;
        }
        let job = Job {
            ctx,
            subject,
            design,
            stone,
            analysis: &analysis,
            baseline: &baseline,
            plan: &plan,
            scoring: &scoring,
            current: current.as_ref(),
            capture_ms,
        };
        for mode in &ctx.args.modes {
            produced = true;
            let mut row = base.clone();
            row.set("ri_from", Cell::num(plan.n_from));
            row.set("target", Cell::text(scoring.label.as_str()));
            row.set("ri_to", Cell::num(plan.n_to));
            row.set("crown_mode", Cell::text(crown_label(ctx.args)));
            let (word, file) = match mode {
                Mode::Shift => ("shift", shift_row(&job, &mut row)),
                Mode::Optimize => ("optimize", optimize_row(&job, &mut row)),
            };
            row.set("mode", Cell::text(word));
            out.rows.push(row);
            if let Some(edit) = file
                && let Some(written) = stone_file(&job, edit, word)
            {
                out.files.push(written);
            }
        }
    }
    if !produced {
        base.set("status", Cell::text("no_target_differs"));
        out.rows.push(base);
    }
    out
}

const fn status_word(status: ValidityStatus) -> &'static str {
    match status {
        ValidityStatus::Valid => "valid",
        ValidityStatus::Invalid => "invalid",
        ValidityStatus::Unchecked => "unchecked",
    }
}

const fn strategy_word(strategy: RetargetStrategy) -> &'static str {
    match strategy {
        RetargetStrategy::AnglesOnly => "angles_only",
        RetargetStrategy::Anchored => "anchored",
        RetargetStrategy::AnchoredRefit => "anchored_refit",
        RetargetStrategy::Optimized => "optimized",
    }
}

const fn reason_kind(reason: &InvalidReason) -> &'static str {
    match reason {
        InvalidReason::Preservation(_) => "preservation",
        InvalidReason::DoesNotSolve(_) => "does_not_solve",
        InvalidReason::NotClosed => "not_closed",
        InvalidReason::GirdleGone => "girdle_gone",
        InvalidReason::GirdleTooThin { .. } => "girdle_too_thin",
        InvalidReason::GirdleThinAtCorners { .. } => "girdle_thin_at_corners",
        InvalidReason::FlatOffGirdle { .. } => "flat_off_girdle",
        InvalidReason::TierLost { .. } => "tier_lost",
        InvalidReason::TierPartlyLost { .. } => "tier_partly_lost",
        InvalidReason::FacetsTooSmall { .. } => "facets_too_small",
        InvalidReason::RelationFails(_) => "relation_fails",
    }
}

fn reason_texts(validity: &RetargetValidity) -> Vec<String> {
    validity
        .reasons
        .iter()
        .map(|reason| {
            format!(
                "{}:{}",
                reason_kind(reason),
                reason.message().replace('\n', " ")
            )
        })
        .collect()
}

fn brilliance(column: Option<MetricColumn>) -> Cell {
    Cell::opt(column.map(|column| f64::from(column.brilliance_pct)))
}

fn set_verdict(row: &mut Row, validity: &RetargetValidity, metrics: &RetargetMetrics) {
    row.set("status", Cell::text(status_word(validity.status)));
    row.set("strategy", Cell::text(strategy_word(validity.strategy)));
    row.set(
        "girdle_step",
        Cell::opt(validity.figures.girdle_thickened_percent),
    );
    row.set("score_before", brilliance(metrics.current_in_current));
    row.set(
        "score_unchanged_in_target",
        brilliance(metrics.current_in_target),
    );
    row.set("score_after", brilliance(metrics.retargeted_in_target));
}

fn set_facts(row: &mut Row, facts: &PreservationFacts) {
    row.set("preserved_meets", Cell::int(facts.original_meets));
    row.set("added_meets", Cell::int(facts.added_meets));
    row.set(
        "max_residual_norm",
        Cell::num(facts.max_normalized_residual),
    );
}

fn set_deltas(row: &mut Row, baseline: &MeetPreservationBaseline, candidate: Option<&Design>) {
    let Some(candidate) = candidate else {
        row.set("census_status", Cell::text("no_candidate"));
        return;
    };
    match baseline.measure_deltas(candidate) {
        Err(violation) => row.set("census_status", Cell::text(violation.kind())),
        Ok(deltas) => {
            row.set("census_status", Cell::text("valid"));
            row.set("d_aspect", Cell::num(deltas.d_aspect));
            row.set("d_table_pts", Cell::opt(deltas.d_table_pts));
            row.set("d_crown", Cell::opt(deltas.d_crown));
            row.set("d_pavilion", Cell::opt(deltas.d_pavilion));
            row.set("d_girdle", Cell::opt(deltas.d_girdle));
            row.set("junction_disp", Cell::num(deltas.junction_disp));
            row.set("travel_width", Cell::num(deltas.travel_width));
            row.set("travel_scale", Cell::num(deltas.travel_scale));
            row.set("excess_depth", Cell::opt(deltas.excess_depth()));
            row.set("excess_girdle", Cell::opt(deltas.excess_girdle()));
            row.set("excess_junction", Cell::num(deltas.excess_junction()));
        }
    }
}

fn set_branch(
    row: &mut Row,
    design: &Design,
    diagnosis: Option<&BranchDiagnosis>,
    reasons: &mut Vec<String>,
) {
    match diagnosis {
        None => row.set("branch", Cell::text("none")),
        Some(BranchDiagnosis::BranchMismatch {
            tiers,
            ideal_spread,
        }) => {
            let display_names = tier_display_names(design);
            let named: Vec<String> = tiers
                .iter()
                .map(|&index| {
                    display_names
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| index.to_string())
                })
                .collect();
            row.set("branch", Cell::text("branch_mismatch"));
            row.set("branch_tiers", Cell::text(named.join(";")));
            row.set("ideal_spread", Cell::num(*ideal_spread));
        }
        Some(BranchDiagnosis::Infeasible { reason }) => {
            row.set("branch", Cell::text("infeasible"));
            reasons.push(format!("last_probe:{}", reason.kind()));
        }
        Some(BranchDiagnosis::Unresolved { reason }) => {
            row.set("branch", Cell::text("unresolved"));
            reasons.push(format!("unresolved:{reason}"));
        }
    }
}

const fn ms(ns: u64) -> f64 {
    Duration::from_nanos(ns).as_secs_f64() * 1000.0
}

/// `now` minus `before`, phase by phase.
fn since(now: &SearchDiagnostics, before: &SearchDiagnostics) -> SearchDiagnostics {
    let mut spent = now.clone();
    for (slot, was) in spent.phase_ns.iter_mut().zip(&before.phase_ns) {
        *slot = slot.saturating_sub(*was);
    }
    spent.iterations = now.iterations.saturating_sub(before.iterations);
    spent.rejected = now.rejected.saturating_sub(before.rejected);
    spent
}

fn set_timings(row: &mut Row, capture_ms: f64, spent: &SearchDiagnostics) {
    row.set("t_capture_ms", Cell::num(capture_ms));
    row.set("t_projection_ms", Cell::num(ms(spent.phase_ns[1])));
    row.set("t_solve_ms", Cell::num(ms(spent.phase_ns[2])));
    row.set("t_validate_ms", Cell::num(ms(spent.phase_ns[3])));
    row.set("t_fast_ms", Cell::num(ms(spent.phase_ns[4])));
    row.set("t_full_ms", Cell::num(ms(spent.phase_ns[5])));
    row.set(
        "iterations",
        Cell::int(usize::try_from(spent.iterations).unwrap_or(usize::MAX)),
    );
    row.set(
        "rejected",
        Cell::int(usize::try_from(spent.rejected).unwrap_or(usize::MAX)),
    );
}

/// The options the dialog's attempt projects its anchored seed with: the editor's own
/// function, so the census cannot drift from the dialog.
fn projection_options(
    original: &Design,
    stone: &StoneAnalysis,
    requested: &Design,
    moves: &[(usize, f64)],
) -> OptimizeOptions {
    indicatrix_editor::retarget::check::retarget_projection_options(
        original, stone, requested, moves,
    )
}

/// For a refused shift: the design to measure and, when the projection was refused, the
/// diagnosis of its last probe.
fn refused_candidate(job: &Job<'_>) -> (Option<Design>, Option<BranchDiagnosis>) {
    let moves = job.plan.moving_angles();
    let Ok((seed, _)) = anchored_candidate_for(job.design, &moves, job.stone) else {
        return (None, Some(unattempted("no anchored seed")));
    };
    let options = projection_options(job.design, job.stone, &seed, &moves);
    let control = SolveControl::default();
    let report = project_preserved_design_report(job.design, &seed, &options, &control);
    match report.result {
        Ok(design) => (Some(design), None),
        Err(_) => report.last_probe.map_or_else(
            || (None, Some(unattempted("no refused probe"))),
            |probe| {
                let diagnosis = job.baseline.diagnose_refusal(&probe, &control);
                (Some(probe), Some(diagnosis))
            },
        ),
    }
}

/// The diagnosis of a refused row the census could not attempt, so the branch column never
/// reads `none` for a row that was not examined.
fn unattempted(reason: &str) -> BranchDiagnosis {
    BranchDiagnosis::Unresolved {
        reason: reason.to_owned(),
    }
}

/// Fills a shift row; the edit of a Valid row is returned for `--out-dir`.
fn shift_row(job: &Job<'_>, row: &mut Row) -> Option<Edit> {
    let before = job.baseline.diagnostics();
    if job.plan.is_empty() {
        row.set("status", Cell::text("no_move"));
        row.set("reasons", Cell::text("none of the facet angles can move"));
        return None;
    }
    let inputs = CheckInputs {
        girdle: None,
        design: job.design,
        plan: job.plan,
        current_gem: job.current,
        lighting: CANONICAL_LIGHTING_PRESET,
    };
    let Some((_, check)) = run_check(&inputs, Some(job.analysis.clone()), &|| false) else {
        row.set("status", Cell::text("unchecked"));
        return None;
    };
    // The dialog's own cost; the measurement below solves and validates on the same baseline.
    let spent = since(&job.baseline.diagnostics(), &before);
    let validity = &check.validity;
    let valid = validity.status == ValidityStatus::Valid;
    set_verdict(row, validity, &check.metrics);
    let mut reasons = reason_texts(validity);
    let (candidate, diagnosis) = if valid {
        (check.candidate.clone(), None)
    } else {
        refused_candidate(job)
    };
    if valid {
        let facts = candidate
            .as_ref()
            .and_then(|c| job.baseline.validate(c, PreservationPolicy::RETARGET).ok());
        if let Some(facts) = facts {
            set_facts(row, &facts);
        }
    }
    set_deltas(row, job.baseline, candidate.as_ref());
    set_branch(row, job.design, diagnosis.as_ref(), &mut reasons);
    row.set("reasons", Cell::text(reasons.join(";")));
    set_timings(row, job.capture_ms, &spent);
    if valid && job.ctx.args.out_dir.is_some() {
        check.validated_edit(job.design).ok()
    } else {
        None
    }
}

/// Fills an optimize row; the edit of a Valid best option is returned for `--out-dir`.
fn optimize_row(job: &Job<'_>, row: &mut Row) -> Option<Edit> {
    let defaults = SearchSettings::default();
    let settings = SearchSettings {
        preset: ObjectivePreset::Balanced,
        range_deg: defaults.range_deg,
        evaluations: job.ctx.args.budget,
        seed: 0,
        keep: defaults.keep,
        keep_look: true,
        girdle: None,
    };
    let inputs = SearchInputs {
        design: job.design,
        plan: job.plan,
        current_gem: job.current,
        lighting: CANONICAL_LIGHTING_PRESET,
        settings: &settings,
    };
    let report = match run_search(&inputs, &AtomicBool::new(false), &|_, _| {}) {
        Ok(report) => report,
        Err(error) => {
            row.set("status", Cell::text("error"));
            row.set("reasons", Cell::text(format!("search:{error}")));
            return None;
        }
    };
    let best = report.best();
    let validity = best.map_or(&report.shift_validity, |option| &option.validity);
    let metrics = best.map_or_else(RetargetMetrics::default, |option| option.metrics);
    set_verdict(row, validity, &metrics);
    if let Some(facts) = best.and_then(|option| option.preservation) {
        set_facts(row, &facts);
    }
    set_deltas(row, job.baseline, best.map(|option| &option.design));
    row.set("branch", Cell::text("none"));
    let mut reasons = reason_texts(validity);
    if best.is_none() {
        reasons.push("search:no valid option".to_string());
    }
    row.set("reasons", Cell::text(reasons.join(";")));
    let spent = report
        .baseline
        .as_ref()
        .map(|baseline| baseline.diagnostics())
        .unwrap_or_default();
    set_timings(row, job.capture_ms, &spent);
    let option = best?;
    if validity.status == ValidityStatus::Valid && job.ctx.args.out_dir.is_some() {
        let proposal = job.plan.with_angles(job.design, &option.angles).proposal();
        apply_with_anchors(job.design, &proposal, &option.anchors).ok()
    } else {
        None
    }
}

/// `edit` and the target material applied to a copy of `design` as one editor edit, checked to
/// be a closed stone (the `retarget` command's own application of a decision).
fn apply_edit(
    design: &Design,
    edit: Edit,
    selection: &MaterialSelection,
) -> Result<Design, String> {
    let edit = indicatrix_editor::optimize_view::edits_for_relation_session(design, edit);
    let mut edits = match edit {
        Edit::Batch(edits) => edits,
        single => vec![single],
    };
    if *selection != design.material {
        edits.push(Edit::SetMaterial {
            material: selection.clone(),
        });
    }
    let mut session = EditorSession::with_history(design.clone(), History::new());
    session
        .try_apply(Edit::Batch(edits))
        .map_err(|error| format!("the change could not be applied: {error}"))?;
    if let Some(problem) = analyze_stone(&session.design).problem(&session.design) {
        return Err(format!(
            "the retargeted design is not a usable stone: {problem}"
        ));
    }
    Ok(session.design)
}

fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The file name of the stone of a Valid row.
pub(super) fn stone_file_name(key: &str, target: &str, mode: &str) -> String {
    format!("{}__{}__{mode}.isdd", sanitize(key), sanitize(target))
}

fn stone_file(job: &Job<'_>, edit: Edit, mode: &str) -> Option<(String, String)> {
    let after = apply_edit(job.design, edit, &job.scoring.selection).ok()?;
    let text = design_text(job.subject, &after, job.ctx.catalogue).ok()?;
    Some((
        stone_file_name(&job.subject.key, &job.scoring.label, mode),
        text,
    ))
}
