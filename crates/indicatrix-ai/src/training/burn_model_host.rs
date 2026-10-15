//! The batched rollout forward of the Burn model, and the state packing the trainers share.
//!
//! A trainer rolls out many environments in lock step, and the network is a few hundred
//! microseconds per sample on a CPU thread but one launch for the whole pass on a GPU.
//! [`BurnSetAttention::forward_host`] takes the flat observations of a whole pass (one per
//! environment, padded to the same tier count), runs ONE forward over the batch on the model's
//! device and hands back the three things sampling needs, the policy means, the log-stds and
//! the values, as plain host vectors ([`HostPolicyBatch`]) trimmed to each sample's real tiers.
//! The sampling itself stays on the host, in sample order, so the random stream does not
//! depend on the device.
//!
//! # No gradient tracking
//!
//! Burn 0.22 has no switch on a forward call that turns tracking off; a computation records a
//! graph when one of its operands is tracked, and a trainer's parameters are autodiff leaves.
//! The forward therefore runs on `Module::valid()` of the model: the same weights re-wrapped
//! without their autodiff association (the data is shared, nothing is copied), with the input
//! tensors created on the device without autodiff as well. `Module::no_grad()` would only
//! clear the gradient requirement and keep the autodiff dispatch, and `Tensor::detach` works
//! per tensor; `valid()` is the supported inference transition and also what
//! `BurnTrainer::rollout_model` hands out. [`BurnSetAttention::forward_host`] applies it
//! itself, so it is gradient-free whichever model it is called on.
//!
//! # Determinism
//!
//! The numbers of this pass are the device's. On a GPU the f32 kernels (the accumulation order
//! of the matrix products, `exp`, `tanh`) differ from the pure-Rust twin in the last bits; the
//! twin is held to about `1e-4` per entry end to end, and the tests here hold the rollout pass
//! to the same bound. Rollout sampling consumes these numbers, so a rollout is reproducible on
//! one device and one thread layout, not across devices. Validation, checkpoints and inference
//! run the pure-Rust twin (`GemstoneSetAttentionNetwork`), whose results do not depend on the
//! machine.
//!
//! # Packing
//!
//! `pack_states` cuts flat states (`[global | padded tier tokens]`, what
//! `GemstoneObservation::flatten(max_tiers)` produces) into the three model inputs. The PPO
//! trainer and the rollout forward pack through it, so a state is laid out the same way
//! wherever the model reads it. (The imitation trainer's `pack_imitation` repeats the same three
//! copies next to its labels and could call it.)

use burn::{prelude::*, tensor::TensorData};

use super::{
    TrainError,
    burn_model::{BurnSetAttention, host_vec},
};

/// The policy-relevant outputs of one batched rollout forward pass, as plain host vectors.
///
/// Row `i` of every field belongs to sample `i` of the batch handed to
/// [`BurnSetAttention::forward_host`], trimmed to that sample's `n_tiers[i]` real tiers
/// (padding positions are dropped, not zeroed).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HostPolicyBatch {
    /// Composed policy means in pre-squash units, within `[-MEAN_BOUND, MEAN_BOUND]`; sample `i`
    /// has `n_tiers[i]` entries. Tiers the environment does not move are not distinguished
    /// here: the caller masks them (`GemstoneObservation::free_tier_mask`).
    pub means: Vec<Vec<f32>>,
    /// Policy log standard deviations (state-independent, selected per tier by its block), of
    /// the same shape as `means`.
    pub log_stds: Vec<Vec<f32>>,
    /// The critic's baseline `V(s)` of each sample.
    pub values: Vec<f32>,
}

/// The widths a batch of flat states is cut by (see [`pack_states`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PackDims {
    /// Width of the global token.
    pub(crate) d_global: usize,
    /// Width of one tier token.
    pub(crate) d_tier: usize,
    /// Padded tier count: how many tier tokens follow the global token in a state.
    pub(crate) max_tiers: usize,
}

/// The three model inputs of a batch of states, on one device.
pub(crate) struct StateTensors {
    /// Tier tokens, `[batch, max_tiers, d_tier]`.
    pub(crate) tiers: Tensor<3>,
    /// Global tokens, `[batch, d_global]`.
    pub(crate) globals: Tensor<2>,
    /// Real-tier mask, `[batch, max_tiers]`: `1.0` for the first `n` tiers of a sample, `0.0`
    /// for its padding.
    pub(crate) mask: Tensor<2>,
}

/// Copies flat states into the model's three inputs on `device`.
///
/// `rows` yields, per sample, its flat state (`[global | max_tiers tier tokens]`) and its real
/// tier count; the output keeps the iteration order. The padding of a state is copied as it is
/// (the model masks it, so its values do not matter). Nothing is validated here: the callers
/// check the shapes first (`TrainBatch`'s validation, the checks of
/// [`BurnSetAttention::forward_host`]).
///
/// # Panics
///
/// When a state is shorter than `d_global + max_tiers * d_tier` values.
pub(crate) fn pack_states<'a, I>(rows: I, dims: PackDims, device: &Device) -> StateTensors
where
    I: ExactSizeIterator<Item = (&'a [f32], usize)>,
{
    let PackDims {
        d_global: dg,
        d_tier: dt,
        max_tiers: mt,
    } = dims;
    let b = rows.len();
    let mut globals = Vec::with_capacity(b * dg);
    let mut tiers = Vec::with_capacity(b * mt * dt);
    let mut mask = Vec::with_capacity(b * mt);
    for (state, n) in rows {
        globals.extend_from_slice(&state[..dg]);
        tiers.extend_from_slice(&state[dg..dg + mt * dt]);
        mask.extend((0..mt).map(|k| if k < n { 1.0f32 } else { 0.0 }));
    }
    StateTensors {
        tiers: Tensor::<3>::from_data(TensorData::new(tiers, [b, mt, dt]), device),
        globals: Tensor::<2>::from_data(TensorData::new(globals, [b, dg]), device),
        mask: Tensor::<2>::from_data(TensorData::new(mask, [b, mt]), device),
    }
}

/// A read-back `[batch, max_tiers]` tensor as one row per sample, each cut to the sample's real
/// tier count (`n_tiers[i] <= max_tiers`, checked by the caller).
fn trimmed_rows(
    tensor: Tensor<2>,
    n_tiers: &[usize],
    max_tiers: usize,
) -> Result<Vec<Vec<f32>>, TrainError> {
    let flat = host_vec(tensor)?;
    if flat.len() != n_tiers.len() * max_tiers {
        return Err(TrainError::Tensor(format!(
            "read back {} values for {} samples of {max_tiers} tiers",
            flat.len(),
            n_tiers.len()
        )));
    }
    Ok(flat
        .chunks_exact(max_tiers)
        .zip(n_tiers)
        .map(|(row, &n)| row[..n].to_vec())
        .collect())
}

impl BurnSetAttention {
    /// One forward pass over a whole batch of flat observations, read back to the host.
    ///
    /// `states[i]` is the flat observation of sample `i` (`[global token | padded tier tokens]`,
    /// what `GemstoneObservation::flatten(max_tiers)` produces) and `n_tiers[i]` its real tier
    /// count. Every state has the same length `d_global + max_tiers * d_tier` with
    /// `max_tiers >= 1`; the padded tier count is derived from it. The batch is packed on the
    /// model's device like a training mini-batch, run through [`Self::forward`] on a
    /// gradient-free view of the model (see the module docs), and the means, log-stds and values
    /// are read back with one host transfer each and cut to the samples' real tiers. An empty
    /// batch gives an empty result and does not touch the device.
    ///
    /// The samples are independent rows: a sample's numbers do not depend on the other samples
    /// or on how far the batch is padded, up to the last bits of the device's kernels. The
    /// numbers are the device's, not the pure-Rust twin's (about `1e-4` apart per entry).
    ///
    /// # Errors
    ///
    /// [`TrainError::ShapeMismatch`] when `states` and `n_tiers` differ in length, the first
    /// state is not `d_global + k * d_tier` long for a `k >= 1`, a later state has another
    /// length, or an `n_tiers[i]` is 0 or exceeds the padded tier count;
    /// [`TrainError::NonFinite`] when a state holds a NaN or an infinity (the outputs would be
    /// meaningless); [`TrainError::Tensor`] when an output cannot be read back.
    pub fn forward_host(
        &self,
        states: &[Vec<f32>],
        n_tiers: &[usize],
    ) -> Result<HostPolicyBatch, TrainError> {
        let Some(max_tiers) = self.check_host_batch(states, n_tiers)? else {
            return Ok(HostPolicyBatch::default());
        };
        let inference = self.valid();
        let device = inference.tier_proj.weight.val().device().without_autodiff();
        let dims = PackDims {
            d_global: self.d_global,
            d_tier: self.d_tier,
            max_tiers,
        };
        let rows = states
            .iter()
            .zip(n_tiers)
            .map(|(state, &n)| (state.as_slice(), n));
        let input = pack_states(rows, dims, &device);
        let out = inference.forward(input.tiers, input.globals, input.mask);

        let values = host_vec(out.value)?;
        if values.len() != states.len() {
            return Err(TrainError::Tensor(format!(
                "read back {} values for {} samples",
                values.len(),
                states.len()
            )));
        }
        Ok(HostPolicyBatch {
            means: trimmed_rows(out.mean, n_tiers, max_tiers)?,
            log_stds: trimmed_rows(out.log_std, n_tiers, max_tiers)?,
            values,
        })
    }

    /// Checks a rollout batch against the model and returns its padded tier count, `None` for
    /// an empty batch (see [`Self::forward_host`] for the errors).
    fn check_host_batch(
        &self,
        states: &[Vec<f32>],
        n_tiers: &[usize],
    ) -> Result<Option<usize>, TrainError> {
        if states.len() != n_tiers.len() {
            return Err(TrainError::ShapeMismatch(format!(
                "{} states but {} tier counts",
                states.len(),
                n_tiers.len()
            )));
        }
        let Some(first) = states.first() else {
            return Ok(None);
        };
        let row_len = first.len();
        let Some(max_tiers) = self.tiers_in_state(row_len) else {
            return Err(TrainError::ShapeMismatch(format!(
                "state 0 has {row_len} values, expected {} global plus a whole number (at least \
                 one) of {}-value tiers",
                self.d_global, self.d_tier
            )));
        };
        for (i, (state, &n)) in states.iter().zip(n_tiers).enumerate() {
            if state.len() != row_len {
                return Err(TrainError::ShapeMismatch(format!(
                    "state {i} has {} values, state 0 has {row_len}",
                    state.len()
                )));
            }
            if n == 0 || n > max_tiers {
                return Err(TrainError::ShapeMismatch(format!(
                    "sample {i} has {n} tiers, expected 1..={max_tiers}"
                )));
            }
            if state.iter().any(|value| !value.is_finite()) {
                return Err(TrainError::NonFinite(format!(
                    "state {i} holds a NaN or infinite value"
                )));
            }
        }
        Ok(Some(max_tiers))
    }

    /// The tier slots of a flat state of `len` values: `(len - d_global) / d_tier`, `None`
    /// unless that is a whole number of at least one.
    fn tiers_in_state(&self, len: usize) -> Option<usize> {
        let tier_values = len.checked_sub(self.d_global)?;
        (tier_values.checked_rem(self.d_tier)? == 0)
            .then(|| tier_values / self.d_tier)
            .filter(|&tiers| tiers > 0)
    }
}

#[cfg(test)]
#[path = "burn_model_host_tests.rs"]
mod tests;
