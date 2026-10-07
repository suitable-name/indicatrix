//! Tests for the face-up tone objective: [`super::super::ToneGoal`], the two tone presets,
//! and the tone every `Full` scoring reports.

use super::{
    super::{
        FaceUpTone, ObjectivePreset, ObjectiveWeights, OptimizeConfig, OptimizeOptions,
        OptimizeResult, SearchHooks, ToneGoal, optimize_design_with,
    },
    fixtures::rbc_445,
};
use indicatrix::optics::{
    materials::{GemMaterial, body_color::BODY_COLOR_PRESETS},
    raytracer::LightingPreset,
};

fn tone(l_star: f32, chroma: f32) -> FaceUpTone {
    FaceUpTone {
        l_star,
        chroma,
        ..FaceUpTone::NONE
    }
}

fn blue_quartz() -> GemMaterial {
    GemMaterial::by_name("Quartz")
        .expect("Quartz resolves")
        .with_body_color(BODY_COLOR_PRESETS[1].absorption_rgb)
}

fn config(weights: ObjectiveWeights, max_evaluations: usize) -> OptimizeConfig {
    OptimizeConfig {
        weights,
        seed: 0,
        max_evaluations,
        ..OptimizeConfig::default()
    }
}

fn run(material: &GemMaterial, config: &OptimizeConfig) -> OptimizeResult {
    optimize_design_with(
        &rbc_445(),
        material,
        config,
        &OptimizeOptions::default(),
        &SearchHooks::default(),
    )
    .expect("RBC-445 must solve")
}

fn bits(tone: &FaceUpTone) -> [u32; 3] {
    [
        tone.l_star.to_bits(),
        tone.chroma.to_bits(),
        tone.mean_path_units.to_bits(),
    ]
}

#[test]
fn a_tone_weight_of_zero_changes_nothing() {
    let weights = ObjectiveWeights::default();
    assert_eq!(weights.tone_weight, 0.0);
    let material = GemMaterial::sapphire().with_body_color(BODY_COLOR_PRESETS[1].absorption_rgb);
    let result = run(&material, &config(weights, 40));
    let outcome = &result.outcome;
    assert_eq!(
        outcome.before_score.to_bits(),
        weights
            .score_with_yield(&outcome.before, outcome.before_yield_loss_pct)
            .to_bits()
    );
    for candidate in &result.candidates {
        assert_eq!(
            candidate.score.to_bits(),
            weights
                .score_with_yield(&candidate.after, candidate.yield_loss_pct)
                .to_bits(),
            "an unweighted tone must not touch a candidate's score"
        );
        assert!(candidate.tone.is_some(), "the tone is always reported");
    }
    assert_eq!(result.tone_goal, None);
    assert!(result.tone_before.is_some(), "the tone is always reported");
}

#[test]
fn the_losses_have_the_right_polarity() {
    let lighter = |l_star: f32| ToneGoal::Lighter.loss_pct(&tone(l_star, 10.0));
    assert!(lighter(80.0) < lighter(60.0));
    assert!(lighter(60.0) < lighter(20.0));
    let deeper = |chroma: f32| ToneGoal::Deeper.loss_pct(&tone(50.0, chroma));
    assert!(deeper(60.0) < deeper(30.0));
    assert!(deeper(30.0) < deeper(5.0));
    assert_eq!(deeper(100.0).to_bits(), deeper(140.0).to_bits());
    assert_eq!(deeper(140.0), 0.0);
    assert_eq!(ToneGoal::default(), ToneGoal::Lighter);
    assert_eq!(ToneGoal::Lighter.label(), "lighter");
    assert_eq!(ToneGoal::Deeper.label(), "deeper");
}

#[test]
fn the_tone_gate_rejects_a_move_against_the_goal() {
    let start = tone(50.0, 40.0);
    // Deeper: chroma must not drop; lightness is irrelevant.
    assert!(ToneGoal::Deeper.not_worse(&start, &tone(20.0, 40.0)));
    assert!(ToneGoal::Deeper.not_worse(&start, &tone(50.0, 45.0)));
    assert!(!ToneGoal::Deeper.not_worse(&start, &tone(50.0, 39.0)));
    // Lighter: L* must not drop; chroma is irrelevant.
    assert!(ToneGoal::Lighter.not_worse(&start, &tone(50.0, 5.0)));
    assert!(ToneGoal::Lighter.not_worse(&start, &tone(60.0, 40.0)));
    assert!(!ToneGoal::Lighter.not_worse(&start, &tone(49.0, 40.0)));
    // NaN never passes.
    assert!(!ToneGoal::Deeper.not_worse(&start, &tone(50.0, f32::NAN)));
}

#[test]
fn score_with_tone_blends_the_fifth_term() {
    let components = super::super::ObjectiveComponents {
        windowing_pct: 10.0,
        extinction_pct: 20.0,
        tilt_brilliance_pct: 70.0,
    };
    let off = ObjectiveWeights::default();
    assert_eq!(
        off.score_with_tone(&components, 15.0, 80.0).to_bits(),
        off.score_with_yield(&components, 15.0).to_bits()
    );
    let on = ObjectiveWeights {
        tone_weight: 3.0,
        ..off
    };
    let low = on.score_with_tone(&components, 15.0, 10.0);
    let high = on.score_with_tone(&components, 15.0, 90.0);
    assert!(low < high);
}

#[test]
fn lighten_dark_rough_ends_no_darker_than_it_starts() {
    let dark = blue_quartz().with_absorption_path_scale(3.0);
    let weights = ObjectivePreset::LightenDark.weights();
    let result = run(&dark, &config(weights, 80));
    assert_eq!(result.tone_goal, Some(ToneGoal::Lighter));
    let before = result.tone_before.expect("the start is toned");
    if let Some(best) = result.best_candidate() {
        let after = best.tone.expect("a candidate is toned");
        assert!(
            after.l_star >= before.l_star - 1e-3,
            "L* went from {} to {}",
            before.l_star,
            after.l_star
        );
    }
}

#[test]
fn intensify_pale_rough_ends_no_paler_than_it_starts() {
    let [r, g, b] = BODY_COLOR_PRESETS[1].absorption_rgb;
    let pale = GemMaterial::by_name("Quartz")
        .expect("Quartz resolves")
        .with_body_color([r * 0.15, g * 0.15, b * 0.15]);
    let weights = ObjectivePreset::IntensifyPale.weights();
    let result = run(&pale, &config(weights, 80));
    assert_eq!(result.tone_goal, Some(ToneGoal::Deeper));
    let before = result.tone_before.expect("the start is toned");
    if let Some(best) = result.best_candidate() {
        let after = best.tone.expect("a candidate is toned");
        assert!(
            after.chroma >= before.chroma - 1e-3,
            "C* went from {} to {}",
            before.chroma,
            after.chroma
        );
    }
}

#[test]
fn presets_round_trip_and_match() {
    assert_eq!(ObjectivePreset::ALL.len(), 7);
    assert_eq!(ObjectivePreset::from_index(5), ObjectivePreset::LightenDark);
    assert_eq!(
        ObjectivePreset::from_index(6),
        ObjectivePreset::IntensifyPale
    );
    assert_eq!(ObjectivePreset::LightenDark.index(), 5);
    assert_eq!(ObjectivePreset::IntensifyPale.index(), 6);
    assert_eq!(
        ObjectivePreset::matching(&ObjectivePreset::LightenDark.weights()),
        Some(ObjectivePreset::LightenDark)
    );
    assert_eq!(
        ObjectivePreset::matching(&ObjectivePreset::IntensifyPale.weights()),
        Some(ObjectivePreset::IntensifyPale)
    );
    assert_ne!(
        ObjectivePreset::LightenDark.label(),
        ObjectivePreset::IntensifyPale.label()
    );
    assert_ne!(
        ObjectivePreset::LightenDark.description(),
        ObjectivePreset::IntensifyPale.description()
    );
    let lighten = ObjectivePreset::LightenDark.weights();
    assert_eq!(
        (lighten.tone_weight, lighten.tone_goal),
        (3.0, ToneGoal::Lighter)
    );
    let intensify = ObjectivePreset::IntensifyPale.weights();
    assert_eq!(
        (intensify.tone_weight, intensify.tone_goal),
        (3.0, ToneGoal::Deeper)
    );
}

#[test]
fn the_tone_follows_the_runs_lighting_preset() {
    let material = blue_quartz();
    let measure = |material: &GemMaterial, lighting: LightingPreset| {
        let config = OptimizeConfig {
            lighting,
            polish_start_step_deg: None,
            ..config(ObjectiveWeights::default(), 0)
        };
        run(material, &config)
    };
    let daylight = measure(&material, LightingPreset::Daylight);
    let incandescent = measure(&material, LightingPreset::Incandescent);
    assert_eq!(daylight.lighting, LightingPreset::Daylight);
    assert_eq!(incandescent.lighting, LightingPreset::Incandescent);
    let (d, i) = (
        daylight.tone_before.expect("toned"),
        incandescent.tone_before.expect("toned"),
    );
    assert_ne!(
        (d.l_star.to_bits(), d.chroma.to_bits()),
        (i.l_star.to_bits(), i.chroma.to_bits()),
        "a coloured stone is toned under the preset's own light"
    );
    let again = measure(&material, LightingPreset::Daylight);
    assert_eq!(bits(&again.tone_before.expect("toned")), bits(&d));

    for lighting in [LightingPreset::Daylight, LightingPreset::Incandescent] {
        let clear = measure(&GemMaterial::diamond(), lighting)
            .tone_before
            .expect("toned");
        assert!(clear.l_star > 99.5, "{lighting:?}: L* {}", clear.l_star);
        assert!(clear.chroma < 0.5, "{lighting:?}: C* {}", clear.chroma);
    }
}
