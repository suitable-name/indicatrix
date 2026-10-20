//! The lit backdrop tray: every escape below the horizon sees the same surface, the tray's
//! radiance is the albedo times the rig's irradiance over pi, and As Lit (`tray = 0`) never
//! moves (the bit guard is `tests_pins.rs`).

use super::*;
use crate::optics::studio_rig::StudioRig;
use crate::render_setup::Backdrop;

const LAMBDAS: [f32; 3] = [450.0, 550.0, 650.0];

/// Unit directions straight down, inside the horizon band, and well above it.
fn probe_dirs() -> Vec<Vec3> {
    let mut out = Vec::new();
    for y in [
        -0.98f32, -0.7, -0.35, -0.1, -0.049, 0.0, 0.049, 0.1, 0.35, 0.7, 0.98,
    ] {
        let radius = y.mul_add(-y, 1.0).sqrt();
        for (hx, hz) in [(1.0f32, 0.0f32), (0.6, 0.8), (-0.8, -0.6)] {
            out.push(Vec3::new(hx * radius, y, hz * radius));
        }
    }
    out
}

/// `tray = 0.0` is the unsuffixed entry point, bit for bit, for every preset.
#[test]
fn a_zero_tray_is_the_unsuffixed_entry_point() {
    let rig = StudioRig::new(0.84, 0.94);
    let observer = Vec3::new(0.1, 0.95, 0.3).normalize();
    for preset in LightingPreset::ALL {
        for dir in probe_dirs() {
            for lambda in LAMBDAS {
                let plain = sample_studio_environment_with_rig_shadow(
                    dir,
                    lambda,
                    preset,
                    1.3,
                    &rig,
                    observer,
                    DEFAULT_HEAD_SHADOW_COSINES,
                );
                let tray = sample_studio_environment_with_rig_shadow_tray(
                    dir,
                    lambda,
                    preset,
                    1.3,
                    &rig,
                    observer,
                    DEFAULT_HEAD_SHADOW_COSINES,
                    0.0,
                );
                assert_eq!(plain.to_bits(), tray.to_bits(), "{preset:?}");
                let spec = studio_spec_power(preset, lambda);
                let a = sample_studio_environment_with_rig_spec_power(
                    dir, preset, 1.3, &rig, observer, spec,
                );
                let b = sample_studio_environment_with_rig_spec_power_tray(
                    dir, preset, 1.3, &rig, observer, spec, 0.0,
                );
                assert_eq!(a.to_bits(), b.to_bits(), "{preset:?}");
                let shadow = head_shadow_cosines(12.0);
                let c = sample_studio_environment_with_rig_spec_power_shadow(
                    dir, preset, 1.3, &rig, observer, spec, shadow,
                );
                let d = sample_studio_environment_with_rig_spec_power_shadow_tray(
                    dir, preset, 1.3, &rig, observer, spec, shadow, 0.0,
                );
                assert_eq!(c.to_bits(), d.to_bits(), "{preset:?}");
            }
        }
    }
}

/// The tracer's per-escape lookup (what a primary miss and an exit at any bounce both call)
/// returns the tray entry point's value for every preset and direction.
#[test]
fn the_tray_is_seen_by_every_bounce() {
    let rig = StudioRig::new(0.4, 0.35);
    let observer = Vec3::new(0.1, 0.95, 0.3).normalize();
    for preset in LightingPreset::ALL {
        let source = preset.studio(1.2, 0.4, 0.35).with_backdrop(0.5);
        for dir in probe_dirs() {
            let channels = sample_environment_channels(source, dir, &LAMBDAS, Some(&rig), observer);
            for (&lambda, &value) in LAMBDAS.iter().zip(&channels) {
                let single = sample_environment_channel(source, dir, lambda, Some(&rig), observer);
                let direct = sample_studio_environment_with_rig_shadow_tray(
                    dir,
                    lambda,
                    preset,
                    1.2,
                    &rig,
                    observer,
                    DEFAULT_HEAD_SHADOW_COSINES,
                    0.5,
                );
                assert_eq!(value.to_bits(), single.to_bits(), "{preset:?} {dir:?}");
                assert_eq!(value.to_bits(), direct.to_bits(), "{preset:?} {dir:?}");
            }
        }
        // Straight down the tray is seen where As Lit saw the preset's own ground.
        let down = -Vec3::Y;
        let with_tray = sample_environment_channels(source, down, &LAMBDAS, Some(&rig), observer);
        let as_lit = sample_environment_channels(
            source.with_backdrop(0.0),
            down,
            &LAMBDAS,
            Some(&rig),
            observer,
        );
        // Not "brighter": White Tray's own ground (0.8) is brighter than this 0.5 tray, which
        // still replaces it. Straight down the horizon blend is exactly 0 (`d.y = -1` is far
        // below the -0.05 edge) and the observer (dot about -0.95, outside the head-shadow
        // cone) leaves the visibility at 1, so every arm reduces to `tray * exposure * S(550)`
        // with S the preset's illuminant power (D65 for ASET, whose tray is a neutral D65 one):
        // 0.5 * 1.2 * S. Only f32 rounding of two products remains (about 2e-7 relative), so
        // the tolerance is 1e-5 relative (the same as `the_tray_replaces_the_ground_of_every_
        // model`), plus an absolute 1e-9 so a UV lamp, whose line is exactly 0 at 550 nm, is
        // held to 0 as well. A dropped exposure (1.2x) or tray (to the own ground) fails by far.
        let s = if preset == LightingPreset::Aset {
            d65_relative_spectral_power(550.0)
        } else {
            preset.spectral_power(550.0)
        };
        let expected = 0.5 * 1.2 * s;
        assert!(
            (with_tray[1] - expected).abs() <= expected.mul_add(1e-5, 1e-9),
            "{preset:?}: {} vs {expected}",
            with_tray[1]
        );
        if !preset.is_uv_lamp() && preset != LightingPreset::WhiteTray {
            assert_ne!(
                with_tray[1].to_bits(),
                as_lit[1].to_bits(),
                "{preset:?}: the tray must replace the own ground"
            );
        }
    }
}

#[test]
fn iso_hemisphere_is_one_above_and_the_tray_below() {
    let tray = 0.5;
    let spd = d65_relative_spectral_power(550.0);
    let rig = StudioRig::new(0.0, 0.0);
    let sample = |dir: Vec3| {
        sample_studio_environment_with_rig_shadow_tray(
            dir,
            550.0,
            LightingPreset::IsoHemisphere,
            1.0,
            &rig,
            Vec3::ZERO,
            DEFAULT_HEAD_SHADOW_COSINES,
            tray,
        )
    };
    assert!((sample(Vec3::Y) - spd).abs() < 1e-5);
    assert!(tray.mul_add(-spd, sample(-Vec3::Y)).abs() < 1e-5);
    // On the horizon the blend is one half: half the sky plus half the tray.
    assert!(1.0_f32.midpoint(tray).mul_add(-spd, sample(Vec3::X)).abs() < 1e-5);
    // The tray is lit: it scales with the exposure.
    let double = sample_studio_environment_with_rig_shadow_tray(
        -Vec3::Y,
        550.0,
        LightingPreset::IsoHemisphere,
        2.0,
        &rig,
        Vec3::ZERO,
        DEFAULT_HEAD_SHADOW_COSINES,
        tray,
    );
    assert!((2.0 * tray).mul_add(-spd, double).abs() < 1e-5);
}

/// The Studio rigs and the tent family: below the horizon the tray replaces everything.
#[test]
fn the_tray_replaces_the_ground_of_every_model() {
    let rig = StudioRig::new(0.4, 0.35);
    let down = -Vec3::Y;
    let tray = 0.7;
    for preset in LightingPreset::ALL {
        let expected = if preset == LightingPreset::Aset {
            tray * d65_relative_spectral_power(550.0)
        } else {
            tray * preset.spectral_power(550.0)
        };
        let got = sample_studio_environment_with_rig_shadow_tray(
            down,
            550.0,
            preset,
            1.0,
            &rig,
            Vec3::ZERO,
            DEFAULT_HEAD_SHADOW_COSINES,
            tray,
        );
        assert!(
            (got - expected).abs() <= expected * 1e-5,
            "{preset:?}: {got} vs {expected}"
        );
    }
}

/// An observer cone that dips below the horizon darkens the lit models' tray there and leaves
/// the Studio rigs alone (they ignore the observer).
#[test]
fn the_head_shadow_darkens_the_tray_of_the_lit_models_only() {
    let rig = StudioRig::new(0.4, 0.35);
    let observer = Vec3::new(0.0, -0.4, 1.0).normalize();
    let sample = |preset: LightingPreset| {
        sample_studio_environment_with_rig_shadow_tray(
            observer,
            550.0,
            preset,
            1.0,
            &rig,
            observer,
            DEFAULT_HEAD_SHADOW_COSINES,
            0.5,
        )
    };
    for preset in [
        LightingPreset::IsoHemisphere,
        LightingPreset::LightTent,
        LightingPreset::DaylightDome,
        LightingPreset::DaylightSun,
    ] {
        assert_eq!(sample(preset), 0.0, "{preset:?}");
    }
    assert!(sample(LightingPreset::RingLights) > 0.0);
}

#[test]
fn tray_resolve_matches_closed_forms() {
    use std::f32::consts::PI as PI32;
    let (yaw, pitch) = (0.84f32, 72.0f32.to_radians());
    let key_y = StudioRig::new(yaw, pitch).key_dir.y;

    let iso = rig_irradiance_over_pi(LightingPreset::IsoHemisphere, yaw, pitch);
    assert!((iso - 0.9995).abs() < 1e-3, "iso {iso}");

    let dome = rig_irradiance_over_pi(LightingPreset::DaylightDome, yaw, pitch);
    let expected = 0.188_50f32.mul_add(key_y, 0.397_94) / PI32;
    assert!(
        (dome / expected - 1.0).abs() < 0.01,
        "dome {dome} vs {expected}"
    );

    // The sun adds L * Omega * key.y exactly (the disc is above the horizon blend here).
    let sun = rig_irradiance_over_pi(LightingPreset::DaylightSun, yaw, pitch);
    let added = (sun - dome) * PI32;
    let expected_sun = sun::SUN_RADIANCE * sun::SUN_SOLID_ANGLE * key_y;
    assert!(
        (added / expected_sun - 1.0).abs() < 1e-4,
        "sun {added} vs {expected_sun}"
    );

    // The white tray's walls are 0.9 flat: its own tray reproduces its ground within 2 %.
    let white = rig_irradiance_over_pi(LightingPreset::WhiteTray, yaw, pitch);
    assert!((white - 0.9).abs() < 0.02 * 0.9, "white tray {white}");

    assert_eq!(
        rig_irradiance_over_pi(LightingPreset::Aset, yaw, pitch).to_bits(),
        1.0f32.to_bits()
    );
}

#[test]
fn tray_radiance_is_albedo_times_irradiance_and_zero_for_as_lit() {
    let (yaw, pitch) = (0.84f32, 0.94f32);
    for preset in LightingPreset::ALL {
        let e = rig_irradiance_over_pi(preset, yaw, pitch);
        assert!(e.is_finite() && e > 0.0, "{preset:?}: {e}");
        assert_eq!(
            tray_radiance(TRAY_ALBEDO_WHITE, preset, yaw, pitch).to_bits(),
            (TRAY_ALBEDO_WHITE * e).to_bits()
        );
        // Cached: the same inputs give the same bits.
        assert_eq!(
            rig_irradiance_over_pi(preset, yaw, pitch).to_bits(),
            e.to_bits()
        );
    }
    assert_eq!(
        tray_radiance(0.0, LightingPreset::LightTent, yaw, pitch).to_bits(),
        0.0f32.to_bits()
    );
}

/// Dragging the light by a degree must not make the surround flicker.
#[test]
fn tray_resolve_does_not_wobble() {
    for preset in LightingPreset::ALL {
        for pitch_deg in [30.0f32, 54.0, 72.0] {
            let pitch = pitch_deg.to_radians();
            let a = rig_irradiance_over_pi(preset, 0.8, pitch);
            let b = rig_irradiance_over_pi(preset, 0.8 + 1.0f32.to_radians(), pitch);
            assert!(
                (a - b).abs() / a < 0.005,
                "{preset:?} at pitch {pitch_deg}: {a} vs {b}"
            );
        }
    }
}

#[test]
fn backdrop_levels_indices_and_default() {
    const ALL: [Backdrop; 4] = [
        Backdrop::AsLit,
        Backdrop::Grey,
        Backdrop::White,
        Backdrop::Black,
    ];
    assert_eq!(Backdrop::default(), Backdrop::White);
    assert_eq!(ALL.map(Backdrop::index), [0, 1, 2, 3]);
    assert_eq!(
        ALL.map(|b| b.albedo().to_bits()),
        [0.0f32, 0.18, 0.90, 0.02].map(f32::to_bits)
    );
    for b in ALL {
        assert_eq!(Backdrop::from_index(b.index()), b);
    }
    assert_eq!(Backdrop::from_index(-1), Backdrop::White);
    assert_eq!(Backdrop::from_index(99), Backdrop::White);
    let preset = LightingPreset::IsoHemisphere;
    assert_eq!(Backdrop::AsLit.resolve(preset, 0.8, 1.2).to_bits(), 0);
    assert_eq!(
        Backdrop::White.resolve(preset, 0.8, 1.2).to_bits(),
        tray_radiance(TRAY_ALBEDO_WHITE, preset, 0.8, 1.2).to_bits()
    );
    // The temporary shim resolves the default preset at the default pose.
    assert_eq!(
        Backdrop::Grey.level().to_bits(),
        Backdrop::Grey
            .resolve(
                LightingPreset::default(),
                48.0f32.to_radians(),
                72.0f32.to_radians()
            )
            .to_bits()
    );
}
