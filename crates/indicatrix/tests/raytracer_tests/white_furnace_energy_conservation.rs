//! White-furnace energy-conservation regression tests for anisotropic (birefringent)
//! transport: a colourless, non-dispersive, birefringent gem immersed in a
//! spatially uniform environment must still converge to that environment's own
//! radiance, both with a polished girdle and with a bruted (frosted) one.

use glam::Vec3;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{EnvironmentSource, trace_spectral_ray, trace_spectral_ray_with_finish},
    },
    renderer::env_map::EnvironmentMap,
};

use crate::fixtures::{bruted_girdle_finishes, furnace_mean_xyz, uniform_furnace_target};

/// CPU-side regression test for TWO energy-conservation bugs at once: a colourless,
/// non-dispersive, BIREFRINGENT (uniaxial) gem immersed in a spatially UNIFORM
/// environment, traced at several bounce caps.
///
/// Decision record: this test guards `apply_internal_mode_coupling` against scaling
/// `stokes` by `1/0.5 = 2.0` per internal reflection with no compensating `path_pdf`
/// effect, which would make this scene diverge without bound as `max_bounces` grows --
/// mean luminance would go from finite at `max_bounces=12` to `NaN`/`inf` by 64. Every
/// isotropic furnace anchor elsewhere in this file never exercises
/// `apply_internal_mode_coupling` at all, so none of them can catch a regression there.
/// It also guards the sibling entry-split path
/// (`apply_refract_bounce`/`apply_refract_channel` dividing `stokes` by the same
/// 0.5 mode-selection probability already implicitly weighted into `trans_matrix_k`),
/// which would produce a roughly constant ~48-50% brightness inflation instead of a
/// compounding one -- flat across bounce caps, so the cross-cap drift check alone would
/// not catch it. CPU-only deliberately (no `gpu` feature required): both paths live in
/// `optics::raytracer::transport`, exercised by a default `cargo test`.
///
/// Asserts convergence to the uniform environment's analytic radiance directly, the
/// same truth anchor every other furnace test in this file uses. The cross-cap drift
/// and finiteness checks are kept as an independent, more direct guard against
/// internal-coupling regressions specifically.
#[test]
fn birefringent_white_furnace_energy_conservation_holds() {
    const L0: f32 = 2.5;
    const SAMPLES_PER_PIXEL: u32 = 64;
    // How far each higher-cap mean may drift from the lowest-cap (12) mean, per XYZ
    // component and relative to that low-cap component (`drift` below). Measured on
    // 2026-10-01 by `furnace_noise_floor` (sixteen seeds at this sample budget): the
    // drift is the truncation loss of cap 12, 1.3 percent (paths still inside the gem
    // at the cap are dropped with their energy; at cap 256 the loss is gone), plus
    // sampling noise of about 0.1 percent; five standard deviations of headroom give
    // 0.02. An unbounded regression would blow straight through it (were every
    // anisotropic internal bounce to multiply brightness by another 2x, cap=64 vs.
    // cap=12 would differ by many, many orders of magnitude, not a percent).
    const CROSS_CAP_TOLERANCE: f32 = 0.02;
    // How far each bounce cap's mean is allowed to sit from the analytic target. The
    // same measurement: 1.3 percent truncation loss at cap 12 (nothing at 64 and 256)
    // plus about 0.1 percent noise, five standard deviations of headroom. Nowhere close
    // to the ~0.48-0.50 an unguarded entry-split regression (see the decision record
    // above) would produce, and tight enough that a branch mis-weighted by a few
    // percent fails.
    const ANALYTIC_TOLERANCE: f32 = 0.02;
    // A generous but finite ceiling on plausible brightness -- comfortably above the
    // analytic target, but many, many orders of magnitude below anything the original
    // per-bounce-doubling bug produced by max_bounces=64 (`examples/bounce_cost.rs`'s
    // real-material Quartz measurement: ~6.8e7 at cap 64, ~5.4e16 by cap 128).
    const SANITY_CEILING: f32 = 50.0 * L0;
    // Bounce caps spanning the range the original bug diverged across: 12 is where the
    // pre-fix Quartz measurement (a real material, not this synthetic furnace scene)
    // was still merely biased; 64 and 256 are deep into where it had already gone
    // exponential.
    const BOUNCE_CAPS: [u32; 3] = [12, 64, 256];

    let planes = StandardGemCuts::standard_round_brilliant();
    // Colourless, non-dispersive, and -- unlike
    // `frosted_girdle_white_furnace_energy_conservation_still_holds`'s material above --
    // genuinely UNIAXIAL: nonzero `birefringence_delta` puts this on the
    // `is_anisotropic` path `apply_internal_mode_coupling` only fires on.
    // `GemMaterial::new_custom`'s own birefringence_delta > 1e-4 branch sets
    // `crystal_system: Trigonal` and `optical_character: OpticalCharacter::UniaxialPositive`
    // automatically. Empty absorption bands (last argument) match the isotropic furnace
    // anchor's own "colourless" construction exactly.
    let material = GemMaterial::new_custom(
        "CPU birefringent furnace probe",
        1.5,
        0.0,
        0.03,
        [0.0, 0.0, 0.0],
    );
    assert!(
        material.birefringence_delta.abs() > 1e-4,
        "test assumes this material is genuinely birefringent"
    );
    let env_map = EnvironmentMap::uniform(1, 1, [L0, L0, L0]);

    // Analytic target: the same quadrature every other furnace test in this file uses --
    // now asserted against directly (see this test's doc comment for why a direct
    // assertion needs the entry-split path).
    let target = uniform_furnace_target(L0);

    let mut means: Vec<(u32, Vec3)> = Vec::with_capacity(BOUNCE_CAPS.len());
    for &max_bounces in &BOUNCE_CAPS {
        // trace_spectral_ray always runs with internal mode coupling
        // enabled (the same as every real production call site) -- see
        // that function's own doc comment.
        let (mean, count) = furnace_mean_xyz(SAMPLES_PER_PIXEL, 0x4257_4946, |ray, seed, hero| {
            trace_spectral_ray(
                ray,
                &planes,
                &material,
                max_bounces,
                EnvironmentSource::HdrMap(&env_map),
                seed,
                hero,
                None,
            )
        });
        let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
        let (ex, ey, ez) = (
            rel_err(mean.x, target.x),
            rel_err(mean.y, target.y),
            rel_err(mean.z, target.z),
        );
        println!(
            "[birefringent furnace] max_bounces={max_bounces} mean={mean:?} \
             analytic_target={target:?} rel_err_vs_target=({ex:.4}, {ey:.4}, {ez:.4}) over \
             {count} samples",
        );
        assert!(
            mean.x.is_finite() && mean.y.is_finite() && mean.z.is_finite(),
            "max_bounces={max_bounces}: birefringent furnace mean must stay finite, got {mean:?} \
             -- a non-finite value here is the exact signature of the internal-mode-coupling \
             energy-doubling bug this test guards against"
        );
        assert!(
            mean.x <= SANITY_CEILING && mean.y <= SANITY_CEILING && mean.z <= SANITY_CEILING,
            "max_bounces={max_bounces}: birefringent furnace mean {mean:?} exceeds the sanity \
             ceiling {SANITY_CEILING} -- this is the signature of unbounded per-bounce energy \
             growth, exactly what the internal-mode-coupling bug produced"
        );
        assert!(
            ex <= ANALYTIC_TOLERANCE && ey <= ANALYTIC_TOLERANCE && ez <= ANALYTIC_TOLERANCE,
            "max_bounces={max_bounces}: birefringent furnace mean {mean:?} strays from the \
             analytic target {target:?} by rel_err=({ex}, {ey}, {ez}), tolerance=\
             {ANALYTIC_TOLERANCE} -- with both the internal-mode-coupling and entry-split \
             energy-conservation bugs fixed, a lossless closed system must return its own \
             uniform radiance to within ordinary sampling noise"
        );
        means.push((max_bounces, mean));
    }

    let (low_cap, low_mean) = means[0];
    for &(hi_cap, hi_mean) in &means[1..] {
        let drift = |lo: f32, hi: f32| (hi - lo).abs() / lo.abs().max(1e-6);
        let (dx, dy, dz) = (
            drift(low_mean.x, hi_mean.x),
            drift(low_mean.y, hi_mean.y),
            drift(low_mean.z, hi_mean.z),
        );
        assert!(
            dx <= CROSS_CAP_TOLERANCE && dy <= CROSS_CAP_TOLERANCE && dz <= CROSS_CAP_TOLERANCE,
            "a lossless closed system's expected brightness must not depend on the bounce \
             cap: max_bounces={low_cap} gave mean={low_mean:?}, max_bounces={hi_cap} gave \
             mean={hi_mean:?}, drift=({dx}, {dy}, {dz}), tolerance={CROSS_CAP_TOLERANCE} -- \
             this is exactly the property the original per-internal-bounce `stokes *= 2.0` \
             broke (each additional internal bounce compounded the brightness further)"
        );
    }
}

/// Closes a coverage gap: this file's other two furnace anchors use EITHER a frosted
/// girdle with an ISOTROPIC material OR a birefringent material with an all-Polished
/// girdle, never both -- so neither exercises `apply_frosted_bounce`'s anisotropic
/// entry-split branch (`entering_anisotropic` inside its transmit branch,
/// `optics::raytracer::scattering`). That is exactly the shape of gap that hid the
/// first two energy-doubling bugs, only caught once a test anchored against the
/// analytic target on a genuinely anisotropic material. This test combines both: a
/// frosted girdle AND a birefringent material, so a path enters through a `Frosted`
/// facet while still needing the 50/50 ordinary/extraordinary mode split -- a third
/// instance of the same bug class divided `stokes` by that 0.5 probability a second
/// time, producing the same ~48-50% brightness inflation.
///
/// Structured like `birefringent_white_furnace_energy_conservation_holds` but with a
/// single bounce cap (an entry-split check, not a compounding-bug check) and the
/// frosted girdle from `frosted_girdle_white_furnace_energy_conservation_still_holds`'s
/// `bruted_girdle_finishes` helper.
#[test]
fn frosted_girdle_birefringent_white_furnace_energy_conservation_holds() {
    const L0: f32 = 2.5;
    const SAMPLES_PER_PIXEL: u32 = 64;
    // Measured on 2026-10-01 by `furnace_noise_floor` at this sample budget: 1.65
    // percent truncation loss at cap 12 (nothing at cap 256) plus about 0.15 percent
    // noise; five standard deviations of headroom.
    const ANALYTIC_TOLERANCE: f32 = 0.025;

    let planes = StandardGemCuts::standard_round_brilliant();
    let finishes = bruted_girdle_finishes(planes.len());
    // Genuinely birefringent (same construction as
    // `birefringent_white_furnace_energy_conservation_holds`), so a girdle entry can hit
    // `apply_frosted_bounce`'s `entering_anisotropic` branch.
    let material = GemMaterial::new_custom(
        "CPU frosted birefringent furnace probe",
        1.5,
        0.0,
        0.03,
        [0.0, 0.0, 0.0],
    );
    assert!(
        material.birefringence_delta.abs() > 1e-4,
        "test assumes this material is genuinely birefringent"
    );
    let env_map = EnvironmentMap::uniform(1, 1, [L0, L0, L0]);

    let (mean, count) = furnace_mean_xyz(SAMPLES_PER_PIXEL, 0x4652_4247, |ray, seed, hero| {
        trace_spectral_ray_with_finish(
            ray,
            &planes,
            &finishes,
            &material,
            12,
            EnvironmentSource::HdrMap(&env_map),
            seed,
            hero,
            None,
        )
    });

    // Analytic target: same quadrature every other furnace test in this file uses.
    let target = uniform_furnace_target(L0);

    let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
    let (ex, ey, ez) = (
        rel_err(mean.x, target.x),
        rel_err(mean.y, target.y),
        rel_err(mean.z, target.z),
    );
    println!(
        "[frosted-girdle birefringent furnace] mean={mean:?} target={target:?} \
         rel_err=({ex:.4}, {ey:.4}, {ez:.4}) over {count} samples"
    );
    assert!(
        ex <= ANALYTIC_TOLERANCE && ey <= ANALYTIC_TOLERANCE && ez <= ANALYTIC_TOLERANCE,
        "frosted-girdle birefringent furnace should still converge to the uniform \
         environment's own radiance (mean={mean:?}, target={target:?}, rel_err=({ex}, {ey}, \
         {ez}), tolerance={ANALYTIC_TOLERANCE}) -- a residual here is the signature of \
         apply_frosted_bounce's anisotropic entry-split bug (dividing stokes by its own \
         0.5 mode-selection probability on top of an already-full-share diffuse-transmitted \
         intensity)"
    );
}
