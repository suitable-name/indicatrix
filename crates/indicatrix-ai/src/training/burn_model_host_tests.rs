//! Tests of the batched rollout forward: agreement with the pure-Rust twin on full and padded
//! samples, independence of the rows, the shape errors, the gradient-free view of a tracked
//! model, the rollout copy of a trainer and the state packing.

use super::*;
use crate::{
    env::{
        GLOBAL_DIM, GemstoneObservation, GlobalFeature, MEAN_BOUND, TIER_DIM, TierFeature,
        WEIGHT_FRACTION_INDEX,
    },
    model::GemstoneSetAttentionNetwork,
    training::{BurnTrainer, ModelShape, PpoConfig},
};

/// Padded tier count of the test batches.
const MAX_TIERS: usize = 5;
/// Agreement bound with the pure-Rust twin, per entry (the bound the twin itself is held to).
const TWIN_TOLERANCE: f32 = 1e-4;

fn val(i: usize) -> f32 {
    (i as f32).mul_add(0.37, 0.11).sin() * 0.8
}

/// Tier `index` of the synthetic observation `seed`: the blocks cycle through crown, pavilion and
/// girdle so the three log-std slots are exercised.
fn tier(seed: usize, index: usize) -> TierFeature {
    let v = |k: usize| val(seed * 101 + index * 13 + k);
    TierFeature {
        angle_normalized: v(0),
        angle_deg: v(0) * 90.0,
        base_angle_deg: v(1) * 90.0,
        orbit_count: 8,
        block_type: [1.0, -1.0, 0.0][(seed + index) % 3],
        is_pinned: 0.0,
        is_flat: 0.0,
        critical_margin_deg: v(2).abs() * 5.0,
        mast: v(3).mul_add(0.4, 0.5),
        min_angle_deg: -80.0,
        max_angle_deg: 80.0,
        index_frac: index as f32 / 8.0,
        azimuth_sin: v(4),
        azimuth_cos: v(5),
    }
}

/// A synthetic observation of `tiers` tiers; `seed` makes the samples differ, the objective's
/// weight fractions included.
fn observation(seed: usize, tiers: usize) -> GemstoneObservation {
    let g = |k: usize| val(seed * 977 + 1000 + k);
    let weight_fractions = [
        [0.4, 0.3, 0.2, 0.1],
        [0.25, 0.25, 0.5, 0.0],
        [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 0.0],
    ][seed % 3];
    GemstoneObservation {
        tiers: (0..tiers).map(|index| tier(seed, index)).collect(),
        global: GlobalFeature {
            refractive_index: g(0).mul_add(0.1, 1.54),
            target_refractive_index: g(1).mul_add(0.1, 1.77),
            critical_angle_deg: g(2).mul_add(5.0, 40.0),
            target_critical_angle_deg: g(3).mul_add(5.0, 34.0),
            dispersion: g(4).abs() * 0.02,
            windowing_pct: g(5).abs() * 60.0,
            extinction_pct: g(6).abs() * 40.0,
            tilt_brilliance_pct: g(7).abs() * 100.0,
            yield_pct: g(8).abs() * 50.0,
            table_pct: g(9).abs() * 70.0,
            crown_to_pavilion_ratio: g(10).abs(),
            min_girdle_ratio: g(11).abs(),
            best_score_so_far: g(12).abs() * 50.0,
            progress_ratio: g(13).abs(),
            base_score: g(14).abs() * 60.0,
            weight_fractions,
        },
    }
}

/// A Xavier-initialised pure network with distinct log-stds and a non-trivial final norm, so every
/// output path of the twins is exercised.
fn pure_network(seed: u64) -> GemstoneSetAttentionNetwork {
    let mut net = GemstoneSetAttentionNetwork::new(TIER_DIM, GLOBAL_DIM, 16, 4, 2, seed).unwrap();
    net.log_std_raw = [0.8, -0.4, 1.7];
    net.final_norm.gamma = (0..16).map(|k| val(k).mul_add(0.05, 1.0)).collect();
    net.final_norm.beta = (0..16).map(|k| 0.1 * val(40 + k)).collect();
    net
}

fn model_of(net: &GemstoneSetAttentionNetwork) -> BurnSetAttention {
    BurnSetAttention::from_set_attention(net, &Device::flex()).unwrap()
}

/// Three observations of 5, 3 and 4 tiers (two of them padded at `MAX_TIERS`), their flat states
/// and their tier counts.
fn batch() -> (Vec<GemstoneObservation>, Vec<Vec<f32>>, Vec<usize>) {
    let observations = vec![observation(0, 5), observation(1, 3), observation(2, 4)];
    let states = observations
        .iter()
        .map(|observation| observation.flatten(MAX_TIERS))
        .collect();
    let n_tiers = observations.iter().map(|o| o.tiers.len()).collect();
    (observations, states, n_tiers)
}

fn assert_close(got: &[f32], want: &[f32], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}");
    for (k, (g, w)) in got.iter().zip(want).enumerate() {
        assert!((g - w).abs() < TWIN_TOLERANCE, "{what}[{k}]: {g} vs {w}");
    }
}

#[test]
fn forward_host_matches_the_pure_twin_on_full_and_padded_samples() {
    let net = pure_network(11);
    let model = model_of(&net);
    let (observations, states, n_tiers) = batch();
    // The objective's weight fractions travel in the flat state, where the composition reads them.
    for (state, observation) in states.iter().zip(&observations) {
        assert_eq!(
            state[WEIGHT_FRACTION_INDEX..WEIGHT_FRACTION_INDEX + 4],
            observation.global.weight_fractions
        );
    }

    let out = model.forward_host(&states, &n_tiers).unwrap();
    assert_eq!(out.means.len(), 3);
    assert_eq!(out.log_stds.len(), 3);
    assert_eq!(out.values.len(), 3);
    for (i, observation) in observations.iter().enumerate() {
        let (policy, value) = net
            .forward(&observation.tier_tokens(), &observation.global_token())
            .unwrap();
        // Trimmed to the sample's real tiers: the padded ones are dropped, not zeroed.
        assert_eq!(out.means[i].len(), n_tiers[i], "sample {i}");
        assert_eq!(out.log_stds[i].len(), n_tiers[i], "sample {i}");
        assert_close(&out.means[i], &policy.mean, &format!("mean of sample {i}"));
        assert_close(
            &out.log_stds[i],
            &policy.log_std,
            &format!("log-std of sample {i}"),
        );
        assert!(
            (out.values[i] - value).abs() < TWIN_TOLERANCE,
            "value of sample {i}: {} vs {value}",
            out.values[i]
        );
        // The composition normalises by the steepest real tier: it sits at the bound.
        let peak = out.means[i]
            .iter()
            .fold(0.0f32, |peak, m| peak.max(m.abs()));
        assert!((peak - MEAN_BOUND).abs() < 1e-3, "sample {i}: {peak}");
    }
    // Three different samples, three different answers.
    assert_ne!(out.values[0], out.values[1]);
    assert_ne!(out.values[1], out.values[2]);
}

#[test]
fn a_sample_does_not_depend_on_its_batch_its_position_or_its_padding() {
    let model = model_of(&pure_network(5));
    let (observations, states, n_tiers) = batch();
    let together = model.forward_host(&states, &n_tiers).unwrap();

    // Deterministic: the same call gives the same bits.
    assert_eq!(model.forward_host(&states, &n_tiers).unwrap(), together);

    for (i, observation) in observations.iter().enumerate() {
        let n = &n_tiers[i..=i];
        // Alone, at the same width; at exactly its own width; padded far wider.
        let alone = model.forward_host(&states[i..=i], n).unwrap();
        let tight = model.forward_host(&[observation.flatten(n[0])], n).unwrap();
        let wide = model
            .forward_host(&[observation.flatten(MAX_TIERS + 4)], n)
            .unwrap();
        for (name, other) in [("alone", alone), ("tight", tight), ("wide", wide)] {
            assert_close(
                &other.means[0],
                &together.means[i],
                &format!("{name} mean {i}"),
            );
            assert_close(
                &other.log_stds[0],
                &together.log_stds[i],
                &format!("{name} log-std {i}"),
            );
            assert_close(
                &other.values,
                &together.values[i..=i],
                &format!("{name} value {i}"),
            );
        }
    }

    // Padding holds junk in a real run (the model masks it): it changes nothing.
    let mut junk = states[1].clone();
    for value in &mut junk[GLOBAL_DIM + n_tiers[1] * TIER_DIM..] {
        *value = 5.0;
    }
    let junked = model.forward_host(&[junk], &n_tiers[1..=1]).unwrap();
    assert_close(
        &junked.means[0],
        &together.means[1],
        "mean with junk padding",
    );
    assert_close(
        &junked.values,
        &together.values[1..=1],
        "value with junk padding",
    );

    // The rows follow the samples: reversing the batch reverses the rows.
    let reversed_states: Vec<Vec<f32>> = states.iter().rev().cloned().collect();
    let reversed_counts: Vec<usize> = n_tiers.iter().rev().copied().collect();
    let reversed = model
        .forward_host(&reversed_states, &reversed_counts)
        .unwrap();
    for i in 0..3 {
        let j = 2 - i;
        assert_close(&reversed.means[j], &together.means[i], "reversed mean");
        assert_close(
            &reversed.log_stds[j],
            &together.log_stds[i],
            "reversed log-std",
        );
        assert!((reversed.values[j] - together.values[i]).abs() < TWIN_TOLERANCE);
    }
}

#[test]
fn malformed_batches_are_errors_not_panics() {
    let model = model_of(&pure_network(2));
    let good = observation(0, 3).flatten(MAX_TIERS);
    assert_eq!(good.len(), GLOBAL_DIM + MAX_TIERS * TIER_DIM);
    let run = |states: &[Vec<f32>], n_tiers: &[usize]| model.forward_host(states, n_tiers);

    // An empty batch is an empty answer.
    assert_eq!(run(&[], &[]).unwrap(), HostPolicyBatch::default());
    // A tier count equal to the padded width is fine, so is a single sample.
    let full = run(&[good.clone()], &[MAX_TIERS]).unwrap();
    assert_eq!(full.means[0].len(), MAX_TIERS);

    // The two lists disagree.
    assert!(matches!(
        run(&[good.clone()], &[3, 3]),
        Err(TrainError::ShapeMismatch(_))
    ));
    assert!(matches!(
        run(&[good.clone(), good.clone()], &[3]),
        Err(TrainError::ShapeMismatch(_))
    ));
    assert!(matches!(run(&[], &[3]), Err(TrainError::ShapeMismatch(_))));

    // A tier count outside 1..=max_tiers.
    for n in [0, MAX_TIERS + 1, usize::MAX] {
        assert!(
            matches!(
                run(&[good.clone()], &[n]),
                Err(TrainError::ShapeMismatch(_))
            ),
            "n = {n}"
        );
    }
    let second_is_bad = run(&[good.clone(), good.clone()], &[3, 0]);
    assert!(
        matches!(&second_is_bad, Err(TrainError::ShapeMismatch(m)) if m.contains("sample 1")),
        "{second_is_bad:?}"
    );

    // A state that is not a global token plus a whole number (at least one) of tier tokens.
    let mut one_too_long = good.clone();
    one_too_long.push(0.0);
    let cases = [
        one_too_long,
        good[..good.len() - 1].to_vec(),
        good[..GLOBAL_DIM].to_vec(),
        good[..GLOBAL_DIM - 1].to_vec(),
        Vec::new(),
    ];
    for state in cases {
        let len = state.len();
        assert!(
            matches!(run(&[state], &[1]), Err(TrainError::ShapeMismatch(_))),
            "{len} values"
        );
    }
    // States of different widths cannot share a tensor.
    let narrow = observation(1, 2).flatten(MAX_TIERS - 1);
    assert!(matches!(
        run(&[good.clone(), narrow], &[3, 2]),
        Err(TrainError::ShapeMismatch(m)) if m.contains("state 1")
    ));

    // A NaN or an infinity anywhere in a state (global token, a real tier, the padding) would
    // make the outputs meaningless.
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for position in [0, GLOBAL_DIM + 1, good.len() - 1] {
            let mut state = good.clone();
            state[position] = bad;
            assert!(
                matches!(run(&[state], &[3]), Err(TrainError::NonFinite(_))),
                "{bad} at {position}"
            );
        }
    }
}

#[test]
fn a_tracked_model_gives_the_numbers_of_its_gradient_free_view() {
    let net = pure_network(7);
    let plain = model_of(&net);
    let tracked = BurnSetAttention::from_set_attention(&net, &Device::flex().autodiff()).unwrap();
    let weight = |model: &BurnSetAttention| model.tier_proj.weight.val();
    // The trainer's parameters are autodiff tensors; the inference view is not.
    assert!(weight(&tracked).is_autodiff());
    assert!(!weight(&plain).is_autodiff());
    assert!(!weight(&tracked.valid()).is_autodiff());

    let (_, states, n_tiers) = batch();
    let from_tracked = tracked.forward_host(&states, &n_tiers).unwrap();
    assert_eq!(from_tracked, plain.forward_host(&states, &n_tiers).unwrap());
    assert_eq!(
        from_tracked,
        tracked.valid().forward_host(&states, &n_tiers).unwrap()
    );
}

#[test]
fn the_rollout_model_gives_the_numbers_of_the_trainers_model() {
    let shape = ModelShape {
        d_tier: TIER_DIM,
        d_global: GLOBAL_DIM,
        d_model: 16,
        num_heads: 4,
        num_layers: 2,
        max_tiers: MAX_TIERS,
    };
    let mut trainer = BurnTrainer::new_cpu(shape, PpoConfig::default()).unwrap();
    let net = pure_network(3);
    trainer.load_weights(&net).unwrap();
    let (observations, states, n_tiers) = batch();

    let rollout = trainer.rollout_model();
    // The copy is the same network without the autodiff association.
    assert!(trainer.model.tier_proj.weight.val().is_autodiff());
    assert!(!rollout.tier_proj.weight.val().is_autodiff());
    assert_eq!(
        (rollout.d_tier, rollout.d_global, rollout.d_model),
        (TIER_DIM, GLOBAL_DIM, 16)
    );
    assert_eq!((rollout.num_heads, rollout.num_layers), (4, 2));

    let from_rollout = rollout.forward_host(&states, &n_tiers).unwrap();
    assert_eq!(
        from_rollout,
        trainer.model.forward_host(&states, &n_tiers).unwrap()
    );
    // And both are the weights that were loaded: the pure twin of the same network agrees.
    for (i, observation) in observations.iter().enumerate() {
        let (policy, value) = net
            .forward(&observation.tier_tokens(), &observation.global_token())
            .unwrap();
        assert_close(&from_rollout.means[i], &policy.mean, "rollout mean");
        assert!((from_rollout.values[i] - value).abs() < TWIN_TOLERANCE);
    }
}

#[test]
fn pack_states_lays_out_globals_tiers_and_the_real_tier_mask() {
    let device = Device::flex();
    let dims = PackDims {
        d_global: GLOBAL_DIM,
        d_tier: TIER_DIM,
        max_tiers: MAX_TIERS,
    };
    let (_, states, _) = batch();
    // Packed in another order than the data lies, as a shuffled mini-batch is, with a padded
    // sample (3 of 5 tiers) and a full one.
    let rows = [(states[1].as_slice(), 3), (states[0].as_slice(), 5)];
    let packed = pack_states(rows.into_iter(), dims, &device);
    assert_eq!(packed.tiers.dims(), [2, MAX_TIERS, TIER_DIM]);
    assert_eq!(packed.globals.dims(), [2, GLOBAL_DIM]);
    assert_eq!(packed.mask.dims(), [2, MAX_TIERS]);

    let globals = host_vec(packed.globals).unwrap();
    assert_eq!(globals[..GLOBAL_DIM], states[1][..GLOBAL_DIM]);
    assert_eq!(globals[GLOBAL_DIM..], states[0][..GLOBAL_DIM]);
    let tiers = host_vec(packed.tiers).unwrap();
    let width = MAX_TIERS * TIER_DIM;
    assert_eq!(tiers[..width], states[1][GLOBAL_DIM..]);
    assert_eq!(tiers[width..], states[0][GLOBAL_DIM..]);
    assert_eq!(
        host_vec(packed.mask).unwrap(),
        [1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0]
    );
}
