//! Tests of the Burn twin: equivalence with the pure-Rust network, the composed mean, the heads,
//! the conversions both ways and the padding invariance. The trainers' tests live with the
//! trainers.

use super::*;
use crate::{
    env::GLOBAL_DIM,
    model::{Linear, compose_mean_masked},
};
use burn::optim::GradientsParams;

const DT: usize = crate::env::TIER_DIM;
const DG: usize = GLOBAL_DIM;

/// Objective weight fractions written into the synthetic global tokens.
const FRACTIONS: [f32; N_COMPONENTS] = [0.4, 0.25, 0.15, 0.1, 0.05, 0.025, 0.025];

#[allow(
    clippy::suboptimal_flops,
    reason = "fixed test-input generator; fusing would change the inputs the tests were tuned on"
)]
fn val(i: usize) -> f32 {
    ((i as f32) * 0.37 + 0.11).sin() * 0.8
}

type Inputs = (Tensor<3>, Tensor<2>, Tensor<2>, Vec<Vec<f32>>, Vec<f32>);

fn tensors(device: &Device, n_real: usize, n_total: usize, pad_value: f32) -> Inputs {
    let mut flat = Vec::new();
    let mut real = Vec::new();
    for t in 0..n_total {
        let row: Vec<f32> = (0..DT)
            .map(|k| {
                if k == crate::env::MOBILITY_INDEX {
                    return if t < n_real { 1.0 } else { 0.0 };
                }
                if t < n_real {
                    val(t * 31 + k)
                } else {
                    pad_value
                }
            })
            .collect();
        if t < n_real {
            real.push(row.clone());
        }
        flat.extend(row);
    }
    let mut global: Vec<f32> = (0..DG).map(|k| val(1000 + k)).collect();
    global[WEIGHT_FRACTION_INDEX..WEIGHT_FRACTION_INDEX + N_COMPONENTS].copy_from_slice(&FRACTIONS);
    let mask: Vec<f32> = (0..n_total)
        .map(|t| if t < n_real { 1.0 } else { 0.0 })
        .collect();
    (
        Tensor::<3>::from_data(TensorData::new(flat, [1, n_total, DT]), device),
        Tensor::<2>::from_data(TensorData::new(global.clone(), [1, DG]), device),
        Tensor::<2>::from_data(TensorData::new(mask, [1, n_total]), device),
        real,
        global,
    )
}

/// `[1, rows.len(), DT]` tier tensor from per-tier token rows.
fn tier_tensor(device: &Device, rows: &[Vec<f32>]) -> Tensor<3> {
    let flat: Vec<f32> = rows.iter().flatten().copied().collect();
    Tensor::<3>::from_data(TensorData::new(flat, [1, rows.len(), DT]), device)
}

/// A Xavier-initialised pure network of the test dimensions, so the head outputs are O(1).
fn pure_network(seed: u64) -> GemstoneSetAttentionNetwork {
    GemstoneSetAttentionNetwork::new(DT, DG, 16, 4, 2, seed).unwrap()
}

/// The sensitivities read back to the host as one row per tier.
fn sensitivity_rows(t: Tensor<3>) -> Vec<[f32; N_COMPONENTS]> {
    let data = host_vec(t).unwrap();
    data.as_chunks::<N_COMPONENTS>().0.to_vec()
}

#[test]
fn burn_and_pure_forward_agree_after_export() {
    let device = Device::flex();
    // A non-trivial final norm and three distinct raw log-stds (crown, pavilion, other), written
    // on the pure side and rebuilt into Burn, so the final norm and the per-block log-std of both
    // twins are exercised (a fresh network has an identity norm and three equal log-stds).
    let mut pure = pure_network(11);
    pure.log_std_raw = [0.8, -0.4, 1.7];
    pure.final_norm.gamma = (0..16).map(|k| val(k).mul_add(0.05, 1.0)).collect();
    pure.final_norm.beta = (0..16).map(|k| 0.1 * val(40 + k)).collect();
    let model = BurnSetAttention::from_set_attention(&pure, &device).unwrap();

    // Blocks: crown, pavilion, other, crown, pavilion (entry 3 of the synthetic tokens).
    let (_, global, mask, mut real, global_vec) = tensors(&device, 5, 5, 0.0);
    let blocks = [1.0f32, -1.0, 0.0, 1.0, -1.0];
    for (token, block) in real.iter_mut().zip(blocks) {
        token[BLOCK_TYPE_INDEX] = block;
    }
    let out = model.forward(tier_tensor(&device, &real), global, mask);
    let full = pure
        .forward_full(&real, &global_vec)
        .expect("pure forward pass");

    let mean = host_vec(out.mean).unwrap();
    let log_std = host_vec(out.log_std).unwrap();
    let value = host_scalar(out.value).unwrap();
    let gain = host_scalar(out.gain).unwrap();
    let sens = sensitivity_rows(out.sensitivities);
    assert_eq!(full.policy.mean.len(), 5);
    assert_eq!(sens.len(), 5);
    // Hand values of -3 + 3.5 * sigmoid(raw) for raw 0.8 (crown), -0.4 (pavilion), 1.7 (other).
    let (crown, pavilion, other) = (-0.585_089_3f32, -1.595_406_8f32, -0.040_628_43f32);
    let want_log_std = [crown, pavilion, other, crown, pavilion];
    for t in 0..5 {
        // The end-to-end mean includes the drift of the two transformer implementations; the
        // composition alone is compared tightly in `the_tensor_composition_matches_the_pure_one`.
        assert!(
            (mean[t] - full.policy.mean[t]).abs() < 1e-4,
            "mean[{t}]: {} vs {}",
            mean[t],
            full.policy.mean[t]
        );
        assert!(
            mean[t].abs() <= MEAN_BOUND + 1e-5,
            "mean[{t}] = {} leaves the bound",
            mean[t]
        );
        assert!(
            (log_std[t] - full.policy.log_std[t]).abs() < 1e-4,
            "log_std[{t}]: {} vs {}",
            log_std[t],
            full.policy.log_std[t]
        );
        assert!(
            (log_std[t] - want_log_std[t]).abs() < 1e-4,
            "log_std[{t}]: {} vs hand value {}",
            log_std[t],
            want_log_std[t]
        );
        for (c, got) in sens[t].iter().enumerate() {
            assert!(
                (got - full.sensitivities[t][c]).abs() < 1e-4,
                "sensitivity[{t}][{c}]: {} vs {}",
                got,
                full.sensitivities[t][c]
            );
        }
    }
    assert!(
        (value - full.value).abs() < 1e-4,
        "value: {value} vs {}",
        full.value
    );
    assert!(
        (gain - full.gain).abs() < 1e-4,
        "gain: {gain} vs {}",
        full.gain
    );
}

#[test]
fn the_tensor_composition_matches_the_pure_one_on_identical_sensitivities() {
    let device = Device::flex();
    let model = BurnSetAttention::from_set_attention(&pure_network(5), &device).unwrap();
    let (tiers, global, mask, _, global_vec) = tensors(&device, 7, 7, 0.0);
    let out = model.forward(tiers, global, mask);
    // The very same f32 sensitivities go through the pure composition: any difference left is the
    // tensor sum against the `mul_add` fold, a last-ulp matter.
    let rows = sensitivity_rows(out.sensitivities);
    let fractions: [f32; N_COMPONENTS] = global_vec
        [WEIGHT_FRACTION_INDEX..WEIGHT_FRACTION_INDEX + N_COMPONENTS]
        .try_into()
        .unwrap();
    assert_eq!(fractions, FRACTIONS);
    let want = compose_mean_masked(&rows, &fractions, &[true; N_COMPONENTS]);
    let got = host_vec(out.mean).unwrap();
    assert_eq!(got.len(), 7);
    for (k, (g, w)) in got.iter().zip(&want).enumerate() {
        assert!((g - w).abs() < 1e-5, "tier {k}: {g} vs {w}");
    }
    let peak = got.iter().fold(0.0f32, |a, m| a.max(m.abs()));
    assert!((peak - MEAN_BOUND).abs() < 1e-3, "{peak}");
}

#[test]
fn the_composed_mean_normalises_over_the_real_tiers_only() {
    let device = Device::flex();
    // Batch of 2, three tiers, windowing/extinction weights chosen per row.
    let mut sensitivities = vec![0.0f32; 2 * 3 * N_COMPONENTS];
    sensitivities[0] = 1.0;
    sensitivities[N_COMPONENTS] = 2.0;
    sensitivities[2 * N_COMPONENTS] = 100.0;
    sensitivities[3 * N_COMPONENTS + 1] = -3.0;
    sensitivities[4 * N_COMPONENTS + 1] = 1.0;
    let sens = Tensor::<3>::from_data(
        TensorData::new(sensitivities, [2, 3, N_COMPONENTS]),
        &device,
    );
    let fractions = Tensor::<3>::from_data(
        TensorData::new(
            [
                [1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                [0.0f32, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            ]
            .into_iter()
            .flatten()
            .collect(),
            [2, 1, N_COMPONENTS],
        ),
        &device,
    );
    let mask = Tensor::<2>::from_data(
        TensorData::new(vec![1.0f32, 1.0, 0.0, 1.0, 1.0, 1.0], [2, 3]),
        &device,
    );
    let mobile = Tensor::<2>::from_data(
        TensorData::new(vec![1.0f32, 1.0, 0.0, 1.0, 1.0, 1.0], [2, 3]),
        &device,
    );
    let mean = host_vec(composed_mean(sens, fractions, mask, mobile)).unwrap();
    // Row 0: g_max over the real tiers is 2, so tier 0 sits at half the bound and tier 1 at the
    // bound; the padded tier (100) does not enter the normalisation.
    assert!((mean[0] + MEAN_BOUND / 2.0).abs() < 1e-5, "{}", mean[0]);
    assert!((mean[1] + MEAN_BOUND).abs() < 1e-5, "{}", mean[1]);
    // Row 1: g = [-3, 1, 0], g_max = 3.
    assert!((mean[3] - MEAN_BOUND).abs() < 1e-5, "{}", mean[3]);
    assert!((mean[4] + MEAN_BOUND / 3.0).abs() < 1e-5, "{}", mean[4]);
    assert_eq!(mean[5], 0.0);
}

#[test]
fn the_composed_mean_of_zero_sensitivities_is_zero_never_nan() {
    let device = Device::flex();
    let sens = Tensor::<3>::zeros([1, 4, N_COMPONENTS], &device);
    let fractions = Tensor::<3>::from_data(
        TensorData::new(vec![0.25f32; N_COMPONENTS], [1, 1, N_COMPONENTS]),
        &device,
    );
    let mask = Tensor::<2>::ones([1, 4], &device);
    let mobile = Tensor::<2>::ones([1, 4], &device);
    let mean = host_vec(composed_mean(sens, fractions, mask, mobile)).unwrap();
    assert!(mean.iter().all(|&m| m == 0.0 && !m.is_nan()), "{mean:?}");
}

#[test]
fn the_composed_mean_uses_the_shared_slope_floor_and_excludes_immobile_tiers() {
    let device = Device::flex();
    let mut sens = vec![0.0f32; 2 * N_COMPONENTS];
    sens[0] = crate::env::SLOPE_NORM_FLOOR * 0.25;
    sens[N_COMPONENTS] = 100.0;
    let sensitivities =
        Tensor::<3>::from_data(TensorData::new(sens, [1, 2, N_COMPONENTS]), &device);
    let fractions = Tensor::<3>::from_data(
        TensorData::new(
            vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [1, 1, N_COMPONENTS],
        ),
        &device,
    );
    let tiers = Tensor::<2>::from_data(TensorData::new(vec![1.0, 1.0], [1, 2]), &device);
    let mobile = Tensor::<2>::from_data(TensorData::new(vec![1.0, 0.0], [1, 2]), &device);
    let mean = host_vec(composed_mean(sensitivities, fractions, tiers, mobile)).unwrap();
    assert!((mean[0] + MEAN_BOUND * 0.25).abs() < 1e-6, "{}", mean[0]);
    assert_eq!(mean[1], 0.0);
}

#[test]
fn forward_returns_the_documented_shapes() {
    let device = Device::flex();
    let model = BurnSetAttention::new(DT, DG, 16, 4, 2, &device).unwrap();
    let (tiers, global, mask, _, _) = tensors(&device, 4, 6, 0.0);
    let out = model.forward(tiers, global, mask);
    assert_eq!(out.mean.dims(), [1, 6]);
    assert_eq!(out.log_std.dims(), [1, 6]);
    assert_eq!(out.value.dims(), [1, 1]);
    assert_eq!(out.sensitivities.dims(), [1, 6, N_COMPONENTS]);
    assert_eq!(out.gain.dims(), [1, 1]);
    assert!(host_scalar(out.gain).unwrap().is_finite());
}

#[test]
fn the_gain_head_reads_the_pooled_vector_the_critic_reads() {
    let device = Device::flex();
    let mut pure = pure_network(8);
    // A gain head that is a copy of the critic head sees the same pooled vector.
    pure.gain_head = pure.critic_head.clone();
    let model = BurnSetAttention::from_set_attention(&pure, &device).unwrap();
    let (tiers, global, mask, _, _) = tensors(&device, 5, 5, 0.0);
    let out = model.forward(tiers, global, mask);
    let (value, gain) = (
        host_scalar(out.value).unwrap(),
        host_scalar(out.gain).unwrap(),
    );
    assert!((value - gain).abs() < 1e-6, "{value} vs {gain}");
}

#[test]
fn layer_norm_epsilon_survives_the_round_trip() {
    let device = Device::flex();
    let model = BurnSetAttention::new(DT, DG, 16, 4, 1, &device).unwrap();
    let mut net = model.export_to_set_attention().unwrap();
    net.pre_attn_norms[0].eps = 1e-3;
    net.post_attn_norms[0].eps = 1e-6;
    net.final_norm.eps = 1e-4;
    let rebuilt = BurnSetAttention::from_set_attention(&net, &device).unwrap();
    assert_eq!(rebuilt.export_to_set_attention().unwrap(), net);
}

#[test]
fn log_std_raw_and_final_norm_survive_the_round_trip() {
    let device = Device::flex();
    let model = BurnSetAttention::new(DT, DG, 16, 4, 1, &device).unwrap();
    let mut net = model.export_to_set_attention().unwrap();
    // A fresh model starts every block at the init value (bounded log-std -0.7).
    assert_eq!(net.log_std_raw, [log_std_raw_init(); LOG_STD_SLOTS]);
    net.log_std_raw = [0.3, -0.2, 0.9];
    net.final_norm.gamma[3] = 1.25;
    net.final_norm.beta[5] = -0.5;
    let rebuilt = BurnSetAttention::from_set_attention(&net, &device).unwrap();
    assert_eq!(rebuilt.export_to_set_attention().unwrap(), net);
}

#[test]
fn exporting_a_log_std_of_the_wrong_length_is_an_error() {
    let device = Device::flex();
    let mut model = BurnSetAttention::new(DT, DG, 16, 4, 1, &device).unwrap();
    model.log_std_raw = Param::from_tensor(Tensor::<1>::from_data(
        TensorData::new(vec![0.0f32; 4], [4]),
        &device,
    ));
    assert!(matches!(
        model.export_to_set_attention(),
        Err(TrainError::ShapeMismatch(_))
    ));
}

#[test]
fn export_then_import_round_trips_including_the_new_heads() {
    let device = Device::flex();
    let model = BurnSetAttention::new(DT, DG, 16, 4, 2, &device).unwrap();
    let exported = model.export_to_set_attention().unwrap();
    assert_eq!(
        (
            exported.sens_head.in_features,
            exported.sens_head.out_features
        ),
        (16, N_COMPONENTS)
    );
    assert_eq!(
        (
            exported.gain_head.in_features,
            exported.gain_head.out_features
        ),
        (16, 1)
    );
    let rebuilt = BurnSetAttention::from_set_attention(&exported, &device).unwrap();
    let again = rebuilt.export_to_set_attention().unwrap();
    assert_eq!(exported, again);

    // Distinct, recognisable head weights survive both directions bit for bit.
    let mut net = pure_network(3);
    net.sens_head = Linear::init_xavier(16, N_COMPONENTS, 99);
    net.gain_head = Linear::init_xavier(16, 1, 98);
    let back = BurnSetAttention::from_set_attention(&net, &device)
        .unwrap()
        .export_to_set_attention()
        .unwrap();
    assert_eq!(back.sens_head, net.sens_head);
    assert_eq!(back.gain_head, net.gain_head);
    assert_eq!(back, net);
}

#[test]
fn corrupt_network_is_an_error_not_a_panic() {
    let device = Device::flex();
    let model = BurnSetAttention::new(DT, DG, 16, 4, 1, &device).unwrap();
    let mut net = model.export_to_set_attention().unwrap();
    net.tier_proj.weights.pop();
    assert!(matches!(
        BurnSetAttention::from_set_attention(&net, &device),
        Err(TrainError::ShapeMismatch(_))
    ));
    assert!(matches!(
        BurnSetAttention::new(DT, DG, 15, 4, 1, &device),
        Err(TrainError::InvalidConfig(_))
    ));
    // A tier token too short to carry the block feature cannot drive the per-block log-std, and
    // one too short for the mobility flag (entry 13, after the block entry 3) cannot compose
    // the mean over the mobile tiers.
    assert!(matches!(
        BurnSetAttention::new(BLOCK_TYPE_INDEX, DG, 16, 4, 1, &device),
        Err(TrainError::InvalidConfig(_))
    ));
    assert!(matches!(
        BurnSetAttention::new(MOBILITY_INDEX, DG, 16, 4, 1, &device),
        Err(TrainError::InvalidConfig(_))
    ));
    assert!(BurnSetAttention::new(MOBILITY_INDEX + 1, DG, 16, 4, 1, &device).is_ok());
    // A global token too short to carry the weight fractions cannot drive the composed mean.
    let needed = WEIGHT_FRACTION_INDEX + N_COMPONENTS;
    assert!(matches!(
        BurnSetAttention::new(DT, needed - 1, 16, 4, 1, &device),
        Err(TrainError::InvalidConfig(_))
    ));
    assert!(BurnSetAttention::new(DT, needed, 16, 4, 1, &device).is_ok());
}

#[test]
fn heads_of_the_wrong_shape_are_refused_on_import() {
    let device = Device::flex();
    let good = pure_network(4);

    let mut net = good.clone();
    net.sens_head = Linear::init_xavier(16, N_COMPONENTS - 1, 1);
    assert!(matches!(
        BurnSetAttention::from_set_attention(&net, &device),
        Err(TrainError::ShapeMismatch(m)) if m.contains("sens head")
    ));

    let mut net = good.clone();
    net.gain_head = Linear::init_xavier(16, 2, 1);
    assert!(matches!(
        BurnSetAttention::from_set_attention(&net, &device),
        Err(TrainError::ShapeMismatch(m)) if m.contains("gain head")
    ));

    let mut net = good;
    net.critic_head = Linear::init_xavier(15, 1, 1);
    assert!(matches!(
        BurnSetAttention::from_set_attention(&net, &device),
        Err(TrainError::ShapeMismatch(_))
    ));
}

#[test]
fn padded_tiers_do_not_change_the_outputs_of_the_real_tiers() {
    let device = Device::flex();
    let model = BurnSetAttention::from_set_attention(&pure_network(6), &device).unwrap();

    let (t3, g3, m3, _, _) = tensors(&device, 3, 3, 0.0);
    // Padding holds large junk values: masking, not zeros, must make them irrelevant.
    let (t9, g9, m9, _, _) = tensors(&device, 3, 9, 5.0);

    let small = model.forward(t3, g3, m3);
    let big = model.forward(t9, g9, m9);

    let read = |t: Tensor<2>| host_vec(t).unwrap();
    let (mean3, mean9) = (read(small.mean), read(big.mean));
    let (ls3, ls9) = (read(small.log_std), read(big.log_std));
    for t in 0..3 {
        assert!(
            (mean3[t] - mean9[t]).abs() < 1e-4,
            "mean[{t}]: {} vs {}",
            mean3[t],
            mean9[t]
        );
        assert!((ls3[t] - ls9[t]).abs() < 1e-4, "log_std[{t}]");
    }
    let (s3, s9) = (
        sensitivity_rows(small.sensitivities),
        sensitivity_rows(big.sensitivities),
    );
    for t in 0..3 {
        for c in 0..N_COMPONENTS {
            assert!((s3[t][c] - s9[t][c]).abs() < 1e-4, "sensitivity[{t}][{c}]");
        }
    }
    assert!((host_scalar(small.value).unwrap() - host_scalar(big.value).unwrap()).abs() < 1e-4);
    assert!((host_scalar(small.gain).unwrap() - host_scalar(big.gain).unwrap()).abs() < 1e-4);
}

#[test]
fn the_mean_and_the_gain_receive_gradient_through_their_own_heads() {
    let device = Device::flex().autodiff();
    let model = BurnSetAttention::new(DT, DG, 16, 4, 1, &device).unwrap();
    let forward = || {
        let (tiers, global, mask, _, _) = tensors(&device, 4, 4, 0.0);
        model.forward(tiers, global, mask)
    };

    // The mean alone reaches the sensitivity head and the trunk, not the gain head.
    let grads = GradientsParams::from_grads(forward().mean.sum().backward(), &model);
    assert!(grads.get::<2>(model.sens_head.weight.id).is_some());
    assert!(grads.get::<2>(model.tier_proj.weight.id).is_some());
    assert!(grads.get::<2>(model.gain_head.weight.id).is_none());

    // The gain alone reaches the gain head, not the sensitivity head.
    let grads = GradientsParams::from_grads(forward().gain.sum().backward(), &model);
    assert!(grads.get::<2>(model.gain_head.weight.id).is_some());
    assert!(grads.get::<2>(model.sens_head.weight.id).is_none());
}
