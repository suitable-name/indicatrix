//! The finite-difference teacher: what the simulator can say about which way to move.
//!
//! The simulator is deterministic and cheap, so instead of inferring a direction from the
//! noisy scalar reward of a proposal, the trainer asks it directly. [`EnvCore::teacher_label`]
//! probes every free tier by one candidate evaluation (`+h` degrees, else `-h`) and reports
//! how each of the four score components moves per degree of that tier
//! ([`TierSensitivity`]). A model that predicts those sensitivities can compose the direction
//! for ANY weighting of the objective from the weight fractions in the observation, so one
//! model serves every preset.
//!
//! # Components
//!
//! Everywhere (labels, model heads, loss) the four components come in this order, all as
//! LOSSES (lower is better), in percentage points:
//!
//! | index | constant | figure |
//! |---|---|---|
//! | 0 | [`COMPONENT_WINDOWING`] | windowing percentage |
//! | 1 | [`COMPONENT_EXTINCTION`] | extinction percentage |
//! | 2 | [`COMPONENT_BRILLIANCE_LOSS`] | `100 - tilt_brilliance_pct` |
//! | 3 | [`COMPONENT_YIELD_LOSS`] | `max(100 - yield_pct, 0)` (NaN when the yield is not measurable) |
//!
//! A slope is the change of such a figure per degree of one tier, in percentage points per
//! degree; the score slope is in score points per degree. A negative slope means "increasing
//! this tier's signed angle lowers the figure".
//!
//! # From slopes to an action
//!
//! [`direction_from_slopes`] turns slopes into the pre-squash direction the policy mean uses:
//! the steepest tier gets `-MEAN_BOUND` times the sign of its slope and the others are
//! proportional. [`action_from_direction`] squashes it into degrees (`tanh(d) * step`), so the
//! steepest tier moves 95 % of the step limit. The oracle of the label and the model's
//! deterministic policy use the same two formulas.
//!
//! # Probes and oracle step, apart
//!
//! A label has two parts of very different shape. The PROBES are independent of each other:
//! one task per free tier ([`RetargetEnv::probe_tier`](super::RetargetEnv::probe_tier), one or
//! two candidate evaluations, a [`TierProbe`]). The ORACLE step needs all of them: it follows
//! the steepest descent of the score slopes and tries it at the line-search scales
//! ([`RetargetEnv::oracle_outcome`](super::RetargetEnv::oracle_outcome), an [`OracleOutcome`]
//! of up to `scales.len()` evaluations of its own). [`assemble_label`] joins the pieces into a
//! [`TeacherLabel`], and `teacher_label` is exactly that composition run serially.
//!
//! The split exists for scheduling. A 24-tier design costs about 28 evaluations to label and a
//! 4-tier design about 5, so a trainer that hands whole labels to threads in chunks leaves most
//! of them waiting for the slowest chunk. With the pieces public, it can make every probe of
//! every labelled slot one task, let the threads pull tasks from a shared counter (dynamic
//! scheduling at probe granularity), and run the oracle steps afterwards. All the pieces are
//! pure functions of the environment's state, so the result does not depend on who ran which
//! task.

use super::{
    action::ContinuousTierAction,
    common::{DesignEval, EnvCore, apply_action, evaluate_design, yield_loss_pct},
    stepping::usable_scales,
};

/// Number of score components a sensitivity vector holds.
pub const N_COMPONENTS: usize = 4;
/// Index of the windowing percentage in a sensitivity vector.
pub const COMPONENT_WINDOWING: usize = 0;
/// Index of the extinction percentage in a sensitivity vector.
pub const COMPONENT_EXTINCTION: usize = 1;
/// Index of the brilliance loss (`100 - tilt_brilliance_pct`) in a sensitivity vector.
pub const COMPONENT_BRILLIANCE_LOSS: usize = 2;
/// Index of the yield loss (`100 - yield_pct`) in a sensitivity vector.
pub const COMPONENT_YIELD_LOSS: usize = 3;

/// `atanh(0.95)`: the pre-squash magnitude of the steepest tier of a composed direction.
pub const MEAN_BOUND: f32 = 1.831_78;
/// Added to the largest `|slope|` before dividing, in both twins and the oracle (bit for
/// bit); it keeps a design whose slopes are all numerical noise from being blown up to the
/// full step.
pub const SLOPE_EPS: f32 = 1e-6;
/// Default line-search scales of a step, tried in this order.
pub const DEFAULT_LINE_SEARCH: [f64; 3] = [1.0, 0.5, 0.25];

/// One free tier's measured slopes at the current design, per degree of that tier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TierSensitivity {
    /// Component slopes, percentage points per degree, in the order of the `COMPONENT_*`
    /// constants; `components[3]` is NaN when the yield is not measurable for this design
    /// (no preform volume).
    pub components: [f32; N_COMPONENTS],
    /// The score slope, points per degree (the weighted composition of `components` as the
    /// environment scores it; measured directly, so it includes the score's own rounding).
    pub score: f32,
    /// Which probe produced it: `+h` or `-h` (`false` = the `-h` fallback was used).
    pub forward_probe: bool,
}

/// What the teacher measures at one state.
#[derive(Debug, Clone, PartialEq)]
pub struct TeacherLabel {
    /// One entry per tier of the design; `None` for a tier the environment does not move or
    /// whose probes both failed.
    pub tiers: Vec<Option<TierSensitivity>>,
    /// Score drop of the oracle step (points, >= 0) at the best line-search scale, `None`
    /// when no tier had a slope or no oracle step was asked for (no scales); `Some(0.0)`
    /// when no scale improved.
    pub oracle_gain: Option<f32>,
    /// The scale that produced `oracle_gain`, `None` when nothing improved.
    pub oracle_scale: Option<f32>,
    /// Candidate evaluations spent on this label (probes + oracle scales).
    pub evaluations: usize,
}

/// The probe of one tier: its slopes and what measuring them cost.
///
/// The unit of work of the teacher. A trainer that labels many designs makes one task of each
/// probe and joins the results with [`assemble_label`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TierProbe {
    /// The tier's slopes, `None` when the tier is not a free tier of the environment, the probe
    /// size is not usable, or both the `+h` and the `-h` candidate were refused.
    pub sensitivity: Option<TierSensitivity>,
    /// Candidate evaluations spent: every probe candidate that reached the scorer counts, a
    /// candidate the environment refuses before scoring is free. `0` for a tier that is not
    /// probed at all.
    pub evaluations: usize,
}

/// The oracle step of a label: the steepest-descent action of the score slopes, tried at the
/// line-search scales in order, without changing the environment.
///
/// The default value is "no oracle step" (no figure, no evaluation): the answer for empty or
/// unusable scales, slopes of the wrong length and slopes with no usable entry.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct OracleOutcome {
    /// Score drop of the first accepted scale (points, > 0 for an accepted candidate),
    /// `Some(0.0)` when no scale improved, `None` when there was no oracle step.
    pub gain: Option<f32>,
    /// The scale that produced `gain`, `None` when nothing improved or there was no step.
    pub scale: Option<f32>,
    /// Candidate evaluations spent on the oracle scales.
    pub evaluations: usize,
}

/// The pre-squash direction of slopes `score` (points per degree, `None` = no slope):
/// `-MEAN_BOUND * s_k / (max_k |s_k| + SLOPE_EPS)`, `0.0` for a tier without a slope.
///
/// A slope that is NaN or infinite counts as "no slope" (it is excluded from the maximum and
/// its entry is `0.0`). When no tier has a slope every entry is `0.0`. The output is never
/// NaN, and lies in `[-MEAN_BOUND, MEAN_BOUND]` for any slope magnitudes below about `1e38`
/// (the numerator `MEAN_BOUND * s` must not overflow `f32`).
#[must_use]
pub fn direction_from_slopes(score: &[Option<f32>]) -> Vec<f32> {
    let usable = |slope: &Option<f32>| slope.filter(|value| value.is_finite());
    let steepest = score
        .iter()
        .filter_map(usable)
        .fold(0.0_f32, |max, slope| max.max(slope.abs()));
    score
        .iter()
        .map(|slope| {
            usable(slope).map_or(0.0, |slope| -MEAN_BOUND * slope / (steepest + SLOPE_EPS))
        })
        .collect()
}

/// `tanh(direction) * max_step_deg` per tier, the action the oracle (and the model) proposes.
///
/// The result has one delta per entry of `direction`, in the same order.
#[must_use]
pub fn action_from_direction(direction: &[f32], max_step_deg: f64) -> ContinuousTierAction {
    ContinuousTierAction::new(
        direction
            .iter()
            .map(|&raw| f64::from(raw.tanh()) * max_step_deg)
            .collect(),
    )
}

/// The score slope of every tier of a label (`None` for a tier without a measurement): the
/// input of the oracle step ([`RetargetEnv::oracle_outcome`](super::RetargetEnv::oracle_outcome)
/// and its twin on the optimizer environment).
#[must_use]
pub fn score_slopes(tiers: &[Option<TierSensitivity>]) -> Vec<Option<f32>> {
    tiers
        .iter()
        .map(|tier| tier.map(|sensitivity| sensitivity.score))
        .collect()
}

/// Joins the pieces of a label: the per-tier probes, the evaluations they cost and the oracle
/// step.
///
/// `tiers` holds one entry per tier of the design (`None` for a tier that was not probed or
/// whose probes failed), `probe_evaluations` the sum of the [`TierProbe::evaluations`] that
/// produced them. The label's `evaluations` is `probe_evaluations + oracle.evaluations`.
/// Nothing is recomputed: the result equals what `teacher_label` returns when the pieces come
/// from the same environment state.
#[must_use]
pub const fn assemble_label(
    tiers: Vec<Option<TierSensitivity>>,
    probe_evaluations: usize,
    oracle: OracleOutcome,
) -> TeacherLabel {
    TeacherLabel {
        tiers,
        oracle_gain: oracle.gain,
        oracle_scale: oracle.scale,
        evaluations: probe_evaluations + oracle.evaluations,
    }
}

/// The four component figures of `eval`, in the order of the `COMPONENT_*` constants.
fn component_values(eval: &DesignEval) -> [f32; N_COMPONENTS] {
    [
        eval.components.windowing_pct,
        eval.components.extinction_pct,
        100.0 - eval.components.tilt_brilliance_pct,
        eval.yield_pct.map_or(f32::NAN, yield_loss_pct),
    ]
}

impl EnvCore {
    /// What the teacher measures at the current design.
    ///
    /// For each free tier the candidate is built exactly as a step builds it
    /// ([`apply_action`] on a one-hot delta of `+h_deg`, so hinge re-anchoring and tier
    /// relations apply) and evaluated at the configured fidelity; when it solves, closes,
    /// scores finitely and passes both gates (girdle, manufacturability, against the base
    /// design as in [`EnvCore::step_scaled`]), its four components and its score give the
    /// one-sided difference quotients `(after - current) / h`. Otherwise the same is tried at
    /// `-h`, with `(current - after) / h`; when both fail the tier is `None`. Lower is better,
    /// so a negative score slope means "increase this tier's angle". A tier the environment
    /// does not move is `None`. The yield slope is NaN when the current or the probed design
    /// has no measurable yield.
    ///
    /// The oracle step follows: [`direction_from_slopes`] over the score slopes,
    /// [`action_from_direction`] at the step limit, then `scales` in order through the same
    /// candidate helper [`EnvCore::step_scaled`] uses. `oracle_gain` is the current score minus
    /// the score of the first accepted candidate, `Some(0.0)` when none was accepted, `None`
    /// when no tier had a slope. An EMPTY `scales` (or one with no usable scale) means "no
    /// oracle step" (`oracle_gain` and `oracle_scale` are `None` and no candidate is spent),
    /// which differs from `step_scaled`, where an empty list means the unit scale.
    ///
    /// The label is the serial composition of the public pieces: [`Self::probe_tier`] for each
    /// free tier, [`Self::oracle_outcome`] on the score slopes of those probes, and
    /// [`assemble_label`].
    ///
    /// Pure: nothing in the core changes, no step is counted and `last_components` stays as
    /// it was. Cost: one or two evaluations per free tier plus up to `scales.len()` for the
    /// oracle ([`TeacherLabel::evaluations`] counts them; a candidate `apply_action` refuses
    /// costs none). An `h_deg` that is not finite and positive gives all `None` and no
    /// evaluations instead of panicking.
    pub(crate) fn teacher_label(&self, h_deg: f64, scales: &[f64]) -> TeacherLabel {
        let mut tiers = vec![None; self.current_design.tiers.len()];
        let mut probe_evaluations = 0;
        for &tier in self.space.free() {
            let probe = self.probe_tier(tier, h_deg);
            tiers[tier] = probe.sensitivity;
            probe_evaluations += probe.evaluations;
        }
        let oracle = self.oracle_outcome(&score_slopes(&tiers), scales);
        assemble_label(tiers, probe_evaluations, oracle)
    }

    /// Per-tier score sensitivity at the current design, in score points per degree: the
    /// score slopes of [`Self::teacher_label`] (without an oracle step).
    ///
    /// One entry per tier of the design. A tier the environment does not move is `None`, and
    /// so is a free tier whose `+h_deg` and `-h_deg` candidates are both refused. Pure, like
    /// the label.
    pub(crate) fn score_gradient_fd(&self, h_deg: f64) -> Vec<Option<f32>> {
        score_slopes(&self.teacher_label(h_deg, &[]).tiers)
    }

    /// The probe of one tier: the unit of work of [`Self::teacher_label`].
    ///
    /// Exactly what the label does for `tier`: the candidate at `+h_deg`, else the one at
    /// `-h_deg`, measured against the current design (see [`Self::teacher_label`] for the
    /// slopes and the gates). The result has `sensitivity: None` and `evaluations: 0` for a
    /// tier that is not a free tier of the environment (pinned, or an index beyond the
    /// design's tiers) and for an `h_deg` that is not finite and positive; it has
    /// `sensitivity: None` and the evaluations actually spent when both candidates are
    /// refused. Pure: nothing in the core changes, and any number of threads may probe the
    /// same environment at once.
    pub(crate) fn probe_tier(&self, tier: usize, h_deg: f64) -> TierProbe {
        if !(h_deg.is_finite() && h_deg > 0.0) || !self.space.free().contains(&tier) {
            return TierProbe::default();
        }
        let mut evaluations = 0;
        let sensitivity = self.tier_sensitivity(tier, h_deg, &mut evaluations);
        TierProbe {
            sensitivity,
            evaluations,
        }
    }

    /// The oracle step for the given score slopes: [`direction_from_slopes`] of `slopes`,
    /// [`action_from_direction`] at the step limit, then `scales` in order through the same
    /// candidate helper [`EnvCore::step_scaled`] uses, without changing the core.
    ///
    /// `slopes` has one entry per tier of the design ([`score_slopes`] of a label's tiers).
    /// The first accepted scale gives `gain` (the current score minus the candidate's) and
    /// `scale`; when no scale is accepted the gain is `Some(0.0)` and the scale `None`. The
    /// default outcome (`None`, `None`, no evaluation) is returned for empty or unusable
    /// `scales` (see [`usable_scales`]; unlike a step, the oracle then takes no unit scale),
    /// for a `slopes` of any other length than the tier count, and when no entry is a finite
    /// slope. Pure, like [`Self::probe_tier`].
    pub(crate) fn oracle_outcome(&self, slopes: &[Option<f32>], scales: &[f64]) -> OracleOutcome {
        let usable: Vec<f64> = usable_scales(scales).collect();
        let has_slope = slopes.iter().flatten().any(|slope| slope.is_finite());
        if usable.is_empty() || !has_slope || slopes.len() != self.current_design.tiers.len() {
            return OracleOutcome::default();
        }
        let action =
            action_from_direction(&direction_from_slopes(slopes), self.settings.max_step_deg);
        let mut evaluations = 0;
        for scale in usable {
            let outcome = self.try_candidate(&action, scale);
            evaluations += usize::from(outcome.evaluated);
            if outcome.is_accepted()
                && let Some(score) = outcome.score()
            {
                return OracleOutcome {
                    gain: Some(self.current_eval.score - score),
                    scale: Some(scale as f32),
                    evaluations,
                };
            }
        }
        OracleOutcome {
            gain: Some(0.0),
            scale: None,
            evaluations,
        }
    }

    /// The slopes of `tier`: the `+h` probe, else the `-h` probe, `None` when both fail.
    /// `evaluations` is incremented for every probe that reached the scorer.
    fn tier_sensitivity(
        &self,
        tier: usize,
        h_deg: f64,
        evaluations: &mut usize,
    ) -> Option<TierSensitivity> {
        [(h_deg, true), (-h_deg, false)]
            .into_iter()
            .find_map(|(delta_deg, forward_probe)| {
                let (probed, evaluated) = self.probe(tier, delta_deg);
                *evaluations += usize::from(evaluated);
                probed.map(|eval| self.sensitivity(&eval, delta_deg, forward_probe))
            })
    }

    /// Evaluates the current design with `tier` moved by `delta_deg` (every other tier at
    /// rest). The first element is the measurement, or `None` when the candidate is refused:
    /// a step would reject it before scoring (unsafe angle, no usable mast, relations), it
    /// does not solve, close or score finitely, or it fails the girdle or manufacturability
    /// gate. The second says whether the candidate reached the scorer (it cost an evaluation).
    fn probe(&self, tier: usize, delta_deg: f64) -> (Option<DesignEval>, bool) {
        let mut deltas = vec![0.0; self.current_design.tiers.len()];
        deltas[tier] = delta_deg;
        // The step limit is the probe size itself, so the delta is not clipped.
        let Ok(candidate) = apply_action(
            &self.space,
            &self.current_design,
            &ContinuousTierAction::new(deltas),
            delta_deg.abs(),
        ) else {
            return (None, false);
        };
        let Ok(eval) = evaluate_design(
            &candidate,
            &self.gem,
            &self.settings.weights,
            self.settings.fidelity,
            self.preform_volume,
        ) else {
            return (None, true);
        };
        let usable = self.girdle_gate(&eval) && self.manufacturing_gate(&eval);
        (usable.then_some(eval), true)
    }

    /// The difference quotients of `probed` against the current design for a move of
    /// `delta_deg` (negative for the `-h` fallback, which makes the quotient
    /// `(current - after) / h`).
    fn sensitivity(
        &self,
        probed: &DesignEval,
        delta_deg: f64,
        forward_probe: bool,
    ) -> TierSensitivity {
        let current = component_values(&self.current_eval);
        let after = component_values(probed);
        let slope = |moved: f32, rest: f32| (f64::from(moved - rest) / delta_deg) as f32;
        TierSensitivity {
            components: std::array::from_fn(|index| slope(after[index], current[index])),
            score: slope(probed.score, self.current_eval.score),
            forward_probe,
        }
    }
}

#[cfg(test)]
#[path = "teacher_tests.rs"]
mod tests;
