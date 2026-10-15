//! Schema-versioned training checkpoints.
//!
//! A [`Checkpoint`] wraps the trained [`GemstoneSetAttentionNetwork`] with the metadata needed to detect
//! mismatches at load time: which training mode and model tier produced it, which observation layout it
//! was trained against, and how far training had progressed. The Adam optimiser state is not persisted;
//! if a Burn recorder is adopted later, the model and optimiser records must use distinct base names
//! (`<run>-model.mpk`, `<run>-optim.mpk`), never names derived by swapping an extension.
//!
//! # Schema history
//!
//! - 1: per-tier `actor_log_std_head`, no final norm. Refused since version 2 (the policy it holds
//!   never trained: the head sat on its clamp).
//! - 2: state-independent `log_std_raw` (crown, pavilion, other) and a `final_norm` before the heads.
//!   Refused since version 3 (its policy head predicted a tanh mean directly and cannot be composed
//!   from sensitivities).
//! - 3: the `actor_mean_head` is replaced by a sensitivity head (`sens_head`, `d_model ->
//!   N_COMPONENTS`, one prediction per tier) and a `gain_head` (`d_model -> 1`, on the pooled vector
//!   the critic reads); the policy mean is composed from the sensitivities and the objective's weight
//!   fractions, which the global token carries since observation layout 4 (`GLOBAL_DIM` 19).
//!
//! The file's schema version is read before the network is parsed, so an old file is refused with
//! [`WeightError::UnsupportedSchema`] rather than a "missing field" JSON error.

use super::{
    actor_critic::{GemstoneSetAttentionNetwork, ModelTier},
    weights::WeightError,
};
use crate::env::{N_COMPONENTS, WEIGHT_FRACTION_INDEX};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fmt,
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

/// Schema version written by [`Checkpoint::save`] and required by [`Checkpoint::load`].
pub const CHECKPOINT_SCHEMA_VERSION: u32 = 3;

/// The one field of a checkpoint file that is read before the rest, so a file of another schema is
/// refused by version and not by whichever field its network happens to be missing.
#[derive(Deserialize)]
struct SchemaProbe {
    schema_version: u32,
}

/// Which task a model was trained for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrainingMode {
    /// Angle optimisation within one material.
    Optimizer,
    /// Retargeting a design from one material to another.
    Retarget,
}

impl fmt::Display for TrainingMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Optimizer => "optimizer",
            Self::Retarget => "retarget",
        })
    }
}

/// A trained network plus the metadata needed to load it safely.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Always [`CHECKPOINT_SCHEMA_VERSION`] for files written by this build.
    pub schema_version: u32,
    /// Task the network was trained for.
    pub mode: TrainingMode,
    /// Size tier the network was built from.
    pub tier: ModelTier,
    /// Observation layout version (`env::observation::OBS_LAYOUT_VERSION`) the network was trained against.
    pub obs_layout_version: u32,
    /// Number of tier slots the batched training pass was padded to.
    pub max_tiers: usize,
    /// PPO updates completed when the checkpoint was written.
    pub iteration: u64,
    /// Seed of the training run.
    pub seed: u64,
    /// The trained network.
    pub network: GemstoneSetAttentionNetwork,
}

impl Checkpoint {
    /// Assemble a checkpoint at the current schema version.
    #[must_use]
    pub const fn new(
        mode: TrainingMode,
        tier: ModelTier,
        obs_layout_version: u32,
        max_tiers: usize,
        iteration: u64,
        seed: u64,
        network: GemstoneSetAttentionNetwork,
    ) -> Self {
        Self {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            mode,
            tier,
            obs_layout_version,
            max_tiers,
            iteration,
            seed,
            network,
        }
    }

    /// Write the checkpoint as compact JSON to `<path>.tmp` and then rename it over `path`, so an
    /// interrupted save never leaves a truncated checkpoint behind.
    ///
    /// # Errors
    /// [`WeightError::Io`] / [`WeightError::Json`]; the temporary file is removed on failure.
    pub fn save(&self, path: &Path) -> Result<(), WeightError> {
        let mut tmp_name: OsString = path
            .file_name()
            .ok_or_else(|| WeightError::Invalid("checkpoint path has no file name".to_owned()))?
            .to_owned();
        tmp_name.push(".tmp");
        let tmp_path = path.with_file_name(tmp_name);

        let write = || -> Result<(), WeightError> {
            let file = File::create(&tmp_path)?;
            let mut writer = BufWriter::new(file);
            serde_json::to_writer(&mut writer, self)?;
            writer.flush()?;
            writer.get_ref().sync_all()?;
            Ok(())
        };
        if let Err(e) = write() {
            // Best effort: the original error is the one worth reporting.
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&tmp_path, path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e.into());
        }
        Ok(())
    }

    /// Load a checkpoint, rejecting a wrong schema version (checked before the network is parsed), a
    /// different observation layout, and any network that fails
    /// [`GemstoneSetAttentionNetwork::validate`] (which covers the sensitivity and gain heads),
    /// cannot carry the objective's weight fractions in its global token, or disagrees with its
    /// recorded tier.
    ///
    /// # Errors
    /// [`WeightError::UnsupportedSchema`], [`WeightError::ObsLayoutMismatch`], validation errors, or
    /// IO / JSON errors.
    pub fn load(path: &Path, expected_obs_layout_version: u32) -> Result<Self, WeightError> {
        let bytes = std::fs::read(path)?;
        let probe: SchemaProbe = serde_json::from_slice(&bytes)?;
        if probe.schema_version != CHECKPOINT_SCHEMA_VERSION {
            return Err(WeightError::UnsupportedSchema {
                expected: CHECKPOINT_SCHEMA_VERSION,
                got: probe.schema_version,
            });
        }
        let checkpoint: Self = serde_json::from_slice(&bytes)?;
        if checkpoint.obs_layout_version != expected_obs_layout_version {
            return Err(WeightError::ObsLayoutMismatch {
                expected: expected_obs_layout_version,
                got: checkpoint.obs_layout_version,
            });
        }
        checkpoint.network.validate()?;

        let (d_model, num_heads, num_layers) = checkpoint.tier.config();
        let net = &checkpoint.network;
        // The composed mean reads the weight fractions from the global token: a network too narrow
        // to carry them can never run, whatever its layout tag says.
        let needed_global = WEIGHT_FRACTION_INDEX + N_COMPONENTS;
        if net.d_global < needed_global {
            return Err(WeightError::Invalid(format!(
                "network global width {} cannot carry the {N_COMPONENTS} weight fractions at \
                 entry {WEIGHT_FRACTION_INDEX} (needs at least {needed_global})",
                net.d_global
            )));
        }
        if net.d_model != d_model
            || net.num_heads != num_heads
            || net.attn_layers.len() != num_layers
        {
            return Err(WeightError::Invalid(format!(
                "network ({}x{} heads, {} layers) does not match tier {}",
                net.d_model,
                net.num_heads,
                net.attn_layers.len(),
                checkpoint.tier.code_str()
            )));
        }
        Ok(checkpoint)
    }

    /// Reject the checkpoint unless it was trained for `expected`.
    ///
    /// # Errors
    /// [`WeightError::ModeMismatch`].
    pub fn ensure_mode(&self, expected: TrainingMode) -> Result<(), WeightError> {
        if self.mode == expected {
            Ok(())
        } else {
            Err(WeightError::ModeMismatch {
                expected,
                got: self.mode,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{env::TierFeature, model::Linear};

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("indicatrix-ai-{}-{name}", std::process::id()))
    }

    fn small_checkpoint() -> Checkpoint {
        let tier = ModelTier::Small;
        Checkpoint::new(
            TrainingMode::Optimizer,
            tier,
            3,
            24,
            17,
            99,
            tier.init_network(5),
        )
    }

    #[test]
    fn round_trip_preserves_everything() {
        let path = temp_path("roundtrip.json");
        let original = small_checkpoint();
        original.save(&path).unwrap();
        let loaded = Checkpoint::load(&path, 3).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(original, loaded);
    }

    #[test]
    fn layout_and_mode_mismatches_are_rejected() {
        let path = temp_path("mismatch.json");
        small_checkpoint().save(&path).unwrap();
        let layout = Checkpoint::load(&path, 4);
        let loaded = Checkpoint::load(&path, 3).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            layout,
            Err(WeightError::ObsLayoutMismatch {
                expected: 4,
                got: 3
            })
        ));
        assert!(matches!(
            loaded.ensure_mode(TrainingMode::Retarget),
            Err(WeightError::ModeMismatch { .. })
        ));
        assert!(loaded.ensure_mode(TrainingMode::Optimizer).is_ok());
    }

    #[test]
    fn wrong_schema_version_is_rejected() {
        let path = temp_path("schema.json");
        let mut checkpoint = small_checkpoint();
        checkpoint.schema_version = CHECKPOINT_SCHEMA_VERSION + 1;
        checkpoint.save(&path).unwrap();
        let result = Checkpoint::load(&path, 3);
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(result, Err(WeightError::UnsupportedSchema { .. })));
    }

    #[test]
    fn schema_version_three_is_written_with_the_new_heads() {
        assert_eq!(CHECKPOINT_SCHEMA_VERSION, 3);
        assert_eq!(small_checkpoint().schema_version, 3);
        let path = temp_path("schema3.json");
        small_checkpoint().save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema_version"], 3);
        let network = value["network"].as_object().unwrap();
        assert!(network.contains_key("sens_head") && network.contains_key("gain_head"));
        assert!(!network.contains_key("actor_mean_head"));
    }

    #[test]
    fn a_version_two_file_is_refused_by_its_schema_version() {
        let path = temp_path("v2.json");
        // A version-2 network had an `actor_mean_head` and neither `sens_head` nor `gain_head`:
        // rewrite a fresh file that way.
        let mut value = serde_json::to_value(small_checkpoint()).unwrap();
        value["schema_version"] = serde_json::Value::from(2u32);
        let network = value["network"].as_object_mut().unwrap();
        assert!(network.remove("sens_head").is_some());
        assert!(network.remove("gain_head").is_some());
        let d_model = ModelTier::Small.config().0;
        network.insert(
            "actor_mean_head".to_owned(),
            serde_json::to_value(Linear::init_xavier(d_model, 1, 3)).unwrap(),
        );
        std::fs::write(&path, value.to_string()).unwrap();
        let result = Checkpoint::load(&path, 3);
        std::fs::remove_file(&path).unwrap();
        assert!(
            matches!(
                result,
                Err(WeightError::UnsupportedSchema {
                    expected: 3,
                    got: 2
                })
            ),
            "{result:?}"
        );
    }

    #[test]
    fn a_network_that_cannot_carry_the_weight_fractions_is_refused() {
        let path = temp_path("narrow.json");
        let (d_model, num_heads, num_layers) = ModelTier::Small.config();
        // Shaped like the Small tier, but a global token that stops before the fractions.
        let narrow = GemstoneSetAttentionNetwork::new(
            TierFeature::FEATURE_DIM,
            WEIGHT_FRACTION_INDEX,
            d_model,
            num_heads,
            num_layers,
            4,
        )
        .unwrap();
        Checkpoint::new(
            TrainingMode::Retarget,
            ModelTier::Small,
            3,
            24,
            0,
            1,
            narrow,
        )
        .save(&path)
        .unwrap();
        let result = Checkpoint::load(&path, 3);
        std::fs::remove_file(&path).unwrap();
        let err = result.unwrap_err();
        assert!(matches!(err, WeightError::Invalid(_)), "{err}");
        assert!(err.to_string().contains("weight fractions"), "{err}");
    }

    /// Save a small checkpoint after `damage` and return the error `load` reports for it.
    fn load_error_after(name: &str, damage: impl FnOnce(&mut Checkpoint)) -> WeightError {
        let path = temp_path(name);
        let mut checkpoint = small_checkpoint();
        damage(&mut checkpoint);
        checkpoint.save(&path).unwrap();
        let result = Checkpoint::load(&path, 3);
        std::fs::remove_file(&path).unwrap();
        result.unwrap_err()
    }

    #[test]
    fn a_malformed_sensitivity_or_gain_head_is_refused_on_load() {
        let err = load_error_after("bad-sens.json", |c| {
            c.network.sens_head.biases.pop();
        });
        assert!(
            matches!(err, WeightError::DimensionMismatch { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("sens_head.biases"), "{err}");

        let err = load_error_after("bad-gain.json", |c| {
            c.network.gain_head.weights.pop();
        });
        assert!(
            matches!(err, WeightError::DimensionMismatch { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("gain_head.weights"), "{err}");
    }

    #[test]
    fn truncated_weights_are_rejected_on_load() {
        let path = temp_path("truncated.json");
        let mut checkpoint = small_checkpoint();
        checkpoint.network.tier_proj.weights.pop();
        checkpoint.save(&path).unwrap();
        let result = Checkpoint::load(&path, 3);
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(result, Err(WeightError::DimensionMismatch { .. })));
    }
}
