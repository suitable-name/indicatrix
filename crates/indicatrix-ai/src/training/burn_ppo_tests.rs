//! Tests of the Burn trainer: batch validation and packing, the masked log-probability, the global
//! gradient clipping and the PPO updates. The model alone is tested in `burn_model_tests.rs`.

use super::*;
use crate::env::GLOBAL_DIM;

const DT: usize = 10;
const DG: usize = GLOBAL_DIM;

#[allow(
    clippy::suboptimal_flops,
    reason = "fixed test-input generator; fusing would change the inputs the tests were tuned on"
)]
fn val(i: usize) -> f32 {
    ((i as f32) * 0.37 + 0.11).sin() * 0.8
}

fn shape(max_tiers: usize) -> ModelShape {
    ModelShape {
        d_tier: DT,
        d_global: DG,
        d_model: 16,
        num_heads: 4,
        num_layers: 2,
        max_tiers,
    }
}

struct Data {
    states: Vec<Vec<f32>>,
    actions: Vec<Vec<f32>>,
    n_tiers: Vec<usize>,
    free_tiers: Vec<Vec<bool>>,
    advantages: Vec<f32>,
    returns: Vec<f32>,
}

impl Data {
    fn batch<'a>(&'a self, old: &'a [f32]) -> TrainBatch<'a> {
        TrainBatch {
            states: &self.states,
            actions: &self.actions,
            old_log_probs: old,
            advantages: &self.advantages,
            returns: &self.returns,
            n_tiers: &self.n_tiers,
            free_tiers: &self.free_tiers,
        }
    }

    /// Append sample `i`: `nt` real tiers padded with `pad` up to `max_tiers`.
    fn push_sample(&mut self, i: usize, nt: usize, max_tiers: usize, pad: f32, advantage: f32) {
        let mut row: Vec<f32> = (0..DG).map(|k| val(i * 7 + k)).collect();
        for t in 0..max_tiers {
            for k in 0..DT {
                row.push(if t < nt {
                    val(i * 131 + t * 17 + k)
                } else {
                    pad
                });
            }
        }
        self.states.push(row);
        self.actions
            .push((0..nt).map(|t| val(i * 5 + t) * 0.9).collect());
        self.n_tiers.push(nt);
        self.free_tiers.push(vec![true; nt]);
        self.advantages.push(advantage);
        self.returns.push(0.0);
    }
}

fn empty_data() -> Data {
    Data {
        states: Vec::new(),
        actions: Vec::new(),
        n_tiers: Vec::new(),
        free_tiers: Vec::new(),
        advantages: Vec::new(),
        returns: Vec::new(),
    }
}

/// `n` samples with 2 to `max_tiers` real tiers (zero padding).
fn synthetic(n: usize, max_tiers: usize, advantage: impl Fn(usize) -> f32) -> Data {
    let mut d = empty_data();
    for i in 0..n {
        d.push_sample(i, 2 + i % (max_tiers - 1), max_tiers, 0.0, advantage(i));
    }
    d
}

/// Log-probabilities of the batch under the trainer's current model.
fn current_log_probs(tr: &BurnTrainer, d: &Data) -> Vec<f32> {
    let zeros = vec![0.0f32; d.states.len()];
    let batch = d.batch(&zeros);
    let idx: Vec<usize> = (0..d.states.len()).collect();
    let p = tr.pack(&batch, &idx);
    let out = tr.model.forward(p.tiers, p.globals, p.mask);
    let (lp, _) = masked_log_prob_and_entropy(p.actions, out.mean, out.log_std, p.action_mask);
    host_vec(lp).unwrap()
}

/// Weights of the sensitivity and gain heads of the trainer's model, read back to the host.
fn head_weights(tr: &BurnTrainer) -> (Vec<f32>, Vec<f32>) {
    let net = tr.model.export_to_set_attention().unwrap();
    (net.sens_head.weights, net.gain_head.weights)
}

#[test]
fn padded_tiers_do_not_change_log_prob_or_entropy() {
    // The same three-tier sample under max_tiers 3 (no padding) and 9 (six junk-padded tiers).
    let narrow = BurnTrainer::new_cpu(shape(3), PpoConfig::default()).unwrap();
    let mut wide = BurnTrainer::new_cpu(shape(9), PpoConfig::default()).unwrap();
    wide.load_weights(&narrow.model.export_to_set_attention().unwrap())
        .unwrap();
    let mut d3 = empty_data();
    d3.push_sample(0, 3, 3, 0.0, 1.0);
    let mut d9 = empty_data();
    // Padding holds large junk values: masking, not zeros, must make them irrelevant.
    d9.push_sample(0, 3, 9, 5.0, 1.0);

    let eval = |tr: &BurnTrainer, d: &Data| {
        let zeros = [0.0f32];
        let p = tr.pack(&d.batch(&zeros), &[0]);
        let out = tr.model.forward(p.tiers, p.globals, p.mask);
        let (lp, ent) =
            masked_log_prob_and_entropy(p.actions, out.mean, out.log_std, p.action_mask);
        (host_scalar(lp).unwrap(), host_scalar(ent).unwrap())
    };
    let (lp3, e3) = eval(&narrow, &d3);
    let (lp9, e9) = eval(&wide, &d9);
    assert!((lp3 - lp9).abs() < 1e-4, "log-prob {lp3} vs {lp9}");
    assert!((e3 - e9).abs() < 1e-4, "entropy {e3} vs {e9}");
}

#[test]
fn non_free_tiers_do_not_enter_log_prob_or_entropy() {
    let device = Device::flex();
    let t = |v: Vec<f32>| Tensor::<2>::from_data(TensorData::new(v, [1, 4]), &device);
    let mean = vec![0.1f32, -0.2, 0.3, 0.0];
    let log_std = vec![-0.5f32, 0.2, -1.0, 0.7];
    // Tiers 0 and 2 free; tier 1 real but pinned; tier 3 padding.
    let mask = t(vec![1.0, 0.0, 1.0, 0.0]);
    let (lp_a, e_a) = masked_log_prob_and_entropy(
        t(vec![0.4, 5.0, -0.3, 9.0]),
        t(mean.clone()),
        t(log_std.clone()),
        mask.clone(),
    );
    let (lp_b, e_b) =
        masked_log_prob_and_entropy(t(vec![0.4, -8.0, -0.3, 1.0]), t(mean), t(log_std), mask);
    let (lp_a, lp_b) = (host_scalar(lp_a).unwrap(), host_scalar(lp_b).unwrap());
    assert!((lp_a - lp_b).abs() < 1e-6, "pinned action changed log-prob");
    // The entropy is the mean over the two free tiers (log_std -0.5 and -1.0), not their sum.
    let want_entropy = ((-0.5 + HALF_LN_2PI_E) + (-1.0 + HALF_LN_2PI_E)) / 2.0;
    assert!((host_scalar(e_a).unwrap() - want_entropy).abs() < 1e-5);
    assert!((host_scalar(e_b).unwrap() - want_entropy).abs() < 1e-5);

    // A row with nothing free has entropy 0 (divisor clamped to 1), not NaN.
    let none = masked_log_prob_and_entropy(
        t(vec![0.4, 5.0, -0.3, 9.0]),
        t(vec![0.1, -0.2, 0.3, 0.0]),
        t(vec![-0.5, 0.2, -1.0, 0.7]),
        t(vec![0.0; 4]),
    );
    assert_eq!(host_scalar(none.1).unwrap(), 0.0);
    assert_eq!(host_scalar(none.0).unwrap(), 0.0);
}

#[test]
fn pack_builds_the_action_mask_from_real_and_free() {
    let tr = BurnTrainer::new_cpu(shape(6), PpoConfig::default()).unwrap();
    let mut d = synthetic(2, 6, |_| 1.0);
    let nt = d.n_tiers[0];
    d.free_tiers[0] = (0..nt).map(|k| k % 2 == 0).collect();
    let old = vec![0.0f32; 2];
    let p = tr.pack(&d.batch(&old), &[0]);
    let real = host_vec(p.mask).unwrap();
    let act = host_vec(p.action_mask).unwrap();
    for k in 0..6 {
        assert_eq!(real[k], if k < nt { 1.0 } else { 0.0 });
        let free = k < nt && k % 2 == 0;
        assert_eq!(act[k], if free { 1.0 } else { 0.0 }, "tier {k}");
    }
}

#[test]
fn pack_copies_states_and_ppo_columns_in_the_order_of_the_indices() {
    let max_tiers = 6;
    let tr = BurnTrainer::new_cpu(shape(max_tiers), PpoConfig::default()).unwrap();
    let mut d = synthetic(3, max_tiers, |i| i as f32 - 1.0);
    for (i, returns) in d.returns.iter_mut().enumerate() {
        *returns = 0.5 * i as f32;
    }
    let old = [-1.5f32, -2.5, -3.5];
    // Shuffled, as a mini-batch is: sample 2, then sample 0.
    let order = [2usize, 0];
    let p = tr.pack(&d.batch(&old), &order);

    let globals = host_vec(p.globals).unwrap();
    let tiers = host_vec(p.tiers).unwrap();
    let mask = host_vec(p.mask).unwrap();
    let actions = host_vec(p.actions).unwrap();
    let tier_values = max_tiers * DT;
    for (row, &sample) in order.iter().enumerate() {
        let state = &d.states[sample];
        assert_eq!(
            globals[row * DG..(row + 1) * DG],
            state[..DG],
            "globals {row}"
        );
        assert_eq!(
            tiers[row * tier_values..(row + 1) * tier_values],
            state[DG..],
            "tiers {row}"
        );
        let n = d.n_tiers[sample];
        for k in 0..max_tiers {
            assert_eq!(
                mask[row * max_tiers + k],
                if k < n { 1.0 } else { 0.0 },
                "mask {row},{k}"
            );
            // Actions are zero-padded to the tier axis.
            let want = if k < n { d.actions[sample][k] } else { 0.0 };
            assert_eq!(actions[row * max_tiers + k], want, "action {row},{k}");
        }
    }
    assert_eq!(host_vec(p.returns).unwrap(), [d.returns[2], d.returns[0]]);
    assert_eq!(
        host_vec(p.advantages).unwrap(),
        [d.advantages[2], d.advantages[0]]
    );
    assert_eq!(host_vec(p.old_log_probs).unwrap(), [old[2], old[0]]);
}

#[test]
fn the_rollout_model_is_a_snapshot_that_an_update_does_not_touch() {
    let mut tr = BurnTrainer::new_cpu(shape(6), PpoConfig::default()).unwrap();
    let d = synthetic(8, 6, |i| if i % 2 == 0 { 1.0 } else { -1.0 });
    let rollout = tr.rollout_model();
    let before = rollout.forward_host(&d.states, &d.n_tiers).unwrap();
    // The copy is the trainer's model, one row per sample trimmed to its tiers.
    assert_eq!(
        tr.model.forward_host(&d.states, &d.n_tiers).unwrap(),
        before
    );
    for (sample, mean) in before.means.iter().enumerate() {
        assert_eq!(mean.len(), d.n_tiers[sample]);
    }

    let old = current_log_probs(&tr, &d);
    tr.train_batch(&d.batch(&old)).unwrap();
    assert_eq!(tr.update_count(), 1);

    // The update moved the trainer's model and left the snapshot alone; a fresh copy is the
    // updated model again.
    let after = tr.model.forward_host(&d.states, &d.n_tiers).unwrap();
    assert_ne!(after, before, "the update changed nothing");
    assert_eq!(rollout.forward_host(&d.states, &d.n_tiers).unwrap(), before);
    assert_eq!(
        tr.rollout_model()
            .forward_host(&d.states, &d.n_tiers)
            .unwrap(),
        after
    );
}

#[test]
fn returns_with_a_huge_spread_are_rejected_as_unnormalised() {
    let mut tr = BurnTrainer::new_cpu(shape(6), PpoConfig::default()).unwrap();
    let mut d = synthetic(4, 6, |_| 1.0);
    let old = vec![0.0f32; 4];
    // Raw discounted sums of -4.6 rewards sit around -150; here +-300 -> std 300 > 100.
    d.returns = vec![300.0, -300.0, 300.0, -300.0];
    let err = tr.train_batch(&d.batch(&old)).unwrap_err();
    assert!(
        matches!(&err, TrainError::InvalidConfig(m) if m.contains("returns are not normalised")),
        "{err}"
    );
    assert_eq!(tr.update_count(), 0);
    // A spread of 50 (std 50) is accepted.
    d.returns = vec![50.0, -50.0, 50.0, -50.0];
    assert!(tr.train_batch(&d.batch(&old)).is_ok());
}

#[test]
fn returns_std_is_the_population_deviation() {
    // Mean 2, deviations -2, 0, 2 -> variance 8/3.
    let got = returns_std(&[0.0, 2.0, 4.0]);
    assert!((got - (8.0f64 / 3.0).sqrt()).abs() < 1e-12, "{got}");
    assert_eq!(returns_std(&[7.0; 5]), 0.0);
    assert_eq!(returns_std(&[]), 0.0);
}

#[test]
fn wrong_free_tier_length_is_an_error() {
    let mut tr = BurnTrainer::new_cpu(shape(6), PpoConfig::default()).unwrap();
    let mut d = synthetic(3, 6, |_| 1.0);
    d.free_tiers[1].push(true);
    let old = vec![0.0f32; 3];
    assert!(matches!(
        tr.train_batch(&d.batch(&old)),
        Err(TrainError::ShapeMismatch(_))
    ));
    d.free_tiers[1].truncate(d.n_tiers[1]);
    d.free_tiers.pop();
    assert!(matches!(
        tr.train_batch(&d.batch(&old)),
        Err(TrainError::ShapeMismatch(_))
    ));
    assert_eq!(tr.update_count(), 0);
}

#[test]
fn training_with_pinned_tiers_runs_and_updates() {
    let mut tr = BurnTrainer::new_cpu(shape(6), PpoConfig::default()).unwrap();
    let mut d = synthetic(8, 6, |_| 1.0);
    for f in &mut d.free_tiers {
        f[0] = false;
    }
    let old = current_log_probs(&tr, &d);
    let m = tr.train_batch(&d.batch(&old)).unwrap();
    assert!(m.entropy.is_finite());
    assert_eq!(tr.update_count(), 1);
}

/// Replaces the synthetic actions of `d` by draws from the trainer's current policy, as a
/// rollout would produce them (the surrogate's ascent direction is only meaningful for actions
/// the policy itself took: the composed mean is always far out for an arbitrary action).
fn draw_actions_from_the_policy(tr: &BurnTrainer, d: &mut Data, seed: u64) {
    let batch = tr.model.forward_host(&d.states, &d.n_tiers).unwrap();
    let mut rng = crate::model::Prng::new(seed);
    for (i, (mean, log_std)) in batch.means.into_iter().zip(batch.log_stds).enumerate() {
        let policy = crate::model::GaussianPolicy { mean, log_std };
        let (raw, _) = policy.sample_raw_masked(&mut rng, &d.free_tiers[i]);
        d.actions[i] = raw;
    }
}

#[test]
fn positive_advantages_raise_the_log_prob_of_taken_actions() {
    // The composed policy mean is a normalised direction (`g / g_max`): while the sensitivities
    // are small, as at initialisation, a parameter change moves a tier's mean by a multiple of
    // the policy's std (measured: one Adam step at 1e-3 shifts the means by about 0.3 pre-squash
    // units, 60 % of the initial std, and lowers the log-probabilities by nats). The property
    // that must hold is the sign of the surrogate at a learning rate the composition tolerates:
    // for actions the policy itself drew, a positive advantage raises their log-probability on
    // the update. Replaying the same batch further is not asserted: the value loss keeps moving
    // the shared trunk, and through the normalisation that moves the means too (see the module
    // docs of `burn_ppo` on why imitation is the primary algorithm).
    let config = PpoConfig {
        learning_rate: 1e-4,
        ..PpoConfig::default()
    };
    let mut tr = BurnTrainer::new_cpu(shape(6), config).unwrap();
    tr.load_weights(&GemstoneSetAttentionNetwork::new(DT, DG, 16, 4, 2, 0xC0FFEE).unwrap())
        .unwrap();
    let mut d = synthetic(16, 6, |_| 1.0);
    draw_actions_from_the_policy(&tr, &mut d, 0x5EED);
    let old = current_log_probs(&tr, &d);
    let before: f32 = old.iter().sum::<f32>() / 16.0;

    let batch = d.batch(&old);
    tr.train_batch(&batch).unwrap();
    let after: f32 = current_log_probs(&tr, &d).iter().sum::<f32>() / 16.0;
    assert!(
        after > before,
        "mean log-prob {before} -> {after} after one update"
    );
    assert_eq!(tr.update_count(), 1);
}

#[test]
fn global_clipping_scales_all_gradients_to_the_max_norm() {
    let tr = BurnTrainer::new_cpu(shape(6), PpoConfig::default()).unwrap();
    let d = synthetic(8, 6, |_| 1.0);
    let old = vec![0.0f32; 8];
    let idx: Vec<usize> = (0..8).collect();
    let p = tr.pack(&d.batch(&old), &idx);
    let out = tr.model.forward(p.tiers, p.globals, p.mask);
    // Any scalar that reaches the heads and the trunk gives a gradient on (almost) every tensor.
    let loss = out.mean.sum() + out.value.sum();
    let grads = GradientsParams::from_grads(loss.backward(), &tr.model);

    let before = global_grad_norm(&grads, &tr.model).unwrap();
    assert!(before > 1e-2, "unclipped norm {before}");

    let max_norm = 1e-3;
    let (clipped, reported) = clip_global_grad_norm(grads, &tr.model, max_norm).unwrap();
    assert!(
        (reported - before).abs() <= 1e-4 * before,
        "reported {reported} vs measured {before}"
    );
    let after = global_grad_norm(&clipped, &tr.model).unwrap();
    assert!(after <= max_norm + 1e-4, "clipped norm {after}");
    // The norm lands on the cap (not below it): every tensor was scaled by the same factor.
    assert!(after > 0.9 * max_norm, "clipped norm {after}");
}

#[test]
fn global_clipping_leaves_a_small_gradient_untouched() {
    let tr = BurnTrainer::new_cpu(shape(6), PpoConfig::default()).unwrap();
    let d = synthetic(8, 6, |_| 1.0);
    let old = vec![0.0f32; 8];
    let idx: Vec<usize> = (0..8).collect();
    let p = tr.pack(&d.batch(&old), &idx);
    let out = tr.model.forward(p.tiers, p.globals, p.mask);
    let loss = out.mean.sum() + out.value.sum();
    let grads = GradientsParams::from_grads(loss.backward(), &tr.model);

    let before = global_grad_norm(&grads, &tr.model).unwrap();
    let (kept, reported) = clip_global_grad_norm(grads, &tr.model, 1.0e6).unwrap();
    let after = global_grad_norm(&kept, &tr.model).unwrap();
    assert!(
        (reported - before).abs() <= 1e-6 * before,
        "{reported} vs {before}"
    );
    assert!(
        (after - before).abs() <= 1e-6 * before,
        "{after} vs {before}"
    );
}

#[test]
fn train_batch_reports_the_pre_clip_global_gradient_norm() {
    // A cap far below any real gradient: the reported norm is the one before clipping, so it
    // stays above the cap (a post-clip norm could not exceed it).
    let config = PpoConfig {
        max_grad_norm: 1e-6,
        ..PpoConfig::default()
    };
    let mut tr = BurnTrainer::new_cpu(shape(6), config).unwrap();
    let d = synthetic(8, 6, |_| 1.0);
    let old = current_log_probs(&tr, &d);
    let m = tr.train_batch(&d.batch(&old)).unwrap();
    assert!(m.grad_norm.is_finite());
    assert!(m.grad_norm > 1e-3, "reported norm {}", m.grad_norm);
    assert_eq!(tr.update_count(), 1);
}

#[test]
fn train_batch_runs_on_a_tiny_batch_and_reports_finite_metrics() {
    let mut tr = BurnTrainer::new_cpu(shape(6), PpoConfig::default()).unwrap();
    let d = synthetic(4, 6, |i| if i % 2 == 0 { 1.0 } else { -1.0 });
    let old = current_log_probs(&tr, &d);
    let m = tr.train_batch(&d.batch(&old)).unwrap();
    for (name, v) in [
        ("policy_loss", m.policy_loss),
        ("value_loss", m.value_loss),
        ("entropy", m.entropy),
        ("approx_kl", m.approx_kl),
        ("clip_fraction", m.clip_fraction),
        ("grad_norm", m.grad_norm),
    ] {
        assert!(v.is_finite(), "{name} = {v}");
    }
    assert!(m.grad_norm > 0.0);
    assert!((0.0..=1.0).contains(&m.clip_fraction));
    assert!(!m.early_stopped);
    assert_eq!(tr.update_count(), 1);

    // The epoch loop reports finite means as well.
    let m = tr.train_rollout_epochs(&d.batch(&old)).unwrap();
    assert!(m.policy_loss.is_finite() && m.value_loss.is_finite() && m.entropy.is_finite());
}

#[test]
fn ppo_moves_the_sensitivity_head_and_leaves_the_gain_head_alone() {
    let config = PpoConfig {
        learning_rate: 5e-3,
        ..PpoConfig::default()
    };
    let mut tr = BurnTrainer::new_cpu(shape(6), config).unwrap();
    let d = synthetic(16, 6, |i| if i % 3 == 0 { 1.0 } else { -0.5 });
    let old = current_log_probs(&tr, &d);
    let (sens_before, gain_before) = head_weights(&tr);
    for _ in 0..4 {
        tr.train_batch(&d.batch(&old)).unwrap();
    }
    let (sens_after, gain_after) = head_weights(&tr);
    // The composed mean carries the policy gradient into the sensitivity head ...
    assert_ne!(sens_before, sens_after, "the sensitivity head never moved");
    assert!(sens_after.iter().all(|w| w.is_finite()));
    // ... while no PPO term reads the gain head.
    assert_eq!(gain_before, gain_after);
}

#[test]
fn the_log_std_parameter_receives_gradient_and_moves() {
    let config = PpoConfig {
        learning_rate: 5e-3,
        ..PpoConfig::default()
    };
    let mut tr = BurnTrainer::new_cpu(shape(6), config).unwrap();
    let before = tr.model.export_to_set_attention().unwrap().log_std_raw;
    let d = synthetic(16, 6, |i| if i % 3 == 0 { 1.0 } else { -0.5 });
    let old = current_log_probs(&tr, &d);
    for _ in 0..4 {
        tr.train_batch(&d.batch(&old)).unwrap();
    }
    let after = tr.model.export_to_set_attention().unwrap().log_std_raw;
    assert!(after.iter().all(|v| v.is_finite()));
    // The synthetic tokens use all three blocks, so every slot sees the entropy and
    // log-probability gradient; at least one has moved.
    assert_ne!(before, after, "log_std_raw never changed");
}

#[test]
fn epoch_loop_stops_early_when_kl_exceeds_target() {
    let mk = |target_kl: f32| PpoConfig {
        learning_rate: 1e-2,
        target_kl,
        epochs_per_batch: 4,
        mini_batch_size: 8,
        ..PpoConfig::default()
    };
    let d = synthetic(16, 6, |i| if i.is_multiple_of(2) { 1.0 } else { -1.0 });
    // Old log-probs 0.3 above the current ones: every ratio starts at exp(-0.3) = 0.741, so the
    // very first mini-batch has KL = (r - 1) - ln r = 0.041, independent of rounding noise.
    let shifted = |tr: &BurnTrainer| -> Vec<f32> {
        current_log_probs(tr, &d)
            .iter()
            .map(|lp| lp + 0.3)
            .collect()
    };

    let mut tight = BurnTrainer::new_cpu(shape(6), mk(1e-12))
        .unwrap()
        .with_seed(3);
    let old = shifted(&tight);
    let m = tight.train_rollout_epochs(&d.batch(&old)).unwrap();
    assert!(m.early_stopped);
    // The check runs after every mini-batch: the very first update already stops the call
    // (the old per-epoch check would have run the epoch's 2 updates).
    assert_eq!(tight.update_count(), 1);
    assert!(
        (m.approx_kl - 0.0408).abs() < 2e-3,
        "first mini-batch KL {}",
        m.approx_kl
    );

    let mut loose = BurnTrainer::new_cpu(shape(6), mk(1.0e6))
        .unwrap()
        .with_seed(3);
    let old = shifted(&loose);
    assert!(
        !loose
            .train_rollout_epochs(&d.batch(&old))
            .unwrap()
            .early_stopped
    );
    // 4 epochs x (16 samples / 8 per mini-batch) = 8 updates.
    assert_eq!(loose.update_count(), 8);
}

#[test]
fn malformed_batches_are_errors() {
    let mut tr = BurnTrainer::new_cpu(shape(6), PpoConfig::default()).unwrap();
    let mut d = synthetic(4, 6, |_| 1.0);
    let old = vec![0.0f32; 4];
    d.states[1].pop();
    assert!(matches!(
        tr.train_batch(&d.batch(&old)),
        Err(TrainError::ShapeMismatch(_))
    ));

    let d = synthetic(4, 6, |_| f32::NAN);
    assert!(matches!(
        tr.train_batch(&d.batch(&old)),
        Err(TrainError::NonFinite(_))
    ));
    assert_eq!(tr.update_count(), 0);
}

#[test]
fn invalid_trainer_arguments_are_rejected() {
    assert!(BurnTrainer::new_cpu(shape(0), PpoConfig::default()).is_err());
    assert!(BurnTrainer::new_cpu(shape(MAX_TIERS_LIMIT + 1), PpoConfig::default()).is_err());
    let bad = PpoConfig {
        mini_batch_size: 0,
        ..PpoConfig::default()
    };
    assert!(BurnTrainer::new_cpu(shape(6), bad).is_err());
    // A global token too narrow to carry the weight fractions cannot build a model.
    let narrow = ModelShape {
        d_global: 15,
        ..shape(6)
    };
    assert!(matches!(
        BurnTrainer::new_cpu(narrow, PpoConfig::default()),
        Err(TrainError::InvalidConfig(_))
    ));
}

#[test]
fn load_weights_transfers_the_model_and_checks_dimensions() {
    let a = BurnTrainer::new_cpu(shape(6), PpoConfig::default()).unwrap();
    let mut b = BurnTrainer::new_cpu(shape(6), PpoConfig::default()).unwrap();
    let net = a.model.export_to_set_attention().unwrap();
    b.load_weights(&net).unwrap();
    assert_eq!(b.model.export_to_set_attention().unwrap(), net);

    let other = BurnSetAttention::new(DT, DG, 32, 4, 2, &Device::flex())
        .unwrap()
        .export_to_set_attention()
        .unwrap();
    assert!(matches!(
        b.load_weights(&other),
        Err(TrainError::ShapeMismatch(_))
    ));
}
