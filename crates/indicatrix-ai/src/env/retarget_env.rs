//! Reinforcement learning environment for cross-material gemstone retargeting.
//!
//! Adapts a design authored for material A (e.g. Quartz, RI 1.54) to material B
//! (e.g. Sapphire, RI 1.77, or Diamond, RI 2.42), training an RL policy to jointly adjust
//! crown and pavilion angles and maintain the critical-angle margin while the stone is
//! scored in material B. The source material is the one the design carries (named preset and
//! overrides, or the `.asc` refractive index), so the source index and critical angle in the
//! observation are real.
//!
//! The episode mechanics (greedy acceptance, girdle and manufacturability gates, hinge
//! anchoring of pinned tiers, the line search of [`RetargetEnv::step_scaled`], the yield term of
//! the score, the refusal of a face-up tone weight) are the optimizer environment's; see
//! `optimizer_env`. The optimizer is this task with the target equal to the source material.

use super::{
    action::ContinuousTierAction,
    common::{
        CoreSettings, CoreStep, DEFAULT_MIN_GIRDLE_RATIO, DEFAULT_TERMINAL_SCORE, EnvCore,
        design_source_material,
    },
    error::EnvError,
    observation::GemstoneObservation,
    reward::RewardConfig,
    teacher::{OracleOutcome, TeacherLabel, TierProbe},
    traits::{GemstoneRlEnv, Step},
};
use indicatrix_cut_core::{
    Design, ObjectiveComponents, ObjectiveWeights, ResolvedMaterial, optimize::ObjectiveFidelity,
};

/// Configuration for material retargeting RL environment.
#[derive(Debug, Clone)]
pub struct RetargetEnvConfig {
    /// Maximum search steps per episode (at least 1). Default 60, so the reachable range per
    /// episode (`max_steps * max_step_deg`) stays 30 degrees at the default step.
    pub max_steps: usize,
    /// Maximum angle delta per step, degrees (finite, positive). Default 0.5: a joint
    /// one-degree move of every tier failed the manufacturability gate on 50 to 80 % of the
    /// random proposals of a 24-free-tier design, a quarter degree on none.
    pub max_step_deg: f64,
    /// Reward calculation hyperparameters.
    pub reward_config: RewardConfig,
    /// Target optical evaluation weights. A yield weight above zero adds the yield loss to the
    /// score (when the design's preform makes the yield measurable); a face-up tone weight
    /// above zero is refused by [`RetargetEnv::new`] ([`EnvError::InvalidConfig`]).
    pub weights: ObjectiveWeights,
    /// Evaluation fidelity.
    pub fidelity: ObjectiveFidelity,
    /// Target material the design is being adapted towards.
    pub target_material: ResolvedMaterial,
    /// Let `ScaleReference` tiers move by turning about their hinge (see
    /// `OptimizerEnvConfig::vary_anchored`).
    pub vary_anchored: bool,
    /// An episode terminates when the score falls below this. Default `0.0`: an episode only
    /// ends at the step limit (scores are never negative).
    pub terminal_score: f32,
    /// Fraction of the base design's thinnest girdle gap a candidate must keep (`0..=1`).
    pub min_girdle_ratio: f32,
}

impl RetargetEnvConfig {
    /// Construct the default config targeted towards `target_material`.
    #[must_use]
    pub fn new_with_target(target_material: ResolvedMaterial) -> Self {
        Self {
            max_steps: 60,
            max_step_deg: 0.5,
            reward_config: RewardConfig::default(),
            weights: ObjectiveWeights::default(),
            fidelity: ObjectiveFidelity::Fast,
            target_material,
            vary_anchored: true,
            terminal_score: DEFAULT_TERMINAL_SCORE,
            min_girdle_ratio: DEFAULT_MIN_GIRDLE_RATIO,
        }
    }
}

/// The cross-material retargeting RL environment.
pub struct RetargetEnv {
    config: RetargetEnvConfig,
    source: ResolvedMaterial,
    core: EnvCore,
}

impl RetargetEnv {
    /// Construct a new retargeting environment adapting `design` towards the configured
    /// target material.
    ///
    /// # Errors
    ///
    /// [`EnvError`] when the configuration is out of range or weighs the face-up tone
    /// ([`EnvError::InvalidConfig`]), the design does not solve into a closed, scorable solid
    /// in the target material, no tier of it can move, or it returns no light face-up in the
    /// target material ([`EnvError::NoLightReturn`]: a dead stone whose score no move
    /// changes).
    pub fn new(design: Design, config: RetargetEnvConfig) -> Result<Self, EnvError> {
        let source = design_source_material(&design);
        let settings = CoreSettings {
            max_steps: config.max_steps,
            max_step_deg: config.max_step_deg,
            weights: config.weights,
            fidelity: config.fidelity,
            terminal_score: config.terminal_score,
            min_girdle_ratio: config.min_girdle_ratio,
            vary_anchored: config.vary_anchored,
        };
        let core = EnvCore::new(
            design,
            config.target_material.gem.clone(),
            settings,
            config.reward_config.clone(),
        )?;
        Ok(Self {
            config,
            source,
            core,
        })
    }

    /// The configuration the environment was built with.
    #[must_use]
    pub const fn config(&self) -> &RetargetEnvConfig {
        &self.config
    }

    /// The material the design was authored for.
    #[must_use]
    pub const fn source_material(&self) -> &ResolvedMaterial {
        &self.source
    }

    /// Return the list of movable tier indices.
    #[must_use]
    pub fn movable_tier_indices(&self) -> &[usize] {
        self.core.free_tiers()
    }

    /// The design the episode started from.
    #[must_use]
    pub const fn base_design(&self) -> &Design {
        self.core.base_design()
    }

    /// The best design accepted so far in this episode (the state never gets worse).
    #[must_use]
    pub const fn current_design(&self) -> &Design {
        self.core.current_design()
    }

    /// Score of the base design in the target material (lower is better).
    #[must_use]
    pub const fn base_score(&self) -> f32 {
        self.core.base_score()
    }

    /// Score of [`Self::current_design`] in the target material.
    #[must_use]
    pub const fn current_score(&self) -> f32 {
        self.core.current_score()
    }

    /// Lowest score reached this episode (equals [`Self::current_score`]).
    #[must_use]
    pub const fn best_score(&self) -> f32 {
        self.core.best_score()
    }

    /// Optical components of the base design in the target material.
    #[must_use]
    pub const fn base_components(&self) -> ObjectiveComponents {
        self.core.base_components()
    }

    /// Optical components of [`Self::current_design`] in the target material.
    #[must_use]
    pub const fn current_components(&self) -> ObjectiveComponents {
        self.core.current_components()
    }

    /// Components of the most recently scored proposal; `None` until one was scored this
    /// episode.
    #[must_use]
    pub const fn last_components(&self) -> Option<ObjectiveComponents> {
        self.core.last_components()
    }

    /// Steps taken in the current episode.
    #[must_use]
    pub const fn step_count(&self) -> usize {
        self.core.step_count()
    }

    /// Extract gemstone observation capturing source and target material characteristics.
    #[must_use]
    pub fn build_observation(&self) -> GemstoneObservation {
        self.core
            .observation(&self.source, &self.config.target_material)
    }

    /// The objective weights the environment scores with (they are also in the observation,
    /// as fractions: `GlobalFeature::weight_fractions`).
    #[must_use]
    pub const fn weights(&self) -> &ObjectiveWeights {
        self.core.weights()
    }

    /// Per-tier score sensitivity at the current design in the target material, in score
    /// points per degree (lower score is better, so a negative entry means "increase this
    /// tier's angle").
    ///
    /// One entry per tier of the design: `None` for a tier the environment does not move, and
    /// for a free tier whose `+h_deg` and `-h_deg` candidates are both refused (unsafe, open,
    /// unscorable, or failing a gate); otherwise the one-sided difference quotient of the
    /// score over `h_deg` degrees (the score slopes of [`Self::teacher_label`]). Cost: one or
    /// two candidate evaluations per free tier. The environment is left exactly as it was (no
    /// step counted, no state change). An `h_deg` that is not finite and positive gives all
    /// `None`.
    #[must_use]
    pub fn score_gradient_fd(&self, h_deg: f64) -> Vec<Option<f32>> {
        self.core.score_gradient_fd(h_deg)
    }

    /// What the teacher measures at the current design: per free tier the slopes of the four
    /// score components and of the score itself per degree (probe at `+h_deg`, else
    /// `-h_deg`), and the gain of the oracle step (the steepest-descent action of the score
    /// slopes tried at `scales`, in order, without changing the environment).
    ///
    /// An empty `scales` means "no oracle step" (`oracle_gain` is `None`); this differs from
    /// [`GemstoneRlEnv::step_scaled`], where an empty list means the unit scale. Pure: no step
    /// is counted and no state changes. An `h_deg` that is not finite and positive gives an
    /// empty label (all `None`, no evaluations). See [`TeacherLabel`] and `env::teacher`.
    #[must_use]
    pub fn teacher_label(&self, h_deg: f64, scales: &[f64]) -> TeacherLabel {
        self.core.teacher_label(h_deg, scales)
    }

    /// The probe of one tier: the unit of work of [`Self::teacher_label`], for a caller that
    /// schedules the probes of many environments itself.
    ///
    /// The candidate at `+h_deg`, else the one at `-h_deg`, measured against the current design
    /// exactly as the label measures each free tier; see [`TierProbe`]. `sensitivity: None` with
    /// `evaluations: 0` for a tier that is not free (pinned, or an index beyond the design's
    /// tiers) and for an `h_deg` that is not finite and positive; `sensitivity: None` with the
    /// evaluations actually spent when both candidates are refused. Pure and `&self`: no state
    /// changes, and probes of different tiers may run on different threads at once. Join the
    /// probes with [`Self::oracle_outcome`] and `assemble_label` (in `env::teacher`).
    #[must_use]
    pub fn probe_tier(&self, tier: usize, h_deg: f64) -> TierProbe {
        self.core.probe_tier(tier, h_deg)
    }

    /// The oracle step of a label for the given score slopes: the steepest-descent action of
    /// `slopes` (one entry per tier of the design, `score_slopes` of the probes in
    /// `env::teacher`) tried at `scales` in order, without changing the environment.
    ///
    /// The gain is the current score minus the score of the first accepted scale, `Some(0.0)`
    /// when none was accepted. The default outcome (no gain, no scale, no evaluation) is the
    /// answer for empty or unusable `scales` (this differs from [`GemstoneRlEnv::step_scaled`],
    /// where an empty list means the unit scale), for `slopes` of any other length than the
    /// design's tier count, and for slopes without a finite entry. See [`OracleOutcome`].
    /// Pure, like [`Self::probe_tier`].
    #[must_use]
    pub fn oracle_outcome(&self, slopes: &[Option<f32>], scales: &[f64]) -> OracleOutcome {
        self.core.oracle_outcome(slopes, scales)
    }

    /// The step the trait methods return: the core outcome plus the observation after it.
    fn finish(&self, outcome: CoreStep) -> Step<GemstoneObservation> {
        Step {
            observation: self.build_observation(),
            reward: outcome.reward,
            terminated: outcome.terminated,
            truncated: outcome.truncated,
            info: outcome.info,
        }
    }
}

impl GemstoneRlEnv for RetargetEnv {
    type Observation = GemstoneObservation;
    type Action = ContinuousTierAction;

    fn reset(&mut self, _seed: u64) -> Self::Observation {
        self.core.reset();
        self.build_observation()
    }

    fn step(&mut self, action: &Self::Action) -> Step<Self::Observation> {
        let outcome = self.core.step(action);
        self.finish(outcome)
    }

    fn step_scaled(&mut self, action: &Self::Action, scales: &[f64]) -> Step<Self::Observation> {
        let outcome = self.core.step_scaled(action, scales);
        self.finish(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{
        observation::{WEIGHT_FRACTION_INDEX, weight_fractions},
        teacher::DEFAULT_LINE_SEARCH,
        test_fixtures::{fixture_env, fixture_env_with, sapphire},
        traits::{StepInfo, StepRejection},
    };
    use indicatrix_cut_core::ObjectivePreset;

    #[test]
    fn source_and_target_indices_differ() {
        let observation = fixture_env().build_observation();
        assert!((observation.global.refractive_index - 1.54).abs() < 1e-4);
        assert!(observation.global.target_refractive_index > 1.7);
        assert!(observation.global.target_refractive_index < 1.8);
        assert!(
            observation.global.target_critical_angle_deg < observation.global.critical_angle_deg
        );
    }

    #[test]
    fn the_design_has_movable_tiers_and_a_finite_target_score() {
        let env = fixture_env();
        assert_eq!(env.movable_tier_indices().len(), 4);
        assert!(env.base_score().is_finite());
    }

    #[test]
    fn reset_restores_the_base_state() {
        let mut env = fixture_env();
        let tiers = env.base_design().tiers.len();
        let base = env.base_score();
        let _ = env.step(&ContinuousTierAction::new(vec![-0.5; tiers]));
        let _ = env.reset(0);
        assert_eq!(env.current_score(), base);
        assert_eq!(env.step_count(), 0);
    }

    #[test]
    fn wrong_length_action_is_rejected() {
        let mut env = fixture_env();
        let step = env.step(&ContinuousTierAction::new(Vec::new()));
        assert!(step.info.rejection.is_some());
        assert_eq!(
            step.reward,
            env.config().reward_config.invalid_geometry_penalty
        );
        // Rejected before it was scored: no candidate score.
        assert_eq!(step.info.candidate_score, None);
    }

    #[test]
    fn defaults_are_the_training_defaults() {
        let config = RetargetEnvConfig::new_with_target(sapphire());
        assert_eq!(config.max_steps, 60);
        assert_eq!(config.max_step_deg, 0.5);
        assert_eq!(config.max_steps as f64 * config.max_step_deg, 30.0);
        assert_eq!(config.terminal_score, 0.0);
    }

    #[test]
    fn score_gradient_fd_marks_pinned_tiers_none_and_leaves_the_state_alone() {
        let env = fixture_env();
        let tiers = env.base_design().tiers.len();
        let score = env.current_score();
        let observation = env.build_observation();

        let gradient = env.score_gradient_fd(0.25);
        assert_eq!(gradient.len(), tiers);
        assert_eq!(env.movable_tier_indices(), &[1, 2, 3, 4]);
        for (index, entry) in gradient.iter().enumerate() {
            if env.movable_tier_indices().contains(&index) {
                assert!(
                    entry.is_some_and(f32::is_finite),
                    "free tier {index}: {entry:?}"
                );
            } else {
                assert!(entry.is_none(), "pinned tier {index}: {entry:?}");
            }
        }

        // Pure: nothing moved, nothing counted, nothing recorded.
        assert_eq!(env.current_score(), score);
        assert_eq!(env.step_count(), 0);
        assert_eq!(env.build_observation(), observation);
        assert!(env.last_components().is_none());

        // A probe size that is not finite and positive gives no gradient and does not panic.
        for h in [0.0, -0.25, f64::NAN, f64::INFINITY] {
            let none = env.score_gradient_fd(h);
            assert_eq!(none.len(), tiers, "h = {h}");
            assert!(none.iter().all(Option::is_none), "h = {h}");
        }
    }

    #[test]
    fn score_gradient_fd_is_the_secant_of_the_step_the_policy_would_make() {
        // For every free tier whose `+h` probe is a stone a step accepts or merely does not
        // improve (closed, both gates passed), the gradient times h is exactly the step's own
        // candidate score minus the current one.
        let mut env = fixture_env();
        let tiers = env.base_design().tiers.len();
        let h = 0.25;
        let gradient = env.score_gradient_fd(h);
        let base = env.current_score();
        let mut checked = 0;
        for tier in env.movable_tier_indices().to_vec() {
            let _ = env.reset(0);
            let mut deltas = vec![0.0; tiers];
            deltas[tier] = h;
            let step = env.step(&ContinuousTierAction::new(deltas));
            if matches!(step.info.rejection, None | Some(StepRejection::NotImproved)) {
                let candidate = step
                    .info
                    .candidate_score
                    .expect("a closed, scored candidate");
                let slope = gradient[tier].expect("a free tier with a feasible +h probe");
                let secant = f64::from(candidate - base) / h;
                assert!((f64::from(slope) - secant).abs() < 1e-4, "tier {tier}");
                checked += 1;
            }
        }
        assert!(checked > 0, "no free tier had a feasible +h probe");
    }

    #[test]
    fn candidate_score_is_reported_for_a_rejected_but_scored_proposal() {
        let mut env = fixture_env();
        let tiers = env.base_design().tiers.len();
        let mut scored_rejections = 0;
        let mut check = |info: &StepInfo| {
            // A candidate that solved and closed is a scored one, whatever the gates said.
            assert_eq!(info.candidate_score.is_some(), info.is_closed);
            match info.rejection {
                None => assert_eq!(info.candidate_score, Some(info.current_score)),
                Some(StepRejection::NotImproved) => {
                    let candidate = info.candidate_score.expect("scored but not improved");
                    assert!(candidate >= info.current_score, "{candidate}");
                    scored_rejections += 1;
                }
                Some(
                    StepRejection::GirdleViolation | StepRejection::ManufacturabilityRegression,
                ) => {
                    assert!(
                        info.candidate_score.is_some(),
                        "a gate failure is still scored"
                    );
                    scored_rejections += 1;
                }
                // Rejected before it was scored.
                Some(_) => assert_eq!(info.candidate_score, None),
            }
        };
        for delta in [0.5, -0.5, 0.25, -0.25] {
            let _ = env.reset(0);
            let first = env.step(&ContinuousTierAction::new(vec![delta; tiers]));
            check(&first.info);
            if first.info.rejection.is_none() {
                // Undoing an accepted move from the improved state lands on the worse base
                // stone: it scores, and it does not improve.
                let back = env.step(&ContinuousTierAction::new(vec![-delta; tiers]));
                check(&back.info);
            }
        }
        assert!(
            scored_rejections > 0,
            "no proposal was rejected after scoring"
        );
    }

    #[test]
    fn a_yield_weight_adds_the_yield_loss_when_the_yield_is_measurable() {
        // Without a yield weight the score is the optical one, bit for bit.
        let optical = fixture_env();
        assert_eq!(optical.weights().yield_weight, 0.0);
        assert_eq!(
            optical.base_score(),
            optical.weights().score(&optical.base_components())
        );

        // With one, the fixture's block preform makes the yield measurable and it joins the
        // score as `100 - yield`.
        let weights = ObjectivePreset::KeepWeight.weights();
        let weighted = fixture_env_with(|config| config.weights = weights);
        assert_eq!(weighted.weights(), &weights);
        let yield_pct = weighted.build_observation().global.yield_pct;
        assert!(yield_pct > 0.0, "the fixture's yield is measurable");
        assert_eq!(
            weighted.base_score(),
            weights.score_with_yield(&weighted.base_components(), 100.0 - yield_pct)
        );
        assert_ne!(
            weighted.base_score(),
            weights.score(&weighted.base_components())
        );
        // The optical figures themselves do not depend on the weights.
        assert_eq!(weighted.base_components(), optical.base_components());
    }

    #[test]
    fn the_observation_carries_the_weight_fractions() {
        let balanced = fixture_env().build_observation();
        let third = 1.0 / 3.0;
        assert_eq!(balanced.global.weight_fractions, [third, third, third, 0.0]);
        assert_eq!(
            balanced.global_token()[WEIGHT_FRACTION_INDEX..],
            [third, third, third, 0.0]
        );

        for preset in [
            ObjectivePreset::Brilliance,
            ObjectivePreset::LowWindowing,
            ObjectivePreset::LowExtinction,
            ObjectivePreset::KeepWeight,
        ] {
            let mut env = fixture_env_with(|config| config.weights = preset.weights());
            let fractions = weight_fractions(&preset.weights());
            let observation = env.build_observation();
            assert_eq!(observation.global.weight_fractions, fractions, "{preset:?}");
            assert_eq!(
                observation.global_token()[WEIGHT_FRACTION_INDEX..],
                fractions,
                "{preset:?}"
            );
            // The fractions are part of the state at every step, not only at the start.
            let tiers = env.base_design().tiers.len();
            let step = env.step(&ContinuousTierAction::new(vec![-0.3; tiers]));
            assert_eq!(step.observation.global.weight_fractions, fractions);
        }
    }

    #[test]
    fn a_face_up_tone_weight_is_refused() {
        for preset in [ObjectivePreset::LightenDark, ObjectivePreset::IntensifyPale] {
            let config = RetargetEnvConfig {
                weights: preset.weights(),
                ..RetargetEnvConfig::new_with_target(sapphire())
            };
            let refused = RetargetEnv::new(Design::concave_fixture(), config);
            assert!(
                matches!(
                    refused,
                    Err(EnvError::InvalidConfig(text)) if text.contains("tone")
                ),
                "{preset:?}"
            );
        }
    }

    #[test]
    fn the_default_line_search_never_makes_the_state_worse() {
        let mut env = fixture_env();
        let tiers = env.base_design().tiers.len();
        let base = env.base_score();
        let mut previous = base;
        for _ in 0..4 {
            let step = env.step_scaled(
                &ContinuousTierAction::new(vec![0.4; tiers]),
                &DEFAULT_LINE_SEARCH,
            );
            assert!(env.current_score() <= previous);
            assert!(usize::from(step.info.evaluations) <= DEFAULT_LINE_SEARCH.len());
            match step.info.scale {
                // An accepted candidate improved the score, at one of the scales.
                Some(scale) => {
                    assert!(step.info.rejection.is_none());
                    assert!(env.current_score() < previous);
                    assert!(
                        DEFAULT_LINE_SEARCH
                            .iter()
                            .any(|&known| known as f32 == scale)
                    );
                }
                None => {
                    assert!(step.info.rejection.is_some());
                    assert_eq!(env.current_score(), previous);
                }
            }
            previous = env.current_score();
        }
        assert_eq!(env.step_count(), 4);
        assert!(env.current_score() <= base);
    }
}
