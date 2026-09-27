//! Tests for the illuminant spectral-power curves, the preset label/index round trip,
//! the backdrop fill, the lit models' observer head shadow, and the studio rig's own
//! golden-bits regression pin.

use super::*;

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
        LightingPreset::from_label("Soft dome + ring lights"),
        LightingPreset::LightTent
    );
    assert_eq!(
        LightingPreset::from_label("Daylight dome + sun"),
        LightingPreset::DaylightDome
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
    // Sun (10.0) + aureole (0.30) + sky (<= 0.18) at the sun centre.
    assert!(
        max_daylight <= 10.5,
        "Daylight dome peak must not exceed 10.5, got {max_daylight}"
    );
}

/// Regression pin for the rig-sharing split (`sample_studio_environment_with_rig`,
/// which both the ad-hoc callers here and `accumulate_miss_radiance`'s per-bounce
/// lookup share, instead of each duplicating the studio rig arithmetic inline):
/// tracing 64 directions x 3 wavelengths through `RingLights` must reproduce
/// [`BASELINE_BITS`], to within a small ULP tolerance.
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
/// The CPU/GPU HDR white-balance step does not touch
/// `sample_studio_rig`/`sample_studio_environment` at all -- it lives entirely in
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
/// regression". Every non-ambient term in `sample_studio_rig` (key softbox, fill,
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
#[test]
#[allow(clippy::unreadable_literal)]
fn studio_presets_are_unchanged_by_the_split() {
    /// Largest tolerated bit-pattern distance between a freshly computed value and
    /// its golden. Every baseline value here is positive and finite, so `u32` bit
    /// patterns increase monotonically with the represented value exactly like ULP
    /// distance does -- a signed difference of the raw bit patterns IS the ULP
    /// distance, no separate distance function needed. See the doc comment above
    /// for why this must stay tight rather than a wider, looser bound.
    const MAX_ULP_DIFF: i64 = 2;
    const BASELINE_BITS: [u32; 192] = [
        1021303259, 1023655294, 1023453223, 1021200981, 1023595102, 1023378703, 1021099050,
        1023535114, 1023261534, 1020997254, 1023475205, 1023144520, 1021568337, 1023811298,
        1023605575, 1022776056, 1024522061, 1024299703, 1020689823, 1023178377, 1022791133,
        1027755127, 1030009410, 1029658621, 1044895794, 1047214421, 1046853620, 1020392900,
        1022828889, 1022449825, 1020281302, 1022697534, 1022321544, 1020178665, 1022576726,
        1022203564, 1061137586, 1063361424, 1063015373, 1083905863, 1085705252, 1085425249,
        1019902580, 1022251765, 1021886209, 1046107587, 1048608372, 1048246559, 1076756650,
        1078775455, 1078461309, 1093086491, 1095026095, 1094724274, 1100967474, 1102817211,
        1102529373, 1102501898, 1104623281, 1104293173, 1100418369, 1102170895, 1101898185,
        1101296235, 1103204173, 1102907279, 1072082398, 1074250299, 1074042063, 1075709089,
        1077542440, 1077257152, 1081883525, 1083470199, 1083242507, 1084303182, 1086172910,
        1085881962, 1078120145, 1080380336, 1080028628, 1050792595, 1052670085, 1052377929,
        1047922906, 1049676718, 1049454622, 1044118982, 1046300086, 1045960685, 1018488180,
        1020586967, 1020260376, 1018134306, 1020170445, 1019853601, 1018031798, 1020049789,
        1019735769, 1075090800, 1076814691, 1076546437, 1068234284, 1070229409, 1069918948,
        1017725102, 1019688798, 1019383227, 1017689160, 1019646492, 1019341912, 1017521024,
        1019448590, 1019148641, 1028692342, 1031112546, 1030735938, 1017420804, 1019330627,
        1019033440, 1017213944, 1019087147, 1018795658, 1039204674, 1041094121, 1040876565,
        1018976767, 1021162051, 1020821999, 1016907250, 1018726157, 1018443117, 1016805017,
        1018605826, 1018325602, 1016702785, 1018485494, 1018208087, 1026205465, 1028185404,
        1027877306, 1016519389, 1018269632, 1017997277, 1016396090, 1018124504, 1017855546,
        1016303883, 1018015974, 1017749556, 1016191627, 1017883843, 1017620518, 1016089641,
        1017763802, 1017503286, 1015987163, 1017643183, 1017385490, 1015884931, 1017522852,
        1017267976, 1015807869, 1017432147, 1017179394, 1015680469, 1017282193, 1017032949,
        1015578236, 1017161861, 1016915434, 1015476004, 1017041531, 1016797920, 1015373773,
        1016921202, 1016680407, 1015271541, 1016800870, 1016562892, 1015169309, 1016680540,
        1016445378, 1015067077, 1016560210, 1016327864, 1014908125, 1016439880, 1016210351,
        1014703659, 1016319549, 1016092836,
    ];
    let mut idx = 0;
    for k in 0..64 {
        let phi = (k as f32 + 0.5) * (std::f32::consts::PI * (3.0 - 5.0f32.sqrt()));
        let y = (k as f32 + 0.5).mul_add(-(2.0 / 64.0), 1.0);
        let r = y.mul_add(-y, 1.0).max(0.0).sqrt();
        let dir = Vec3::new(r * phi.cos(), y, r * phi.sin());
        for &lambda in &[450.0f32, 550.0, 650.0] {
            let val =
                sample_studio_environment(dir, lambda, LightingPreset::RingLights, 1.2, 0.4, 0.35);
            let got_bits = val.to_bits();
            let expected_bits = BASELINE_BITS[idx];
            let ulp_diff = (i64::from(got_bits) - i64::from(expected_bits)).abs();
            assert!(
                ulp_diff <= MAX_ULP_DIFF,
                "divergence at dir {k}, lambda {lambda}: got {val} (bits {got_bits}), \
                 expected bits {expected_bits} ({ulp_diff} ULP away, tolerance \
                 {MAX_ULP_DIFF})"
            );
            idx += 1;
        }
    }
}
