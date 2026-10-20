//! Set-attention actor-critic network for continuous angle control and value estimation.
//!
//! # Heads and the composed policy mean
//!
//! The network does not predict an action. After the transformer stack and a final `LayerNorm` it
//! reads three kinds of heads:
//!
//! - **Sensitivities** (`sens_head`, `d_model -> N_COMPONENTS`, one prediction per tier): how much
//!   each objective component changes per degree of that tier, in the order windowing, extinction,
//!   brilliance loss, yield loss, lighter-tone loss, deeper-tone loss, and unweighted shape distance
//!   times 100. The values are raw head outputs (no activation).
//! - **Value and gain** (`critic_head` and `gain_head`, both `d_model -> 1`) on the same pooled
//!   vector: the context token plus the mean of the tier tokens. The value is the baseline `V(s)`;
//!   the gain is the predicted score drop (points) of the oracle step at this state.
//! - **Log-std**: three raw values (crown, pavilion, girdle or other), state-independent.
//!
//! The policy mean is composed, not predicted: the global token carries the objective's weight
//! fractions at [`WEIGHT_FRACTION_INDEX`], and [`compose_mean`] weights the sensitivities of each
//! tier with them (`g_k = sum_c frac[c] * sens[k][c]`) and normalizes by the steepest mobile tier,
//! with a documented score-slope floor so nearly flat predictions produce proportionally small
//! actions rather than an unstable full-scale direction. Above the floor the steepest tier sits at
//! `-MEAN_BOUND` or `+MEAN_BOUND` in pre-squash units. One optimizer network serves every objective preset and any
//! custom weighting. Retarget and optimizer artifacts remain distinct capabilities. The Burn twin
//! (`training::burn_model`) composes the same way with the same operation order.
//!
//! Conventions shared with the environments and the trainer: angles are in degrees and **signed**
//! (crown tiers positive, pavilion tiers negative); the sampler draws a raw value around the mean
//! and [`GaussianPolicy::squash`] maps it through `tanh` into a degree step. A final `LayerNorm`
//! normalises every token after the last transformer block; the value and gain heads pool, and the
//! sensitivity head reads, the normalised tokens.

use super::{
    network::{
        InferenceScratch, LayerNorm, Linear, MlpBlock, ModelError, MultiHeadAttention, gelu,
    },
    weights::WeightError,
};
use crate::env::{
    ContinuousTierAction, GlobalFeature, MOBILITY_INDEX, N_COMPONENTS, TierFeature,
    WEIGHT_FRACTION_INDEX,
};
use serde::{Deserialize, Serialize};

// The policy distribution lives in `policy`; these re-exports keep the historical item paths.
pub use super::policy::{
    BLOCK_TYPE_INDEX, GaussianPolicy, HALF_LN_2PI, HALF_LN_2PI_E, LOG_STD_INIT, LOG_STD_MAX,
    LOG_STD_MIN, LOG_STD_SLOTS, block_slot, bounded_log_std, compose_mean_masked, log_std_raw_init,
};

/// Everything one forward pass of the network produces.
#[derive(Debug, Clone, PartialEq)]
pub struct ForwardOutput {
    /// The per-tier Gaussian: the composed mean (pre-squash, within `[-MEAN_BOUND, MEAN_BOUND]`)
    /// and the block's bounded log-std.
    pub policy: GaussianPolicy,
    /// The value baseline `V(s)`.
    pub value: f32,
    /// Per tier, the predicted component slopes (points per degree), `[n_tiers][N_COMPONENTS]`,
    /// the raw head outputs.
    pub sensitivities: Vec<[f32; N_COMPONENTS]>,
    /// Predicted score drop of the oracle step at this state, in points.
    pub gain: f32,
}

/// Permutation-invariant Geometric Set Attention network for arbitrary gemstone tier counts.
///
/// Models inter-tier reflection and refraction coupling via Multi-Head Self-Attention,
/// supports variable-tier stones dynamically, and provides physical kinematic safety projections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GemstoneSetAttentionNetwork {
    /// Projection layer from per-tier features into hidden dimension.
    pub tier_proj: Linear,
    /// Projection layer from global features into hidden dimension.
    pub global_proj: Linear,
    /// Multi-head self-attention layers across tier tokens and context token.
    pub attn_layers: Vec<MultiHeadAttention>,
    /// Pre-attention `LayerNorm` layers.
    pub pre_attn_norms: Vec<LayerNorm>,
    /// Post-attention `LayerNorm` layers.
    pub post_attn_norms: Vec<LayerNorm>,
    /// MLP feedforward blocks.
    pub mlp_blocks: Vec<MlpBlock>,
    /// `LayerNorm` applied to every token (context token included) after the last transformer block.
    ///
    /// A pre-LN stack leaves the residual stream un-normalised, so without it the heads read a scale
    /// that grows with depth and training.
    pub final_norm: LayerNorm,
    /// Value function head predicting scalar state baseline V(s) from the pooled vector.
    pub critic_head: Linear,
    /// Sensitivity head (`d_model -> N_COMPONENTS`): per tier, the predicted slope of each objective
    /// component per degree of that tier. The policy mean is composed from these (see the module
    /// docs).
    pub sens_head: Linear,
    /// Gain head (`d_model -> 1`) on the same pooled vector as the critic: the predicted score drop
    /// of the oracle step at this state, in points.
    pub gain_head: Linear,
    /// Raw state-independent log standard deviation per block: crown, pavilion, girdle or other (see
    /// [`block_slot`]). The policy uses [`bounded_log_std`] of each value.
    pub log_std_raw: [f32; LOG_STD_SLOTS],
    /// Model hidden dimension.
    pub d_model: usize,
    /// Number of attention heads.
    pub num_heads: usize,
    /// Tier feature input dimension.
    pub d_tier: usize,
    /// Global feature input dimension.
    pub d_global: usize,
}

/// Residual add `a += b`, refusing mismatched widths instead of silently truncating.
fn add_in_place(a: &mut [f32], b: &[f32]) -> Result<(), ModelError> {
    if a.len() != b.len() {
        return Err(ModelError::Dimension {
            what: "residual branch",
            expected: a.len(),
            got: b.len(),
        });
    }
    for (x, y) in a.iter_mut().zip(b) {
        *x += y;
    }
    Ok(())
}

/// First (and only) output of a one-wide head.
fn head_scalar(out: &[f32], what: &'static str) -> Result<f32, ModelError> {
    out.first().copied().ok_or(ModelError::Dimension {
        what,
        expected: 1,
        got: 0,
    })
}

/// The `N_COMPONENTS` outputs of the sensitivity head for one tier.
fn sensitivity_row(out: &[f32]) -> Result<[f32; N_COMPONENTS], ModelError> {
    <[f32; N_COMPONENTS]>::try_from(out).map_err(|_| ModelError::Dimension {
        what: "sensitivity head output",
        expected: N_COMPONENTS,
        got: out.len(),
    })
}

/// The objective's six normalized optical, yield and selected-tone coefficients followed by the
/// raw shape coefficient, read from the global token at [`WEIGHT_FRACTION_INDEX`].
fn weight_fractions(global_token: &[f32]) -> Result<[f32; N_COMPONENTS], ModelError> {
    let end = WEIGHT_FRACTION_INDEX + N_COMPONENTS;
    global_token
        .get(WEIGHT_FRACTION_INDEX..end)
        .and_then(|slice| <[f32; N_COMPONENTS]>::try_from(slice).ok())
        .ok_or(ModelError::Dimension {
            what: "global token weight fractions",
            expected: end,
            got: global_token.len(),
        })
}

/// The vector the critic and gain heads read: the context token plus the mean of the tier tokens
/// (`tokens[0]` is the context token; the tier tokens follow).
fn pool(tokens: &[Vec<f32>]) -> Vec<f32> {
    let n_tiers = (tokens.len() - 1) as f32;
    let mut pooled = tokens[0].clone();
    for tok in &tokens[1..] {
        for (p, &v) in pooled.iter_mut().zip(tok) {
            *p += v / n_tiers;
        }
    }
    pooled
}

impl GemstoneSetAttentionNetwork {
    /// Construct a new Gemstone Set Attention network with Xavier initialization.
    ///
    /// # Errors
    /// [`WeightError::Invalid`] when a dimension is zero or `d_model` is not divisible by `num_heads`.
    pub fn new(
        d_tier: usize,
        d_global: usize,
        d_model: usize,
        num_heads: usize,
        num_layers: usize,
        seed: u64,
    ) -> Result<Self, WeightError> {
        if d_tier == 0
            || d_global == 0
            || d_model == 0
            || num_heads == 0
            || !d_model.is_multiple_of(num_heads)
        {
            return Err(WeightError::Invalid(format!(
                "d_tier {d_tier}, d_global {d_global} and d_model {d_model} must be positive and d_model \
                 divisible by num_heads {num_heads}"
            )));
        }
        Ok(Self::build(
            d_tier, d_global, d_model, num_heads, num_layers, seed,
        ))
    }

    /// Construct without re-validating; the caller guarantees positive dimensions and
    /// `d_model % num_heads == 0`.
    fn build(
        d_tier: usize,
        d_global: usize,
        d_model: usize,
        num_heads: usize,
        num_layers: usize,
        seed: u64,
    ) -> Self {
        let tier_proj = Linear::init_xavier(d_tier, d_model, seed ^ 0x1111_1111);
        let global_proj = Linear::init_xavier(d_global, d_model, seed ^ 0x2222_2222);

        let mut attn_layers = Vec::with_capacity(num_layers);
        let mut pre_attn_norms = Vec::with_capacity(num_layers);
        let mut post_attn_norms = Vec::with_capacity(num_layers);
        let mut mlp_blocks = Vec::with_capacity(num_layers);

        for i in 0..num_layers {
            let layer_seed = seed ^ ((i as u64 + 1) * 0x9E37_79B9);
            attn_layers.push(MultiHeadAttention::build(d_model, num_heads, layer_seed));
            pre_attn_norms.push(LayerNorm::new(d_model));
            post_attn_norms.push(LayerNorm::new(d_model));
            mlp_blocks.push(MlpBlock::new(
                d_model,
                d_model * 2,
                layer_seed ^ 0x5555_AAAA,
            ));
        }

        Self {
            tier_proj,
            global_proj,
            attn_layers,
            pre_attn_norms,
            post_attn_norms,
            mlp_blocks,
            final_norm: LayerNorm::new(d_model),
            critic_head: Linear::init_xavier(d_model, 1, seed ^ 0xCCCC_DDDD),
            sens_head: Linear::init_xavier(d_model, N_COMPONENTS, seed ^ 0xAAAA_BBBB),
            gain_head: Linear::init_xavier(d_model, 1, seed ^ 0x6A1A_6A1A),
            log_std_raw: [log_std_raw_init(); LOG_STD_SLOTS],
            d_model,
            num_heads,
            d_tier,
            d_global,
        }
    }

    /// Total number of trainable parameters (weights, biases, layer-norm scales/shifts including the
    /// final norm, and the three raw log-std values).
    ///
    /// Closed form for `d = d_model`, `L` layers: `(d_tier + 1) d + (d_global + 1) d +
    /// L (8 d^2 + 11 d) + (d + 1)` critic `+ (d + 1) N_COMPONENTS` sensitivity head `+ (d + 1)` gain
    /// head `+ 2 d` final norm `+ 3` raw log-std values.
    #[must_use]
    pub fn param_count(&self) -> usize {
        self.tier_proj.param_count()
            + self.global_proj.param_count()
            + self.critic_head.param_count()
            + self.sens_head.param_count()
            + self.gain_head.param_count()
            + self.final_norm.param_count()
            + self.log_std_raw.len()
            + self
                .attn_layers
                .iter()
                .map(MultiHeadAttention::param_count)
                .sum::<usize>()
            + self
                .pre_attn_norms
                .iter()
                .map(LayerNorm::param_count)
                .sum::<usize>()
            + self
                .post_attn_norms
                .iter()
                .map(LayerNorm::param_count)
                .sum::<usize>()
            + self
                .mlp_blocks
                .iter()
                .map(MlpBlock::param_count)
                .sum::<usize>()
    }

    /// Forward pass through the Set Attention network for an arbitrary number of tiers (zero tiers is
    /// valid: the policy is then empty and the value is computed from the context token alone).
    ///
    /// - `tier_tokens`: per-tier feature vectors, shape `[N, d_tier]`.
    /// - `global_token`: global context feature vector, shape `[d_global]`; its entries from
    ///   [`WEIGHT_FRACTION_INDEX`] on are the objective's coefficients: six normalized optical,
    ///   yield and selected-tone weights followed by the raw shape weight.
    ///
    /// Returns the [`GaussianPolicy`] (the composed pre-squash mean of every tier, bounded to
    /// `[-MEAN_BOUND, MEAN_BOUND]`, and the block's [`bounded_log_std`], read from entry
    /// [`BLOCK_TYPE_INDEX`] of the input token) and the value baseline `V(s)`. Use
    /// [`Self::forward_full`] for the sensitivities and the gain as well.
    ///
    /// # Errors
    /// [`ModelError::Dimension`] when a token's length is not `d_tier` / `d_global`, the global token
    /// is too short to carry the weight fractions, or the layer widths disagree with each other.
    pub fn forward(
        &self,
        tier_tokens: &[Vec<f32>],
        global_token: &[f32],
    ) -> Result<(GaussianPolicy, f32), ModelError> {
        self.forward_with_scratch(tier_tokens, global_token, &mut InferenceScratch::default())
    }

    /// [`Self::forward`] with caller-owned softmax buffers, for rollout loops that run the
    /// network thousands of times per iteration.
    ///
    /// # Errors
    /// Same as [`Self::forward`].
    pub fn forward_with_scratch(
        &self,
        tier_tokens: &[Vec<f32>],
        global_token: &[f32],
        scratch: &mut InferenceScratch,
    ) -> Result<(GaussianPolicy, f32), ModelError> {
        let out = self.forward_full_with_scratch(tier_tokens, global_token, scratch)?;
        Ok((out.policy, out.value))
    }

    /// [`Self::forward`] returning the per-tier sensitivities and the gain prediction as well.
    ///
    /// # Errors
    /// Same as [`Self::forward`].
    pub fn forward_full(
        &self,
        tier_tokens: &[Vec<f32>],
        global_token: &[f32],
    ) -> Result<ForwardOutput, ModelError> {
        self.forward_full_with_scratch(tier_tokens, global_token, &mut InferenceScratch::default())
    }

    /// [`Self::forward_full`] with caller-owned softmax buffers (see
    /// [`Self::forward_with_scratch`]).
    ///
    /// # Errors
    /// Same as [`Self::forward`].
    pub fn forward_full_with_scratch(
        &self,
        tier_tokens: &[Vec<f32>],
        global_token: &[f32],
        scratch: &mut InferenceScratch,
    ) -> Result<ForwardOutput, ModelError> {
        // 1. Embed the context token and every tier token fused with it
        let mut tokens = self.embed(tier_tokens, global_token)?;
        let fractions = weight_fractions(global_token)?;

        // 2. Multi-Head Attention and MLP Transformer layers (pre-LN, residual owned here)
        self.run_blocks(&mut tokens, scratch)?;

        // 3. Final LayerNorm on every token (context included); the heads read the normalised stream
        for tok in &mut tokens {
            *tok = self.final_norm.forward(tok)?;
        }

        // 4. Value and gain heads on the pooled vector: context token + mean of the tier tokens
        let pooled = pool(&tokens);
        let value = head_scalar(&self.critic_head.forward(&pooled)?, "critic head output")?;
        let gain = head_scalar(&self.gain_head.forward(&pooled)?, "gain head output")?;

        // 5. Sensitivity head per tier; the state-independent log std comes from the block of the
        //    *input* token (entry BLOCK_TYPE_INDEX)
        let bounded = self.log_std_raw.map(bounded_log_std);
        let mut sensitivities = Vec::with_capacity(tier_tokens.len());
        let mut log_stds = Vec::with_capacity(tier_tokens.len());
        for (tok, input) in tokens[1..].iter().zip(tier_tokens) {
            sensitivities.push(sensitivity_row(&self.sens_head.forward(tok)?)?);
            let block = input.get(BLOCK_TYPE_INDEX).copied().unwrap_or(0.0);
            log_stds.push(bounded[block_slot(block)]);
        }

        // 6. The policy mean is composed from the sensitivities and the weight fractions
        let mobile: Vec<bool> = tier_tokens
            .iter()
            .map(|token| token.get(MOBILITY_INDEX).copied().unwrap_or(0.0) > 0.5)
            .collect();
        let mean = compose_mean_masked(&sensitivities, &fractions, &mobile);
        Ok(ForwardOutput {
            policy: GaussianPolicy {
                mean,
                log_std: log_stds,
            },
            value,
            sensitivities,
            gain,
        })
    }

    /// The context token followed by one fused token per tier: `[1 + N][d_model]`.
    fn embed(
        &self,
        tier_tokens: &[Vec<f32>],
        global_token: &[f32],
    ) -> Result<Vec<Vec<f32>>, ModelError> {
        let ctx_emb: Vec<f32> = self
            .global_proj
            .forward(global_token)?
            .into_iter()
            .map(gelu)
            .collect();

        let mut tokens = Vec::with_capacity(tier_tokens.len() + 1);
        tokens.push(ctx_emb.clone()); // Token 0: Global Context / CLS token

        for t in tier_tokens {
            let t_raw = self.tier_proj.forward(t)?;
            if t_raw.len() != ctx_emb.len() {
                return Err(ModelError::Dimension {
                    what: "tier embedding width",
                    expected: ctx_emb.len(),
                    got: t_raw.len(),
                });
            }
            tokens.push(
                t_raw
                    .into_iter()
                    .zip(&ctx_emb)
                    .map(|(x, c)| gelu(x + c))
                    .collect(),
            );
        }
        Ok(tokens)
    }

    /// The pre-LN transformer stack over `tokens` (`[1 + N, d_model]`, context token first), in place.
    fn run_blocks(
        &self,
        tokens: &mut [Vec<f32>],
        scratch: &mut InferenceScratch,
    ) -> Result<(), ModelError> {
        if self.pre_attn_norms.len() != self.attn_layers.len()
            || self.post_attn_norms.len() != self.attn_layers.len()
            || self.mlp_blocks.len() != self.attn_layers.len()
        {
            return Err(ModelError::Structure(
                "layer vectors have different lengths",
            ));
        }
        for i in 0..self.attn_layers.len() {
            let normed = tokens
                .iter()
                .map(|tok| self.pre_attn_norms[i].forward(tok))
                .collect::<Result<Vec<_>, _>>()?;
            let attn_out = self.attn_layers[i].forward_with_scratch(&normed, scratch)?;
            for (tok, att) in tokens.iter_mut().zip(&attn_out) {
                add_in_place(tok, att)?;
            }

            for tok in &mut *tokens {
                let normed_tok = self.post_attn_norms[i].forward(tok)?;
                let ffn_out = self.mlp_blocks[i].forward(&normed_tok)?;
                add_in_place(tok, &ffn_out)?;
            }
        }
        Ok(())
    }

    /// Clamp proposed deltas to the shared sign-preserving angle and step limits. Mobility comes from
    /// the observation token, so pinned/flat/projected-away coordinates stay visible to attention but
    /// receive neither a policy mean nor an action. The target critical angle remains in this public
    /// signature for source compatibility; material score determines whether a safe pavilion move helps.
    pub fn project_actions(
        action: &mut ContinuousTierAction,
        tiers: &[TierFeature],
        _target_crit_deg: f32,
        max_step_deg: f64,
    ) {
        for (i, delta) in action.deltas_deg.iter_mut().enumerate() {
            let Some(t) = tiers.get(i) else {
                *delta = 0.0;
                continue;
            };
            if t.is_mobile <= 0.5 || !delta.is_finite() {
                *delta = 0.0;
                continue;
            }
            let current = f64::from(t.angle_deg);
            let min_safe = crate::env::MIN_SAFE_CANDIDATE_ANGLE_DEG;
            let max_safe = crate::env::MAX_SAFE_CANDIDATE_ANGLE_DEG;
            // Preserve the sign of each facet. Intersect the observation's explicit signed limits
            // with physical safety bounds; an empty range or an out-of-range source is not repaired
            // by replacing the sampled move with an automatic correction.
            let (safe_lo, safe_hi) = if current.is_sign_negative() {
                (-max_safe, -min_safe)
            } else {
                (min_safe, max_safe)
            };
            let has_tier_bounds = t.min_angle_deg.is_finite()
                && t.max_angle_deg.is_finite()
                && t.min_angle_deg < t.max_angle_deg;
            let (lo, hi) = if has_tier_bounds {
                (
                    safe_lo.max(f64::from(t.min_angle_deg)),
                    safe_hi.min(f64::from(t.max_angle_deg)),
                )
            } else {
                (safe_lo, safe_hi)
            };
            if lo <= hi && (lo..=hi).contains(&current) {
                let step_bound = if max_step_deg.is_finite() {
                    max_step_deg.abs()
                } else {
                    0.0
                };
                let step = delta.clamp(-step_bound, step_bound);
                let candidate = current + step;
                *delta = candidate.clamp(lo, hi) - current;
            } else {
                *delta = 0.0;
            }
        }
    }
}

/// Predefined model sizing tiers for gemstone set attention policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ModelTier {
    /// Small (S): ~42k parameters.
    Small,
    /// Medium (M): ~106k parameters - the default balanced tier.
    #[default]
    Medium,
    /// Large (L): ~540k parameters.
    Large,
    /// Extra Large (XL): ~3.2M parameters.
    ExtraLarge,
    /// Extra Extra Large (XXL): ~9.5M parameters.
    ExtraExtraLarge,
}

impl ModelTier {
    /// Returns (`d_model`, `num_heads`, `num_layers`) for this tier.
    #[must_use]
    pub const fn config(self) -> (usize, usize, usize) {
        match self {
            Self::Small => (48, 4, 2),
            Self::Medium => (64, 4, 3),
            Self::Large => (128, 8, 4),
            Self::ExtraLarge => (256, 8, 6),
            Self::ExtraExtraLarge => (384, 12, 8),
        }
    }

    /// Approximate parameter count for this tier, rounded to tens; a test checks it against
    /// [`GemstoneSetAttentionNetwork::param_count`] within 10 %.
    ///
    /// Computed for the observation layout of 26 tier and 41 global features (layout 5) and the
    /// seven-component sensitivity head, for `d = d_model` and `L` layers: `(d_tier + 1) d +
    /// (d_global + 1) d + L (8 d^2 + 11 d) + (d + 1)` critic `+ (d + 1) N_COMPONENTS` sensitivity
    /// head `+ (d + 1)` gain head `+ 2 d` final norm `+ 3` raw log-std values, which is
    /// `L (8 d^2 + 11 d) + 80 d + 12` for these dimensions.
    #[must_use]
    pub const fn param_count_approx(self) -> usize {
        match self {
            Self::Small => 41_770,
            Self::Medium => 105_550,
            Self::Large => 540_170,
            Self::ExtraLarge => 3_183_120,
            Self::ExtraExtraLarge => 9_501_710,
        }
    }

    /// Short label string: "S", "M", "L", "XL", "XXL".
    #[must_use]
    pub const fn code_str(self) -> &'static str {
        match self {
            Self::Small => "S",
            Self::Medium => "M",
            Self::Large => "L",
            Self::ExtraLarge => "XL",
            Self::ExtraExtraLarge => "XXL",
        }
    }

    /// Parse loose string ("s", "small", "m", "medium", "l", "large", "xl", "xxl").
    #[must_use]
    pub fn from_str_loose(s: &str) -> Option<Self> {
        let clean = s.trim().to_ascii_uppercase();
        match clean.as_str() {
            "S" | "SMALL" => Some(Self::Small),
            "M" | "MEDIUM" => Some(Self::Medium),
            "L" | "LARGE" => Some(Self::Large),
            "XL" | "EXTRALARGE" | "EXTRA_LARGE" => Some(Self::ExtraLarge),
            "XXL" | "EXTRAEXTRALARGE" | "EXTRA_EXTRA_LARGE" => Some(Self::ExtraExtraLarge),
            _ => None,
        }
    }

    /// Instantiate a Geometric Set Attention network initialized with Xavier weights for this tier.
    ///
    /// Infallible: every tier configuration satisfies the network's structural rules (a unit test builds
    /// and validates all five).
    #[must_use]
    pub fn init_network(self, seed: u64) -> GemstoneSetAttentionNetwork {
        let (d_model, num_heads, num_layers) = self.config();
        GemstoneSetAttentionNetwork::build(
            TierFeature::FEATURE_DIM,
            GlobalFeature::FEATURE_DIM,
            d_model,
            num_heads,
            num_layers,
            seed,
        )
    }
}

#[cfg(test)]
#[path = "actor_critic_tests.rs"]
mod tests;
