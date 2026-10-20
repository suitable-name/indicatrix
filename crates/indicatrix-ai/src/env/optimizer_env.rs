//! Reinforcement learning environment for single-material gemstone cut optimization.
//!
//! Keeps the design's material fixed (the one it was authored for: its named preset and
//! overrides, or the `.asc` refractive index) and explores joint angle adjustments of the
//! movable tiers, mimicking `indicatrix-cut-core::optimize` while training an RL agent to
//! navigate the coupled non-convex optical space directly.
//!
//! The MDP is greedy (see `traits`): a candidate replaces the current design only when it is
//! a valid, girdle-keeping, non-regressing stone that scores strictly lower. A step may try
//! its action at several scales (a line search, [`OptimizerEnv::step_scaled`]) and keeps the
//! first improving one. The shared scorer includes yield when measurable and both face-up tone
//! goals. Imported
//! `.asc` designs pin every tier to a mast; with [`OptimizerEnvConfig::vary_anchored`] (the
//! default) those tiers turn about their measured hinge, as in the editor's Optimize.

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
use indicatrix::optics::raytracer::LightingPreset;
use indicatrix_cut_core::ShapeTarget;
use indicatrix_cut_core::{
    Design, ObjectiveComponents, ObjectiveWeights, ResolvedMaterial, optimize::ObjectiveFidelity,
};
use std::collections::{BTreeMap, BTreeSet};

/// Environment configuration for cut optimization.
#[derive(Debug, Clone)]
pub struct OptimizerEnvConfig {
    /// Maximum search steps per episode (at least 1). Default 60, so the reachable range per
    /// episode (`max_steps * max_step_deg`) stays 30 degrees at the default step.
    pub max_steps: usize,
    /// Maximum angle delta permitted in a single step, degrees (finite, positive). Default
    /// 0.5: a joint one-degree move of every tier failed the manufacturability gate on 50 to
    /// 80 % of the random proposals of a 24-free-tier design, a quarter degree on none.
    pub max_step_deg: f64,
    /// Reward function weights and penalties.
    pub reward_config: RewardConfig,
    /// Objective scoring weights, including measured yield and either tone goal when available.
    pub weights: ObjectiveWeights,
    /// Evaluation fidelity during rollouts (Fast vs Full).
    pub fidelity: ObjectiveFidelity,
    /// Lighting preset used for training and endpoint scoring.
    pub lighting: LightingPreset,
    /// Optional shape target and additive penalty.
    pub shape_target: Option<ShapeTarget>,
    /// Probe displacement in degrees, also represented in observations.
    pub probe_scale_deg: f64,
    /// Optional allow-list of angle coordinates; coupled mast repairs remain enabled.
    pub only_tiers: Option<BTreeSet<usize>>,
    /// Inclusive authored-angle bounds by tier index.
    pub angle_bounds: BTreeMap<usize, (f64, f64)>,
    /// Optional measured source material override (for deterministic material augmentation).
    pub source_material_override: Option<ResolvedMaterial>,
    /// Let `ScaleReference` tiers (every tier of an imported `.asc`) move by turning about
    /// their hinge. Off, only meet-constrained tiers move and an imported design has none.
    pub vary_anchored: bool,
    /// An episode terminates when the score falls below this. Default `0.0`: ordinary positive
    /// scores continue to the step limit.
    pub terminal_score: f32,
    /// Fraction of the base design's thinnest girdle gap a candidate must keep (`0..=1`).
    pub min_girdle_ratio: f32,
}

impl Default for OptimizerEnvConfig {
    fn default() -> Self {
        Self {
            max_steps: 60,
            max_step_deg: 0.5,
            reward_config: RewardConfig::default(),
            weights: ObjectiveWeights::default(),
            fidelity: ObjectiveFidelity::Fast,
            lighting: LightingPreset::IsoHemisphere,
            shape_target: None,
            probe_scale_deg: 0.25,
            only_tiers: None,
            angle_bounds: BTreeMap::new(),
            source_material_override: None,
            vary_anchored: true,
            terminal_score: DEFAULT_TERMINAL_SCORE,
            min_girdle_ratio: DEFAULT_MIN_GIRDLE_RATIO,
        }
    }
}

/// The single-material RL optimization environment.
pub struct OptimizerEnv {
    config: OptimizerEnvConfig,
    source: ResolvedMaterial,
    core: EnvCore,
}

impl OptimizerEnv {
    /// Construct a new optimizer environment seeded from a baseline design.
    ///
    /// # Errors
    ///
    /// [`EnvError`] when configuration is invalid, the design cannot be solved and scored, or
    /// no tier can move. A no-light result is scored normally and is not rejected at construction.
    pub fn new(design: Design, config: OptimizerEnvConfig) -> Result<Self, EnvError> {
        let source = config
            .source_material_override
            .clone()
            .unwrap_or_else(|| design_source_material(&design));
        let settings = CoreSettings {
            max_steps: config.max_steps,
            max_step_deg: config.max_step_deg,
            weights: config.weights,
            fidelity: config.fidelity,
            terminal_score: config.terminal_score,
            min_girdle_ratio: config.min_girdle_ratio,
            vary_anchored: config.vary_anchored,
            task_kind: 0.0,
            lighting: config.lighting,
            shape_target: config.shape_target,
            probe_scale_deg: config.probe_scale_deg,
            only_tiers: config.only_tiers.clone(),
            angle_bounds: config.angle_bounds.clone(),
        };
        let core = EnvCore::new(
            design,
            source.gem.clone(),
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
    pub const fn config(&self) -> &OptimizerEnvConfig {
        &self.config
    }

    /// The material the design is scored in.
    #[must_use]
    pub const fn source_material(&self) -> &ResolvedMaterial {
        &self.source
    }

    /// Return the list of free tier indices currently being manipulated.
    #[must_use]
    pub fn free_tier_indices(&self) -> &[usize] {
        self.core.free_tiers()
    }

    /// The design the episode started from.
    #[must_use]
    pub const fn base_design(&self) -> &Design {
        self.core.base_design()
    }

    pub(crate) fn preservation_baseline(
        &self,
    ) -> std::sync::Arc<indicatrix_cut_core::meet_preservation::MeetPreservationBaseline> {
        self.core.preservation_baseline()
    }

    /// The best design accepted so far in this episode (the state never gets worse).
    #[must_use]
    pub const fn current_design(&self) -> &Design {
        self.core.current_design()
    }

    /// Score of the base design under the configured fidelity (lower is better).
    #[must_use]
    pub const fn base_score(&self) -> f32 {
        self.core.base_score()
    }

    /// Score of [`Self::current_design`] under the configured fidelity.
    #[must_use]
    pub const fn current_score(&self) -> f32 {
        self.core.current_score()
    }

    /// Lowest score reached this episode (equals [`Self::current_score`]).
    #[must_use]
    pub const fn best_score(&self) -> f32 {
        self.core.best_score()
    }

    /// Optical components of the base design.
    #[must_use]
    pub const fn base_components(&self) -> ObjectiveComponents {
        self.core.base_components()
    }

    /// Optical components of [`Self::current_design`].
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

    /// Extract an observation from the current state.
    #[must_use]
    pub fn build_observation(&self) -> GemstoneObservation {
        self.core.observation(&self.source, &self.source)
    }

    /// The objective weights the environment scores with (they are also in the observation,
    /// as fractions: `GlobalFeature::weight_fractions`).
    #[must_use]
    pub const fn weights(&self) -> &ObjectiveWeights {
        self.core.weights()
    }

    /// Per-tier score sensitivity at the current design, in score points per degree (lower
    /// score is better, so a negative entry means "increase this tier's angle").
    ///
    /// One entry per tier of the design: `None` for a tier the environment does not move, and
    /// for a free tier whose `+h_deg` and `-h_deg` candidates are both refused (unsafe, open,
    /// unscorable, or failing a gate); otherwise the two-sided derivative (nonuniform when
    /// clipped) of the
    /// score over `h_deg` degrees (the score slopes of [`Self::teacher_label`]). Cost: one or
    /// two candidate evaluations per free tier. The environment is left exactly as it was (no
    /// step counted, no state change). An `h_deg` that is not finite and positive gives all
    /// `None`.
    #[must_use]
    pub fn score_gradient_fd(&self, h_deg: f64) -> Vec<Option<f32>> {
        self.core.score_gradient_fd(h_deg)
    }

    /// What the teacher measures at the current design: per free tier the slopes of seven
    /// score components and of the score itself per degree (two-sided when both probes are
    /// feasible, otherwise one-sided), and the gain of the oracle step (the steepest-descent action of the score
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

    /// [`Self::oracle_outcome`] with the candidates of the line search evaluated concurrently
    /// when `parallel` is set (one scoped thread per scale). The outcome is bit-identical to the
    /// sequential one, `evaluations` included.
    #[must_use]
    pub fn oracle_outcome_with(
        &self,
        slopes: &[Option<f32>],
        scales: &[f64],
        parallel: bool,
    ) -> OracleOutcome {
        self.core.oracle_outcome_with(slopes, scales, parallel)
    }

    /// [`GemstoneRlEnv::step_scaled`] with the candidates of the line search evaluated
    /// concurrently when `parallel` is set (one scoped thread per scale). The step is
    /// bit-identical to the sequential one, and `StepInfo::evaluations` is the count of the
    /// sequential search.
    pub fn step_scaled_with(
        &mut self,
        action: &ContinuousTierAction,
        scales: &[f64],
        parallel: bool,
    ) -> Step<GemstoneObservation> {
        let outcome = self.core.step_scaled_with(action, scales, parallel);
        self.finish(outcome)
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

impl GemstoneRlEnv for OptimizerEnv {
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
        observation::weight_fractions, teacher::DEFAULT_LINE_SEARCH, traits::StepRejection,
    };
    use indicatrix_cut_core::ObjectivePreset;

    fn fixture_env() -> OptimizerEnv {
        OptimizerEnv::new(Design::concave_fixture(), OptimizerEnvConfig::default())
            .expect("the fixture is a closed, movable stone")
    }

    #[test]
    fn fixture_has_four_free_tiers_and_a_finite_base_score() {
        let env = fixture_env();
        assert_eq!(env.free_tier_indices().len(), 4);
        assert!(env.base_score().is_finite());
        assert_eq!(env.base_score(), env.current_score());
    }

    #[test]
    fn source_index_comes_from_the_design_not_from_diamond() {
        let observation = fixture_env().build_observation();
        assert!((observation.global.refractive_index - 1.54).abs() < 1e-4);
        assert!((observation.global.target_refractive_index - 1.54).abs() < 1e-4);
        assert!(observation.global.windowing_pct.is_finite());
    }

    #[test]
    fn a_design_without_a_material_reports_its_i_line_index_and_no_dispersion() {
        let mut design = Design::concave_fixture();
        design.material = indicatrix_cut_core::MaterialSelection::none();
        let env = OptimizerEnv::new(design, OptimizerEnvConfig::default())
            .expect("the fixture is a closed, movable stone");
        let observation = env.build_observation();
        assert!((observation.global.refractive_index - 1.54).abs() < 1e-4);
        assert!(observation.global.dispersion.abs() < 1e-6);
    }

    #[test]
    fn free_tier_mask_matches_the_environment() {
        let env = fixture_env();
        let observation = env.build_observation();
        let mask = observation.free_tier_mask();
        assert_eq!(mask.len(), observation.tiers.len());
        let crit = observation.global.target_critical_angle_deg;
        for (index, free) in mask.iter().enumerate() {
            // The mask is the environment's movable set minus the tiers the projection forces
            // (the fixture's -38 degree pavilion tier lies below the quartz envelope).
            let forced =
                crate::env::observation::projection_overrides_tier(&observation.tiers[index], crit);
            assert_eq!(
                *free,
                env.free_tier_indices().contains(&index) && !forced,
                "tier {index}"
            );
        }
    }

    #[test]
    fn observation_carries_one_token_per_tier() {
        let env = fixture_env();
        let observation = env.build_observation();
        assert_eq!(observation.tiers.len(), env.base_design().tiers.len());
        let pinned = observation
            .tiers
            .iter()
            .filter(|t| t.is_pinned > 0.5)
            .count();
        assert_eq!(pinned, observation.tiers.len() - 4);
    }

    #[test]
    fn wrong_length_action_is_rejected_without_panic() {
        let mut env = fixture_env();
        let before = env.current_score();
        let action = ContinuousTierAction::new(vec![0.1; 2]);
        let step = env.step(&action);
        assert!(matches!(
            step.info.rejection,
            Some(StepRejection::WrongActionLength {
                expected: 5,
                got: 2
            })
        ));
        assert_eq!(
            step.reward,
            env.config().reward_config.invalid_geometry_penalty
        );
        assert_eq!(env.current_score(), before);
    }

    #[test]
    fn reset_restores_the_base_state() {
        let mut env = fixture_env();
        let base = env.base_score();
        let tiers = env.base_design().tiers.len();
        let _ = env.step(&ContinuousTierAction::new(vec![0.3; tiers]));
        let observation = env.reset(7);
        assert_eq!(env.current_score(), base);
        assert_eq!(env.step_count(), 0);
        assert_eq!(observation.global.progress_ratio, 0.0);
        assert_eq!(env.current_design().tiers.len(), tiers);
    }

    #[test]
    fn defaults_are_the_training_defaults() {
        let config = OptimizerEnvConfig::default();
        assert_eq!(config.max_steps, 60);
        assert_eq!(config.max_step_deg, 0.5);
        // 60 steps of 0.5 degrees keep the reachable range per episode at 30 degrees.
        assert_eq!(config.max_steps as f64 * config.max_step_deg, 30.0);
        assert_eq!(config.terminal_score, 0.0);
    }

    #[test]
    fn base_angle_is_the_episode_start_angle() {
        let mut env = fixture_env();
        let start: Vec<f32> = env
            .base_design()
            .tiers
            .iter()
            .map(|tier| tier.angle_deg as f32)
            .collect();
        for (tier, base) in env.build_observation().tiers.iter().zip(&start) {
            assert_eq!(tier.base_angle_deg, *base);
            assert_eq!(tier.angle_deg, *base);
        }
        for _ in 0..3 {
            let before: Vec<f32> = env
                .build_observation()
                .tiers
                .iter()
                .map(|tier| tier.angle_deg)
                .collect();
            let step = env.step(&ContinuousTierAction::new(vec![-0.4; start.len()]));
            let mut moved = false;
            for (index, tier) in step.observation.tiers.iter().enumerate() {
                // The episode-start angle never changes, whatever the step did.
                assert_eq!(tier.base_angle_deg, start[index], "tier {index}");
                if step.info.rejection.is_some() {
                    assert_eq!(tier.angle_deg, before[index], "rejected, tier {index}");
                }
                if tier.is_pinned > 0.5 {
                    assert_eq!(tier.angle_deg, start[index], "pinned, tier {index}");
                }
                moved |= tier.angle_deg != before[index];
            }
            // An accepted step moved the current angle of at least one tier.
            assert_eq!(moved, step.info.rejection.is_none());
        }
        for tier in &env.reset(0).tiers {
            assert_eq!(tier.angle_deg, tier.base_angle_deg);
        }
    }

    #[test]
    fn the_observation_carries_the_tier_position_the_azimuth_and_the_base_score() {
        let env = fixture_env();
        let design = env.base_design();
        // The fixture is a 16-tooth wheel; the first index of each tier is 0, 0, 2, 0, 2.
        assert_eq!(design.meta.gear_teeth, 16);
        let first: Vec<Option<f64>> = design
            .tiers
            .iter()
            .map(|tier| tier.indices.first().copied())
            .collect();
        assert_eq!(
            first,
            [Some(0.0), Some(0.0), Some(2.0), Some(0.0), Some(2.0)]
        );
        let observation = env.build_observation();
        // Index 0 is azimuth 0; index 2 of 16 is an eighth turn (sqrt(1/2), sqrt(1/2)).
        let eighth = std::f32::consts::FRAC_1_SQRT_2;
        for (index, tier) in observation.tiers.iter().enumerate() {
            let (sin, cos) = if first[index] == Some(2.0) {
                (eighth, eighth)
            } else {
                (0.0, 1.0)
            };
            assert!((tier.azimuth_sin - sin).abs() < 1e-6, "tier {index}");
            assert!((tier.azimuth_cos - cos).abs() < 1e-6, "tier {index}");
            // Position over count: 0/5, 1/5, ... 4/5.
            assert!(
                (tier.index_frac - index as f32 / 5.0).abs() < 1e-6,
                "tier {index}"
            );
        }
        assert_eq!(observation.global.base_score, env.base_score());
    }

    #[test]
    fn the_state_never_gets_worse() {
        let mut env = fixture_env();
        let tiers = env.base_design().tiers.len();
        let base = env.base_score();
        for _ in 0..3 {
            let _ = env.step(&ContinuousTierAction::new(vec![0.4; tiers]));
            assert!(env.current_score() <= base);
        }
    }

    #[test]
    fn invalid_configuration_is_an_error() {
        let config = OptimizerEnvConfig {
            max_steps: 0,
            ..OptimizerEnvConfig::default()
        };
        assert!(matches!(
            OptimizerEnv::new(Design::concave_fixture(), config),
            Err(EnvError::InvalidConfig(_))
        ));
        let config = OptimizerEnvConfig {
            max_step_deg: f64::NAN,
            ..OptimizerEnvConfig::default()
        };
        assert!(matches!(
            OptimizerEnv::new(Design::concave_fixture(), config),
            Err(EnvError::InvalidConfig(_))
        ));
    }

    #[test]
    fn a_design_with_no_movable_tier_is_an_error() {
        let config = OptimizerEnvConfig {
            vary_anchored: false,
            ..OptimizerEnvConfig::default()
        };
        assert!(matches!(
            OptimizerEnv::new(Design::concave_fixture(), config),
            Err(EnvError::NoMovableTiers)
        ));
    }

    #[test]
    fn a_face_up_tone_weight_is_scored() {
        for preset in [ObjectivePreset::LightenDark, ObjectivePreset::IntensifyPale] {
            let config = OptimizerEnvConfig {
                weights: preset.weights(),
                ..OptimizerEnvConfig::default()
            };
            let env = OptimizerEnv::new(Design::concave_fixture(), config).expect("tone scores");
            assert!(env.base_score().is_finite(), "{preset:?}");
        }
    }

    #[test]
    fn the_weights_reach_the_score_and_the_observation() {
        let weights = ObjectivePreset::KeepWeight.weights();
        let env = OptimizerEnv::new(
            Design::concave_fixture(),
            OptimizerEnvConfig {
                weights,
                ..OptimizerEnvConfig::default()
            },
        )
        .expect("the fixture is a closed, movable stone");
        assert_eq!(env.weights(), &weights);
        let observation = env.build_observation();
        assert_eq!(
            observation.global.weight_fractions,
            weight_fractions(&weights)
        );
        // The yield is measurable on the fixture and weighs in the score.
        let yield_pct = observation.global.yield_pct;
        assert!(yield_pct > 0.0);
        assert_eq!(
            env.base_score(),
            weights.score_with_yield(&env.base_components(), 100.0 - yield_pct)
        );
        let optical = fixture_env();
        assert_eq!(optical.weights(), &ObjectiveWeights::default());
        assert_eq!(
            optical.base_score(),
            optical.weights().score(&optical.base_components())
        );
    }

    #[test]
    fn step_scaled_with_the_unit_scale_is_step() {
        let mut plain = fixture_env();
        let mut scaled = fixture_env();
        let tiers = plain.base_design().tiers.len();
        let action = ContinuousTierAction::new(vec![0.3; tiers]);
        for _ in 0..2 {
            assert_eq!(plain.step(&action), scaled.step_scaled(&action, &[1.0]));
        }
        assert_eq!(plain.step_count(), 2);
        assert_eq!(scaled.step_count(), 2);
    }

    #[test]
    fn the_teacher_label_covers_the_free_tiers_and_changes_nothing() {
        let env = fixture_env();
        let score = env.current_score();
        let observation = env.build_observation();
        let label = env.teacher_label(0.25, &DEFAULT_LINE_SEARCH);
        assert_eq!(label.tiers.len(), env.base_design().tiers.len());
        let mut measured = 0;
        for (index, entry) in label.tiers.iter().enumerate() {
            if env.free_tier_indices().contains(&index) {
                measured += usize::from(entry.is_some());
            } else {
                assert!(entry.is_none(), "pinned tier {index}: {entry:?}");
            }
        }
        assert!(measured > 0, "no free tier had a feasible probe");
        assert!(label.oracle_gain.is_some_and(|gain| gain >= 0.0));
        assert_eq!(env.current_score(), score);
        assert_eq!(env.step_count(), 0);
        assert_eq!(env.build_observation(), observation);
        assert!(env.last_components().is_none());
        // The wrapper is the core's label: the score slopes are the gradient.
        let gradient = env.score_gradient_fd(0.25);
        for (entry, slope) in label.tiers.iter().zip(&gradient) {
            assert_eq!(entry.map(|sensitivity| sensitivity.score), *slope);
        }
    }
}
