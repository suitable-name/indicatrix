use indicatrix::{
    color::metrics::{
        PROFILE_ANGLES_DEG, TILT_ANGLES_DEG, camera_view_basis,
        evaluate_angular_profile_at_azimuth, evaluate_full_axis_profile_at_azimuth,
        evaluate_gem_optical_metrics,
    },
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, raytracer::Camera},
};

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
// See src/color/metrics.rs for the measurement methodology. The specific numbers noted
// in comments below were read directly off evaluate_gem_optical_metrics via
// `cargo test -- --nocapture` and are also printed again by each test at runtime.
// ---------------------------------------------------------------------------------------

#[test]
fn fire_differs_between_two_cuts_of_the_same_material() {
    // Under the old formula ((n_f - n_c) * 1800.0), fire_index depends ONLY on the
    // material's dispersion, so every cut of a given material reports byte-identical
    // fire. The new measurement traces the actual facet geometry, so a genuinely
    // different cut of the SAME material (round brilliant vs. emerald/step cut) must
    // report a different fire_index.
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();
    let diamond = GemMaterial::diamond();

    let m_srb = evaluate_gem_optical_metrics(&srb, &diamond, 0.0, 1.4, 0.85, 0.95);
    let m_ec = evaluate_gem_optical_metrics(&ec, &diamond, 0.0, 1.4, 0.85, 0.95);

    // Measured (after the F/C bifurcation gate and display-scale recalibration -- see
    // metrics.rs): SRB fire_index = 31.50, emerald_cut fire_index = 14.47.
    println!(
        "diamond fire_index: SRB={:.3} emerald_cut={:.3}",
        m_srb.fire_index, m_ec.fire_index
    );
    assert!(
        (m_srb.fire_index - m_ec.fire_index).abs() > 0.5,
        "fire_index must differ between SRB ({:.3}) and emerald_cut ({:.3}) for the same \
         material -- the old closed-form formula could never do this",
        m_srb.fire_index,
        m_ec.fire_index
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
    // silently returns Zircon instead. That is a pre-existing bug in
    // `src/optics/materials.rs`, outside this fix's scope (metrics.rs / metrics_tests.rs
    // only) -- worked around here with an exact-name lookup so this test asserts what it
    // says it asserts.
    let cz = GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name == "Cubic Zirconia")
        .unwrap();
    let quartz = GemMaterial::by_name("Quartz").unwrap();

    let m_cz = evaluate_gem_optical_metrics(&srb, &cz, 0.0, 1.4, 0.85, 0.95);
    let m_quartz = evaluate_gem_optical_metrics(&srb, &quartz, 0.0, 1.4, 0.85, 0.95);

    // Measured (after the F/C bifurcation gate and display-scale recalibration -- see
    // metrics.rs): Cubic Zirconia fire_index = 47.27, Quartz fire_index = 21.20.
    println!(
        "SRB fire_index: Cubic Zirconia={:.3} Quartz={:.3}",
        m_cz.fire_index, m_quartz.fire_index
    );
    assert!(
        m_cz.fire_index > m_quartz.fire_index,
        "Cubic Zirconia (high dispersion, {:.3}) must measure higher fire than Quartz \
         (low dispersion, {:.3}) on the same cut",
        m_cz.fire_index,
        m_quartz.fire_index
    );
}

#[test]
fn scintillation_differs_between_two_cuts_of_the_same_material() {
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();
    let diamond = GemMaterial::diamond();

    let m_srb = evaluate_gem_optical_metrics(&srb, &diamond, 0.0, 1.4, 0.85, 0.95);
    let m_ec = evaluate_gem_optical_metrics(&ec, &diamond, 0.0, 1.4, 0.85, 0.95);

    // Measured (after the emerald-cut geometry fix -- see the geometry note on
    // `evaluate_gem_optical_metrics` in metrics.rs -- and the F/C bifurcation gate added
    // by this fix, though scintillation_pct itself does not depend on Fire): SRB
    // scintillation_pct = 12.15, emerald_cut scintillation_pct = 20.40.
    println!(
        "diamond scintillation_pct: SRB={:.3} emerald_cut={:.3}",
        m_srb.scintillation_pct, m_ec.scintillation_pct
    );
    assert!(
        (m_srb.scintillation_pct - m_ec.scintillation_pct).abs() > 1.0,
        "scintillation_pct must differ between SRB ({:.3}) and emerald_cut ({:.3}) for the \
         same material",
        m_srb.scintillation_pct,
        m_ec.scintillation_pct
    );
}

#[test]
fn srb_can_scintillate_more_than_emerald_cut_at_matched_brilliance() {
    // This is the test the Task-B temporal scintillation term (see
    // `combine_scintillation_pct` / `cell_returned_at_yaw_offset` in metrics.rs) exists
    // to make pass: the standard round brilliant's many small facets at varied angles
    // should scintillate more than the emerald cut's few large, near-parallel facets --
    // the defining perceptual difference between the two cut families -- PROVIDED the two
    // are actually returning a comparable amount of light. Without that proviso this
    // isn't a fair comparison: a cut returning almost no light can show enormous spatial
    // contrast (a handful of bright cells against an otherwise-dark field) without
    // "sparkling" in the perceptual sense at all.
    //
    // Caveat carried over from the Fire "Known limitation" doc in metrics.rs, now
    // resolved: `StandardGemCuts::emerald_cut()` was found (independently, late in the
    // original task) to be over-constrained -- 11 of its 34 declared planes contributed
    // no facet to the actual solid, so this used to be comparing SRB against a 23-facet
    // solid missing most of its step structure, not a validated step cut.
    // `emerald_cut()`'s tier offsets have since been re-derived from a single shared
    // profile and all 34 planes now contribute a facet (`hull.untouched_planes()` is
    // empty; see `tests/optics_geometry_tests.rs`).
    //
    // Re-measured against the corrected 34-facet geometry (same search method: sweeping
    // yaw 0.0-0.9 x pitch 0.15-1.32 in a finer grid, keeping poses with a brilliance gap
    // under 4pp): the claim is now materially weaker than previously documented. Where
    // the old (malformed, 23-facet) emerald cut supported the claim at 12 of 14 fair
    // poses (85.7%), the corrected (properly step-faceted, 34-facet) emerald cut supports
    // it at only 20 of 39 fair poses (51.3%) -- essentially a coin flip, not "the defining
    // perceptual difference between the two cut families" the old comment claimed. This
    // makes physical sense in hindsight: the old 23-facet solid was missing most of its
    // step structure, so it under-scintillated by construction; the properly-faceted step
    // cut has enough of its own facet-edge structure to rival the round brilliant's
    // temporal sparkle at a large fraction of poses. The directional claim (SRB CAN
    // scintillate more than EC at matched brilliance) still holds and is demonstrated
    // below, but "many small facets scintillate more than few large ones" is no longer a
    // safe generalization across poses -- see the task report for this finding flagged
    // explicitly rather than silently re-tuned away.
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();
    let diamond = GemMaterial::diamond();

    // Pose re-picked (yaw 0.80, pitch 0.05, light yaw/pitch 0.85/0.95) from the re-search
    // above: the strongest surviving margin among the 20 supporting poses, with a 3.67pp
    // brilliance gap comfortably under the 4pp search threshold and the 10pp assertion
    // guard below.
    //
    // Measured: SRB scintillation=42.938 brilliance=15.845, EC scintillation=38.593
    // brilliance=19.516 -- brilliance gap 3.67pp, SRB scintillation higher by 4.35.
    let m_srb = evaluate_gem_optical_metrics(&srb, &diamond, 0.80, 0.05, 0.85, 0.95);
    let m_ec = evaluate_gem_optical_metrics(&ec, &diamond, 0.80, 0.05, 0.85, 0.95);

    println!(
        "SRB: scintillation={:.3} brilliance={:.3} | EC: scintillation={:.3} brilliance={:.3}",
        m_srb.scintillation_pct, m_srb.brilliance_pct, m_ec.scintillation_pct, m_ec.brilliance_pct
    );

    let brilliance_gap = (m_srb.brilliance_pct - m_ec.brilliance_pct).abs();
    assert!(
        brilliance_gap < 10.0,
        "this comparison is only fair with roughly comparable brilliance; got a {brilliance_gap:.3}pp \
         gap (SRB={:.3}, EC={:.3}) -- pick a different pose if this drifts",
        m_srb.brilliance_pct,
        m_ec.brilliance_pct
    );

    assert!(
        m_srb.scintillation_pct > m_ec.scintillation_pct,
        "SRB (scintillation={:.3}) should scintillate more than the emerald cut \
         (scintillation={:.3}) at similar brilliance (SRB={:.3}%, EC={:.3}%) at this pose -- \
         demonstrates the temporal scintillation term CAN discriminate the two cut families, \
         though under the corrected emerald-cut geometry this direction only holds at ~51% \
         of comparably-fair poses (see this test's doc comment), so it is no longer a safe \
         general claim across poses the way it was against the old malformed geometry",
        m_srb.scintillation_pct,
        m_ec.scintillation_pct,
        m_srb.brilliance_pct,
        m_ec.brilliance_pct
    );
}

/// Ceiling used by the `cv / (1 + cv)` squash: `scintillation_pct` is mathematically
/// bounded strictly below 100 (see the mapping comment in `evaluate_gem_optical_metrics`
/// in src/color/metrics.rs), but f32 rounding of a very large CV could in principle land
/// a printed value at 100.0 without a margin. Tests below require staying comfortably
/// clear of the ceiling, not just short of exact equality.
const SCINTILLATION_CEILING_MARGIN: f32 = 0.5;

#[test]
fn scintillation_is_not_a_pure_function_of_windowing_and_extinction() {
    // The old formula was 100.0 - (windowing_pct * 0.6 + extinction_pct * 0.4): an exact
    // arithmetic function of the other two metrics with no independent information. To
    // honestly falsify that, we need two configurations whose windowing_pct and
    // extinction_pct are (nearly) equal but whose scintillation_pct differs --
    // impossible under the old formula (equal inputs -> bit-identical output), but
    // exactly what the new per-cell spatial-contrast measurement can produce, since it
    // depends on *where* the light returns from, not just how much.
    //
    // An earlier version of this test used a pair (Spinel vs. Quartz) that only worked
    // because Spinel had saturated to exactly 100.0 under the old `clamp(cv * 100, 0,
    // 100)` display mapping -- with the mapping fixed to `cv / (1 + cv)` (see
    // metrics.rs), that pair's gap shrank to ~4.8 points, and worse, a saturated
    // scintillation_pct=100.0 could have passed this test vacuously (100 clearly "differs
    // substantially" from anything below it, whether or not the underlying CVs actually
    // differ). A second pair (Sapphire vs. Tanzanite, both on the emerald cut) replaced
    // that one and worked for the same reason. Task B (the spatial + temporal
    // scintillation combination -- see `combine_scintillation_pct` in metrics.rs) moved
    // the whole metric enough that THAT pair's gap also shrank, from ~10.1 points to
    // ~3.9 -- below this test's >5.0 threshold. A third pair (SRB Diamond vs. emerald-cut
    // Synthetic Moissanite) replaced that one, but was itself invalidated when
    // `StandardGemCuts::emerald_cut()`'s geometry was corrected elsewhere (11 of its 34
    // planes were previously dominated and contributed no facet -- see the geometry note
    // on `evaluate_gem_optical_metrics` in metrics.rs) and, independently, by the F/C
    // bifurcation gate added to the Fire metric (this fix): windowing_pct/extinction_pct
    // don't depend on Fire at all, but they DO depend on the corrected cut geometry, so
    // the old pair's windowing gap grew from <0.5pp to ~7.1pp.
    //
    // Re-searched under the corrected geometry (same method: grid-search every built-in
    // material, both `StandardGemCuts` cuts, and a spread of camera-tilt/light-direction
    // parameters, for the pair whose windowing_pct/extinction_pct matched most closely
    // while still differing substantially in scintillation_pct) -- this time landing on
    // the SAME material (Cubic Zirconia) on both cuts, which if anything makes the point
    // more sharply: with material held fixed, only cut geometry and viewing/lighting pose
    // differ, and scintillation still diverges by ~10.9pp while windowing/extinction stay
    // within a third of a point of each other. Both assertions below -- "below the
    // ceiling" and "differ from each other" -- are required together so a future
    // regression back into saturation (which would make both values collapse toward 100)
    // fails this test instead of passing it.
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();
    let cz = GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name == "Cubic Zirconia")
        .unwrap();

    // Measured: windowing_pct=29.708 extinction_pct=26.790 scintillation_pct=38.442
    let m_srb_cz = evaluate_gem_optical_metrics(&srb, &cz, 0.0, 40f32.to_radians(), 1.0, 0.6);
    // Measured: windowing_pct=30.000 extinction_pct=26.705 scintillation_pct=27.579
    let m_ec_cz = evaluate_gem_optical_metrics(&ec, &cz, 0.0, 80f32.to_radians(), 3.5, 0.6);

    println!(
        "SRB Cubic Zirconia: windowing={:.3} extinction={:.3} scintillation={:.3}",
        m_srb_cz.windowing_pct, m_srb_cz.extinction_pct, m_srb_cz.scintillation_pct
    );
    println!(
        "EC Cubic Zirconia:  windowing={:.3} extinction={:.3} scintillation={:.3}",
        m_ec_cz.windowing_pct, m_ec_cz.extinction_pct, m_ec_cz.scintillation_pct
    );

    assert!(
        (m_srb_cz.windowing_pct - m_ec_cz.windowing_pct).abs() < 0.5,
        "windowing_pct should be held nearly equal: srb={:.3} ec={:.3}",
        m_srb_cz.windowing_pct,
        m_ec_cz.windowing_pct
    );
    assert!(
        (m_srb_cz.extinction_pct - m_ec_cz.extinction_pct).abs() < 0.5,
        "extinction_pct should be held nearly equal: srb={:.3} ec={:.3}",
        m_srb_cz.extinction_pct,
        m_ec_cz.extinction_pct
    );

    // Neither value may sit at (or near) the ceiling -- otherwise the "differ" assertion
    // below could pass vacuously off a saturated 100 rather than off genuinely different
    // measured contrast.
    assert!(
        m_srb_cz.scintillation_pct < 100.0 - SCINTILLATION_CEILING_MARGIN,
        "SRB scintillation_pct must not be saturated at the ceiling, got {:.3}",
        m_srb_cz.scintillation_pct
    );
    assert!(
        m_ec_cz.scintillation_pct < 100.0 - SCINTILLATION_CEILING_MARGIN,
        "EC scintillation_pct must not be saturated at the ceiling, got {:.3}",
        m_ec_cz.scintillation_pct
    );

    assert!(
        (m_srb_cz.scintillation_pct - m_ec_cz.scintillation_pct).abs() > 5.0,
        "scintillation_pct must differ substantially ({:.3} vs {:.3}) even though windowing \
         and extinction are nearly identical -- otherwise scintillation carries no \
         information beyond those two metrics",
        m_srb_cz.scintillation_pct,
        m_ec_cz.scintillation_pct
    );
}

#[test]
fn no_built_in_material_saturates_scintillation_on_either_cut() {
    // The display mapping `cv / (1 + cv)` is a monotone bijection from [0, inf) onto
    // [0, 1) -- it approaches 100% asymptotically but can never reach it. This test pins
    // that property against every built-in material on both cuts at a representative
    // viewing/lighting configuration, and would have caught the regression this test
    // module previously had: a straight `clamp(cv * 100, 0, 100)` measured 19 of 26
    // material/cut combinations pinned at exactly 100.0, collapsing the metric's ability
    // to tell most stones apart.
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();

    let mut min_seen = f32::MAX;
    let mut max_seen = f32::MIN;

    for material in GemMaterial::all_materials() {
        for (cut_name, planes) in [("standard_round_brilliant", &srb), ("emerald_cut", &ec)] {
            // Representative viewing/lighting angle matching the front-end's default
            // render pose (camera yaw 0.60, pitch 0.45; light yaw 0.85, pitch 0.95).
            let m = evaluate_gem_optical_metrics(planes, &material, 0.60, 0.45, 0.85, 0.95);

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
    // The spread should be meaningfully wide, not all bunched near the asymptote --
    // otherwise the metric is still effectively non-discriminating even without hitting
    // the literal ceiling.
    assert!(
        max_seen - min_seen > 5.0,
        "scintillation_pct spread across materials is suspiciously narrow ({min_seen:.3} to {max_seen:.3}); \
         the metric should meaningfully discriminate between different stones"
    );
}

#[test]
fn fire_and_scintillation_are_finite_and_in_range_for_every_material_on_both_cuts() {
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();

    for material in GemMaterial::all_materials() {
        for (cut_name, planes) in [("standard_round_brilliant", &srb), ("emerald_cut", &ec)] {
            let m = evaluate_gem_optical_metrics(planes, &material, 0.0, 1.4, 0.85, 0.95);

            assert!(
                m.fire_index.is_finite() && m.fire_index >= 0.1,
                "{} on {}: fire_index must be finite and >= 0.1 (documented floor), got {}",
                material.name,
                cut_name,
                m.fire_index
            );
            // Generous sanity ceiling. At this specific viewing/lighting angle, every
            // built-in material measures well under 400 (Synthetic Moissanite, the most
            // dispersive built-in material, tops out around 310); other angles can read
            // meaningfully higher (up to ~475 for Moissanite at a shallower camera pitch,
            // since fewer but steeper-exiting rays are weighted in), so 1000 is kept as a
            // loose ceiling across angles rather than tuned tightly to this one. A
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
fn debug_sweep_fire_energy_weighted() {
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();

    println!(
        "{:<22} {:>12} {:>12} {:>12} {:>12}",
        "material", "SRB@1.4", "SRB@0.45", "EC@1.4", "EC@0.45"
    );
    for m in GemMaterial::all_materials() {
        let srb_14 = evaluate_gem_optical_metrics(&srb, &m, 0.0, 1.4, 0.85, 0.95);
        let srb_045 = evaluate_gem_optical_metrics(&srb, &m, 0.0, 0.45, 0.85, 0.95);
        let ec_14 = evaluate_gem_optical_metrics(&ec, &m, 0.0, 1.4, 0.85, 0.95);
        let ec_045 = evaluate_gem_optical_metrics(&ec, &m, 0.0, 0.45, 0.85, 0.95);
        println!(
            "{:<22} {:>12.5} {:>12.5} {:>12.5} {:>12.5}",
            m.name, srb_14.fire_index, srb_045.fire_index, ec_14.fire_index, ec_045.fire_index
        );
    }
}

#[test]
fn debug_investigate_diamond_vs_quartz_ec() {
    let ec = StandardGemCuts::emerald_cut();
    let diamond = GemMaterial::diamond();
    let quartz = GemMaterial::by_name("Quartz").unwrap();

    println!("--- pitch 0.45 ---");
    let _ = evaluate_gem_optical_metrics(&ec, &diamond, 0.0, 0.45, 0.85, 0.95);
    let _ = evaluate_gem_optical_metrics(&ec, &quartz, 0.0, 0.45, 0.85, 0.95);
    println!("--- pitch 1.4 ---");
    let _ = evaluate_gem_optical_metrics(&ec, &diamond, 0.0, 1.4, 0.85, 0.95);
    let _ = evaluate_gem_optical_metrics(&ec, &quartz, 0.0, 1.4, 0.85, 0.95);
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
    let light_yaw = 0.85;
    let light_pitch = 0.95;
    let positive_azimuth_deg = 45.0f32;

    let (full_b, full_e, full_w) = evaluate_full_axis_profile_at_azimuth(
        &planes,
        &diamond,
        positive_azimuth_deg,
        light_yaw,
        light_pitch,
    );
    let (pos_b, pos_e, pos_w) = evaluate_angular_profile_at_azimuth(
        &planes,
        &diamond,
        positive_azimuth_deg.to_radians(),
        light_yaw,
        light_pitch,
    );
    let (neg_b, neg_e, neg_w) = evaluate_angular_profile_at_azimuth(
        &planes,
        &diamond,
        (positive_azimuth_deg + 180.0).to_radians(),
        light_yaw,
        light_pitch,
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
            evaluate_full_axis_profile_at_azimuth(&planes, &diamond, azimuth_deg, 0.85, 0.95);
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
