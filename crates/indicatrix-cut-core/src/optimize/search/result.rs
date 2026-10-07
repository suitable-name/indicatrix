//! The last step of [`optimize_design_with`](super::optimize_design_with): measuring the
//! ending point(s) at full fidelity and turning them into ranked candidates
//! ([`build_result`]). Split from the parent module so the file stays readable; the
//! functions are unchanged.

use super::{AngleChange, Baseline, OptimizeOutcome, SearchHooks, SearchRunSummary, SearchStage};
use crate::{
    design::Design,
    optimize::{
        candidate::{SearchContext, measure_with_tone, shape_penalty_for_planes, yield_loss_pct},
        objective::{ObjectiveComponents, ObjectiveFidelity, ToneGoal, to_gpu_planes},
        options::{MastChange, OptimizeCandidate, OptimizeResult},
    },
};
use indicatrix::{
    color::metrics::FaceUpTone,
    geometry::{meet_solver::MeetConstraint, stone_metrics::measure_solid},
};

#[cfg(doc)]
use super::{baseline_report, optimize_design, optimize_design_with};

/// One design measured at [`ObjectiveFidelity::Full`].
struct ScoredState {
    after: ObjectiveComponents,
    score: f32,
    yield_loss_pct: f32,
    tone: Option<FaceUpTone>,
}

/// Re-solves `state`, scores it at [`ObjectiveFidelity::Full`] and blends the yield
/// in -- the measurement [`build_result`] gates every candidate on. `None` if `state`
/// no longer solves (only a state that was never accepted by the search could not).
///
/// Reports [`SearchStage::FinalFull`] through `hooks` (evaluations `evaluations`,
/// unchanged by this call -- same reasoning as [`baseline_report`]'s own report)
/// before running the scoring -- this call is as expensive as the baseline's;
/// without this report, a caller's progress ticker would stay stuck on its last
/// coordinate/polish reading through the whole final scoring, reading as the run
/// having already finished when it had not.
fn score_state(
    state: &Design,
    ctx: &SearchContext,
    hooks: &SearchHooks<'_>,
    evaluations: usize,
) -> Option<ScoredState> {
    hooks.report(evaluations, SearchStage::FinalFull);
    let solved = state.solve().ok()?;
    let planes = state.planes_from_solved(&solved);
    let (after, tone, tone_loss) = measure_with_tone(
        &to_gpu_planes(&planes),
        ctx.material,
        ctx.weights,
        ObjectiveFidelity::Full,
        ctx.lighting,
    );
    let metrics = measure_solid(&planes);
    let loss = yield_loss_pct(state, metrics.as_ref());
    let mut score = ctx.weights.score_with_tone(&after, loss, tone_loss);
    if ctx.shape_target.is_some() {
        score += shape_penalty_for_planes(ctx.shape_target, metrics.as_ref(), &planes);
    }
    Some(ScoredState {
        after,
        score,
        yield_loss_pct: loss,
        tone,
    })
}

/// Every tier whose angle differs between `original` and `state`, except the tiers that
/// follow a relation: their angle is the result of the tiers they read, so applying the
/// change of those moves them, and listing them too would ask for a direct change of an
/// angle the relation owns.
fn angle_changes(original: &Design, state: &Design) -> Vec<AngleChange> {
    original
        .tiers
        .iter()
        .zip(&state.tiers)
        .enumerate()
        .filter_map(|(index, (before_tier, after_tier))| {
            (before_tier.angle_deg != after_tier.angle_deg && !original.is_tier_driven(index))
                .then_some(AngleChange {
                    index,
                    from_deg: before_tier.angle_deg,
                    to_deg: after_tier.angle_deg,
                })
        })
        .collect()
}

/// Every `ScaleReference` tier whose mast differs between `original` and `state`.
fn mast_changes(original: &Design, state: &Design) -> Vec<MastChange> {
    original
        .tiers
        .iter()
        .zip(&state.tiers)
        .enumerate()
        .filter_map(|(index, (before_tier, after_tier))| {
            match (&before_tier.constraint, &after_tier.constraint) {
                (MeetConstraint::ScaleReference(from), MeetConstraint::ScaleReference(to))
                    if from.to_bits() != to.to_bits() =>
                {
                    Some(MastChange {
                        index,
                        from_mast: *from,
                        to_mast: *to,
                    })
                }
                _ => None,
            }
        })
        .collect()
}

/// `state` as a ranked alternative to `original`, or `None` when it changes nothing.
fn candidate_from(
    original: &Design,
    state: &Design,
    scored: &ScoredState,
) -> Option<OptimizeCandidate> {
    let changes = angle_changes(original, state);
    let masts = mast_changes(original, state);
    if changes.is_empty() && masts.is_empty() {
        return None;
    }
    Some(OptimizeCandidate {
        changes,
        mast_changes: masts,
        after: scored.after,
        score: scored.score,
        yield_loss_pct: scored.yield_loss_pct,
        tone: scored.tone,
    })
}

/// The tone guarantee of a tone-preset run: a candidate whose tone moved against the
/// goal versus the start (paler under `Deeper`, darker under `Lighter`) never qualifies,
/// whatever its weighted score. No goal (tone weight 0) or an unmeasured tone passes.
fn tone_gate_passes(
    goal: Option<ToneGoal>,
    before: Option<FaceUpTone>,
    after: Option<FaceUpTone>,
) -> bool {
    match (goal, before, after) {
        (Some(goal), Some(before), Some(after)) => goal.not_worse(&before, &after),
        _ => true,
    }
}

/// [`optimize_design_with`]'s "measure the ending point" half: re-solves `current`
/// (see [`optimize_design`]'s own `# Panics` section for why the `.expect()` inside
/// this is safe) and every state in `rivals`, scores each at
/// [`ObjectiveFidelity::Full`] (see [`score_state`]), and diffs its tier angles and
/// masts against `design`'s original ones to build the [`OptimizeCandidate`] list.
///
/// The end point `current` is always scored. The `rivals` (the pool's other states)
/// are scored one by one, best fast score first, and not at all once the search was
/// cancelled; a cancel that arrives between two scorings drops the rest and marks the
/// outcome cancelled. A state becomes a candidate when it changes something and its
/// `Full` score is no worse than the starting design's -- the search accepts
/// candidates on the `Fast` objective and the `Full` measurement here is the gate.
/// Candidates are ranked by that `Full` score (ties keep the end point first), and
/// the outcome describes the best one. When no state qualifies the outcome proposes no
/// change: the design is reported as it is, with the evaluations that were spent.
pub(super) fn build_result(
    design: &Design,
    current: &Design,
    rivals: &[Design],
    ctx: &SearchContext,
    baseline: &Baseline,
    hooks: &SearchHooks<'_>,
    summary: &SearchRunSummary,
) -> OptimizeResult {
    let end_point = score_state(current, ctx, hooks, summary.evaluations)
        .expect("current was only ever advanced via evaluate_candidate-accepted, solvable states");
    let mut scored: Vec<(&Design, ScoredState)> = Vec::with_capacity(1 + rivals.len());
    scored.push((current, end_point));
    let mut cancelled = summary.cancelled;
    if !cancelled {
        for rival in rivals {
            if hooks.is_cancelled() {
                cancelled = true;
                break;
            }
            if let Some(state) = score_state(rival, ctx, hooks, summary.evaluations) {
                scored.push((rival, state));
            }
        }
    }

    let tone_gate = (ctx.weights.tone_weight > 0.0).then_some(ctx.weights.tone_goal);
    let mut candidates: Vec<OptimizeCandidate> = scored
        .into_iter()
        .filter(|(_, state)| state.score <= baseline.before_score)
        .filter(|(_, state)| tone_gate_passes(tone_gate, baseline.tone_before, state.tone))
        .filter_map(|(state_design, state)| candidate_from(design, state_design, &state))
        .collect();
    candidates.sort_by(|a, b| a.score.total_cmp(&b.score));

    let tone_goal = (ctx.weights.tone_weight > 0.0).then_some(ctx.weights.tone_goal);
    let Some(best) = candidates.first() else {
        let mut unchanged = OptimizeResult::unchanged(
            OptimizeOutcome {
                before: baseline.before,
                before_score: baseline.before_score,
                before_yield_loss_pct: baseline.before_yield_loss_pct,
                after: baseline.before,
                after_score: baseline.before_score,
                after_yield_loss_pct: baseline.before_yield_loss_pct,
                evaluations: summary.evaluations,
                changes: Vec::new(),
                cancelled,
                polish_evaluations: summary.polish_evaluations,
                polish_improvement: 0.0,
            },
            ctx.lighting,
            baseline.tone_before,
            tone_goal,
        );
        unchanged.starts_run = summary.starts_run;
        unchanged.best_start = summary.best_start;
        return unchanged;
    };
    let outcome = OptimizeOutcome {
        before: baseline.before,
        before_score: baseline.before_score,
        before_yield_loss_pct: baseline.before_yield_loss_pct,
        after: best.after,
        after_score: best.score,
        after_yield_loss_pct: best.yield_loss_pct,
        evaluations: summary.evaluations,
        changes: best.changes.clone(),
        cancelled,
        polish_evaluations: summary.polish_evaluations,
        polish_improvement: summary.polish_improvement,
    };
    let best_mast_changes = best.mast_changes.clone();
    OptimizeResult {
        outcome,
        mast_changes: best_mast_changes,
        candidates,
        tone_before: baseline.tone_before,
        tone_goal,
        lighting: ctx.lighting,
        starts_run: summary.starts_run,
        best_start: summary.best_start,
    }
}
