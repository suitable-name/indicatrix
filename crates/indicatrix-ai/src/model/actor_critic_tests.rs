//! Unit tests of the set-attention network: kinematic projection, the heads, the composed policy
//! mean, the gain head and the parameter counts.

use super::*;
use crate::env::{
    GLOBAL_DIM, MAX_SAFE_CANDIDATE_ANGLE_DEG, MEAN_BOUND, MIN_SAFE_CANDIDATE_ANGLE_DEG,
};
use crate::model::compose_mean;

/// Balanced objective: equal windowing, extinction and tilt brilliance, no yield.
const BALANCED: [f32; N_COMPONENTS] = [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 0.0, 0.0, 0.0, 0.0];

fn weights4(a: f32, b: f32, c: f32, d: f32) -> [f32; N_COMPONENTS] {
    let mut weights = [0.0; N_COMPONENTS];
    weights[..4].copy_from_slice(&[a, b, c, d]);
    weights
}

fn row4(a: f32, b: f32, c: f32, d: f32) -> [f32; N_COMPONENTS] {
    weights4(a, b, c, d)
}

fn tier(angle_deg: f32, block_type: f32) -> TierFeature {
    TierFeature {
        angle_normalized: angle_deg / 90.0,
        angle_deg,
        orbit_count: 8,
        block_type,
        is_pinned: 0.0,
        is_flat: 0.0,
        is_mobile: 1.0,
        critical_margin_deg: 0.0,
        mast: 0.5,
        min_angle_deg: if block_type < -0.5 {
            -(MAX_SAFE_CANDIDATE_ANGLE_DEG as f32)
        } else {
            MIN_SAFE_CANDIDATE_ANGLE_DEG as f32
        },
        max_angle_deg: if block_type < -0.5 {
            -(MIN_SAFE_CANDIDATE_ANGLE_DEG as f32)
        } else {
            MAX_SAFE_CANDIDATE_ANGLE_DEG as f32
        },
        base_angle_deg: 0.0,
        index_frac: 0.0,
        azimuth_sin: 0.0,
        azimuth_cos: 0.0,
        orbit_sin_2: 0.0,
        orbit_cos_2: 1.0,
        orbit_sin_4: 0.0,
        orbit_cos_4: 1.0,
        live_area_fraction: 1.0,
        live_centroid_x: 0.0,
        live_centroid_y: 0.0,
        live_centroid_z: 0.0,
        geometry_available: 1.0,
        constraint_kind: 0.0,
        relation_driven: 0.0,
        target_driven: 0.0,
    }
}

/// Final signed angle after projecting `delta` on a tier at `angle_deg`.
fn projected(angle_deg: f32, block_type: f32, delta: f64, crit: f32) -> f64 {
    let mut action = ContinuousTierAction::new(vec![delta]);
    GemstoneSetAttentionNetwork::project_actions(
        &mut action,
        &[tier(angle_deg, block_type)],
        crit,
        20.0,
    );
    f64::from(angle_deg) + action.deltas_deg[0]
}

#[test]
fn projection_preserves_sign_and_uses_the_shared_safe_angle_interval() {
    let crit = 25.0f32;
    let tol = 1e-4;
    let min = MIN_SAFE_CANDIDATE_ANGLE_DEG;
    let max = MAX_SAFE_CANDIDATE_ANGLE_DEG;
    // A 20-degree request toward either end of the interval is clipped at that end, never past
    // it and never across the girdle: -15 + 20 would be +5, 15 - 20 would be -5.
    assert!((projected(-80.0, -1.0, -20.0, crit) + max).abs() < tol);
    assert!((projected(-15.0, -1.0, 20.0, crit) + min).abs() < tol);
    assert!((projected(70.0, 1.0, 20.0, crit) - max).abs() < tol);
    assert!((projected(15.0, 1.0, -20.0, crit) - min).abs() < tol);
    // Inside the interval the request passes through.
    assert!((projected(-30.0, -1.0, 20.0, crit) + 10.0).abs() < tol);
    assert!((projected(30.0, 1.0, -20.0, crit) - 10.0).abs() < tol);
}

#[test]
fn zero_delta_does_not_retarget_an_out_of_range_angle() {
    let step = 0.75;
    let run = |angle_deg: f32, block_type: f32| {
        let mut action = ContinuousTierAction::new(vec![0.0]);
        GemstoneSetAttentionNetwork::project_actions(
            &mut action,
            &[tier(angle_deg, block_type)],
            40.0,
            step,
        );
        action.deltas_deg[0]
    };
    assert_eq!(run(-38.0, -1.0), 0.0);
    assert_eq!(run(-88.0, -1.0), 0.0);
    assert_eq!(run(10.0, 1.0), 0.0);
    assert_eq!(run(85.0, 1.0), 0.0);
    assert_eq!(run(-60.0, -1.0), 0.0);
    assert_eq!(run(50.0, 1.0), 0.0);
}

#[test]
fn projection_respects_explicit_tier_bounds_and_empty_intersections() {
    let mut bounded = tier(40.0, 1.0);
    bounded.min_angle_deg = 35.0;
    bounded.max_angle_deg = 45.0;
    let mut action = ContinuousTierAction::new(vec![20.0]);
    GemstoneSetAttentionNetwork::project_actions(&mut action, &[bounded.clone()], 90.0, 20.0);
    assert!((40.0 + action.deltas_deg[0] - 45.0).abs() < 1e-5);

    bounded.min_angle_deg = 100.0;
    bounded.max_angle_deg = 110.0;
    let mut action = ContinuousTierAction::new(vec![10.0]);
    GemstoneSetAttentionNetwork::project_actions(&mut action, &[bounded], 90.0, 20.0);
    assert_eq!(action.deltas_deg[0], 0.0);
}

/// A small network (`d_model` 16, two layers) with distinct raw log-stds, so the block each tier
/// reads is visible in the output.
fn network_with_distinct_log_stds() -> GemstoneSetAttentionNetwork {
    let mut net = small_network(5);
    net.log_std_raw = [0.8, -0.4, 1.7];
    net
}

fn small_network(seed: u64) -> GemstoneSetAttentionNetwork {
    GemstoneSetAttentionNetwork::new(TierFeature::FEATURE_DIM, GLOBAL_DIM, 16, 4, 2, seed).unwrap()
}

/// Tier token whose block entry is `block` and whose other entries are small distinct values.
fn token_with_block(block: f32, seed: f32) -> Vec<f32> {
    let mut token: Vec<f32> = (0..TierFeature::FEATURE_DIM)
        .map(|k| 0.05 * (k as f32 + seed))
        .collect();
    token[BLOCK_TYPE_INDEX] = block;
    token[crate::env::MOBILITY_INDEX] = 1.0;
    token
}

/// `n` tier tokens cycling through crown, pavilion and girdle blocks.
fn tokens(n: usize) -> Vec<Vec<f32>> {
    (0..n)
        .map(|k| {
            let block = match k % 3 {
                0 => 1.0,
                1 => -1.0,
                _ => 0.0,
            };
            token_with_block(block, k as f32)
        })
        .collect()
}

/// A global token of `GLOBAL_DIM` entries whose weight fractions (entries 15 to 18) are `fractions`.
fn global_token(fractions: [f32; N_COMPONENTS]) -> Vec<f32> {
    let mut token = vec![0.1; GLOBAL_DIM];
    token[WEIGHT_FRACTION_INDEX..WEIGHT_FRACTION_INDEX + N_COMPONENTS].copy_from_slice(&fractions);
    token
}

#[test]
fn forward_reads_the_log_std_of_the_tiers_block() {
    let net = network_with_distinct_log_stds();
    let tokens = [
        token_with_block(1.0, 0.0),
        token_with_block(-1.0, 1.0),
        token_with_block(0.0, 2.0),
        token_with_block(1.0, 3.0),
    ];
    let (policy, value) = net.forward(&tokens, &global_token(BALANCED)).unwrap();
    assert!(value.is_finite());
    // Hand values of -3 + 3.5 * sigmoid(raw) for raw 0.8, -0.4 and 1.7.
    let crown = -0.585_089_3;
    let pavilion = -1.595_406_8;
    let other = -0.040_628_43;
    let want = [crown, pavilion, other, crown];
    for (i, (got, want)) in policy.log_std.iter().zip(want).enumerate() {
        assert!((got - want).abs() < 1e-5, "tier {i}: {got} vs {want}");
    }
    // The log-std does not depend on the other features of the token, only on its block.
    let (again, _) = net
        .forward(
            &[token_with_block(1.0, 9.0), token_with_block(-1.0, 7.0)],
            &global_token(weights4(0.1, 0.2, 0.3, 0.4)),
        )
        .unwrap();
    assert_eq!(again.log_std, vec![policy.log_std[0], policy.log_std[1]]);
    // The means are the composed, bounded pre-squash directions.
    assert_eq!(policy.mean.len(), 4);
    assert!(
        policy.mean.iter().all(|m| m.abs() <= MEAN_BOUND + 1e-5),
        "{:?}",
        policy.mean
    );
}

#[test]
fn forward_agrees_with_forward_full() {
    let net = small_network(21);
    let toks = tokens(5);
    let global = global_token(weights4(0.4, 0.3, 0.2, 0.1));
    let (policy, value) = net.forward(&toks, &global).unwrap();
    let full = net.forward_full(&toks, &global).unwrap();
    assert_eq!(policy, full.policy);
    assert_eq!(value, full.value);
    assert_eq!(full.sensitivities.len(), 5);
    assert!(full.gain.is_finite());
    // The scratch variants are the same computation with caller-owned buffers.
    let mut scratch = InferenceScratch::default();
    let (policy_s, value_s) = net
        .forward_with_scratch(&toks, &global, &mut scratch)
        .unwrap();
    assert_eq!((policy_s, value_s), (full.policy.clone(), full.value));
    let full_s = net
        .forward_full_with_scratch(&toks, &global, &mut scratch)
        .unwrap();
    assert_eq!(full_s, full);
}

#[test]
fn the_policy_mean_is_composed_from_the_sensitivities_and_the_weight_fractions() {
    let net = small_network(5);
    let toks = tokens(4);
    for fractions in [
        weights4(0.4, 0.3, 0.2, 0.1),
        weights4(0.0, 0.0, 0.0, 1.0),
        BALANCED,
    ] {
        let out = net.forward_full(&toks, &global_token(fractions)).unwrap();
        // The network composes exactly what `compose_mean` composes, with the fractions read from
        // entries 15 to 18 of the global token.
        assert_eq!(
            out.policy.mean,
            compose_mean(&out.sensitivities, &fractions)
        );
        // Independent f64 reference of the same formula.
        let g: Vec<f64> = out
            .sensitivities
            .iter()
            .map(|s| {
                s.iter()
                    .zip(&fractions)
                    .map(|(&sens, &frac)| f64::from(sens) * f64::from(frac))
                    .sum()
            })
            .collect();
        let g_max = g.iter().fold(0.0f64, |a, v| a.max(v.abs()));
        for (k, (mean, g)) in out.policy.mean.iter().zip(&g).enumerate() {
            let want =
                -(g / g_max.max(f64::from(crate::env::SLOPE_NORM_FLOOR))) * f64::from(MEAN_BOUND);
            assert!(
                (f64::from(*mean) - want).abs() < 1e-4,
                "tier {k}: {mean} vs {want}"
            );
        }
        // The steepest tier sits at the bound (a hair below: the epsilon of the division).
        let peak = out.policy.mean.iter().fold(0.0f32, |a, m| a.max(m.abs()));
        assert!((peak - MEAN_BOUND).abs() < 1e-3, "{peak}");
    }
    // Different weights give a different direction from the same network.
    let a = net.forward(&toks, &global_token(BALANCED)).unwrap().0;
    let b = net
        .forward(&toks, &global_token(weights4(0.0, 0.0, 0.0, 1.0)))
        .unwrap()
        .0;
    assert_ne!(a.mean, b.mean);
}

#[test]
fn a_hand_set_sens_head_gives_every_tier_the_same_slope() {
    let mut net = small_network(7);
    net.sens_head.weights.fill(0.0);
    let biases = row4(0.9, 0.6, 0.3, 0.5);
    net.sens_head.biases.copy_from_slice(&biases);

    // Balanced: g = (0.9 + 0.6 + 0.3) / 3 = 0.6 on every tier, above the floor, so the mean is
    // exactly -MEAN_BOUND.
    let out = net
        .forward_full(&tokens(2), &global_token(BALANCED))
        .unwrap();
    assert_eq!(out.sensitivities, vec![biases; 2]);
    for m in &out.policy.mean {
        assert!((m + MEAN_BOUND).abs() < 1e-5, "{m}");
    }

    // Extinction only with a negative bias: g = -0.9 on every tier, the mean flips to +MEAN_BOUND.
    net.sens_head
        .biases
        .copy_from_slice(&row4(0.2, -0.9, 0.1, 0.0));
    let out = net
        .forward_full(&tokens(3), &global_token(weights4(0.0, 1.0, 0.0, 0.0)))
        .unwrap();
    for m in &out.policy.mean {
        assert!((m - MEAN_BOUND).abs() < 1e-5, "{m}");
    }

    // The sensitivities are the raw head outputs: nothing squashes a slope of 7 or -3.
    net.sens_head
        .biases
        .copy_from_slice(&row4(7.0, -3.0, 0.0, 0.25));
    let out = net
        .forward_full(&tokens(2), &global_token(BALANCED))
        .unwrap();
    assert_eq!(out.sensitivities, vec![row4(7.0, -3.0, 0.0, 0.25); 2]);
}

#[test]
fn a_hand_set_head_with_a_per_tier_difference_puts_the_steepest_tier_at_the_bound() {
    let mut net = small_network(13);
    let d = net.d_model;
    // The final norm passes only entry 0 of each normalised token; the sensitivity head reads that
    // entry alone: sens[k] = [z_k, 2 z_k, 0, 0] with z_k the tier's own normalised entry, so the
    // tiers differ by a known factor and the head adds nothing else.
    net.final_norm.gamma = vec![0.0; d];
    net.final_norm.gamma[0] = 1.0;
    net.final_norm.beta = vec![0.0; d];
    net.sens_head.weights.fill(0.0);
    net.sens_head.biases.fill(0.0);
    net.sens_head.weights[0] = 1.0; // component 0 reads entry 0
    net.sens_head.weights[d] = 2.0; // component 1 reads entry 0 twice as strongly

    let out = net
        .forward_full(&tokens(5), &global_token(weights4(1.0, 0.0, 0.0, 0.0)))
        .unwrap();
    let z: Vec<f32> = out.sensitivities.iter().map(|s| s[0]).collect();
    let spread =
        z.iter().fold(f32::MIN, |a, &v| a.max(v)) - z.iter().fold(f32::MAX, |a, &v| a.min(v));
    assert!(spread > 1e-4, "the tiers must differ: {z:?}");
    for (s, z) in out.sensitivities.iter().zip(&z) {
        assert_eq!(*s, row4(*z, 2.0 * z, 0.0, 0.0));
    }

    // Windowing only: g_k = z_k, so the steepest tier sits at -+MEAN_BOUND and the others scale.
    let z_max = z.iter().fold(0.0f32, |a, v| a.max(v.abs()));
    for (k, (mean, z)) in out.policy.mean.iter().zip(&z).enumerate() {
        let want = -f64::from(MEAN_BOUND) * f64::from(*z) / (f64::from(z_max) + 1e-6);
        assert!(
            (f64::from(*mean) - want).abs() < 1e-5,
            "tier {k}: {mean} vs {want}"
        );
    }
    let steepest = z
        .iter()
        .position(|v| v.abs() == z_max)
        .expect("a steepest tier");
    let want_sign = -z[steepest].signum();
    assert!((out.policy.mean[steepest] - want_sign * MEAN_BOUND).abs() < 1e-3);

    // Extinction alone reads the doubled slope of the same tiers: the same direction, since the
    // composition is scale-free.
    let extinction = compose_mean(&out.sensitivities, &weights4(0.0, 1.0, 0.0, 0.0));
    for (a, b) in extinction.iter().zip(&out.policy.mean) {
        assert!((a - b).abs() < 1e-5, "{a} vs {b}");
    }
}

#[test]
fn all_zero_sensitivities_give_a_zero_mean_never_nan() {
    let mut net = small_network(8);
    net.sens_head.weights.fill(0.0);
    net.sens_head.biases.fill(0.0);
    let out = net
        .forward_full(&tokens(3), &global_token(BALANCED))
        .unwrap();
    assert_eq!(out.policy.mean.len(), 3);
    assert!(out.policy.mean.iter().all(|&m| m == 0.0 && !m.is_nan()));
    // Weight fractions of zero (a degenerate global token) are equally harmless.
    net.sens_head.biases.fill(1.0);
    let out = net
        .forward_full(&tokens(3), &global_token([0.0; N_COMPONENTS]))
        .unwrap();
    assert!(out.policy.mean.iter().all(|&m| m == 0.0));
}

#[test]
fn the_gain_head_reads_the_pooled_vector_the_critic_reads() {
    let net = small_network(9);
    let global = global_token(BALANCED);
    let before = net.forward_full(&tokens(3), &global).unwrap();
    assert!(before.gain.is_finite());
    assert_ne!(before.gain, before.value, "two heads, two seeds");

    // Changing one tier token changes the pooled vector, so the gain moves.
    let mut changed = tokens(3);
    changed[1][0] += 0.7;
    let after = net.forward_full(&changed, &global).unwrap();
    assert!(
        (before.gain - after.gain).abs() > 1e-6,
        "{} -> {}",
        before.gain,
        after.gain
    );

    // A gain head that is a copy of the critic head reads the same vector, so the outputs are equal.
    let mut tied = small_network(9);
    tied.gain_head = tied.critic_head.clone();
    let out = tied.forward_full(&tokens(3), &global).unwrap();
    assert_eq!(out.gain, out.value);
}

#[test]
fn zero_tiers_give_an_empty_policy_and_a_finite_value_and_gain() {
    let net = small_network(10);
    let out = net.forward_full(&[], &global_token(BALANCED)).unwrap();
    assert!(out.policy.mean.is_empty() && out.policy.log_std.is_empty());
    assert!(out.sensitivities.is_empty());
    assert!(out.value.is_finite() && out.gain.is_finite());
}

#[test]
fn a_global_token_too_short_for_the_weight_fractions_is_a_dimension_error() {
    // A network whose global width stops before the fractions: it cannot compose a mean.
    let net = GemstoneSetAttentionNetwork::new(
        TierFeature::FEATURE_DIM,
        WEIGHT_FRACTION_INDEX,
        16,
        4,
        1,
        3,
    )
    .unwrap();
    let err = net
        .forward(&tokens(2), &[0.1; WEIGHT_FRACTION_INDEX])
        .unwrap_err();
    assert_eq!(
        err,
        ModelError::Dimension {
            what: "global token weight fractions",
            expected: WEIGHT_FRACTION_INDEX + N_COMPONENTS,
            got: WEIGHT_FRACTION_INDEX,
        }
    );
    // A token that disagrees with the declared global width is refused by the projection first.
    let net = small_network(3);
    assert!(matches!(
        net.forward(&tokens(2), &[0.1; 3]),
        Err(ModelError::Dimension {
            what: "linear input",
            ..
        })
    ));
}

/// The closed form of the parameter count for the observation layout of this build.
fn closed_form(d: usize, layers: usize) -> usize {
    let (dt, dg) = (TierFeature::FEATURE_DIM, GLOBAL_DIM);
    // Embeddings, per-layer block (attention 4d^2+4d, four norms 4d, MLP 4d^2+3d), the critic, the
    // sensitivity and the gain head, the final norm (2d) and the three raw log-stds.
    (dt + 1) * d
        + (dg + 1) * d
        + layers * (8 * d * d + 11 * d)
        + (d + 1)
        + (d + 1) * N_COMPONENTS
        + (d + 1)
        + 2 * d
        + 3
}

#[test]
fn param_count_follows_the_closed_form_with_the_new_heads() {
    let net = network_with_distinct_log_stds();
    assert_eq!(net.param_count(), closed_form(16, 2));
    // The closed form of the dimensions of this build (26 tier and 41 global features, seven
    // components): 80 d + 12 outside the layers.
    assert_eq!((TierFeature::FEATURE_DIM, GLOBAL_DIM), (26, 41));
    assert_eq!(
        closed_form(48, 2),
        2 * (8 * 48 * 48 + 11 * 48) + 80 * 48 + 12
    );
}

#[test]
fn param_count_approx_matches_the_closed_form_and_the_built_networks() {
    for tier in [
        ModelTier::Small,
        ModelTier::Medium,
        ModelTier::Large,
        ModelTier::ExtraLarge,
        ModelTier::ExtraExtraLarge,
    ] {
        let (d, _, layers) = tier.config();
        let exact = closed_form(d, layers) as f64;
        let approx = tier.param_count_approx() as f64;
        assert!(
            (exact - approx).abs() <= 10.0,
            "tier {}: approx {approx} vs closed form {exact}",
            tier.code_str()
        );
    }
    // The smaller tiers are cheap enough to build and count for real.
    for tier in [ModelTier::Small, ModelTier::Medium, ModelTier::Large] {
        let (d, _, layers) = tier.config();
        let net = tier.init_network(1);
        net.validate().unwrap();
        assert_eq!(net.param_count(), closed_form(d, layers));
        let actual = net.param_count() as f64;
        assert!((actual - tier.param_count_approx() as f64).abs() <= 0.10 * actual);
    }
}

#[test]
fn the_heads_start_at_full_xavier_scale_with_their_documented_seeds() {
    let a = ModelTier::Small.init_network(11);
    let b = ModelTier::Small.init_network(11);
    assert_eq!(a, b);
    assert_ne!(a, ModelTier::Small.init_network(12));

    let d = a.d_model;
    assert_eq!(
        (a.sens_head.in_features, a.sens_head.out_features),
        (d, N_COMPONENTS)
    );
    assert_eq!((a.gain_head.in_features, a.gain_head.out_features), (d, 1));
    assert_eq!(
        a.sens_head,
        Linear::init_xavier(d, N_COMPONENTS, 11 ^ 0xAAAA_BBBB)
    );
    assert_eq!(a.gain_head, Linear::init_xavier(d, 1, 11 ^ 0x6A1A_6A1A));
    assert_eq!(a.critic_head, Linear::init_xavier(d, 1, 11 ^ 0xCCCC_DDDD));
    assert!(a.sens_head.biases.iter().all(|&v| v == 0.0));
    assert!(a.gain_head.biases.iter().all(|&v| v == 0.0));

    // Gain 1.0: the sensitivity weights span the full Xavier range, no 0.01 head gain any more.
    let bound = (6.0 / (d + N_COMPONENTS) as f32).sqrt();
    let peak = a
        .sens_head
        .weights
        .iter()
        .fold(0.0f32, |m, w| m.max(w.abs()));
    assert!(peak <= bound + 1e-6, "{peak} vs {bound}");
    assert!(peak > 0.5 * bound, "{peak} vs {bound}");

    assert_eq!(a.log_std_raw, [log_std_raw_init(); LOG_STD_SLOTS]);
    assert!(a.final_norm.gamma.iter().all(|&g| g == 1.0));
    assert!(a.final_norm.beta.iter().all(|&b| b == 0.0));
}

#[test]
fn the_composed_mean_stays_inside_the_bound_on_a_full_size_stone() {
    let net = ModelTier::Small.init_network(3);
    let out = net
        .forward_full(
            &tokens(24),
            &global_token([1.0 / N_COMPONENTS as f32; N_COMPONENTS]),
        )
        .unwrap();
    assert_eq!(out.policy.mean.len(), 24);
    assert_eq!(out.sensitivities.len(), 24);
    assert!(out.policy.mean.iter().all(|m| m.is_finite()));
    let peak = out.policy.mean.iter().fold(0.0f32, |a, m| a.max(m.abs()));
    assert!((peak - MEAN_BOUND).abs() < 1e-3, "{peak}");
}
