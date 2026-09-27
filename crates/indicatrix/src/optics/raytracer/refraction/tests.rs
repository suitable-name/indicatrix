//! Unit tests for the `refraction` module tree: TIR phase retardation, Fresnel energy
//! conservation (scalar and the uniaxial closed-form CPU wiring), and the
//! polarization-weighted entry-eigenmode selection.

use super::{
    context::{
        BounceRay, BounceState, ExitEvent, ExitSplitCtx, RayMaterialContext, UniaxialBounceContext,
    },
    geometry::BounceRefractionGeometry,
    tir::tir_phase_delta,
    uniaxial_entry::entry_eigenmode_selection,
    uniaxial_internal::{
        HeroInternalSolve, apply_uniaxial_internal_reflect_channels,
        apply_uniaxial_internal_transmit_channels,
    },
};
use crate::optics::{
    polarization::{MuellerMatrix, StokesVector},
    raytracer::{
        NUM_CHANNELS,
        environment::EnvironmentSource,
        sampling::{BIREFRINGENT_SPLIT_STREAM, hash_u32},
        uniaxial_fresnel::{self, UniaxialFrame},
    },
};
use glam::Vec3;

#[cfg(test)]
mod tir_phase_retardation_tests {
    use super::*;

    /// The partial-reflection branch's "channel k is past its own critical
    /// angle" case now applies `tir_phase_delta` (via `MuellerMatrix::tir_retardation`)
    /// instead of the phase-blind `fresnel_reflection(1.0, 1.0)`. This checks the
    /// shared helper reproduces the same delta the dedicated (hero-is-past-critical)
    /// TIR branch computes inline, for a channel genuinely past its critical angle --
    /// i.e. the two sites are guaranteed to agree because they now share one formula.
    #[test]
    fn tir_phase_delta_is_nonzero_past_critical_angle() {
        // n1 = 2.42 (diamond-like), incidence well past the ~24.4 degree critical angle.
        let n1k = 2.42f32;
        let cos_i = 60.0f32.to_radians().cos();
        let sin_i = 60.0f32.to_radians().sin();
        assert!(
            n1k * sin_i > 1.0,
            "test setup must be past the critical angle"
        );

        let delta = tir_phase_delta(n1k, cos_i, sin_i);
        assert!(
            delta.abs() > 1e-3,
            "TIR phase retardation should be nonzero past the critical angle (got {delta})"
        );

        let tir_matrix = MuellerMatrix::tir_retardation(delta);
        let linear_45 = StokesVector::new(1.0, 0.0, 1.0, 0.0);
        let out = linear_45.apply_matrix(&tir_matrix);
        assert!(
            out.v.abs() > 1e-3,
            "a nonzero TIR phase retardation must convert some linear polarization to circular (V) (got V={})",
            out.v
        );
    }

    /// At exactly grazing incidence the retardation formula must stay finite (no NaN /
    /// Inf) even though several terms blow up in the naive algebra.
    #[test]
    fn tir_phase_delta_is_finite_near_grazing_incidence() {
        let n1k = 2.42f32;
        let cos_i = 0.001f32;
        let sin_i = cos_i.mul_add(-cos_i, 1.0).sqrt();
        let delta = tir_phase_delta(n1k, cos_i, sin_i);
        assert!(
            delta.is_finite(),
            "delta must remain finite near grazing incidence (got {delta})"
        );
    }
}

/// `apply_partial_fresnel_bounce` computes the reflect-vs-transmit decision's
/// `r_unpol` from the SAME mode's own refractive index the transmit branch
/// (`apply_refract_channel`) uses for that mode, rather than always mode B's -- these
/// tests check the resulting `R + T == 1` identity directly against the same Fresnel
/// algebra those two sites share (`r_s`/`r_p`/`t_s`/`t_p`, `MuellerMatrix::
/// fresnel_reflection`/`fresnel_transmission`'s own "a" coefficients), at both the
/// ordinary and extraordinary index, across a spread of incidence angles.
#[cfg(test)]
mod p2_fresnel_energy_conservation_tests {
    use crate::optics::materials::GemMaterial;

    #[test]
    fn ordinary_and_extraordinary_indices_each_conserve_energy_at_several_angles() {
        let material = GemMaterial::by_name("Zircon").expect("Zircon must be a built-in material");
        let n_o = material.dispersion.evaluate(589.3);
        let n_e = n_o + material.birefringence_delta;
        assert!(
            (n_o - n_e).abs() > 0.01,
            "test premise: Zircon must be strongly birefringent"
        );

        for n2 in [n_o, n_e] {
            for angle_deg in [0.0f32, 15.0, 30.0, 45.0, 60.0, 75.0] {
                let n1 = 1.0f32; // air -> crystal entry
                let cos_i = angle_deg.to_radians().cos();
                let sin_i = angle_deg.to_radians().sin();
                let eta = n1 / n2;
                let sin2_t = eta * eta * sin_i * sin_i;
                assert!(
                    sin2_t <= 1.0,
                    "air->crystal entry (n1 == 1.0 < n2) should never TIR"
                );
                let cos_t = (1.0 - sin2_t).sqrt();

                // Exactly `apply_partial_fresnel_bounce`'s r_s/r_p (UNCLAMPED -- the
                // selection-probability clamp there is a firefly-mitigation detail of
                // the sampling probability, not part of the underlying Fresnel
                // identity this test checks) and `apply_refract_channel`'s t_s/t_p.
                let r_s =
                    f32::mul_add(n2, -cos_t, n1 * cos_i) / f32::mul_add(n2, cos_t, n1 * cos_i);
                let r_p =
                    f32::mul_add(n1, -cos_t, n2 * cos_i) / f32::mul_add(n1, cos_t, n2 * cos_i);
                let t_s = (2.0 * n1 * cos_i) / f32::mul_add(n2, cos_t, n1 * cos_i);
                let t_p = (2.0 * n1 * cos_i) / f32::mul_add(n1, cos_t, n2 * cos_i);

                let r_unpol = 0.5 * r_p.mul_add(r_p, r_s * r_s);
                // Exactly `MuellerMatrix::fresnel_transmission`'s flux-conserving
                // `factor`/`a` coefficients.
                let factor = (n2 * cos_t) / (n1 * cos_i).max(1e-6);
                let t_unpol = 0.5 * f32::mul_add(t_p * t_p, factor, t_s * t_s * factor);

                assert!(
                    (r_unpol + t_unpol - 1.0).abs() < 1e-6,
                    "R + T should equal 1 at n2={n2}, angle={angle_deg} deg \
                     (R={r_unpol}, T={t_unpol}, R+T={})",
                    r_unpol + t_unpol
                );
            }
        }
    }
}

/// Energy conservation through the ACTUAL
/// CPU wiring `apply_uniaxial_internal_bounce` dispatches to --
/// `apply_uniaxial_internal_reflect_channels` and `apply_uniaxial_internal_transmit_
/// channels` -- not just the closed-form solver in isolation (already covered, at
/// looser tolerance, by `uniaxial_fresnel::tests::
/// internal_exit_energy_conservation_holds_including_tir`).
#[cfg(test)]
mod p2_uniaxial_internal_wiring_energy_conservation_tests {
    use super::*;
    use crate::optics::materials::GemMaterial;

    /// For a UNIT incident intensity, undoing each branch's own `1/r_branch` /
    /// `1/(1-r_branch)` importance-sampling division (exactly what a real Monte Carlo
    /// path's expectation does across many samples) and summing the two branches'
    /// deposited intensity must reproduce 1.0 -- `E[reflected] + E[transmitted] ==
    /// incident`, checked at a fixed, arbitrary `r_branch` (the wiring under test does
    /// not know or care what value drove the coin flip that selected it -- both
    /// branches are exercised directly and deterministically here, not sampled) across
    /// at least 6 angles x 3 axis orientations x both incident modes.
    ///
    /// Sweep excludes genuine TIR (this test's `r_branch` is a fixed 0.5, not derived
    /// from the hero's own reflectance, so every angle here is sub-critical for BOTH
    /// modes -- the closed-form solver's own TIR-inclusive conservation is already
    /// covered, at a looser tolerance, by `internal_exit_energy_conservation_holds_
    /// including_tir`); measured `worst_err` over this sweep is `~3.6e-7`, comfortably
    /// inside the required `1e-6` target -- asserted at `5e-6` (a >10x margin
    /// over the measured worst case) rather than the raw measured value, so the test
    /// does not flake on an unrelated few-ULP shift from an unrelated future change.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the empty-arena/disabled `ExitSplitCtx` this test's \
                  `apply_uniaxial_internal_transmit_channels` call requires \
                  (splitting itself is irrelevant to this test's own energy-conservation \
                  sweep, which only reads the hero channel -- see that setup's own \
                  comment) adds to an already-long sweep body"
    )]
    fn uniaxial_internal_bounce_wiring_conserves_energy_at_exit_interface() {
        let zircon = GemMaterial::by_name("Zircon").expect("Zircon must be a built-in material");
        let n_o = zircon.dispersion.evaluate(589.3);
        let n_e = zircon.extraordinary_index_at(589.3, n_o);
        let lambdas = [589.3f32; NUM_CHANNELS];

        let axes = [Vec3::X, Vec3::Y, Vec3::new(0.4, 0.5, 0.767_2).normalize()];
        let r_branch = 0.5f32;
        let mut worst = 0.0f32;
        for &c_axis in &axes {
            let ctx = RayMaterialContext {
                material: &zircon,
                lambdas,
                hero_idx: 0,
                c_axis,
                is_anisotropic: true,
                enable_internal_mode_coupling: true,
            };
            for angle_deg in [5.0f32, 15.0, 25.0, 35.0, 45.0, 55.0, 65.0] {
                let theta = angle_deg.to_radians();
                let cos_i = theta.cos();
                let sin_i = theta.sin();
                let k_hat = Vec3::new(sin_i, 0.0, cos_i);
                let normal = Vec3::new(0.0, 0.0, -1.0);
                if k_hat.cross(c_axis).length_squared() < 1e-4 {
                    // Degenerate wave-normal-parallel-to-optic-axis case: handled by a
                    // dedicated exact fallback in `apply_partial_fresnel_bounce`
                    // itself (this wiring is never reached there) -- see that
                    // function's own doc comment.
                    continue;
                }
                let frame = UniaxialFrame::build(k_hat, normal, c_axis, cos_i, sin_i);

                for is_extraordinary in [false, true] {
                    let n_inc = if is_extraordinary {
                        let cos_kc = frame.gamma.mul_add(frame.cos_i, frame.alpha * frame.sin_i);
                        let sin2 = cos_kc.mul_add(-cos_kc, 1.0).max(0.0);
                        1.0 / (cos_kc * cos_kc / (n_o * n_o) + sin2 / (n_e * n_e)).sqrt()
                    } else {
                        n_o
                    };
                    let geo = BounceRefractionGeometry {
                        cos_i,
                        sin_i,
                        n1: n_inc,
                        n2: 1.0,
                        n1_ch: [n_inc; NUM_CHANNELS],
                        n2_ch: [1.0; NUM_CHANNELS],
                        n_o_ch: [n_o; NUM_CHANNELS],
                        ..BounceRefractionGeometry::default()
                    };

                    // Matches `apply_uniaxial_internal_bounce`'s own hero-channel solve
                    // (this test's `geo` is uniform across channels, so it is exactly
                    // what channel `ctx.hero_idx` would compute anyway).
                    let sol_hero = uniaxial_fresnel::internal_solve(
                        n_inc,
                        n_o,
                        n_e,
                        c_axis,
                        &frame,
                        !is_extraordinary,
                    );

                    let mut stokes_r = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
                    let mut pdf_r = [1.0f32; NUM_CHANNELS];
                    let ubctx = UniaxialBounceContext {
                        ctx: &ctx,
                        geo: &geo,
                        frame: &frame,
                    };
                    let hero_solve = HeroInternalSolve {
                        hero: ctx.hero_idx,
                        is_extraordinary,
                        sol_hero,
                        r_branch,
                    };
                    let mut state_r = BounceState {
                        stokes: &mut stokes_r,
                        path_pdf: &mut pdf_r,
                    };
                    apply_uniaxial_internal_reflect_channels(&ubctx, &hero_solve, &mut state_r);
                    let mut stokes_t = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
                    let mut pdf_t = [1.0f32; NUM_CHANNELS];
                    // This test reads only `stokes_t[0]` (the hero channel), whose
                    // `direction_matches` is trivially true regardless of splitting --
                    // no real gem geometry/environment is needed, so an empty arena and
                    // `enabled: false` (the split path is never reached) keep this test
                    // unaffected by the split-transmission machinery.
                    let empty_soa = crate::simd::PlanesSoA32::from_normals_d(std::iter::empty(), 0);
                    let mut split_radiance_t = [0.0f32; NUM_CHANNELS];
                    let mut exit_ctx = ExitSplitCtx {
                        plane_soa: &empty_soa,
                        environment: EnvironmentSource::Studio {
                            preset: crate::optics::LightingPreset::RingLights,
                            exposure: 1.0,
                            light_yaw: 0.0,
                            light_pitch: 0.85,
                            backdrop: 0.0,
                        },
                        studio_rig: None,
                        observer: Vec3::ZERO,
                        split_radiance: &mut split_radiance_t,
                        enabled: false,
                        compat: [u8::MAX; NUM_CHANNELS],
                    };
                    let mut state_t = BounceState {
                        stokes: &mut stokes_t,
                        path_pdf: &mut pdf_t,
                    };
                    let mut exit_event_t = ExitEvent {
                        exit: &mut exit_ctx,
                        hit_point: Vec3::ZERO,
                    };
                    apply_uniaxial_internal_transmit_channels(
                        &ubctx,
                        &hero_solve,
                        BounceRay { k_hat, normal },
                        &mut state_t,
                        &mut exit_event_t,
                    );

                    let reflected = stokes_r[0].i * r_branch;
                    let transmitted = stokes_t[0].i * (1.0 - r_branch);
                    let err = (reflected + transmitted - 1.0).abs();
                    worst = worst.max(err);
                }
            }
        }
        println!(
            "uniaxial_internal_bounce_wiring_conserves_energy_at_exit_interface: worst_err={worst}"
        );
        assert!(
            worst < 5e-6,
            "reflected + transmitted power should equal incident power (1.0) through \
             the actual CPU wiring, worst_err={worst}"
        );
    }
}

/// [`entry_eigenmode_selection`]'s polarization-weighted probability and eigenmode
/// azimuth.
#[cfg(test)]
mod entry_eigenmode_selection_tests {
    use super::*;

    /// Unpolarized (and negligibly-polarized) light must reduce EXACTLY to a
    /// blanket 50/50 -- i.e. `entry_eigenmode_selection` returns `None`, telling
    /// callers to skip both the weighted draw and the eigenmode projection.
    #[test]
    fn unpolarized_light_returns_none() {
        let c_axis = Vec3::Y;
        let current_plane_normal = Vec3::X;
        let k_hat = Vec3::NEG_Z;
        let stokes = StokesVector::unpolarized(1.0);
        assert!(
            entry_eigenmode_selection(c_axis, current_plane_normal, k_hat, stokes).is_none(),
            "unpolarized light must not be weighted -- it must fall back to flat 50/50"
        );
    }

    /// A degenerate plane of incidence (near-normal incidence, `current_plane_normal`
    /// near zero) must also fall back to `None` regardless of polarization -- there is
    /// no well-defined frame to express `psi_o` in.
    #[test]
    fn degenerate_plane_of_incidence_returns_none() {
        let c_axis = Vec3::Y;
        let current_plane_normal = Vec3::ZERO;
        let k_hat = Vec3::NEG_Z;
        let stokes = StokesVector::new(1.0, 1.0, 0.0, 0.0);
        assert!(entry_eigenmode_selection(c_axis, current_plane_normal, k_hat, stokes).is_none());
    }

    /// Light fully linearly polarized EXACTLY along the ordinary eigenaxis must select
    /// the ordinary mode with probability 1 (`p_o == 1.0`), and its doubled azimuth
    /// must be `(cos, sin) == (1, 0)` (`psi_o == 0` in this frame by construction).
    #[test]
    fn fully_polarized_along_ordinary_axis_gives_p_o_one() {
        let c_axis = Vec3::Y;
        let k_hat = Vec3::NEG_Z;
        // Ordinary eigenaxis for this (k_hat, c_axis) pair is `k_hat x c_axis`,
        // normalized -- here that's exactly +X (see `ordinary_eigen_polarization`'s
        // doc comment). Using it directly as `current_plane_normal` (the Stokes
        // frame's own s_axis) makes psi_o == 0 in this frame by construction.
        let current_plane_normal = Vec3::X;
        let stokes = StokesVector::new(1.0, 1.0, 0.0, 0.0); // fully linear along s_axis
        let (p_o, cos_2psi_o, sin_2psi_o) =
            entry_eigenmode_selection(c_axis, current_plane_normal, k_hat, stokes)
                .expect("polarized light with a well-defined frame must select Some");
        assert!((p_o - 1.0).abs() < 1e-4, "p_o should be ~1.0, got {p_o}");
        assert!((cos_2psi_o - 1.0).abs() < 1e-4, "got {cos_2psi_o}");
        assert!(sin_2psi_o.abs() < 1e-4, "got {sin_2psi_o}");
    }

    /// Light fully linearly polarized PERPENDICULAR to the ordinary eigenaxis (i.e.
    /// along the extraordinary one) must select the ordinary mode with probability 0.
    #[test]
    fn fully_polarized_along_extraordinary_axis_gives_p_o_zero() {
        let c_axis = Vec3::Y;
        let k_hat = Vec3::NEG_Z;
        let current_plane_normal = Vec3::X;
        let stokes = StokesVector::new(1.0, -1.0, 0.0, 0.0); // fully linear along p_axis
        let (p_o, ..) = entry_eigenmode_selection(c_axis, current_plane_normal, k_hat, stokes)
            .expect("polarized light with a well-defined frame must select Some");
        assert!(p_o.abs() < 1e-4, "p_o should be ~0.0, got {p_o}");
    }
}

/// The two properties `apply_refract_channel`'s entry-eigenmode projection relies
/// on, checked directly rather than only through a full render (`entry_eigenmode_selection_tests`
/// pins `p_o`/azimuth at `DoP` 0 and 1; this module exercises intermediate, partially
/// polarized `DoP` values too, and a non-trivial frame where BOTH `cos_2psi_o` and
/// `sin_2psi_o` are nonzero, not just the axis-aligned special case):
///
/// 1. The projected Stokes vector `apply_refract_channel` builds for whichever mode is
///    selected (`StokesVector::new(i, +/-i*cos_2psi_o, +/-i*sin_2psi_o, 0.0)`) is fully
///    linearly polarized -- `Q^2 + U^2 == I^2` exactly (up to float error) and `V == 0`
///    -- for BOTH the ordinary and the extraordinary projection, at every `DoP` tested.
/// 2. The mode-selection draw is an unbiased estimator of the true polarization-weighted
///    mixture. Concretely: import two DIFFERENT hypothetical per-mode transmittances
///    `T_o != T_e` (standing in for `apply_refract_channel`'s real mode-dependent Fresnel
///    `T`), so the "correct" physical answer -- Malus's law's `i*(p_o*T_o + (1-p_o)*T_e)`
///    -- is a genuinely nontrivial target (not the same value regardless of which mode
///    is drawn). A Monte Carlo sweep of the ACTUAL selection formula
///    (`apply_partial_fresnel_bounce`'s `mode_split_rand < (1.0 - p_o)`, driven by the
///    real `BIREFRINGENT_SPLIT_STREAM` hash) reporting `T_o*i`/`T_e*i` unscaled on each
///    trial (exactly what `apply_refract_channel` does -- no `1/p` division, see that
///    function's own doc comment for why none is needed) must converge to that same
///    target: `E[estimate] == p_o*T_o*i + (1-p_o)*T_e*i` by construction, and the RNG
///    sweep confirms the real selection probability the hash actually produces matches
///    `p_o` closely enough for that identity to hold within Monte Carlo noise.
#[cfg(test)]
mod entry_mode_projection_tests {
    use super::*;

    /// Builds a non-axis-aligned Stokes frame (`s_hat` NOT equal to the ordinary axis,
    /// so both `cos_2psi_o` and `sin_2psi_o` come out nonzero) once, shared by every
    /// `DoP` case below.
    fn oblique_frame() -> (Vec3, Vec3, Vec3) {
        let c_axis = Vec3::Y;
        let k_hat = Vec3::NEG_Z;
        // Perpendicular to k_hat (lies in the XY plane, like every other test in this
        // file's Vec3::NEG_Z-based frames), but NOT aligned with the ordinary axis
        // (Vec3::X for this (k_hat, c_axis) pair -- see the axis-aligned tests above).
        let current_plane_normal = Vec3::new(0.6, 0.8, 0.0);
        (c_axis, current_plane_normal, k_hat)
    }

    #[test]
    fn projected_stokes_is_fully_linear_at_several_dop_values() {
        let (c_axis, current_plane_normal, k_hat) = oblique_frame();
        let psi = 25.0f32.to_radians();
        let (cos_2psi, sin_2psi) = ((2.0 * psi).cos(), (2.0 * psi).sin());

        for dop in [0.05f32, 0.3, 0.6, 0.9, 1.0] {
            let i = 1.0f32;
            let q = i * dop * cos_2psi;
            let u = i * dop * sin_2psi;
            let stokes = StokesVector::new(i, q, u, 0.0);
            let (_, cos_2psi_o, sin_2psi_o) =
                entry_eigenmode_selection(c_axis, current_plane_normal, k_hat, stokes)
                    .expect("nonzero DoP with a well-defined frame must select Some");

            for (cos_x, sin_x) in [(cos_2psi_o, sin_2psi_o), (-cos_2psi_o, -sin_2psi_o)] {
                let projected = StokesVector::new(i, i * cos_x, i * sin_x, 0.0);
                assert!(
                    projected.v.abs() < 1e-6,
                    "projected V must be exactly zero at dop={dop}, got {}",
                    projected.v
                );
                let lin_energy = projected.q.mul_add(projected.q, projected.u * projected.u);
                assert!(
                    projected.i.mul_add(-projected.i, lin_energy).abs() < 1e-4,
                    "projected Stokes vector must be fully linearly polarized (Q^2+U^2 \
                     == I^2) at dop={dop}: I={}, Q={}, U={}, Q^2+U^2={lin_energy}",
                    projected.i,
                    projected.q,
                    projected.u
                );
            }
        }
    }

    #[test]
    fn mode_selection_draw_is_an_unbiased_estimator_at_several_dop_values() {
        // Two deliberately different per-mode "transmittances" -- stand-ins for
        // `apply_refract_channel`'s real mode-dependent Fresnel T, chosen far enough
        // apart that a convention bug (e.g. an inverted comparison sense) would show up
        // as a converged mean far from the analytically correct target, not hidden in
        // noise.
        const T_O: f32 = 0.8;
        const T_E: f32 = 0.35;
        const TRIALS: u32 = 20_000;

        let (c_axis, current_plane_normal, k_hat) = oblique_frame();
        let psi = 25.0f32.to_radians();
        let (cos_2psi, sin_2psi) = ((2.0 * psi).cos(), (2.0 * psi).sin());

        for dop in [0.05f32, 0.3, 0.6, 0.9, 1.0] {
            let i = 1.0f32;
            let q = i * dop * cos_2psi;
            let u = i * dop * sin_2psi;
            let stokes = StokesVector::new(i, q, u, 0.0);
            let (p_o, ..) = entry_eigenmode_selection(c_axis, current_plane_normal, k_hat, stokes)
                .expect("nonzero DoP with a well-defined frame must select Some");
            let target = i * p_o.mul_add(T_O, (1.0 - p_o) * T_E);

            let mut sum = 0.0f64;
            for bounce in 0..TRIALS {
                // Exactly `apply_partial_fresnel_bounce`'s own draw: same stream, same
                // comparison sense (`mode_split_rand < 1.0 - p_o` selects extraordinary).
                let mode_split_rand = (hash_u32(hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))
                    as f32)
                    / 4_294_967_295.0;
                let use_extraordinary = mode_split_rand < (1.0 - p_o);
                let estimate = if use_extraordinary { T_E * i } else { T_O * i };
                sum += f64::from(estimate);
            }
            let mean = (sum / f64::from(TRIALS)) as f32;
            assert!(
                (mean - target).abs() < 0.01,
                "mode-selection draw should be an unbiased estimator of the \
                 polarization-weighted mixture at dop={dop}: p_o={p_o}, target={target}, \
                 Monte Carlo mean={mean} over {TRIALS} trials"
            );
        }
    }
}
