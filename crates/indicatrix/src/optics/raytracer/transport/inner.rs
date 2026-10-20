//! The shared bounce loop every public entry point delegates to.
//!
//! [`trace_spectral_ray_inner`], plus its own small per-bounce helpers (primary-hit
//! capture, facet finish lookup, interior-segment absorption, and the per-sample ray
//! context builders).

use super::{PathTermination, path_events::PathEvents};

use super::super::{
    NUM_CHANNELS,
    absorption::apply_segment_absorption,
    camera::{FacetFinish, HitRecord, Ray},
    environment::EnvironmentSource,
    intersect::shading_normal_near_edge,
    refraction::{RayMaterialContext, RayWavelengthCache, build_ray_wavelength_cache},
};
use crate::{
    geometry::{
        plane::GpuFacetPlane,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::{
        fluorescence::Fluorescence,
        materials::{CrystalSystem, GemMaterial},
        polarization::StokesVector,
        studio_rig::StudioRig,
    },
};
use glam::Vec3;

mod bounce_body;
use bounce_body::trace_spectral_ray_core;

/// Records the primary ray's first hit for the denoiser's guide buffers. Only bounce 0
/// writes: at that point `current_ray` is still the camera ray, and the traced path is
/// always driven by the hero channel, so this is unambiguously the hero's own first hit.
fn capture_primary_hit(
    primary_hit_out: &mut Option<&mut Option<HitRecord>>,
    bounce: u32,
    hit: Option<HitRecord>,
) {
    if bounce == 0
        && let Some(slot) = primary_hit_out.as_deref_mut()
    {
        *slot = hit;
    }
}

/// The facet's finish, with `Polished` for any index `facet_finishes` doesn't cover.
fn facet_finish_for(facet_finishes: &[FacetFinish], facet_idx: usize) -> FacetFinish {
    facet_finishes.get(facet_idx).copied().unwrap_or_default()
}

/// Edge-rounded shading normal for a hit on a (possibly concave) stone.
///
/// A convex stone calls [`shading_normal_near_edge`] unchanged, so the planar output is
/// bit-identical. With tools, rounding is skipped (v1) for a hit on a tool surface, and
/// for a plane hit within `rounding_radius` of a tool: `shading_normal_near_edge` blends
/// toward the nearest *plane* normal, which would be the wrong neighbour there. The
/// tool-proximity test probes the four in-plane points one radius away, which is exact
/// for the common case of a tool crossing the facet and cheap (tools are few).
fn shading_normal_for_hit(
    geom: StoneGeometry<'_>,
    hit_point: Vec3,
    hit_facet_idx: usize,
    hit_normal: Vec3,
    rounding_radius: f32,
) -> Vec3 {
    if geom.is_convex() || rounding_radius <= 0.0 {
        return shading_normal_near_edge(
            geom.planes,
            hit_point,
            hit_facet_idx,
            hit_normal,
            rounding_radius,
        );
    }
    if hit_facet_idx >= geom.planes.len() {
        return hit_normal;
    }
    let (u, v) = crate::optics::raytracer::scattering::frosted_orthonormal_basis(hit_normal);
    let near_tool = [u, -u, v, -v].into_iter().any(|dir| {
        let probe = hit_point + dir * rounding_radius;
        geom.tools.iter().any(|tool| tool.contains(probe))
    });
    if near_tool {
        return hit_normal;
    }
    shading_normal_near_edge(
        geom.planes,
        hit_point,
        hit_facet_idx,
        hit_normal,
        rounding_radius,
    )
}

/// `true` when `p` is in the stone's material: inside every plane and outside every tool.
#[cfg(debug_assertions)]
fn point_in_material(geom: StoneGeometry<'_>, p: Vec3) -> bool {
    geom.planes
        .iter()
        .all(|pl| Vec3::from_array(pl.normal).dot(p) + pl.d <= 0.0)
        && !geom.tools.iter().any(|tool| tool.contains(p))
}

/// Debug check of the `inside_gem` toggle against the geometry (concave stones only).
///
/// The toggle is correct because every boundary of the material is a crossing, so the
/// point just before the next hit is in material exactly when `inside_gem`. A point
/// within a few `1e-4` of another surface can classify either way, so the check fires
/// only when two probe points straddling the `1e-4` hit offset agree with each other.
///
/// `inside_gem` is the intersection's own state by the time this runs (the bounce loop
/// adopts the side `intersect_stone_soa_side` reports), so the check compares that state
/// with a classification that shares no code with the intersection. The toggle alone is
/// not enough on a concave stone: a `1e-4` restart can step over a second surface (the
/// rim where a tool meets a facet), and the toggle then names the wrong medium for the
/// whole next stretch.
#[cfg(debug_assertions)]
fn debug_assert_inside_gem_matches_geometry(
    geom: StoneGeometry<'_>,
    ray: Ray,
    t: f32,
    inside_gem: bool,
) {
    if geom.is_convex() || t < 1e-3 {
        return;
    }
    let a = point_in_material(geom, ray.origin + ray.dir * (t - 1e-4));
    let b = point_in_material(geom, ray.origin + ray.dir * (t - 2e-4));
    debug_assert!(
        a != b || a == inside_gem,
        "inside_gem = {inside_gem} disagrees with the point classification {a} at t = {t}"
    );
}

/// Interior-side handling of one facet hit: flips the geometric normal to face the
/// interior ray, then applies the segment's plain Beer-Lambert absorption. A no-op
/// while the ray is outside the gem.
///
/// A scattering-active material's extinction for this segment is already applied by
/// `try_scatter_step`'s no-scatter branch -- calling `apply_absorption` here too would
/// double-charge absorption. `scattering_sigma_s <= 0.0` is the non-scattering case,
/// where this is the only absorption application.
///
/// `ray_ctx` bundles `(&RayMaterialContext, &RayWavelengthCache)` to keep this
/// function's argument count within clippy's `too_many_arguments` limit.
///
/// `k_hat` is the WAVE NORMAL `k` (`current_k` in the caller), not the Poynting
/// direction `S` -- `apply_absorption`'s assigned-mode E-field derives from `k`; the
/// segment's path length stays geometric. See `refraction`'s design note, rule 6.
///
/// `segment` is the interior segment: the ray it travelled (its current origin and direction
/// in model units) and its length `hit_t`. The ray matters only to a zoned material, whose
/// absorption depends on where the segment lies (`absorption::apply_segment_absorption`).
///
/// `is_extraordinary` names which eigenmode this path was assigned to at its most recent
/// air->crystal entry -- see `absorption::channel_absorption_alphas_assigned`'s doc
/// comment.
fn apply_interior_segment(
    ray_ctx: (&RayMaterialContext, &RayWavelengthCache),
    k_hat: Vec3,
    is_extraordinary: bool,
    segment: (Ray, f32),
    inside_gem: bool,
    normal: &mut Vec3,
    stokes: &mut [StokesVector; NUM_CHANNELS],
) {
    let (ctx, cache) = ray_ctx;
    if !inside_gem {
        return;
    }
    *normal = -*normal;
    if ctx.material.scattering_sigma_s <= 0.0 {
        apply_segment_absorption(ctx, cache, k_hat, is_extraordinary, segment, stokes);
    }
}

/// Builds the per-sample [`RayMaterialContext`].
pub(super) fn build_ray_material_context(
    material: &GemMaterial,
    lambdas: [f32; NUM_CHANNELS],
    hero_idx: usize,
    enable_internal_mode_coupling: bool,
) -> RayMaterialContext<'_> {
    // Per-material optical c-axis for anisotropic birefringence.
    let c_axis = material.c_axis;
    let is_anisotropic = material.crystal_system != CrystalSystem::Cubic
        && material.birefringence_delta.abs() > 1e-4;
    RayMaterialContext {
        material,
        lambdas,
        hero_idx,
        c_axis,
        is_anisotropic,
        enable_internal_mode_coupling,
    }
}

/// Bundles [`build_ray_material_context`] and [`build_ray_wavelength_cache`] into one
/// call -- see [`RayWavelengthCache`]'s doc comment for what it caches.
fn build_ray_context(
    material: &GemMaterial,
    lambdas: [f32; NUM_CHANNELS],
    hero_idx: usize,
    enable_internal_mode_coupling: bool,
) -> (RayMaterialContext<'_>, RayWavelengthCache) {
    let mat_ctx =
        build_ray_material_context(material, lambdas, hero_idx, enable_internal_mode_coupling);
    let wavelength_cache = build_ray_wavelength_cache(&mat_ctx);
    (mat_ctx, wavelength_cache)
}

/// Records why/where the bounce loop stopped into the caller's `termination_out` slot;
/// a no-op if it is `None`.
fn record_termination(
    termination_out: &mut Option<&mut (u32, PathTermination)>,
    bounce: u32,
    reason: PathTermination,
) {
    if let Some(out) = termination_out.as_deref_mut() {
        *out = (bounce, reason);
    }
}

/// The shared body of [`trace_spectral_ray`](super::trace_spectral_ray)/
/// [`trace_spectral_ray_with_finish`](super::trace_spectral_ray_with_finish) -- see
/// those functions' doc comments for the parameter list. `enable_internal_mode_coupling
/// = false` reproduces the pre-existing behaviour (mode fixed at entry for the whole
/// interior traversal); only this module's tests use `false`.
///
/// `plane_soa` is the caller-supplied [`crate::simd::PlanesSoA32`] arena
/// `intersect_polyhedron_soa` scans, built once by the caller rather than once per
/// sample. `planes` is still taken separately since `shading_normal_near_edge` needs
/// the per-facet plane records themselves, not just the `SoA` arena.
///
/// `pub(super)`: called by every public entry point in `transport::mod`, and directly by
/// `transport::tests`' own A/B on/off sweeps (`enable_internal_mode_coupling`,
/// `enable_exit_splitting`, `enable_nee`), which the public entry points hardcode to
/// `true` and so cannot express.
#[expect(
    clippy::too_many_arguments,
    reason = "the shared bounce loop every public entry point delegates to -- see \
              trace_spectral_ray's own reason, plus the debug/instrumentation output \
              hooks (primary_hit_out, termination_out) and the caller-supplied \
              plane_soa arena alongside the planes slice it was built from"
)]
pub(super) fn trace_spectral_ray_inner(
    initial_ray: Ray,
    planes: &[GpuFacetPlane],
    plane_soa: &crate::simd::PlanesSoA32,
    facet_finishes: &[FacetFinish],
    material: &GemMaterial,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    rng_seed: u32,
    hero_rand: f32,
    primary_hit_out: Option<&mut Option<HitRecord>>,
    enable_internal_mode_coupling: bool,
    enable_exit_splitting: bool,
    // `true` only at every public entry point when `environment` is `HdrMap`
    // (the procedural studio rig has no importance distribution to draw NEE samples
    // from, and its own sampling already matches its structure -- see
    // `NeeContext::enabled`'s doc comment), or when this file's own tests force it
    // explicitly for an on/off A-B comparison, mirroring `enable_exit_splitting`'s
    // identical precedent.
    enable_nee: bool,
    termination_out: Option<&mut (u32, PathTermination)>,
) -> Vec3 {
    trace_spectral_ray_inner_geom(
        initial_ray,
        planes,
        &[],
        plane_soa,
        facet_finishes,
        material,
        Fluorescence::none(),
        max_bounces,
        environment,
        rng_seed,
        hero_rand,
        primary_hit_out,
        enable_internal_mode_coupling,
        enable_exit_splitting,
        enable_nee,
        termination_out,
    )
}

/// [`trace_spectral_ray_inner`] for a stone with tools: `tools` are subtracted from the
/// polyhedron `planes` define, and `plane_soa` is still the arena built from `planes`
/// alone. With `tools` empty this is the old function, bit for bit.
#[expect(
    clippy::too_many_arguments,
    reason = "the shared bounce loop every public entry point delegates to -- see \
              trace_spectral_ray's own reason, plus the debug/instrumentation output \
              hooks (primary_hit_out, termination_out) and the caller-supplied \
              plane_soa arena alongside the planes slice it was built from"
)]
pub(super) fn trace_spectral_ray_inner_geom(
    initial_ray: Ray,
    planes: &[GpuFacetPlane],
    tools: &[ToolPrimitive],
    plane_soa: &crate::simd::PlanesSoA32,
    facet_finishes: &[FacetFinish],
    material: &GemMaterial,
    fluorescence: &Fluorescence,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    rng_seed: u32,
    hero_rand: f32,
    primary_hit_out: Option<&mut Option<HitRecord>>,
    enable_internal_mode_coupling: bool,
    enable_exit_splitting: bool,
    // `true` only at every public entry point when `environment` is `HdrMap`
    // (the procedural studio rig has no importance distribution to draw NEE samples
    // from, and its own sampling already matches its structure -- see
    // `NeeContext::enabled`'s doc comment), or when this file's own tests force it
    // explicitly for an on/off A-B comparison, mirroring `enable_exit_splitting`'s
    // identical precedent.
    enable_nee: bool,
    termination_out: Option<&mut (u32, PathTermination)>,
) -> Vec3 {
    trace_spectral_ray_inner_geom_events(
        initial_ray,
        planes,
        tools,
        plane_soa,
        facet_finishes,
        material,
        fluorescence,
        max_bounces,
        environment,
        rng_seed,
        hero_rand,
        primary_hit_out,
        enable_internal_mode_coupling,
        enable_exit_splitting,
        enable_nee,
        (termination_out, None),
    )
}

/// [`trace_spectral_ray_inner_geom`] with an optional [`PathEvents`] recorder beside the
/// termination slot (`outputs.1`). With `None` for the recorder it is the function above, bit
/// for bit; the recorder is only ever written by the bounce loop's hooks, never read.
#[expect(
    clippy::too_many_arguments,
    reason = "see trace_spectral_ray_inner_geom's own reason, plus the attribution recorder"
)]
pub(super) fn trace_spectral_ray_inner_geom_events(
    initial_ray: Ray,
    planes: &[GpuFacetPlane],
    tools: &[ToolPrimitive],
    plane_soa: &crate::simd::PlanesSoA32,
    facet_finishes: &[FacetFinish],
    material: &GemMaterial,
    fluorescence: &Fluorescence,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    rng_seed: u32,
    hero_rand: f32,
    primary_hit_out: Option<&mut Option<HitRecord>>,
    enable_internal_mode_coupling: bool,
    enable_exit_splitting: bool,
    enable_nee: bool,
    outputs: (Option<&mut (u32, PathTermination)>, Option<&mut PathEvents>),
) -> Vec3 {
    let (termination_out, events) = outputs;
    let scene = TraceScene {
        planes,
        tools,
        plane_soa,
        facet_finishes,
        material,
        fluorescence,
        max_bounces,
        environment,
        surface_glare: environment.surface_glare(),
    };
    let switches = TraceSwitches {
        internal_mode_coupling: enable_internal_mode_coupling,
        exit_splitting: enable_exit_splitting,
        nee: enable_nee,
    };
    trace_spectral_ray_core(
        &scene,
        initial_ray,
        rng_seed,
        hero_rand,
        switches,
        (primary_hit_out, termination_out, events),
    )
}

/// The per-trace inputs of one traced ray, bundled to keep the loop's argument count
/// down. `surface_glare` is read from the environment (see
/// `EnvironmentSource::surface_glare`), so it rides every existing entry point.
#[derive(Clone, Copy)]
struct TraceScene<'a> {
    planes: &'a [GpuFacetPlane],
    /// Convex volumes subtracted from the polyhedron; empty for a planar stone.
    tools: &'a [ToolPrimitive],
    plane_soa: &'a crate::simd::PlanesSoA32,
    facet_finishes: &'a [FacetFinish],
    material: &'a GemMaterial,
    /// Fluorescent emitters beside the material; empty (the usual case) leaves the
    /// whole trace exactly as it was without the field.
    fluorescence: &'a Fluorescence,
    max_bounces: u32,
    environment: EnvironmentSource<'a>,
    surface_glare: f32,
}

thread_local! {
    /// The last [`StudioRig`] this thread built, keyed by the exact bits of its
    /// `(light_yaw, light_pitch)`. A render batch traces thousands of samples under one light
    /// pose, so the rig (~38 transcendentals and 18 normalizes, about 1-2 us) is built once
    /// per thread and copied (a plain `memcpy` of ~250 bytes) per sample.
    static STUDIO_RIG_CACHE: std::cell::Cell<Option<(u32, u32, StudioRig)>> =
        const { std::cell::Cell::new(None) };
}

/// The [`StudioRig`] for a light pose, from this thread's one-entry cache.
///
/// Value-identical to `StudioRig::new(light_yaw, light_pitch)`: the cache returns a copy of
/// that function's own output for the same input bits, so nothing downstream can tell the
/// difference. NaN poses key on their bits too and rebuild to the same NaN rig.
fn cached_studio_rig(light_yaw: f32, light_pitch: f32) -> StudioRig {
    let key = (light_yaw.to_bits(), light_pitch.to_bits());
    if let Some((yaw_bits, pitch_bits, rig)) = STUDIO_RIG_CACHE.get()
        && (yaw_bits, pitch_bits) == key
    {
        return rig;
    }
    let rig = StudioRig::new(light_yaw, light_pitch);
    STUDIO_RIG_CACHE.set(Some((key.0, key.1, rig)));
    rig
}

/// The A/B switches of the bounce loop. The public entry points hardcode every one
/// (`nee` follows the environment kind); only this module's tests vary them.
#[derive(Clone, Copy)]
struct TraceSwitches {
    internal_mode_coupling: bool,
    exit_splitting: bool,
    nee: bool,
}
