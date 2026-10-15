//! The Burn trainer: PPO policy optimization with autograd over the Geometric Set Attention model.
//!
//! The model and its conventions shared with the pure-Rust twin live in `burn_model`
//! ([`BurnSetAttention`]). PPO is kept as the secondary algorithm; imitation (`imitation`) is the
//! primary one and shares [`BurnTrainer`], its optimizer and the global gradient clipping.
//!
//! # Conventions of the objective
//!
//! - The policy mean of a sample is the model's composed pre-squash direction (bounded to
//!   `[-MEAN_BOUND, MEAN_BOUND]`); `log_std` is state-independent (three learnable raw values, one
//!   per block). The standard deviation is `exp(log_std)` everywhere (rollout sampler,
//!   log-probability, entropy).
//! - The entropy is the **mean** over a sample's free tiers (matching `GaussianPolicy::entropy_masked`),
//!   not the sum.
//! - The action is the raw (pre-squash) Gaussian sample; the environment applies `tanh` and the step
//!   limit afterwards.
//!
//! # Loss
//!
//! `total = policy_loss + value_loss_coeff * value_loss - entropy_coeff * entropy`. The earlier
//! `h^2` penalty on the pre-`tanh` head output and the teacher regression term are gone: the mean is
//! bounded by construction, and per-tier supervision is the job of the imitation loss. The gain head
//! and the sensitivity head receive no PPO gradient of their own (the sensitivities do, through the
//! mean).
//!
//! # Conditioning of the composed mean under PPO
//!
//! The mean is `-MEAN_BOUND * g / (g_max + SLOPE_EPS)` per tier: a direction, normalised by the
//! steepest tier. Its Jacobian with respect to the sensitivities scales with `1 / g_max`, so
//! while the sensitivities are small (at initialisation, or on a design whose score is flat) a
//! parameter change moves the means by a multiple of the policy's std. Measured on the unit-test
//! model: one Adam step at a learning rate of `1e-3` shifts the means by about 0.3 pre-squash units
//! (60 % of the initial std) and lowers the log-probability of the policy's own samples by nats;
//! the approximate KL of a single update reaches 1 to 40, so the KL stop fires. PPO through this
//! head therefore needs a learning rate around `1e-4` and still couples the value loss into the
//! means through the shared trunk. That is why imitation, which fits the sensitivities directly
//! (a well-conditioned regression) and the direction by a scale-free cosine, is the primary
//! algorithm; PPO is kept for experiments.
//!
//! # Gradient clipping
//!
//! The global L2 norm over **all** parameters is clipped to `max_grad_norm` (see
//! `clip_global_grad_norm`). Burn's built-in `GradientClippingConfig::Norm` clips each parameter tensor
//! on its own, which rescales the large critic gradients tensor by tensor while the small policy
//! gradients pass untouched; it is not used.
//!
//! # Batch layout
//!
//! A state row is `[global (d_global) | tier 0 (d_tier) | ... | tier max_tiers-1]`, exactly what
//! `GemstoneObservation::flatten(max_tiers)` produces (global token followed by the padded tier
//! tokens). Tiers beyond `n_tiers[i]` are padding: they are masked out of attention (keys), of the
//! pooled value input, of the log-probability and of the entropy.
//!
//! # Free-tier mask (action objective)
//!
//! Tiers whose sampled action the environment will not use (pinned, flat, or outside their angle
//! envelope so the projection overrides them, see `GemstoneObservation::free_tier_mask`) are
//! real tiers and stay visible to attention and the value head, but their action dimension is excluded
//! from the log-probability, the entropy and the action sample: `TrainBatch::free_tiers` carries one
//! `bool` per real tier of each sample, and the action mask is `real AND free`. The training
//! objective therefore only fits and rewards the dimensions that reach the environment.

use burn::{
    module::{ModuleVisitor, Param},
    optim::{AdamConfig, GradientsParams, ModuleOptimizer},
    prelude::*,
    tensor::TensorData,
};

pub(crate) use super::burn_model::{host_scalar, host_vec};
use super::{
    PpoConfig, TrainError, TrainingMetrics,
    burn_model::{BurnSetAttention, COUNT_CLAMP_MAX},
    burn_model_host::{PackDims, pack_states},
};
use crate::model::{
    GemstoneSetAttentionNetwork, Prng,
    policy::{HALF_LN_2PI, HALF_LN_2PI_E},
};

/// Largest `max_tiers` accepted by the trainer; bounds the attention sequence length and memory.
pub const MAX_TIERS_LIMIT: usize = 64;

/// The update loop stops after the first mini-batch whose approximate KL exceeds this multiple of
/// `target_kl` (checked after every mini-batch, not once per epoch).
const KL_STOP_FACTOR: f32 = 1.5;
/// Added to the gradient norm in the clipping scale `max_norm / (norm + EPS)`.
const CLIP_NORM_EPS: f32 = 1.0e-6;
/// A rollout whose critic targets have a larger standard deviation was not normalised (the buffer
/// scales rewards, TQ-15); the value loss would then drown the policy gradient.
const MAX_RETURNS_STD: f64 = 100.0;

/// Per-sample masked log-probability and entropy of a diagonal Gaussian over the tier dimensions.
///
/// `actions`, `mean`, `log_std` and `mask` are `[batch, max_tiers]`; masked dimensions (`mask == 0`:
/// padding, and real tiers the environment will not move) contribute nothing. Returns `(log_prob, entropy)`,
/// both `[batch, 1]`: the log-probability is the **sum** over the unmasked dimensions, the entropy their
/// **mean** (`Σ(mask·(log_std + ½ln2πe)) / clamp(Σmask, 1, ∞)`, so `0.0` for a row with nothing free).
/// The standard deviation is `exp(log_std)` with no second clamp (the forward pass already bounded
/// `log_std`).
pub(crate) fn masked_log_prob_and_entropy(
    actions: Tensor<2>,
    mean: Tensor<2>,
    log_std: Tensor<2>,
    mask: Tensor<2>,
) -> (Tensor<2>, Tensor<2>) {
    let std = log_std.clone().exp();
    let diff = (actions - mean) / std;
    let per_dim = -(diff.clone() * diff).mul_scalar(0.5) - log_std.clone() - HALF_LN_2PI;
    let n_free = mask.clone().sum_dim(1).clamp(1.0, COUNT_CLAMP_MAX);
    let log_prob = (per_dim * mask.clone()).sum_dim(1);
    let entropy = (log_std.add_scalar(HALF_LN_2PI_E) * mask).sum_dim(1) / n_free;
    (log_prob, entropy)
}

/// The PPO clipped surrogate: `(policy_loss, ratio, log_ratio)` with shapes `[1]`, `[batch, 1]`,
/// `[batch, 1]`. The log-ratio is clamped to ±10 before the exponential.
pub(crate) fn clipped_surrogate(
    new_log_prob: Tensor<2>,
    old_log_probs: Tensor<2>,
    advantages: Tensor<2>,
    eps: f32,
) -> (Tensor<1>, Tensor<2>, Tensor<2>) {
    let log_ratio = (new_log_prob - old_log_probs).clamp(-10.0, 10.0);
    let ratio = log_ratio.clone().exp();
    let surr1 = ratio.clone() * advantages.clone();
    let surr2 = ratio.clone().clamp(1.0 - eps, 1.0 + eps) * advantages;

    let diff_surr = (surr1.clone() - surr2.clone()).abs();
    let min_surr = (surr1 + surr2 - diff_surr).mul_scalar(0.5);
    (-min_surr.mean(), ratio, log_ratio)
}

/// Clip fraction and approximate KL of a mini-batch from the ratios the loss used.
pub(crate) fn ratio_stats(
    ratio: Tensor<2>,
    log_ratio: Tensor<2>,
    eps: f32,
) -> Result<(f32, f32), TrainError> {
    let ratios = host_vec(ratio)?;
    let log_ratios = host_vec(log_ratio)?;
    let denom = ratios.len().max(1) as f32;
    let clip_count = ratios.iter().filter(|&&r| (r - 1.0).abs() > eps).count();
    let kl = ratios
        .iter()
        .zip(&log_ratios)
        .map(|(&r, &lr)| (r - 1.0) - lr)
        .sum::<f32>()
        / denom;
    Ok((clip_count as f32 / denom, kl.max(0.0)))
}

/// Sums the squared entries of every parameter gradient into one device scalar.
struct GradSquareSum<'a> {
    grads: &'a GradientsParams,
    total: Option<Tensor<1>>,
}

impl ModuleVisitor for GradSquareSum<'_> {
    fn visit_float<const D: usize>(&mut self, param: &Param<Tensor<D>>) {
        let Some(grad) = self.grads.get::<D>(param.id) else {
            return;
        };
        // `g * g` rather than `powf_scalar(2.0)`: a power of a negative base is undefined on a GPU.
        let square_sum = (grad.clone() * grad).sum();
        self.total = Some(match self.total.take() {
            Some(total) => total + square_sum,
            None => square_sum,
        });
    }
}

/// Multiplies every parameter gradient by `scale`.
struct GradScale<'a> {
    grads: &'a mut GradientsParams,
    scale: f32,
}

impl ModuleVisitor for GradScale<'_> {
    fn visit_float<const D: usize>(&mut self, param: &Param<Tensor<D>>) {
        if let Some(grad) = self.grads.remove::<D>(param.id) {
            self.grads
                .register::<D>(param.id, grad.mul_scalar(self.scale));
        }
    }
}

/// Global L2 norm of the gradients of every parameter of `model` (one `host_scalar` read).
fn global_grad_norm(grads: &GradientsParams, model: &BurnSetAttention) -> Result<f32, TrainError> {
    let mut visitor = GradSquareSum { grads, total: None };
    model.visit(&mut visitor);
    match visitor.total {
        Some(total) => host_scalar(total.sqrt()),
        None => Ok(0.0),
    }
}

/// Scales every gradient so the global L2 norm is at most `max_norm`; returns the norm before clipping.
///
/// Burn's own `GradientClippingConfig::Norm` clips each parameter tensor on its own, which distorts
/// the update direction when one tensor (the critic's trunk) has a far larger gradient than the rest.
///
/// # Errors
///
/// [`TrainError::NonFinite`] when the norm is NaN or infinite (the caller skips the optimizer step),
/// [`TrainError::Tensor`] when the norm cannot be read back.
pub(crate) fn clip_global_grad_norm(
    mut grads: GradientsParams,
    model: &BurnSetAttention,
    max_norm: f32,
) -> Result<(GradientsParams, f32), TrainError> {
    let norm = global_grad_norm(&grads, model)?;
    if !norm.is_finite() {
        return Err(TrainError::NonFinite(format!(
            "gradient norm {norm}; optimizer step skipped"
        )));
    }
    if norm > max_norm {
        let mut scaler = GradScale {
            grads: &mut grads,
            scale: max_norm / (norm + CLIP_NORM_EPS),
        };
        model.visit(&mut scaler);
    }
    Ok((grads, norm))
}

/// Borrowed training data for one PPO update; all slices are parallel (one entry per sample).
#[derive(Debug, Clone, Copy)]
pub struct TrainBatch<'a> {
    /// Flattened observations, `[global | padded tier tokens]`, see the module docs.
    pub states: &'a [Vec<f32>],
    /// Raw (pre-squash) Gaussian samples per tier; length between `n_tiers[i]` and `max_tiers`.
    pub actions: &'a [Vec<f32>],
    /// Log-probability of each action under the sampling policy.
    pub old_log_probs: &'a [f32],
    /// Standardised advantages.
    pub advantages: &'a [f32],
    /// Critic regression targets. They must be on an O(1) scale (the buffer divides the rewards by a
    /// running return deviation): a batch whose standard deviation exceeds 100 is rejected.
    pub returns: &'a [f32],
    /// Number of real (unpadded) tiers of each sample, at least 1 and at most `max_tiers`.
    pub n_tiers: &'a [usize],
    /// Free-tier mask of each sample: `free_tiers[i][k]` is `true` when the sampled action of real tier `k`
    /// reaches the environment (`GemstoneObservation::free_tier_mask`). Length exactly `n_tiers[i]`; the trainer pads
    /// it with `false` up to `max_tiers`. Non-free tiers are excluded from log-prob, entropy and sampling.
    pub free_tiers: &'a [Vec<bool>],
}

impl TrainBatch<'_> {
    /// Number of samples.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.states.len()
    }

    /// Whether the batch holds no samples.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    fn validate(&self, d_global: usize, d_tier: usize, max_tiers: usize) -> Result<(), TrainError> {
        let n = self.states.len();
        if self.actions.len() != n
            || self.old_log_probs.len() != n
            || self.advantages.len() != n
            || self.returns.len() != n
            || self.n_tiers.len() != n
            || self.free_tiers.len() != n
        {
            return Err(TrainError::ShapeMismatch(format!(
                "batch slices disagree: {n} states / {} actions / {} old_log_probs / {} advantages / {} returns / {} n_tiers / {} free_tiers",
                self.actions.len(),
                self.old_log_probs.len(),
                self.advantages.len(),
                self.returns.len(),
                self.n_tiers.len(),
                self.free_tiers.len()
            )));
        }
        let row_len = d_global + max_tiers * d_tier;
        for i in 0..n {
            if self.states[i].len() != row_len {
                return Err(TrainError::ShapeMismatch(format!(
                    "state {i} has {} values, expected {row_len} (= {d_global} global + {max_tiers} tiers x {d_tier})",
                    self.states[i].len()
                )));
            }
            let nt = self.n_tiers[i];
            if nt == 0 || nt > max_tiers {
                return Err(TrainError::ShapeMismatch(format!(
                    "sample {i} has {nt} tiers, expected 1..={max_tiers}"
                )));
            }
            if self.free_tiers[i].len() != nt {
                return Err(TrainError::ShapeMismatch(format!(
                    "sample {i} has {} free-tier flags, expected {nt} (one per real tier)",
                    self.free_tiers[i].len()
                )));
            }
            let al = self.actions[i].len();
            if al < nt || al > max_tiers {
                return Err(TrainError::ShapeMismatch(format!(
                    "action {i} has {al} entries, expected {nt}..={max_tiers}"
                )));
            }
            if !self.old_log_probs[i].is_finite()
                || !self.advantages[i].is_finite()
                || !self.returns[i].is_finite()
                || self.states[i].iter().any(|v| !v.is_finite())
                || self.actions[i].iter().any(|v| !v.is_finite())
            {
                return Err(TrainError::NonFinite(format!(
                    "sample {i} holds a NaN or infinite value"
                )));
            }
        }
        let std = returns_std(self.returns);
        if std > MAX_RETURNS_STD {
            return Err(TrainError::InvalidConfig(format!(
                "returns are not normalised (std {std:.1}); see TQ-15"
            )));
        }
        Ok(())
    }
}

/// Population standard deviation of the critic targets (`0.0` for an empty slice).
fn returns_std(returns: &[f32]) -> f64 {
    if returns.is_empty() {
        return 0.0;
    }
    let n = returns.len() as f64;
    let mean = returns.iter().map(|&r| f64::from(r)).sum::<f64>() / n;
    let variance = returns
        .iter()
        .map(|&r| (f64::from(r) - mean).powi(2))
        .sum::<f64>()
        / n;
    variance.sqrt()
}

/// Dimensions of the model and of the padded tier axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelShape {
    /// Per-tier feature dimension (`TierFeature::FEATURE_DIM`).
    pub d_tier: usize,
    /// Global feature dimension (`GlobalFeature::FEATURE_DIM`).
    pub d_global: usize,
    /// Hidden dimension.
    pub d_model: usize,
    /// Attention heads.
    pub num_heads: usize,
    /// Transformer layers.
    pub num_layers: usize,
    /// Padded tier count of the training batches: the largest design in the data set, at most
    /// [`MAX_TIERS_LIMIT`]. Designs with more tiers must be rejected when the data set is loaded.
    pub max_tiers: usize,
}

/// Mini-batch tensors on the training device.
struct PackedBatch {
    tiers: Tensor<3>,
    globals: Tensor<2>,
    actions: Tensor<2>,
    /// Real-tier mask (attention keys, value pooling), `[batch, max_tiers]`.
    mask: Tensor<2>,
    /// Action mask = real AND free (log-prob, entropy), `[batch, max_tiers]`.
    action_mask: Tensor<2>,
    returns: Tensor<2>,
    advantages: Tensor<2>,
    old_log_probs: Tensor<2>,
}

/// Whether tier `k` of a sample with `n` real tiers is real and free (its action reaches the
/// environment and enters the loss).
fn is_action_tier(k: usize, n: usize, free: &[bool]) -> bool {
    k < n && free.get(k).copied().unwrap_or(false)
}

/// Burn trainer managing automatic differentiation updates for Geometric Set Attention models:
/// PPO here, the imitation loss in `imitation`.
pub struct BurnTrainer {
    /// Geometric Set Attention model instance.
    pub model: BurnSetAttention,
    /// Module optimizer with autograd.
    pub optimizer: ModuleOptimizer,
    /// PPO hyperparameter configuration.
    pub config: PpoConfig,
    /// Target device.
    pub device: Device,
    /// Padded tier count of the batches this trainer accepts.
    pub max_tiers: usize,
    /// Optimizer steps taken so far (drives the learning-rate schedule).
    pub(crate) updates: u64,
    /// Mini-batch shuffling generator.
    pub(crate) shuffle: Prng,
}

impl BurnTrainer {
    /// Plain Adam. Gradient clipping is **not** configured here: Burn's built-in clipping works per
    /// parameter tensor, so the global norm is clipped by `clip_global_grad_norm` before the step.
    fn build_optimizer() -> ModuleOptimizer {
        AdamConfig::new().init()
    }

    /// Initialize trainer targeting `device`.
    ///
    /// # Errors
    ///
    /// [`TrainError::InvalidConfig`] for an invalid [`PpoConfig`], a zero or oversized `max_tiers`
    /// (limit [`MAX_TIERS_LIMIT`]) or invalid model dimensions.
    pub fn new(shape: ModelShape, config: PpoConfig, device: Device) -> Result<Self, TrainError> {
        config.validate()?;
        if shape.max_tiers == 0 || shape.max_tiers > MAX_TIERS_LIMIT {
            return Err(TrainError::InvalidConfig(format!(
                "max_tiers must be 1..={MAX_TIERS_LIMIT} (got {})",
                shape.max_tiers
            )));
        }
        let autodiff_device = device.autodiff();
        let model = BurnSetAttention::new(
            shape.d_tier,
            shape.d_global,
            shape.d_model,
            shape.num_heads,
            shape.num_layers,
            &autodiff_device,
        )?;
        let optimizer = Self::build_optimizer();
        Ok(Self {
            model,
            optimizer,
            config,
            device: autodiff_device,
            max_tiers: shape.max_tiers,
            updates: 0,
            shuffle: Prng::new(0),
        })
    }

    /// Seed the mini-batch shuffling generator (default seed 0); same seed, same update sequence.
    #[must_use]
    pub const fn with_seed(mut self, seed: u64) -> Self {
        self.shuffle = Prng::new(seed);
        self
    }

    /// Create a trainer targeting the GPU (via WebGPU / Vulkan / DX12).
    ///
    /// # Errors
    ///
    /// [`TrainError::DeviceUnavailable`] when no usable adapter exists (checked with a one-element tensor
    /// operation); there is no silent fallback. Use [`Self::new_auto`] to opt into one.
    pub fn new_gpu(shape: ModelShape, config: PpoConfig) -> Result<Self, TrainError> {
        let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let device = Device::wgpu(burn::tensor::DeviceKind::default());
            probe_device(&device).map(|()| device)
        }));
        match attempt {
            Ok(Ok(device)) => Self::new(shape, config, device),
            Ok(Err(e)) => Err(e),
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("panic while initialising the GPU backend");
                Err(TrainError::DeviceUnavailable(msg.to_owned()))
            }
        }
    }

    /// Create a trainer targeting CPU SIMD (Flex).
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn new_cpu(shape: ModelShape, config: PpoConfig) -> Result<Self, TrainError> {
        Self::new(shape, config, Device::flex())
    }

    /// Create a trainer on the GPU when one works, otherwise on the CPU.
    ///
    /// The second tuple element carries the reason the GPU was skipped (`None` when the GPU is in use);
    /// the caller is expected to log it as a warning.
    ///
    /// # Errors
    ///
    /// As [`Self::new`]; a GPU failure alone is not an error here.
    pub fn new_auto(
        shape: ModelShape,
        config: PpoConfig,
    ) -> Result<(Self, Option<TrainError>), TrainError> {
        match Self::new_gpu(shape, config.clone()) {
            Ok(t) => Ok((t, None)),
            Err(gpu_err @ TrainError::DeviceUnavailable(_)) => {
                Self::new_cpu(shape, config).map(|t| (t, Some(gpu_err)))
            }
            Err(other) => Err(other),
        }
    }

    /// Replace the model weights with those of a pure-Rust network and reset the optimizer state.
    ///
    /// Used to resume from a checkpoint and to move a trained model to a freshly created trainer on
    /// another device.
    ///
    /// # Errors
    ///
    /// [`TrainError::ShapeMismatch`] when the network's dimensions differ from this trainer's model or
    /// its arrays are inconsistent; the current model is left untouched.
    pub fn load_weights(&mut self, net: &GemstoneSetAttentionNetwork) -> Result<(), TrainError> {
        if net.d_tier != self.model.d_tier
            || net.d_global != self.model.d_global
            || net.d_model != self.model.d_model
            || net.num_heads != self.model.num_heads
            || net.attn_layers.len() != self.model.num_layers
        {
            return Err(TrainError::ShapeMismatch(format!(
                "network (d_tier {}, d_global {}, d_model {}, heads {}, layers {}) does not match the trainer model \
                 (d_tier {}, d_global {}, d_model {}, heads {}, layers {})",
                net.d_tier,
                net.d_global,
                net.d_model,
                net.num_heads,
                net.attn_layers.len(),
                self.model.d_tier,
                self.model.d_global,
                self.model.d_model,
                self.model.num_heads,
                self.model.num_layers
            )));
        }
        self.model = BurnSetAttention::from_set_attention(net, &self.device)?;
        self.optimizer = Self::build_optimizer();
        Ok(())
    }

    /// Number of optimizer steps taken so far.
    #[must_use]
    pub const fn update_count(&self) -> u64 {
        self.updates
    }

    /// Restores the optimizer-step count of a resumed run, so the learning-rate schedule
    /// continues where the checkpoint left it.
    pub const fn set_update_count(&mut self, updates: u64) {
        self.updates = updates;
    }

    /// An inference copy of the trainer's current model, for batched rollouts
    /// ([`BurnSetAttention::forward_host`]).
    ///
    /// The copy holds the same weights on the same device without the autodiff association
    /// (`Module::valid()`): its forward passes track no gradient, and it shares the parameter
    /// data with the trainer's model instead of duplicating it. It is a snapshot. An optimizer
    /// step builds new parameter tensors for the trainer and leaves the copy at the weights of
    /// the moment it was taken, so it must be refreshed (call this again) after every update
    /// that the next rollout should see; the runner does so after each round. Taking the copy
    /// is cheap (a re-wrap of every parameter handle, no data transfer).
    #[must_use]
    pub fn rollout_model(&self) -> BurnSetAttention {
        self.model.valid()
    }

    /// Copy the samples `idx` of `batch` into device tensors (padding actions and tiers with zeros).
    ///
    /// The states (global tokens, tier tokens, real-tier mask) are packed by the helper the
    /// batched rollout forward shares (`pack_states`); the PPO-only columns (actions, action
    /// mask, returns, advantages, old log-probabilities) are packed here.
    fn pack(&self, batch: &TrainBatch<'_>, idx: &[usize]) -> PackedBatch {
        let mt = self.max_tiers;
        let b = idx.len();
        let mut actions = Vec::with_capacity(b * mt);
        let mut action_mask = Vec::with_capacity(b * mt);
        let mut returns = Vec::with_capacity(b);
        let mut advantages = Vec::with_capacity(b);
        let mut old_lp = Vec::with_capacity(b);

        for &i in idx {
            let a = &batch.actions[i];
            actions.extend_from_slice(a);
            actions.resize(actions.len() + (mt - a.len()), 0.0);
            let n = batch.n_tiers[i];
            let free = &batch.free_tiers[i];
            action_mask.extend((0..mt).map(|k| {
                if is_action_tier(k, n, free) {
                    1.0f32
                } else {
                    0.0
                }
            }));
            returns.push(batch.returns[i]);
            advantages.push(batch.advantages[i]);
            old_lp.push(batch.old_log_probs[i]);
        }

        let dev = &self.device;
        let states = pack_states(
            idx.iter()
                .map(|&i| (batch.states[i].as_slice(), batch.n_tiers[i])),
            PackDims {
                d_global: self.model.d_global,
                d_tier: self.model.d_tier,
                max_tiers: mt,
            },
            dev,
        );
        PackedBatch {
            tiers: states.tiers,
            globals: states.globals,
            actions: Tensor::<2>::from_data(TensorData::new(actions, [b, mt]), dev),
            mask: states.mask,
            action_mask: Tensor::<2>::from_data(TensorData::new(action_mask, [b, mt]), dev),
            returns: Tensor::<2>::from_data(TensorData::new(returns, [b, 1]), dev),
            advantages: Tensor::<2>::from_data(TensorData::new(advantages, [b, 1]), dev),
            old_log_probs: Tensor::<2>::from_data(TensorData::new(old_lp, [b, 1]), dev),
        }
    }

    /// One PPO step over the samples `idx` of an already validated batch.
    ///
    /// Loss: `policy_loss + value_loss_coeff * value_loss - entropy_coeff * entropy` (see the module
    /// docs). The gradient is clipped to the global `max_grad_norm` before the Adam step; a non-finite
    /// loss or gradient norm skips the step with [`TrainError::NonFinite`].
    fn train_indices(
        &mut self,
        batch: &TrainBatch<'_>,
        idx: &[usize],
    ) -> Result<TrainingMetrics, TrainError> {
        let p = self.pack(batch, idx);
        let out = self.model.forward(p.tiers, p.globals, p.mask);
        let (new_log_prob, entropy_per_sample) =
            masked_log_prob_and_entropy(p.actions, out.mean, out.log_std, p.action_mask);

        let eps = self.config.clip_epsilon;
        let (policy_loss, ratio, log_ratio) =
            clipped_surrogate(new_log_prob, p.old_log_probs, p.advantages, eps);

        let val_diff = out.value - p.returns;
        let value_loss = (val_diff.clone() * val_diff).mean().mul_scalar(0.5);
        let entropy = entropy_per_sample.mean();

        let total_loss = policy_loss.clone()
            + value_loss.clone().mul_scalar(self.config.value_loss_coeff)
            - entropy.clone().mul_scalar(self.config.entropy_coeff);

        // One read-back for the three reported scalars.
        let stats = host_vec(Tensor::cat(vec![policy_loss, value_loss, entropy], 0))?;
        let [p_loss, v_loss, ent] = <[f32; 3]>::try_from(stats).map_err(|v| {
            TrainError::Tensor(format!("expected 3 loss statistics, read {}", v.len()))
        })?;
        if ![p_loss, v_loss, ent].iter().all(|v| v.is_finite()) {
            return Err(TrainError::NonFinite(format!(
                "policy loss {p_loss}, value loss {v_loss}, entropy {ent}; optimizer step skipped"
            )));
        }

        let grads = GradientsParams::from_grads(total_loss.backward(), &self.model);
        let (grads, grad_norm) =
            clip_global_grad_norm(grads, &self.model, self.config.max_grad_norm)?;
        let lr = self
            .config
            .lr_schedule
            .learning_rate(self.config.learning_rate, self.updates);
        self.model = self
            .optimizer
            .step(f64::from(lr), self.model.clone(), grads);
        self.updates += 1;

        let (clip_fraction, approx_kl) = ratio_stats(ratio, log_ratio, eps)?;
        Ok(TrainingMetrics {
            policy_loss: p_loss,
            value_loss: v_loss,
            entropy: ent,
            approx_kl,
            clip_fraction,
            early_stopped: false,
            grad_norm,
        })
    }

    /// Perform a single PPO clipped surrogate optimization step over the whole batch.
    ///
    /// # Errors
    ///
    /// [`TrainError::ShapeMismatch`] / [`TrainError::NonFinite`] for malformed input (nothing is padded
    /// or guessed), [`TrainError::NonFinite`] for a diverged loss (the step is skipped).
    pub fn train_batch(&mut self, batch: &TrainBatch<'_>) -> Result<TrainingMetrics, TrainError> {
        if batch.is_empty() {
            return Ok(TrainingMetrics::default());
        }
        batch.validate(self.model.d_global, self.model.d_tier, self.max_tiers)?;
        let idx: Vec<usize> = (0..batch.len()).collect();
        self.train_indices(batch, &idx)
    }

    /// Multi-epoch mini-batch PPO training over a rollout buffer.
    ///
    /// Uses `config.epochs_per_batch` and `config.mini_batch_size`; the sample order is reshuffled every
    /// epoch (Fisher-Yates via [`Prng::shuffle`], seeded by [`Self::with_seed`]). After **every**
    /// mini-batch its approximate KL is compared with `1.5 * config.target_kl`; the first one that
    /// exceeds it ends the call (the rest of the epoch and all remaining epochs are skipped) and sets
    /// [`TrainingMetrics::early_stopped`], so a bad epoch cannot run to its end. The returned metrics
    /// are means over all updates taken (`grad_norm` is the mean pre-clip global norm).
    ///
    /// # Errors
    ///
    /// As [`Self::train_batch`]. Updates already applied before an error stay applied.
    pub fn train_rollout_epochs(
        &mut self,
        batch: &TrainBatch<'_>,
    ) -> Result<TrainingMetrics, TrainError> {
        if batch.is_empty() {
            return Ok(TrainingMetrics::default());
        }
        batch.validate(self.model.d_global, self.model.d_tier, self.max_tiers)?;
        let total = batch.len();
        let mb_size = self.config.mini_batch_size.min(total);

        let mut sum = TrainingMetrics::default();
        let mut num_updates = 0usize;
        let mut early_stopped = false;
        let mut order: Vec<usize> = (0..total).collect();

        'epochs: for _ in 0..self.config.epochs_per_batch {
            self.shuffle.shuffle(&mut order);
            for chunk in order.chunks(mb_size) {
                let m = self.train_indices(batch, chunk)?;
                add_metrics(&mut sum, &m);
                num_updates += 1;
                if m.approx_kl > KL_STOP_FACTOR * self.config.target_kl {
                    early_stopped = true;
                    break 'epochs;
                }
            }
        }

        let denom = num_updates.max(1) as f32;
        Ok(TrainingMetrics {
            policy_loss: sum.policy_loss / denom,
            value_loss: sum.value_loss / denom,
            entropy: sum.entropy / denom,
            approx_kl: sum.approx_kl / denom,
            clip_fraction: sum.clip_fraction / denom,
            early_stopped,
            grad_norm: sum.grad_norm / denom,
        })
    }
}

/// Add one update's metrics into the running sums (the mean is taken by the caller).
fn add_metrics(sum: &mut TrainingMetrics, m: &TrainingMetrics) {
    sum.policy_loss += m.policy_loss;
    sum.value_loss += m.value_loss;
    sum.entropy += m.entropy;
    sum.clip_fraction += m.clip_fraction;
    sum.approx_kl += m.approx_kl;
    sum.grad_norm += m.grad_norm;
}

/// Run a one-element tensor operation to prove the device works.
fn probe_device(device: &Device) -> Result<(), TrainError> {
    let t =
        Tensor::<2>::from_data(TensorData::new(vec![1.0f32, 2.0], [1, 2]), device).add_scalar(1.0);
    let v = host_vec(t)?;
    if v == [2.0, 3.0] {
        Ok(())
    } else {
        Err(TrainError::DeviceUnavailable(format!(
            "device probe returned {v:?}, expected [2.0, 3.0]"
        )))
    }
}

#[cfg(test)]
#[path = "burn_ppo_tests.rs"]
mod burn_ppo_tests;
