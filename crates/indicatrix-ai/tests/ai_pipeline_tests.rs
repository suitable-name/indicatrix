//! Model-layer tests: layers, the set-attention network, its sensitivity heads and the composed
//! policy mean, sampling, weight persistence, action projection.
//!
//! Environment-reward and rollout-buffer tests live in `ai_env_training_tests.rs` so that this file only
//! depends on the model API (plus `TierFeature`, which `project_actions` takes, and the observation
//! constants the composed mean reads).

use indicatrix_ai::{
    env::{
        ContinuousTierAction, GlobalFeature, MAX_SAFE_CANDIDATE_ANGLE_DEG, MEAN_BOUND,
        MOBILITY_INDEX, N_COMPONENTS, TierFeature, WEIGHT_FRACTION_INDEX,
    },
    model::{
        GaussianPolicy, GemstoneSetAttentionNetwork, LOG_STD_MAX, LOG_STD_MIN, Linear, MlpBlock,
        ModelError, ModelTier, Prng, WeightError, compose_mean,
    },
};

const D_TIER: usize = TierFeature::FEATURE_DIM;
const D_GLOBAL: usize = GlobalFeature::FEATURE_DIM;

/// Slack on the `+-MEAN_BOUND` bound of the composed mean for `f32` rounding.
const MEAN_SLACK: f32 = 1e-5;

/// A global token whose weight fractions (the last seven entries) are `fractions`.
fn global_token_with(fractions: [f32; N_COMPONENTS]) -> Vec<f32> {
    let mut token = vec![0.3; D_GLOBAL];
    token[WEIGHT_FRACTION_INDEX..WEIGHT_FRACTION_INDEX + N_COMPONENTS].copy_from_slice(&fractions);
    token
}

/// `n` mobile tier tokens that differ from tier to tier. The mobility flag is set: an immobile
/// tier takes part in attention but gets no mean, so the composition would read zero there.
fn varied_tier_tokens(n: usize) -> Vec<Vec<f32>> {
    (0..n)
        .map(|i| {
            let mut token: Vec<f32> = (0..D_TIER)
                .map(|j| ((i * D_TIER + j) as f32 * 0.37).sin())
                .collect();
            token[MOBILITY_INDEX] = 1.0;
            token
        })
        .collect()
}

fn tier(angle_deg: f32, block_type: f32, pinned: bool) -> TierFeature {
    TierFeature {
        angle_normalized: angle_deg / 90.0,
        angle_deg,
        orbit_count: 8,
        block_type,
        is_pinned: if pinned { 1.0 } else { 0.0 },
        is_flat: 0.0,
        critical_margin_deg: 0.0,
        mast: 1.0,
        min_angle_deg: 0.0,
        max_angle_deg: 0.0,
        base_angle_deg: angle_deg,
        index_frac: 0.0,
        azimuth_sin: 0.0,
        azimuth_cos: 1.0,
        is_mobile: if pinned { 0.0 } else { 1.0 },
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

fn small_net() -> GemstoneSetAttentionNetwork {
    GemstoneSetAttentionNetwork::new(D_TIER, D_GLOBAL, 32, 2, 1, 777).expect("valid dimensions")
}

#[test]
fn test_linear_forward_pass() {
    let layer = Linear::init_xavier(4, 2, 42);
    let output = layer
        .forward(&[1.0, 0.5, -0.5, 0.0])
        .expect("matching input");
    assert_eq!(output.len(), 2);
    assert!(output.iter().all(|v| v.is_finite()));
}

#[test]
fn test_linear_new_mismatch_is_an_error() {
    assert!(matches!(
        Linear::new(3, 2, vec![0.0; 5], vec![0.0; 2]),
        Err(WeightError::DimensionMismatch { .. })
    ));
    assert!(matches!(
        Linear::new(3, 2, vec![0.0; 6], vec![0.0; 3]),
        Err(WeightError::DimensionMismatch { .. })
    ));
    assert!(Linear::new(3, 2, vec![0.0; 6], vec![0.0; 2]).is_ok());
}

#[test]
fn test_mlp_block_has_no_internal_residual() {
    let block = MlpBlock::new(8, 16, 12_345);
    let zero = block.forward(&[0.0; 8]).expect("matching input");
    // With zero input and zero biases, fc2(gelu(fc1(0))) = 0; a residual would also be 0, so probe a
    // non-zero input: the output must equal fc2(gelu(fc1(x))) exactly, with nothing of x added back.
    assert!(zero.iter().all(|v| *v == 0.0));
    let x = [0.1; 8];
    let y = block.forward(&x).expect("matching input");
    let hidden: Vec<f32> = block
        .fc1
        .forward(&x)
        .expect("fc1")
        .into_iter()
        .map(indicatrix_ai::model::gelu)
        .collect();
    assert_eq!(y, block.fc2.forward(&hidden).expect("fc2"));
}

#[test]
fn test_gemstone_set_attention_variable_tiers() {
    let net =
        GemstoneSetAttentionNetwork::new(D_TIER, D_GLOBAL, 64, 4, 2, 42).expect("valid dimensions");
    let global_token = [0.3; D_GLOBAL];

    for n in [3usize, 14, 28] {
        let tokens: Vec<Vec<f32>> = (0..n)
            .map(|i| vec![(i as f32).mul_add(0.02, 0.2); D_TIER])
            .collect();
        let (policy, value) = net.forward(&tokens, &global_token).expect("valid tokens");
        assert_eq!(policy.mean.len(), n);
        assert_eq!(policy.log_std.len(), n);
        assert!(value.is_finite());
        // The composed mean is in pre-squash units and reaches `+-MEAN_BOUND` on the steepest tier.
        assert!(
            policy
                .mean
                .iter()
                .all(|m| m.abs() <= MEAN_BOUND + MEAN_SLACK)
        );
        assert!(
            policy
                .log_std
                .iter()
                .all(|l| (LOG_STD_MIN..=LOG_STD_MAX).contains(l))
        );
    }
}

#[test]
fn test_forward_rejects_wrong_token_lengths_without_panicking() {
    let net = small_net();
    let global = [0.0; D_GLOBAL];
    let good_tier = vec![0.1; D_TIER];

    let bad_tier = net.forward(&[vec![0.0; D_TIER - 1]], &global);
    assert!(matches!(bad_tier, Err(ModelError::Dimension { .. })));

    let bad_global = net.forward(&[good_tier], &[0.0; 3]);
    assert!(matches!(bad_global, Err(ModelError::Dimension { .. })));
}

#[test]
fn test_forward_with_zero_tiers_gives_empty_policy_and_finite_value() {
    let net = small_net();
    let (policy, value) = net
        .forward(&[], &[0.1; D_GLOBAL])
        .expect("zero tiers are valid");
    assert!(policy.mean.is_empty() && policy.log_std.is_empty());
    assert!(value.is_finite());
}

#[test]
fn test_policy_sampling_is_deterministic_and_seed_sensitive() {
    let net = small_net();
    let tokens = vec![vec![0.4; D_TIER]; 5];
    let (policy, _) = net
        .forward(&tokens, &[0.2; D_GLOBAL])
        .expect("valid tokens");

    let (raw_a, lp_a) = policy.sample_raw(&mut Prng::new(1));
    let (raw_b, lp_b) = policy.sample_raw(&mut Prng::new(1));
    assert_eq!(raw_a, raw_b);
    assert_eq!(lp_a, lp_b);
    assert!(lp_a.is_finite());
    // The returned log-probability is the density of the drawn value.
    assert!((lp_a - policy.log_prob(&raw_a)).abs() < 1e-6);

    // Seed 0 is a valid, distinct stream (not a degenerate all-equal draw).
    let (raw_zero, _) = policy.sample_raw(&mut Prng::new(0));
    let (raw_one, _) = policy.sample_raw(&mut Prng::new(1));
    assert_ne!(raw_zero, raw_one);
    assert!(
        raw_zero.windows(2).any(|w| w[0] != w[1]),
        "per-dimension noise must differ"
    );

    // Deterministic mode uses the mean; the step is bounded by max_step_deg.
    let det = policy.sample(0, true, 0.75);
    assert_eq!(det.deltas_deg.len(), 5);
    assert!(det.deltas_deg.iter().all(|d| d.abs() <= 0.75));
    let stochastic = GaussianPolicy::squash(&raw_a, 0.75);
    assert!(stochastic.deltas_deg.iter().all(|d| d.abs() <= 0.75));
}

#[test]
fn test_gemstone_set_attention_serialization_roundtrip() {
    let net = small_net();
    let json = net.to_json_string().expect("serializes");
    assert_ne!(json, "");

    let loaded = GemstoneSetAttentionNetwork::from_json_str(&json).expect("valid json deserialize");
    assert_eq!(net, loaded);
}

#[test]
fn test_load_rejects_truncated_weights_instead_of_panicking_later() {
    let net = small_net();
    let mut value: serde_json::Value =
        serde_json::from_str(&net.to_json_string().expect("serializes")).unwrap();
    value["tier_proj"]["weights"]
        .as_array_mut()
        .expect("weights array")
        .pop();
    let truncated = value.to_string();

    let result = GemstoneSetAttentionNetwork::from_json_str(&truncated);
    assert!(
        matches!(result, Err(WeightError::DimensionMismatch { .. })),
        "got {result:?}"
    );
}

#[test]
fn test_load_rejects_structurally_broken_networks() {
    let mut net = small_net();
    net.pre_attn_norms.pop();
    assert!(net.validate().is_err());

    let mut net = small_net();
    net.num_heads = 3; // 32 % 3 != 0
    assert!(net.validate().is_err());

    let mut net = small_net();
    net.critic_head.weights[0] = f32::NAN;
    assert!(net.validate().is_err());
}

#[test]
fn test_every_tier_builds_a_valid_network_and_param_count_matches_the_estimate() {
    for tier in [
        ModelTier::Small,
        ModelTier::Medium,
        ModelTier::Large,
        ModelTier::ExtraLarge,
        ModelTier::ExtraExtraLarge,
    ] {
        let net = tier.init_network(1);
        net.validate()
            .unwrap_or_else(|e| panic!("tier {} invalid: {e}", tier.code_str()));
        let actual = net.param_count() as f64;
        let approx = tier.param_count_approx() as f64;
        assert!(
            (actual - approx).abs() <= 0.10 * actual,
            "tier {}: approx {approx} vs actual {actual}",
            tier.code_str()
        );
    }
}

#[test]
fn test_model_tier_sizing_and_xxl() {
    assert_eq!(ModelTier::Small.config(), (48, 4, 2));
    assert_eq!(ModelTier::Medium.config(), (64, 4, 3));
    assert_eq!(ModelTier::Large.config(), (128, 8, 4));
    assert_eq!(ModelTier::ExtraLarge.config(), (256, 8, 6));
    assert_eq!(ModelTier::ExtraExtraLarge.config(), (384, 12, 8));

    assert_eq!(ModelTier::Small.code_str(), "S");
    assert_eq!(ModelTier::ExtraExtraLarge.code_str(), "XXL");

    assert_eq!(
        ModelTier::from_str_loose("xxl"),
        Some(ModelTier::ExtraExtraLarge)
    );
    assert_eq!(ModelTier::from_str_loose("m"), Some(ModelTier::Medium));
    assert_eq!(ModelTier::from_str_loose("huge"), None);

    let xxl_net = ModelTier::ExtraExtraLarge.init_network(12_345);
    assert_eq!(xxl_net.d_model, 384);
    assert_eq!(xxl_net.num_heads, 12);
    assert_eq!(xxl_net.attn_layers.len(), 8);

    let (policy, val) = xxl_net
        .forward(&vec![vec![0.5; D_TIER]; 8], &[1.5; D_GLOBAL])
        .expect("valid tokens");
    assert_eq!(policy.mean.len(), 8);
    assert!(val.is_finite());
}

#[test]
fn test_new_rejects_invalid_dimensions() {
    assert!(GemstoneSetAttentionNetwork::new(D_TIER, D_GLOBAL, 30, 4, 1, 0).is_err());
    assert!(GemstoneSetAttentionNetwork::new(D_TIER, D_GLOBAL, 32, 0, 1, 0).is_err());
    assert!(GemstoneSetAttentionNetwork::new(0, D_GLOBAL, 32, 2, 1, 0).is_err());
}

#[test]
fn test_kinematic_projection_pins_tiers_keeps_signs_and_leaves_the_critical_angle_to_the_score() {
    let crit_angle = 39.0f32;
    let tiers = [
        tier(0.0, 1.0, true),     // pinned table
        tier(35.0, 1.0, false),   // movable crown
        tier(-40.2, -1.0, false), // pavilion just above the critical angle, stored negative
    ];

    // delta[2] = +0.6 makes the pavilion -39.6, shallower than the critical angle plus the
    // one-degree margin. The margin is context for the policy, not a kinematic bound: whether a
    // shallow pavilion pays off is the material score's verdict, so the step passes through.
    let mut action = ContinuousTierAction::new(vec![0.5, 0.4, 0.6]);
    GemstoneSetAttentionNetwork::project_actions(&mut action, &tiers, crit_angle, 0.75);

    assert_eq!(action.deltas_deg[0], 0.0, "pinned tier must not move");
    assert!(
        (action.deltas_deg[1] - 0.4).abs() < 1e-9,
        "valid crown step passes through"
    );
    assert!(
        (action.deltas_deg[2] - 0.6).abs() < 1e-12,
        "the pavilion step passes through, got {}",
        action.deltas_deg[2]
    );
    let resulting = f64::from(-40.2f32) + action.deltas_deg[2];
    assert!(resulting < 0.0, "pavilion sign must never flip");
    assert!(
        resulting.abs() < f64::from(crit_angle) + 1.0,
        "|pavilion| = {} is inside the margin: the projection does not enforce it",
        resulting.abs()
    );
}

#[test]
fn test_projection_does_not_force_pavilion_to_max_step() {
    let tiers = [tier(-45.0, -1.0, false)];

    // A modest step toward shallower is far from the floor and must pass through unchanged,
    // not be forced to +max_step as the signed/positive mix-up used to do.
    let mut up = ContinuousTierAction::new(vec![0.6]);
    GemstoneSetAttentionNetwork::project_actions(&mut up, &tiers, 39.0, 0.75);
    assert!((up.deltas_deg[0] - 0.6).abs() < 1e-12);

    let mut down = ContinuousTierAction::new(vec![-0.3]);
    GemstoneSetAttentionNetwork::project_actions(&mut down, &tiers, 39.0, 0.75);
    assert!((down.deltas_deg[0] + 0.3).abs() < 1e-12);

    let mut zero = ContinuousTierAction::new(vec![0.0]);
    GemstoneSetAttentionNetwork::project_actions(&mut zero, &tiers, 39.0, 0.75);
    assert_eq!(zero.deltas_deg[0], 0.0);
}

#[test]
fn test_projection_clamps_to_upper_bounds_and_step_limit() {
    // The shared safe maximum (cut-core's `MAX_SAFE_CANDIDATE_ANGLE_DEG`, 89.5 degrees) bounds
    // both blocks: a deep pavilion 0.3 degrees short of it asked to go 0.75 deeper is capped
    // there, as is a crown 0.2 degrees short of it.
    let max = MAX_SAFE_CANDIDATE_ANGLE_DEG;
    let pavilion = -(max - 0.3) as f32;
    let crown = (max - 0.2) as f32;
    let tiers = [
        tier(pavilion, -1.0, false),
        tier(crown, 1.0, false),
        tier(20.0, 1.0, false),
    ];
    let mut action = ContinuousTierAction::new(vec![-5.0, 5.0, -5.0]);
    GemstoneSetAttentionNetwork::project_actions(&mut action, &tiers, 39.0, 0.75);
    assert!((f64::from(pavilion) + action.deltas_deg[0] + max).abs() < 1e-4);
    assert!((f64::from(crown) + action.deltas_deg[1] - max).abs() < 1e-4);
    // Large crown request is limited to one step.
    assert!((action.deltas_deg[2] + 0.75).abs() < 1e-12);
}

#[test]
fn test_projection_zeroes_non_finite_and_unmatched_deltas() {
    let tiers = [tier(30.0, 1.0, false)];
    let mut action = ContinuousTierAction::new(vec![f64::NAN, 0.5]);
    GemstoneSetAttentionNetwork::project_actions(&mut action, &tiers, 39.0, 0.75);
    assert_eq!(action.deltas_deg, vec![0.0, 0.0]);
}

#[test]
fn test_forward_full_exposes_the_sensitivities_the_mean_is_composed_from() {
    let net = small_net();
    let tokens = varied_tier_tokens(6);
    let fractions = [0.4, 0.3, 0.2, 0.1, 0.0, 0.0, 0.0];
    let global = global_token_with(fractions);

    let full = net.forward_full(&tokens, &global).expect("valid tokens");
    assert_eq!(full.sensitivities.len(), 6);
    assert!(full.sensitivities.iter().flatten().all(|s| s.is_finite()));
    assert!(full.value.is_finite() && full.gain.is_finite());
    assert_eq!(full.policy.mean.len(), 6);
    // The mean is exactly the composition of the sensitivities with the token's fractions.
    assert_eq!(
        full.policy.mean,
        compose_mean(&full.sensitivities, &fractions)
    );

    // `forward` is `forward_full` without the extras.
    let (policy, value) = net.forward(&tokens, &global).expect("valid tokens");
    assert_eq!(policy, full.policy);
    assert_eq!(value, full.value);
}

#[test]
fn test_the_weight_fractions_in_the_global_token_steer_the_mean() {
    let net = small_net();
    let tokens = varied_tier_tokens(6);
    let pure = [
        [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
    ];
    let mut means = Vec::new();
    for fractions in pure
        .into_iter()
        .chain([[1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 0.0, 0.0, 0.0, 0.0]])
    {
        let full = net
            .forward_full(&tokens, &global_token_with(fractions))
            .expect("valid tokens");
        assert_eq!(
            full.policy.mean,
            compose_mean(&full.sensitivities, &fractions),
            "{fractions:?}"
        );
        // The steepest tier sits at the bound whatever the weighting.
        let peak = full.policy.mean.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - MEAN_BOUND).abs() < 1e-3, "{fractions:?}: {peak}");
        means.push(full.policy.mean);
    }
    // One network, seven one-hot weightings, seven different directions.
    for i in 0..7 {
        for j in (i + 1)..7 {
            assert_ne!(means[i], means[j], "weightings {i} and {j}");
        }
    }
}

#[test]
fn test_a_network_with_no_sensitivities_has_a_zero_mean() {
    let mut net = small_net();
    net.sens_head.weights.fill(0.0);
    net.sens_head.biases.fill(0.0);
    let (policy, value) = net
        .forward(
            &varied_tier_tokens(5),
            &global_token_with([1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 0.0, 0.0, 0.0, 0.0]),
        )
        .expect("valid tokens");
    assert_eq!(policy.mean, vec![0.0; 5]);
    assert!(value.is_finite());
}

#[test]
fn test_a_constant_sensitivity_network_turns_every_tier_one_way() {
    // Zero weights and bias `[b, 0, ...]`: every tier has the windowing sensitivity `b`.
    let constant = |bias: f32| {
        let mut net = small_net();
        net.sens_head.weights.fill(0.0);
        net.sens_head.biases.fill(0.0);
        net.sens_head.biases[0] = bias;
        net
    };
    let tokens = varied_tier_tokens(4);
    let windowing_only = global_token_with([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);

    // A positive windowing slope means "lower the angle": every tier at -MEAN_BOUND.
    let (down, _) = constant(0.7)
        .forward(&tokens, &windowing_only)
        .expect("valid tokens");
    for mean in &down.mean {
        assert!((mean + MEAN_BOUND).abs() < 1e-4, "{mean}");
    }
    // The opposite sign gives the opposite direction, of the same size.
    let (up, _) = constant(-0.7)
        .forward(&tokens, &windowing_only)
        .expect("valid tokens");
    for mean in &up.mean {
        assert!((mean - MEAN_BOUND).abs() < 1e-4, "{mean}");
    }
    // An objective that ignores windowing has nothing to descend.
    let (silent, _) = constant(0.7)
        .forward(
            &tokens,
            &global_token_with([0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        )
        .expect("valid tokens");
    assert_eq!(silent.mean, vec![0.0; 4]);

    // The squashed step of the steepest tier is 95 % of the step limit.
    let step = down.sample(0, true, 0.5);
    for delta in &step.deltas_deg {
        assert!((delta + 0.475).abs() < 1e-4, "{delta}");
    }
}

#[test]
fn test_a_global_token_too_short_for_the_weight_fractions_is_an_error_not_a_panic() {
    // A network whose global token stops before the fractions can embed its input but cannot
    // compose a mean.
    let narrow = GemstoneSetAttentionNetwork::new(D_TIER, WEIGHT_FRACTION_INDEX, 32, 2, 1, 5)
        .expect("valid dimensions");
    let result = narrow.forward(&varied_tier_tokens(3), &[0.1; WEIGHT_FRACTION_INDEX]);
    match result {
        Err(ModelError::Dimension { expected, got, .. }) => {
            assert_eq!(expected, WEIGHT_FRACTION_INDEX + N_COMPONENTS);
            assert_eq!(got, WEIGHT_FRACTION_INDEX);
        }
        other => panic!("expected a dimension error, got {other:?}"),
    }
}
