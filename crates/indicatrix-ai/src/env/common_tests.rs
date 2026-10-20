use super::*;
use crate::env::features::absorption_summary;
use indicatrix_cut_core::ObjectivePreset;

fn fixture_settings() -> CoreSettings {
    CoreSettings {
        max_steps: 60,
        max_step_deg: 0.5,
        weights: ObjectiveWeights::default(),
        fidelity: ObjectiveFidelity::Fast,
        terminal_score: DEFAULT_TERMINAL_SCORE,
        min_girdle_ratio: DEFAULT_MIN_GIRDLE_RATIO,
        vary_anchored: true,
        task_kind: 0.0,
        lighting: LightingPreset::IsoHemisphere,
        shape_target: None,
        probe_scale_deg: 0.25,
        only_tiers: None,
        angle_bounds: std::collections::BTreeMap::new(),
    }
}

#[test]
fn source_material_reads_the_design_refractive_index() {
    let material = design_source_material(&Design::concave_fixture());
    assert!((material.n_d - 1.54).abs() < 1e-9, "{}", material.n_d);
    assert!((material.critical_angle_deg - critical_angle_deg(1.54)).abs() < 1e-9);
}

#[test]
fn absorption_feature_is_invariant_to_splitting_an_identical_band() {
    let mut gem = design_source_material(&Design::concave_fixture()).gem;
    let band = indicatrix::optics::absorption::AbsorptionBand::new(550.0, 20.0, 0.8);
    gem.absorption.o_ray = vec![band];
    gem.absorption.e_ray.clear();
    let one = absorption_summary(&gem, 550.0);
    gem.absorption.o_ray = vec![
        indicatrix::optics::absorption::AbsorptionBand::new(550.0, 20.0, 0.4),
        indicatrix::optics::absorption::AbsorptionBand::new(550.0, 20.0, 0.4),
    ];
    let split = absorption_summary(&gem, 550.0);
    assert!((one - split).abs() < 1e-6, "{one} != {split}");
}

#[test]
fn source_material_of_a_design_without_a_material_takes_the_i_line_index() {
    // Every `.asc` import carries `MaterialSelection::none()`: the index comes from the
    // design metadata, as a flat (dispersion-free) material.
    let mut design = Design::concave_fixture();
    design.material = MaterialSelection::none();
    let material = design_source_material(&design);
    assert!((material.n_d - 1.54).abs() < 1e-9, "{}", material.n_d);
    assert!((material.critical_angle_deg - critical_angle_deg(1.54)).abs() < 1e-9);
    let at_d = f64::from(material.gem.dispersion.evaluate(589.3));
    assert!((at_d - 1.54).abs() < 1e-4, "{at_d}");
    let spread = material.gem.dispersion.evaluate(486.1) - material.gem.dispersion.evaluate(656.3);
    assert!(
        spread.abs() < 1e-6,
        "flat index has no dispersion: {spread}"
    );
}

#[test]
fn the_fixture_returns_light_so_the_core_accepts_it() {
    let design = Design::concave_fixture();
    let material = design_source_material(&design);
    let core = EnvCore::new(
        design,
        material.gem,
        fixture_settings(),
        RewardConfig::default(),
    )
    .expect("the fixture returns light face-up");
    assert!(core.base_components().tilt_brilliance_pct > 0.0);
    assert_eq!(core.weights(), &ObjectiveWeights::default());
}

#[test]
fn girdle_gate_rules() {
    assert!(girdle_ok(None, None, 0.5));
    assert!(!girdle_ok(Some(1.0), None, 0.5));
    assert!(girdle_ok(Some(1.0), Some(0.6), 0.5));
    assert!(!girdle_ok(Some(1.0), Some(0.4), 0.5));
    assert!(!girdle_ok(Some(1.0), Some(0.0), 0.0));
}

#[test]
fn tone_objectives_are_scored_at_construction() {
    let design = Design::concave_fixture();
    let material = design_source_material(&design);
    for preset in [ObjectivePreset::LightenDark, ObjectivePreset::IntensifyPale] {
        let settings = CoreSettings {
            weights: preset.weights(),
            ..fixture_settings()
        };
        let accepted = EnvCore::new(
            design.clone(),
            material.gem.clone(),
            settings,
            RewardConfig::default(),
        );
        assert!(accepted.is_ok(), "{preset:?}");
    }
    // Every other preset builds.
    let settings = CoreSettings {
        weights: ObjectivePreset::KeepWeight.weights(),
        ..fixture_settings()
    };
    assert!(
        EnvCore::new(design, material.gem, settings, RewardConfig::default()).is_ok(),
        "a yield weight is supported"
    );
}

#[test]
fn the_score_carries_the_yield_only_when_it_is_weighted_and_measurable() {
    let design = Design::concave_fixture();
    let gem = design_source_material(&design).gem;
    let volume = preform_volume(&design);
    assert!(volume.is_some(), "the fixture's block preform measures");
    let evaluate = |weights: &ObjectiveWeights, volume| {
        evaluate_design(
            &design,
            &gem,
            weights,
            ObjectiveFidelity::Fast,
            volume,
            LightingPreset::IsoHemisphere,
            None,
        )
        .expect("the fixture scores")
    };

    // Weight zero: the optical score, bit for bit, whatever the yield is.
    let optical_weights = ObjectiveWeights::default();
    let optical = evaluate(&optical_weights, volume);
    assert_eq!(optical.score, optical_weights.score(&optical.components));
    assert!(optical.yield_pct.is_some());

    // A positive weight and a measurable yield: the yield loss joins the score.
    let weights = ObjectivePreset::KeepWeight.weights();
    let weighted = evaluate(&weights, volume);
    let yield_pct = weighted.yield_pct.expect("measurable");
    assert!(yield_pct > 0.0 && yield_pct.is_finite());
    assert_eq!(
        weighted.score,
        weights.score_with_yield(&weighted.components, 100.0 - yield_pct)
    );
    assert_ne!(weighted.score, weights.score(&weighted.components));
    // The optical measurements do not depend on the weights.
    assert_eq!(weighted.components, optical.components);

    // No preform volume: the yield cannot be measured and the optics score alone.
    let blind = evaluate(&weights, None);
    assert_eq!(blind.yield_pct, None);
    assert_eq!(blind.score, weights.score(&blind.components));
}

#[test]
fn objective_score_falls_back_to_the_optics() {
    let components = ObjectiveComponents {
        windowing_pct: 20.0,
        extinction_pct: 30.0,
        tilt_brilliance_pct: 70.0,
    };
    let weights = ObjectivePreset::KeepWeight.weights();
    assert_eq!(
        objective_score(&weights, &components, None),
        weights.score(&components)
    );
    assert_eq!(
        objective_score(&weights, &components, Some(40.0)),
        weights.score_with_yield(&components, 60.0)
    );
    let plain = ObjectiveWeights::default();
    assert_eq!(
        objective_score(&plain, &components, Some(40.0)),
        plain.score(&components)
    );
}
