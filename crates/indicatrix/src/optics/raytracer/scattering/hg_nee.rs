//! Next-event estimation at a Henyey-Greenstein scattering point.
//!
//! [`nee_contribution_hg_scatter`] draws one light sample, traces the shadow ray to the
//! stone's exit, and deposits the balance-heuristic-weighted direct light. [`ShadowAlphas`]
//! is how the shadow ray's own per-channel extinction is evaluated: at the SHADOW
//! direction for an anisotropic material, since that is what the phase-sampled
//! continuation in the same direction pays (see its doc comment for the bias the earlier
//! incoming-direction alphas caused). Split out of `hg.rs`, which keeps the free-path
//! estimator, the phase function and the bounce-loop wrapper.

use super::{
    super::{
        NUM_CHANNELS,
        absorption::channel_absorption_alphas_assigned,
        camera::{FacetFinish, Ray},
        environment::{EnvironmentSource, sample_environment_for_nee},
        intersect_stone::intersect_stone_soa,
        refraction::{RayMaterialContext, RayWavelengthCache},
        sampling::{NEE_ENV_DIR_U_STREAM, NEE_ENV_DIR_V_STREAM, hash_u32},
    },
    HG_NEE_MAX_CROSSINGS, NeeContext, balance_heuristic,
    hg::henyey_greenstein_phase,
};
use crate::optics::{polarization::StokesVector, raytracer::camera::HitRecord};
use glam::Vec3;

/// How far along the sampled direction the shadow probe starts from the scattering point,
/// clearing the point itself before the intersection test (the same `1e-4`
/// `try_split_exit_channel` uses). The probe's origin lies that far INSIDE the material,
/// so every in-material segment the probe reports is that much shorter than the path a
/// continuation starting at the scattering point itself travels; [`single_exit`] and
/// [`walk_exits`] add it back. `transport_physics/04_scattering.wgsl`'s
/// `HG_SHADOW_PROBE_OFFSET` is the twin.
const SHADOW_PROBE_OFFSET: f32 = 1e-4;

/// The per-channel extinction the shadow ray of [`nee_contribution_hg_scatter`] pays,
/// evaluated at the SHADOW direction.
///
/// A phase-sampled continuation leaving the scattering point in direction `w` pays, on its
/// next segment, `channel_absorption_alphas_assigned` evaluated at `w` (the bounce loop
/// recomputes the alphas for every segment from that segment's own wave normal). The
/// shadow ray in direction `w` is the light-sampling technique's estimate of that same
/// path, so it must pay the same coefficients. For an anisotropic material the assigned
/// mode's absorption depends on the wave normal: the extraordinary mode of a uniaxial
/// crystal reads `alpha_o cos^2(theta) + alpha_e sin^2(theta)` against the optic axis, so
/// a ray arriving along the axis (the camera ray through the table of a `c_axis = +Y`
/// stone) carries the ordinary coefficient while its shadow rays across the axis should
/// carry the extraordinary one. Feeding the shadow ray the segment's alphas instead
/// (evaluated at the INCOMING wave normal) made every NEE deposit of such a path pay the
/// wrong Beer-Lambert factor, a bias the phase-sampled escape could not cancel: measured
/// on the built-in ruby as NEE-on reading 0.6 % darker in X and 0.45 % darker in Y than
/// NEE-off (`transport::tests::nee_tests`'s
/// `ruby_scattering_nee_on_and_off_agree_within_half_a_percent`), concentrated at
/// 480-510 nm and 590-630 nm where the e-ray/o-ray coefficients differ most.
///
/// [`Self::Fixed`] reuses one array for every direction. That is exact for an isotropic
/// material (`!RayMaterialContext::is_anisotropic`: `channel_absorption_alphas_assigned`
/// then takes its direction-independent unpolarised average, and every built-in and
/// `GemMaterial::new_custom` isotropic material carries an isotropic tensor), so such
/// scenes stay bit-identical to the pre-fix estimator, and it is what the GPU Tier 2
/// harness (`renderer::gpu::transport_check::nee`) feeds its fixed per-case alphas
/// through, matching the standalone WGSL kernel's explicit `alphas` input.
#[derive(Clone, Copy)]
pub(crate) enum ShadowAlphas<'a> {
    /// Direction-independent: the same per-channel array for every shadow direction.
    Fixed(&'a [f32; NUM_CHANNELS]),
    /// Re-evaluated at the shadow direction for the path's assigned eigenmode
    /// (`is_extraordinary` as the bounce loop carries it).
    AssignedMode {
        ctx: &'a RayMaterialContext<'a>,
        cache: &'a RayWavelengthCache,
        is_extraordinary: bool,
    },
}

impl ShadowAlphas<'_> {
    /// The per-channel extinction coefficients a shadow ray travelling along `dir` pays.
    pub(super) fn at(self, dir: Vec3) -> [f32; NUM_CHANNELS] {
        match self {
            Self::Fixed(alphas) => *alphas,
            Self::AssignedMode {
                ctx,
                cache,
                is_extraordinary,
            } => channel_absorption_alphas_assigned(ctx, cache, dir, is_extraordinary),
        }
    }
}

/// Next-event estimation at a Henyey-Greenstein scattering event.
///
/// Draws one direction from the environment's own importance distribution
/// ([`sample_environment_for_nee`]), traces a shadow ray from the scattering point to the
/// (necessarily convex) polyhedron's exit facet, applies a scalar Fresnel transmittance
/// there, refracts the sampled direction through that exit facet to look up
/// the environment radiance where the light-sampled ray actually leaves along, applies
/// the medium's own transmittance over the shadow ray's interior path length, and adds
/// the balance-heuristic-weighted contribution directly into `radiance`.
/// A no-op whenever `!nee.enabled`, whenever the environment has no importance
/// distribution to sample (`Studio`), whenever the sampled direction is totally
/// internally reflected at the exit facet (cannot reach the environment at all along that
/// direction -- not a bias: the phase-sampled continuation still has its own chance to
/// escape through a different exit, at full weight, since no NEE sample competes with it
/// there), or whenever the exit facet is [`FacetFinish::Frosted`] (a frosted
/// exit has no well-defined specular Fresnel/refraction for this shadow ray to use --
/// [`nee_contribution_frosted_exterior`](super::nee_contribution_frosted_exterior)
/// already handles NEE for a frosted exit's own diffusely-sampled surface point, so this
/// is a genuine division of labour, not a dropped case).
///
/// Consumes exactly one new 2D RNG draw
/// ([`NEE_ENV_DIR_U_STREAM`]/[`NEE_ENV_DIR_V_STREAM`]) -- a stream pair no existing draw
/// uses, so a trace with NEE disabled never even hashes against it and stays
/// bit-identical to one traced with NEE support absent altogether.
///
/// # Simplification: one achromatic exit transmittance
///
/// Like [`maybe_scatter_or_extinguish`](super::hg::maybe_scatter_or_extinguish) itself
/// (hazard 4: one achromatic direction/distance shared by every channel), this uses ONE
/// scalar Fresnel transmittance -- computed from the HERO channel's own dispersion index
/// (`n_inside_hero`) via the plain isotropic unpolarised Fresnel formula -- shared by
/// every channel's own spectral radiance at the sampled direction, rather than each
/// channel's own (dispersion-shifted) exit index, and rather than the assigned mode's own
/// polarised transmittance the phase-sampled continuation's exit
/// (`refraction::apply_uniaxial_internal_bounce`) applies. Both differences are
/// second-order next to the achromatic phase-function/free-path sampling already in
/// place (the polarised-vs-unpolarised one measured at ~0.04 % of the ruby test's XYZ
/// means); a future revision could add per-channel, per-mode exit transmittances the way
/// `compute_channel_transmission` (`refraction/exit_split.rs`) already does for the
/// polished exit path.
///
/// # Medium transmittance
///
/// The shadow ray pays exactly what a phase-sampled continuation leaving the scattering
/// point in the SAME direction pays on its way to the same boundary: the per-channel
/// `exp(-(alpha_k(w) + sigma_s/path_scale) * d * path_scale)` of
/// [`maybe_scatter_or_extinguish`](super::hg::maybe_scatter_or_extinguish)'s survive
/// branch, with `alpha_k(w)` the assigned mode's coefficients AT the shadow direction
/// (`shadow_alphas.at(sample.dir)`, see [`ShadowAlphas`]) and `d` the distance from the
/// scattering point itself to the exit (`hit.t` plus the probe's [`SHADOW_PROBE_OFFSET`],
/// since the probe starts that far inside the material). `sigma_s` is per model unit and
/// does not scale with the stone's size; the alphas do. Evaluated via the same
/// [`crate::simd::exp_f32x8`] idiom as the survive branch.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors try_scatter_step's own argument shape -- context, this bounce's own \
              geometric/RNG state, and the per-channel accumulators an NEE contribution \
              reads/writes; see that function's identical justification"
)]
pub(crate) fn nee_contribution_hg_scatter(
    nee: NeeContext<'_>,
    lambdas: &[f32; NUM_CHANNELS],
    n_inside_hero: f32,
    scatter_point: Vec3,
    scatter_dir_in: Vec3,
    g: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: &[StokesVector; NUM_CHANNELS],
    radiance: &mut [f32; NUM_CHANNELS],
    shadow_alphas: ShadowAlphas<'_>,
    sigma_s: f32,
    absorption_path_scale: f32,
    facet_finishes: &[FacetFinish],
) {
    // HDR maps only. The analytic sun's light sample is an EXTERIOR direction, but this
    // estimator treats the sample as an INTERIOR direction and refracts it through the exit
    // facet before the radiance lookup; for a disc 0.27 degrees wide that lookup would almost
    // never land on the sun (a biased deposit). A proper sun estimator would have to invert
    // the exit refraction, so a scattering medium under `DaylightSun` gets no NEE here and its
    // phase-sampled continuation keeps full weight (see `phase_pdf_for_mis` below).
    if !nee.enabled || !matches!(nee.environment, EnvironmentSource::HdrMap(_)) {
        return;
    }
    let u0 =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_U_STREAM)) as f32) / 4_294_967_295.0;
    let u1 =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_V_STREAM)) as f32) / 4_294_967_295.0;
    let Some(sample) = sample_environment_for_nee(nee.environment, u0, u1) else {
        return;
    };

    // Shadow ray from the scattering point toward the sampled direction, offset by a
    // small epsilon along it -- mirrors `try_split_exit_channel`'s identical `1e-4`
    // offset (`refraction.rs`) to clear the origin point itself before probing. The
    // offset is added back to every in-material length below (`SHADOW_PROBE_OFFSET`).
    let probe = Ray {
        origin: scatter_point + sample.dir * SHADOW_PROBE_OFFSET,
        dir: sample.dir,
    };
    let Some(first) = intersect_stone_soa(probe, nee.plane_soa, nee.plane_soa.len(), nee.tools)
    else {
        // No exit facet found -- shouldn't happen for a point genuinely inside a closed
        // solid, but a degenerate/open mesh must not panic or fabricate energy.
        return;
    };
    // A convex stone has exactly one crossing: the exit. A concave one may cross out into
    // a cavity and back in before the final exit, so the probe walks its boundaries.
    let Some(exit) = (if nee.tools.is_empty() {
        single_exit(first, sample.dir, n_inside_hero, facet_finishes)
    } else {
        walk_exits(nee, probe, first, n_inside_hero, facet_finishes)
    }) else {
        return;
    };
    let ExitProbe {
        normal: exit_normal,
        t_material: hit_t,
        cos_i,
        cos_t,
        t_unpol,
    } = exit;

    // The competing (phase-function) technique's own density at this SAME
    // light-sampled direction -- the balance heuristic's other half.
    let phase_cos = sample.dir.dot(scatter_dir_in);
    let phase_val = henyey_greenstein_phase(phase_cos, g);
    let mis_weight = balance_heuristic(sample.pdf, phase_val);
    if mis_weight <= 0.0 {
        return;
    }

    // The exterior direction this light sample actually leaves along --
    // Snell's law at the exit facet, `sample.dir` playing the incident-direction role
    // and (the inward-flipped) `-hit.normal` the surface normal, mirroring
    // `refraction.rs`'s own `eta*k_hat + (eta*cos_i - cos_t)*normal` vector form (see
    // this function's doc comment for the sign-convention correspondence). `sample.pdf`
    // itself stays in the INTERIOR (pre-refraction) measure `sample_environment_for_nee`
    // sampled in -- only the radiance LOOKUP moves to the refracted direction.
    let refracted_dir = (n_inside_hero * sample.dir
        - n_inside_hero.mul_add(cos_i, -cos_t) * exit_normal)
        .normalize_or_zero();
    let EnvironmentSource::HdrMap(env_map) = nee.environment else {
        // `sample` is `Some` only for `HdrMap` (see `sample_environment_for_nee`), so
        // this is unreachable in practice -- a defensive `return`, not a real branch.
        return;
    };
    let env_rgb = env_map.radiance_rgb(refracted_dir);

    // The medium transmittance a phase-sampled continuation in this same direction would
    // pay on its way to this same boundary -- see this function's own "Medium
    // transmittance" doc section: the assigned mode's alphas AT the shadow direction, over
    // the full distance from the scattering point. Vectorized Beer-Lambert via the same
    // `exp_f32x8` idiom `maybe_scatter_or_extinguish`'s survive branch uses, so the CPU
    // stays self-consistent between the two estimators.
    let alphas = shadow_alphas.at(sample.dir);
    let hit_t_scaled = hit_t * absorption_path_scale;
    // `sigma_s` is per model unit (size-independent): same conversion as
    // `maybe_scatter_or_extinguish`'s `sigma_s_model`.
    let sigma_s_abs = sigma_s / absorption_path_scale;
    let mut trans_args = [0f32; NUM_CHANNELS];
    for k in 0..NUM_CHANNELS {
        trans_args[k] = -(alphas[k] + sigma_s_abs) * hit_t_scaled;
    }
    let transmittance = crate::simd::exp_f32x8(trans_args);

    let common = t_unpol * phase_val * mis_weight / sample.pdf;
    for k in 0..NUM_CHANNELS {
        let env_k = crate::renderer::env_map::rgb_to_spectral_radiance(env_rgb, lambdas[k]);
        radiance[k] =
            (stokes[k].intensity() * transmittance[k] * common * env_k).mul_add(1.0, radiance[k]);
    }
}

/// The shadow probe's resolved exit: the surface the sample direction finally leaves
/// through, and the chain's total Fresnel transmittance.
struct ExitProbe {
    /// Outward normal of the last exit facet, which refracts the direction the
    /// environment is looked up along.
    normal: Vec3,
    /// Total path length inside the material, measured from the scattering point itself:
    /// the sum of every in-material segment of the walk (just the distance to the exit on
    /// a convex stone), each including the probe's [`SHADOW_PROBE_OFFSET`] its origin sits
    /// inside the material. Air gaps across a cavity carry no absorption.
    t_material: f32,
    /// Cosine of incidence at the last exit.
    cos_i: f32,
    /// Cosine of the refracted angle at the last exit.
    cos_t: f32,
    /// Product of the unpolarised Fresnel transmittance at every exit crossed.
    t_unpol: f32,
}

/// Unpolarised exit Fresnel transmittance and refracted-angle cosine for a ray leaving
/// an interior of index `n_inside` at `cos_i`, or `None` on total internal reflection.
///
/// One scalar hero-index transmittance, as the doc comment of
/// [`nee_contribution_hg_scatter`] describes.
fn exit_transmittance(n_inside: f32, cos_i: f32) -> Option<(f32, f32)> {
    let sin2_t = (n_inside * n_inside * cos_i.mul_add(-cos_i, 1.0)).min(1.0);
    if sin2_t >= 1.0 {
        // Total internal reflection along this direction -- see the NEE doc comment for
        // why this is a legitimate zero, not a bias.
        return None;
    }
    let cos_t = (1.0 - sin2_t).max(0.0).sqrt();
    let r_s = n_inside.mul_add(cos_i, -cos_t) / n_inside.mul_add(cos_i, cos_t);
    let r_p = n_inside.mul_add(-cos_t, cos_i) / n_inside.mul_add(cos_t, cos_i);
    let r_unpol = (0.5 * r_p.mul_add(r_p, r_s * r_s)).clamp(0.0, 1.0);
    Some((1.0 - r_unpol, cos_t))
}

/// Whether the facet `hit` is on has a frosted finish; tool facets are always polished.
fn hit_is_frosted(facet_finishes: &[FacetFinish], hit: HitRecord) -> bool {
    facet_finishes
        .get(hit.facet_idx)
        .copied()
        .unwrap_or_default()
        == FacetFinish::Frosted
}

/// The convex-stone probe result: the single crossing is the exit. This is the
/// pre-concave logic, kept as its own function so a planar trace stays bit-identical.
///
/// A frosted exit facet has no well-defined specular Fresnel/refraction for this shadow
/// ray to use -- see [`nee_contribution_hg_scatter`]'s doc comment for why that is a
/// division of labour with `nee_contribution_frosted_exterior`, not a dropped case.
fn single_exit(
    hit: HitRecord,
    dir: Vec3,
    n_inside: f32,
    facet_finishes: &[FacetFinish],
) -> Option<ExitProbe> {
    if hit_is_frosted(facet_finishes, hit) {
        return None;
    }
    // `hit.normal` is the exit facet's own OUTWARD normal (`intersect_polyhedron`'s
    // convention: an interior origin's far/exit intersection returns the plane's own
    // stored normal, unflipped), and the shadow ray travels outward through it, so
    // `cos_i = dot(dir, hit.normal)` (no negation) is this interface's cosine of
    // incidence -- the same sign convention `compute_channel_transmission`'s own
    // `n1 -> n2` exit formula uses.
    let cos_i = dir.dot(hit.normal).clamp(0.0, 1.0);
    let (t_unpol, cos_t) = exit_transmittance(n_inside, cos_i)?;
    Some(ExitProbe {
        normal: hit.normal,
        // `hit.t` is measured from the probe's origin, which sits `SHADOW_PROBE_OFFSET`
        // inside the material past the scattering point.
        t_material: hit.t + SHADOW_PROBE_OFFSET,
        cos_i,
        cos_t,
        t_unpol,
    })
}

/// The concave-stone probe: follows the sample direction through up to
/// [`HG_NEE_MAX_CROSSINGS`] boundaries, multiplying in the Fresnel transmittance at each
/// exit and at each re-entry through a cavity's far wall (the same unpolarised Fresnel
/// formula, evaluated at the interior-side cosine of the unbent direction, so the two
/// interfaces of one air gap are both paid), and summing the length of every in-material
/// segment for the Beer-Lambert term. Every in-material segment starts
/// [`SHADOW_PROBE_OFFSET`] inside the material (the probe's own origin, or the restart
/// just past a re-entry), so each adds that offset back. Drops the sample
/// above the cap, on TIR or a frosted exit at any crossing, and when the walk ends on an
/// entry (a ray that never leaves a closed stone is degenerate geometry).
///
/// The walk keeps the interior direction through the air gaps rather than refracting it
/// at each interface: v1 bends the direction only once, at the last exit, where the
/// environment is looked up. That matches the single-exit estimator exactly for a stone
/// with no cavity in the way and is an approximation, noted in `physics.md`, otherwise.
fn walk_exits(
    nee: NeeContext<'_>,
    probe: Ray,
    first: HitRecord,
    n_inside: f32,
    facet_finishes: &[FacetFinish],
) -> Option<ExitProbe> {
    let dir = probe.dir;
    let mut ray = probe;
    let mut hit = first;
    let mut crossings = 1;
    let mut chain = 1.0f32;
    let mut material_len = 0.0f32;
    let mut last: Option<(Vec3, f32, f32)>;
    loop {
        if hit_is_frosted(facet_finishes, hit) {
            return None;
        }
        let cos = dir.dot(hit.normal);
        if cos > 0.0 {
            // The segment ending at an exit is in-material, from an origin offset
            // `SHADOW_PROBE_OFFSET` inside it.
            material_len += hit.t + SHADOW_PROBE_OFFSET;
            let cos_i = cos.clamp(0.0, 1.0);
            let (t, cos_t) = exit_transmittance(n_inside, cos_i)?;
            chain *= t;
            last = Some((hit.normal, cos_i, cos_t));
        } else {
            // Re-entry through a far cavity wall: the air-gap segment ending here is
            // not absorbing, but the interface transmits only `T`.
            let (t, _) = exit_transmittance(n_inside, (-cos).clamp(0.0, 1.0))?;
            chain *= t;
            last = None;
        }
        ray = Ray {
            origin: ray.origin + hit.t * dir + dir * SHADOW_PROBE_OFFSET,
            dir,
        };
        let Some(next) = intersect_stone_soa(ray, nee.plane_soa, nee.plane_soa.len(), nee.tools)
        else {
            break;
        };
        crossings += 1;
        if crossings > HG_NEE_MAX_CROSSINGS {
            return None;
        }
        hit = next;
    }
    let (normal, cos_i, cos_t) = last?;
    Some(ExitProbe {
        normal,
        t_material: material_len,
        cos_i,
        cos_t,
        t_unpol: chain,
    })
}

#[cfg(test)]
mod walk_tests {
    use super::*;
    use crate::{
        geometry::{plane::GpuFacetPlane, tool::ToolPrimitive},
        optics::raytracer::{build_plane_soa, environment::LightingPreset},
    };

    /// A shadow ray that exits through a groove wall, crosses the cavity and re-enters
    /// the far wall must pay Beer-Lambert over BOTH in-material segments, measured from
    /// the scattering point itself, and the Fresnel transmittance at all three interfaces
    /// (exit, re-entry, final exit).
    #[test]
    fn through_groove_probe_matches_analytic_segments_and_interfaces() {
        // Unit cube, half-extent 0.5, with a Z-axis through-groove of radius 0.1.
        let planes: Vec<GpuFacetPlane> = [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ]
        .into_iter()
        .map(|n| GpuFacetPlane::new(n, -0.5))
        .collect();
        let soa = build_plane_soa(&planes);
        let tools = [ToolPrimitive::cylinder(Vec3::ZERO, Vec3::Z, 0.1, 2.0)];
        let env = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);
        let nee = NeeContext {
            environment: env,
            plane_soa: &soa,
            tools: &tools,
            enabled: true,
        };
        let n = 1.76f32;
        let start = Vec3::new(-0.4, 0.05, 0.0);
        let probe = Ray {
            origin: start + Vec3::X * SHADOW_PROBE_OFFSET,
            dir: Vec3::X,
        };
        let first = intersect_stone_soa(probe, &soa, planes.len(), &tools).expect("groove wall");
        let exit = walk_exits(nee, probe, first, n, &[]).expect("probe escapes");

        // Walls of the groove at this height: x = -+sqrt(0.1^2 - 0.05^2).
        let half_chord = 0.05f32.mul_add(-0.05, 0.1f32 * 0.1).sqrt();
        let seg_a = (-half_chord) - (-0.4);
        let seg_b = 0.5 - half_chord;
        // Both segments are measured from the scattering point / the re-entry wall, not
        // from the offset probe origins: 1e-4 would be a visible miss at this tolerance.
        assert!(
            (exit.t_material - (seg_a + seg_b)).abs() < 5e-5,
            "material path {} vs {}",
            exit.t_material,
            seg_a + seg_b
        );
        // Interface transmittances from the unpolarised Fresnel formula at each
        // crossing's own cosine (the groove walls are tilted, the cube face is normal).
        let cos_wall = (0.05f32 / 0.1).mul_add(-(0.05f32 / 0.1), 1.0).sqrt();
        let t_wall = exit_transmittance(n, cos_wall).expect("no TIR").0;
        let t_face = exit_transmittance(n, 1.0).expect("no TIR").0;
        let analytic = t_wall * t_wall * t_face;
        assert!(
            (exit.t_unpol - analytic).abs() < 1e-4,
            "chain {} vs {}",
            exit.t_unpol,
            analytic
        );
        // And the absorption the caller multiplies in is strictly larger than the old
        // first-segment-only value.
        assert!(exit.t_material > first.t + 0.4);
    }
}
