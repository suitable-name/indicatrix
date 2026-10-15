//! Supervised imitation of the finite-difference teacher: the primary training algorithm.
//!
//! The environment's teacher measures, at a state, how much each of the four score components
//! (windowing, extinction, brilliance loss, yield loss) changes per degree of every free tier, plus
//! the score drop of the oracle step. The network predicts exactly those figures, and the policy
//! direction is composed from the predicted sensitivities with the objective's weight fractions, so
//! one model serves every weighting. Training is a regression onto the labels; nothing here needs a
//! reward, an advantage or a rollout probability.
//!
//! # Loss
//!
//! `loss = sens_mse + cosine_coeff * cos_loss + gain_coeff * gain_loss`.
//!
//! - **Sensitivity regression** (`sens_mse`). A label is a finite difference of a quantised score
//!   (one ray of the sampling grid is about 0.16 points), so a single label is noisy but unbiased.
//!   The mean squared error averages that noise out, and a tier whose true slope is below one
//!   quantum regresses to zero, which is the right answer for it. There is no threshold: every
//!   finite label trains. The mask keeps out padding, tiers the environment does not move, and
//!   missing entries (a NaN: the yield of a design without a preform volume, a tier whose probes
//!   both failed). The sum is divided by the number of labelled entries, so a mini-batch with few
//!   labels is not diluted.
//! - **Direction cosine** (`cos_loss`). The environment's line search picks the scale of a step,
//!   so what the policy must get right is the direction, not the magnitude. The cosine between
//!   the composed mean (the pre-squash policy direction) and the direction of the teacher's score
//!   slopes is scale-free: it keeps the direction right while the magnitudes are still off, and
//!   it trains the sensitivity head through the weight fractions of the observation. Samples
//!   without a single usable slope contribute nothing and do not count in the divisor.
//! - **Gain regression** (`gain_loss`). The gain head predicts how many points the oracle step
//!   would win at the state, which is telemetry (how much is left to gain) and a future stall
//!   rule. It uses the Huber loss with a one-point knee: the gain spans zero to tens of points and
//!   a rare large oracle jump must not dominate the sensitivity regression.
//!
//! # Updates without labels
//!
//! A mini-batch in which no entry carries any label (every sensitivity, slope and gain is missing)
//! has nothing to learn from. It is skipped instead of stepping with an all-zero gradient, because
//! Adam would still move the weights by its running momentum. Skipped mini-batches are not counted
//! in [`ImitationMetrics::updates`] and do not advance the learning-rate schedule.
//!
//! # Batch layout
//!
//! The states use the layout of the PPO batch (`[global | padded tier tokens]`, see
//! [`super::TrainBatch`]). Tier `k` of sample `i` is real when `k < n_tiers[i]`, and its labels
//! count only when `free_tiers[i][k]` holds as well: the environment never moves any other tier, so
//! a label there would teach the network something it is never asked to do.

use burn::{optim::GradientsParams, prelude::*, tensor::TensorData};
use serde::{Deserialize, Serialize};

use super::{
    LrSchedule, TrainError,
    burn_model::{BurnForward, COUNT_CLAMP_MAX, host_vec},
    burn_ppo::{BurnTrainer, clip_global_grad_norm},
};
use crate::env::{N_COMPONENTS, direction_from_slopes};

/// Added to the product of the two norms in the cosine denominator.
const COSINE_EPS: f32 = 1.0e-8;
/// Added under the square root of the predicted direction's squared norm. The gradient of a norm
/// is infinite at zero; with this term a sample without direction labels (prediction masked to
/// zero) yields a gradient of exactly zero instead of NaN.
const NORM_SQ_EPS: f32 = 1.0e-12;
/// Knee of the Huber loss on the gain, in points.
const HUBER_DELTA: f32 = 1.0;

/// Hyperparameters of the imitation trainer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImitationConfig {
    /// Adam learning rate (peak value when a decaying schedule is used).
    pub learning_rate: f32,
    /// Learning-rate schedule over optimizer updates (the trainer's own update counter).
    pub lr_schedule: LrSchedule,
    /// Samples per mini-batch (one optimizer update each); capped at the batch size.
    pub mini_batch_size: usize,
    /// Passes over the batch per call, reshuffled every pass.
    pub epochs_per_round: usize,
    /// Maximum **global** gradient L2 norm; a larger gradient is rescaled to it before the step.
    pub max_grad_norm: f32,
    /// Weight of the direction-cosine term.
    pub cosine_coeff: f32,
    /// Weight of the gain-regression term.
    pub gain_coeff: f32,
}

impl Default for ImitationConfig {
    fn default() -> Self {
        Self {
            learning_rate: 3e-4,
            lr_schedule: LrSchedule::Constant,
            mini_batch_size: 256,
            epochs_per_round: 2,
            max_grad_norm: 1.0,
            cosine_coeff: 0.5,
            gain_coeff: 0.1,
        }
    }
}

impl ImitationConfig {
    /// Reject out-of-range hyperparameters instead of silently clamping them.
    ///
    /// # Errors
    ///
    /// [`TrainError::InvalidConfig`] naming the first offending field: a zero mini-batch size or
    /// epoch count, a learning rate or gradient-norm cap that is not finite and positive, a
    /// coefficient that is not finite and non-negative, or a linear schedule over zero updates.
    pub fn validate(&self) -> Result<(), TrainError> {
        if self.mini_batch_size == 0 {
            return Err(TrainError::InvalidConfig(
                "mini_batch_size must be >= 1".into(),
            ));
        }
        if self.epochs_per_round == 0 {
            return Err(TrainError::InvalidConfig(
                "epochs_per_round must be >= 1".into(),
            ));
        }
        for (name, value) in [
            ("learning_rate", self.learning_rate),
            ("max_grad_norm", self.max_grad_norm),
        ] {
            if !(value.is_finite() && value > 0.0) {
                return Err(TrainError::InvalidConfig(format!(
                    "{name} must be finite and > 0 (got {value})"
                )));
            }
        }
        for (name, value) in [
            ("cosine_coeff", self.cosine_coeff),
            ("gain_coeff", self.gain_coeff),
        ] {
            if !(value.is_finite() && value >= 0.0) {
                return Err(TrainError::InvalidConfig(format!(
                    "{name} must be finite and >= 0 (got {value})"
                )));
            }
        }
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

/// Borrowed labelled states for one imitation call; all slices are parallel (one entry per sample).
#[derive(Debug, Clone, Copy)]
pub struct ImitationBatch<'a> {
    /// Flat observations, `[global | padded tier tokens]`, as in the PPO batch.
    pub states: &'a [Vec<f32>],
    /// Number of real (unpadded) tiers of each sample, at least 1 and at most `max_tiers`.
    pub n_tiers: &'a [usize],
    /// Free-tier mask of each sample, one flag per real tier. Labels of a tier that is not free
    /// are ignored.
    pub free_tiers: &'a [Vec<bool>],
    /// Per sample, per real tier: the four component slopes (points per degree), NaN = no label
    /// for that entry; a tier without a label is all NaN. Length `n_tiers[i]`.
    pub sensitivities: &'a [Vec<[f32; N_COMPONENTS]>],
    /// Per sample, per real tier: the score slope (points per degree, NaN = none). It is the
    /// target of the direction cosine. Length `n_tiers[i]`.
    pub score_slopes: &'a [Vec<f32>],
    /// Per sample: the score drop of the oracle step (points), NaN = none.
    pub oracle_gain: &'a [f32],
}

impl ImitationBatch<'_> {
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

    /// Check that the slices agree and that every sample has the layout the trainer expects.
    ///
    /// A missing label is NaN and is fine; what is refused is a wrong length or a state that is
    /// not finite.
    ///
    /// # Errors
    ///
    /// [`TrainError::ShapeMismatch`] naming the sample index and the field when the slices differ
    /// in length, a state does not hold `d_global + max_tiers * d_tier` values, `n_tiers[i]` is
    /// outside `1..=max_tiers`, or `free_tiers[i]`, `sensitivities[i]` or `score_slopes[i]` does
    /// not have `n_tiers[i]` entries; [`TrainError::NonFinite`] for a state holding NaN or an
    /// infinity.
    pub fn validate(
        &self,
        d_global: usize,
        d_tier: usize,
        max_tiers: usize,
    ) -> Result<(), TrainError> {
        self.validate_lengths()?;
        let row_len = d_global + max_tiers * d_tier;
        for i in 0..self.len() {
            self.validate_sample(i, row_len, max_tiers)?;
        }
        Ok(())
    }

    /// Every parallel slice holds one entry per state.
    fn validate_lengths(&self) -> Result<(), TrainError> {
        let n = self.states.len();
        let fields = [
            ("n_tiers", self.n_tiers.len()),
            ("free_tiers", self.free_tiers.len()),
            ("sensitivities", self.sensitivities.len()),
            ("score_slopes", self.score_slopes.len()),
            ("oracle_gain", self.oracle_gain.len()),
        ];
        fields
            .iter()
            .find(|&&(_, len)| len != n)
            .map_or(Ok(()), |&(name, len)| {
                Err(TrainError::ShapeMismatch(format!(
                    "batch slices disagree: {n} states but {len} entries of `{name}`"
                )))
            })
    }

    /// Layout and finiteness of sample `i`.
    fn validate_sample(
        &self,
        i: usize,
        row_len: usize,
        max_tiers: usize,
    ) -> Result<(), TrainError> {
        let mismatch = |detail: String| TrainError::ShapeMismatch(format!("sample {i}: {detail}"));
        let state = &self.states[i];
        if state.len() != row_len {
            return Err(mismatch(format!(
                "`states` row has {} values, expected {row_len} (global + {max_tiers} tiers)",
                state.len()
            )));
        }
        let n = self.n_tiers[i];
        if n == 0 || n > max_tiers {
            return Err(mismatch(format!(
                "`n_tiers` is {n}, expected 1..={max_tiers}"
            )));
        }
        for (field, len) in [
            ("free_tiers", self.free_tiers[i].len()),
            ("sensitivities", self.sensitivities[i].len()),
            ("score_slopes", self.score_slopes[i].len()),
        ] {
            if len != n {
                return Err(mismatch(format!(
                    "`{field}` has {len} entries, expected {n} (one per real tier)"
                )));
            }
        }
        if state.iter().any(|v| !v.is_finite()) {
            return Err(TrainError::NonFinite(format!(
                "sample {i}: the state holds a NaN or infinite value"
            )));
        }
        Ok(())
    }
}

/// Means over the optimizer updates of one [`BurnTrainer::train_imitation_epochs`] call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ImitationMetrics {
    /// Mean total loss (the loss each update saw before its step).
    pub loss: f32,
    /// Mean squared error of the predicted sensitivities over the labelled entries.
    pub sens_mse: f32,
    /// Mean cosine between the composed mean and the teacher's direction, over the samples that
    /// have at least one usable slope (`1` is perfect, an update without any such sample counts
    /// as `0`).
    pub cosine: f32,
    /// Mean Huber loss of the predicted gain.
    pub gain_loss: f32,
    /// Mean pre-clip global gradient L2 norm.
    pub grad_norm: f32,
    /// Real, free tiers of the batch that carry at least one finite component label; counted once
    /// over the whole batch, not once per epoch.
    pub labelled_tiers: usize,
    /// Optimizer steps taken (mini-batches without any label are skipped and not counted).
    pub updates: usize,
}

/// The five figures one update reports.
#[derive(Debug, Clone, Copy, Default)]
struct UpdateStats {
    loss: f32,
    sens_mse: f32,
    cosine: f32,
    gain_loss: f32,
    grad_norm: f32,
}

/// Add one update's figures into the running sums (the mean is taken by the caller).
const fn add_stats(sum: &mut UpdateStats, m: &UpdateStats) {
    sum.loss += m.loss;
    sum.sens_mse += m.sens_mse;
    sum.cosine += m.cosine;
    sum.gain_loss += m.gain_loss;
    sum.grad_norm += m.grad_norm;
}

/// `1.0` for `true`, `0.0` for `false`: the value of a mask entry.
const fn flag(on: bool) -> f32 {
    if on { 1.0 } else { 0.0 }
}

/// Whether tier `k` of a sample with `n` real tiers is real and free.
fn is_free_tier(k: usize, n: usize, free: &[bool]) -> bool {
    k < n && free.get(k).copied().unwrap_or(false)
}

/// The finite component labels of tier `k`; every entry is `None` unless the tier is `usable`
/// (real and free) and has a row.
fn component_labels(
    sens: &[[f32; N_COMPONENTS]],
    k: usize,
    usable: bool,
) -> [Option<f32>; N_COMPONENTS] {
    let row = sens.get(k).filter(|_| usable);
    std::array::from_fn(|c| row.map(|r| r[c]).filter(|v| v.is_finite()))
}

/// Real, free tiers of the whole batch with at least one finite component label.
fn count_labelled_tiers(batch: &ImitationBatch<'_>) -> usize {
    (0..batch.len())
        .map(|i| {
            let n = batch.n_tiers[i];
            let free = &batch.free_tiers[i];
            (0..n)
                .filter(|&k| {
                    component_labels(&batch.sensitivities[i], k, is_free_tier(k, n, free))
                        .iter()
                        .any(Option::is_some)
                })
                .count()
        })
        .sum()
}

/// Host-side label rows of one mini-batch, padded to `max_tiers`, before the upload.
///
/// Masks hold `1.0` for a usable label and `0.0` otherwise; a target is `0.0` wherever its mask is.
#[derive(Debug, Default)]
struct LabelRows {
    /// `[sample][tier][component]` flattened.
    sens_target: Vec<f32>,
    sens_mask: Vec<f32>,
    /// `[sample][tier]` flattened: the pre-squash direction of the teacher's score slopes.
    dir_target: Vec<f32>,
    dir_mask: Vec<f32>,
    /// One entry per sample.
    gain_target: Vec<f32>,
    gain_mask: Vec<f32>,
}

impl LabelRows {
    /// Whether any mask entry is set, that is, whether the mini-batch can teach anything.
    fn has_labels(&self) -> bool {
        [&self.sens_mask, &self.dir_mask, &self.gain_mask]
            .iter()
            .any(|mask| mask.iter().any(|&m| m > 0.0))
    }
}

/// Build the label rows of the samples `idx`.
///
/// - Component labels: real AND free AND finite entries.
/// - Direction: `direction_from_slopes` of the sample's finite score slopes (NaN = no slope),
///   kept where the tier is real AND free AND its slope is finite.
/// - Gain: the sample's oracle gain when finite.
fn label_rows(batch: &ImitationBatch<'_>, idx: &[usize], max_tiers: usize) -> LabelRows {
    let mut rows = LabelRows::default();
    for &i in idx {
        let n = batch.n_tiers[i];
        let free = &batch.free_tiers[i];
        let slopes: Vec<Option<f32>> = batch.score_slopes[i]
            .iter()
            .map(|&s| s.is_finite().then_some(s))
            .collect();
        let direction = direction_from_slopes(&slopes);
        for k in 0..max_tiers {
            let usable = is_free_tier(k, n, free);
            for label in component_labels(&batch.sensitivities[i], k, usable) {
                rows.sens_target.push(label.unwrap_or(0.0));
                rows.sens_mask.push(flag(label.is_some()));
            }
            let has_slope = usable && slopes.get(k).copied().flatten().is_some();
            let target = if has_slope {
                direction.get(k).copied().unwrap_or(0.0)
            } else {
                0.0
            };
            rows.dir_target.push(target);
            rows.dir_mask.push(flag(has_slope));
        }
        let gain = Some(batch.oracle_gain[i]).filter(|g| g.is_finite());
        rows.gain_target.push(gain.unwrap_or(0.0));
        rows.gain_mask.push(flag(gain.is_some()));
    }
    rows
}

/// Label tensors of one mini-batch on the training device.
struct LabelTensors {
    /// `[batch, max_tiers, N_COMPONENTS]`
    sens_target: Tensor<3>,
    sens_mask: Tensor<3>,
    /// `[batch, max_tiers]`
    dir_target: Tensor<2>,
    dir_mask: Tensor<2>,
    /// `[batch, 1]`
    gain_target: Tensor<2>,
    gain_mask: Tensor<2>,
}

/// One mini-batch on the training device.
struct PackedImitation {
    /// `[batch, max_tiers, d_tier]`
    tiers: Tensor<3>,
    /// `[batch, d_global]`
    globals: Tensor<2>,
    /// Real-tier mask (attention keys, value pooling), `[batch, max_tiers]`.
    mask: Tensor<2>,
    labels: LabelTensors,
    /// Whether any label mask entry is set.
    has_labels: bool,
}

/// Masked mean squared error of the sensitivities,
/// `sum(mask * (hat - target)^2) / max(sum mask, 1)`, shape `[1]`; exactly `0` when the mask is
/// empty.
fn masked_sens_mse(hat: Tensor<3>, target: Tensor<3>, mask: Tensor<3>) -> Tensor<1> {
    let diff = hat - target;
    let n_labels = mask.clone().sum().clamp(1.0, COUNT_CLAMP_MAX);
    (diff.clone() * diff * mask).sum() / n_labels
}

/// Masked cosine between the predicted and the target direction, per sample, over the tiers of
/// `mask` (all `[batch, max_tiers]`). Returns `(cosine, cos_loss)`, both `[1]`: the mean cosine
/// over the samples that have at least one masked tier, and `1 - cosine` over the same samples.
/// A sample without any masked tier contributes `0` to both and is left out of the divisor, so a
/// batch without any direction label gives exactly `(0, 0)`.
fn masked_cosine(hat: Tensor<2>, target: Tensor<2>, mask: Tensor<2>) -> (Tensor<1>, Tensor<1>) {
    let has_direction = mask.clone().sum_dim(1).clamp(0.0, 1.0);
    let hat = hat * mask.clone();
    let target = target * mask;
    let dot = (hat.clone() * target.clone()).sum_dim(1);
    let norm_hat = (hat.clone() * hat)
        .sum_dim(1)
        .add_scalar(NORM_SQ_EPS)
        .sqrt();
    let norm_target = (target.clone() * target).sum_dim(1).sqrt();
    let cosine = dot / (norm_hat * norm_target).add_scalar(COSINE_EPS);
    let n_samples = has_direction.clone().sum().clamp(1.0, COUNT_CLAMP_MAX);
    let cosine_sum = (cosine * has_direction.clone()).sum();
    let loss = (has_direction.sum() - cosine_sum.clone()) / n_samples.clone();
    (cosine_sum / n_samples, loss)
}

/// Masked Huber loss (knee [`HUBER_DELTA`]) of `[batch, 1]` predictions, averaged over the masked
/// samples, shape `[1]`; exactly `0` when the mask is empty.
fn masked_huber(hat: Tensor<2>, target: Tensor<2>, mask: Tensor<2>) -> Tensor<1> {
    let n_samples = mask.clone().sum().clamp(1.0, COUNT_CLAMP_MAX);
    let abs_error = ((hat - target) * mask).abs();
    // Quadratic up to the knee, linear beyond it: `0.5 q^2 + delta (a - q)`, `q = min(a, delta)`.
    let quadratic = abs_error.clone().clamp(0.0, HUBER_DELTA);
    let linear = abs_error - quadratic.clone();
    ((quadratic.clone() * quadratic).mul_scalar(0.5) + linear.mul_scalar(HUBER_DELTA)).sum()
        / n_samples
}

/// The device-side loss terms of one mini-batch, each of shape `[1]`.
struct LossTerms {
    total: Tensor<1>,
    sens_mse: Tensor<1>,
    cosine: Tensor<1>,
    gain_loss: Tensor<1>,
}

/// `sens_mse + cosine_coeff * cos_loss + gain_coeff * gain_loss` of a forward pass.
fn imitation_terms(out: BurnForward, labels: LabelTensors, config: &ImitationConfig) -> LossTerms {
    let sens_mse = masked_sens_mse(out.sensitivities, labels.sens_target, labels.sens_mask);
    let (cosine, cos_loss) = masked_cosine(out.mean, labels.dir_target, labels.dir_mask);
    let gain_loss = masked_huber(out.gain, labels.gain_target, labels.gain_mask);
    let total = sens_mse.clone()
        + cos_loss.mul_scalar(config.cosine_coeff)
        + gain_loss.clone().mul_scalar(config.gain_coeff);
    LossTerms {
        total,
        sens_mse,
        cosine,
        gain_loss,
    }
}

impl BurnTrainer {
    /// Copy the samples `idx` of `batch` into device tensors (padding tiers with zeros), labels
    /// included. The batch must have passed [`ImitationBatch::validate`].
    fn pack_imitation(&self, batch: &ImitationBatch<'_>, idx: &[usize]) -> PackedImitation {
        let (dt, dg, mt) = (self.model.d_tier, self.model.d_global, self.max_tiers);
        let b = idx.len();
        let mut globals = Vec::with_capacity(b * dg);
        let mut tiers = Vec::with_capacity(b * mt * dt);
        let mut mask = Vec::with_capacity(b * mt);
        for &i in idx {
            let state = &batch.states[i];
            globals.extend_from_slice(&state[..dg]);
            tiers.extend_from_slice(&state[dg..dg + mt * dt]);
            let n = batch.n_tiers[i];
            mask.extend((0..mt).map(|k| flag(k < n)));
        }
        let rows = label_rows(batch, idx, mt);
        let has_labels = rows.has_labels();
        let dev = &self.device;
        PackedImitation {
            tiers: Tensor::<3>::from_data(TensorData::new(tiers, [b, mt, dt]), dev),
            globals: Tensor::<2>::from_data(TensorData::new(globals, [b, dg]), dev),
            mask: Tensor::<2>::from_data(TensorData::new(mask, [b, mt]), dev),
            labels: LabelTensors {
                sens_target: Tensor::<3>::from_data(
                    TensorData::new(rows.sens_target, [b, mt, N_COMPONENTS]),
                    dev,
                ),
                sens_mask: Tensor::<3>::from_data(
                    TensorData::new(rows.sens_mask, [b, mt, N_COMPONENTS]),
                    dev,
                ),
                dir_target: Tensor::<2>::from_data(TensorData::new(rows.dir_target, [b, mt]), dev),
                dir_mask: Tensor::<2>::from_data(TensorData::new(rows.dir_mask, [b, mt]), dev),
                gain_target: Tensor::<2>::from_data(TensorData::new(rows.gain_target, [b, 1]), dev),
                gain_mask: Tensor::<2>::from_data(TensorData::new(rows.gain_mask, [b, 1]), dev),
            },
            has_labels,
        }
    }

    /// One optimizer step over the samples `idx` of an already validated batch; `None` when the
    /// mini-batch carries no label at all (no forward pass is run and the model is untouched).
    ///
    /// The gradient is clipped to the global `max_grad_norm` before the Adam step; a non-finite
    /// loss or gradient norm skips the step with [`TrainError::NonFinite`].
    fn imitation_update(
        &mut self,
        batch: &ImitationBatch<'_>,
        idx: &[usize],
        config: &ImitationConfig,
    ) -> Result<Option<UpdateStats>, TrainError> {
        let packed = self.pack_imitation(batch, idx);
        if !packed.has_labels {
            return Ok(None);
        }
        let out = self
            .model
            .forward(packed.tiers, packed.globals, packed.mask);
        let terms = imitation_terms(out, packed.labels, config);

        // One read-back for the four reported scalars.
        let figures = host_vec(Tensor::cat(
            vec![
                terms.total.clone(),
                terms.sens_mse,
                terms.cosine,
                terms.gain_loss,
            ],
            0,
        ))?;
        let [loss, sens_mse, cosine, gain_loss] = <[f32; 4]>::try_from(figures).map_err(|v| {
            TrainError::Tensor(format!("expected 4 loss statistics, read {}", v.len()))
        })?;
        if ![loss, sens_mse, cosine, gain_loss]
            .iter()
            .all(|v| v.is_finite())
        {
            return Err(TrainError::NonFinite(format!(
                "imitation loss {loss}, sensitivity mse {sens_mse}, cosine {cosine}, gain loss \
                 {gain_loss}; optimizer step skipped"
            )));
        }

        let grads = GradientsParams::from_grads(terms.total.backward(), &self.model);
        let (grads, grad_norm) = clip_global_grad_norm(grads, &self.model, config.max_grad_norm)?;
        let lr = config
            .lr_schedule
            .learning_rate(config.learning_rate, self.updates);
        self.model = self
            .optimizer
            .step(f64::from(lr), self.model.clone(), grads);
        self.updates += 1;
        Ok(Some(UpdateStats {
            loss,
            sens_mse,
            cosine,
            gain_loss,
            grad_norm,
        }))
    }

    /// Multi-epoch mini-batch imitation training over a labelled batch.
    ///
    /// Runs `config.epochs_per_round` passes; every pass reshuffles the samples (Fisher-Yates via
    /// the trainer's seeded generator, see [`Self::with_seed`]) and takes one optimizer step per
    /// mini-batch of `config.mini_batch_size` samples (capped at the batch size). A mini-batch
    /// without any label is skipped (see the module docs). The learning rate of every step comes
    /// from `config.lr_schedule` and the trainer's update counter. The returned figures are means
    /// over the updates taken; an empty batch (or one that yields no update) returns zeros, and an
    /// empty batch does not touch the model.
    ///
    /// # Errors
    ///
    /// [`TrainError::InvalidConfig`] for an invalid `config`, [`TrainError::ShapeMismatch`] /
    /// [`TrainError::NonFinite`] for a malformed batch (see [`ImitationBatch::validate`];
    /// nothing is padded or guessed), [`TrainError::NonFinite`] for a diverged loss or gradient
    /// (that step is skipped). Updates already applied before an error stay applied.
    pub fn train_imitation_epochs(
        &mut self,
        batch: &ImitationBatch<'_>,
        config: &ImitationConfig,
    ) -> Result<ImitationMetrics, TrainError> {
        config.validate()?;
        if batch.is_empty() {
            return Ok(ImitationMetrics::default());
        }
        batch.validate(self.model.d_global, self.model.d_tier, self.max_tiers)?;
        let total = batch.len();
        let mb_size = config.mini_batch_size.min(total);

        let mut sum = UpdateStats::default();
        let mut updates = 0usize;
        let mut order: Vec<usize> = (0..total).collect();
        for _ in 0..config.epochs_per_round {
            self.shuffle.shuffle(&mut order);
            for chunk in order.chunks(mb_size) {
                if let Some(stats) = self.imitation_update(batch, chunk, config)? {
                    add_stats(&mut sum, &stats);
                    updates += 1;
                }
            }
        }

        let denom = updates.max(1) as f32;
        Ok(ImitationMetrics {
            loss: sum.loss / denom,
            sens_mse: sum.sens_mse / denom,
            cosine: sum.cosine / denom,
            gain_loss: sum.gain_loss / denom,
            grad_norm: sum.grad_norm / denom,
            labelled_tiers: count_labelled_tiers(batch),
            updates,
        })
    }
}

#[cfg(test)]
#[path = "imitation_tests.rs"]
mod tests;
