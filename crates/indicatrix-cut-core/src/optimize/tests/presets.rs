//! Tests for [`super::super::ObjectivePreset`]: the weights each named preset stands
//! for, and the text a front end shows.

use super::super::{ObjectiveComponents, ObjectivePreset, ObjectiveWeights};

fn weights(
    windowing: f32,
    extinction: f32,
    tilt_brilliance: f32,
    yield_weight: f32,
) -> ObjectiveWeights {
    ObjectiveWeights {
        windowing,
        extinction,
        tilt_brilliance,
        yield_weight,
        ..ObjectiveWeights::default()
    }
}

#[test]
fn balanced_is_exactly_the_default_weighting() {
    assert_eq!(
        ObjectivePreset::Balanced.weights(),
        ObjectiveWeights::default()
    );
    assert_eq!(ObjectivePreset::default(), ObjectivePreset::Balanced);
}

#[test]
fn every_preset_maps_to_its_documented_weights() {
    assert_eq!(
        ObjectivePreset::Brilliance.weights(),
        weights(1.0, 1.0, 4.0, 0.0)
    );
    assert_eq!(
        ObjectivePreset::LowWindowing.weights(),
        weights(4.0, 1.0, 1.0, 0.0)
    );
    assert_eq!(
        ObjectivePreset::LowExtinction.weights(),
        weights(1.0, 4.0, 1.0, 0.0)
    );
    assert_eq!(
        ObjectivePreset::KeepWeight.weights(),
        weights(1.0, 1.0, 1.0, 3.0)
    );
}

#[test]
fn only_keep_weight_puts_weight_on_yield() {
    for preset in ObjectivePreset::ALL {
        let yield_weight = preset.weights().yield_weight;
        if preset == ObjectivePreset::KeepWeight {
            assert!(yield_weight > 0.0);
        } else {
            assert_eq!(yield_weight, 0.0, "{preset:?} must not weigh the yield");
        }
    }
}

#[test]
fn labels_and_descriptions_are_plain_distinct_text() {
    let mut labels = Vec::new();
    let mut descriptions = Vec::new();
    for preset in ObjectivePreset::ALL {
        let label = preset.label();
        let description = preset.description();
        assert_ne!(label, "");
        assert!(
            description.starts_with(&format!("{label}:")),
            "{description:?} should open with the preset's name"
        );
        assert!(description.ends_with('.'));
        assert!(
            description.is_ascii(),
            "plain ASCII keeps the UI font from drawing boxes"
        );
        labels.push(label);
        descriptions.push(description);
    }
    labels.sort_unstable();
    labels.dedup();
    descriptions.sort_unstable();
    descriptions.dedup();
    assert_eq!(labels.len(), ObjectivePreset::ALL.len());
    assert_eq!(descriptions.len(), ObjectivePreset::ALL.len());
    assert_eq!(
        ObjectivePreset::Balanced.description(),
        "Balanced: brightness, windowing and extinction weighed equally."
    );
}

#[test]
fn index_round_trips_and_an_unknown_index_is_balanced() {
    for (position, preset) in ObjectivePreset::ALL.into_iter().enumerate() {
        assert_eq!(preset.index(), position);
        assert_eq!(ObjectivePreset::from_index(position), preset);
    }
    assert_eq!(ObjectivePreset::from_index(99), ObjectivePreset::Balanced);
}

#[test]
fn matching_finds_the_preset_a_set_of_weights_belongs_to() {
    for preset in ObjectivePreset::ALL {
        assert_eq!(ObjectivePreset::matching(&preset.weights()), Some(preset));
    }
    assert_eq!(
        ObjectivePreset::matching(&weights(2.0, 1.0, 1.0, 0.0)),
        None
    );
}

/// A preset changes what wins: the same two designs, one with more windowing and one
/// with more extinction, swap places between the two matching presets.
#[test]
fn the_presets_rank_designs_by_what_they_favour() {
    let windowy = ObjectiveComponents {
        windowing_pct: 30.0,
        extinction_pct: 10.0,
        tilt_brilliance_pct: 60.0,
    };
    let dark = ObjectiveComponents {
        windowing_pct: 10.0,
        extinction_pct: 30.0,
        tilt_brilliance_pct: 60.0,
    };
    let low_windowing = ObjectivePreset::LowWindowing.weights();
    let low_extinction = ObjectivePreset::LowExtinction.weights();
    assert!(low_windowing.score(&dark) < low_windowing.score(&windowy));
    assert!(low_extinction.score(&windowy) < low_extinction.score(&dark));
    // Balanced cannot tell them apart.
    let balanced = ObjectivePreset::Balanced.weights();
    assert!((balanced.score(&windowy) - balanced.score(&dark)).abs() < 1e-4);
}

#[test]
fn keep_weight_prefers_the_design_that_keeps_more_of_the_rough() {
    let components = ObjectiveComponents {
        windowing_pct: 20.0,
        extinction_pct: 20.0,
        tilt_brilliance_pct: 60.0,
    };
    let keep = ObjectivePreset::KeepWeight.weights();
    assert!(keep.score_with_yield(&components, 40.0) < keep.score_with_yield(&components, 60.0));
    let balanced = ObjectivePreset::Balanced.weights();
    assert_eq!(
        balanced.score_with_yield(&components, 40.0),
        balanced.score_with_yield(&components, 60.0),
        "the balanced preset ignores the yield"
    );
}
