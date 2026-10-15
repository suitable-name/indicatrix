//! Tests of the teacher: the slope-to-action formulas, the label, and the probe/oracle pieces
//! the label is assembled from.

use super::*;
use crate::env::{
    OptimizerEnv, RetargetEnv,
    test_fixtures::{fixture_env, fixture_env_with},
    traits::GemstoneRlEnv,
};
use indicatrix_cut_core::ObjectivePreset;

/// The probe size the tests label with (a quarter degree, the size the existing gradient
/// tests use).
const H: f64 = 0.25;

fn close(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs() <= tolerance
}

/// The probes of every tier of the design, one task each as a trainer's pool would run them:
/// the per-tier sensitivities and the sum of the evaluations they cost.
fn probe_all(env: &RetargetEnv, h_deg: f64) -> (Vec<Option<TierSensitivity>>, usize) {
    let mut evaluations = 0;
    let tiers = (0..env.base_design().tiers.len())
        .map(|tier| {
            let probe = env.probe_tier(tier, h_deg);
            evaluations += probe.evaluations;
            probe.sensitivity
        })
        .collect();
    (tiers, evaluations)
}

#[test]
fn the_yield_loss_is_floored_at_zero_for_a_stone_bigger_than_its_preform() {
    use crate::env::common::{objective_score, yield_loss_pct};
    use indicatrix_cut_core::{ObjectiveComponents, ObjectiveWeights};

    assert_eq!(yield_loss_pct(40.0), 60.0);
    assert_eq!(yield_loss_pct(100.0), 0.0);
    assert_eq!(
        yield_loss_pct(130.0),
        0.0,
        "a yield above 100 % is a full yield, not a gain"
    );

    // With the floor the score of a Keep Weight objective stays within 0..=100 whatever the
    // yield says, so an episode cannot end as "perfect" because the stone outgrew its preform.
    let weights = ObjectiveWeights {
        yield_weight: 3.0,
        ..ObjectiveWeights::default()
    };
    let components = ObjectiveComponents {
        windowing_pct: 0.0,
        extinction_pct: 0.0,
        tilt_brilliance_pct: 100.0,
    };
    let score = objective_score(&weights, &components, Some(130.0));
    assert!(score >= 0.0, "score {score} went negative");
    assert_eq!(score, objective_score(&weights, &components, Some(100.0)));
}

#[test]
fn the_constants_are_the_contract() {
    assert_eq!(N_COMPONENTS, 4);
    assert_eq!(
        [
            COMPONENT_WINDOWING,
            COMPONENT_EXTINCTION,
            COMPONENT_BRILLIANCE_LOSS,
            COMPONENT_YIELD_LOSS
        ],
        [0, 1, 2, 3]
    );
    assert_eq!(DEFAULT_LINE_SEARCH, [1.0, 0.5, 0.25]);
    assert_eq!(SLOPE_EPS, 1e-6);
    // 95 % of the step for the steepest tier.
    assert!(
        close(MEAN_BOUND.tanh(), 0.95, 1e-6),
        "{}",
        MEAN_BOUND.tanh()
    );
}

#[test]
fn the_steepest_tier_gets_the_mean_bound_against_its_slope() {
    let direction = direction_from_slopes(&[Some(2.0), Some(-4.0), Some(1.0), Some(4.0)]);
    assert_eq!(direction.len(), 4);
    // Both |4| tiers are steepest: the bound, opposite to the sign of the slope.
    assert!(close(direction[1], MEAN_BOUND, 1e-5), "{direction:?}");
    assert!(close(direction[3], -MEAN_BOUND, 1e-5), "{direction:?}");
    // The others are proportional to their slope.
    assert!(
        close(direction[0], -MEAN_BOUND * 0.5, 1e-5),
        "{direction:?}"
    );
    assert!(
        close(direction[2], -MEAN_BOUND * 0.25, 1e-5),
        "{direction:?}"
    );
    assert!(direction.iter().all(|entry| entry.abs() <= MEAN_BOUND));
}

#[test]
fn a_missing_or_non_finite_slope_is_no_slope() {
    let direction = direction_from_slopes(&[
        None,
        Some(f32::NAN),
        Some(f32::INFINITY),
        Some(f32::NEG_INFINITY),
        Some(-1.0),
        Some(0.5),
    ]);
    // The four non-slopes are zero and do not take part in the maximum.
    assert_eq!(direction[..4], [0.0; 4]);
    assert!(close(direction[4], MEAN_BOUND, 1e-5), "{direction:?}");
    assert!(
        close(direction[5], -MEAN_BOUND * 0.5, 1e-5),
        "{direction:?}"
    );
    assert!(direction.iter().all(|entry| entry.is_finite()));
}

#[test]
fn without_any_slope_every_entry_is_zero() {
    assert_eq!(direction_from_slopes(&[None, None, None]), [0.0; 3]);
    assert_eq!(
        direction_from_slopes(&[Some(f32::NAN), None, Some(f32::INFINITY)]),
        [0.0; 3]
    );
    assert!(direction_from_slopes(&[]).is_empty());
    // Slopes that are all exactly zero stay at zero (and are not NaN).
    let flat = direction_from_slopes(&[Some(0.0), Some(0.0)]);
    assert!(flat.iter().all(|entry| *entry == 0.0));
}

#[test]
fn noise_level_slopes_are_not_blown_up_to_the_full_step() {
    // The epsilon dominates a design whose slopes are all numerical noise.
    let tiny = direction_from_slopes(&[Some(1e-9), Some(-1e-9)]);
    assert!(tiny.iter().all(|entry| entry.abs() < 1e-2), "{tiny:?}");
    // Huge but finite slopes still give the bound, never NaN.
    let huge = direction_from_slopes(&[Some(1e30), Some(-1e30), Some(5e29)]);
    assert!(close(huge[0], -MEAN_BOUND, 1e-4) && close(huge[1], MEAN_BOUND, 1e-4));
    assert!(huge.iter().all(|entry| entry.is_finite()));
}

#[test]
fn the_action_is_tanh_of_the_direction_times_the_step() {
    let direction = [0.0, MEAN_BOUND, -MEAN_BOUND, 10.0, -0.5];
    let action = action_from_direction(&direction, 0.5);
    assert_eq!(action.deltas_deg.len(), direction.len());
    assert_eq!(action.deltas_deg[0], 0.0);
    // The steepest tier moves 95 % of the step limit, in the direction's sign.
    assert!((action.deltas_deg[1] - 0.475).abs() < 1e-5, "{action:?}");
    assert!((action.deltas_deg[2] + 0.475).abs() < 1e-5, "{action:?}");
    // The squash saturates at the step limit.
    assert!((action.deltas_deg[3] - 0.5).abs() < 1e-6, "{action:?}");
    let expected = f64::from((-0.5_f32).tanh()) * 0.5;
    assert_eq!(action.deltas_deg[4], expected);
    assert!(action_from_direction(&[], 0.5).deltas_deg.is_empty());
}

#[test]
fn the_label_measures_every_free_tier_and_leaves_the_environment_alone() {
    let env = fixture_env();
    let tiers = env.base_design().tiers.len();
    let score = env.current_score();
    let observation = env.build_observation();

    let label = env.teacher_label(H, &DEFAULT_LINE_SEARCH);
    assert_eq!(label.tiers.len(), tiers);
    for (index, entry) in label.tiers.iter().enumerate() {
        if env.movable_tier_indices().contains(&index) {
            let sensitivity = entry.expect("a free tier with a feasible probe");
            assert!(sensitivity.score.is_finite(), "tier {index}");
            assert!(
                sensitivity.components.iter().all(|slope| slope.is_finite()),
                "tier {index}: {sensitivity:?}"
            );
        } else {
            assert!(entry.is_none(), "pinned tier {index}: {entry:?}");
        }
    }
    let free = env.movable_tier_indices().len();
    assert!(label.evaluations >= free, "{}", label.evaluations);
    assert!(label.evaluations <= 2 * free + DEFAULT_LINE_SEARCH.len());

    // Pure: nothing moved, nothing counted, nothing recorded.
    assert_eq!(env.current_score(), score);
    assert_eq!(env.step_count(), 0);
    assert_eq!(env.build_observation(), observation);
    assert!(env.last_components().is_none());
}

#[test]
fn the_score_slope_is_the_gradient_the_trainer_already_used() {
    let env = fixture_env();
    let label = env.teacher_label(H, &DEFAULT_LINE_SEARCH);
    let gradient = env.score_gradient_fd(H);
    assert_eq!(label.tiers.len(), gradient.len());
    for (entry, slope) in label.tiers.iter().zip(&gradient) {
        assert_eq!(entry.map(|sensitivity| sensitivity.score), *slope);
    }
}

#[test]
fn the_component_slopes_are_the_probes_own_differences() {
    let mut env = fixture_env();
    let tiers = env.base_design().tiers.len();
    let label = env.teacher_label(H, &[]);
    let base = env.base_components();
    let h = H as f32;
    let mut checked = 0;
    for tier in env.movable_tier_indices().to_vec() {
        let sensitivity = label.tiers[tier].expect("a free tier");
        if !sensitivity.forward_probe {
            continue;
        }
        // The step of the same one-hot delta scores the very same candidate.
        let _ = env.reset(0);
        let mut deltas = vec![0.0; tiers];
        deltas[tier] = H;
        let _ = env.step(&ContinuousTierAction::new(deltas));
        let probed = env.last_components().expect("the +h candidate scored");
        let [windowing, extinction, brilliance_loss, _] = sensitivity.components;
        assert!(close(
            windowing,
            (probed.windowing_pct - base.windowing_pct) / h,
            1e-3
        ));
        assert!(close(
            extinction,
            (probed.extinction_pct - base.extinction_pct) / h,
            1e-3
        ));
        // The component is the LOSS: a brilliance gain is a negative slope.
        let brilliance_delta = probed.tilt_brilliance_pct - base.tilt_brilliance_pct;
        assert!(
            close(brilliance_loss, -brilliance_delta / h, 1e-3),
            "tier {tier}: {brilliance_loss} vs {}",
            -brilliance_delta / h
        );
        checked += 1;
    }
    assert!(checked > 0, "no free tier had a feasible +h probe");
}

#[test]
fn the_score_slope_is_the_weighted_composition_of_the_component_slopes() {
    for preset in [
        ObjectivePreset::Balanced,
        ObjectivePreset::Brilliance,
        ObjectivePreset::LowWindowing,
        ObjectivePreset::LowExtinction,
        ObjectivePreset::KeepWeight,
    ] {
        let env = fixture_env_with(|config| config.weights = preset.weights());
        let fractions = env.build_observation().global.weight_fractions;
        let label = env.teacher_label(H, &[]);
        for (tier, entry) in label.tiers.iter().enumerate() {
            let Some(sensitivity) = entry else { continue };
            let composed: f32 = fractions
                .iter()
                .zip(&sensitivity.components)
                .map(|(fraction, slope)| fraction * slope)
                .sum();
            assert!(
                close(composed, sensitivity.score, 1e-3),
                "{preset:?} tier {tier}: {composed} vs {}",
                sensitivity.score
            );
        }
    }
}

#[test]
fn the_oracle_step_is_what_step_scaled_does_with_the_oracle_action() {
    let mut env = fixture_env();
    let label = env.teacher_label(H, &DEFAULT_LINE_SEARCH);
    let gain = label.oracle_gain.expect("the fixture has slopes");
    assert!(gain.is_finite() && gain >= 0.0, "{gain}");

    let slopes: Vec<Option<f32>> = label
        .tiers
        .iter()
        .map(|tier| tier.map(|sensitivity| sensitivity.score))
        .collect();
    let action = action_from_direction(&direction_from_slopes(&slopes), env.config().max_step_deg);
    let before = env.current_score();
    let step = env.step_scaled(&action, &DEFAULT_LINE_SEARCH);
    match label.oracle_scale {
        Some(scale) => {
            assert!(gain > 0.0);
            assert!(
                DEFAULT_LINE_SEARCH
                    .iter()
                    .any(|&known| known as f32 == scale)
            );
            assert_eq!(step.info.scale, Some(scale));
            assert_eq!(step.info.score_delta, gain);
            assert!(env.current_score() < before);
        }
        None => {
            assert_eq!(gain, 0.0);
            assert_eq!(step.info.scale, None);
            assert_eq!(env.current_score(), before);
        }
    }
}

#[test]
fn no_scales_means_no_oracle_step() {
    let env = fixture_env();
    let full = env.teacher_label(H, &DEFAULT_LINE_SEARCH);
    for scales in [&[][..], &[f64::NAN, -1.0][..]] {
        let probes_only = env.teacher_label(H, scales);
        assert_eq!(probes_only.oracle_gain, None);
        assert_eq!(probes_only.oracle_scale, None);
        // The probes do not depend on the scales.
        assert_eq!(probes_only.tiers, full.tiers);
        // The oracle candidates are what the full label spent on top of the probes.
        assert!(full.evaluations > probes_only.evaluations);
    }
}

#[test]
fn a_probe_size_that_cannot_be_one_gives_an_empty_label() {
    let env = fixture_env();
    let tiers = env.base_design().tiers.len();
    for h in [0.0, -0.25, f64::NAN, f64::INFINITY] {
        let label = env.teacher_label(h, &DEFAULT_LINE_SEARCH);
        assert_eq!(label.tiers.len(), tiers, "h = {h}");
        assert!(label.tiers.iter().all(Option::is_none), "h = {h}");
        assert_eq!(
            (label.oracle_gain, label.oracle_scale, label.evaluations),
            (None, None, 0),
            "h = {h}"
        );
    }
}

#[test]
fn the_label_is_the_probes_and_the_oracle_assembled() {
    let env = fixture_env();
    let (tiers, probe_evaluations) = probe_all(&env, H);
    let slopes = score_slopes(&tiers);
    let scale_lists: [&[f64]; 4] = [&DEFAULT_LINE_SEARCH, &[1.0], &[], &[f64::NAN, 0.5]];
    for scales in scale_lists {
        let oracle = env.oracle_outcome(&slopes, scales);
        let assembled = assemble_label(tiers.clone(), probe_evaluations, oracle);
        assert_eq!(env.teacher_label(H, scales), assembled, "{scales:?}");
    }
    // The same on a weighted objective, where the yield joins the score.
    let weighted =
        fixture_env_with(|config| config.weights = ObjectivePreset::KeepWeight.weights());
    let (tiers, probe_evaluations) = probe_all(&weighted, H);
    let oracle = weighted.oracle_outcome(&score_slopes(&tiers), &DEFAULT_LINE_SEARCH);
    assert_eq!(
        weighted.teacher_label(H, &DEFAULT_LINE_SEARCH),
        assemble_label(tiers, probe_evaluations, oracle)
    );
}

#[test]
fn a_probe_reports_the_slopes_and_the_evaluations_it_spent() {
    let env = fixture_env();
    let label = env.teacher_label(H, &[]);
    let mut spent = 0;
    for &tier in env.movable_tier_indices() {
        let probe = env.probe_tier(tier, H);
        assert_eq!(probe.sensitivity, label.tiers[tier], "tier {tier}");
        let sensitivity = probe
            .sensitivity
            .expect("a free tier with a feasible probe");
        // One candidate when `+h` was usable; the `-h` fallback adds at most one more.
        assert!(
            (1..=2).contains(&probe.evaluations),
            "tier {tier}: {probe:?}"
        );
        if sensitivity.forward_probe {
            assert_eq!(probe.evaluations, 1, "tier {tier}");
        }
        spent += probe.evaluations;
    }
    // Without an oracle step the label's cost is the probes' cost.
    assert_eq!(spent, label.evaluations);
}

#[test]
fn a_tier_that_cannot_be_probed_costs_nothing() {
    let env = fixture_env();
    let tiers = env.base_design().tiers.len();
    let nothing = TierProbe {
        sensitivity: None,
        evaluations: 0,
    };
    assert_eq!(TierProbe::default(), nothing);

    // Pinned tiers are not probed (the fixture pins its first tier).
    let pinned: Vec<usize> = (0..tiers)
        .filter(|tier| !env.movable_tier_indices().contains(tier))
        .collect();
    assert!(!pinned.is_empty());
    for tier in pinned {
        assert_eq!(env.probe_tier(tier, H), nothing, "pinned tier {tier}");
    }
    // Neither is a tier the design does not have.
    for tier in [tiers, tiers + 7, usize::MAX] {
        assert_eq!(env.probe_tier(tier, H), nothing, "tier {tier}");
    }
    // A probe size that is not finite and positive measures nothing, on any tier.
    for h in [0.0, -0.25, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for tier in 0..tiers {
            assert_eq!(env.probe_tier(tier, h), nothing, "tier {tier}, h = {h}");
        }
    }
}

#[test]
fn probing_and_the_oracle_step_leave_the_environment_alone() {
    let env = fixture_env();
    let score = env.current_score();
    let observation = env.build_observation();

    let (tiers, _) = probe_all(&env, H);
    let _ = env.oracle_outcome(&score_slopes(&tiers), &DEFAULT_LINE_SEARCH);

    assert_eq!(env.current_score(), score);
    assert_eq!(env.step_count(), 0);
    assert_eq!(env.build_observation(), observation);
    assert!(env.last_components().is_none());
}

#[test]
fn the_oracle_outcome_follows_its_slopes_and_its_scales() {
    let env = fixture_env();
    let (tiers, _) = probe_all(&env, H);
    let slopes = score_slopes(&tiers);
    let none = OracleOutcome {
        gain: None,
        scale: None,
        evaluations: 0,
    };
    assert_eq!(OracleOutcome::default(), none);

    // Empty or unusable scales: no oracle step and no candidate spent (a step would try the
    // unit scale; the oracle does not).
    for scales in [&[][..], &[f64::NAN, -1.0, 0.0, f64::INFINITY][..]] {
        assert_eq!(env.oracle_outcome(&slopes, scales), none, "{scales:?}");
    }

    // Slopes that do not belong to this design cannot be followed.
    let longer = [slopes.clone(), vec![Some(-1.0)]].concat();
    let shorter = slopes[..slopes.len() - 1].to_vec();
    for wrong in [longer, shorter, Vec::new()] {
        assert_eq!(
            env.oracle_outcome(&wrong, &DEFAULT_LINE_SEARCH),
            none,
            "{} slopes",
            wrong.len()
        );
    }

    // So cannot slopes without a usable entry.
    for blind in [
        vec![None; slopes.len()],
        vec![Some(f32::NAN); slopes.len()],
        vec![Some(f32::INFINITY); slopes.len()],
    ] {
        assert_eq!(env.oracle_outcome(&blind, &DEFAULT_LINE_SEARCH), none);
    }

    // The fixture has slopes: the outcome is a gain of at least zero and a count of the
    // candidates that reached the scorer.
    let full = env.oracle_outcome(&slopes, &DEFAULT_LINE_SEARCH);
    let gain = full.gain.expect("the fixture has slopes");
    assert!(gain.is_finite() && gain >= 0.0, "{gain}");
    assert!(
        (1..=DEFAULT_LINE_SEARCH.len()).contains(&full.evaluations),
        "{}",
        full.evaluations
    );
    match full.scale {
        Some(scale) => {
            assert!(gain > 0.0);
            assert!(
                DEFAULT_LINE_SEARCH
                    .iter()
                    .any(|&known| known as f32 == scale)
            );
        }
        None => assert_eq!(gain, 0.0),
    }

    // A longer list never spends less than its first scale alone, and it stops at the first
    // accepted scale: when the unit scale improves, the rest is not tried.
    let first = env.oracle_outcome(&slopes, &[1.0]);
    assert!(first.evaluations <= 1);
    assert!(full.evaluations >= first.evaluations);
    if first.scale.is_some() {
        assert_eq!(full, first);
    }
    // Scales that cannot be used are skipped, not counted.
    assert_eq!(env.oracle_outcome(&slopes, &[f64::NAN, 1.0]), first);
}

#[test]
fn assemble_label_joins_the_pieces_and_adds_the_evaluations() {
    let measured = TierSensitivity {
        components: [1.0, -2.0, 0.5, 0.25],
        score: -0.75,
        forward_probe: false,
    };
    let tiers = vec![None, Some(measured), None];
    let oracle = OracleOutcome {
        gain: Some(1.5),
        scale: Some(0.5),
        evaluations: 2,
    };
    assert_eq!(
        assemble_label(tiers.clone(), 4, oracle),
        TeacherLabel {
            tiers: tiers.clone(),
            oracle_gain: Some(1.5),
            oracle_scale: Some(0.5),
            evaluations: 6,
        }
    );

    // No oracle step: the label is the probes alone.
    let probes_only = assemble_label(tiers.clone(), 4, OracleOutcome::default());
    assert_eq!(probes_only.tiers, tiers);
    assert_eq!(
        (
            probes_only.oracle_gain,
            probes_only.oracle_scale,
            probes_only.evaluations
        ),
        (None, None, 4)
    );

    // An oracle step that improved nothing still cost its candidates.
    let stalled = assemble_label(
        tiers,
        3,
        OracleOutcome {
            gain: Some(0.0),
            scale: None,
            evaluations: 3,
        },
    );
    assert_eq!(
        (
            stalled.oracle_gain,
            stalled.oracle_scale,
            stalled.evaluations
        ),
        (Some(0.0), None, 6)
    );

    // Nothing probed, nothing asked.
    assert_eq!(
        assemble_label(Vec::new(), 0, OracleOutcome::default()),
        TeacherLabel {
            tiers: Vec::new(),
            oracle_gain: None,
            oracle_scale: None,
            evaluations: 0,
        }
    );
}

#[test]
fn score_slopes_keeps_one_entry_per_tier() {
    let measured = |score| TierSensitivity {
        components: [0.0; N_COMPONENTS],
        score,
        forward_probe: true,
    };
    let slopes = score_slopes(&[Some(measured(-1.5)), None, Some(measured(0.25))]);
    assert_eq!(slopes, [Some(-1.5), None, Some(0.25)]);
    assert!(score_slopes(&[]).is_empty());
}

#[test]
fn probes_from_several_threads_equal_the_serial_ones() {
    let env = fixture_env();
    let (serial, serial_evaluations) = probe_all(&env, H);
    let free = env.movable_tier_indices().to_vec();

    // One task per tier, as a trainer's pool runs them against a shared `&env`.
    let probes: Vec<(usize, TierProbe)> = std::thread::scope(|scope| {
        let shared = &env;
        // Every task is started before the first one is joined.
        let mut handles = Vec::new();
        for &tier in &free {
            handles.push(scope.spawn(move || (tier, shared.probe_tier(tier, H))));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a probe thread"))
            .collect()
    });

    assert_eq!(probes.len(), free.len());
    let mut evaluations = 0;
    for (tier, probe) in probes {
        assert_eq!(probe.sensitivity, serial[tier], "tier {tier}");
        evaluations += probe.evaluations;
    }
    assert_eq!(evaluations, serial_evaluations);
}

#[test]
fn both_environments_can_be_shared_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RetargetEnv>();
    assert_send_sync::<OptimizerEnv>();
}
