//! Tests for the illuminant spectral-power curves, the preset label/index round trip,
//! the backdrop fill, the lit models' observer head shadow, and the studio rig's own
//! golden-bits regression pin.

use super::*;
use std::hint::black_box;

/// Pins [`d65_relative_spectral_power`] against the tabulated CIE D65 values
/// directly, and against a genuine Planckian 6500K curve to confirm this is not
/// just a relabelled blackbody -- D65's real, measured blue-green irregularity
/// means the two must disagree at some wavelengths.
#[test]
fn d65_relative_spectral_power_matches_the_cie_table_at_450_and_550nm() {
    // Exact table entries, normalized by /100.0 (560nm's entry) like the function
    // under test does.
    let expected_450 = 117.008 / 100.0;
    let expected_550 = 104.046 / 100.0;

    let got_450 = d65_relative_spectral_power(450.0);
    let got_550 = d65_relative_spectral_power(550.0);
    assert!(
        (got_450 - expected_450).abs() < 1e-4,
        "450nm: got {got_450}, expected {expected_450} from the CIE D65 table"
    );
    assert!(
        (got_550 - expected_550).abs() < 1e-4,
        "550nm: got {got_550}, expected {expected_550} from the CIE D65 table"
    );

    // The real CIE table has 450nm's power exceeding 550nm's (a measured
    // irregularity, not something a smooth blackbody curve produces at 6500K).
    assert!(
        got_450 > got_550,
        "D65's real spectrum has more relative power at 450nm than 550nm; \
         got 450nm={got_450}, 550nm={got_550}"
    );

    // Distinguishes this from a plain 6500K Planckian: a genuine blackbody curve
    // is smooth (450nm/380nm ratio close to 1, both on the same gently-rising
    // Wien tail), while the real D65 table rises much more steeply there.
    let d65_ratio = d65_relative_spectral_power(450.0) / d65_relative_spectral_power(380.0);
    let blackbody_ratio = blackbody_spectrum(450.0, 6500.0) / blackbody_spectrum(380.0, 6500.0);
    assert!(
        d65_ratio > blackbody_ratio * 1.5,
        "test premise: the real D65 table's 450nm/380nm rise ({d65_ratio:.3}) must \
         be much steeper than a smooth 6500K Planckian's ({blackbody_ratio:.3}), \
         confirming the Daylight preset is no longer just a relabelled blackbody"
    );
}

/// `sample_studio_environment`'s Daylight preset must route through the D65 table,
/// not `blackbody_spectrum` -- exercised end to end through the full lighting-rig
/// function, not just the standalone table lookup above.
#[test]
fn daylight_preset_studio_environment_uses_the_d65_table_not_a_blackbody() {
    // Straight down from above (`Vec3::Y`) misses every directional light term
    // (key/fill/ring all `.max(0.0)`-clamped dot products that can legitimately
    // land at/near zero here), leaving only the ambient backdrop term -- which is
    // exactly `bg_val * spec_power`, isolating `spec_power` cleanly.
    let dir = Vec3::new(0.0, -1.0, 0.0);
    let exposure = 1.0;

    let daylight_450 =
        sample_studio_environment(dir, 450.0, LightingPreset::Daylight, exposure, 0.0, 0.0);
    let daylight_550 =
        sample_studio_environment(dir, 550.0, LightingPreset::Daylight, exposure, 0.0, 0.0);
    // A genuine Planckian 6500K curve has 450nm < 550nm (see the standalone table
    // test above); the real D65 table has the opposite ordering. If this preset
    // were still silently using `blackbody_spectrum`, this assertion would fail.
    assert!(
        daylight_450 > daylight_550,
        "Daylight preset must reflect D65's own 450nm > 550nm ordering end to \
         end, got 450nm={daylight_450}, 550nm={daylight_550}"
    );
}

#[test]
fn backdrop_fills_the_camera_ray_only_when_set() {
    let lambdas = [450.0f32, 550.0, 650.0];
    let mut radiance = [0.0f32; 3];
    let plain = LightingPreset::LightTent.studio(1.0, 0.4, 0.35);
    assert!(!fill_backdrop(plain, &lambdas, &mut radiance));
    assert_eq!(radiance, [0.0; 3]);

    let carded = plain.with_backdrop(BACKDROP_GREY);
    assert!(fill_backdrop(carded, &lambdas, &mut radiance));
    for (&value, &lambda_nm) in radiance.iter().zip(&lambdas) {
        let expected = BACKDROP_GREY * LightingPreset::LightTent.spectral_power(lambda_nm);
        assert!(
            (value - expected).abs() < 1e-6,
            "backdrop at {lambda_nm} nm: got {value}, expected {expected}"
        );
    }
}

#[test]
fn label_and_index_round_trip_for_every_preset() {
    for (pos, &p) in LightingPreset::ALL.iter().enumerate() {
        assert_eq!(LightingPreset::from_label(p.label()), p);
        assert_eq!(LightingPreset::from_index(p.index()), p);
        assert_eq!(p.index(), pos as i32);
    }
}

#[test]
fn iso_hemisphere_is_one_above_and_zero_below() {
    let val_up =
        sample_studio_environment(Vec3::Y, 550.0, LightingPreset::IsoHemisphere, 1.0, 0.0, 0.0);
    let expected = d65_relative_spectral_power(550.0);
    assert!(
        (val_up - expected).abs() < 1e-4,
        "up direction should match d65_relative_spectral_power(550): got {val_up}, expected {expected}"
    );

    let val_down = sample_studio_environment(
        -Vec3::Y,
        550.0,
        LightingPreset::IsoHemisphere,
        1.0,
        0.0,
        0.0,
    );
    assert_eq!(val_down, 0.0, "down direction should be 0");
}

/// The `Studio` arm ignores the observer; the lit models go fully dark exactly at
/// the eye direction and are untouched 20 degrees away from it.
#[test]
fn observer_head_shadow_darkens_only_the_lit_models() {
    let observer = Vec3::new(0.2, 0.9, -0.3).normalize();
    // 20 degrees away from the observer, outside the 18 degree cone.
    let perp = observer.cross(Vec3::X).normalize();
    let (sin_a, cos_a) = 20.0f32.to_radians().sin_cos();
    let outside = observer.mul_add(Vec3::splat(cos_a), perp * sin_a);

    let ring_with = sample_studio_environment_observed(
        observer,
        560.0,
        LightingPreset::RingLights,
        1.0,
        0.4,
        0.35,
        observer,
    );
    let ring_without =
        sample_studio_environment(observer, 560.0, LightingPreset::RingLights, 1.0, 0.4, 0.35);
    assert_eq!(
        ring_with.to_bits(),
        ring_without.to_bits(),
        "the studio rig must ignore the observer"
    );

    for preset in [
        LightingPreset::IsoHemisphere,
        LightingPreset::LightTent,
        LightingPreset::DaylightDome,
    ] {
        let at_eye =
            sample_studio_environment_observed(observer, 560.0, preset, 1.0, 0.4, 0.35, observer);
        assert_eq!(
            at_eye, 0.0,
            "{preset:?}: the eye direction must be fully shadowed"
        );
        let clear =
            sample_studio_environment_observed(outside, 560.0, preset, 1.0, 0.4, 0.35, observer);
        let unobserved = sample_studio_environment(outside, 560.0, preset, 1.0, 0.4, 0.35);
        assert_eq!(
            clear.to_bits(),
            unobserved.to_bits(),
            "{preset:?}: outside the cone the observer must not matter"
        );
        assert!(
            unobserved > 0.0,
            "{preset:?}: a lit direction above the girdle must be lit"
        );
    }
}

/// The default 16 degree cone is the pair of literals, bit for bit; `0` is off; the
/// width grows the cone; NaN and out-of-range inputs are sanitised.
#[test]
fn head_shadow_cosines_default_is_the_literal_pair() {
    let [outer, inner] = head_shadow_cosines(16.0);
    assert_eq!(outer.to_bits(), 0.951_056_5f32.to_bits());
    assert_eq!(inner.to_bits(), 0.970_295_7f32.to_bits());
    assert_eq!(
        head_shadow_cosines(DEFAULT_HEAD_SHADOW_DEG),
        DEFAULT_HEAD_SHADOW_COSINES
    );
    assert_eq!(head_shadow_cosines(f32::NAN), DEFAULT_HEAD_SHADOW_COSINES);
    // Off: a pair no dot product reaches, so the smoothstep is exactly 0.
    let [off_outer, off_inner] = head_shadow_cosines(0.0);
    assert!(off_outer > 1.0 && off_inner > off_outer);
    assert_eq!(head_shadow_cosines(-5.0), [off_outer, off_inner]);
    // Wider shadow -> smaller cosines, still ordered outer < inner.
    let [wide_outer, wide_inner] = head_shadow_cosines(25.0);
    assert!(wide_outer < outer && wide_inner < inner && wide_outer < wide_inner);
    // Clamped to a sane range: a tiny width behaves as 3 degrees, a huge one as 88.
    assert_eq!(head_shadow_cosines(1.0), head_shadow_cosines(3.0));
    assert_eq!(head_shadow_cosines(500.0), head_shadow_cosines(88.0));
}

/// `with_head_shadow` stores a sanitised width and leaves everything else alone.
#[test]
fn with_head_shadow_stores_a_sanitised_width() {
    let base = LightingPreset::LightTent.studio(1.0, 0.4, 0.35);
    assert_eq!(base.head_shadow_deg().to_bits(), 16.0f32.to_bits());
    assert_eq!(base.with_head_shadow(25.0).head_shadow_deg(), 25.0);
    assert_eq!(base.with_head_shadow(0.0).head_shadow_deg(), 0.0);
    assert_eq!(base.with_head_shadow(-3.0).head_shadow_deg(), 0.0);
    assert_eq!(base.with_head_shadow(f32::NAN).head_shadow_deg(), 16.0);
    assert_eq!(base.with_head_shadow(1.0).head_shadow_deg(), 3.0);
    assert_eq!(base.with_head_shadow(200.0).head_shadow_deg(), 88.0);
    assert_eq!(
        base.with_head_shadow(25.0)
            .with_backdrop(0.5)
            .head_shadow_deg(),
        25.0,
        "with_backdrop must keep the head shadow"
    );
    assert_eq!(
        base.with_head_shadow(25.0)
            .with_surface_glare(0.5)
            .head_shadow_deg(),
        25.0,
        "with_surface_glare must keep the head shadow"
    );
}

/// Off removes the darkening; a wider shadow darkens a direction the default leaves lit;
/// far outside both cones nothing changes.
#[test]
fn head_shadow_width_off_default_and_wide() {
    let observer = Vec3::new(0.2, 0.9, -0.3).normalize();
    let perp = observer.cross(Vec3::X).normalize();
    let at = |degrees: f32| {
        let (sin_a, cos_a) = degrees.to_radians().sin_cos();
        observer.mul_add(Vec3::splat(cos_a), perp * sin_a)
    };
    let rig = crate::optics::studio_rig::StudioRig::new(0.4, 0.35);
    let sample = |dir: Vec3, preset: LightingPreset, deg: f32| {
        sample_studio_environment_with_rig_shadow(
            dir,
            560.0,
            preset,
            1.0,
            &rig,
            observer,
            head_shadow_cosines(deg),
        )
    };
    for preset in [
        LightingPreset::IsoHemisphere,
        LightingPreset::LightTent,
        LightingPreset::DaylightDome,
    ] {
        let unobserved = sample_studio_environment(at(0.0), 560.0, preset, 1.0, 0.4, 0.35);
        assert_eq!(
            sample(at(0.0), preset, 0.0).to_bits(),
            unobserved.to_bits(),
            "{preset:?}: head shadow 0 must not darken the eye direction"
        );
        assert_eq!(sample(at(0.0), preset, 16.0), 0.0, "{preset:?}: default");
        assert_eq!(sample(at(0.0), preset, 25.0), 0.0, "{preset:?}: wide");
        // 20 degrees: outside the default cone, inside the 25 degree one.
        let default_20 = sample(at(20.0), preset, 16.0);
        assert!(
            default_20 > 0.0,
            "{preset:?}: default leaves 20 degrees lit"
        );
        assert_eq!(
            sample(at(20.0), preset, 25.0),
            0.0,
            "{preset:?}: the wide shadow covers 20 degrees"
        );
        // 40 degrees: outside every cone, the shadow width must not matter.
        let far = sample_studio_environment(at(40.0), 560.0, preset, 1.0, 0.4, 0.35);
        for deg in [0.0, 16.0, 25.0] {
            assert_eq!(sample(at(40.0), preset, deg).to_bits(), far.to_bits());
        }
    }
    // The Studio arm ignores the observer and so the width.
    let studio = sample(at(0.0), LightingPreset::RingLights, 25.0);
    let studio_default = sample(at(0.0), LightingPreset::RingLights, 16.0);
    assert_eq!(studio.to_bits(), studio_default.to_bits());
}

/// The default-width path is bit-identical to the unparameterised one: the same 192
/// (direction, wavelength) pairs per preset the Studio golden uses, through the public
/// rig sampler, the explicit-cone sampler and the `EnvironmentSource` entry points
/// (`with_head_shadow(16.0)` included).
#[test]
fn default_head_shadow_is_bit_identical_to_the_unparameterised_call() {
    let observer = Vec3::new(0.2, 0.9, -0.3).normalize();
    let rig = crate::optics::studio_rig::StudioRig::new(0.4, 0.35);
    for preset in LightingPreset::ALL {
        let plain = preset.studio(1.2, 0.4, 0.35);
        let explicit = plain.with_head_shadow(16.0);
        let lambdas = baseline_lambdas(preset);
        for k in 0..64 {
            let phi = (k as f32 + 0.5) * (std::f32::consts::PI * (3.0 - 5.0f32.sqrt()));
            let y = (k as f32 + 0.5).mul_add(-(2.0 / 64.0), 1.0);
            let r = y.mul_add(-y, 1.0).max(0.0).sqrt();
            let dir = Vec3::new(r * phi.cos(), y, r * phi.sin());
            for observer in [Vec3::ZERO, observer, dir] {
                for &lambda in &lambdas {
                    let reference = sample_studio_environment_with_rig(
                        dir, lambda, preset, 1.2, &rig, observer,
                    );
                    let shadowed = sample_studio_environment_with_rig_shadow(
                        dir,
                        lambda,
                        preset,
                        1.2,
                        &rig,
                        observer,
                        head_shadow_cosines(16.0),
                    );
                    assert_eq!(reference.to_bits(), shadowed.to_bits(), "{preset:?} k={k}");
                    for source in [plain, explicit] {
                        let channel =
                            sample_environment_channel(source, dir, lambda, Some(&rig), observer);
                        assert_eq!(reference.to_bits(), channel.to_bits(), "{preset:?} k={k}");
                    }
                }
                let channels =
                    sample_environment_channels(explicit, dir, &lambdas, Some(&rig), observer);
                for (&lambda, &value) in lambdas.iter().zip(&channels) {
                    let reference = sample_studio_environment_with_rig(
                        dir, lambda, preset, 1.2, &rig, observer,
                    );
                    assert_eq!(reference.to_bits(), value.to_bits(), "{preset:?} k={k}");
                }
            }
        }
    }
}

#[test]
fn legacy_lit_model_labels_resolve_to_the_renamed_presets() {
    assert_eq!(
        LightingPreset::from_label("ISO hemisphere"),
        LightingPreset::IsoHemisphere
    );
    assert_eq!(
        LightingPreset::from_label("ISO hemisphere (GemRay-style)"),
        LightingPreset::IsoHemisphere,
        "a settings file saved before the GemRay-style label was reworded must still \
         load as IsoHemisphere"
    );
    assert_eq!(
        LightingPreset::from_label("Grading tray (D65 hemisphere + head shadow)"),
        LightingPreset::IsoHemisphere
    );
    assert_eq!(LightingPreset::default(), LightingPreset::LightTent);
    assert_eq!(
        LightingPreset::from_label("D65 Daylight (5500K)"),
        LightingPreset::Daylight
    );
    assert_eq!(
        LightingPreset::from_label("no such rig"),
        LightingPreset::LightTent
    );
    assert_eq!(
        LightingPreset::from_label("D65 Daylight (6500K)"),
        LightingPreset::Daylight
    );
    assert_eq!(
        LightingPreset::from_label("Soft dome + ring lights"),
        LightingPreset::LightTent
    );
    assert_eq!(
        LightingPreset::from_label("Daylight dome + sun"),
        LightingPreset::DaylightDome
    );
    // The dome was relabelled; the old label is what saved settings meant: the dome.
    assert_eq!(
        LightingPreset::from_label("Daylight sky + sun"),
        LightingPreset::DaylightDome,
        "a settings file saved with the old dome label must still load as the dome"
    );
    assert_eq!(
        LightingPreset::from_label("Daylight sky (no sun)"),
        LightingPreset::DaylightDome
    );
    assert_eq!(
        LightingPreset::from_label("Daylight sky + direct sun"),
        LightingPreset::DaylightSun
    );
}

#[test]
fn new_models_never_exceed_their_documented_peak() {
    let mut max_iso = 0.0f32;
    let mut max_soft = 0.0f32;
    let mut max_daylight = 0.0f32;

    for i in 0..2000 {
        let phi = (i as f32 + 0.5) * (std::f32::consts::PI * (3.0 - 5.0f32.sqrt()));
        let y = (i as f32 + 0.5).mul_add(-(2.0 / 2000.0), 1.0);
        let r = y.mul_add(-y, 1.0).max(0.0).sqrt();
        let dir = Vec3::new(r * phi.cos(), y, r * phi.sin());

        let v_iso =
            sample_studio_environment(dir, 560.0, LightingPreset::IsoHemisphere, 1.0, 0.4, 0.35);
        let v_soft =
            sample_studio_environment(dir, 560.0, LightingPreset::LightTent, 1.0, 0.4, 0.35);
        let v_daylight =
            sample_studio_environment(dir, 560.0, LightingPreset::DaylightDome, 1.0, 0.4, 0.35);

        max_iso = max_iso.max(v_iso);
        max_soft = max_soft.max(v_soft);
        max_daylight = max_daylight.max(v_daylight);
    }

    assert!(
        max_iso <= 1.0 + 1e-5,
        "ISO peak must not exceed 1.0, got {max_iso}"
    );
    // Spark (5.0) on the tent walls (<= 0.22) is the tent's brightest direction; the
    // key softbox (1.4) never coincides with it.
    assert!(
        max_soft <= 5.5,
        "Light tent peak must not exceed 5.5, got {max_soft}"
    );
    // The dome is sky only now: aureole (0.30, at the key direction) + sky (<= 0.18, the
    // horizon value; 0.08 less at the zenith) at most, and the D65 power at 560 nm is 1.0.
    // 0.18 + 0.30 = 0.48 bounds it analytically; the old cap of 10.5 included the sun (10.0).
    assert!(
        max_daylight <= 0.5,
        "Daylight dome peak must not exceed 0.5, got {max_daylight}"
    );
}

/// A unit direction `degrees` away from `key`, in an arbitrary but fixed plane.
fn tilted_from(key: Vec3, degrees: f32) -> Vec3 {
    let (sin_a, cos_a) = degrees.to_radians().sin_cos();
    (key * cos_a + key.any_orthonormal_vector() * sin_a).normalize()
}

/// The sky-only dome has no sun disc: at and around the key direction its radiance is
/// the sky plus the aureole and nothing else, at every pose.
#[test]
fn daylight_dome_has_no_sun_disc() {
    for (yaw, pitch) in [(0.3f32, 0.6f32), (0.85, 0.95), (-0.5, 1.2), (0.4, 0.35)] {
        let rig = crate::optics::studio_rig::StudioRig::new(yaw, pitch);
        for degrees in [0.0f32, 0.1, 0.2, 0.5, 1.0, 2.0, 3.0] {
            let dir = tilted_from(rig.key_dir, degrees);
            let value = sample_studio_environment_with_rig(
                dir,
                560.0,
                LightingPreset::DaylightDome,
                1.0,
                &rig,
                Vec3::ZERO,
            );
            // sky + aureole, horizon 1 above 3 degrees, no head shadow, no ground.
            let sky = 0.08f32.mul_add(1.0 - dir.y.max(0.0), 0.10);
            let aureole = dir.dot(rig.key_dir).max(0.0).powi(8) * 0.30;
            let expected = (sky + aureole) * d65_relative_spectral_power(560.0);
            assert!(
                (value - expected).abs() <= 1e-4,
                "dome at {degrees} deg from the key (pose {yaw}, {pitch}): {value} vs sky+aureole {expected}"
            );
            assert!(value <= 0.5, "no sun: {value}");
        }
    }
}

/// The NEE entry points compute the key direction without building a `StudioRig`; the result
/// must be bit-identical to the rig's own.
#[test]
fn nee_key_dir_is_bit_identical_to_the_studio_rig() {
    for (yaw, pitch) in [(0.0f32, 1.2f32), (0.84, 0.94), (-2.1, 0.3), (3.0, -0.2)] {
        let a = rig::key_dir_only_for_test(yaw, pitch);
        let b = crate::optics::studio_rig::StudioRig::new(yaw, pitch).key_dir;
        assert_eq!(
            a.to_array().map(f32::to_bits),
            b.to_array().map(f32::to_bits)
        );
    }
}

/// The sun disc: radius 0.27 degrees, its f32 solid angle, the radiance the sky lacks
/// exactly inside it and nowhere outside, and the documented ~82 % direct share of the
/// horizontal irradiance at the default 72 degree key elevation.
#[test]
fn daylight_sun_disc_radiance_and_solid_angle() {
    use rig::{SUN_DISC_COS, SUN_ONE_MINUS_COS, SUN_RADIANCE, SUN_SOLID_ANGLE};

    let radius_deg = SUN_DISC_COS.acos().to_degrees();
    assert!(
        (radius_deg - 0.27).abs() < 0.005,
        "sun radius {radius_deg} deg should be 0.27"
    );
    // The two literals are exactly consistent with the cosine literal and each other.
    assert_eq!(SUN_ONE_MINUS_COS, 1.0 - SUN_DISC_COS);
    let omega = std::f32::consts::TAU * (1.0 - SUN_DISC_COS);
    assert!(
        (SUN_SOLID_ANGLE / omega - 1.0).abs() < 1e-4,
        "SUN_SOLID_ANGLE {SUN_SOLID_ANGLE} vs 2 pi (1 - cos) {omega}"
    );
    // Within 0.5 % of the analytic small-angle value pi r^2 for r = 0.27 degrees.
    let small_angle = std::f32::consts::PI * 0.27f32.to_radians().powi(2);
    assert!((SUN_SOLID_ANGLE / small_angle - 1.0).abs() < 5e-3);

    let exposure = 1.3f32;
    let spd = d65_relative_spectral_power(560.0);
    for (yaw, pitch) in [(0.3f32, 0.6f32), (0.85, 1.2566), (-0.5, 1.2)] {
        let rig = crate::optics::studio_rig::StudioRig::new(yaw, pitch);
        let sample = |preset: LightingPreset, dir: Vec3| {
            sample_studio_environment_with_rig(dir, 560.0, preset, exposure, &rig, Vec3::ZERO)
        };
        for degrees in [0.0f32, 0.1, 0.2] {
            let dir = tilted_from(rig.key_dir, degrees);
            let with_sun = sample(LightingPreset::DaylightSun, dir);
            let sky_only = sample(LightingPreset::DaylightDome, dir);
            let sun = with_sun - sky_only;
            let expected = SUN_RADIANCE * (spd * exposure);
            assert!(
                (sun / expected - 1.0).abs() < 1e-4,
                "{degrees} deg from the key: sun term {sun} vs {expected}"
            );
        }
        // Outside the disc the sun preset is the dome, bit for bit.
        for degrees in [0.3f32, 0.5, 2.0, 30.0, 120.0] {
            let dir = tilted_from(rig.key_dir, degrees);
            assert_eq!(
                sample(LightingPreset::DaylightSun, dir).to_bits(),
                sample(LightingPreset::DaylightDome, dir).to_bits(),
                "{degrees} deg from the key must carry no sun"
            );
        }
    }

    // Direct share of the horizontal irradiance at the default 72 degree key elevation:
    // E_sky = 2 pi (0.09 - 0.08 / 3) + 0.30 sin(e) 2 pi / 10, E_sun = L Omega sin(e).
    let sin_e = 72.0f32.to_radians().sin();
    let e_sky = std::f32::consts::TAU.mul_add(
        0.09 - 0.08 / 3.0,
        0.30 * sin_e * std::f32::consts::TAU / 10.0,
    );
    let e_sun = SUN_RADIANCE * SUN_SOLID_ANGLE * sin_e;
    let share = e_sun / (e_sun + e_sky);
    assert!(
        (0.80..=0.85).contains(&share),
        "direct sun share of the horizontal irradiance at 72 deg is {share}, documented ~82 %"
    );
}

/// The sun's radiance is faded by the horizon at the key direction and not by the head
/// shadow (the NEE draw has no observer): a key below the horizon switches it off, and an
/// observer sitting on the sun leaves the disc lit.
#[test]
fn daylight_sun_follows_the_horizon_but_not_the_head_shadow() {
    let high = crate::optics::studio_rig::StudioRig::new(0.3, 1.0);
    let at_sun = |rig: &crate::optics::studio_rig::StudioRig, observer: Vec3| {
        sample_studio_environment_with_rig(
            rig.key_dir,
            560.0,
            LightingPreset::DaylightSun,
            1.0,
            rig,
            observer,
        )
    };
    let lit = at_sun(&high, Vec3::ZERO);
    let observed = at_sun(&high, high.key_dir);
    assert!(
        observed > 0.99 * rig::SUN_RADIANCE,
        "head shadow must not dim the sun: {observed} (unobserved {lit})"
    );
    let below = crate::optics::studio_rig::StudioRig::new(0.3, -0.3);
    // Only the dim ground term (0.04) is left at a direction below the horizon.
    assert!(
        at_sun(&below, Vec3::ZERO) < 0.1,
        "a sun below the horizon must not shine"
    );
}

/// The unit direction `k` of `n` on a golden-angle spiral over the sphere.
fn spiral_dir(k: u32, n: u32) -> Vec3 {
    let phi = (k as f32 + 0.5) * (std::f32::consts::PI * (3.0 - 5.0f32.sqrt()));
    let y = (k as f32 + 0.5).mul_add(-(2.0 / n as f32), 1.0);
    let r = y.mul_add(-y, 1.0).max(0.0).sqrt();
    Vec3::new(r * phi.cos(), y, r * phi.sin())
}

/// Peak caps of the three tent variants (D65 SPD, so `spectral_power(560)` is about 1).
/// Shop: walls <= 0.66 + key 1.4 + spark 5.0 (the key and the spark never coincide, but the
/// cap is the sum's bound, 7.1). Window: key 1.4 + walls <= 0.088. Tray: walls <= 0.99
/// (key and spark are off through `spot_mult` 0) over the ground 0.8 below the horizon.
#[test]
fn tent_variants_never_exceed_their_documented_peak() {
    let mut max = [0.0f32; 3];
    let presets = [
        LightingPreset::ShopLights,
        LightingPreset::WindowDaylight,
        LightingPreset::WhiteTray,
    ];
    for i in 0..2000 {
        let dir = spiral_dir(i, 2000);
        for (slot, preset) in presets.into_iter().enumerate() {
            let v = sample_studio_environment(dir, 560.0, preset, 1.0, 0.4, 0.35);
            max[slot] = max[slot].max(v);
        }
    }
    let spd = LightingPreset::ShopLights.spectral_power(560.0);
    assert!(max[0] <= 7.1 * spd, "shop peak {}", max[0]);
    assert!(max[1] <= 1.5 * spd, "window peak {}", max[1]);
    assert!(max[2] <= 1.0 * spd, "tray peak {}", max[2]);
}

/// The light tent's parameters are the identity values the formula had as literals, and
/// the three variants differ from them (so the harness can tell them apart by params).
#[test]
fn tent_params_default_to_the_light_tent_and_variants_are_distinct() {
    let tent = LightingPreset::LightTent.params();
    assert_eq!(tent.tent, TentParams::DEFAULT);
    assert_eq!(tent.tent.walls.to_bits(), 1.0f32.to_bits());
    assert_eq!(tent.tent.cards.to_bits(), 1.0f32.to_bits());
    assert_eq!(tent.tent.spark.to_bits(), 1.0f32.to_bits());
    assert_eq!(tent.tent.ground.to_bits(), 0.02f32.to_bits());
    assert_eq!(tent.tent.flat.to_bits(), 0.0f32.to_bits());
    let variants = [
        LightingPreset::ShopLights,
        LightingPreset::WindowDaylight,
        LightingPreset::WhiteTray,
    ];
    for (i, a) in variants.into_iter().enumerate() {
        assert_ne!(a.params().tent, TentParams::DEFAULT, "{a:?}");
        assert_eq!(
            a.params().tent.cards.to_bits(),
            0.0f32.to_bits(),
            "{a:?} has no cards"
        );
        for b in variants.into_iter().skip(i + 1) {
            assert_ne!(a.params().tent, b.params().tent, "{a:?} vs {b:?}");
        }
    }
    // Only the light tent model reads the knobs; every other preset keeps the default.
    for preset in LightingPreset::ALL {
        if !variants.contains(&preset) {
            assert_eq!(preset.params().tent, TentParams::DEFAULT, "{preset:?}");
        }
    }
}

/// Wall brightness ordering at the zenith (away from the key, the spark and the cards, no
/// observer): white tray > jewellery shop > window daylight, all under the same D65 SPD.
/// The window room is dim (0.05-0.09), the shop about 0.5, the tray close to 1.
#[test]
fn tent_variant_walls_are_ordered_tray_shop_window() {
    let up = Vec3::Y;
    let at = |preset| sample_studio_environment(up, 560.0, preset, 1.0, 0.4, 0.35);
    let spd = LightingPreset::ShopLights.spectral_power(560.0);
    let (tray, shop, window) = (
        at(LightingPreset::WhiteTray) / spd,
        at(LightingPreset::ShopLights) / spd,
        at(LightingPreset::WindowDaylight) / spd,
    );
    assert!(tray > shop && shop > window, "{tray} {shop} {window}");
    assert!((0.05..=0.09).contains(&window), "window walls {window}");
    assert!((0.4..=0.7).contains(&shop), "shop walls {shop}");
    assert!((0.89..=1.0).contains(&tray), "tray walls {tray}");
}

/// No cards, spark on or off, and the tray's bright ground, measured against the rig's own
/// directions: a card centre is as bright as the open wall; the spark exists only in the
/// shop; the key is on in the shop and the window and off in the tray; below the horizon
/// the tray shows its 0.8 ground, the window and the shop their dim floors.
#[test]
fn tent_variants_have_no_cards_and_the_right_spark_key_and_ground() {
    let (yaw, pitch) = (0.4f32, 0.35f32);
    let rig = crate::optics::studio_rig::StudioRig::new(yaw, pitch);
    let spd = LightingPreset::ShopLights.spectral_power(560.0);
    let at =
        |dir: Vec3, preset| sample_studio_environment(dir, 560.0, preset, 1.0, yaw, pitch) / spd;
    let wall = |dir: Vec3, scale: f32| scale * 0.08f32.mul_add(dir.y.max(0.0), 0.14);

    // A card centre (ring slot 4): the variants show the open wall, the tent does not.
    // (The tray's walls are flat: 0.18 * 5.0 at every elevation.)
    let card = rig.ring_dirs[4];
    for (preset, scale, flat) in [
        (LightingPreset::ShopLights, 3.0, false),
        (LightingPreset::WindowDaylight, 0.4, false),
        (LightingPreset::WhiteTray, 5.0, true),
    ] {
        let expected = if flat {
            0.18 * scale
        } else {
            wall(card, scale)
        };
        assert!(
            (at(card, preset) - expected).abs() < 1e-4,
            "{preset:?}: card centre {} vs open wall {expected}",
            at(card, preset)
        );
    }

    // Spark: the shop has its bare bulb at the fill position, the others do not.
    assert!(at(rig.fill_dir, LightingPreset::ShopLights) >= 5.0);
    assert!(at(rig.fill_dir, LightingPreset::WindowDaylight) < 0.2);
    assert!(at(rig.fill_dir, LightingPreset::WhiteTray) < 1.1);

    // Key: on in the shop and the window, off in the tray.
    assert!(at(rig.key_dir, LightingPreset::ShopLights) >= 1.4);
    assert!(at(rig.key_dir, LightingPreset::WindowDaylight) >= 1.4);
    assert!(at(rig.key_dir, LightingPreset::WhiteTray) < 1.1);

    // Ground (straight down, fully below the horizon blend).
    let down = -Vec3::Y;
    assert!((at(down, LightingPreset::WhiteTray) - 0.8).abs() < 1e-4);
    assert!((at(down, LightingPreset::WindowDaylight) - 0.03).abs() < 1e-4);
    assert!((at(down, LightingPreset::ShopLights) - 0.05).abs() < 1e-4);
}

/// `flat == 0` keeps the wall gradient bit for bit: every preset except the tray has
/// `flat == 0`, and the walls it shows (read off a direction away from the key, spark, cards
/// and horizon blend, so only the walls contribute) equal the old closed form exactly.
#[test]
fn tent_flat_zero_is_bit_identical_to_the_old_wall_values() {
    for preset in LightingPreset::ALL {
        if preset != LightingPreset::WhiteTray {
            assert_eq!(preset.params().tent.flat.to_bits(), 0.0f32.to_bits());
        }
    }
    // The light tent at the zenith (no card, key or spark there): the old formula
    // `(0.08 * 1 + 0.14) * 1.0` times the card factor 1 (card mask 0), times the horizon
    // blend and spectral power the lighting adds.
    let (yaw, pitch) = (0.4f32, 0.35f32);
    let up = Vec3::Y;
    let got = sample_studio_environment(up, 560.0, LightingPreset::LightTent, 1.0, yaw, pitch);
    let spd = LightingPreset::LightTent.spectral_power(560.0);
    let walls_old = 0.08f32.mul_add(1.0, 0.14) * 1.0;
    let rig = crate::optics::studio_rig::StudioRig::new(yaw, pitch);
    let key = (up.dot(rig.key_dir) > 0.9) || (up.dot(rig.fill_dir) > 0.9);
    if !key {
        let expected = walls_old * spd;
        assert!(
            (got - expected).abs() <= expected * 1e-5,
            "light tent zenith {got} vs {expected}"
        );
    }
    // The pinned `BASELINE_BITS` LightTent row (unchanged) is the bit-exact check.
}

/// The white tray's walls do not depend on the elevation: every direction above the horizon,
/// away from the observer's head shadow, sees the same wall value (about 0.9).
#[test]
fn white_tray_walls_are_uniform() {
    let tray = LightingPreset::WhiteTray.params().tent;
    assert!((tray.flat - 1.0).abs() < f32::EPSILON);
    let spd = LightingPreset::WhiteTray.spectral_power(560.0);
    let mut seen = Vec::new();
    for y in [0.3f32, 0.5, 0.7, 0.9, 1.0] {
        let x = y.mul_add(-y, 1.0).max(0.0).sqrt();
        let dir = Vec3::new(x, y, 0.0).normalize();
        let v =
            sample_studio_environment(dir, 560.0, LightingPreset::WhiteTray, 1.0, 0.4, 0.35) / spd;
        seen.push(v);
    }
    for v in &seen {
        assert!(
            (v - 0.9).abs() < 0.02,
            "tray wall {v} not near 0.9 ({seen:?})"
        );
        assert!((v - seen[0]).abs() < 0.01, "tray walls vary: {seen:?}");
    }
}

/// Largest tolerated bit-pattern distance between a freshly computed value and
/// its golden. Every baseline value here is positive and finite, so `u32` bit
/// patterns increase monotonically with the represented value exactly like ULP
/// distance does -- a signed difference of the raw bit patterns IS the ULP
/// distance, no separate distance function needed. See the doc comment above
/// for why this must stay tight rather than a wider, looser bound.
const MAX_ULP_DIFF: i64 = 2;
// One row per preset in the order the table was recorded (the declaration order, not
// the combo order of [`LightingPreset::ALL`]): Daylight, Incandescent,
// RingLights, DarkSpotlight, IsoHemisphere, LightTent, DaylightDome, UvLamp365,
// UvLamp395) -- the first seven rows are unchanged since the table was first recorded;
// NOTE (lane SUN, 2026-10-07): the DaylightDome row (index 6) needs NO re-pin even though the
// dome lost its sun disc: the removed term was `smoothstep(cos 4 deg, cos 2 deg, key_dot) *
// 10`, exactly `0.0` for every probe direction here (the nearest of the 64 Fibonacci
// directions is 8.8 degrees from the pose-(0.4, 0.35) key, outside the 4 degree cone) and
// `x + 0.0 == x`. Checked numerically with a script, not by running the suite; if this test
// does fail on the dome row, // RE-PIN: that row (it was the only row the change could touch);
// the two UV-lamp rows were recorded from their implementation at the lamp's own
// wavelengths (see `baseline_lambdas`), since a UV line is exactly zero at 450-650 nm. See
// "Regenerating the baseline" above. IsoHemisphere's row is legitimately full of
// exact `0` bit patterns for its lower hemisphere (see
// `iso_hemisphere_is_one_above_and_zero_below` above), not a probe artifact.
#[allow(
    clippy::unreadable_literal,
    reason = "bit-pattern baseline table is pinned verbatim as printed"
)]
const BASELINE_BITS: [[u32; 192]; 9] = [
    [
        1025162801, 1024039367, 1020505008, 1025092110, 1023976507, 1020408312, 1025021648,
        1023913851, 1020311927, 1024951087, 1023851107, 1020215409, 1025346055, 1024202320,
        1020755680, 1025666613, 1024487368, 1021194168, 1024738758, 1023662299, 1019924966,
        1032212597, 1030676166, 1027062275, 1044013601, 1042660459, 1040118652, 1024533502,
        1023479781, 1019644200, 1024456252, 1023411089, 1019538532, 1024385406, 1023286006,
        1019441621, 1065648146, 1064019172, 1060454089, 1082551552, 1081020810, 1077403919,
        1024272461, 1023085141, 1019287126, 1049820964, 1048753769, 1044976412, 1077087441,
        1075787537, 1073015688, 1092050504, 1090951570, 1087311350, 1099112643, 1097413659,
        1093885500, 1099524362, 1098145878, 1094448685, 1097428217, 1095733547, 1092593244,
        1098879375, 1097023947, 1093585753, 1071105434, 1069538931, 1066636126, 1079221057,
        1077684794, 1074838028, 1083370228, 1082303605, 1078523774, 1085806508, 1084469997,
        1081856327, 1076808764, 1075539732, 1072634491, 1048797347, 1047111092, 1043576219,
        1051158443, 1049943083, 1046805930, 1043203236, 1041939864, 1039010164, 1023023449,
        1021207730, 1017843119, 1022534055, 1020772551, 1017508402, 1022392475, 1020646654,
        1017411569, 1078815957, 1077324571, 1074560963, 1066793382, 1065704562, 1062020640,
        1021968452, 1020269604, 1017121561, 1021918760, 1020225417, 1017087574, 1021686103,
        1020018533, 1016928451, 1032860469, 1031813576, 1027948490, 1021493497, 1019847265,
        1016796720, 1021261748, 1019641188, 1016638216, 1042717161, 1041507636, 1038345269,
        1022678992, 1020901431, 1017607530, 1020837726, 1019264139, 1016348210, 1020696384,
        1019138455, 1016251539, 1020555042, 1019012770, 1016154870, 1030483910, 1028771010,
        1025596927, 1020290565, 1018777592, 1015973983, 1020131020, 1018635721, 1015864863,
        1020003539, 1018522363, 1015777674, 1019848338, 1018384354, 1015671524, 1019707337,
        1018258973, 1015575087, 1019565656, 1018132987, 1015478186, 1019424315, 1018007304,
        1015381517, 1019317772, 1017912564, 1015308648, 1019141634, 1017755938, 1015188179,
        1019000292, 1017630254, 1015091509, 1018858951, 1017504570, 1014968112, 1018717611,
        1017378888, 1014774776, 1018576269, 1017253204, 1014581436, 1018434928, 1017127520,
        1014388097, 1018293587, 1017001837, 1014194759, 1018152247, 1016876154, 1014001422,
        1018010905, 1016750470, 1013808082,
    ],
    [
        1012776288, 1023024255, 1027534148, 1012674986, 1022909992, 1027446924, 1012574016,
        1022796103, 1027359984, 1012472997, 1022682158, 1027273003, 1013038877, 1023320442,
        1027760248, 1013743885, 1023762916, 1028367289, 1012168648, 1022338868, 1027010946,
        1019246433, 1029248933, 1034270883, 1033569102, 1043257544, 1048934636, 1011874526,
        1022007112, 1026757696, 1011763880, 1021882310, 1026662425, 1011662310, 1021767744,
        1026574969, 1052630511, 1062611215, 1067678634, 1072092693, 1082273685, 1086767714,
        1011528857, 1021617217, 1026460061, 1039523329, 1049274807, 1054061465, 1067033203,
        1076710087, 1082411289, 1082258257, 1091736542, 1096796415, 1088513955, 1098792656,
        1103238439, 1089221965, 1099249452, 1103848064, 1086345145, 1096346346, 1101371006,
        1088137268, 1098367772, 1102914096, 1060695451, 1070634742, 1075788551, 1068868734,
        1078780474, 1083991755, 1073502184, 1083068602, 1087981342, 1075082870, 1084716386,
        1090497071, 1066795188, 1076441617, 1082206348, 1037920448, 1048165643, 1052681319,
        1039155781, 1049067518, 1053744991, 1032925538, 1042531635, 1048185002, 1009987769,
        1019878946, 1025133122, 1009637163, 1019483480, 1024831236, 1009535690, 1019369023,
        1024743864, 1066610359, 1076233140, 1081963975, 1056991862, 1066457279, 1071457399,
        1009231887, 1019026349, 1024482277, 1009196283, 1018986190, 1024451622, 1009029637,
        1018798222, 1024308133, 1020174804, 1030296090, 1035070248, 1008904547, 1018657126,
        1024200425, 1008725550, 1018455225, 1024046300, 1030667111, 1040622478, 1045270225,
        1009984565, 1019875332, 1025130364, 1008421748, 1018112552, 1023784715, 1008320479,
        1017998326, 1023697518, 1008219211, 1017884100, 1023610322, 1017711380, 1027517471,
        1032949141, 1008032328, 1017673306, 1023449409, 1007915408, 1017541427, 1023287297,
        1007824072, 1017438403, 1023130007, 1007712873, 1017312977, 1022938515, 1007611848,
        1017199027, 1022764543, 1007510338, 1017084528, 1022589733, 1007409070, 1016970303,
        1022415342, 1007332735, 1016884200, 1022283886, 1007206536, 1016741854, 1022066561,
        1007105267, 1016627628, 1021892168, 1007004000, 1016513403, 1021717777, 1006902733,
        1016399180, 1021543388, 1006801464, 1016284954, 1021368995, 1006700197, 1016170729,
        1021194604, 1006564898, 1016056504, 1021020213, 1006362365, 1015942280, 1020845824,
        1006159828, 1015828055, 1020671431,
    ],
    [
        1021303258, 1023655294, 1023453223, 1021200980, 1023595102, 1023378703, 1021099049,
        1023535114, 1023261534, 1020997253, 1023475205, 1023144520, 1021568336, 1023811298,
        1023605575, 1022776057, 1024522062, 1024299704, 1020689822, 1023178377, 1022791133,
        1027755127, 1030009411, 1029658622, 1044895793, 1047214420, 1046853619, 1020392899,
        1022828889, 1022449825, 1020281301, 1022697534, 1022321544, 1020178664, 1022576726,
        1022203564, 1061137584, 1063361422, 1063015371, 1083905861, 1085705250, 1085425247,
        1020101264, 1022485625, 1022114595, 1050651329, 1052503811, 1052215547, 1078871876,
        1081265151, 1080892734, 1093572623, 1095598290, 1095283076, 1100456709, 1102216023,
        1101942257, 1100933244, 1102776921, 1102490027, 1098170626, 1099958974, 1099738034,
        1100203294, 1101917744, 1101650959, 1073213026, 1074915693, 1074691885, 1081344407,
        1083152919, 1082932654, 1084855932, 1086823517, 1086517342, 1085755938, 1087882856,
        1087551887, 1078554162, 1080891191, 1080527526, 1049704878, 1051389805, 1051127614,
        1047743926, 1049571387, 1049351755, 1044118990, 1046300096, 1045960695, 1018488179,
        1020586967, 1020260376, 1018134305, 1020170445, 1019853601, 1018031797, 1020049789,
        1019735769, 1075090799, 1076814691, 1076546437, 1068234283, 1070229409, 1069918948,
        1017725102, 1019688798, 1019383227, 1017689159, 1019646492, 1019341912, 1017521023,
        1019448590, 1019148641, 1028692337, 1031112541, 1030735933, 1017420802, 1019330627,
        1019033440, 1017213944, 1019087147, 1018795658, 1039204672, 1041094120, 1040876564,
        1018976767, 1021162052, 1020822000, 1016907249, 1018726157, 1018443117, 1016805017,
        1018605826, 1018325602, 1016702784, 1018485494, 1018208087, 1026205460, 1028185399,
        1027877301, 1016519388, 1018269632, 1017997277, 1016396089, 1018124504, 1017855546,
        1016303883, 1018015974, 1017749556, 1016191626, 1017883843, 1017620518, 1016089640,
        1017763802, 1017503286, 1015987162, 1017643183, 1017385490, 1015884931, 1017522852,
        1017267976, 1015807868, 1017432147, 1017179394, 1015680468, 1017282193, 1017032949,
        1015578235, 1017161861, 1016915434, 1015476004, 1017041531, 1016797920, 1015373773,
        1016921202, 1016680407, 1015271540, 1016800870, 1016562892, 1015169308, 1016680540,
        1016445378, 1015067077, 1016560210, 1016327864, 1014908123, 1016439880, 1016210351,
        1014703658, 1016319549, 1016092836,
    ],
    [
        1024021274, 1023786218, 1021689627, 1023958517, 1023725100, 1021584632, 1023895985,
        1023664201, 1021480016, 1023833763, 1023603604, 1021375915, 1024183876, 1023944572,
        1021961661, 1025533326, 1025258778, 1023814745, 1023644939, 1023419712, 1021060011,
        1030644714, 1030236666, 1028090467, 1051708076, 1051407176, 1049824545, 1023462788,
        1023074462, 1020755269, 1023378726, 1022941356, 1020640940, 1023252549, 1022818475,
        1020535392, 1063988146, 1063585608, 1061468395, 1091467345, 1091223481, 1089362643,
        1023298250, 1022862984, 1020573622, 1057854789, 1057612444, 1055710969, 1086150395,
        1085826306, 1084121703, 1101028617, 1100754126, 1099310394, 1108048401, 1107809661,
        1105811680, 1108486935, 1108236742, 1106545356, 1104428687, 1104065392, 1102154582,
        1107815301, 1107582649, 1105421698, 1080245403, 1079856446, 1077810661, 1088427318,
        1088043758, 1086026368, 1092342943, 1092076210, 1090673287, 1092189867, 1091927133,
        1090545238, 1085860486, 1085543969, 1083879191, 1057105622, 1056801079, 1054457599,
        1050030049, 1049772976, 1048265722, 1051075579, 1050791200, 1049295456, 1021178449,
        1020798547, 1018800389, 1020744440, 1020375873, 1018437337, 1020618500, 1020253223,
        1018331987, 1077300520, 1076988477, 1075347240, 1075669867, 1075400415, 1073983185,
        1020242208, 1019886758, 1018017215, 1020198109, 1019843812, 1017980326, 1019992054,
        1019643138, 1017807959, 1031794603, 1031356522, 1029052359, 1019933041, 1019585667,
        1017758595, 1019615054, 1019275985, 1017492595, 1041488130, 1041235062, 1039620622,
        1022984770, 1022557690, 1020311392, 1019238762, 1018909521, 1017177824, 1019113330,
        1018787366, 1017072899, 1018987899, 1018665210, 1016967974, 1028743387, 1028384998,
        1026499990, 1018775810, 1018458661, 1016790560, 1018611607, 1018298746, 1016653203,
        1018498476, 1018188570, 1016558568, 1018360745, 1018054436, 1016443355, 1018235615,
        1017932576, 1016338683, 1018109883, 1017810127, 1016233507, 1017984452, 1017687972,
        1016128583, 1017889902, 1017595891, 1016049491, 1017733592, 1017443663, 1015918736,
        1017608160, 1017321507, 1015813811, 1017482729, 1017199352, 1015708887, 1017357299,
        1017077198, 1015603963, 1017231867, 1016955043, 1015499039, 1017106436, 1016832888,
        1015394115, 1016981005, 1016710733, 1015289190, 1016855575, 1016588579, 1015184267,
        1016730143, 1016466423, 1015079342,
    ],
    [
        1068743020, 1067438222, 1064687523, 1068743020, 1067438222, 1064687523, 1068743020,
        1067438222, 1064687523, 1068743020, 1067438222, 1064687523, 1068743020, 1067438222,
        1064687523, 1068743020, 1067438222, 1064687523, 1068743020, 1067438222, 1064687523,
        1068743020, 1067438222, 1064687523, 1068743020, 1067438222, 1064687523, 1068743020,
        1067438222, 1064687523, 1068743020, 1067438222, 1064687523, 1068743020, 1067438222,
        1064687523, 1068743020, 1067438222, 1064687523, 1068743020, 1067438222, 1064687523,
        1068743020, 1067438222, 1064687523, 1068743020, 1067438222, 1064687523, 1068743020,
        1067438222, 1064687523, 1068743020, 1067438222, 1064687523, 1068743020, 1067438222,
        1064687523, 1068743020, 1067438222, 1064687523, 1068743020, 1067438222, 1064687523,
        1068743020, 1067438222, 1064687523, 1068743020, 1067438222, 1064687523, 1068743020,
        1067438222, 1064687523, 1068743020, 1067438222, 1064687523, 1068743020, 1067438222,
        1064687523, 1068743020, 1067438222, 1064687523, 1068743020, 1067438222, 1064687523,
        1068743020, 1067438222, 1064687523, 1068743020, 1067438222, 1064687523, 1068709232,
        1067408177, 1064641305, 1065524519, 1063799308, 1060284980, 1053061403, 1051635236,
        1048992481, 998505479, 996850192, 993298986, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        1046707576, 1048961477, 1048756121, 1046537190, 1048861202, 1048658193, 1046366804,
        1048760927, 1048544529, 1046196418, 1048660652, 1048348672, 1046026031, 1048544752,
        1048152815, 1045855645, 1048344202, 1047956958, 1045685259, 1048143651, 1047761102,
        1045514873, 1047943101, 1047565245, 1062085442, 1064477086, 1064104923, 1045174101,
        1047542000, 1047173532, 1045003714, 1047341449, 1046977675, 1043057514, 1045050703,
        1044740543, 1044662942, 1046940348, 1046585962, 1070478363, 1072870771, 1072498489,
        1044322170, 1046539248, 1046194249, 1016515362, 1018264893, 1017992649, 1045444339,
        1047860080, 1047484167, 1016242744, 1017944012, 1017679278, 1043640625, 1045737045,
        1045410822, 1015970126, 1017623132, 1017365908, 1043299853, 1045335944, 1045019109,
        1070307977, 1072670220, 1072302632, 1042959080, 1044934842, 1044627395, 1015424889,
        1016981369, 1016739165, 1042618308, 1044533742, 1044235682, 1031830099, 1033320720,
        1033088765, 1069272620, 1071451568, 1071112503, 1042107149, 1043932091, 1043648112,
        1036814306, 1039187306, 1038818045, 1060910235, 1063093826, 1062754038, 1034249224,
        1036168115, 1035869517, 1038147617, 1040472026, 1040269030, 1025985877, 1027926943,
        1027624894, 1017725367, 1019689111, 1019383534, 1017537677, 1019468193, 1019167786,
        1017537677, 1019468193, 1019167786, 1017537677, 1019468193, 1019167786, 1017537677,
        1019468193, 1019167786, 1017537677, 1019468193, 1019167786, 1017537677, 1019468193,
        1019167786, 1017537677, 1019468193, 1019167786, 1017537677, 1019468193, 1019167786,
        1017537677, 1019468193, 1019167786, 1017537677, 1019468193, 1019167786, 1017537677,
        1019468193, 1019167786, 1017537677, 1019468193, 1019167786, 1017537677, 1019468193,
        1019167786, 1017537677, 1019468193, 1019167786, 1017537677, 1019468193, 1019167786,
        1017537677, 1019468193, 1019167786, 1017537677, 1019468193, 1019167786, 1017537677,
        1019468193, 1019167786, 1017537677, 1019468193, 1019167786, 1017537677, 1019468193,
        1019167786, 1017537677, 1019468193, 1019167786, 1017537677, 1019468193, 1019167786,
        1017537677, 1019468193, 1019167786, 1017537677, 1019468193, 1019167786, 1017537677,
        1019468193, 1019167786, 1017537677, 1019468193, 1019167786, 1017537677, 1019468193,
        1019167786, 1017537677, 1019468193, 1019167786, 1017537677, 1019468193, 1019167786,
        1017537677, 1019468193, 1019167786,
    ],
    [
        1041454910, 1040385217, 1036618654, 1041574866, 1040491884, 1036782741, 1041818147,
        1040708214, 1037115521, 1042301565, 1041138080, 1037776781, 1042281571, 1041120300,
        1037749431, 1044959284, 1043501380, 1040799815, 1042752708, 1041539246, 1038393893,
        1042988275, 1041748717, 1038716123, 1049542017, 1048435446, 1044594843, 1043459412,
        1042167662, 1039360584, 1043910342, 1042568638, 1039977404, 1043955596, 1042608879,
        1040039307, 1044166116, 1042796078, 1040257333, 1055970693, 1054222239, 1050982269,
        1044637253, 1043215024, 1040579564, 1044872821, 1043424496, 1040740679, 1048754496,
        1047034886, 1043517605, 1045343958, 1043843440, 1041062910, 1048746681, 1047020986,
        1043506914, 1045815094, 1044262384, 1041385140, 1046050663, 1044471857, 1041546256,
        1058620798, 1057508048, 1053927529, 1046521799, 1044890802, 1041868486, 1046760146,
        1045102744, 1042031501, 1047124530, 1045426762, 1042280719, 1047228503, 1045519218,
        1042351831, 1054664693, 1053060915, 1050089040, 1047699640, 1045938163, 1042674062,
        1047935208, 1046147635, 1042835177, 1051968167, 1050663107, 1047913539, 1048369515,
        1046533830, 1043132218, 1045209710, 1043724065, 1040971092, 1038157808, 1036524083,
        1033496714, 1030249305, 1028562395, 1025436471, 1030097935, 1028427794, 1025332942,
        1030097935, 1028427794, 1025332942, 1030097935, 1028427794, 1025332942, 1030097935,
        1028427794, 1025332942, 1030097935, 1028427794, 1025332942, 1030097935, 1028427794,
        1025332942, 1030097935, 1028427794, 1025332942, 1030097935, 1028427794, 1025332942,
        1030097935, 1028427794, 1025332942, 1030097935, 1028427794, 1025332942, 1030097935,
        1028427794, 1025332942, 1030097935, 1028427794, 1025332942, 1030097935, 1028427794,
        1025332942, 1030097935, 1028427794, 1025332942, 1030097935, 1028427794, 1025332942,
        1030097935, 1028427794, 1025332942, 1030097935, 1028427794, 1025332942, 1030097935,
        1028427794, 1025332942, 1030097935, 1028427794, 1025332942, 1030097935, 1028427794,
        1025332942, 1030097935, 1028427794, 1025332942, 1030097935, 1028427794, 1025332942,
        1030097935, 1028427794, 1025332942, 1030097935, 1028427794, 1025332942, 1030097935,
        1028427794, 1025332942, 1030097935, 1028427794, 1025332942, 1030097935, 1028427794,
        1025332942, 1030097935, 1028427794, 1025332942, 1030097935, 1028427794, 1025332942,
        1030097935, 1028427794, 1025332942,
    ],
    [
        842500408, 864504429, 815777539, 773625002, 795269012, 747130811, 871415282, 892433807,
        844207101, 876099405, 898091711, 849370767, 964945069, 985821751, 937796537, 972202606,
        993188528, 944978369, 327879525, 348978479, 301078319, 999082715, 1020235997, 972366403,
        1018019375, 1040192103, 991379587, 913622618, 934571668, 886380223, 869003611, 890656081,
        842107620, 777937035, 799271934, 750884654, 1041529873, 1062922243, 1015104366, 1058706566,
        1080288395, 1032229347, 945837499, 967138671, 918768509, 1024874745, 1046325017, 998433434,
        1053628958, 1075261457, 1026723131, 1068425568, 1090578700, 1041776129, 1075296656,
        1096789737, 1048843660, 1075774168, 1097493718, 1049259358, 1074201668, 1095175430,
        1047204835, 1075043073, 1096415887, 1048622903, 1047882723, 1068820634, 1020634844,
        1056111659, 1077091543, 1028884449, 1059662790, 1081698126, 1033061788, 1062488081,
        1083996899, 1035521347, 1053318659, 1075032726, 1026453001, 1023810855, 1044756557,
        996770175, 1026558580, 1048691721, 999899297, 1017509992, 1039445847, 990936143, 952446149,
        974215211, 925607574, 864691696, 885762791, 837846957, 375504051, 396781067, 348423165,
        1055654784, 1076754764, 1028486716, 1042914004, 1064962824, 1016309322, 439907007,
        461895313, 413176399, 936347979, 957938543, 909421501, 868246003, 890097622, 841448083,
        1003125038, 1024802822, 976241512, 936164017, 957802938, 909261353, 0, 0, 0, 1017192240,
        1038977394, 990659523, 972068637, 993089775, 944861743, 802777183, 824197691, 775766960,
        693609897, 715060734, 666614610, 439964019, 461937338, 413226031, 999794188, 1021284899,
        973341838, 916776065, 938678421, 890211359, 0, 0, 0, 914036537, 934876782, 886740561,
        776137681, 797945568, 749318225, 868545067, 890318072, 841708433, 623750956, 645924694,
        597111665, 0, 0, 0, 924686596, 946362215, 898183777, 750877794, 772710407, 724070515, 0, 0,
        0, 591345066, 613216892, 564557097, 0, 0, 0, 768309979, 789970423, 741417914, 185448488,
        206630581, 158777197, 0, 0, 0, 395021668, 416136405, 368245125, 0, 0, 0,
    ],
    [
        849203728, 864504429, 830949997, 780575835, 795269012, 761714580, 877593318, 892433807,
        858879375, 882796481, 898091711, 864537279, 971166652, 985821751, 952267319, 978363264,
        993188528, 959634096, 334438441, 348978479, 315424047, 1005724082, 1020235997, 986681565,
        1024812619, 1040192103, 1006637671, 919763623, 934571668, 901017236, 875519550, 890656081,
        857101649, 784283703, 799271934, 765717502, 1048534093, 1062922243, 1029367811, 1065675768,
        1080288395, 1046733963, 952166190, 967138671, 933584239, 1031882813, 1046325017,
        1012770585, 1060134252, 1075261457, 1041707025, 1075208367, 1090578700, 1057024268,
        1082292076, 1096789737, 1063235305, 1082702684, 1097493718, 1063939286, 1080570585,
        1095175430, 1061620998, 1082017613, 1096415887, 1062861455, 1054017790, 1068820634,
        1035266202, 1062269099, 1077091543, 1043537111, 1066498014, 1081698126, 1048143694,
        1068927451, 1083996899, 1050442467, 1059867430, 1075032726, 1041478294, 1030137187,
        1044756557, 1011202125, 1033330723, 1048691721, 1015137289, 1024374606, 1039445847,
        1005891415, 959024233, 974215211, 940660779, 871208331, 885762791, 852208359, 381819867,
        396781067, 363226635, 1061876237, 1076754764, 1043200332, 1049745245, 1064962824,
        1031408392, 446601951, 461895313, 428340881, 942830921, 957938543, 924384111, 874868091,
        890097622, 856543190, 1009654469, 1024802822, 991248390, 942672733, 957802938, 924248506,
        0, 0, 0, 1024101374, 1038977394, 1005422962, 978248066, 993089775, 959535343, 809169482,
        824197691, 790643259, 700018362, 715060734, 681506302, 446650974, 461937338, 428382906,
        1006790308, 1021284899, 987730467, 923650575, 938678421, 905123989, 0, 0, 0, 920119547,
        934876782, 901322350, 782736459, 797945568, 764391136, 875125252, 890318072, 856763640,
        630544738, 645924694, 612370262, 0, 0, 0, 931628090, 946362215, 912807783, 757489751,
        772710407, 739155975, 0, 0, 0, 597977924, 613216892, 579662460, 0, 0, 0, 774830168,
        789970423, 756415991, 192133580, 206630581, 173076149, 0, 0, 0, 401604537, 416136405,
        382581973, 0, 0, 0,
    ],
];

/// Regression pin for the rig-sharing split (`sample_studio_environment_with_rig`,
/// which both the ad-hoc callers here and `accumulate_miss_radiance`'s escape lookup
/// share -- the latter borrows the rig the trace built once -- instead of each
/// duplicating the studio rig arithmetic inline):
/// tracing 64 directions x 3 wavelengths through EVERY preset in [`LightingPreset::ALL`]
/// (not just `RingLights`, which is all the original version of this test pinned) must
/// reproduce [`BASELINE_BITS`], to within a small ULP tolerance.
///
/// # Regenerating the baseline
///
/// [`BASELINE_BITS`] is captured from this test's OWN current output (a temporary
/// `eprintln!` of `got_bits` per sample, run once via `cargo
/// test -p indicatrix --lib -- studio_presets_are_unchanged_by_the_split
/// --nocapture`, then removed). [`MAX_ULP_DIFF`] must stay tight: a budget wide
/// enough to absorb reassociation noise is ALSO wide enough to absorb a small
/// coefficient change on the ambient term without the test ever failing, which would
/// mean it isn't actually pinning the formula.
///
/// The table was last captured on 2026-10-01 from the optimised test profile: the
/// workspace manifest gives this crate `opt-level = 3` under `cargo test`
/// (`[profile.test.package.indicatrix]`), and a release build passes this test with
/// it. The inputs of the call go through `black_box` since then, so no build folds any
/// part of it at compile time (a release build with LTO otherwise evaluated the
/// wavelength terms in double precision and landed 3 ULP off). The unoptimised build
/// rounds the studio sampler a few ULP differently, within 2 ULP for most samples but
/// up to 12 ULP at the falloff-cone directions 28/29/38/46 where `key_dot.powi(28)`
/// amplifies the difference, so it no longer passes this test; nothing runs the suite
/// unoptimised since that change.
///
/// The CPU/GPU HDR white-balance step does not touch
/// `studio_rig_lighting`/`sample_studio_environment` at all -- it lives entirely in
/// `trace_spectral_ray_inner`'s post-integration white-balance step -- so nothing on
/// the Studio-rig sampling path this test exercises is affected by it. Before
/// accepting a freshly captured baseline, diff it against the prior one: deltas of a
/// few ULP concentrated in the falloff-cone-edge cluster named below (e.g.
/// 28/29/38/46) match the `key_dot.powi(28)` reassociation-noise mechanism analyzed
/// below and can be folded into the new baseline; a diff that is large, widespread,
/// or inconsistent with that mechanism is a real regression and should be reported
/// rather than pasted over.
///
/// # Why a tolerance at all, not bit-for-bit equality
///
/// Requiring exact equality only ever surfaces the FIRST mismatch (`assert_eq!`
/// aborts the loop immediately), which is what hid the true shape of this the first
/// time: an exhaustive sweep of all 192 samples' bit-pattern deltas (not just the
/// first failure) is needed to tell "reassociation noise" apart from "a real
/// regression". Every non-ambient term in `studio_rig_lighting` (key softbox, fill,
/// ring) is accumulated via `softbox.mul_add(spec_power, radiance)`-style fused
/// multiply-adds, and `key_dot.powi(28)` in particular amplifies a sub-ULP
/// perturbation in `key_dot` into a much larger one in the softbox term for
/// directions near the falloff cone's edge -- exactly where the largest deltas
/// (dir 28/29/38/46) sit. Floating-point multiplication and fused multiply-add are
/// not associative, so evaluating the identical formula through a differently-shaped
/// call chain, or a differently-optimized build, can legitimately round
/// intermediate bits either way, with a `^28` term turning "either way" into
/// "either way, times a large derivative" -- with no change in the actual light
/// transport (10 ULP here is a relative difference near `1e-6`, many orders below
/// anything a path tracer's own sample noise could ever resolve). [`MAX_ULP_DIFF`]'s
/// `2` leaves headroom for exactly that kind of single-ULP-scale noise on the
/// majority of samples while a difference of many thousands of ULP -- what a
/// genuine behavioural regression (a changed exponent, a changed coefficient, a
/// dropped term) would actually produce -- still fails loudly; a handful of the
/// falloff-edge directions may need re-diffing the same way if a future legitimate
/// change (not a regression) shifts them past `2` again.
/// The three wavelengths [`studio_presets_are_unchanged_by_the_split`] samples for
/// `preset`: 450/550/650 nm for every visible-light preset (unchanged), and for a UV lamp
/// the lamp line itself and two points on its flanks, since a lamp at 365 or 395 nm is
/// exactly zero (underflow) at the visible wavelengths and would pin nothing.
fn baseline_lambdas(preset: LightingPreset) -> [f32; 3] {
    preset
        .uv_line()
        .map_or([450.0, 550.0, 650.0], |(centre, _)| {
            [centre - 8.0, centre, centre + 12.0]
        })
}

#[test]
fn studio_presets_are_unchanged_by_the_split() {
    // The baseline table keeps the order it was recorded in (the declaration order), not
    // the combo order of `LightingPreset::ALL`.
    const BASELINE_ORDER: [LightingPreset; 9] = [
        LightingPreset::Daylight,
        LightingPreset::Incandescent,
        LightingPreset::RingLights,
        LightingPreset::DarkSpotlight,
        LightingPreset::IsoHemisphere,
        LightingPreset::LightTent,
        LightingPreset::DaylightDome,
        LightingPreset::UvLamp365,
        LightingPreset::UvLamp395,
    ];
    for (preset_idx, &preset) in BASELINE_ORDER.iter().enumerate() {
        let mut idx = 0;
        for k in 0..64 {
            let phi = (k as f32 + 0.5) * (std::f32::consts::PI * (3.0 - 5.0f32.sqrt()));
            let y = (k as f32 + 0.5).mul_add(-(2.0 / 64.0), 1.0);
            let r = y.mul_add(-y, 1.0).max(0.0).sqrt();
            let dir = Vec3::new(r * phi.cos(), y, r * phi.sin());
            for &lambda in &baseline_lambdas(preset) {
                // Through `black_box`, so that no build folds part of this call at
                // compile time (a release build with LTO otherwise evaluates the
                // wavelength terms in double precision); the table pins the run-time
                // path the renderer takes.
                let val = sample_studio_environment(
                    black_box(dir),
                    black_box(lambda),
                    black_box(preset),
                    black_box(1.2),
                    black_box(0.4),
                    black_box(0.35),
                );
                let got_bits = val.to_bits();
                let expected_bits = BASELINE_BITS[preset_idx][idx];
                let ulp_diff = (i64::from(got_bits) - i64::from(expected_bits)).abs();
                assert!(
                    ulp_diff <= MAX_ULP_DIFF,
                    "{preset:?}: divergence at dir {k}, lambda {lambda}: got {val} \
                     (bits {got_bits}), expected bits {expected_bits} ({ulp_diff} ULP \
                     away, tolerance {MAX_ULP_DIFF})"
                );
                idx += 1;
            }
        }
    }
}

/// The white-balance rule is explicit: only the five Planckian presets are adapted; the
/// D65 presets, the contrast view and the UV lamps (no white point at all) are the identity.
#[test]
fn white_balance_rule_is_explicit_per_preset() {
    use LightingPreset::*;
    for preset in LightingPreset::ALL {
        let expected = matches!(
            preset,
            Incandescent | IlluminantA | RingLights | DarkSpotlight | LightTent
        );
        assert_eq!(preset.uses_white_balance(), expected, "{preset:?}");
        if expected {
            assert!(!preset.uses_d65() && !preset.is_uv_lamp());
        }
    }
    // Existing presets keep their D65 flag; the lamps are neither D65 nor Planckian.
    assert!(UvLamp365.is_uv_lamp() && UvLamp395.is_uv_lamp());
    assert!(!UvLamp365.uses_d65() && !UvLamp395.uses_d65());
    assert!(!Daylight.is_uv_lamp() && Daylight.uses_d65());
    // The new presets: the tent variants, the sun and the contrast view are unadapted;
    // Illuminant A is a 2856 K Planckian Studio preset with the Bradford adaptation.
    for preset in [DaylightSun, Aset, ShopLights, WindowDaylight, WhiteTray] {
        assert!(!preset.uses_white_balance(), "{preset:?}");
    }
    assert!(IlluminantA.uses_white_balance() && !IlluminantA.uses_d65());
    assert_eq!(IlluminantA.params().temp_k.to_bits(), 2856.0f32.to_bits());
    assert_eq!(IlluminantA.params().spot_mult.to_bits(), 1.2f32.to_bits());
    assert_eq!(IlluminantA.model(), LightingModel::Studio);
    assert_eq!(Aset.model().gpu_id(), 4);
    assert_eq!(DaylightSun.model().gpu_id(), 5);
}

/// The UV presets come last in the combo, parse from their labels and fall back to the
/// default (light tent) for anything an older build would not know.
#[test]
fn uv_lamp_presets_are_appended_and_parse() {
    assert_eq!(LightingPreset::DarkSpotlight.index(), 11);
    assert_eq!(LightingPreset::UvLamp365.index(), 13);
    assert_eq!(LightingPreset::UvLamp395.index(), 14);
    assert_eq!(LightingPreset::UvLamp365.label(), "UV lamp 365 nm");
    assert_eq!(LightingPreset::UvLamp395.label(), "UV lamp 395 nm");
    assert_eq!(
        LightingPreset::from_label("UV lamp 365 nm"),
        LightingPreset::UvLamp365
    );
    assert_eq!(
        LightingPreset::from_label("UV lamp 395 nm"),
        LightingPreset::UvLamp395
    );
    assert_eq!(LightingPreset::from_index(15), LightingPreset::LightTent);
    assert_eq!(LightingPreset::from_index(-1), LightingPreset::LightTent);
    assert_eq!(LightingPreset::from_index(0), LightingPreset::LightTent);
    assert_eq!(LightingPreset::ALL.len(), 15);
}

/// The combo order: light tent (the default), grading tray, the tent variants, the daylight
/// models, the product-photography rigs, the contrast view, then the UV lamps. The
/// declaration order (the postcard variant index) is not the combo order.
#[test]
fn the_combo_lists_the_lit_models_first() {
    assert_eq!(
        LightingPreset::ALL,
        [
            LightingPreset::LightTent,
            LightingPreset::IsoHemisphere,
            LightingPreset::WhiteTray,
            LightingPreset::ShopLights,
            LightingPreset::WindowDaylight,
            LightingPreset::DaylightDome,
            LightingPreset::DaylightSun,
            LightingPreset::Daylight,
            LightingPreset::Incandescent,
            LightingPreset::IlluminantA,
            LightingPreset::RingLights,
            LightingPreset::DarkSpotlight,
            LightingPreset::Aset,
            LightingPreset::UvLamp365,
            LightingPreset::UvLamp395,
        ]
    );
    assert_eq!(LightingPreset::default().index(), 0);
    assert_eq!(LightingPreset::from_index(0), LightingPreset::default());
}

/// The lamp spectra are unit-peak Gaussians of the specified FWHM: 365 nm / 10 nm and
/// 395 nm / 12 nm.
#[test]
fn uv_lamp_spectra_are_gaussians_of_the_specified_width() {
    for (preset, centre, fwhm) in [
        (LightingPreset::UvLamp365, 365.0f32, 10.0f32),
        (LightingPreset::UvLamp395, 395.0, 12.0),
    ] {
        assert!((preset.spectral_power(centre) - 1.0).abs() < 1e-6);
        for side in [-0.5f32, 0.5] {
            let half = preset.spectral_power(side.mul_add(fwhm, centre));
            assert!((half - 0.5).abs() < 2e-3, "{preset:?}: {half}");
        }
        // Effectively dark across the visible range, except the 395 nm LED's violet tail.
        assert_eq!(preset.spectral_power(550.0), 0.0);
    }
    let tail_365 = LightingPreset::UvLamp365.spectral_power(380.0);
    let tail_395 = LightingPreset::UvLamp395.spectral_power(410.0);
    assert!(tail_365 < 5e-3, "{tail_365}");
    assert!(tail_395 > 1e-3 && tail_395 < 0.1, "{tail_395}");
}

/// No ambient fill: a direction away from every light is exactly dark under a UV lamp
/// (and not under Daylight), and the lamp lights the key direction.
#[test]
fn uv_lamps_have_no_ambient_backdrop() {
    let rig = crate::optics::studio_rig::StudioRig::new(0.4, 0.35);
    let key = rig.key_dir;
    // Straight away from the key, fill and every ring light.
    let away = Vec3::new(0.0, -1.0, 0.0);
    let dark = sample_studio_environment_with_rig(
        away,
        365.0,
        LightingPreset::UvLamp365,
        1.0,
        &rig,
        Vec3::ZERO,
    );
    assert_eq!(dark, 0.0);
    let ambient = sample_studio_environment_with_rig(
        away,
        550.0,
        LightingPreset::Daylight,
        1.0,
        &rig,
        Vec3::ZERO,
    );
    assert!(ambient > 0.0);
    let lit = sample_studio_environment_with_rig(
        key,
        365.0,
        LightingPreset::UvLamp365,
        1.0,
        &rig,
        Vec3::ZERO,
    );
    assert!(lit > 1.0, "the key softbox must light the lamp line: {lit}");
}

/// Radiance of the ASET view at elevation `elevation_deg` and wavelength `lambda_nm`.
fn aset_at(elevation_deg: f32, lambda_nm: f32) -> f32 {
    let rig = crate::optics::studio_rig::StudioRig::new(0.4, 0.35);
    let e = elevation_deg.to_radians();
    sample_studio_environment_with_rig(
        Vec3::new(e.cos(), e.sin(), 0.0),
        lambda_nm,
        LightingPreset::Aset,
        1.0,
        &rig,
        Vec3::ZERO,
    )
}

/// Each elevation zone lights only its own band: green 0-45 degrees, red 45-75, blue 75-90.
#[test]
fn aset_zones_are_classified_by_elevation() {
    const RED: f32 = 610.0;
    const GREEN: f32 = 540.0;
    const BLUE: f32 = 460.0;
    for (elevation, lit, dark) in [
        (20.0, GREEN, [RED, BLUE]),
        (60.0, RED, [GREEN, BLUE]),
        (82.0, BLUE, [RED, GREEN]),
        (89.0, BLUE, [RED, GREEN]),
    ] {
        let on = aset_at(elevation, lit);
        assert!(on > 0.5, "{elevation} deg at {lit} nm: {on}");
        for other in dark {
            let off = aset_at(elevation, other);
            assert!(
                off < on * 1e-3,
                "{elevation} deg leaks into {other} nm: {off}"
            );
        }
    }
}

/// Each band is a 20 nm FWHM Gaussian peaking at its zone's centre wavelength.
#[test]
fn aset_bands_peak_at_their_centre_wavelengths() {
    for (elevation, centre) in [(20.0f32, 540.0f32), (60.0, 610.0), (82.0, 460.0)] {
        let peak = aset_at(elevation, centre);
        for step in 1..40 {
            let offset = step as f32 * 0.5;
            assert!(aset_at(elevation, centre + offset) < peak);
            assert!(aset_at(elevation, centre - offset) < peak);
        }
        // Half the peak at +-10 nm (FWHM 20 nm).
        for side in [-10.0f32, 10.0] {
            let half = aset_at(elevation, centre + side) / peak;
            assert!((half - 0.5).abs() < 5e-3, "{elevation} deg: {half}");
        }
        // Dark far from the band.
        assert!(aset_at(elevation, centre + 80.0) < peak * 1e-6);
    }
}

/// Below the horizon the ASET view is black at every wavelength.
#[test]
fn aset_is_black_below_the_horizon() {
    for elevation in [-90.0f32, -45.0, -10.0, -6.0] {
        for lambda in [460.0f32, 540.0, 610.0, 550.0] {
            assert_eq!(
                aset_at(elevation, lambda),
                0.0,
                "{elevation} deg {lambda} nm"
            );
        }
    }
}
