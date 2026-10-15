//! The greedy episode step and its line search.
//!
//! A step proposes one action. [`EnvCore::step_scaled`] tries it at a short list of scales
//! (`DEFAULT_LINE_SEARCH`: the action itself, then half and a quarter of it) and keeps the
//! first candidate that is a valid stone, passes both gates and scores strictly lower; the
//! rest of the list is not evaluated. The policy therefore only has to supply a direction:
//! how far to go along it is found by the environment, whose acceptance gate is quantised
//! (a score only moves in whole ray counts) and would otherwise reject any move too small
//! to cross a quantum.
//!
//! [`EnvCore::try_candidate`] is the one place a candidate is built, scored and judged. The
//! step, the teacher's oracle probe and the tests all go through it, so they cannot disagree
//! about what an acceptable candidate is.

#![allow(
    clippy::redundant_pub_crate,
    reason = "crate-private helper module: `pub(crate)` documents the intended reach of each item"
)]

use super::{
    action::ContinuousTierAction,
    common::{
        CoreStep, DesignEval, EnvCore, apply_action, evaluate_design, girdle_ratio, shape_drift_pct,
    },
    reward::StepFlags,
    traits::{StepInfo, StepRejection},
};
use indicatrix_cut_core::{Design, ObjectiveComponents};

/// The scales of a line search that can be tried: finite and positive, in the given order.
///
/// A scale that is NaN, infinite, zero or negative is skipped. The list may come out empty;
/// [`EnvCore::step_scaled`] then falls back to the unit scale, the teacher's oracle takes it
/// as "no oracle step".
pub(crate) fn usable_scales(scales: &[f64]) -> impl Iterator<Item = f64> + '_ {
    scales
        .iter()
        .copied()
        .filter(|scale| scale.is_finite() && *scale > 0.0)
}

/// `action` with every delta multiplied by `scale` (the step limit clips afterwards, in
/// `apply_action`).
fn scaled(action: &ContinuousTierAction, scale: f64) -> ContinuousTierAction {
    ContinuousTierAction::new(
        action
            .deltas_deg
            .iter()
            .map(|delta| delta * scale)
            .collect(),
    )
}

/// What trying one candidate produced.
pub(crate) struct CandidateOutcome {
    /// Why the candidate does not replace the current design; `None` when it does (a valid
    /// stone that passes both gates and scores strictly lower).
    pub(crate) rejection: Option<StepRejection>,
    /// The candidate design and its measurements, whenever it solved, closed and scored
    /// finitely (whatever the gates and the acceptance decided).
    pub(crate) scored: Option<(Design, DesignEval)>,
    /// Whether the candidate reached `evaluate_design`: it cost an evaluation even if that
    /// evaluation failed. A candidate `apply_action` refused costs none.
    pub(crate) evaluated: bool,
    /// Whether the candidate keeps the girdle (`true` when it was not judged).
    pub(crate) girdle_gate: bool,
    /// Whether the candidate has no more vanishing or undersized facets than the base design
    /// (`true` when it was not judged).
    pub(crate) manufacturing_gate: bool,
    /// Silhouette drift from the base design, percent (`0.0` when it was not scored).
    pub(crate) shape_penalty: f32,
}

impl CandidateOutcome {
    /// A candidate refused before it was judged.
    const fn refused(rejection: StepRejection, evaluated: bool) -> Self {
        Self {
            rejection: Some(rejection),
            scored: None,
            evaluated,
            girdle_gate: true,
            manufacturing_gate: true,
            shape_penalty: 0.0,
        }
    }

    /// Whether the candidate replaces the current design.
    pub(crate) const fn is_accepted(&self) -> bool {
        self.rejection.is_none()
    }

    /// The candidate's own score, whenever it was scored.
    pub(crate) fn score(&self) -> Option<f32> {
        self.scored.as_ref().map(|(_, eval)| eval.score)
    }

    /// The candidate's optical components, whenever it was scored.
    fn components(&self) -> Option<ObjectiveComponents> {
        self.scored.as_ref().map(|(_, eval)| eval.components)
    }
}

/// The result of trying a list of scales in order.
struct Search {
    /// The candidate the step reports: the accepted one, else the first one tried.
    outcome: CandidateOutcome,
    /// The scale of the accepted candidate; `None` when nothing was accepted.
    accepted_scale: Option<f64>,
    /// Candidates that reached `evaluate_design`.
    evaluations: usize,
    /// The components of the last candidate that scored, `None` when none did.
    last_components: Option<ObjectiveComponents>,
}

impl EnvCore {
    /// Builds the candidate `action * scale` from the current design, scores it and judges it
    /// against the current design, the girdle gate and the manufacturability gate.
    ///
    /// Pure: nothing in the core changes. The candidate is accepted
    /// ([`CandidateOutcome::is_accepted`]) when it is valid, passes both gates and scores
    /// strictly lower than the current design.
    pub(crate) fn try_candidate(
        &self,
        action: &ContinuousTierAction,
        scale: f64,
    ) -> CandidateOutcome {
        let candidate = match apply_action(
            &self.space,
            &self.current_design,
            &scaled(action, scale),
            self.settings.max_step_deg,
        ) {
            Ok(candidate) => candidate,
            Err(reason) => return CandidateOutcome::refused(reason, false),
        };
        let eval = match evaluate_design(
            &candidate,
            &self.gem,
            &self.settings.weights,
            self.settings.fidelity,
            self.preform_volume,
        ) {
            Ok(eval) => eval,
            Err(failure) => return CandidateOutcome::refused(failure.rejection(), true),
        };
        let girdle_gate = self.girdle_gate(&eval);
        let manufacturing_gate = self.manufacturing_gate(&eval);
        let shape_penalty = shape_drift_pct(&self.base_eval, &eval);
        let rejection = if !girdle_gate {
            Some(StepRejection::GirdleViolation)
        } else if !manufacturing_gate {
            Some(StepRejection::ManufacturabilityRegression)
        } else if eval.score < self.current_eval.score {
            None
        } else {
            Some(StepRejection::NotImproved)
        };
        CandidateOutcome {
            rejection,
            scored: Some((candidate, eval)),
            evaluated: true,
            girdle_gate,
            manufacturing_gate,
            shape_penalty,
        }
    }

    /// Tries `scales` in order until one candidate is accepted. An empty list (or one with
    /// no usable scale, see [`usable_scales`]) tries the unit scale.
    fn search(&self, action: &ContinuousTierAction, scales: &[f64]) -> Search {
        let mut usable = usable_scales(scales);
        let first_scale = usable.next().unwrap_or(1.0);
        let first = self.try_candidate(action, first_scale);
        let mut evaluations = usize::from(first.evaluated);
        let mut last_components = first.components();
        if first.is_accepted() {
            return Search {
                outcome: first,
                accepted_scale: Some(first_scale),
                evaluations,
                last_components,
            };
        }
        for scale in usable {
            let outcome = self.try_candidate(action, scale);
            evaluations += usize::from(outcome.evaluated);
            last_components = outcome.components().or(last_components);
            if outcome.is_accepted() {
                return Search {
                    outcome,
                    accepted_scale: Some(scale),
                    evaluations,
                    last_components,
                };
            }
        }
        Search {
            outcome: first,
            accepted_scale: None,
            evaluations,
            last_components,
        }
    }

    /// Applies `action` at the unit scale and advances the episode by one step; exactly
    /// [`Self::step_scaled`] with `scales = [1.0]`.
    pub(crate) fn step(&mut self, action: &ContinuousTierAction) -> CoreStep {
        self.step_scaled(action, &[1.0])
    }

    /// Applies `action` along a line search and advances the episode by ONE step, whatever
    /// the number of scales.
    ///
    /// For each usable scale (see [`usable_scales`]; an empty list tries `1.0`), in order, the
    /// candidate is `action * scale` ([`Self::try_candidate`]): it replaces the current
    /// design only when it is valid, passes both gates and scores strictly lower, and the
    /// first such candidate stops the search. A candidate that fails the girdle or the
    /// manufacturability gate is a closed stone the environment refuses: it earns NO credit
    /// for its score (the reward sees `score_before` for it, so `Δ = 0`) and pays only the
    /// gate penalty, the shape drift and the step cost. Its own score is still reported in
    /// [`StepInfo::candidate_score`], as is that of a valid candidate that did not improve.
    ///
    /// The reward and the [`StepInfo`] describe the accepted candidate or, when none was
    /// accepted, the FIRST candidate tried (the policy's own proposal at the first scale);
    /// `StepInfo::scale` is the accepted scale and `StepInfo::evaluations` counts the
    /// candidates that reached `evaluate_design` (a candidate `apply_action` refused is free,
    /// so a rejected malformed action reports `0`). `last_components` becomes the components
    /// of the last candidate that scored.
    pub(crate) fn step_scaled(
        &mut self,
        action: &ContinuousTierAction,
        scales: &[f64],
    ) -> CoreStep {
        self.step_count += 1;
        let score_before = self.current_eval.score;
        let search = self.search(action, scales);
        self.last_components = search.last_components.or(self.last_components);
        let CandidateOutcome {
            rejection,
            scored,
            girdle_gate,
            manufacturing_gate,
            shape_penalty,
            ..
        } = search.outcome;

        let candidate_score = scored.as_ref().map(|(_, eval)| eval.score);
        // A solved, closed and finitely scored candidate is what `is_valid_solid` means.
        let is_valid_solid = candidate_score.is_some();
        // A candidate that failed a gate earns no credit for its score: Δ = 0.
        let scored_after = match candidate_score {
            Some(score) if girdle_gate && manufacturing_gate => score,
            _ => score_before,
        };
        if rejection.is_none()
            && let Some((design, eval)) = scored
        {
            self.best_score = self.best_score.min(eval.score);
            self.current_design = design;
            self.current_eval = eval;
        }

        let terminated = self.current_eval.score < self.settings.terminal_score;
        let truncated = self.step_count >= self.settings.max_steps;
        let ended_improved =
            (terminated || truncated) && self.current_eval.score < self.base_eval.score;
        let is_safe_bounds = !matches!(
            rejection,
            Some(StepRejection::UnsafeAngle | StepRejection::NoUsableMast)
        );
        let reward = self.reward.compute_step_reward(
            score_before,
            scored_after,
            StepFlags {
                is_valid_solid,
                is_safe_bounds,
                girdle_ok: girdle_gate,
                manufacturability_ok: manufacturing_gate,
                is_new_best: ended_improved,
            },
            shape_penalty,
        );

        let current = &self.current_eval;
        CoreStep {
            reward,
            terminated,
            truncated,
            info: StepInfo {
                current_score: current.score,
                score_delta: score_before - current.score,
                windowing_pct: current.components.windowing_pct,
                extinction_pct: current.components.extinction_pct,
                tilt_brilliance_pct: current.components.tilt_brilliance_pct,
                yield_pct: current.yield_pct.unwrap_or(0.0),
                girdle_ratio: girdle_ratio(
                    self.base_eval.girdle_thickness,
                    current.girdle_thickness,
                ),
                is_closed: is_valid_solid,
                manufacturability_ok: manufacturing_gate,
                episode_step: self.step_count,
                rejection,
                candidate_score,
                scale: search.accepted_scale.map(|scale| scale as f32),
                evaluations: u8::try_from(search.evaluations).unwrap_or(u8::MAX),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{
        retarget_env::RetargetEnv,
        teacher::DEFAULT_LINE_SEARCH,
        test_fixtures::{fixture_env, fixture_env_with},
        traits::GemstoneRlEnv,
    };

    /// Joint moves of every tier in both directions and along the measured descent
    /// direction, at several magnitudes: some of them overshoot at full scale.
    fn ladder(env: &RetargetEnv) -> Vec<ContinuousTierAction> {
        let gradient = env.score_gradient_fd(0.25);
        let tiers = gradient.len();
        let mut actions = Vec::new();
        for magnitude in [0.5, 1.0, 2.0, 4.0] {
            actions.push(ContinuousTierAction::new(
                gradient
                    .iter()
                    .map(|slope| slope.map_or(0.0, |slope| -f64::from(slope).signum() * magnitude))
                    .collect(),
            ));
            actions.push(ContinuousTierAction::new(vec![magnitude; tiers]));
            actions.push(ContinuousTierAction::new(vec![-magnitude; tiers]));
        }
        actions
    }

    #[test]
    fn usable_scales_skips_what_cannot_be_a_scale() {
        let all = [1.0, f64::NAN, 0.5, -0.25, 0.0, f64::INFINITY, 0.25];
        assert_eq!(usable_scales(&all).collect::<Vec<_>>(), [1.0, 0.5, 0.25]);
        assert_eq!(usable_scales(&[]).count(), 0);
        assert_eq!(usable_scales(&[f64::NAN, -1.0]).count(), 0);
    }

    #[test]
    fn step_is_step_scaled_with_the_unit_scale_bit_for_bit() {
        let mut plain = fixture_env();
        let mut scaled_env = fixture_env();
        let tiers = plain.base_design().tiers.len();
        let action = ContinuousTierAction::new(vec![-0.4; tiers]);
        for _ in 0..3 {
            let one = plain.step(&action);
            let other = scaled_env.step_scaled(&action, &[1.0]);
            assert_eq!(one, other);
            // The action reaches the scorer (it is safe and well-formed): one evaluation.
            assert_eq!(one.info.evaluations, 1);
            assert_eq!(one.info.scale.is_some(), one.info.rejection.is_none());
            if let Some(scale) = one.info.scale {
                assert_eq!(scale, 1.0);
            }
        }
        assert_eq!(plain.current_score(), scaled_env.current_score());
        assert_eq!(plain.step_count(), scaled_env.step_count());
        assert_eq!(plain.last_components(), scaled_env.last_components());
    }

    #[test]
    fn an_empty_or_unusable_scale_list_is_the_unit_scale() {
        let mut reference = fixture_env();
        let tiers = reference.base_design().tiers.len();
        let action = ContinuousTierAction::new(vec![-0.4; tiers]);
        let expected = reference.step_scaled(&action, &[1.0]);
        let cases: [&[f64]; 4] = [
            &[],
            &[f64::NAN],
            &[0.0, -1.0, f64::INFINITY],
            &[f64::NAN, 1.0],
        ];
        for scales in cases {
            let mut env = fixture_env();
            assert_eq!(env.step_scaled(&action, scales), expected, "{scales:?}");
        }
    }

    #[test]
    fn a_malformed_action_costs_no_evaluation_at_any_scale() {
        let mut env = fixture_env();
        let step = env.step_scaled(&ContinuousTierAction::new(Vec::new()), &DEFAULT_LINE_SEARCH);
        assert!(matches!(
            step.info.rejection,
            Some(StepRejection::WrongActionLength { .. })
        ));
        assert_eq!(step.info.evaluations, 0);
        assert_eq!(step.info.scale, None);
        assert_eq!(step.info.candidate_score, None);
        assert_eq!(
            step.reward,
            env.config().reward_config.invalid_geometry_penalty
        );
        // One step is counted whatever the number of scales.
        assert_eq!(env.step_count(), 1);
        assert_eq!(step.info.episode_step, 1);
    }

    #[test]
    fn the_line_search_accepts_the_first_improving_scale_and_counts_one_step() {
        let scales = [1.0, 0.5, 0.25];
        // A roomy step limit lets the larger actions of the ladder overshoot.
        let mut line = fixture_env_with(|config| config.max_step_deg = 4.0);
        let mut reference = fixture_env_with(|config| config.max_step_deg = 4.0);
        let base = line.base_score();
        let mut accepted_any = false;
        for action in ladder(&line) {
            // What each scale does on its own, from the base state.
            let singles: Vec<_> = scales
                .iter()
                .map(|&scale| {
                    let _ = reference.reset(0);
                    reference.step(&scaled(&action, scale))
                })
                .collect();
            let first_accepted = singles
                .iter()
                .position(|single| single.info.rejection.is_none());

            let _ = line.reset(0);
            let searched = line.step_scaled(&action, &scales);
            assert_eq!(line.step_count(), 1, "one step whatever the scales");
            assert_eq!(searched.info.episode_step, 1);
            match first_accepted {
                Some(index) => {
                    accepted_any = true;
                    let single = &singles[index];
                    assert_eq!(searched.info.rejection, None);
                    assert_eq!(searched.info.scale, Some(scales[index] as f32));
                    assert_eq!(searched.info.current_score, single.info.current_score);
                    assert_eq!(searched.info.candidate_score, single.info.candidate_score);
                    assert_eq!(searched.reward, single.reward);
                    // The state is the accepted candidate.
                    assert_eq!(searched.observation, single.observation);
                    assert!(line.current_score() < base);
                    let tried: u8 = singles[..=index]
                        .iter()
                        .map(|single| single.info.evaluations)
                        .sum();
                    assert_eq!(searched.info.evaluations, tried);
                }
                None => {
                    // Nothing improved: the report is the first candidate's, the state is
                    // untouched and every scale that could be evaluated was.
                    let first = &singles[0];
                    assert_eq!(searched.info.scale, None);
                    assert_eq!(searched.info.rejection, first.info.rejection);
                    assert_eq!(searched.info.candidate_score, first.info.candidate_score);
                    assert_eq!(searched.reward, first.reward);
                    assert_eq!(searched.info.current_score, base);
                    assert_eq!(line.current_score(), base);
                    let tried: u8 = singles.iter().map(|single| single.info.evaluations).sum();
                    assert_eq!(searched.info.evaluations, tried);
                }
            }
        }
        assert!(accepted_any, "no action of the ladder was accepted");
    }
}
