//! Tests for the per-preset white-balance cache.

use super::*;

/// The `OnceLock`-per-preset cache must not change the visible value: asserts the
/// cached value for every preset matches the expectation derived from the preset's
/// illuminant: the identity wherever the preset has no white balance
/// (`!uses_white_balance`: D65 presets and UV lamps), otherwise a fresh, uncached Bradford adaptation of the blackbody.
#[test]
fn illuminant_white_balance_matches_direct_computation_for_all_presets() {
    for preset in LightingPreset::ALL {
        let cached = illuminant_white_balance(preset);
        let direct = if preset.uses_white_balance() {
            compute_illuminant_white_balance(illuminant_temperature_k(preset))
        } else {
            Vec3::ONE
        };
        assert!(
            (cached - direct).length() < 1e-5,
            "cached white balance for {preset:?} should match the expected value (cached={cached:?}, direct={direct:?})"
        );
    }
}

/// The D65 white point must stay neutral through the Daylight preset's white
/// balance: a grey at the sRGB white chromaticity keeps its xy to within 0.001.
#[test]
fn daylight_white_balance_keeps_a_d65_grey_neutral() {
    let grey = Vec3::new(D65_WHITE_X, D65_WHITE_Y, 1.0 - D65_WHITE_X - D65_WHITE_Y);
    let scale = illuminant_white_balance(LightingPreset::Daylight);
    let balanced = apply_von_kries_white_balance(grey, scale);
    let sum = balanced.x + balanced.y + balanced.z;
    let (x, y) = (balanced.x / sum, balanced.y / sum);
    assert!(
        (x - 0.3127).abs() < 0.001 && (y - 0.3290).abs() < 0.001,
        "D65 grey drifted to xy=({x:.4}, {y:.4}) after the Daylight white balance"
    );
}

/// Every unrecognized preset label falls back to `LightingPreset::default()` (the light
/// tent) and so shares its white balance, while the legacy, mislabelled
/// `"D65 Daylight (5500K)"` string an older settings file may still contain maps
/// explicitly to `Daylight` (identity white balance).
#[test]
fn illuminant_white_balance_default_arm_is_shared() {
    let a = illuminant_white_balance(LightingPreset::from_label("Totally Unknown Preset A"));
    let b = illuminant_white_balance(LightingPreset::from_label("Totally Unknown Preset B"));
    let default = illuminant_white_balance(LightingPreset::default());
    let legacy = illuminant_white_balance(LightingPreset::from_label("D65 Daylight (5500K)"));
    let daylight = illuminant_white_balance(LightingPreset::Daylight);
    assert!(
        (a - b).length() < 1e-6,
        "distinct unrecognized presets must share the default white balance"
    );
    assert!(
        (a - default).length() < 1e-6,
        "unrecognized labels must give the default preset's white balance"
    );
    assert!(
        (legacy - daylight).length() < 1e-6,
        "the legacy 5500K label must map to Daylight's white balance"
    );
    assert!(
        (legacy - Vec3::ONE).length() < 1e-5,
        "Daylight's white balance is the identity"
    );
}

/// Confirms the lock-free `OnceLock`-per-preset statics are race-free: many
/// threads racing to initialize the same preset's `OnceLock` must all observe the
/// identical value.
#[test]
fn illuminant_white_balance_is_stable_across_concurrent_threads() {
    let presets = LightingPreset::ALL;

    let handles: Vec<_> = (0..32)
        .map(|i| {
            std::thread::spawn(move || {
                let preset = presets[i % presets.len()];
                (preset, illuminant_white_balance(preset))
            })
        })
        .collect();

    let mut by_preset: std::collections::HashMap<LightingPreset, Vec3> =
        std::collections::HashMap::new();
    for h in handles {
        let (preset, v) = h.join().unwrap();
        if let Some(existing) = by_preset.get(&preset) {
            assert!(
                (*existing - v).length() < 1e-6,
                "value for {preset:?} differs across threads"
            );
        } else {
            by_preset.insert(preset, v);
        }
    }
}
