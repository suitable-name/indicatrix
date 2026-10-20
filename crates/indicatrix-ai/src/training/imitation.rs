//! Bounded supervised imitation of the seven-component finite-difference teacher.
//!
//! Component sensitivities use a masked Huber regression. Raw masked MSE and per-component support
//! remain telemetry. Direction cosine ignores zero-norm targets; oracle gain uses a separate Huber
//! term. Immobile and padded tiers remain context tokens but cannot receive action or label weight.
//! Selected rows are packed once per bounded window and gathered into device mini-batches under a
//! 32 MiB prepared-input payload bound. Unavailable metrics are masked out.

use burn::{optim::GradientsParams, prelude::*, tensor::TensorData};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;

use super::{
    LrSchedule, TrainError,
    burn_model::{BurnForward, COUNT_CLAMP_MAX, host_vec},
    burn_ppo::{BurnTrainer, clip_global_grad_norm},
};
use crate::{
    env::{GLOBAL_DIM, MOBILITY_INDEX, N_COMPONENTS, TIER_DIM, direction_from_slopes},
    model::Prng,
};

/// Added to the product of the two norms in the cosine denominator.
const COSINE_EPS: f32 = 1.0e-8;
/// Added under the square root of the predicted direction's squared norm. The gradient of a norm
/// is infinite at zero; with this term a sample without direction labels (prediction masked to
/// zero) yields a gradient of exactly zero instead of NaN.
const NORM_SQ_EPS: f32 = 1.0e-12;
/// Knee of the Huber loss on the gain, in points.
const HUBER_DELTA: f32 = 1.0;
/// Maximum observation and label tensor payload for one prepared window; requested updates are
/// streamed through additional windows rather than truncated to fit.
const PREPARED_ROUND_BUDGET_BYTES: usize = 32 * 1024 * 1024;
/// Bounds the schedule metadata when a tiny replay subset is sampled repeatedly.
const PREPARED_BATCHES_PER_WINDOW: usize = 4096;

/// Default of [`ImitationConfig::updates_per_round`]: 64 mini-batches of 256 samples per round.
const DEFAULT_UPDATES_PER_ROUND: usize = 64;

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
    /// Passes over the batch per call, reshuffled every pass. Used only when
    /// [`Self::updates_per_round`] is `0`.
    pub epochs_per_round: usize,
    /// Optimizer updates per call. `0` restores the full-epoch behaviour (`epochs_per_round`
    /// passes over the whole batch, a cost that grows with the batch). `n > 0` takes exactly `n`
    /// mini-batches of `mini_batch_size` samples, each drawn uniformly without replacement from
    /// the batch (mini-batches are independent of each other), so the cost of a call does not
    /// depend on the batch size. A batch smaller than one mini-batch is used whole every time.
    pub updates_per_round: usize,
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
            updates_per_round: DEFAULT_UPDATES_PER_ROUND,
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
    /// Per sample, per real tier: the seven component slopes (points per degree), NaN = no label
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
        if d_global != GLOBAL_DIM || d_tier != TIER_DIM {
            return Err(TrainError::ShapeMismatch(format!(
                "imitation data uses observation layout {}x{}, trainer model is {}x{}",
                GLOBAL_DIM, TIER_DIM, d_global, d_tier
            )));
        }
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
    /// Number of finite, mobile-tier labels for each objective component, in env component order.
    /// This reports target support independently of raw MSE, so an unavailable metric is visible.
    #[serde(default)]
    pub component_support: [usize; N_COMPONENTS],
    /// Optimizer steps taken (mini-batches without any label are skipped and not counted).
    pub updates: usize,
    /// Cumulative host elapsed milliseconds spent preparing and uploading bounded row windows.
    /// This is not device-kernel time; asynchronous device work may spill into later phases.
    #[serde(default)]
    pub packing_ms: f32,
    /// Cumulative host elapsed milliseconds spent gathering and updating mini-batches, including
    /// existing device readbacks for loss and gradient guards. Not GPU utilization.
    #[serde(default)]
    pub update_ms: f32,
}

/// Loss/gradient aggregates and cumulative host timings for one imitation call.
#[derive(Debug, Clone, Copy, Default)]
struct UpdateStats {
    loss: f32,
    sens_mse: f32,
    cosine: f32,
    gain_loss: f32,
    grad_norm: f32,
    packing_ms: f32,
    update_ms: f32,
}

/// Add one update's figures into the running sums (the mean is taken by the caller).
const fn add_stats(sum: &mut UpdateStats, m: &UpdateStats) {
    sum.loss += m.loss;
    sum.sens_mse += m.sens_mse;
    sum.cosine += m.cosine;
    sum.gain_loss += m.gain_loss;
    sum.grad_norm += m.grad_norm;
    sum.packing_ms += m.packing_ms;
    sum.update_ms += m.update_ms;
}

/// Move `count` uniformly drawn entries of `order` to its front (a partial Fisher-Yates step), so
/// that `order[..count]` is a uniform sample without replacement. `order` must stay a permutation
/// of the sample indices between calls; whatever permutation it holds, the draw is uniform.
fn draw_minibatch(rng: &mut Prng, order: &mut [usize], count: usize) {
    let total = order.len();
    for at in 0..count.min(total) {
        let pick = at + rng.next_index(total - at);
        order.swap(at, pick);
    }
}

/// `1.0` for `true`, `0.0` for `false`: the value of a mask entry.
const fn flag(on: bool) -> f32 {
    if on { 1.0 } else { 0.0 }
}

/// Whether tier `k` can receive an action and has a finite-difference label.
fn is_actionable_tier(batch: &ImitationBatch<'_>, sample: usize, k: usize) -> bool {
    let n = batch.n_tiers[sample];
    if k >= n || !batch.free_tiers[sample].get(k).copied().unwrap_or(false) {
        return false;
    }
    let offset = GLOBAL_DIM + k * TIER_DIM + MOBILITY_INDEX;
    batch.states[sample]
        .get(offset)
        .is_some_and(|&mobile| mobile > 0.5)
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
fn count_labelled_tiers(batch: &ImitationBatch<'_>, indices: &[usize]) -> usize {
    indices
        .iter()
        .map(|&i| {
            let n = batch.n_tiers[i];
            (0..n)
                .filter(|&k| {
                    component_labels(&batch.sensitivities[i], k, is_actionable_tier(batch, i, k))
                        .iter()
                        .any(Option::is_some)
                })
                .count()
        })
        .sum()
}

/// Count finite labels by component, on real tiers the environment can move.
fn count_component_support(batch: &ImitationBatch<'_>, indices: &[usize]) -> [usize; N_COMPONENTS] {
    let mut support = [0; N_COMPONENTS];
    for &i in indices {
        let n = batch.n_tiers[i];
        for k in 0..n {
            if is_actionable_tier(batch, i, k) {
                for (component, label) in batch.sensitivities[i][k].iter().enumerate() {
                    support[component] += usize::from(label.is_finite());
                }
            }
        }
    }
    support
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
    sample_has_labels: Vec<bool>,
}

/// Build the label rows of the samples `idx`.
///
/// - Component labels: real AND free AND finite entries.
/// - Direction: `direction_from_slopes` of the finite actionable score slopes (NaN = no slope),
///   kept where the tier is mobile, real, free and has a finite slope. Zero directions have
///   no cosine label.
/// - Gain: the sample's oracle gain when finite.
fn label_rows(batch: &ImitationBatch<'_>, idx: &[usize], max_tiers: usize) -> LabelRows {
    let mut rows = LabelRows::default();
    for &i in idx {
        let sample_start = rows.sens_mask.len();
        let direction_start = rows.dir_mask.len();
        let n = batch.n_tiers[i];
        let slopes: Vec<Option<f32>> = batch.score_slopes[i]
            .iter()
            .enumerate()
            .map(|(k, &s)| (is_actionable_tier(batch, i, k) && s.is_finite()).then_some(s))
            .collect();
        let direction = direction_from_slopes(&slopes);
        let usable_direction = (0..n).any(|k| {
            is_actionable_tier(batch, i, k) && direction.get(k).is_some_and(|value| *value != 0.0)
        });
        for k in 0..max_tiers {
            let usable = is_actionable_tier(batch, i, k);
            for label in component_labels(&batch.sensitivities[i], k, usable) {
                rows.sens_target.push(label.unwrap_or(0.0));
                rows.sens_mask.push(flag(label.is_some()));
            }
            let has_slope =
                usable_direction && usable && slopes.get(k).copied().flatten().is_some();
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
        let sample_end = rows.sens_mask.len();
        let has_component = rows.sens_mask[sample_start..sample_end]
            .iter()
            .any(|&m| m > 0.0);
        let has_direction = rows.dir_mask[direction_start..].iter().any(|&m| m > 0.0);
        rows.sample_has_labels
            .push(has_component || has_direction || gain.is_some());
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
    /// Per-row label support, used to skip optimizer steps without any target.
    row_has_labels: Vec<bool>,
}

impl PackedImitation {
    /// Gather a deterministic mini-batch from the prepared round tensors on the training device.
    fn gather_rows(&self, rows: &[usize], device: &Device) -> Self {
        let index_values = rows.iter().map(|&row| row as i64).collect::<Vec<_>>();
        let index_len = index_values.len();
        let indices =
            Tensor::<1, Int>::from_data(TensorData::new(index_values, [index_len]), device);
        let row_has_labels = rows.iter().map(|&row| self.row_has_labels[row]).collect();
        Self {
            tiers: self.tiers.clone().select(0, indices.clone()),
            globals: self.globals.clone().select(0, indices.clone()),
            mask: self.mask.clone().select(0, indices.clone()),
            labels: LabelTensors {
                sens_target: self.labels.sens_target.clone().select(0, indices.clone()),
                sens_mask: self.labels.sens_mask.clone().select(0, indices.clone()),
                dir_target: self.labels.dir_target.clone().select(0, indices.clone()),
                dir_mask: self.labels.dir_mask.clone().select(0, indices.clone()),
                gain_target: self.labels.gain_target.clone().select(0, indices.clone()),
                gain_mask: self.labels.gain_mask.clone().select(0, indices),
            },
            row_has_labels,
        }
    }
}

#[path = "imitation_loss.rs"]
mod loss;
use loss::*;

impl BurnTrainer {
    /// Copy the samples `idx` of `batch` into device tensors (padding tiers with zeros), labels
    /// included. The batch must have passed [`ImitationBatch::validate`].
    fn pack_imitation(&self, batch: &ImitationBatch<'_>, idx: &[usize]) -> PackedImitation {
        let (dt, dg) = (self.model.d_tier, self.model.d_global);
        let mt = idx.iter().map(|&i| batch.n_tiers[i]).max().unwrap_or(1);
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
            row_has_labels: rows.sample_has_labels,
        }
    }

    /// One optimizer step over the samples `idx` of an already validated batch; `None` when the
    /// mini-batch carries no label at all (no forward pass is run and the model is untouched).
    ///
    /// The gradient is clipped to the global `max_grad_norm` before the Adam step; a non-finite
    /// loss or gradient norm skips the step with [`TrainError::NonFinite`].
    fn imitation_update(
        &mut self,
        prepared: &PackedImitation,
        idx: &[usize],
        config: &ImitationConfig,
    ) -> Result<Option<UpdateStats>, TrainError> {
        let packed = prepared.gather_rows(idx, &self.device);
        if !packed.row_has_labels.iter().any(|&has| has) {
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
            ..UpdateStats::default()
        }))
    }

    /// Train every update in one bounded prepared window, accumulating the round metrics.
    fn train_prepared_window(
        &mut self,
        batch: &ImitationBatch<'_>,
        selected_rows: &[usize],
        local_batches: &[Vec<usize>],
        config: &ImitationConfig,
        sum: &mut UpdateStats,
        updates: &mut usize,
    ) -> Result<(), TrainError> {
        if selected_rows.is_empty() {
            return Ok(());
        }
        let packing_started = Instant::now();
        let prepared = self.pack_imitation(batch, selected_rows);
        sum.packing_ms += packing_started.elapsed().as_secs_f64() as f32 * 1000.0;
        for rows in local_batches {
            let update_started = Instant::now();
            if let Some(stats) = self.imitation_update(&prepared, rows, config)? {
                add_stats(sum, &stats);
                *updates += 1;
            }
            sum.update_ms += update_started.elapsed().as_secs_f64() as f32 * 1000.0;
        }
        Ok(())
    }

    /// Mini-batch imitation training over a labelled batch.
    ///
    /// Packs selected rows into bounded device windows, then gathers each mini-batch on-device.
    /// All requested updates and epochs are processed. Steps without labels are skipped. The same
    /// selected row may be packed again if a long schedule requires another bounded window.
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
        let row_values = self.model.d_global
            + self.max_tiers * self.model.d_tier
            + self.max_tiers * (2 * N_COMPONENTS + 3)
            + 2;
        let bytes_per_row = row_values.saturating_mul(std::mem::size_of::<f32>());
        let mb_size = config.mini_batch_size.min(total);
        // Preserve the configured batch size even when a single batch itself exceeds the nominal
        // bound; otherwise memory windows are capped by available rows under the bound.
        let max_rows = (PREPARED_ROUND_BUDGET_BYTES / bytes_per_row.max(1))
            .max(mb_size)
            .max(1);
        let mut sum = UpdateStats::default();
        let mut updates = 0usize;
        let mut selected_rows = Vec::<usize>::new();
        let mut selected_any = vec![false; total];
        let mut row_positions = HashMap::<usize, usize>::new();
        let mut local_batches = Vec::<Vec<usize>>::new();
        let mut add_batch = |this: &mut Self, rows: &[usize]| -> Result<(), TrainError> {
            for &row in rows {
                selected_any[row] = true;
            }
            let additional = rows
                .iter()
                .filter(|&&row| !row_positions.contains_key(&row))
                .count();
            if !local_batches.is_empty()
                && (selected_rows.len() + additional > max_rows
                    || local_batches.len() >= PREPARED_BATCHES_PER_WINDOW)
            {
                this.train_prepared_window(
                    batch,
                    &selected_rows,
                    &local_batches,
                    config,
                    &mut sum,
                    &mut updates,
                )?;
                selected_rows.clear();
                row_positions.clear();
                local_batches.clear();
            }
            let mut local = Vec::with_capacity(rows.len());
            for &row in rows {
                let position = *row_positions.entry(row).or_insert_with(|| {
                    let position = selected_rows.len();
                    selected_rows.push(row);
                    position
                });
                local.push(position);
            }
            local_batches.push(local);
            Ok(())
        };
        let mut order: Vec<usize> = (0..total).collect();
        if config.updates_per_round == 0 {
            for _ in 0..config.epochs_per_round {
                self.shuffle.shuffle(&mut order);
                for chunk in order.chunks(mb_size) {
                    add_batch(self, chunk)?;
                }
            }
        } else {
            for _ in 0..config.updates_per_round {
                draw_minibatch(&mut self.shuffle, &mut order, mb_size);
                add_batch(self, &order[..mb_size])?;
            }
        }
        drop(add_batch);
        self.train_prepared_window(
            batch,
            &selected_rows,
            &local_batches,
            config,
            &mut sum,
            &mut updates,
        )?;

        let selected_indices: Vec<usize> = selected_any
            .iter()
            .enumerate()
            .filter_map(|(index, &selected)| selected.then_some(index))
            .collect();
        let denom = updates.max(1) as f32;
        Ok(ImitationMetrics {
            loss: sum.loss / denom,
            sens_mse: sum.sens_mse / denom,
            cosine: sum.cosine / denom,
            gain_loss: sum.gain_loss / denom,
            grad_norm: sum.grad_norm / denom,
            labelled_tiers: count_labelled_tiers(batch, &selected_indices),
            component_support: count_component_support(batch, &selected_indices),
            updates,
            packing_ms: sum.packing_ms,
            update_ms: sum.update_ms,
        })
    }
}

#[cfg(test)]
#[path = "imitation_rework_tests.rs"]
mod rework_tests;
#[cfg(test)]
#[path = "imitation_tests.rs"]
mod tests;
