//! Burn twin of the pure-Rust set-attention network (`crate::model::GemstoneSetAttentionNetwork`).
//!
//! The twin exists for training: the same architecture on Burn tensors with automatic
//! differentiation, convertible to and from the pure network ([`BurnSetAttention::from_set_attention`],
//! [`BurnSetAttention::export_to_set_attention`]). Both trainers (`BurnTrainer` in `burn_ppo`, the
//! imitation loss in `imitation`) drive this model.
//!
//! # Conventions shared with the pure-Rust twin
//!
//! - The activation is the tanh GELU (`gelu_approximate`) on both sides.
//! - The transformer block is `tok += attn(norm(tok)); tok += ffn(norm(tok))`, one residual per
//!   sub-layer; a final `LayerNorm` then normalises every token (context token included) and the
//!   heads and the pooling read the normalised tokens.
//! - The **sensitivity head** (`sens_head`, `d_model -> N_COMPONENTS`) reads each normalised tier
//!   token: per tier, the predicted slope of each objective component per degree of that tier (raw
//!   outputs). The **value** and **gain** heads (`d_model -> 1`) read the same pooled vector, the
//!   context token plus the mean of the real tier tokens.
//! - The policy `mean` is composed from the sensitivities and the objective's weight fractions
//!   (global-token entries `WEIGHT_FRACTION_INDEX..+N_COMPONENTS`):
//!   `g_k = sum_c frac[c] * sens[k][c]`, `g_max = max over real tiers of |g_k|`,
//!   `mean_k = -MEAN_BOUND * g_k / (g_max + SLOPE_EPS)`, in pre-squash units. The pure twin
//!   accumulates `g_k` with `mul_add`, the tensor version with a broadcast product and a sum over the
//!   four entries in index order, so the two may differ in the last ulp (tests compare them to `1e-5`).
//! - `log_std` is **state-independent**: three learnable raw values (crown, pavilion, other),
//!   selected per tier by the block feature (input token entry 3) and squashed by
//!   `LOG_STD_MIN + (LOG_STD_MAX - LOG_STD_MIN) * sigmoid(raw)` (the pure twin's `bounded_log_std`).
//! - The action is the raw (pre-squash) Gaussian sample; the environment applies `tanh` and the step
//!   limit afterwards.
//!
//! # Padding
//!
//! Tiers beyond a design's real tier count are padding: they are masked out of attention (keys), of
//! the pooled input of the value and gain heads and of the largest-slope normalisation of the mean.
//! Outputs at padded positions are meaningless and must be masked by the caller.
//!
//! # Batched rollout forward
//!
//! A rollout needs the policy of every environment of a pass at once.
//! [`BurnSetAttention::forward_host`] (in `burn_model_host`, with [`HostPolicyBatch`]) packs the
//! flat observations of the whole pass, runs one forward over the batch without gradient tracking
//! (on `Module::valid()` of the model, which `BurnTrainer::rollout_model` also hands out) and
//! reads the means, log-stds and values back to the host, one transfer each, trimmed to every
//! sample's real tiers.
//!
//! The numbers of that pass are the device's: a GPU's f32 kernels differ from the pure-Rust twin
//! in the last bits (about `1e-4` per entry end to end), so a rollout is reproducible on one
//! device, not across devices. Validation, checkpoints and inference run the pure-Rust twin, whose
//! results do not depend on the machine.

use burn::{
    module::Param,
    nn::{LayerNorm, LayerNormConfig, Linear, LinearConfig},
    prelude::*,
    tensor::{
        TensorData,
        activation::{gelu_approximate, sigmoid},
    },
};

use super::TrainError;
pub use super::burn_model_host::HostPolicyBatch;
use crate::{
    env::{MEAN_BOUND, N_COMPONENTS, SLOPE_EPS, WEIGHT_FRACTION_INDEX},
    model::{
        GemstoneSetAttentionNetwork, LOG_STD_MAX, LOG_STD_MIN,
        policy::{BLOCK_TYPE_INDEX, LOG_STD_SLOTS, log_std_raw_init},
    },
};

/// Additive attention bias for padded keys (finite, so no NaN from `-inf - -inf`).
const MASKED_KEY_BIAS: f32 = 1.0e9;
/// The `LayerNorm` epsilon of a freshly built model; the same as the pure-Rust
/// [`crate::model::LayerNorm::new`] default and Burn's own default.
const DEFAULT_LAYER_NORM_EPS: f32 = 1e-5;
/// Upper clamp of the divisors that count masked entries (the lower clamp is 1, so an empty mask
/// divides by 1 and yields exactly 0).
pub(crate) const COUNT_CLAMP_MAX: f32 = 1.0e9;

/// Burn-backed Geometric Set Attention neural network.
///
/// Implements full Multi-Head Attention transformer layers over gemstone tiers,
/// enabling permutation-invariant geometric reasoning, variable-tier cut policies,
/// and end-to-end autodiff training with Burn.
#[derive(Module, Debug)]
pub struct BurnSetAttention {
    /// Projection layer from tier feature tokens into hidden dimension.
    pub tier_proj: Linear,
    /// Projection layer from global context token into hidden dimension.
    pub global_proj: Linear,
    /// Multi-head attention query linear projections across layers.
    pub attn_q: Vec<Linear>,
    /// Multi-head attention key linear projections across layers.
    pub attn_k: Vec<Linear>,
    /// Multi-head attention value linear projections across layers.
    pub attn_v: Vec<Linear>,
    /// Multi-head attention output projections across layers.
    pub attn_out: Vec<Linear>,
    /// Pre-attention `LayerNorm` layers.
    pub pre_attn_norms: Vec<LayerNorm>,
    /// Post-attention `LayerNorm` layers.
    pub post_attn_norms: Vec<LayerNorm>,
    /// The epsilon of each pre-attention norm.
    ///
    /// Burn keeps a `LayerNorm`'s epsilon private, so the model records it to hand it back to
    /// the pure-Rust twin on export.
    pub pre_attn_eps: Vec<f32>,
    /// The epsilon of each post-attention norm (see [`Self::pre_attn_eps`]).
    pub post_attn_eps: Vec<f32>,
    /// Feedforward MLP first projection layers.
    pub mlp_fc1: Vec<Linear>,
    /// Feedforward MLP second projection layers.
    pub mlp_fc2: Vec<Linear>,
    /// `LayerNorm` over every token after the last transformer block (twin of the pure network's
    /// `final_norm`).
    pub final_norm: LayerNorm,
    /// The epsilon of the final norm (see [`Self::pre_attn_eps`]).
    pub final_eps: f32,
    /// Critic head predicting scalar baseline value V(s) from the pooled vector.
    pub critic_head: Linear,
    /// Sensitivity head (`d_model -> N_COMPONENTS`): per tier, the predicted slope of each objective
    /// component per degree of that tier.
    pub sens_head: Linear,
    /// Gain head (`d_model -> 1`) on the pooled vector the critic reads: the predicted score drop of
    /// the oracle step at the state, in points.
    pub gain_head: Linear,
    /// Raw state-independent log standard deviation per block (crown, pavilion, other), shape `[3]`;
    /// the policy uses `LOG_STD_MIN + (LOG_STD_MAX - LOG_STD_MIN) * sigmoid(raw)`.
    pub log_std_raw: Param<Tensor<1>>,
    /// Model hidden dimension.
    pub d_model: usize,
    /// Number of attention heads.
    pub num_heads: usize,
    /// Number of transformer layers.
    pub num_layers: usize,
    /// Tier feature input dimension.
    pub d_tier: usize,
    /// Global context feature input dimension.
    pub d_global: usize,
}

/// Everything one forward pass of [`BurnSetAttention`] produces. Entries at padded tier positions
/// are meaningless and must be masked by the caller.
#[derive(Debug, Clone)]
pub struct BurnForward {
    /// `[batch, n_tiers]` composed policy means in pre-squash units, within
    /// `[-MEAN_BOUND, MEAN_BOUND]` at the real tiers.
    pub mean: Tensor<2>,
    /// `[batch, n_tiers]` policy log standard deviations in `LOG_STD_MIN..=LOG_STD_MAX`,
    /// state-independent: the bounded raw value of the tier's block.
    pub log_std: Tensor<2>,
    /// `[batch, 1]` critic baseline predictions.
    pub value: Tensor<2>,
    /// `[batch, n_tiers, N_COMPONENTS]` predicted component slopes per tier (raw head outputs).
    pub sensitivities: Tensor<3>,
    /// `[batch, 1]` predicted score drop of the oracle step (points).
    pub gain: Tensor<2>,
}

/// Read a tensor back to the host as `f32`.
pub(crate) fn host_vec<const D: usize>(t: Tensor<D>) -> Result<Vec<f32>, TrainError> {
    let data = t.into_data();
    data.as_slice::<f32>()
        .map(<[f32]>::to_vec)
        .map_err(|e| TrainError::Tensor(format!("{e:?}")))
}

/// Read a one-element tensor back to the host.
pub(crate) fn host_scalar<const D: usize>(t: Tensor<D>) -> Result<f32, TrainError> {
    host_vec(t)?
        .first()
        .copied()
        .ok_or_else(|| TrainError::Tensor("empty tensor".into()))
}

/// The three raw log-stds as a trainable parameter of shape `[3]`.
fn log_std_param(values: [f32; LOG_STD_SLOTS], device: &Device) -> Param<Tensor<1>> {
    Param::from_tensor(Tensor::<1>::from_data(
        TensorData::new(values.to_vec(), [LOG_STD_SLOTS]),
        device,
    ))
}

/// Convert a pure-Rust linear layer (`[out, in]` row-major weights) into a Burn layer (`[in, out]`).
fn burn_linear(
    l: &crate::model::Linear,
    what: &str,
    device: &Device,
) -> Result<Linear, TrainError> {
    let (i, o) = (l.in_features, l.out_features);
    if l.weights.len() != i * o || l.biases.len() != o {
        return Err(TrainError::ShapeMismatch(format!(
            "{what}: {} weights / {} biases for a {i}x{o} layer",
            l.weights.len(),
            l.biases.len()
        )));
    }
    let mut w = vec![0.0f32; i * o];
    for out in 0..o {
        for inp in 0..i {
            w[inp * o + out] = l.weights[out * i + inp];
        }
    }
    let weight = Tensor::<2>::from_data(TensorData::new(w, [i, o]), device);
    let bias = Tensor::<1>::from_data(TensorData::new(l.biases.clone(), [o]), device);
    Ok(Linear {
        weight: Param::from_tensor(weight),
        bias: Some(Param::from_tensor(bias)),
    })
}

/// Convert a pure-Rust `LayerNorm` into a Burn `LayerNorm`.
fn burn_layernorm(
    ln: &crate::model::LayerNorm,
    what: &str,
    dim: usize,
    device: &Device,
) -> Result<LayerNorm, TrainError> {
    if ln.gamma.len() != dim || ln.beta.len() != dim {
        return Err(TrainError::ShapeMismatch(format!(
            "{what}: gamma {} / beta {} for dim {dim}",
            ln.gamma.len(),
            ln.beta.len()
        )));
    }
    let mut out = LayerNormConfig::new(dim)
        .with_epsilon(f64::from(ln.eps))
        .init(device);
    out.gamma = Param::from_tensor(Tensor::<1>::from_data(
        TensorData::new(ln.gamma.clone(), [dim]),
        device,
    ));
    out.beta = Some(Param::from_tensor(Tensor::<1>::from_data(
        TensorData::new(ln.beta.clone(), [dim]),
        device,
    )));
    Ok(out)
}

/// Convert a Burn linear layer into the pure-Rust layer (`[out, in]` row-major weights).
fn pure_linear(l: &Linear, what: &str) -> Result<crate::model::Linear, TrainError> {
    let [in_features, out_features] = l.weight.val().dims();
    let w = host_vec(l.weight.val())?;
    if w.len() != in_features * out_features {
        return Err(TrainError::ShapeMismatch(format!(
            "{what}: read back {} weights for a {in_features}x{out_features} layer",
            w.len()
        )));
    }
    let mut weights = vec![0.0f32; w.len()];
    for inp in 0..in_features {
        for out in 0..out_features {
            weights[out * in_features + inp] = w[inp * out_features + out];
        }
    }
    let biases = match &l.bias {
        Some(b) => host_vec(b.val())?,
        None => vec![0.0; out_features],
    };
    if biases.len() != out_features {
        return Err(TrainError::ShapeMismatch(format!(
            "{what}: read back {} biases for {out_features} outputs",
            biases.len()
        )));
    }
    Ok(crate::model::Linear {
        weights,
        biases,
        in_features,
        out_features,
    })
}

/// Convert a Burn `LayerNorm` into the pure-Rust `LayerNorm`; `eps` is the epsilon the model
/// recorded for it (Burn does not expose its own).
fn pure_layernorm(
    ln: &LayerNorm,
    eps: f32,
    what: &str,
    dim: usize,
) -> Result<crate::model::LayerNorm, TrainError> {
    let gamma = host_vec(ln.gamma.val())?;
    let beta = match &ln.beta {
        Some(b) => host_vec(b.val())?,
        None => vec![0.0; dim],
    };
    if gamma.len() != dim || beta.len() != dim {
        return Err(TrainError::ShapeMismatch(format!(
            "{what}: read back gamma {} / beta {} for dim {dim}",
            gamma.len(),
            beta.len()
        )));
    }
    Ok(crate::model::LayerNorm { gamma, beta, eps })
}

/// Reject a pure-Rust head that is not a `d_in -> d_out` layer (the layer conversion alone only
/// checks that the arrays agree with the layer's own declared shape).
fn expect_head(
    l: &crate::model::Linear,
    what: &str,
    d_in: usize,
    d_out: usize,
) -> Result<(), TrainError> {
    if l.in_features == d_in && l.out_features == d_out {
        Ok(())
    } else {
        Err(TrainError::ShapeMismatch(format!(
            "{what}: a {}x{} layer, expected {d_in}x{d_out}",
            l.in_features, l.out_features
        )))
    }
}

/// The policy mean on tensors: the twin of [`crate::model::compose_mean`].
///
/// `sensitivities` is `[batch, n_tiers, N_COMPONENTS]`, `fractions` the weight fractions reshaped to
/// `[batch, 1, N_COMPONENTS]`, `tier_mask` the real-tier mask `[batch, n_tiers]` (padded tiers
/// contribute `0` to the largest slope). Returns `[batch, n_tiers]`.
fn composed_mean(
    sensitivities: Tensor<3>,
    fractions: Tensor<3>,
    tier_mask: Tensor<2>,
) -> Tensor<2> {
    let [batch, n_tiers, _] = sensitivities.dims();
    let slopes = (sensitivities * fractions)
        .sum_dim(2)
        .reshape([batch, n_tiers]);
    let g_max = (slopes.clone().abs() * tier_mask).max_dim(1);
    slopes.mul_scalar(-MEAN_BOUND) / g_max.add_scalar(SLOPE_EPS)
}

impl BurnSetAttention {
    /// Initialize a new Set Attention model on the target Burn device.
    ///
    /// Every layer, the sensitivity and gain heads included, keeps Burn's default initialiser, and
    /// the three raw log-stds start at the value whose bounded log-std is `LOG_STD_INIT`, like the
    /// pure network's `init_network`. (The trainer app loads a pure network's weights over this
    /// skeleton, so the initialisation only matters for a run that starts from nothing.)
    ///
    /// # Errors
    ///
    /// [`TrainError::InvalidConfig`] when a dimension is zero, `d_tier` is too small to carry the block
    /// feature (entry [`BLOCK_TYPE_INDEX`]), `d_global` is too small to carry the objective's weight
    /// fractions (entries `WEIGHT_FRACTION_INDEX..+N_COMPONENTS`) or `d_model` is not divisible by
    /// `num_heads`.
    pub fn new(
        d_tier: usize,
        d_global: usize,
        d_model: usize,
        num_heads: usize,
        num_layers: usize,
        device: &Device,
    ) -> Result<Self, TrainError> {
        Self::check_dims(d_tier, d_global, d_model, num_heads)?;

        let tier_proj = LinearConfig::new(d_tier, d_model).init(device);
        let global_proj = LinearConfig::new(d_global, d_model).init(device);

        let mut attn_q = Vec::with_capacity(num_layers);
        let mut attn_k = Vec::with_capacity(num_layers);
        let mut attn_v = Vec::with_capacity(num_layers);
        let mut attn_out = Vec::with_capacity(num_layers);
        let mut pre_attn_norms = Vec::with_capacity(num_layers);
        let mut post_attn_norms = Vec::with_capacity(num_layers);
        let mut mlp_fc1 = Vec::with_capacity(num_layers);
        let mut mlp_fc2 = Vec::with_capacity(num_layers);

        for _ in 0..num_layers {
            attn_q.push(LinearConfig::new(d_model, d_model).init(device));
            attn_k.push(LinearConfig::new(d_model, d_model).init(device));
            attn_v.push(LinearConfig::new(d_model, d_model).init(device));
            attn_out.push(LinearConfig::new(d_model, d_model).init(device));

            pre_attn_norms.push(
                LayerNormConfig::new(d_model)
                    .with_epsilon(f64::from(DEFAULT_LAYER_NORM_EPS))
                    .init(device),
            );
            post_attn_norms.push(
                LayerNormConfig::new(d_model)
                    .with_epsilon(f64::from(DEFAULT_LAYER_NORM_EPS))
                    .init(device),
            );

            mlp_fc1.push(LinearConfig::new(d_model, d_model * 2).init(device));
            mlp_fc2.push(LinearConfig::new(d_model * 2, d_model).init(device));
        }

        Ok(Self {
            tier_proj,
            global_proj,
            attn_q,
            attn_k,
            attn_v,
            attn_out,
            pre_attn_norms,
            post_attn_norms,
            pre_attn_eps: vec![DEFAULT_LAYER_NORM_EPS; num_layers],
            post_attn_eps: vec![DEFAULT_LAYER_NORM_EPS; num_layers],
            mlp_fc1,
            mlp_fc2,
            final_norm: LayerNormConfig::new(d_model)
                .with_epsilon(f64::from(DEFAULT_LAYER_NORM_EPS))
                .init(device),
            final_eps: DEFAULT_LAYER_NORM_EPS,
            critic_head: LinearConfig::new(d_model, 1).init(device),
            sens_head: LinearConfig::new(d_model, N_COMPONENTS).init(device),
            gain_head: LinearConfig::new(d_model, 1).init(device),
            log_std_raw: log_std_param([log_std_raw_init(); LOG_STD_SLOTS], device),
            d_model,
            num_heads,
            num_layers,
            d_tier,
            d_global,
        })
    }

    /// The dimension rules of [`Self::new`].
    fn check_dims(
        d_tier: usize,
        d_global: usize,
        d_model: usize,
        num_heads: usize,
    ) -> Result<(), TrainError> {
        if d_tier == 0 || d_global == 0 || d_model == 0 || num_heads == 0 {
            return Err(TrainError::InvalidConfig(
                "d_tier, d_global, d_model and num_heads must be >= 1".into(),
            ));
        }
        if d_tier <= BLOCK_TYPE_INDEX {
            return Err(TrainError::InvalidConfig(format!(
                "d_tier {d_tier} must exceed {BLOCK_TYPE_INDEX}: the log-std head reads the block \
                 feature at tier-token entry {BLOCK_TYPE_INDEX}"
            )));
        }
        let needed_global = WEIGHT_FRACTION_INDEX + N_COMPONENTS;
        if d_global < needed_global {
            return Err(TrainError::InvalidConfig(format!(
                "d_global {d_global} must be at least {needed_global}: the composed mean reads the \
                 {N_COMPONENTS} weight fractions at global-token entry {WEIGHT_FRACTION_INDEX}"
            )));
        }
        if !d_model.is_multiple_of(num_heads) {
            return Err(TrainError::InvalidConfig(format!(
                "d_model {d_model} must be divisible by num_heads {num_heads}"
            )));
        }
        Ok(())
    }

    /// Build a Burn model holding exactly the weights of a pure-Rust network (inverse of
    /// [`Self::export_to_set_attention`]); used to resume training and to move a model between devices.
    ///
    /// # Errors
    ///
    /// [`TrainError::ShapeMismatch`] when the network's layer lists or weight arrays are inconsistent
    /// or a head has the wrong shape (critic and gain `d_model -> 1`, sensitivities
    /// `d_model -> N_COMPONENTS`), [`TrainError::InvalidConfig`] when `d_model` is not divisible by
    /// `num_heads`, `d_tier` is too small to carry the block feature or `d_global` is too small to
    /// carry the weight fractions (see [`Self::new`]).
    pub fn from_set_attention(
        net: &GemstoneSetAttentionNetwork,
        device: &Device,
    ) -> Result<Self, TrainError> {
        let num_layers = net.attn_layers.len();
        if net.pre_attn_norms.len() != num_layers
            || net.post_attn_norms.len() != num_layers
            || net.mlp_blocks.len() != num_layers
        {
            return Err(TrainError::ShapeMismatch(format!(
                "layer lists disagree: {num_layers} attention / {} pre-norm / {} post-norm / {} mlp",
                net.pre_attn_norms.len(),
                net.post_attn_norms.len(),
                net.mlp_blocks.len()
            )));
        }
        // Allocates the layer skeleton (validates dims); every parameter is replaced below.
        let mut model = Self::new(
            net.d_tier,
            net.d_global,
            net.d_model,
            net.num_heads,
            num_layers,
            device,
        )?;
        expect_head(&net.critic_head, "critic head", net.d_model, 1)?;
        expect_head(&net.sens_head, "sens head", net.d_model, N_COMPONENTS)?;
        expect_head(&net.gain_head, "gain head", net.d_model, 1)?;

        model.tier_proj = burn_linear(&net.tier_proj, "tier_proj", device)?;
        model.global_proj = burn_linear(&net.global_proj, "global_proj", device)?;
        for l in 0..num_layers {
            let a = &net.attn_layers[l];
            model.attn_q[l] = burn_linear(&a.q_proj, "attn q", device)?;
            model.attn_k[l] = burn_linear(&a.k_proj, "attn k", device)?;
            model.attn_v[l] = burn_linear(&a.v_proj, "attn v", device)?;
            model.attn_out[l] = burn_linear(&a.out_proj, "attn out", device)?;
            model.pre_attn_norms[l] =
                burn_layernorm(&net.pre_attn_norms[l], "pre-attn norm", net.d_model, device)?;
            model.post_attn_norms[l] = burn_layernorm(
                &net.post_attn_norms[l],
                "post-attn norm",
                net.d_model,
                device,
            )?;
            model.pre_attn_eps[l] = net.pre_attn_norms[l].eps;
            model.post_attn_eps[l] = net.post_attn_norms[l].eps;
            model.mlp_fc1[l] = burn_linear(&net.mlp_blocks[l].fc1, "mlp fc1", device)?;
            model.mlp_fc2[l] = burn_linear(&net.mlp_blocks[l].fc2, "mlp fc2", device)?;
        }
        model.final_norm = burn_layernorm(&net.final_norm, "final norm", net.d_model, device)?;
        model.final_eps = net.final_norm.eps;
        model.critic_head = burn_linear(&net.critic_head, "critic head", device)?;
        model.sens_head = burn_linear(&net.sens_head, "sens head", device)?;
        model.gain_head = burn_linear(&net.gain_head, "gain head", device)?;
        model.log_std_raw = log_std_param(net.log_std_raw, device);
        Ok(model)
    }

    /// Forward pass through the Set Attention model.
    ///
    /// - `tier_inputs`: `[batch, n_tiers, d_tier]` (padded to the longest design in the batch)
    /// - `global_inputs`: `[batch, d_global]`; its entries from `WEIGHT_FRACTION_INDEX` on are the
    ///   objective's weight fractions
    /// - `tier_mask`: `[batch, n_tiers]`, `1.0` for a real tier and `0.0` for padding; `n_tiers` must be
    ///   at least 1. Padded tiers are excluded as attention keys, from the pooled input of the value
    ///   and gain heads and from the largest-slope normalisation of the mean; a row with no real tier
    ///   pools to the context token alone.
    ///
    /// Returns a [`BurnForward`]; see its fields for shapes. Outputs at padded positions are
    /// meaningless and must be masked by the caller.
    #[must_use]
    pub fn forward(
        &self,
        tier_inputs: Tensor<3>,
        global_inputs: Tensor<2>,
        tier_mask: Tensor<2>,
    ) -> BurnForward {
        let [batch, n_tiers, _] = tier_inputs.dims();
        let seq_len = 1 + n_tiers;
        let log_std = self.block_log_std(tier_inputs.clone());
        let fractions = global_inputs
            .clone()
            .slice([
                0..batch,
                WEIGHT_FRACTION_INDEX..WEIGHT_FRACTION_INDEX + N_COMPONENTS,
            ])
            .reshape([batch, 1, N_COMPONENTS]);

        // 1. Embed global context token
        let ctx_raw = self.global_proj.forward(global_inputs);
        let ctx_emb = gelu_approximate(ctx_raw).reshape([batch, 1, self.d_model]);

        // 2. Embed each tier token and fuse with global context
        let t_raw = self.tier_proj.forward(tier_inputs);
        let fused_tiers = gelu_approximate(t_raw + ctx_emb.clone().repeat_dim(1, n_tiers));

        // 3. Stack tokens: [ctx_token, tier_1, ..., tier_N] -> [batch, 1 + n_tiers, d_model]
        let tokens: Tensor<3> = Tensor::cat(vec![ctx_emb, fused_tiers], 1);

        // Additive key bias: 0 for the context token and real tiers, -1e9 for padded tiers.
        let ctx_col = tier_mask
            .clone()
            .slice([0..batch, 0..1])
            .mul_scalar(0.0)
            .add_scalar(1.0);
        let key_bias = Tensor::cat(vec![ctx_col, tier_mask.clone()], 1)
            .add_scalar(-1.0)
            .mul_scalar(MASKED_KEY_BIAS)
            .reshape([batch, 1, 1, seq_len]);

        // 4. Attention and MLP Transformer layers, then the final LayerNorm on every token
        let tokens = self.run_layers(tokens, &key_bias);
        let tokens = self.final_norm.forward(tokens);

        // 5. Value and gain heads: context token + mean over the REAL (normalised) tier tokens
        let ctx_tok = tokens
            .clone()
            .slice([0..batch, 0..1, 0..self.d_model])
            .reshape([batch, self.d_model]);
        let tier_toks = tokens.slice([0..batch, 1..seq_len, 0..self.d_model]);
        let n_real = tier_mask.clone().sum_dim(1).clamp(1.0, COUNT_CLAMP_MAX);
        let mask3 = tier_mask.clone().reshape([batch, n_tiers, 1]);
        let pooled_tiers = (tier_toks.clone() * mask3)
            .sum_dim(1)
            .reshape([batch, self.d_model])
            / n_real;
        let pooled = ctx_tok + pooled_tiers;
        let value = self.critic_head.forward(pooled.clone());
        let gain = self.gain_head.forward(pooled);

        // 6. Sensitivity head on the normalised tier tokens, composed into the policy mean with the
        //    weight fractions; log std was built from the input blocks
        let sensitivities = self.sens_head.forward(tier_toks);
        let mean = composed_mean(sensitivities.clone(), fractions, tier_mask);

        BurnForward {
            mean,
            log_std,
            value,
            sensitivities,
            gain,
        }
    }

    /// The transformer stack (pre-LN attention and MLP, one residual per sub-layer) over
    /// `tokens` `[batch, 1 + n_tiers, d_model]`; `key_bias` is the additive attention bias
    /// `[batch, 1, 1, 1 + n_tiers]` that hides padded tiers.
    fn run_layers(&self, mut tokens: Tensor<3>, key_bias: &Tensor<4>) -> Tensor<3> {
        let [batch, seq_len, _] = tokens.dims();
        let head_dim = self.d_model / self.num_heads;
        let scale = 1.0 / (head_dim as f32).sqrt();

        for l in 0..self.num_layers {
            let normed = self.pre_attn_norms[l].forward(tokens.clone());

            let q = self.attn_q[l].forward(normed.clone());
            let k = self.attn_k[l].forward(normed.clone());
            let v = self.attn_v[l].forward(normed);

            let q = q
                .reshape([batch, seq_len, self.num_heads, head_dim])
                .swap_dims(1, 2);
            let k = k
                .reshape([batch, seq_len, self.num_heads, head_dim])
                .swap_dims(1, 2);
            let v = v
                .reshape([batch, seq_len, self.num_heads, head_dim])
                .swap_dims(1, 2);

            let scores = q.matmul(k.swap_dims(2, 3)).mul_scalar(scale) + key_bias.clone();
            let weights = burn::tensor::activation::softmax(scores, 3);
            let attn_out = weights.matmul(v);
            let attn_out = attn_out
                .swap_dims(1, 2)
                .reshape([batch, seq_len, self.d_model]);
            let attn_proj = self.attn_out[l].forward(attn_out);

            tokens = tokens + attn_proj;

            let normed_mlp = self.post_attn_norms[l].forward(tokens.clone());
            let ffn = self.mlp_fc1[l].forward(normed_mlp);
            let ffn = gelu_approximate(ffn);
            let ffn = self.mlp_fc2[l].forward(ffn);

            tokens = tokens + ffn;
        }
        tokens
    }

    /// State-independent per-tier `log_std` `[batch, n_tiers]`, built on the device from the block
    /// feature (`tier_inputs[.., .., BLOCK_TYPE_INDEX]`): crown (`> 0.5`) takes the first raw value,
    /// pavilion (`< -0.5`) the second, anything else the third, each squashed by
    /// `LOG_STD_MIN + (LOG_STD_MAX - LOG_STD_MIN) * sigmoid(raw)`. Twin of the pure network's
    /// `block_slot` + `bounded_log_std`; the gradient reaches the three raw values through the masks.
    fn block_log_std(&self, tier_inputs: Tensor<3>) -> Tensor<2> {
        let [batch, n_tiers, _] = tier_inputs.dims();
        let block = tier_inputs
            .slice([0..batch, 0..n_tiers, BLOCK_TYPE_INDEX..BLOCK_TYPE_INDEX + 1])
            .reshape([batch, n_tiers]);
        let crown = block.clone().greater_elem(0.5f32).float();
        let pavilion = block.lower_elem(-0.5f32).float();
        // Neither: 1 - crown - pavilion (the two masks are exclusive).
        let other = (crown.clone() + pavilion.clone())
            .mul_scalar(-1.0)
            .add_scalar(1.0);

        let bounded = sigmoid(self.log_std_raw.val())
            .mul_scalar(LOG_STD_MAX - LOG_STD_MIN)
            .add_scalar(LOG_STD_MIN);
        let slot = |k: usize| bounded.clone().slice([k..k + 1]).reshape([1, 1]);
        crown * slot(0) + pavilion * slot(1) + other * slot(2)
    }

    /// Exports trained Burn weights into the zero-dependency pure-Rust `GemstoneSetAttentionNetwork`.
    ///
    /// # Errors
    ///
    /// [`TrainError::Tensor`] when a parameter cannot be read back to the host and
    /// [`TrainError::ShapeMismatch`] when a read-back array has the wrong length. The caller keeps its
    /// previous network in either case.
    pub fn export_to_set_attention(&self) -> Result<GemstoneSetAttentionNetwork, TrainError> {
        let tier_proj = pure_linear(&self.tier_proj, "tier_proj")?;
        let global_proj = pure_linear(&self.global_proj, "global_proj")?;

        let mut attn_layers = Vec::with_capacity(self.num_layers);
        let mut pre_attn_norms = Vec::with_capacity(self.num_layers);
        let mut post_attn_norms = Vec::with_capacity(self.num_layers);
        let mut mlp_blocks = Vec::with_capacity(self.num_layers);

        for l in 0..self.num_layers {
            attn_layers.push(crate::model::MultiHeadAttention {
                q_proj: pure_linear(&self.attn_q[l], "attn q")?,
                k_proj: pure_linear(&self.attn_k[l], "attn k")?,
                v_proj: pure_linear(&self.attn_v[l], "attn v")?,
                out_proj: pure_linear(&self.attn_out[l], "attn out")?,
                dim: self.d_model,
                num_heads: self.num_heads,
            });
            pre_attn_norms.push(pure_layernorm(
                &self.pre_attn_norms[l],
                self.pre_attn_eps[l],
                "pre-attn norm",
                self.d_model,
            )?);
            post_attn_norms.push(pure_layernorm(
                &self.post_attn_norms[l],
                self.post_attn_eps[l],
                "post-attn norm",
                self.d_model,
            )?);
            mlp_blocks.push(crate::model::MlpBlock {
                fc1: pure_linear(&self.mlp_fc1[l], "mlp fc1")?,
                fc2: pure_linear(&self.mlp_fc2[l], "mlp fc2")?,
            });
        }

        let log_std_raw = host_vec(self.log_std_raw.val())?;
        let log_std_raw = <[f32; LOG_STD_SLOTS]>::try_from(log_std_raw).map_err(|v| {
            TrainError::ShapeMismatch(format!(
                "log_std_raw: read back {} values, expected {LOG_STD_SLOTS}",
                v.len()
            ))
        })?;

        Ok(GemstoneSetAttentionNetwork {
            tier_proj,
            global_proj,
            attn_layers,
            pre_attn_norms,
            post_attn_norms,
            mlp_blocks,
            final_norm: pure_layernorm(
                &self.final_norm,
                self.final_eps,
                "final norm",
                self.d_model,
            )?,
            critic_head: pure_linear(&self.critic_head, "critic head")?,
            sens_head: pure_linear(&self.sens_head, "sens head")?,
            gain_head: pure_linear(&self.gain_head, "gain head")?,
            log_std_raw,
            d_model: self.d_model,
            num_heads: self.num_heads,
            d_tier: self.d_tier,
            d_global: self.d_global,
        })
    }
}

#[cfg(test)]
#[path = "burn_model_tests.rs"]
mod tests;
