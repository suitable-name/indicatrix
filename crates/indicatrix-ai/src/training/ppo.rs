//! Proximal Policy Optimization (PPO) hyperparameters, the learning-rate schedule shared with the
//! imitation trainer, and the metrics a PPO update reports.
//!
//! PPO is the secondary training algorithm; imitation (`training::imitation`) is the primary one.
//! The configuration files and statistics of older runs still load: serde ignores the fields that
//! were removed (`pre_tanh_penalty_coeff`, `teacher_coeff`, `pre_tanh_sq`, `teacher_loss`).

use serde::{Deserialize, Serialize};

use super::TrainError;

/// Learning-rate schedule applied over optimizer updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LrSchedule {
    /// Constant learning rate.
    #[default]
    Constant,
    /// Linear decay from `learning_rate` to zero over `total_updates` optimizer steps.
    Linear {
        /// Number of optimizer steps after which the rate reaches zero (at least 1).
        total_updates: u64,
    },
}

impl LrSchedule {
    /// Learning rate for the optimizer step with zero-based index `update_index`.
    #[must_use]
    pub fn learning_rate(self, base: f32, update_index: u64) -> f32 {
        match self {
            Self::Constant => base,
            Self::Linear { total_updates } => {
                let progress = (update_index as f64 / total_updates.max(1) as f64).min(1.0);
                base * (1.0 - progress) as f32
            }
        }
    }
}

/// Hyperparameters for PPO training of gemstone optimizer and retarget policies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PpoConfig {
    /// Learning rate for Adam optimizer (peak value when a decaying schedule is used).
    pub learning_rate: f32,
    /// Learning-rate schedule over optimizer updates.
    #[serde(default)]
    pub lr_schedule: LrSchedule,
    /// Discount factor `gamma` for future rewards.
    pub gamma: f32,
    /// GAE bias-variance trade-off parameter `lambda`.
    pub gae_lambda: f32,
    /// PPO policy clipping threshold `epsilon` (typically 0.2).
    pub clip_epsilon: f32,
    /// Value function loss coefficient in combined objective.
    pub value_loss_coeff: f32,
    /// Entropy bonus coefficient to encourage exploration.
    ///
    /// The entropy is the **mean** over a sample's free tiers (not the sum), so the coefficient means
    /// the same on a 4-tier and a 24-tier stone. The log-std is state-independent now, which is why the
    /// default is small: the bonus only guards against an early collapse of the three values.
    pub entropy_coeff: f32,
    /// Number of training epochs per rollout batch.
    pub epochs_per_batch: usize,
    /// Mini-batch size for SGD updates.
    pub mini_batch_size: usize,
    /// Maximum **global** gradient L2 norm (over every parameter at once); a larger gradient is
    /// rescaled to this norm before the Adam step. The pre-clip norm is reported in
    /// [`TrainingMetrics::grad_norm`].
    pub max_grad_norm: f32,
    /// Target KL divergence; the epoch loop stops after the first mini-batch whose approximate KL
    /// exceeds `1.5 * target_kl`, so a bad epoch cannot run to its end.
    pub target_kl: f32,
}

impl Default for PpoConfig {
    fn default() -> Self {
        Self {
            learning_rate: 1e-4,
            lr_schedule: LrSchedule::Constant,
            gamma: 0.99,
            gae_lambda: 0.95,
            clip_epsilon: 0.2,
            value_loss_coeff: 0.5,
            entropy_coeff: 0.002,
            epochs_per_batch: 8,
            mini_batch_size: 128,
            max_grad_norm: 0.5,
            target_kl: 0.02,
        }
    }
}

impl PpoConfig {
    /// Reject out-of-range hyperparameters instead of silently clamping them.
    ///
    /// # Errors
    ///
    /// Returns [`TrainError::InvalidConfig`] naming the first offending field.
    pub fn validate(&self) -> Result<(), TrainError> {
        fn positive(name: &str, v: f32) -> Result<(), TrainError> {
            if v.is_finite() && v > 0.0 {
                Ok(())
            } else {
                Err(TrainError::InvalidConfig(format!(
                    "{name} must be finite and > 0 (got {v})"
                )))
            }
        }
        fn unit(name: &str, v: f32) -> Result<(), TrainError> {
            if v.is_finite() && (0.0..=1.0).contains(&v) {
                Ok(())
            } else {
                Err(TrainError::InvalidConfig(format!(
                    "{name} must lie in [0, 1] (got {v})"
                )))
            }
        }
        positive("learning_rate", self.learning_rate)?;
        unit("gamma", self.gamma)?;
        unit("gae_lambda", self.gae_lambda)?;
        positive("clip_epsilon", self.clip_epsilon)?;
        if self.clip_epsilon >= 1.0 {
            return Err(TrainError::InvalidConfig(format!(
                "clip_epsilon must be < 1 (got {})",
                self.clip_epsilon
            )));
        }
        if !(self.value_loss_coeff.is_finite() && self.value_loss_coeff >= 0.0) {
            return Err(TrainError::InvalidConfig(
                "value_loss_coeff must be finite and >= 0".into(),
            ));
        }
        if !(self.entropy_coeff.is_finite() && self.entropy_coeff >= 0.0) {
            return Err(TrainError::InvalidConfig(
                "entropy_coeff must be finite and >= 0".into(),
            ));
        }
        if self.epochs_per_batch == 0 {
            return Err(TrainError::InvalidConfig(
                "epochs_per_batch must be >= 1".into(),
            ));
        }
        if self.mini_batch_size == 0 {
            return Err(TrainError::InvalidConfig(
                "mini_batch_size must be >= 1".into(),
            ));
        }
        positive("max_grad_norm", self.max_grad_norm)?;
        positive("target_kl", self.target_kl)?;
        if let LrSchedule::Linear { total_updates } = self.lr_schedule
            && total_updates == 0
        {
            return Err(TrainError::InvalidConfig(
                "lr_schedule total_updates must be >= 1".into(),
            ));
        }
        Ok(())
    }
}

/// Mean PPO losses and cumulative host phase timings for one training call.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrainingMetrics {
    /// Mean policy clipping loss.
    pub policy_loss: f32,
    /// Mean critic value MSE loss.
    pub value_loss: f32,
    /// Mean policy entropy per free tier (real, unpinned, non-flat, inside their envelope), averaged
    /// over the samples.
    pub entropy: f32,
    /// Approximate KL divergence between old and updated policy.
    pub approx_kl: f32,
    /// Fraction of samples triggering clip threshold.
    pub clip_fraction: f32,
    /// Whether the epoch loop stopped early because a mini-batch's KL exceeded `1.5 * target_kl`.
    pub early_stopped: bool,
    /// Mean pre-clip global gradient L2 norm over the optimizer updates of the call.
    #[serde(default)]
    pub grad_norm: f32,
    /// Cumulative host elapsed milliseconds spent packing PPO mini-batches during this call.
    /// This is not device-kernel time; asynchronous device work may spill into later phases.
    #[serde(default)]
    pub packing_ms: f32,
    /// Cumulative host elapsed milliseconds in PPO update bodies after packing, including device
    /// synchronization at existing statistics/gradient-guard readbacks. Not GPU utilization.
    #[serde(default)]
    pub update_ms: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_schedule_decays_to_zero() {
        let s = LrSchedule::Linear { total_updates: 10 };
        assert!((s.learning_rate(1.0, 0) - 1.0).abs() < 1e-6);
        assert!((s.learning_rate(1.0, 5) - 0.5).abs() < 1e-6);
        assert_eq!(s.learning_rate(1.0, 10), 0.0);
        assert_eq!(s.learning_rate(1.0, 99), 0.0);
        assert_eq!(LrSchedule::Constant.learning_rate(0.25, 1234), 0.25);
    }

    #[test]
    fn default_config_is_valid_and_bad_values_are_rejected() {
        assert!(PpoConfig::default().validate().is_ok());
        let bad = PpoConfig {
            epochs_per_batch: 0,
            ..PpoConfig::default()
        };
        assert!(bad.validate().is_err());
        let bad = PpoConfig {
            learning_rate: f32::NAN,
            ..PpoConfig::default()
        };
        assert!(bad.validate().is_err());
        let bad = PpoConfig {
            lr_schedule: LrSchedule::Linear { total_updates: 0 },
            ..PpoConfig::default()
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn defaults_are_the_documented_ones() {
        let c = PpoConfig::default();
        assert_eq!(c.learning_rate, 1e-4);
        assert_eq!(c.mini_batch_size, 128);
        assert_eq!(c.entropy_coeff, 0.002);
        assert_eq!(c.target_kl, 0.02);
        assert_eq!(c.epochs_per_batch, 8);
        assert_eq!(c.max_grad_norm, 0.5);
        assert_eq!(c.value_loss_coeff, 0.5);
        assert_eq!(c.clip_epsilon, 0.2);
        assert_eq!(c.gamma, 0.99);
        assert_eq!(c.gae_lambda, 0.95);
    }

    #[test]
    fn loss_coefficients_must_be_finite_and_non_negative() {
        for bad in [-1e-3, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let value = PpoConfig {
                value_loss_coeff: bad,
                ..PpoConfig::default()
            };
            let entropy = PpoConfig {
                entropy_coeff: bad,
                ..PpoConfig::default()
            };
            assert!(value.validate().is_err(), "value {bad}");
            assert!(entropy.validate().is_err(), "entropy {bad}");
        }
        // Zero switches a term off and is valid.
        let off = PpoConfig {
            value_loss_coeff: 0.0,
            entropy_coeff: 0.0,
            ..PpoConfig::default()
        };
        assert!(off.validate().is_ok());
    }

    #[test]
    fn the_removed_loss_terms_are_gone_from_the_serialised_config_and_metrics() {
        let config = serde_json::to_value(PpoConfig::default()).unwrap();
        let fields = config.as_object().unwrap();
        assert!(!fields.contains_key("pre_tanh_penalty_coeff"));
        assert!(!fields.contains_key("teacher_coeff"));
        let metrics = serde_json::to_value(TrainingMetrics::default()).unwrap();
        let fields = metrics.as_object().unwrap();
        assert!(!fields.contains_key("pre_tanh_sq"));
        assert!(!fields.contains_key("teacher_loss"));
    }

    #[test]
    fn an_older_json_config_with_the_removed_fields_still_parses() {
        // Configs written before the removal carry the two extra keys: serde ignores them.
        let mut value = serde_json::to_value(PpoConfig::default()).unwrap();
        let fields = value.as_object_mut().unwrap();
        fields.insert("pre_tanh_penalty_coeff".to_owned(), serde_json::json!(1e-3));
        fields.insert("teacher_coeff".to_owned(), serde_json::json!(0.5));
        let parsed: PpoConfig = serde_json::from_value(value).unwrap();
        assert_eq!(parsed, PpoConfig::default());

        // So do the metrics of an older run, with or without the optional `grad_norm`.
        let old_metrics = serde_json::json!({
            "policy_loss": 0.1,
            "value_loss": 0.2,
            "entropy": 0.3,
            "approx_kl": 0.01,
            "clip_fraction": 0.05,
            "early_stopped": false,
            "grad_norm": 0.4,
            "pre_tanh_sq": 0.6,
            "teacher_loss": 0.7,
        });
        let parsed: TrainingMetrics = serde_json::from_value(old_metrics).unwrap();
        assert_eq!(parsed.policy_loss, 0.1);
        assert_eq!(parsed.grad_norm, 0.4);
        assert_eq!((parsed.packing_ms, parsed.update_ms), (0.0, 0.0));
        let bare = serde_json::json!({
            "policy_loss": 0.1,
            "value_loss": 0.2,
            "entropy": 0.3,
            "approx_kl": 0.01,
            "clip_fraction": 0.05,
            "early_stopped": true,
        });
        let parsed: TrainingMetrics = serde_json::from_value(bare).unwrap();
        assert!(parsed.early_stopped);
        assert_eq!(parsed.grad_norm, 0.0);
        assert_eq!((parsed.packing_ms, parsed.update_ms), (0.0, 0.0));
    }

    #[test]
    fn a_config_without_a_schedule_still_parses_as_constant() {
        let mut value = serde_json::to_value(PpoConfig::default()).unwrap();
        assert!(
            value
                .as_object_mut()
                .unwrap()
                .remove("lr_schedule")
                .is_some()
        );
        let parsed: PpoConfig = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.lr_schedule, LrSchedule::Constant);
    }
}
