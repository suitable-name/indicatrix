//! The frosted-facet BSDF.
//!
//! [`apply_frosted_bounce`]'s diffuse reflect/transmit dispatch, its cosine-weighted
//! hemisphere direction sampler, and the exterior-side next-event-estimation
//! contribution for the two outcomes that land outside the gem.

use super::{
    super::{
        NUM_CHANNELS,
        environment::sample_environment_for_nee,
        refraction::{
            BounceRefractionGeometry, R_UNPOL_PDF_MAX, R_UNPOL_PDF_MIN, RayMaterialContext,
        },
        sampling::{
            BIREFRINGENT_SPLIT_STREAM, FRESNEL_BRANCH_STREAM, FROSTED_DIR_U_STREAM,
            FROSTED_DIR_V_STREAM, FROSTED_NEE_ENV_DIR_U_STREAM, FROSTED_NEE_ENV_DIR_V_STREAM,
            hash_u32,
        },
    },
    NeeContext, balance_heuristic, frosted_orthonormal_basis,
};
use crate::optics::polarization::StokesVector;
use glam::Vec3;

/// Cosine-weighted hemisphere direction about `n`, from two independent uniform
/// `[0, 1)` randoms (Malley's method -- polar mapping, not the concentric-disk variant,
/// for a direct WGSL trig-call translation). See `apply_frosted_bounce` for why its
/// call sites need no explicit pdf division.
///
/// `pub(crate)`: `renderer::gpu::transport_check`'s Tier 2 ULP check compares this real
/// function against `shaders/transport_physics.wgsl`'s translation. Visibility only.
pub(crate) fn cosine_weighted_hemisphere(u1: f32, u2: f32, n: Vec3) -> Vec3 {
    let r = u1.sqrt();
    let theta = 2.0 * std::f32::consts::PI * u2;
    let (sin_t, cos_t) = theta.sin_cos();
    let (t, b) = frosted_orthonormal_basis(n);
    (t * (r * cos_t) + b * (r * sin_t) + n * (1.0 - u1).max(0.0).sqrt()).normalize_or_zero()
}

/// Next-event estimation for the two [`apply_frosted_bounce`] outcomes whose
/// sampled hemisphere lands on the EXTERIOR side of the gem -- see that function's own
/// doc comment ("Sign convention for NEE eligibility") for exactly which two branches
/// those are and why.
///
/// # Why this needs no shadow ray, unlike `nee_contribution_hg_scatter`
///
/// [`nee_contribution_hg_scatter`](super::nee_contribution_hg_scatter)'s scattering
/// point sits somewhere in the INTERIOR of the (necessarily convex) polyhedron, so it
/// must trace a shadow ray to find which exit facet the light-sampled direction would
/// leave through, and pay that facet's own Fresnel transmittance. This function's
/// bounce point, by contrast, is already ON the polyhedron's own surface, and
/// `ext_normal` is that facet's true (mathematical) outward-facing normal for this
/// specific outcome. A convex solid's own surface point, moving into the outward
/// half-space about its own true normal, never re-enters the solid for any positive
/// distance -- so the environment is unconditionally visible along any `dir` with
/// `dir.dot(ext_normal) > 0`, with no occlusion test and no second Fresnel interface to
/// cross (the interface AT this point was already paid for by the caller's
/// `r_unpol`/`t_unpol` branch-selection division before `stokes` reached here).
///
/// # Simplification, mirroring `nee_contribution_hg_scatter`
///
/// One broadband cosine-weighted-hemisphere ("Lambertian") BSDF density
/// (`cos_light / pi`) stands in for the true frosted BSDF's competing technique here,
/// exactly like [`apply_frosted_bounce`] itself already uses one broadband `r_unpol`/
/// `t_unpol` split for every channel (see that function's own "The model, honestly"
/// section) -- consistent with, not an additional approximation on top of, the rest of
/// this BSDF's treatment.
///
/// Consumes exactly one new 2D RNG draw ([`FROSTED_NEE_ENV_DIR_U_STREAM`]/
/// [`FROSTED_NEE_ENV_DIR_V_STREAM`]) whenever `nee.enabled` -- callers gate the call
/// itself on eligibility (see [`apply_frosted_bounce`]), so a bounce that lands on the
/// interior side, or any bounce with NEE disabled, never touches this stream at all.
///
/// `stokes` must already carry this branch's own throughput (post `/r_unpol` or
/// `/t_unpol` rescale, exactly like
/// [`nee_contribution_hg_scatter`](super::nee_contribution_hg_scatter)'s identical
/// precondition on its own `stokes` argument).
pub(crate) fn nee_contribution_frosted_exterior(
    nee: NeeContext<'_>,
    lambdas: &[f32; NUM_CHANNELS],
    ext_normal: Vec3,
    rng_seed: u32,
    bounce: u32,
    stokes: &[StokesVector; NUM_CHANNELS],
    radiance: &mut [f32; NUM_CHANNELS],
) {
    if !nee.enabled {
        return;
    }
    let u0 = (hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_U_STREAM)) as f32)
        / 4_294_967_295.0;
    let u1 = (hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_V_STREAM)) as f32)
        / 4_294_967_295.0;
    let Some(sample) = sample_environment_for_nee(nee.environment, u0, u1) else {
        return;
    };

    // `EnvironmentMap::sample` draws from the FULL sphere, not this bounce's own
    // hemisphere -- roughly half its draws land behind the surface (`cos_light <= 0.0`)
    // and must be rejected outright, not folded in as negative energy. Not a bias: the
    // BSDF-sampled continuation (`new_dir`, always in the correct hemisphere by
    // construction) still gets its own full chance to reach the environment there.
    let cos_light = sample.dir.dot(ext_normal);
    if cos_light <= 0.0 {
        return;
    }

    // The competing (BSDF) technique's own density at this SAME light-sampled
    // direction -- the balance heuristic's other half. See this function's own
    // "Simplification" doc section for why a plain Lambertian `cos / pi` (rather than
    // the frosted BSDF's actual `r_unpol`/`t_unpol`-scaled value) is the right density
    // to compete the light sample against.
    let brdf_pdf = cos_light / std::f32::consts::PI;
    let mis_weight = balance_heuristic(sample.pdf, brdf_pdf);
    if mis_weight <= 0.0 {
        return;
    }

    let common = brdf_pdf * mis_weight / sample.pdf;
    for k in 0..NUM_CHANNELS {
        let env_k = crate::renderer::env_map::rgb_to_spectral_radiance(sample.rgb, lambdas[k]);
        radiance[k] = (stokes[k].intensity() * common * env_k).mul_add(1.0, radiance[k]);
    }
}

/// The replacement for the specular TIR/partial-reflect/refract dispatch at a
/// `FacetFinish::Frosted` facet.
///
/// # The model, honestly
///
/// A real bruted/ground surface's reflectance doesn't carry the polished interface's
/// sharp per-wavelength Fresnel structure -- roughness averages over local micro-facet
/// angles, washing out the fine structure -- so this uses ONE broadband reflect/
/// transmit fraction (`r_unpol`, hero channel only) for every spectral channel, unlike
/// the polished path's per-channel `r_unpol_k`. Every channel shares both the sampled
/// direction (roughness scattering isn't meaningfully dispersive) and the reflect/
/// transmit split probability (a modeling simplification).
///
/// Direction is drawn from the cosine-weighted hemisphere about the correct macroscopic
/// normal (`normal` reflect, `-normal` transmit) -- standard importance sampling for a
/// Lambertian BRDF/BTDF, so `f * cos(theta) / pdf` is the constant albedo (`1.0`). The
/// branch itself is drawn from `FRESNEL_BRANCH_STREAM` with probability `r_unpol` (or
/// `t_unpol`), and `path_pdf[k] *= r_unpol` (or `t_unpol`) below divides by exactly that
/// selection probability -- the intensity itself is left unscaled (weight `1`, not
/// `1 / r_unpol`): the branch's own energy fraction and its own selection probability
/// are the same number, so they cancel and no throughput division belongs here.
///
/// Depolarizes every channel (`StokesVector::unpolarized`): multiple internal
/// micro-scattering events scramble the coherent phase relationship polarization
/// depends on.
///
/// # Sign convention for NEE eligibility
///
/// `normal` arrives here ALREADY resolved by the caller (`transport::apply_interior_segment`)
/// to the facet's outward normal flipped to face the current medium: unflipped (the raw,
/// truly outward-pointing facet normal) when `!inside_gem` (this bounce is happening at a
/// ray currently in air, about to enter or bounce off the gem from outside), and flipped
/// to point INWARD when `inside_gem` (this bounce is happening at a ray currently inside
/// the crystal, hitting a facet from the inside). Given that:
///
/// - **`!inside_gem`, reflect branch** (hemisphere about `normal`, i.e. the true outward
///   normal): the ray was already outside and bounces back outside. `inside_gem` is
///   unchanged (`false`) -- EXTERIOR side. NEE-eligible.
/// - **`!inside_gem`, transmit branch** (hemisphere about `-normal`, i.e. pointing
///   inward): the ray enters the crystal for the first time through this frosted facet.
///   `inside_gem` flips to `true` -- INTERIOR side. Not handled here: a light sample
///   drawn into the interior hemisphere would need to find where THIS path eventually
///   exits the solid and pay that exit facet's own Fresnel transmittance, exactly the
///   shadow-ray search `nee_contribution_hg_scatter` already does from an interior
///   scattering point. Deliberately not implemented (documented gap, not an oversight):
///   entry-facet frosted transmission is the less common case in practice (a frosted
///   girdle scattering light back OUT to the camera -- the exterior-transmit case below
///   -- is the one every existing frosted-girdle test/scene exercises), and adding it
///   would double this function's shadow-ray-tracing surface for comparatively little
///   variance-reduction benefit. `pending_light_mis` is `None` for this branch, so a
///   phase-sampled continuation that (rarely, since it heads inward) still escapes
///   directly next bounce keeps its full weight rather than being MIS-weighted
///   against a light sample this function never drew.
/// - **`inside_gem`, forced-reflect (TIR) branch** (hemisphere about `normal`, i.e.
///   ALREADY flipped inward): TIR is only reachable with `inside_gem == true` (`n1 > n2`
///   for the hero channel -- same premise `dispatch_bounce`'s polished-path doc comment
///   states), so this hemisphere stays interior. `inside_gem` unchanged (`true`) --
///   INTERIOR side. Not NEE-eligible (an internal reflection has no direct environment
///   visibility at this point).
/// - **`inside_gem`, reflect branch** (hemisphere about `normal`, flipped inward): an
///   internal reflection off a frosted facet, staying inside. INTERIOR side. Not
///   NEE-eligible, for the same reason as the TIR branch above.
/// - **`inside_gem`, transmit branch** (hemisphere about `-normal`, i.e. the true
///   outward normal once the inward flip is undone): the ray exits the crystal to the
///   environment through this frosted facet -- the frosted-girdle scene every existing
///   test exercises. `inside_gem` flips to `false` -- EXTERIOR side. NEE-eligible.
///
/// In short: NEE fires exactly when this bounce's own `inside_gem` input and its
/// resulting output disagree in the "becoming exterior" direction reflect can never
/// take (`!inside_gem` reflect) or transmit does take (`inside_gem` transmit) -- both
/// cases share the property that the branch's own hemisphere normal (`normal` for the
/// first, `-normal` for the second) is the TRUE, un-flipped outward-facing normal.
///
/// # Estimator composition -- why this needs no chromatic termination
///
/// The polished path drops a companion channel to exactly zero when its specular
/// direction misses the hero-driven direction, because a delta BSDF has zero density
/// elsewhere. Neither premise holds here: direction is shared and the BSDF is a smooth
/// hemisphere, so `path_pdf[k] *= r_unpol` (or `t_unpol`) uses the same factor for
/// every channel.
///
/// Reuses `FRESNEL_BRANCH_STREAM` for the reflect-vs-transmit branch (divides by its
/// own selection probability, disjoint energy). `BIREFRINGENT_SPLIT_STREAM` on an
/// air->crystal entry picks which ~0.5 energy SHARE this path becomes, so it is NOT
/// divided by its own 0.5 (same reasoning as `apply_refract_bounce`). Mode
/// re-coupling is applied by the caller.
///
/// # Why the exterior NEE deposit needs no separate `nee_xyz` accumulator (F-09b)
///
/// `nee_contribution_frosted_exterior` deposits straight into the caller's shared
/// `radiance` array (weighted, like every other channel's contribution, by the FINAL
/// post-loop `path_pdf` at `trace_spectral_ray_inner`'s integration step) rather than
/// into a separate `nee_xyz` accumulator integrated with THIS moment's `path_pdf`, the
/// fix `try_scatter_step`'s identical deposit needed (see that function's own "NEE
/// spectral weighting" doc section). That asymmetry is safe, not an oversight: both
/// NEE-eligible outcomes here are, by construction, cases where `new_dir` lies in the
/// open half-space strictly outward of this bounce point's TRUE outward normal (a
/// cosine-weighted hemisphere about `normal` for the `!inside_gem` reflect branch,
/// about `-normal` for the `inside_gem` transmit branch -- see "Sign convention for NEE
/// eligibility" above). A convex polyhedron's own surface point moving into that exact
/// half-space can never re-intersect the solid at any positive distance (the same
/// convexity property `nee_contribution_hg_scatter`'s and `try_split_exit_channel`'s own
/// doc comments already lean on) -- so the VERY NEXT bounce-loop iteration is guaranteed
/// to find no facet hit and escape directly, meaning no further `path_pdf[k] *=` update
/// can happen between this deposit and the trace's final integration. The FINAL
/// `path_pdf` this deposit is (eventually) weighted by is therefore always identical to
/// the `path_pdf` live at the moment of the deposit itself, so riding the shared
/// `radiance` array is exact here, unlike the HG scattering-point case (an interior
/// point with an unbounded number of further bounces still ahead of it).
///
/// # Return value
///
/// The fourth tuple element is the `pending_light_mis` carry, mirroring
/// `ScatterStepOutcome::ScatteredAndSurvived`'s identical carry for the
/// Henyey-Greenstein path: `Some(cos(theta_new) / pi)` -- the cosine-weighted-hemisphere
/// BSDF's own density at the just-sampled continuation direction `new_dir` -- whenever
/// `nee.enabled` and this outcome is NEE-eligible (see above), `None` otherwise
/// (including every trace with NEE disabled). The caller stashes this exactly like the
/// HG path's own carry: consumed only by the VERY NEXT bounce-loop iteration's escape
/// check, and only if that iteration's ray escapes directly.
// `pub(crate)`: `renderer::gpu::transport_check`'s Tier 2 ULP check compares this real
// function against `shaders/transport_physics.wgsl`'s translation. Visibility only.
#[expect(
    clippy::too_many_arguments,
    reason = "bundles ctx/geo, the same contexts apply_partial_fresnel_bounce and \
              apply_refract_bounce use in refraction.rs; nee/lambdas/radiance are \
              the HDR-map NEE inputs/output, mirroring try_scatter_step's identical \
              addition for the Henyey-Greenstein path; the rest matches \
              transport_physics.wgsl's own apply_frosted_bounce parameter-for-parameter"
)]
pub(crate) fn apply_frosted_bounce(
    ctx: &RayMaterialContext,
    geo: &BounceRefractionGeometry,
    normal: Vec3,
    inside_gem: bool,
    is_extraordinary: bool,
    rng_seed: u32,
    bounce: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
    nee: NeeContext<'_>,
    lambdas: &[f32; NUM_CHANNELS],
    radiance: &mut [f32; NUM_CHANNELS],
) -> (Vec3, bool, Option<bool>, Option<f32>) {
    let u1 =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_DIR_U_STREAM)) as f32) / 4_294_967_295.0;
    let u2 =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_DIR_V_STREAM)) as f32) / 4_294_967_295.0;

    if geo.sin2_t > 1.0 {
        // Forced reflect (TIR), probability 1 -- no draw, no pdf division, mirroring
        // `apply_tir_bounce` for the polished path. Always interior -- see this
        // function's "Sign convention for NEE eligibility" doc section -- so no NEE.
        let new_dir = cosine_weighted_hemisphere(u1, u2, normal);
        for s in &mut *stokes {
            *s = StokesVector::unpolarized(s.intensity());
        }
        return (new_dir, inside_gem, None, None);
    }

    let cos_t = (1.0 - geo.sin2_t).max(0.0).sqrt();
    let r_s = f32::mul_add(geo.n2, -cos_t, geo.n1 * geo.cos_i)
        / f32::mul_add(geo.n2, cos_t, geo.n1 * geo.cos_i);
    let r_p = f32::mul_add(geo.n1, -cos_t, geo.n2 * geo.cos_i)
        / f32::mul_add(geo.n1, cos_t, geo.n2 * geo.cos_i);
    let r_unpol = (0.5 * r_p.mul_add(r_p, r_s * r_s)).clamp(R_UNPOL_PDF_MIN, R_UNPOL_PDF_MAX);
    let rng_bounce =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM)) as f32) / 4_294_967_295.0;

    if rng_bounce < r_unpol {
        let new_dir = cosine_weighted_hemisphere(u1, u2, normal);
        for k in 0..NUM_CHANNELS {
            stokes[k] = StokesVector::unpolarized(stokes[k].intensity());
            path_pdf[k] *= r_unpol;
        }
        // Exterior-side NEE only when this reflect happened while already
        // outside the gem -- see "Sign convention for NEE eligibility" above.
        let phase_pdf_for_mis = if nee.enabled && !inside_gem {
            nee_contribution_frosted_exterior(
                nee, lambdas, normal, rng_seed, bounce, stokes, radiance,
            );
            Some(new_dir.dot(normal) / std::f32::consts::PI)
        } else {
            None
        };
        (new_dir, inside_gem, None, phase_pdf_for_mis)
    } else {
        let new_dir = cosine_weighted_hemisphere(u1, u2, -normal);
        let entering_anisotropic = !inside_gem && ctx.is_anisotropic;
        // Mode SELECTION is still a stochastic 50/50 draw -- no throughput weighting
        // accompanies it (a `split_pdf` divisor would estimate energy no interface can
        // deliver; see `apply_refract_bounce` in `refraction.rs`).
        let use_extraordinary = if entering_anisotropic {
            let split_rand = (hash_u32(rng_seed ^ hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))
                as f32)
                / 4_294_967_295.0;
            split_rand < 0.5
        } else {
            is_extraordinary
        };
        let t_unpol = 1.0 - r_unpol;
        for k in 0..NUM_CHANNELS {
            // No `/ split_pdf` here (see `apply_refract_channel` in `refraction.rs`):
            // the selected mode already carries only its ~0.5 energy share, so dividing
            // by the 0.5 selection probability on top would double-count it.
            stokes[k] = StokesVector::unpolarized(stokes[k].intensity());
            // No `* split_pdf` either -- `spectral_mis_weight` is scale-invariant under
            // multiplying every channel's `path_pdf` by the same uniform factor, so this
            // was a no-op on the actual MIS weight, just one risking underflow.
            path_pdf[k] *= t_unpol;
        }
        let ext_normal = -normal;
        // Exterior-side NEE only when this transmit happened while inside
        // the gem (exiting to the environment) -- see "Sign convention for NEE
        // eligibility" above.
        let phase_pdf_for_mis = if nee.enabled && inside_gem {
            nee_contribution_frosted_exterior(
                nee, lambdas, ext_normal, rng_seed, bounce, stokes, radiance,
            );
            Some(new_dir.dot(ext_normal) / std::f32::consts::PI)
        } else {
            None
        };
        (
            new_dir,
            !inside_gem,
            entering_anisotropic.then_some(use_extraordinary),
            phase_pdf_for_mis,
        )
    }
}
