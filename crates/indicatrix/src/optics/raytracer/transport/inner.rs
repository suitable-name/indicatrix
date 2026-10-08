//! The shared bounce loop every public entry point delegates to.
//!
//! [`trace_spectral_ray_inner`], plus its own small per-bounce helpers (primary-hit
//! capture, facet finish lookup, interior-segment absorption, and the per-sample ray
//! context builders).

use super::{
    PathTermination,
    bounce::{accumulate_miss_radiance, apply_russian_roulette, dispatch_bounce},
    wrapped_hero_wavelengths,
};

use super::super::{
    NUM_CHANNELS,
    absorption::{apply_absorption, rotate_stokes_to_plane_of_incidence},
    camera::{FacetFinish, HitRecord, Ray},
    color::{
        apply_von_kries_white_balance, integrate_channels_to_xyz,
        integrate_channels_to_xyz_families,
    },
    environment::{
        EnvironmentSource, environment_nee_pdf, environment_white_balance, fill_backdrop,
    },
    intersect::shading_normal_near_edge,
    intersect_stone::intersect_stone_soa,
    refraction::{
        ExitSplitCtx, RayMaterialContext, RayWavelengthCache, build_ray_wavelength_cache,
        compute_bounce_refraction_geometry,
    },
    sampling::{
        FLUORESCENCE_DIR_U_STREAM, FLUORESCENCE_DIR_V_STREAM, FLUORESCENCE_DISTANCE_STREAM,
        FLUORESCENCE_EMITTER_STREAM, FLUORESCENCE_EXCITATION_STREAM, hash_u32,
    },
    scattering::{NeeContext, ScatterStepOutcome, balance_heuristic, try_scatter_step},
};
use crate::{
    geometry::{
        plane::GpuFacetPlane,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::{
        fluorescence::{Fluorescence, sample_vertex},
        materials::{CrystalSystem, GemMaterial},
        polarization::StokesVector,
    },
};
use glam::Vec3;

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
/// direction `S` -- `apply_absorption`'s assigned-mode E-field derives from `k`; `hit_t`
/// (path length) stays geometric. See `refraction`'s design note, rule 6.
///
/// `is_extraordinary` names which eigenmode this path was assigned to at its most recent
/// air->crystal entry -- see `absorption::channel_absorption_alphas_assigned`'s doc
/// comment.
fn apply_interior_segment(
    ray_ctx: (&RayMaterialContext, &RayWavelengthCache),
    k_hat: Vec3,
    is_extraordinary: bool,
    hit_t: f32,
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
        apply_absorption(ctx, cache, k_hat, is_extraordinary, hit_t, stokes);
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
        (primary_hit_out, termination_out),
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

/// The A/B switches of the bounce loop. The public entry points hardcode every one
/// (`nee` follows the environment kind); only this module's tests vary them.
#[derive(Clone, Copy)]
struct TraceSwitches {
    internal_mode_coupling: bool,
    exit_splitting: bool,
    nee: bool,
}

/// Upper bound on the expected number of fluorescence vertices the free-path sampler
/// of one segment aims for (`mu_s * segment <= CAP`): the sampling rate is
/// `min(mu_f, CAP / segment)`, and the estimator weights absorb the difference
/// (`mu_f / mu_s` at a vertex, `exp(mu_s * ell)` for a path that goes on). The plain
/// exponential at `mu_s = mu_f` is unbiased too, but a strongly emitting line
/// (`mu_f * segment` in the tens) would give the surviving paths weights of
/// `exp(mu_f * segment)`, an unusable variance; capped at 2 the largest such weight is 7.4.
const FLUORESCENCE_SAMPLING_CAP: f32 = 2.0;

/// A `[0, 1]` draw from `rng_seed`'s stream `stream` at `bounce`, in the same form every
/// other per-bounce draw of this module tree uses.
fn unit_draw(rng_seed: u32, bounce: u32, stream: u32) -> f32 {
    (hash_u32(rng_seed ^ hash_u32(bounce ^ stream)) as f32) / 4_294_967_295.0
}

/// Sets every channel's Stokes vector to unpolarized light of `weight` times its
/// current intensity: the fluorescence emission is isotropic and unpolarized.
fn depolarize_and_scale(stokes: &mut [StokesVector; NUM_CHANNELS], weight: f32) {
    for s in &mut *stokes {
        *s = StokesVector::unpolarized(s.intensity() * weight);
    }
}

/// Scales every channel's Stokes vector (polarization state kept).
fn scale_stokes(stokes: &mut [StokesVector; NUM_CHANNELS], weight: f32) {
    for s in &mut *stokes {
        *s = s.scale(weight);
    }
}

/// Applies the surface-glare scale to every channel's Stokes vector. Called only for a
/// camera path whose first event is the polished specular reflection off the stone's
/// surface (it never entered), and only when the scale is below one.
fn apply_surface_glare(stokes: &mut [StokesVector; NUM_CHANNELS], surface_glare: f32) {
    for s in &mut *stokes {
        *s = s.scale(surface_glare);
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "already at the pedantic line-length threshold; the bounce_cost harness's \
              termination bookkeeping pushes it a few lines over -- see \
              record_termination's doc comment for why that extraction, not further \
              splitting this already heavily-decomposed loop, is the right amount of \
              surgery"
)]
fn trace_spectral_ray_core(
    scene: &TraceScene<'_>,
    initial_ray: Ray,
    rng_seed: u32,
    hero_rand: f32,
    switches: TraceSwitches,
    outputs: (
        Option<&mut Option<HitRecord>>,
        Option<&mut (u32, PathTermination)>,
    ),
) -> Vec3 {
    let TraceScene {
        planes,
        tools,
        plane_soa,
        facet_finishes,
        material,
        fluorescence,
        max_bounces,
        environment,
        surface_glare,
    } = *scene;
    let TraceSwitches {
        internal_mode_coupling: enable_internal_mode_coupling,
        exit_splitting: enable_exit_splitting,
        nee: enable_nee,
    } = switches;
    let (mut primary_hit_out, mut termination_out) = outputs;
    // Clamped at the boundary; a NaN fails the `< 1.0` test below and so behaves as off.
    let surface_glare = surface_glare.clamp(0.0, 1.0);
    // Hero is drawn over the full visible range [380, 780) with wraparound, so a hero
    // draw `h` and `h + channel_width` generate the same 8-member comb, cyclically
    // rotated -- each member is equally likely to be drawn as hero, which is the
    // premise spectral MIS (`spectral_mis_weight` below) requires: every wavelength
    // must be reachable as the hero channel at positive, uniform probability. See
    // `wrapped_hero_wavelengths`'s doc comment for the formula.
    //
    // A scene with fluorescence traces SINGLE-wavelength paths instead: all channels carry
    // the one camera wavelength `lambda_em` (drawn over [380, 780) from `hero_rand`, see
    // `Fluorescence::camera_wavelength`), so the in-medium fluorescence vertex can switch the whole path to the
    // excitation wavelength and the image still converges unbiased. Exit-event
    // splitting is off then (its companions would all be the same wavelength). Without
    // fluorescence none of this runs and `lambdas` is the comb.
    let fluorescent = !fluorescence.is_empty();
    let enable_exit_splitting = enable_exit_splitting && !fluorescent;
    // `camera_weight` corrects the importance-sampled camera wavelength (see
    // `Fluorescence::camera_wavelength`); 1 and unused without fluorescence.
    let (camera_wavelength, camera_weight) = if fluorescent {
        fluorescence.camera_wavelength(hero_rand)
    } else {
        (0.0, 1.0)
    };
    let mut lambdas: [f32; NUM_CHANNELS] = if fluorescent {
        [camera_wavelength; NUM_CHANNELS]
    } else {
        wrapped_hero_wavelengths(hero_rand)
    };
    // The wavelengths the camera integrates the result at; `lambdas` itself moves to the
    // excitation wavelength at a fluorescence vertex.
    let camera_lambdas = lambdas;
    // Fluorescence pseudo-extinction at the camera wavelength (1/absorption-length) and
    // whether this path may still take its (single) vertex.
    // Emitters are PerMm-only: the pseudo-extinction below is in 1/mm and the vertex sampling
    // scales the segment by `absorption_path_scale`. A ModelUnit material's scale is
    // `(W / 7) / MODEL_UNIT_FACE_UP_PATH`, which would make its emitters fluoresce about 2.5x
    // (1 / K) too weakly. Today only PerMm physics recipes carry emitters.
    debug_assert!(
        !fluorescent
            || material.absorption_unit == crate::optics::materials::AbsorptionUnit::PerMm
            || material.absorption_path_scale.to_bits() == 1.0_f32.to_bits(),
        "fluorescent emitters need a PerMm material (or an unscaled bare one)"
    );
    let fluorescence_rate = if fluorescent {
        fluorescence.pseudo_extinction(camera_lambdas[0])
    } else {
        0.0
    };
    let mut fluorescence_armed = fluorescence_rate > 0.0;

    let mut stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
    let mut radiance = [0.0f32; NUM_CHANNELS];
    // A Henyey-Greenstein scattering-point NEE deposit is integrated to XYZ
    // immediately, using THAT moment's own `path_pdf`/`compat` -- see
    // `try_scatter_step`'s "NEE spectral weighting" doc comment for why this
    // must not ride the shared `radiance` array (which is later weighted by the
    // FINAL, post-loop `path_pdf`/`compat` instead). Summed unconditionally into the
    // final result below, regardless of how the path eventually terminates -- the same
    // unconditional inclusion the old shared-`radiance` deposit always had.
    let mut nee_xyz = Vec3::ZERO;

    // `hero_idx` names which slot of `lambdas` (and every other per-channel array
    // below) holds the wavelength driving the shared geometric path. Provably 0 for
    // every invocation under this construction (`lambdas[0] == lambda_hero`
    // identically), but threaded explicitly so the code documents which channel plays
    // the hero role.
    let hero_idx: usize = 0;

    // Per-channel running density of "technique k (channel k as hero) would have
    // generated this exact realized path" -- see the TIR/reflect/refract branches
    // below and `spectral_mis_weight`'s doc comment. Starts at 1.0 for every channel.
    let mut path_pdf = [1.0f32; NUM_CHANNELS];

    let mut current_ray = initial_ray;
    // Unit direction from the stone back towards the eye: the lit lighting models darken
    // exit directions inside the head-shadow cone around it (see
    // `environment::sample_studio_environment_observed`). Per pixel rather than the
    // camera axis, so the cone is centred exactly where this pixel's eye sits.
    let observer = -initial_ray.dir;
    // The wave normal `k`, tracked alongside `current_ray.dir` (the Poynting/energy
    // direction `S`) -- see `refraction`'s "wave normal vs Poynting direction" note.
    // Starts equal to the initial ray direction (k == S in isotropic air).
    let mut current_k = initial_ray.dir;
    let mut inside_gem = false;
    // `None` until the first plane of incidence is recorded, so no spurious frame
    // rotation applies at the very first surface.
    let mut prev_plane_normal: Option<Vec3> = None;
    // Which eigenmode the ray inside the crystal was stochastically assigned at its
    // most recent air->crystal entry. Meaningless while `!inside_gem`; carried across
    // internal bounces so a path keeps using its entry index until it exits.
    //
    // For a UNIAXIAL material this is the ordinary/extraordinary split. For a BIAXIAL
    // material (Alexandrite, Topaz, Tanzanite) there is no "ordinary" ray -- both
    // eigenmodes are direction-dependent and both walk off -- so this flag is a plain
    // two-valued mode selector: `false` selects mode A (faster, lower-index root of
    // `BiaxialIndicatrix::wave_indices`), `true` selects mode B (slower, higher-index).
    let mut is_extraordinary = false;

    // Fixed across every bounce below -- see `RayMaterialContext`/`RayWavelengthCache`.
    let (mut mat_ctx, mut wavelength_cache) =
        build_ray_context(material, lambdas, hero_idx, enable_internal_mode_coupling);

    // Built once per trace and shared: the exit-split probes read it through
    // `exit_split_ctx.studio_rig`, and the escape lookup in `accumulate_miss_radiance`
    // borrows that same instance. It depends only on `(light_yaw, light_pitch)`, so the
    // single build is value-identical to rebuilding it at each use.
    let exit_split_studio_rig = match environment {
        EnvironmentSource::Studio {
            light_yaw,
            light_pitch,
            ..
        } => Some(crate::optics::studio_rig::StudioRig::new(
            light_yaw,
            light_pitch,
        )),
        EnvironmentSource::HdrMap(_) => None,
    };
    // Exit-event splitting's own staging accumulator -- see
    // `ExitSplitCtx::split_radiance`'s doc comment for why a split channel's
    // contribution lands here first, folded into `radiance` only if the shared/hero
    // path terminates via `PathTermination::Escaped`.
    let mut split_radiance = [0.0f32; NUM_CHANNELS];
    // Per-trace context for exit-event spectral splitting -- see `refraction`'s
    // "Exit-event spectral splitting" doc comment.
    let mut exit_split_ctx = ExitSplitCtx {
        plane_soa,
        tools,
        environment,
        studio_rig: exit_split_studio_rig,
        observer,
        split_radiance: &mut split_radiance,
        enabled: enable_exit_splitting,
        compat: [u8::MAX; NUM_CHANNELS],
        split_mis_weight: 1.0,
    };
    // Set alongside `record_termination(.., PathTermination::Escaped)` below -- the
    // only point `radiance` is ever populated from a real environment sample. See
    // `ExitSplitCtx::split_radiance`'s doc comment for why `split_radiance` must only
    // be committed when this ends up `true`.
    let mut path_escaped = false;

    // Next-event estimation's per-trace context (fixed for the whole trace,
    // like `exit_split_ctx` above) plus the one piece of state that carries from a
    // scatter event to its VERY NEXT loop iteration -- see `ScatterStepOutcome::
    // ScatteredAndSurvived`'s doc comment for exactly what this represents and why it is
    // consumed (via `.take()`) every single iteration regardless of outcome.
    let nee_ctx = NeeContext {
        environment,
        plane_soa,
        tools,
        enabled: enable_nee,
    };
    let mut pending_light_mis: Option<(f32, Vec3)> = None;

    for bounce in 0..max_bounces {
        // Consumed unconditionally every iteration: meaningful only if THIS iteration's
        // ray turns out to have escaped directly (see below); otherwise silently
        // dropped, which is correct -- a phase-sampled continuation that instead hits
        // more geometry has no competing NEE sample to weigh itself against.
        let phase_pdf_for_mis_this_check = pending_light_mis.take();
        // With no tools this is `intersect_polyhedron_soa` exactly. On a concave stone a
        // hit after an exit is a legitimate re-entry across the cavity, not an error.
        let hit = intersect_stone_soa(current_ray, plane_soa, planes.len(), tools);

        // Denoiser wiring: the primary ray's first-hit depth/normal/facet index feed
        // the A-Trous denoiser's guide buffers (see `renderer::denoise`). Captured
        // only at bounce 0, which is unambiguously the hero channel's own first hit.
        capture_primary_hit(&mut primary_hit_out, bounce, hit);

        let Some(hit_rec) = hit else {
            // The camera ray sees the backdrop card, if the scene has one -- only the
            // stone's own light reaches the environment lookup below.
            if bounce == 0 && fill_backdrop(environment, &lambdas, &mut radiance) {
                path_escaped = true;
                record_termination(&mut termination_out, bounce, PathTermination::Escaped);
                break;
            }
            // Ray exited or missed the gemstone -> sample the environment source.
            // If an earlier bounce left a pending NEE-eligible carry still live (a
            // scatter event whose continuation just now escaped directly, possibly
            // after first transmitting out through a polished exit facet -- see
            // `dispatch_bounce`'s doc comment for why the carry must survive that
            // intervening refraction), this escape is the SAME light-sampling
            // technique's competing (BSDF/phase-sampled) continuation -- weight it by
            // the balance heuristic so the two techniques' contributions sum to the
            // true value rather than double-counting. Evaluated at the carried INTERIOR
            // direction, the same measure `nee_contribution_hg_scatter`
            // sampled in -- NOT `current_ray.dir`, which by the time a transmit-out
            // carry reaches here is the refracted EXTERIOR direction instead.
            // `phase_pdf_for_mis_this_check.map_or(1.0, ..)` reproduces the full-weight
            // behaviour exactly whenever no carry is live (including every trace with
            // NEE disabled, where this is always `None`).
            let mis_weight =
                phase_pdf_for_mis_this_check.map_or(1.0, |(phase_pdf, interior_dir)| {
                    let light_pdf = environment_nee_pdf(environment, interior_dir);
                    balance_heuristic(phase_pdf, light_pdf)
                });
            accumulate_miss_radiance(
                &exit_split_ctx,
                current_ray.dir,
                &lambdas,
                &stokes,
                mis_weight,
                &mut radiance,
            );
            path_escaped = true;
            record_termination(&mut termination_out, bounce, PathTermination::Escaped);
            break;
        };
        #[cfg(debug_assertions)]
        debug_assert_inside_gem_matches_geometry(
            StoneGeometry { planes, tools },
            current_ray,
            hit_rec.t,
            inside_gem,
        );
        // Fluorescence: sample a vertex along the interior segment, competing with the
        // scattering sample and the boundary below (see `FLUORESCENCE_SAMPLING_CAP`).
        // `segment_t` is where the scatter sampler must stop: the vertex if one fires
        // first, else the boundary. Without fluorescence `segment_t == hit_rec.t` and
        // nothing here draws.
        let mut segment_t = hit_rec.t;
        let mut vertex_t = None;
        let mut sampling_rate = 0.0f32;
        let path_scale = material.absorption_path_scale;
        if fluorescence_armed && inside_gem && hit_rec.t > 0.0 {
            let segment_scaled = hit_rec.t * path_scale;
            sampling_rate = fluorescence_rate.min(FLUORESCENCE_SAMPLING_CAP / segment_scaled);
            let u = unit_draw(rng_seed, bounce, FLUORESCENCE_DISTANCE_STREAM);
            let t_scaled = -((1.0 - u).max(1e-7).ln()) / sampling_rate;
            if t_scaled < segment_scaled {
                segment_t = t_scaled / path_scale;
                vertex_t = Some(t_scaled);
            }
        }
        let origin_before = current_ray.origin;
        // Attempt a Henyey-Greenstein scattering event along this segment before
        // processing the facet -- see `try_scatter_step`'s doc comment.
        if inside_gem {
            match try_scatter_step(
                &mat_ctx,
                &wavelength_cache,
                material,
                is_extraordinary,
                &mut current_ray,
                &mut current_k,
                segment_t,
                rng_seed,
                bounce,
                &mut stokes,
                &mut path_pdf,
                exit_split_ctx.split_radiance,
                nee_ctx,
                &lambdas,
                &mut nee_xyz,
                enable_exit_splitting,
                exit_split_ctx.compat,
                facet_finishes,
            ) {
                ScatterStepOutcome::NotApplicable | ScatterStepOutcome::ReachedBoundary => {
                    if let Some(t_scaled) = vertex_t {
                        // The fluorescence vertex fired before the boundary: the
                        // medium's extinction up to it is already applied for a
                        // scattering material (`try_scatter_step`), else apply the
                        // Beer-Lambert absorption of the stretch to the vertex here.
                        if material.scattering_sigma_s <= 0.0 {
                            apply_absorption(
                                &mat_ctx,
                                &wavelength_cache,
                                current_k,
                                is_extraordinary,
                                segment_t,
                                &mut stokes,
                            );
                        }
                        let Some(vertex) = sample_vertex(
                            fluorescence,
                            camera_lambdas[0],
                            unit_draw(rng_seed, bounce, FLUORESCENCE_EMITTER_STREAM),
                            unit_draw(rng_seed, bounce, FLUORESCENCE_EXCITATION_STREAM),
                        ) else {
                            record_termination(
                                &mut termination_out,
                                bounce,
                                PathTermination::RussianRoulette,
                            );
                            break;
                        };
                        // Estimator weight: the rate ratio of the capped sampler, the
                        // reweighting of its truncated exponential (`exp(mu_s * t)`),
                        // and the photon-energy ratio of the Stokes shift.
                        let weight = (fluorescence_rate / sampling_rate)
                            * (sampling_rate * t_scaled).exp()
                            * vertex.energy_ratio;
                        depolarize_and_scale(&mut stokes, weight);
                        // Isotropic emission from the vertex, continuing at the
                        // excitation wavelength, with no further vertex.
                        let z = (-2.0f32)
                            .mul_add(unit_draw(rng_seed, bounce, FLUORESCENCE_DIR_U_STREAM), 1.0);
                        let phi = std::f32::consts::TAU
                            * unit_draw(rng_seed, bounce, FLUORESCENCE_DIR_V_STREAM);
                        #[expect(
                            clippy::suboptimal_flops,
                            reason = "transport results are pinned bit for bit (goldens, twins); a fused `1 - z*z` rounds once and moves the last bit"
                        )]
                        let r = (1.0 - z * z).max(0.0).sqrt();
                        let new_dir = Vec3::new(r * phi.cos(), r * phi.sin(), z);
                        current_ray.origin += segment_t * current_ray.dir;
                        current_ray.dir = new_dir;
                        current_k = new_dir;
                        prev_plane_normal = None;
                        lambdas = [vertex.lambda_ex; NUM_CHANNELS];
                        (mat_ctx, wavelength_cache) = build_ray_context(
                            material,
                            lambdas,
                            hero_idx,
                            enable_internal_mode_coupling,
                        );
                        fluorescence_armed = false;
                        continue;
                    }
                    // No vertex before the boundary: the path goes on, reweighted by
                    // the inverse of that probability (1 / exp(-mu_s * ell)).
                    if sampling_rate > 0.0 {
                        let g = (sampling_rate * hit_rec.t * path_scale).exp();
                        scale_stokes(&mut stokes, g);
                    }
                }
                ScatterStepOutcome::ScatteredAndSurvived(phase_pdf_for_mis) => {
                    if sampling_rate > 0.0 {
                        // Scattered at distance `s` with the vertex clock not having
                        // fired: weight `exp(mu_s * s)`.
                        let s = (current_ray.origin - origin_before).length() * path_scale;
                        scale_stokes(&mut stokes, (sampling_rate * s).exp());
                    }
                    // Scattered Stokes vectors are already depolarized, so the previous
                    // plane of incidence is no longer meaningful -- reset it so the
                    // next facet hit applies no spurious frame rotation.
                    prev_plane_normal = None;
                    // Consumed by the VERY NEXT iteration's escape check
                    // above, and only there -- see that check's own doc comment.
                    pending_light_mis = phase_pdf_for_mis;
                    continue;
                }
                ScatterStepOutcome::ScatteredAndTerminated => {
                    record_termination(
                        &mut termination_out,
                        bounce,
                        PathTermination::ScatterAbsorbed,
                    );
                    break;
                }
            }
        }

        let hit_point = current_ray.origin + hit_rec.t * current_ray.dir;
        // Facet edge rounding: see `shading_normal_near_edge`'s doc comment.
        let mut normal = shading_normal_for_hit(
            StoneGeometry { planes, tools },
            hit_point,
            hit_rec.facet_idx,
            hit_rec.normal,
            material.edge_rounding_radius,
        );

        // See `rotate_stokes_to_plane_of_incidence`'s doc comment. The Stokes
        // plane-of-incidence frame is defined by the wave normal `k`, not `S`.
        let current_plane_normal =
            rotate_stokes_to_plane_of_incidence(current_k, normal, prev_plane_normal, &mut stokes);
        prev_plane_normal = Some(current_plane_normal);

        // See `apply_interior_segment`'s doc comment. Assigned-mode absorption's
        // E-field direction uses `k`; `hit_rec.t` stays geometric/`S`-based.
        apply_interior_segment(
            (&mat_ctx, &wavelength_cache),
            current_k,
            is_extraordinary,
            hit_rec.t,
            inside_gem,
            &mut normal,
            &mut stokes,
        );

        // See `compute_bounce_refraction_geometry`'s doc comment. Index lookups,
        // angles, and every downstream Snell/Fresnel evaluation use the wave normal
        // `k`, not `S`.
        let geo = compute_bounce_refraction_geometry(
            &mat_ctx,
            &wavelength_cache,
            normal,
            current_k,
            inside_gem,
            is_extraordinary,
        );

        // Girdle finish: `Polished` (default) takes the pre-existing dispatch;
        // `Frosted` takes `apply_frosted_bounce` instead.
        let finish = facet_finish_for(facet_finishes, hit_rec.facet_idx);
        // `dispatch_bounce`'s return is `Some` for an NEE-eligible frosted-facet
        // outcome (see `apply_frosted_bounce`'s doc comment), OR for a
        // polished transmit-out event that had a live incoming carry
        // (`phase_pdf_for_mis_this_check`, this same iteration's `.take()` result from
        // above) to pass through -- `None` for every other dispatch.
        pending_light_mis = dispatch_bounce(
            &mat_ctx,
            &wavelength_cache,
            &geo,
            hit_point,
            normal,
            current_plane_normal,
            finish,
            rng_seed,
            bounce,
            &mut stokes,
            &mut path_pdf,
            &mut exit_split_ctx,
            &mut current_ray,
            &mut current_k,
            &mut inside_gem,
            &mut is_extraordinary,
            nee_ctx,
            &lambdas,
            &mut radiance,
            phase_pdf_for_mis_this_check,
        );

        // Surface glare: the camera path's first event at a polished facet left the
        // ray outside the stone, so it was the specular (Fresnel) reflection and the
        // light never entered. Scaling the throughput here scales everything this path
        // later deposits. Skipped entirely at `1.0` (and for a frosted facet, whose
        // diffuse scatter is not a mirror image), so that case is bit-identical.
        if bounce == 0 && !inside_gem && surface_glare < 1.0 && finish != FacetFinish::Frosted {
            apply_surface_glare(&mut stokes, surface_glare);
        }

        // See `apply_russian_roulette`'s doc comment. Reborrowed through
        // `exit_split_ctx.split_radiance` since `exit_split_ctx` already holds that
        // local borrowed mutably for the whole loop.
        if bounce > 4
            && !apply_russian_roulette(bounce, rng_seed, &mut stokes, exit_split_ctx.split_radiance)
        {
            record_termination(
                &mut termination_out,
                bounce,
                PathTermination::RussianRoulette,
            );
            break;
        }
    }

    // Each channel's MIS family, final after the last interior dispersive event.
    let compat = exit_split_ctx.compat;

    // Commit staged exit-split contributions into `radiance` only if the shared/hero
    // path itself reached its own environment lookup -- see
    // `ExitSplitCtx::split_radiance`'s doc comment for why this all-or-nothing gate is
    // required. Every channel is then integrated under the one shared
    // `spectral_mis_weight`.
    if path_escaped {
        for k in 0..NUM_CHANNELS {
            radiance[k] += split_radiance[k];
        }
    }

    // See `integrate_channels_to_xyz`'s doc comment. With splitting enabled every
    // channel is weighted over its own family instead -- see
    // `integrate_channels_to_xyz_families`'s doc comment.
    let xyz = if enable_exit_splitting {
        integrate_channels_to_xyz_families(&radiance, &camera_lambdas, &path_pdf, hero_idx, compat)
    } else {
        integrate_channels_to_xyz(&radiance, &camera_lambdas, &path_pdf, hero_idx)
    };
    // `nee_xyz` (see its own doc comment above) is already fully integrated to XYZ --
    // added in directly, not run back through `integrate_channels_to_xyz[_families]`
    // a second time. `HdrMap`-only in practice (`nee.enabled` is `false` for `Studio`
    // at every public entry point), so this is a no-op for the white-balanced branch
    // below.
    let xyz = xyz + nee_xyz;
    let xyz = if fluorescent {
        xyz * camera_weight
    } else {
        xyz
    };

    // Von Kries white-balance (diagonalised in Bradford LMS, not raw XYZ -- see
    // `compute_illuminant_white_balance`'s doc comment) so the chosen illuminant
    // itself renders as neutral white. Only the analytic `Studio` rig has a
    // single well-defined illuminant color temperature to neutralize against -- see
    // `environment_white_balance`'s own doc comment, which already documents the
    // `HdrMap` no-op. The transform is skipped entirely for `HdrMap` rather than run at
    // that documented-no-op `Vec3::ONE` scale, because the round trip is not
    // quite the identity in f32 (the two published Bradford matrices are not exact
    // inverses, `max|B*A - I| ~= 5.2e-7`): running it anyway would make a hybrid CPU/GPU
    // HDR frame -- the WGSL twin gates this same transform on `params.env_mode == 1u`
    // (`Studio`), never running it for `HdrMap` -- sum CPU and GPU tiles that disagree
    // systematically at exactly this floor. Skipping the transform entirely for
    // `HdrMap`, matching the WGSL gate, is exact instead of merely close.
    //
    // `.max(Vec3::ZERO)` clamps the result the same way `StokesVector::intensity`/
    // `cie_1931_cmf(_x8)` already clamp every other radiance quantity to non-negative.
    // Pre-white-balance `xyz` is provably non-negative, but the Bradford LMS
    // chromatic-adaptation matrices have negative off-diagonal entries, so the
    // transform does not itself preserve non-negativity for a sufficiently
    // saturated/spectrally-narrow input -- a pre-existing property of that transform.
    // Exit-event splitting can make more previously-terminated companion channels
    // survive to shift some inputs into that regime, so the clamp matters more now,
    // but it is the same physical floor already applied elsewhere in this file. Kept
    // unconditionally (including for `HdrMap`, which now skips the transform above it)
    // since it is a physical floor on `xyz` itself, not a byproduct of white balance.
    match environment {
        EnvironmentSource::Studio { .. } => {
            apply_von_kries_white_balance(xyz, environment_white_balance(environment))
        }
        EnvironmentSource::HdrMap(_) => xyz,
    }
    .max(Vec3::ZERO)
}
