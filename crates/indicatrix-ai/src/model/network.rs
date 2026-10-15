//! Neural network primitives, dense layers, and attention mechanisms for geometric policy networks.
//!
//! Every fallible operation returns a `Result`: a dimension disagreement between a layer and its input
//! (or between a layer's own fields, for example after hand-editing a checkpoint) is an error, never a panic.

use super::{rng::Prng, weights::WeightError};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Error raised by a forward pass whose inputs or layer shapes disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    /// A vector had the wrong length for the layer it was fed to.
    Dimension {
        /// Which input or parameter disagreed.
        what: &'static str,
        /// Length the layer expects.
        expected: usize,
        /// Length that was supplied.
        got: usize,
    },
    /// The layer configuration itself is inconsistent (zero heads, non-divisible width, ...).
    Structure(&'static str),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dimension {
                what,
                expected,
                got,
            } => {
                write!(
                    f,
                    "dimension mismatch in {what}: expected {expected}, got {got}"
                )
            }
            Self::Structure(msg) => write!(f, "inconsistent network structure: {msg}"),
        }
    }
}

impl std::error::Error for ModelError {}

/// Linear (dense) fully-connected layer with weights and biases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Linear {
    /// Weight matrix flattened in row-major order: `[out_features, in_features]`.
    pub weights: Vec<f32>,
    /// Bias vector: `[out_features]`.
    pub biases: Vec<f32>,
    /// Input dimension.
    pub in_features: usize,
    /// Output dimension.
    pub out_features: usize,
}

impl Linear {
    /// Construct a linear layer from explicit weights and biases.
    ///
    /// # Errors
    /// [`WeightError::DimensionMismatch`] when `weights.len() != in_features * out_features` or
    /// `biases.len() != out_features`, and [`WeightError::Invalid`] when a dimension is zero.
    pub fn new(
        in_features: usize,
        out_features: usize,
        weights: Vec<f32>,
        biases: Vec<f32>,
    ) -> Result<Self, WeightError> {
        let layer = Self {
            weights,
            biases,
            in_features,
            out_features,
        };
        layer.validate("linear")?;
        Ok(layer)
    }

    /// Check the layer's internal consistency (`name` labels the layer in error messages).
    ///
    /// # Errors
    /// [`WeightError::Invalid`] for zero dimensions or non-finite parameters,
    /// [`WeightError::DimensionMismatch`] when the parameter vectors disagree with the declared shape.
    pub fn validate(&self, name: &str) -> Result<(), WeightError> {
        if self.in_features == 0 || self.out_features == 0 {
            return Err(WeightError::Invalid(format!("{name}: zero-sized layer")));
        }
        let expected_weights =
            self.in_features
                .checked_mul(self.out_features)
                .ok_or_else(|| {
                    WeightError::Invalid(format!("{name}: in_features * out_features overflows"))
                })?;
        if self.weights.len() != expected_weights {
            return Err(WeightError::DimensionMismatch {
                what: format!("{name}.weights"),
                expected: expected_weights,
                got: self.weights.len(),
            });
        }
        if self.biases.len() != self.out_features {
            return Err(WeightError::DimensionMismatch {
                what: format!("{name}.biases"),
                expected: self.out_features,
                got: self.biases.len(),
            });
        }
        if self
            .weights
            .iter()
            .chain(&self.biases)
            .any(|v| !v.is_finite())
        {
            return Err(WeightError::Invalid(format!(
                "{name}: non-finite parameter"
            )));
        }
        Ok(())
    }

    /// Number of trainable parameters (`weights + biases`).
    #[must_use]
    pub const fn param_count(&self) -> usize {
        self.weights.len() + self.biases.len()
    }

    /// Construct a Xavier/Glorot-initialised layer from `seed` (zero biases). Deterministic.
    #[must_use]
    pub fn init_xavier(in_features: usize, out_features: usize, seed: u64) -> Self {
        Self::init_xavier_scaled(in_features, out_features, seed, 1.0)
    }

    /// Xavier/Glorot weights multiplied by `gain` (zero biases), deterministic from `seed`.
    ///
    /// The draws and their order are those of [`Self::init_xavier`], which is this function with
    /// `gain = 1.0` (the result is bit-identical: multiplying the scale by 1.0 is exact). A small gain
    /// such as 0.01 gives the "small policy head" start: its output begins near zero.
    #[must_use]
    pub fn init_xavier_scaled(
        in_features: usize,
        out_features: usize,
        seed: u64,
        gain: f32,
    ) -> Self {
        let mut rng = Prng::new(seed);
        let scale = (6.0 / (in_features + out_features).max(1) as f32).sqrt() * gain;
        let weights = (0..in_features * out_features)
            .map(|_| rng.next_signed_f32() * scale)
            .collect();
        Self {
            weights,
            biases: vec![0.0; out_features],
            in_features,
            out_features,
        }
    }

    /// Forward pass computing `y = W * x + b`.
    ///
    /// # Errors
    /// [`ModelError::Dimension`] when `input.len() != in_features` or the parameter vectors do not match
    /// the declared shape.
    pub fn forward(&self, input: &[f32]) -> Result<Vec<f32>, ModelError> {
        if self.in_features == 0 {
            return Err(ModelError::Structure(
                "linear layer with zero input features",
            ));
        }
        if input.len() != self.in_features {
            return Err(ModelError::Dimension {
                what: "linear input",
                expected: self.in_features,
                got: input.len(),
            });
        }
        if self.weights.len() != self.in_features * self.out_features {
            return Err(ModelError::Dimension {
                what: "linear weights",
                expected: self.in_features * self.out_features,
                got: self.weights.len(),
            });
        }
        if self.biases.len() != self.out_features {
            return Err(ModelError::Dimension {
                what: "linear biases",
                expected: self.out_features,
                got: self.biases.len(),
            });
        }

        let output = self
            .weights
            .chunks_exact(self.in_features)
            .zip(&self.biases)
            .map(|(row, &bias)| {
                let mut sum = 0.0f32;
                for (w, x) in row.iter().zip(input) {
                    sum += w * x;
                }
                bias + sum
            })
            .collect();
        Ok(output)
    }
}

/// Gaussian Error Linear Unit (GELU), tanh approximation.
///
/// This is the same function as `burn::tensor::activation::gelu_approximate`, which the Burn training twin
/// uses; the exact (erf) GELU would make the two forward passes disagree.
#[inline]
#[must_use]
#[allow(
    clippy::suboptimal_flops,
    reason = "operation order mirrors the Burn model; the equivalence test depends on it"
)]
pub fn gelu(x: f32) -> f32 {
    0.5 * x * (1.0 + (x * 0.797_884_6 * (1.0 + 0.044_715 * x * x)).tanh())
}

/// Two-layer feed-forward block: `fc2(gelu(fc1(x)))`.
///
/// The block carries no residual connection; the caller adds the skip (pre-LN transformer layout), exactly
/// as the Burn twin does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MlpBlock {
    /// First projection layer.
    pub fc1: Linear,
    /// Second projection layer.
    pub fc2: Linear,
}

impl MlpBlock {
    /// Construct MLP block with hidden expansion dimension.
    #[must_use]
    pub fn new(dim: usize, hidden_dim: usize, seed: u64) -> Self {
        Self {
            fc1: Linear::init_xavier(dim, hidden_dim, seed),
            fc2: Linear::init_xavier(hidden_dim, dim, seed ^ 0x5555_AAAA),
        }
    }

    /// Forward pass with GELU non-linearity (no residual).
    ///
    /// # Errors
    /// Propagates [`ModelError`] from the two linear layers.
    pub fn forward(&self, x: &[f32]) -> Result<Vec<f32>, ModelError> {
        let h = self.fc1.forward(x)?;
        let h_act: Vec<f32> = h.into_iter().map(gelu).collect();
        self.fc2.forward(&h_act)
    }

    /// Number of trainable parameters.
    #[must_use]
    pub const fn param_count(&self) -> usize {
        self.fc1.param_count() + self.fc2.param_count()
    }
}

/// Layer Normalization across feature dimension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerNorm {
    /// Learnable scaling parameter.
    pub gamma: Vec<f32>,
    /// Learnable shift parameter.
    pub beta: Vec<f32>,
    /// Numerical stability epsilon.
    pub eps: f32,
}

impl LayerNorm {
    /// Construct `LayerNorm` with identity scaling and zero shift.
    #[must_use]
    pub fn new(dim: usize) -> Self {
        Self {
            gamma: vec![1.0; dim],
            beta: vec![0.0; dim],
            eps: 1e-5,
        }
    }

    /// Check the layer's internal consistency against the expected feature width `dim`.
    ///
    /// # Errors
    /// [`WeightError::DimensionMismatch`] for wrong vector lengths, [`WeightError::Invalid`] for
    /// non-finite parameters or a non-positive `eps`.
    pub fn validate(&self, name: &str, dim: usize) -> Result<(), WeightError> {
        if self.gamma.len() != dim {
            return Err(WeightError::DimensionMismatch {
                what: format!("{name}.gamma"),
                expected: dim,
                got: self.gamma.len(),
            });
        }
        if self.beta.len() != dim {
            return Err(WeightError::DimensionMismatch {
                what: format!("{name}.beta"),
                expected: dim,
                got: self.beta.len(),
            });
        }
        if !(self.eps.is_finite() && self.eps > 0.0) {
            return Err(WeightError::Invalid(format!(
                "{name}: eps must be finite and positive"
            )));
        }
        if self.gamma.iter().chain(&self.beta).any(|v| !v.is_finite()) {
            return Err(WeightError::Invalid(format!(
                "{name}: non-finite parameter"
            )));
        }
        Ok(())
    }

    /// Number of trainable parameters.
    #[must_use]
    pub const fn param_count(&self) -> usize {
        self.gamma.len() + self.beta.len()
    }

    /// Forward pass normalizing across feature dimensions.
    ///
    /// # Errors
    /// [`ModelError::Dimension`] when `x` does not match the width of `gamma`/`beta`.
    #[allow(
        clippy::suboptimal_flops,
        reason = "operation order mirrors the Burn model; the equivalence test depends on it"
    )]
    pub fn forward(&self, x: &[f32]) -> Result<Vec<f32>, ModelError> {
        if self.gamma.len() != x.len() || self.beta.len() != x.len() {
            return Err(ModelError::Dimension {
                what: "layer norm input",
                expected: self.gamma.len().max(self.beta.len()),
                got: x.len(),
            });
        }
        let n = x.len();
        if n == 0 {
            return Ok(Vec::new());
        }
        let mean = x.iter().sum::<f32>() / n as f32;
        let var = x.iter().map(|&v| (v - mean).powi(2)).sum::<f32>() / n as f32;
        let std = (var + self.eps).sqrt();

        Ok(x.iter()
            .zip(self.gamma.iter().zip(&self.beta))
            .map(|(&v, (&g, &b))| ((v - mean) / std) * g + b)
            .collect())
    }
}

/// Reusable softmax buffers for [`MultiHeadAttention::forward_with_scratch`]; they grow to the
/// token count on first use and are cleared, not freed, between calls.
#[derive(Debug, Default, Clone)]
pub struct InferenceScratch {
    scores: Vec<f32>,
    exps: Vec<f32>,
}

/// Multi-Head Self-Attention over sequence of vectors (such as gemstone tiers).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiHeadAttention {
    /// Query projection.
    pub q_proj: Linear,
    /// Key projection.
    pub k_proj: Linear,
    /// Value projection.
    pub v_proj: Linear,
    /// Output projection.
    pub out_proj: Linear,
    /// Model hidden dimension.
    pub dim: usize,
    /// Number of attention heads.
    pub num_heads: usize,
}

impl MultiHeadAttention {
    /// Construct multi-head attention module.
    ///
    /// # Errors
    /// [`WeightError::Invalid`] when `num_heads` is zero or `dim` is not divisible by `num_heads`.
    pub fn new(dim: usize, num_heads: usize, seed: u64) -> Result<Self, WeightError> {
        if num_heads == 0 || dim == 0 || !dim.is_multiple_of(num_heads) {
            return Err(WeightError::Invalid(format!(
                "attention: dim {dim} must be positive and divisible by num_heads {num_heads}"
            )));
        }
        Ok(Self::build(dim, num_heads, seed))
    }

    /// Construct without re-validating; the caller guarantees `dim > 0` and `dim % num_heads == 0`.
    pub(crate) fn build(dim: usize, num_heads: usize, seed: u64) -> Self {
        Self {
            q_proj: Linear::init_xavier(dim, dim, seed ^ 0x1111_2222),
            k_proj: Linear::init_xavier(dim, dim, seed ^ 0x3333_4444),
            v_proj: Linear::init_xavier(dim, dim, seed ^ 0x5555_6666),
            out_proj: Linear::init_xavier(dim, dim, seed ^ 0x7777_8888),
            dim,
            num_heads,
        }
    }

    /// Check the module's internal consistency against the expected model width.
    ///
    /// # Errors
    /// Propagates the layer validation errors, plus [`WeightError::Invalid`] for a head count that does
    /// not divide the width.
    pub fn validate(
        &self,
        name: &str,
        d_model: usize,
        num_heads: usize,
    ) -> Result<(), WeightError> {
        if self.dim != d_model || self.num_heads != num_heads {
            return Err(WeightError::Invalid(format!(
                "{name}: dim/heads ({}/{}) disagree with the network ({d_model}/{num_heads})",
                self.dim, self.num_heads
            )));
        }
        if self.num_heads == 0 || !self.dim.is_multiple_of(self.num_heads) {
            return Err(WeightError::Invalid(format!(
                "{name}: dim not divisible by num_heads"
            )));
        }
        for (label, layer) in [
            ("q_proj", &self.q_proj),
            ("k_proj", &self.k_proj),
            ("v_proj", &self.v_proj),
            ("out_proj", &self.out_proj),
        ] {
            let full = format!("{name}.{label}");
            layer.validate(&full)?;
            if layer.in_features != d_model || layer.out_features != d_model {
                return Err(WeightError::Invalid(format!(
                    "{full}: expected a {d_model}x{d_model} layer"
                )));
            }
        }
        Ok(())
    }

    /// Number of trainable parameters.
    #[must_use]
    pub const fn param_count(&self) -> usize {
        self.q_proj.param_count()
            + self.k_proj.param_count()
            + self.v_proj.param_count()
            + self.out_proj.param_count()
    }

    /// Forward self-attention pass over sequence of `tokens` (shape: [N, D]).
    ///
    /// # Errors
    /// [`ModelError`] when the head configuration is inconsistent or a token has the wrong width.
    #[allow(
        clippy::suboptimal_flops,
        reason = "operation order mirrors the Burn model; the equivalence test depends on it"
    )]
    pub fn forward(&self, tokens: &[Vec<f32>]) -> Result<Vec<Vec<f32>>, ModelError> {
        self.forward_with_scratch(tokens, &mut InferenceScratch::default())
    }

    /// Same as [`Self::forward`] but reuses the caller's softmax buffers, so a hot loop that
    /// calls it per observation allocates them once instead of once per token and head.
    ///
    /// # Errors
    /// [`ModelError`] when the head configuration is inconsistent or a token has the wrong width.
    #[allow(
        clippy::suboptimal_flops,
        reason = "operation order mirrors the Burn model; the equivalence test depends on it"
    )]
    pub fn forward_with_scratch(
        &self,
        tokens: &[Vec<f32>],
        scratch: &mut InferenceScratch,
    ) -> Result<Vec<Vec<f32>>, ModelError> {
        if self.num_heads == 0 || !self.dim.is_multiple_of(self.num_heads) {
            return Err(ModelError::Structure(
                "attention width is not divisible by the head count",
            ));
        }
        let n_tokens = tokens.len();
        if n_tokens == 0 {
            return Ok(Vec::new());
        }
        let head_dim = self.dim / self.num_heads;
        let scale = 1.0 / (head_dim as f32).sqrt();

        // Project Q, K, V for all tokens
        let project = |layer: &Linear| -> Result<Vec<Vec<f32>>, ModelError> {
            let projected = tokens
                .iter()
                .map(|t| layer.forward(t))
                .collect::<Result<Vec<_>, _>>()?;
            if projected.iter().any(|p| p.len() != self.dim) {
                return Err(ModelError::Dimension {
                    what: "attention projection output",
                    expected: self.dim,
                    got: layer.out_features,
                });
            }
            Ok(projected)
        };
        let queries = project(&self.q_proj)?;
        let keys = project(&self.k_proj)?;
        let values = project(&self.v_proj)?;

        let mut out = vec![vec![0.0f32; self.dim]; n_tokens];

        for h in 0..self.num_heads {
            let offset = h * head_dim;

            // Attention scores A_ij = (Q_i * K_j) * scale
            for i in 0..n_tokens {
                let q_h = &queries[i][offset..offset + head_dim];

                // Compute dot products with all K_j
                scratch.scores.clear();
                let mut max_score = f32::NEG_INFINITY;
                for kj in &keys {
                    let k_h = &kj[offset..offset + head_dim];
                    let dot = q_h.iter().zip(k_h).map(|(a, b)| a * b).sum::<f32>() * scale;
                    if dot > max_score {
                        max_score = dot;
                    }
                    scratch.scores.push(dot);
                }

                // Softmax with numerical stabilization (the maximum term is exp(0) = 1, so the sum
                // is >= 1). Each `exp` is computed once and reused for the sum and the weight.
                scratch.exps.clear();
                scratch
                    .exps
                    .extend(scratch.scores.iter().map(|&s| (s - max_score).exp()));
                let exp_sum: f32 = scratch.exps.iter().sum();

                // Weighted sum over V_j
                for (j, &e) in scratch.exps.iter().enumerate() {
                    let weight = e / exp_sum;
                    let v_h = &values[j][offset..offset + head_dim];
                    for (d, &val) in v_h.iter().enumerate() {
                        out[i][offset + d] += weight * val;
                    }
                }
            }
        }

        // Final output linear projection
        out.iter().map(|vec| self.out_proj.forward(vec)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_new_rejects_mismatched_shapes() {
        assert!(matches!(
            Linear::new(2, 3, vec![0.0; 5], vec![0.0; 3]),
            Err(WeightError::DimensionMismatch { .. })
        ));
        assert!(matches!(
            Linear::new(2, 3, vec![0.0; 6], vec![0.0; 2]),
            Err(WeightError::DimensionMismatch { .. })
        ));
        assert!(Linear::new(2, 3, vec![0.0; 6], vec![0.0; 3]).is_ok());
    }

    #[test]
    fn linear_forward_reports_wrong_input_and_truncated_weights() {
        let layer = Linear::init_xavier(4, 2, 1);
        assert!(matches!(
            layer.forward(&[0.0; 3]),
            Err(ModelError::Dimension { .. })
        ));

        let mut truncated = layer;
        truncated.weights.pop();
        assert!(matches!(
            truncated.forward(&[0.0; 4]),
            Err(ModelError::Dimension { .. })
        ));
    }

    #[test]
    fn init_xavier_keeps_its_draws_and_the_scaled_init_multiplies_the_scale() {
        // The historical formula written out: one signed draw per weight times sqrt(6 / (in + out)).
        let mut rng = Prng::new(9);
        let scale = (6.0f32 / 7.0).sqrt();
        let expected: Vec<f32> = (0..12).map(|_| rng.next_signed_f32() * scale).collect();
        assert_eq!(Linear::init_xavier(4, 3, 9).weights, expected);
        assert_eq!(Linear::init_xavier_scaled(4, 3, 9, 1.0).weights, expected);

        let small = Linear::init_xavier_scaled(4, 3, 9, 0.01);
        for (s, e) in small.weights.iter().zip(&expected) {
            assert!((s - 0.01 * e).abs() <= 1e-7, "{s} vs {}", 0.01 * e);
        }
        assert!(small.biases.iter().all(|&b| b == 0.0));
        assert!(small.weights.iter().all(|w| w.abs() <= 0.01 * scale));
    }

    #[test]
    fn linear_forward_matches_hand_computation() {
        let layer = Linear::new(2, 2, vec![1.0, 2.0, 3.0, 4.0], vec![0.5, -0.5]).unwrap();
        assert_eq!(layer.forward(&[1.0, 1.0]).unwrap(), vec![3.5, 6.5]);
    }

    #[test]
    fn attention_new_rejects_bad_head_counts() {
        assert!(MultiHeadAttention::new(10, 3, 0).is_err());
        assert!(MultiHeadAttention::new(10, 0, 0).is_err());
        assert!(MultiHeadAttention::new(12, 3, 0).is_ok());
    }

    #[test]
    fn layer_norm_rejects_wrong_width() {
        let norm = LayerNorm::new(4);
        assert!(norm.forward(&[1.0; 3]).is_err());
        assert!(norm.forward(&[1.0, 2.0, 3.0, 4.0]).is_ok());
    }
}
