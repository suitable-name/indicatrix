//! Serialization, persistence, and loading of neural network weights.
//!
//! Bare-network JSON (`save_json`, `load_json`, `to_json_string`, `from_json_str`) always validates the
//! network structure on load. Training runs persist a [`Checkpoint`](super::checkpoint::Checkpoint) instead,
//! which wraps the network in a schema-versioned header.

use super::{GemstoneSetAttentionNetwork, checkpoint::TrainingMode};
use crate::env::N_COMPONENTS;
use std::{
    fmt,
    io::{Read, Write},
};

/// Error encountered during model weight serialization, validation or loading.
#[derive(Debug)]
pub enum WeightError {
    /// IO error during read/write.
    Io(std::io::Error),
    /// JSON format error.
    Json(serde_json::Error),
    /// A parameter vector or layer width disagrees with the declared architecture.
    DimensionMismatch {
        /// Which parameter disagreed (for example `attn_layers[2].q_proj.weights`).
        what: String,
        /// Expected length / width.
        expected: usize,
        /// Length / width found in the data.
        got: usize,
    },
    /// The network or checkpoint violates a structural rule (zero heads, non-finite weights, ...).
    Invalid(String),
    /// The checkpoint was written with a different schema version.
    UnsupportedSchema {
        /// Schema version this build reads.
        expected: u32,
        /// Schema version found in the file.
        got: u32,
    },
    /// The checkpoint was trained against a different observation layout.
    ObsLayoutMismatch {
        /// Layout version the caller expects.
        expected: u32,
        /// Layout version found in the file.
        got: u32,
    },
    /// The checkpoint belongs to a different training mode.
    ModeMismatch {
        /// Mode the caller expects.
        expected: TrainingMode,
        /// Mode found in the file.
        got: TrainingMode,
    },
}

impl fmt::Display for WeightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {e}"),
            Self::Json(e) => write!(f, "JSON error: {e}"),
            Self::DimensionMismatch {
                what,
                expected,
                got,
            } => {
                write!(
                    f,
                    "dimension mismatch in {what}: expected {expected}, got {got}"
                )
            }
            Self::Invalid(msg) => write!(f, "invalid network: {msg}"),
            Self::UnsupportedSchema { expected, got } => {
                write!(
                    f,
                    "unsupported checkpoint schema version {got} (this build reads {expected})"
                )
            }
            Self::ObsLayoutMismatch { expected, got } => {
                write!(
                    f,
                    "checkpoint observation layout version {got} does not match expected {expected}"
                )
            }
            Self::ModeMismatch { expected, got } => {
                write!(f, "checkpoint is a {got} model, expected {expected}")
            }
        }
    }
}

impl std::error::Error for WeightError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for WeightError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for WeightError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl GemstoneSetAttentionNetwork {
    /// Check every layer shape against the network's declared `d_model`, `num_heads`, `d_tier` and
    /// `d_global`, and reject non-finite parameters. Called by every loader; a network that passes cannot
    /// make `forward` index out of bounds.
    ///
    /// The heads must be the critic (`d_model -> 1`), the sensitivity head (`d_model ->
    /// N_COMPONENTS`) and the gain head (`d_model -> 1`).
    ///
    /// # Errors
    /// [`WeightError::DimensionMismatch`] or [`WeightError::Invalid`] naming the first offending layer.
    pub fn validate(&self) -> Result<(), WeightError> {
        if self.d_model == 0 || self.num_heads == 0 || self.d_tier == 0 || self.d_global == 0 {
            return Err(WeightError::Invalid("zero-sized dimension".to_owned()));
        }
        if !self.d_model.is_multiple_of(self.num_heads) {
            return Err(WeightError::Invalid(format!(
                "d_model {} is not divisible by num_heads {}",
                self.d_model, self.num_heads
            )));
        }
        let layers = self.attn_layers.len();
        for (label, len) in [
            ("pre_attn_norms", self.pre_attn_norms.len()),
            ("post_attn_norms", self.post_attn_norms.len()),
            ("mlp_blocks", self.mlp_blocks.len()),
        ] {
            if len != layers {
                return Err(WeightError::DimensionMismatch {
                    what: format!("{label}.len()"),
                    expected: layers,
                    got: len,
                });
            }
        }

        check_shape("tier_proj", &self.tier_proj, self.d_tier, self.d_model)?;
        check_shape(
            "global_proj",
            &self.global_proj,
            self.d_global,
            self.d_model,
        )?;
        check_shape("critic_head", &self.critic_head, self.d_model, 1)?;
        check_shape("sens_head", &self.sens_head, self.d_model, N_COMPONENTS)?;
        check_shape("gain_head", &self.gain_head, self.d_model, 1)?;
        self.final_norm.validate("final_norm", self.d_model)?;
        if let Some((slot, value)) = self
            .log_std_raw
            .iter()
            .enumerate()
            .find(|(_, v)| !v.is_finite())
        {
            return Err(WeightError::Invalid(format!(
                "log_std_raw[{slot}] is not finite ({value})"
            )));
        }

        for i in 0..layers {
            self.attn_layers[i].validate(
                &format!("attn_layers[{i}]"),
                self.d_model,
                self.num_heads,
            )?;
            self.pre_attn_norms[i].validate(&format!("pre_attn_norms[{i}]"), self.d_model)?;
            self.post_attn_norms[i].validate(&format!("post_attn_norms[{i}]"), self.d_model)?;

            let mlp = &self.mlp_blocks[i];
            let name = format!("mlp_blocks[{i}]");
            mlp.fc1.validate(&format!("{name}.fc1"))?;
            mlp.fc2.validate(&format!("{name}.fc2"))?;
            if mlp.fc1.in_features != self.d_model
                || mlp.fc2.out_features != self.d_model
                || mlp.fc1.out_features != mlp.fc2.in_features
            {
                return Err(WeightError::Invalid(format!(
                    "{name}: fc1/fc2 widths do not chain {} -> hidden -> {}",
                    self.d_model, self.d_model
                )));
            }
        }
        Ok(())
    }

    /// Save the bare network as compact JSON to a writer.
    ///
    /// # Errors
    /// [`WeightError::Json`] / [`WeightError::Io`] on failure.
    pub fn save_json<W: Write>(&self, mut writer: W) -> Result<(), WeightError> {
        serde_json::to_writer(&mut writer, self)?;
        Ok(())
    }

    /// Load and validate a bare network from a JSON reader.
    ///
    /// # Errors
    /// Parse errors, or any [`validate`](Self::validate) failure.
    pub fn load_json<R: Read>(reader: R) -> Result<Self, WeightError> {
        let model: Self = serde_json::from_reader(reader)?;
        model.validate()?;
        Ok(model)
    }

    /// Serialize the bare network as compact JSON.
    ///
    /// # Errors
    /// [`WeightError::Json`] if serialization fails (it does not return an empty string).
    pub fn to_json_string(&self) -> Result<String, WeightError> {
        Ok(serde_json::to_string(self)?)
    }

    /// Load and validate a bare network from a JSON string.
    ///
    /// # Errors
    /// Parse errors, or any [`validate`](Self::validate) failure.
    pub fn from_json_str(s: &str) -> Result<Self, WeightError> {
        let model: Self = serde_json::from_str(s)?;
        model.validate()?;
        Ok(model)
    }
}

fn check_shape(
    name: &str,
    layer: &super::Linear,
    in_features: usize,
    out_features: usize,
) -> Result<(), WeightError> {
    layer.validate(name)?;
    if layer.in_features != in_features || layer.out_features != out_features {
        return Err(WeightError::Invalid(format!(
            "{name}: expected a {in_features}->{out_features} layer, found {}->{}",
            layer.in_features, layer.out_features
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        env::{GLOBAL_DIM, TierFeature},
        model::{Linear, ModelTier},
    };

    #[test]
    fn a_fresh_network_validates() {
        assert!(ModelTier::Small.init_network(1).validate().is_ok());
    }

    #[test]
    fn validate_names_a_malformed_sens_head() {
        // Truncated weights.
        let mut net = ModelTier::Small.init_network(1);
        net.sens_head.weights.pop();
        let err = net.validate().unwrap_err();
        assert!(
            matches!(err, WeightError::DimensionMismatch { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("sens_head.weights"), "{err}");

        // A consistent layer of the wrong width: three components instead of four.
        let mut net = ModelTier::Small.init_network(1);
        net.sens_head = Linear::init_xavier(net.d_model, N_COMPONENTS - 1, 4);
        let err = net.validate().unwrap_err();
        assert!(matches!(err, WeightError::Invalid(_)), "{err}");
        assert!(
            err.to_string().contains(&format!(
                "sens_head: expected a {}->{N_COMPONENTS} layer",
                net.d_model
            )),
            "{err}"
        );

        // A wrong input width.
        let mut net = ModelTier::Small.init_network(1);
        net.sens_head = Linear::init_xavier(net.d_model + 1, N_COMPONENTS, 4);
        assert!(
            net.validate()
                .unwrap_err()
                .to_string()
                .contains("sens_head")
        );

        // A non-finite bias.
        let mut net = ModelTier::Small.init_network(1);
        net.sens_head.biases[2] = f32::NAN;
        let err = net.validate().unwrap_err();
        assert!(matches!(err, WeightError::Invalid(_)), "{err}");
        assert!(err.to_string().contains("sens_head"), "{err}");
    }

    #[test]
    fn validate_names_a_malformed_gain_head() {
        let mut net = ModelTier::Small.init_network(1);
        net.gain_head.weights.pop();
        let err = net.validate().unwrap_err();
        assert!(
            matches!(err, WeightError::DimensionMismatch { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("gain_head.weights"), "{err}");

        // The gain is one number: a two-wide head is refused.
        let mut net = ModelTier::Small.init_network(1);
        net.gain_head = Linear::init_xavier(net.d_model, 2, 4);
        let err = net.validate().unwrap_err();
        assert!(matches!(err, WeightError::Invalid(_)), "{err}");
        assert!(
            err.to_string()
                .contains(&format!("gain_head: expected a {}->1 layer", net.d_model)),
            "{err}"
        );
    }

    #[test]
    fn the_json_round_trip_keeps_the_new_heads_and_the_forward_pass() {
        let net = ModelTier::Small.init_network(3);
        let json = net.to_json_string().unwrap();
        assert!(json.contains("\"sens_head\"") && json.contains("\"gain_head\""));
        assert!(!json.contains("actor_mean_head"));
        let back = GemstoneSetAttentionNetwork::from_json_str(&json).unwrap();
        assert_eq!(back, net);

        let mut buffer = Vec::new();
        net.save_json(&mut buffer).unwrap();
        let loaded = GemstoneSetAttentionNetwork::load_json(buffer.as_slice()).unwrap();
        assert_eq!(loaded, net);

        let tiers = vec![vec![0.1; TierFeature::FEATURE_DIM]; 3];
        let global = vec![0.2; GLOBAL_DIM];
        assert_eq!(
            loaded.forward_full(&tiers, &global).unwrap(),
            net.forward_full(&tiers, &global).unwrap()
        );
    }

    #[test]
    fn a_network_json_of_the_old_head_layout_is_refused() {
        let net = ModelTier::Small.init_network(3);
        let mut value = serde_json::to_value(&net).unwrap();
        let fields = value.as_object_mut().unwrap();
        assert!(fields.remove("sens_head").is_some());
        assert!(fields.remove("gain_head").is_some());
        fields.insert(
            "actor_mean_head".to_owned(),
            serde_json::to_value(Linear::init_xavier(net.d_model, 1, 9)).unwrap(),
        );
        let err = GemstoneSetAttentionNetwork::from_json_str(&value.to_string()).unwrap_err();
        assert!(matches!(err, WeightError::Json(_)), "{err}");
        assert!(err.to_string().contains("sens_head"), "{err}");
    }

    #[test]
    fn validate_names_a_malformed_final_norm() {
        let mut net = ModelTier::Small.init_network(1);
        net.final_norm.gamma.pop();
        let err = net.validate().unwrap_err();
        assert!(
            matches!(err, WeightError::DimensionMismatch { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("final_norm.gamma"), "{err}");

        let mut net = ModelTier::Small.init_network(1);
        net.final_norm.beta[0] = f32::NAN;
        let err = net.validate().unwrap_err();
        assert!(err.to_string().contains("final_norm"), "{err}");
    }

    #[test]
    fn validate_rejects_a_non_finite_log_std_raw_and_names_the_slot() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut net = ModelTier::Small.init_network(1);
            net.log_std_raw[1] = bad;
            let err = net.validate().unwrap_err();
            assert!(matches!(err, WeightError::Invalid(_)), "{err}");
            assert!(err.to_string().contains("log_std_raw[1]"), "{err}");
        }
    }
}
