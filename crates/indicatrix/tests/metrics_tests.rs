use indicatrix::{
    color::metrics::{
        PROFILE_ANGLES_DEG, TILT_ANGLES_DEG, camera_view_basis,
        evaluate_angular_profile_at_azimuth, evaluate_full_axis_profile_at_azimuth,
        evaluate_gem_optical_metrics,
    },
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, LightingPreset},
    },
};

/// The light pose the metrics tests score under unless a test varies it: the editor's
/// default preset at the canonical key-light pose.
const fn studio() -> EnvironmentSource<'static> {
    LightingPreset::RingLights.studio(1.0, 0.85, 0.95)
}

/// The tests that compare two stones or two materials look at several camera poses rather
/// than one, so a single pose where both readings sit on the same floor cannot decide them.
const COMPARISON_POSES: [(f32, f32); 4] = [(0.0, 1.4), (0.0, 0.45), (0.6, 0.9), (0.8, 0.05)];

/// The gemological metrics evaluate rays from an observer `PoV` basis that must exactly
/// match the real render camera's basis (`Camera::new`), including the `world_up`
/// fallback threshold and axis used near the poles. A mismatch there means
/// `evaluate_gem_optical_metrics` / `evaluate_angular_profile` score a differently-rolled
/// frame than what is actually rendered, most visibly at steep camera tilt.
fn assert_bases_match(yaw: f32, pitch: f32) {
    let camera = Camera::new(yaw, pitch, 2.4, 42.0);
    let (forward, right, up) = camera_view_basis(yaw, pitch);

    let eps = 1e-4;
    assert!(
        (camera.forward - forward).length() < eps,
        "forward mismatch at yaw={yaw}, pitch={pitch}: camera={:?} metrics={:?}",
        camera.forward,
        forward
    );
    assert!(
        (camera.right - right).length() < eps,
        "right mismatch at yaw={yaw}, pitch={pitch}: camera={:?} metrics={:?}",
        camera.right,
        right
    );
    assert!(
        (camera.up - up).length() < eps,
        "up mismatch at yaw={yaw}, pitch={pitch}: camera={:?} metrics={:?}",
        camera.up,
        up
    );
}

#[test]
fn metrics_camera_basis_matches_render_camera_at_shallow_and_typical_angles() {
    for &yaw in &[0.0f32, 0.6, 1.5, -1.2] {
        assert_bases_match(yaw, 0.0);
        assert_bases_match(yaw, 45f32.to_radians());
    }
}

#[test]
fn metrics_camera_basis_matches_render_camera_near_the_pole() {
    // The user's own camera clamps pitch at roughly +/-1.48 rad (~85 deg), and
    // evaluate_angular_profile sweeps all the way to 90 deg, so both the steep-but-clamped
    // case and the exact-pole case must agree between the metrics basis and the render camera.
    for &yaw in &[0.0f32, 0.6, 2.1] {
        assert_bases_match(yaw, 85f32.to_radians());
        assert_bases_match(yaw, 90f32.to_radians());
        assert_bases_match(yaw, -90f32.to_radians());
    }
}

#[test]
fn metrics_camera_basis_matches_render_camera_past_the_pole() {
    // Past +/-90 deg of pitch the render camera switches its `world_up` so the image does
    // not mirror; the metrics basis is that same camera's basis and must follow.
    for &yaw in &[0.0f32, 0.6, 2.1] {
        assert_bases_match(yaw, 100f32.to_radians());
        assert_bases_match(yaw, 135f32.to_radians());
        assert_bases_match(yaw, -110f32.to_radians());
    }
}

#[test]
fn metrics_camera_basis_right_up_forward_are_orthonormal() {
    // Sanity check on the shared basis helper itself: regardless of the world_up branch
    // taken, the result should always be an orthonormal right-handed-ish basis.
    for &pitch_deg in &[0.0f32, 5.0, 45.0, 85.0, 90.0] {
        let (forward, right, up) = camera_view_basis(0.3, pitch_deg.to_radians());
        assert!((forward.length() - 1.0).abs() < 1e-4);
        assert!((right.length() - 1.0).abs() < 1e-4);
        assert!((up.length() - 1.0).abs() < 1e-4);
        assert!(forward.dot(right).abs() < 1e-3);
        assert!(forward.dot(up).abs() < 1e-3);
        assert!(right.dot(up).abs() < 1e-3);
    }
}

// ---------------------------------------------------------------------------------------
// Fire and Scintillation: these are genuinely measured from the traced facet geometry
// (not closed-form fits), so both must respond to the *cut*, not just the material.
// See src/color/metrics/mod.rs for the measurement methodology. The assertions below are
// orderings and physical ranges, not pinned readings: the readings move with the lighting
// model and the fan scale, the orderings are what the physics fixes.
// ---------------------------------------------------------------------------------------

#[test]
fn fire_differs_between_two_cuts_of_the_same_material() {
    // Under the old formula ((n_f - n_c) * 1800.0), fire_index depends ONLY on the
    // material's dispersion, so every cut of a given material reports byte-identical
    // fire. The measurement traces the actual facet geometry, so a genuinely different
    // cut of the SAME material (round brilliant vs. emerald/step cut) must report a
    // different fire_index at some pose.
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();
    let diamond = GemMaterial::diamond();

    let differing_poses = COMPARISON_POSES
        .iter()
        .filter(|&&(yaw, pitch)| {
            let m_srb = evaluate_gem_optical_metrics(&srb, &diamond, yaw, pitch, studio());
            let m_ec = evaluate_gem_optical_metrics(&ec, &diamond, yaw, pitch, studio());
            println!(
                "diamond fire_index at yaw {yaw} pitch {pitch}: SRB={:.3} emerald_cut={:.3}",
                m_srb.fire_index, m_ec.fire_index
            );
            m_srb.fire_index.to_bits() != m_ec.fire_index.to_bits()
        })
        .count();
    assert!(
        differing_poses > 0,
        "fire_index must differ between the round brilliant and the emerald cut for the same \
         material at some pose -- the old closed-form formula could never do this"
    );
}

#[test]
fn fire_is_higher_for_a_high_dispersion_material_than_a_low_dispersion_one() {
    // Holding the cut fixed, a material with genuinely higher dispersion (larger n_F -
    // n_C) must still measure higher Fire than a low-dispersion material -- the cut-vs-
    // material effects should not scramble this ordering for a clearly-separated pair.
    let srb = StandardGemCuts::standard_round_brilliant();
    // NOTE: deliberately NOT using `GemMaterial::by_name("Cubic Zirconia")` here --
    // `by_name`'s substring fallback (`name.to_lowercase().contains(&m.name.to_lowercase())`)
    // matches "Zircon" as a substring of "cubic zirconia" before it ever reaches the
    // "Cubic Zirconia" entry later in `all_materials()`, so `by_name("Cubic Zirconia")`
    // silently returns Zircon instead. Worked around here with an exact-name lookup so
    // this test asserts what it says it asserts.
    let cz = GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name == "Cubic Zirconia")
        .unwrap();
    let quartz = GemMaterial::by_name("Quartz").unwrap();

    let (mut cz_total, mut quartz_total) = (0.0f32, 0.0f32);
    for &(yaw, pitch) in &COMPARISON_POSES {
        cz_total += evaluate_gem_optical_metrics(&srb, &cz, yaw, pitch, studio()).fire_index;
        quartz_total +=
            evaluate_gem_optical_metrics(&srb, &quartz, yaw, pitch, studio()).fire_index;
    }
    println!(
        "SRB fire_index summed over poses: Cubic Zirconia={cz_total:.3} Quartz={quartz_total:.3}"
    );
    assert!(
        cz_total > quartz_total,
        "Cubic Zirconia (high dispersion, {cz_total:.3}) must measure higher fire than Quartz \
         (low dispersion, {quartz_total:.3}) on the same cut"
    );
}

#[test]
fn scintillation_differs_between_two_cuts_of_the_same_material() {
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();
    let diamond = GemMaterial::diamond();

    let differing_poses = COMPARISON_POSES
        .iter()
        .filter(|&&(yaw, pitch)| {
            let m_srb = evaluate_gem_optical_metrics(&srb, &diamond, yaw, pitch, studio());
            let m_ec = evaluate_gem_optical_metrics(&ec, &diamond, yaw, pitch, studio());
            println!(
                "diamond scintillation_pct at yaw {yaw} pitch {pitch}: SRB={:.3} emerald_cut={:.3}",
                m_srb.scintillation_pct, m_ec.scintillation_pct
            );
            m_srb.scintillation_pct.to_bits() != m_ec.scintillation_pct.to_bits()
        })
        .count();
    assert!(
        differing_poses > 0,
        "scintillation_pct must differ between the round brilliant and the emerald cut for \
         the same material at some pose"
    );
}

#[test]
fn the_metrics_follow_the_lighting_but_windowing_does_not() {
    // Windowing is decided before any light is consulted -- a ray that leaks through the
    // pavilion leaks under every lighting -- so it is bit-identical across presets. The ISO
    // hemisphere radiates into every upward direction, so whatever a studio rig counts as
    // a returned ray the ISO hemisphere counts too (same head shadow, same traced rays):
    // brilliance can only be higher and extinction lower under it. Every share is a share
    // of the rays that hit the stone, so together they cannot exceed the whole.
    let srb = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    let iso = LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95);

    for &(yaw, pitch) in &COMPARISON_POSES {
        let rig = evaluate_gem_optical_metrics(&srb, &diamond, yaw, pitch, studio());
        let uniform = evaluate_gem_optical_metrics(&srb, &diamond, yaw, pitch, iso);

        assert_eq!(
            rig.windowing_pct.to_bits(),
            uniform.windowing_pct.to_bits(),
            "windowing must not depend on the lighting (yaw {yaw}, pitch {pitch})"
        );
        assert!(
            uniform.brilliance_pct >= rig.brilliance_pct,
            "the ISO hemisphere ({:.3}) must return at least what the studio rig ({:.3}) does \
             (yaw {yaw}, pitch {pitch})",
            uniform.brilliance_pct,
            rig.brilliance_pct
        );
        assert!(
            uniform.extinction_pct <= rig.extinction_pct,
            "the ISO hemisphere ({:.3}) must extinguish no more than the studio rig ({:.3}) \
             (yaw {yaw}, pitch {pitch})",
            uniform.extinction_pct,
            rig.extinction_pct
        );
        for m in [rig, uniform] {
            assert!(
                m.brilliance_pct + m.extinction_pct + m.windowing_pct <= 100.0 + 1e-3,
                "brilliance, extinction and windowing are shares of one whole (got {:.3})",
                m.brilliance_pct + m.extinction_pct + m.windowing_pct
            );
        }
    }
}

#[test]
fn a_different_lighting_preset_changes_what_is_returned() {
    // The preset is part of what the metrics describe: the ring-lights rig and the light
    // tent light the same stone from different sources, so at some pose they must not
    // agree on how much light is returned.
    let srb = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    let tent = LightingPreset::LightTent.studio(1.0, 0.85, 0.95);

    let differing_poses = COMPARISON_POSES
        .iter()
        .filter(|&&(yaw, pitch)| {
            let ring = evaluate_gem_optical_metrics(&srb, &diamond, yaw, pitch, studio());
            let in_tent = evaluate_gem_optical_metrics(&srb, &diamond, yaw, pitch, tent);
            ring.brilliance_pct.to_bits() != in_tent.brilliance_pct.to_bits()
        })
        .count();
    assert!(
        differing_poses > 0,
        "brilliance must depend on the lighting preset at some pose"
    );
}

/// Ceiling used by the `cv / (1 + cv)` squash: `scintillation_pct` is mathematically
/// bounded strictly below 100 (see the mapping comment in `scintillation.rs`), but f32
/// rounding of a very large CV could in principle land a printed value at 100.0 without a
/// margin. Tests below require staying comfortably clear of the ceiling, not just short of
/// exact equality.
const SCINTILLATION_CEILING_MARGIN: f32 = 0.5;

#[test]
fn scintillation_is_not_a_pure_function_of_windowing_and_extinction() {
    // A pure arithmetic function of windowing_pct and extinction_pct (for example
    // 100.0 - (windowing_pct * 0.6 + extinction_pct * 0.4)) would carry no independent
    // information. Scintillation depends on *where* the light returns from, not just how
    // much, so across a spread of stones and poses it must depart from any such formula
    // for at least one of them -- while staying clear of the display ceiling, which would
    // make the departure vacuous (a saturated 100 differs from everything below it).
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();
    let cz = GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name == "Cubic Zirconia")
        .unwrap();

    let mut departures = 0;
    for (cut_name, planes) in [("standard_round_brilliant", &srb), ("emerald_cut", &ec)] {
        for &(yaw, pitch) in &COMPARISON_POSES {
            let m = evaluate_gem_optical_metrics(planes, &cz, yaw, pitch, studio());
            println!(
                "{cut_name} Cubic Zirconia at yaw {yaw} pitch {pitch}: windowing={:.3} \
                 extinction={:.3} scintillation={:.3}",
                m.windowing_pct, m.extinction_pct, m.scintillation_pct
            );
            assert!(
                m.scintillation_pct < 100.0 - SCINTILLATION_CEILING_MARGIN,
                "{cut_name}: scintillation_pct must not be saturated at the ceiling, got {:.3}",
                m.scintillation_pct
            );
            let formula =
                0.4f32.mul_add(-m.extinction_pct, 0.6f32.mul_add(-m.windowing_pct, 100.0));
            if (m.scintillation_pct - formula).abs() > 1.0 {
                departures += 1;
            }
        }
    }
    assert!(
        departures > 0,
        "scintillation_pct tracks a fixed function of windowing and extinction at every pose \
         -- it would carry no information of its own"
    );
}

#[test]
fn no_built_in_material_saturates_scintillation_on_either_cut() {
    // The display mapping `cv / (1 + cv)` is a monotone bijection from [0, inf) onto
    // [0, 1) -- it approaches 100% asymptotically but can never reach it. This test pins
    // that property against every built-in material on both cuts at a representative
    // viewing/lighting configuration, guarding against a straight `clamp(cv * 100, 0,
    // 100)` mapping, which pinned most material/cut combinations at exactly 100.0,
    // collapsing the metric's ability to tell most stones apart.
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();

    let mut min_seen = f32::MAX;
    let mut max_seen = f32::MIN;

    for material in GemMaterial::all_materials() {
        for (cut_name, planes) in [("standard_round_brilliant", &srb), ("emerald_cut", &ec)] {
            // Representative viewing/lighting angle matching the front-end's default
            // render pose (camera yaw 0.60, pitch 0.45; light yaw 0.85, pitch 0.95).
            let m = evaluate_gem_optical_metrics(planes, &material, 0.60, 0.45, studio());

            assert!(
                (m.scintillation_pct - 100.0).abs() > 0.01,
                "{} on {}: scintillation_pct saturated at the ceiling ({:.5}); the CV -> \
                 display mapping should never reach exactly 100%",
                material.name,
                cut_name,
                m.scintillation_pct
            );

            min_seen = min_seen.min(m.scintillation_pct);
            max_seen = max_seen.max(m.scintillation_pct);
        }
    }

    println!(
        "scintillation_pct spread across all built-in materials x both cuts: min={min_seen:.3} max={max_seen:.3}"
    );
    // The metric must tell stones apart: not every material/cut pair may read alike.
    assert!(
        max_seen > min_seen,
        "scintillation_pct is identical ({min_seen:.3}) for every built-in material on both \
         cuts; the metric should discriminate between different stones"
    );
}

#[test]
fn fire_and_scintillation_are_finite_and_in_range_for_every_material_on_both_cuts() {
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();

    for material in GemMaterial::all_materials() {
        for (cut_name, planes) in [("standard_round_brilliant", &srb), ("emerald_cut", &ec)] {
            let m = evaluate_gem_optical_metrics(planes, &material, 0.0, 1.4, studio());

            assert!(
                m.fire_index.is_finite() && m.fire_index >= 0.1,
                "{} on {}: fire_index must be finite and >= 0.1 (documented floor), got {}",
                material.name,
                cut_name,
                m.fire_index
            );
            // Generous sanity ceiling: other angles can read meaningfully higher than a
            // face-up pose (fewer but steeper-exiting rays are weighted in for the most
            // dispersive materials), so 1000 is kept as a loose ceiling across angles. A
            // blow-up past this would indicate a bug, not real fire.
            assert!(
                m.fire_index < 1000.0,
                "{} on {}: fire_index suspiciously large ({}), expected a measured value \
                 well under 1000 for a built-in material",
                material.name,
                cut_name,
                m.fire_index
            );

            assert!(
                m.scintillation_pct.is_finite() && (0.0..=100.0).contains(&m.scintillation_pct),
                "{} on {}: scintillation_pct must be finite and in [0, 100], got {}",
                material.name,
                cut_name,
                m.scintillation_pct
            );
        }
    }
}

#[test]
#[ignore = "manual probe"]
fn debug_sweep_fire_energy_weighted() {
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();

    println!(
        "{:<22} {:>12} {:>12} {:>12} {:>12}",
        "material", "SRB@1.4", "SRB@0.45", "EC@1.4", "EC@0.45"
    );
    for m in GemMaterial::all_materials() {
        let srb_14 = evaluate_gem_optical_metrics(&srb, &m, 0.0, 1.4, studio());
        let srb_045 = evaluate_gem_optical_metrics(&srb, &m, 0.0, 0.45, studio());
        let ec_14 = evaluate_gem_optical_metrics(&ec, &m, 0.0, 1.4, studio());
        let ec_045 = evaluate_gem_optical_metrics(&ec, &m, 0.0, 0.45, studio());
        println!(
            "{:<22} {:>12.5} {:>12.5} {:>12.5} {:>12.5}",
            m.name, srb_14.fire_index, srb_045.fire_index, ec_14.fire_index, ec_045.fire_index
        );
    }
}

#[test]
#[ignore = "manual probe"]
fn debug_investigate_diamond_vs_quartz_ec() {
    let ec = StandardGemCuts::emerald_cut();
    let diamond = GemMaterial::diamond();
    let quartz = GemMaterial::by_name("Quartz").unwrap();

    println!("--- pitch 0.45 ---");
    let _ = evaluate_gem_optical_metrics(&ec, &diamond, 0.0, 0.45, studio());
    let _ = evaluate_gem_optical_metrics(&ec, &quartz, 0.0, 0.45, studio());
    println!("--- pitch 1.4 ---");
    let _ = evaluate_gem_optical_metrics(&ec, &diamond, 0.0, 1.4, studio());
    let _ = evaluate_gem_optical_metrics(&ec, &quartz, 0.0, 1.4, studio());
}

/// [`TILT_ANGLES_DEG`]'s own shape: 181 points, exact 1° steps, `-90..=90` inclusive,
/// with the shared table-up (tilt = 0°) point (see
/// `evaluate_full_axis_profile_at_azimuth`'s doc comment for why pitch-90/table-up is
/// the pose that's actually shared, not pitch-0/edge-on) at the array's midpoint.
/// Pure/no raytracing, so this stays fast regardless of the per-sample cost that
/// motivated widening from 37 to 181 points.
#[test]
fn tilt_angles_deg_spans_the_full_axis_in_exact_one_degree_steps() {
    assert_eq!(TILT_ANGLES_DEG.len(), 181);
    assert_eq!(TILT_ANGLES_DEG[0], -90.0);
    assert_eq!(TILT_ANGLES_DEG[90], 0.0);
    assert_eq!(TILT_ANGLES_DEG[180], 90.0);
    for i in 0..180 {
        assert_eq!(
            TILT_ANGLES_DEG[i + 1] - TILT_ANGLES_DEG[i],
            1.0,
            "step between index {i} and {} must be exactly 1 degree",
            i + 1
        );
    }
}

/// The full-axis (181-point, ±90° TILT-from-table-up) sweep and the original
/// 19-point/5°-step `evaluate_angular_profile_at_azimuth` sweep (which is an ELEVATION
/// sweep, `0..=90°` of camera pitch at a single fixed azimuth -- a different
/// parameterisation of the same underlying machinery, see
/// `evaluate_full_axis_profile_at_azimuth`'s doc comment) must agree EXACTLY once the
/// tilt-to-pose formula is applied -- both ultimately run the identical
/// `sample_elevation_sweep` loop over the identical `evaluate_gem_optical_metrics`
/// call, so this is a determinism/wiring check (did the merge indexing land each half
/// in the right slot, at the right pitch, with the right sign), not a physics check.
///
/// The tilt-to-pose formula (see `evaluate_full_axis_profile_at_azimuth`'s doc
/// comment): `pitch = 90 - |tilt|`, azimuth = positive for `tilt >= 0`, `+180°` for
/// `tilt < 0`. So PITCH `deg` (a `PROFILE_ANGLES_DEG` entry) at the POSITIVE azimuth
/// lands at TILT index `180 - deg` (`deg = 90` -> index 90, the pole; `deg = 0` ->
/// index 180, positive-side edge-on), and the same pitch at the NEGATIVE azimuth lands
/// at tilt index `deg` (`deg = 90` -> index 90 again; `deg = 0` -> index 0,
/// negative-side edge-on).
///
/// The pitch-90 (table-up) sample is skipped in the per-angle loop and checked once,
/// separately, against ONLY the positive-azimuth sweep's own pitch-90 sample: the pole
/// is evaluated at the positive azimuth by convention (see that function's doc comment
/// for why azimuth is provably irrelevant there), so it need not -- and, since
/// `Camera::new`'s trig at `pitch == 90°` is only azimuth-independent up to
/// floating-point epsilon, should not -- be asserted bit-identical to the
/// NEGATIVE-azimuth sweep's own separately-computed pitch-90 sample too.
#[test]
fn full_axis_profile_agrees_with_the_five_degree_sweep_at_every_shared_angle() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    let positive_azimuth_deg = 45.0f32;

    let (full_b, full_e, full_w) =
        evaluate_full_axis_profile_at_azimuth(&planes, &diamond, positive_azimuth_deg, studio());
    let (pos_b, pos_e, pos_w) = evaluate_angular_profile_at_azimuth(
        &planes,
        &diamond,
        positive_azimuth_deg.to_radians(),
        studio(),
    );
    let (neg_b, neg_e, neg_w) = evaluate_angular_profile_at_azimuth(
        &planes,
        &diamond,
        (positive_azimuth_deg + 180.0).to_radians(),
        studio(),
    );

    // Shared table-up pole: TILT_ANGLES_DEG[90] must be the POSITIVE azimuth's own
    // pitch-90 (last entry, PROFILE_ANGLES_DEG[18] == 90.0) sample, bit-identical
    // (both are the exact same evaluate_gem_optical_metrics call).
    assert_eq!(full_b[90], pos_b[18]);
    assert_eq!(full_e[90], pos_e[18]);
    assert_eq!(full_w[90], pos_w[18]);

    for (i, &deg) in PROFILE_ANGLES_DEG.iter().enumerate() {
        if deg == 90.0 {
            continue; // the shared table-up pole, already checked above
        }
        let p = deg as usize;
        let positive_idx = 180 - p; // TILT_ANGLES_DEG[180 - p] == p (pitch, positive azimuth)
        let negative_idx = p; // TILT_ANGLES_DEG[p] == p (pitch, negative azimuth)

        assert_eq!(
            full_b[positive_idx], pos_b[i],
            "brilliance positive half at pitch {deg} deg"
        );
        assert_eq!(
            full_e[positive_idx], pos_e[i],
            "extinction positive half at pitch {deg} deg"
        );
        assert_eq!(
            full_w[positive_idx], pos_w[i],
            "windowing positive half at pitch {deg} deg"
        );

        assert_eq!(
            full_b[negative_idx], neg_b[i],
            "brilliance negative half at pitch {deg} deg"
        );
        assert_eq!(
            full_e[negative_idx], neg_e[i],
            "extinction negative half at pitch {deg} deg"
        );
        assert_eq!(
            full_w[negative_idx], neg_w[i],
            "windowing negative half at pitch {deg} deg"
        );
    }
}

/// Every sample the full-axis sweep produces must land in the same `0..=100` percentage
/// range the underlying per-pose metrics are already clamped to -- a basic sanity check
/// that the 90/1/90 merge never leaves a slot uninitialized (which would show up as a
/// stray `0.0` array-default that happens to also be in-range, so this test is a floor,
/// not a substitute for the exact-agreement check above).
#[test]
fn full_axis_profile_values_are_in_range_for_every_axis() {
    use indicatrix::color::metrics::PROFILE_AZIMUTHS_DEG;

    let planes = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();

    for &azimuth_deg in &PROFILE_AZIMUTHS_DEG {
        let (b, e, w) =
            evaluate_full_axis_profile_at_azimuth(&planes, &diamond, azimuth_deg, studio());
        assert_eq!(b.len(), 181);
        assert_eq!(e.len(), 181);
        assert_eq!(w.len(), 181);
        for i in 0..181 {
            assert!(
                (0.0..=100.0).contains(&b[i]),
                "brilliance[{i}] = {} out of range",
                b[i]
            );
            assert!(
                (0.0..=100.0).contains(&e[i]),
                "extinction[{i}] = {} out of range",
                e[i]
            );
            assert!(
                (0.0..=100.0).contains(&w[i]),
                "windowing[{i}] = {} out of range",
                w[i]
            );
        }
    }
}
