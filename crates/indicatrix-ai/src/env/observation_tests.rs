use super::*;
use indicatrix_cut_core::ObjectivePreset;

fn sample(tiers: usize) -> GemstoneObservation {
    GemstoneObservation {
        tiers: (0..tiers)
            .map(|i| TierFeature {
                angle_normalized: i as f32 / 90.0,
                angle_deg: i as f32,
                base_angle_deg: i as f32 - 1.0,
                orbit_count: 8,
                block_type: if i.is_multiple_of(2) { 1.0 } else { -1.0 },
                is_pinned: 0.0,
                is_flat: 0.0,
                critical_margin_deg: 3.0,
                mast: 0.5,
                min_angle_deg: -85.0,
                max_angle_deg: 80.0,
                index_frac: i as f32 / tiers as f32,
                azimuth_sin: 0.0,
                azimuth_cos: 1.0,
                is_mobile: 1.0,
                orbit_sin_2: 0.0,
                orbit_cos_2: 1.0,
                orbit_sin_4: 0.0,
                orbit_cos_4: 1.0,
                live_area_fraction: 0.0,
                live_centroid_x: 0.0,
                live_centroid_y: 0.0,
                live_centroid_z: 0.0,
                geometry_available: 0.0,
                constraint_kind: 1.0,
                relation_driven: 0.0,
                target_driven: 0.0,
            })
            .collect(),
        global: GlobalFeature {
            refractive_index: 1.54,
            target_refractive_index: 1.77,
            critical_angle_deg: 40.5,
            target_critical_angle_deg: 34.4,
            dispersion: 0.02,
            windowing_pct: 10.0,
            extinction_pct: 20.0,
            tilt_brilliance_pct: 70.0,
            yield_pct: 40.0,
            table_pct: 55.0,
            crown_to_pavilion_ratio: 0.4,
            min_girdle_ratio: 1.0,
            best_score_so_far: 30.0,
            progress_ratio: 0.25,
            base_score: 35.0,
            task_kind: 1.0,
            tone_lighter_loss_pct: 0.0,
            tone_deeper_loss_pct: 0.0,
            shape_distance_pct: 0.0,
            shape_target_table_pct: 0.0,
            shape_target_ratio: 0.0,
            shape_weight: 0.0,
            probe_scale_deg: 0.25,
            action_step_deg: 0.5,
            fidelity: 0.0,
            yield_available: 1.0,
            shape_target_available: 0.0,
            absorption_450: 0.0,
            absorption_550: 0.0,
            absorption_650: 0.0,
            absorption_path_scale: 1.0,
            scattering_sigma_s: 0.0,
            lighting_index: 0.0,
            tone_available: 1.0,
            weight_fractions: [0.5, 0.25, 0.125, 0.125, 0.0, 0.0, 0.0],
        },
    }
}

#[test]
fn flatten_is_global_token_then_tier_tokens_then_zeros() {
    assert_eq!(OBS_LAYOUT_VERSION, 5);
    assert_eq!((GLOBAL_DIM, TIER_DIM), (41, 26));
    let obs = sample(3);
    assert_eq!(obs.global_token().len(), 41);
    assert!(obs.tier_tokens().iter().all(|token| token.len() == 26));
    let flat = obs.flatten(5);
    let mut expected = obs.global_token();
    expected.extend(obs.tier_tokens().concat());
    expected.resize(flat_len(5), 0.0);
    assert_eq!(flat, expected);
    assert_eq!(flat.len(), flat_len(5));
    assert_eq!(flat.len(), 41 + 5 * 26);
    // Two padded tier slots, all zero.
    assert!(flat[41 + 3 * 26..].iter().all(|&v| v == 0.0));
}

#[test]
fn the_objective_coefficients_are_the_last_seven_global_entries_unscaled() {
    assert_eq!(WEIGHT_FRACTION_INDEX, 34);
    assert_eq!(WEIGHT_FRACTION_INDEX + 7, GLOBAL_DIM);
    let obs = sample(2);
    let token = obs.global_token();
    assert_eq!(
        token[WEIGHT_FRACTION_INDEX..],
        [0.5, 0.25, 0.125, 0.125, 0.0, 0.0, 0.0]
    );
    // The flat vector carries them at the same place, ahead of the tier tokens.
    let flat = obs.flatten(3);
    assert_eq!(
        flat[WEIGHT_FRACTION_INDEX..GLOBAL_DIM],
        obs.global.weight_fractions
    );
    assert_eq!(
        flat[GLOBAL_DIM..GLOBAL_DIM + TIER_DIM],
        obs.tier_tokens()[0]
    );
}

#[test]
fn the_preset_weights_give_their_fractions() {
    let fractions = |preset: ObjectivePreset| weight_fractions(&preset.weights());
    let close = |actual: [f32; 7], expected: [f32; 7]| {
        actual
            .iter()
            .zip(expected)
            .all(|(a, e)| (a - e).abs() < 1e-6)
    };
    let sixth = 1.0 / 6.0;
    let third = 1.0 / 3.0;
    assert!(close(
        fractions(ObjectivePreset::Balanced),
        [third, third, third, 0.0, 0.0, 0.0, 0.0]
    ));
    assert!(close(
        fractions(ObjectivePreset::Brilliance),
        [sixth, sixth, 4.0 * sixth, 0.0, 0.0, 0.0, 0.0]
    ));
    assert!(close(
        fractions(ObjectivePreset::LowWindowing),
        [4.0 * sixth, sixth, sixth, 0.0, 0.0, 0.0, 0.0]
    ));
    assert!(close(
        fractions(ObjectivePreset::LowExtinction),
        [sixth, 4.0 * sixth, sixth, 0.0, 0.0, 0.0, 0.0]
    ));
    assert!(close(
        fractions(ObjectivePreset::KeepWeight),
        [sixth, sixth, sixth, 3.0 * sixth, 0.0, 0.0, 0.0]
    ));
    assert!(close(
        fractions(ObjectivePreset::LightenDark),
        [1.0 / 6.0, 1.0 / 6.0, 1.0 / 6.0, 0.0, 0.5, 0.0, 0.0]
    ));
    assert!(close(
        fractions(ObjectivePreset::IntensifyPale),
        [1.0 / 6.0, 1.0 / 6.0, 1.0 / 6.0, 0.0, 0.0, 0.5, 0.0]
    ));
    // Balanced is exactly a third each: the fraction is the weight over the sum.
    assert_eq!(
        fractions(ObjectivePreset::Balanced),
        [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 0.0, 0.0, 0.0, 0.0]
    );
}

#[test]
fn the_weight_fractions_add_up_to_one_and_scale_free() {
    let weights = ObjectiveWeights {
        windowing: 2.0,
        extinction: 0.5,
        tilt_brilliance: 3.5,
        yield_weight: 1.0,
        ..ObjectiveWeights::default()
    };
    let fractions = weight_fractions(&weights);
    assert!((fractions.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    assert!(
        fractions
            .iter()
            .all(|fraction| (0.0..=1.0).contains(fraction))
    );
    assert!((fractions[0] - 2.0 / 7.0).abs() < 1e-6);
    // Scaling every weight by the same factor changes nothing.
    let doubled = ObjectiveWeights {
        windowing: 4.0,
        extinction: 1.0,
        tilt_brilliance: 7.0,
        yield_weight: 2.0,
        ..weights
    };
    let again = weight_fractions(&doubled);
    assert!(
        fractions
            .iter()
            .zip(again)
            .all(|(a, b)| (a - b).abs() < 1e-6)
    );
}

#[test]
fn weights_without_a_positive_sum_fall_back_to_the_optical_thirds() {
    let fallback = [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 0.0, 0.0, 0.0, 0.0];
    let zero = ObjectiveWeights {
        windowing: 0.0,
        extinction: 0.0,
        tilt_brilliance: 0.0,
        yield_weight: 0.0,
        ..ObjectiveWeights::default()
    };
    assert_eq!(weight_fractions(&zero), fallback);
    // Negative weights are clamped to zero before the sum: nothing positive is left.
    let negative = ObjectiveWeights {
        windowing: -1.0,
        extinction: -2.0,
        tilt_brilliance: -3.0,
        yield_weight: -4.0,
        ..zero
    };
    assert_eq!(weight_fractions(&negative), fallback);
    // A non-finite weight counts as zero; the other weights keep their share.
    let broken = ObjectiveWeights {
        windowing: f32::NAN,
        extinction: f32::INFINITY,
        tilt_brilliance: 3.0,
        yield_weight: 1.0,
        ..zero
    };
    let fractions = weight_fractions(&broken);
    assert!(fractions.iter().all(|fraction| fraction.is_finite()));
    assert_eq!(fractions, [0.0, 0.0, 0.75, 0.25, 0.0, 0.0, 0.0]);
    // One negative weight among positive ones is dropped, not subtracted.
    let mixed = ObjectiveWeights {
        windowing: -5.0,
        extinction: 1.0,
        tilt_brilliance: 1.0,
        yield_weight: 0.0,
        ..zero
    };
    assert_eq!(
        weight_fractions(&mixed),
        [0.0, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0]
    );
}

#[test]
fn the_token_orders_are_the_contract_with_the_network() {
    let tier = TierFeature {
        angle_normalized: 0.5,
        angle_deg: 45.0,
        base_angle_deg: 36.0,
        orbit_count: 16,
        block_type: 1.0,
        is_pinned: 0.0,
        is_flat: 1.0,
        critical_margin_deg: 9.0,
        mast: 0.25,
        min_angle_deg: 18.0,
        max_angle_deg: 72.0,
        index_frac: 0.4,
        azimuth_sin: 0.6,
        azimuth_cos: 0.8,
        is_mobile: 1.0,
        orbit_sin_2: 0.0,
        orbit_cos_2: 1.0,
        orbit_sin_4: 0.0,
        orbit_cos_4: 1.0,
        live_area_fraction: 0.0,
        live_centroid_x: 0.0,
        live_centroid_y: 0.0,
        live_centroid_z: 0.0,
        geometry_available: 0.0,
        constraint_kind: 1.0,
        relation_driven: 0.0,
        target_driven: 0.0,
    };
    let tier_token = tier.to_feature_vector();
    assert_eq!(tier_token.len(), TIER_DIM);
    assert_eq!(tier_token[0..4], [0.5, 0.4, 0.5, 1.0]);
    assert_eq!(tier_token[MOBILITY_INDEX], 1.0);
    let global = sample(1).global.to_feature_vector();
    // best score 30 / 100, progress 0.25, the base score 35 / 100, then seven weight
    // fractions last (unscaled).
    assert_eq!(global[12..15], [0.3, 0.25, 0.35]);
    assert_eq!(
        global[WEIGHT_FRACTION_INDEX..],
        [0.5, 0.25, 0.125, 0.125, 0.0, 0.0, 0.0]
    );
}

#[test]
fn azimuth_of_index_zero_is_sin_0_cos_1_and_of_a_quarter_turn_is_sin_1_cos_0() {
    assert_eq!(azimuth_sin_cos(Some(0.0), 96), (0.0, 1.0));
    // A quarter turn of a 96-tooth wheel is position 24.
    let (sin, cos) = azimuth_sin_cos(Some(24.0), 96);
    assert!((sin - 1.0).abs() < 1e-6 && cos.abs() < 1e-6, "{sin} {cos}");
    // Half a turn: position 48 is (0, -1); an eighth (12) is (sqrt(1/2), sqrt(1/2)).
    let (sin, cos) = azimuth_sin_cos(Some(48.0), 96);
    assert!(sin.abs() < 1e-6 && (cos + 1.0).abs() < 1e-6, "{sin} {cos}");
    let (sin, cos) = azimuth_sin_cos(Some(12.0), 96);
    let eighth = std::f32::consts::FRAC_1_SQRT_2;
    assert!((sin - eighth).abs() < 1e-6 && (cos - eighth).abs() < 1e-6);
    // The wheel is a ring: the closing position is the same azimuth as position 0.
    let (sin, cos) = azimuth_sin_cos(Some(96.0), 96);
    assert!(sin.abs() < 1e-6 && (cos - 1.0).abs() < 1e-6, "{sin} {cos}");
    // No index, no gear teeth or a negative count, or a non-finite index: azimuth zero.
    assert_eq!(azimuth_sin_cos(None, 96), (0.0, 1.0));
    assert_eq!(azimuth_sin_cos(Some(5.0), 0), (0.0, 1.0));
    assert_eq!(azimuth_sin_cos(Some(5.0), -96), (0.0, 1.0));
    assert_eq!(azimuth_sin_cos(Some(f64::NAN), 96), (0.0, 1.0));
}

#[test]
fn split_flat_inverts_flatten() {
    let obs = sample(2);
    let flat = obs.flatten(4);
    let (global, tiers) = split_flat(&flat, 4).expect("length matches");
    assert_eq!(global, obs.global_token().as_slice());
    assert_eq!(tiers.len(), 4 * TIER_DIM);
    assert!(tiers[2 * TIER_DIM..].iter().all(|&v| v == 0.0));
    assert!(split_flat(&flat, 3).is_none());
}

#[test]
fn try_flatten_rejects_overflow_instead_of_truncating() {
    let obs = sample(6);
    assert_eq!(
        obs.try_flatten(4),
        Err(ObservationError {
            tiers: 6,
            max_tiers: 4
        })
    );
    assert!(obs.try_flatten(6).is_ok());
}

fn tier_at(angle_deg: f32, block_type: f32) -> TierFeature {
    TierFeature {
        angle_normalized: angle_deg / 90.0,
        angle_deg,
        base_angle_deg: angle_deg,
        orbit_count: 8,
        block_type,
        is_pinned: 0.0,
        is_flat: 0.0,
        critical_margin_deg: 0.0,
        mast: 0.5,
        min_angle_deg: 0.0,
        max_angle_deg: 0.0,
        index_frac: 0.0,
        azimuth_sin: 0.0,
        azimuth_cos: 1.0,
        is_mobile: 1.0,
        orbit_sin_2: 0.0,
        orbit_cos_2: 1.0,
        orbit_sin_4: 0.0,
        orbit_cos_4: 1.0,
        live_area_fraction: 0.0,
        live_centroid_x: 0.0,
        live_centroid_y: 0.0,
        live_centroid_z: 0.0,
        geometry_available: 0.0,
        constraint_kind: 1.0,
        relation_driven: 0.0,
        target_driven: 0.0,
    }
}

#[test]
fn free_tier_mask_excludes_tiers_the_projection_forces() {
    let mut obs = sample(0);
    // The critical angle is context only: the window is the shared safe interval of the block,
    // `[1, MAX_SAFE]` in magnitude, whatever the material.
    obs.global.target_critical_angle_deg = 40.0;
    assert_eq!(PAVILION_MAX_ANGLE_DEG, CROWN_MAX_ANGLE_DEG);
    let steep = PAVILION_MAX_ANGLE_DEG;
    // A pinned or flat tier is immobile in the observation as well (the environment sets both).
    let mut pinned = tier_at(-60.0, -1.0);
    pinned.is_pinned = 1.0;
    pinned.is_mobile = 0.0;
    let mut flat = tier_at(0.0, 1.0);
    flat.is_flat = 1.0;
    flat.is_mobile = 0.0;
    obs.tiers = vec![
        tier_at(-60.0, -1.0),          // pavilion inside the window: free
        tier_at(-38.0, -1.0),          // shallower than the critical angle: still free
        tier_at(-(steep + 0.3), -1.0), // pavilion steeper than the shared safe maximum: frozen
        tier_at(50.0, 1.0),            // crown inside the shared safety interval: free
        tier_at(10.0, 1.0),            // signed-safe and movable
        tier_at(steep + 0.2, 1.0),     // crown beyond the shared safe maximum: frozen
        tier_at(0.0, 0.0),             // girdle: no envelope, free
        pinned,
        flat,
    ];
    assert_eq!(
        obs.free_tier_mask(),
        [true, true, false, true, true, false, true, false, false]
    );

    // The mask and the projection agree: a free crown or pavilion tier keeps a small sample, a
    // masked one is frozen whatever the sample (an out-of-range angle is never repaired by an
    // automatic correction, and an immobile tier receives no action).
    for (tier, free) in obs.tiers.iter().zip(obs.free_tier_mask()) {
        if tier.block_type.abs() < 0.5 {
            continue;
        }
        let mut action = crate::env::ContinuousTierAction::new(vec![0.3]);
        crate::model::GemstoneSetAttentionNetwork::project_actions(
            &mut action,
            std::slice::from_ref(tier),
            40.0,
            0.75,
        );
        assert_eq!(action.deltas_deg[0] != 0.0, free, "{}", tier.angle_deg);
    }

    // Critical angle is context only; safe authoring bounds do not autonomously correct it.
    obs.global.target_critical_angle_deg = 35.0;
    assert!(obs.free_tier_mask()[1]);
}

#[test]
fn envelope_follows_the_block() {
    assert_eq!(angle_envelope(1.0, 40.0), (1.0, CROWN_MAX_ANGLE_DEG));
    assert_eq!(angle_envelope(-1.0, 40.0), (-PAVILION_MAX_ANGLE_DEG, -1.0));
    assert_eq!(angle_envelope(0.0, 40.0), (0.0, 0.0));
}
