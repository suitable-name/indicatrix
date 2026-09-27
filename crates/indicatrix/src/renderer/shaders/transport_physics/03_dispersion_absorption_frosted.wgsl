
// ---------------------------------------------------------------------------------
// optics::dispersion::DispersionModel::evaluate -- takes the dispersion params as
// explicit arguments (rather than reading a `material: GpuGemMaterial` binding
// directly) so the exact same function body is callable both from the megakernel
// (which has that binding) and from Tier 2's `dispersion_main` (which reads per-case
// values out of its own `DispersionCase` storage buffer instead).
// ---------------------------------------------------------------------------------

fn dispersion_evaluate(model_type: u32, param_a: vec4<f32>, param_b: vec4<f32>, lambda_nm: f32) -> f32 {
    let lambda_um = lambda_nm * 1e-3;
    let l2 = lambda_um * lambda_um;
    if (model_type == 0u) {
        let n2 = 1.0 + (param_a.x * l2) / (l2 - param_b.x);
        return sqrt(max(n2, 1.0));
    } else if (model_type == 1u) {
        var n2: f32 = 1.0;
        n2 = n2 + (param_a.x * l2) / (l2 - param_b.x);
        n2 = n2 + (param_a.y * l2) / (l2 - param_b.y);
        n2 = n2 + (param_a.z * l2) / (l2 - param_b.z);
        return sqrt(max(n2, 1.0));
    } else {
        let l4 = l2 * l2;
        return param_a.x + (param_a.y / l2) + (param_a.z / l4);
    }
}

// P3 (extraordinary-ray dispersion GPU port): optics::dispersion::DispersionModel::
// evaluate, evaluated against a material's OPTIONAL independent extraordinary-ray
// curve (`GpuGemMaterial::has_extraordinary_dispersion`/`extraordinary_model_type`/
// `extraordinary_param_a`/`extraordinary_param_b` -- see `renderer::buffers::
// GpuGemMaterial`'s own doc comment). A separate function from `dispersion_evaluate`
// above -- rather than that function reused as-is -- because the CPU's
// `DispersionModel::evaluate` floors EVERY variant at `n >= 1.0`
// (`sqrt(max(n2, 1.0))` on both Sellmeier branches, an explicit `.max(1.0)` on the
// Cauchy branch -- see that function's own doc comment on why an out-of-fit-range
// extrapolation must never produce a physically-impossible index), while
// `dispersion_evaluate` -- ported only for the ORDINARY-ray curve, whose Cauchy fits
// this crate's built-ins only ever evaluate well within their validated range -- has
// never needed the Cauchy floor and omits it. Every built-in extraordinary-ray curve
// today (Quartz/Amethyst/Citrine) is Sellmeier3, whose floor `dispersion_evaluate`
// already applies identically, but this function stays a faithful, complete port of
// `DispersionModel::evaluate` (Cauchy floor included) rather than one that happens to
// agree only for the variant currently in use.
fn extraordinary_dispersion_evaluate(model_type: u32, param_a: vec4<f32>, param_b: vec4<f32>, lambda_nm: f32) -> f32 {
    let lambda_um = lambda_nm * 1e-3;
    let l2 = lambda_um * lambda_um;
    if (model_type == 0u) {
        let n2 = 1.0 + (param_a.x * l2) / (l2 - param_b.x);
        return sqrt(max(n2, 1.0));
    } else if (model_type == 1u) {
        var n2: f32 = 1.0;
        n2 = n2 + (param_a.x * l2) / (l2 - param_b.x);
        n2 = n2 + (param_a.y * l2) / (l2 - param_b.y);
        n2 = n2 + (param_a.z * l2) / (l2 - param_b.z);
        return sqrt(max(n2, 1.0));
    } else {
        let l4 = l2 * l2;
        return max(param_a.x + (param_a.y / l2) + (param_a.z / l4), 1.0);
    }
}

// optics::raytracer::per_channel_uniaxial_indices -- one channel's (n_o, n_eff) pair;
// see the Phase 3 section header comment above for why the CPU's internal
// NUM_CHANNELS loop is the WGSL caller's responsibility instead of this function's.
// Placed after `dispersion_evaluate` (which it calls) rather than up in the Phase 3
// section above, purely so every function here is defined after everything it calls.
fn per_channel_uniaxial_index(
    model_type: u32,
    param_a: vec4<f32>,
    param_b: vec4<f32>,
    lambda_nm: f32,
    birefringence_delta: f32,
    is_anisotropic: bool,
    theta_c: f32,
) -> vec2<f32> {
    let n_o_k = dispersion_evaluate(model_type, param_a, param_b, lambda_nm);
    let n_e_k = n_o_k + birefringence_delta;
    var n_eff_k = n_o_k;
    if (is_anisotropic) {
        n_eff_k = effective_extraordinary_index(n_o_k, n_e_k, theta_c);
    }
    return vec2<f32>(n_o_k, n_eff_k);
}

// ---------------------------------------------------------------------------------
// optics::raytracer::spectral_absorption -- takes the band array/count as explicit
// arguments (rather than reading `material.o_ray_bands`/`material.e_ray_bands`
// directly) for the same reason as `dispersion_evaluate` above: one shared body, two
// different binding shapes at the call sites. The megakernel calls this once per
// eigenmode (`material.o_ray_bands`/`o_ray_band_count`, then `e_ray_bands`/
// `e_ray_band_count`), avoiding two near-identical `_o`/`_e` copies of this function.
// ---------------------------------------------------------------------------------

// P6: `shape` selects which domain this band is Gaussian in (0 = GaussianWavelength,
// 1 = GaussianEnergy) -- see `optics::absorption::BandShape` and
// `renderer::buffers::band_shape`. Mirrors `renderer::buffers::GpuAbsorptionBand`
// field-for-field, echoed and size-asserted by `renderer::gpu::layout_check` against
// `layout_echo.wgsl`.
struct AbsorptionBand {
    center_nm: f32,
    width_nm: f32,
    peak: f32,
    shape: u32,
}

// optics::raytracer::spectral_absorption / optics::absorption::AbsorptionBand::evaluate.
// The `shape == 0u` (GaussianWavelength) branch keeps the exact same f32 op order as
// before `shape` was added, so every existing wavelength-domain material's GPU result is
// byte-identical to before this function grew a shape branch.
fn spectral_absorption(bands: array<AbsorptionBand, 8>, band_count: u32, lambda_nm: f32) -> f32 {
    var sum: f32 = 0.0;
    for (var i: u32 = 0u; i < band_count; i = i + 1u) {
        let band = bands[i];
        if (band.shape == 1u) {
            // Well-conditioned wavenumber-difference form -- see
            // optics::absorption::AbsorptionBand::evaluate's GaussianEnergy branch (in
            // absorption.rs) for the full derivation and the f64-verified measurement.
            // Must stay op-for-op identical to that branch.
            let delta_nm = band.center_nm - lambda_nm;
            let nu_diff = 1.0e7 * delta_nm / (lambda_nm * band.center_nm);
            let t = nu_diff / band.width_nm;
            sum = sum + band.peak * exp(-0.5 * t * t);
        } else {
            let t = (lambda_nm - band.center_nm) / band.width_nm;
            sum = sum + band.peak * exp(-0.5 * t * t);
        }
    }
    return sum;
}

// ---------------------------------------------------------------------------------
// Frosted-facet GPU port: optics::raytracer::{frosted_orthonormal_basis,
// cosine_weighted_hemisphere, apply_frosted_bounce} -- the diffuse (bruted/frosted
// girdle facet) bounce, ported here (not into `spectral_transport.wgsl` or
// `transport_functions.wgsl` separately) so the shipped megakernel and Tier 2's
// standalone `frosted_bounce_main`/`cosine_hemisphere_main` kernels call the exact same
// function object, never two texts that could drift -- see this file's own header
// comment for why that property matters (a duplicate-vs-shipped-code fault is
// otherwise caught only by luck, see `renderer::gpu::transport_check`'s module doc
// comment).
//
// # The one deliberate simplification, preserved exactly
//
// `apply_frosted_bounce` is achromatic BY DESIGN: every spectral channel shares the ONE
// direction drawn below (not a per-channel direction) and the ONE broadband
// reflect/transmit split `r_unpol` (computed from the HERO channel's `n1`/`n2`/`cos_i`
// only -- never a per-channel `r_unpol_k`). That is what lets a frosted bounce compose
// with the existing per-channel `path_pdf` bookkeeping and the final
// `spectral_mis_weight`/MIS combination with NO chromatic-termination guard: a smooth,
// finite-support hemisphere BSDF assigns strictly positive density to the realized
// direction under every channel's own hypothetical hero-driven technique (unlike a
// delta BSDF, whose density is exactly zero off its one wavelength-dependent
// direction), so there is no measure-zero mismatch to drop to zero. See
// `optics::raytracer::apply_frosted_bounce`'s own doc comment for the full derivation
// -- this WGSL translation must never diverge from it: no per-channel direction, no
// per-channel `r_unpol_k`, no extra `path_pdf` division (the cosine-weighted-hemisphere
// pdf already exactly cancels the assumed Lambertian `albedo = 1.0` BRDF/BTDF, folded
// into the `1.0 / r_unpol` / `1.0 / t_unpol` throughput scale below).
// ---------------------------------------------------------------------------------

struct FrostedBasis {
    t: vec3<f32>,
    b: vec3<f32>,
}

// optics::raytracer::frosted_orthonormal_basis
fn frosted_orthonormal_basis(n: vec3<f32>) -> FrostedBasis {
    var a: vec3<f32>;
    if (abs(n.x) > 0.9) {
        a = vec3<f32>(0.0, 1.0, 0.0);
    } else {
        a = vec3<f32>(1.0, 0.0, 0.0);
    }
    let t = normalize_or_zero(a - n * dot(n, a));
    let b = cross(n, t);
    var result: FrostedBasis;
    result.t = t;
    result.b = b;
    return result;
}

// optics::raytracer::cosine_weighted_hemisphere -- Malley's method (polar mapping, not
// the concentric-disk variant), so this is a direct line-for-line translation.
fn cosine_weighted_hemisphere(u1: f32, u2: f32, n: vec3<f32>) -> vec3<f32> {
    let r = sqrt(u1);
    let theta = 2.0 * PI * u2;
    let sin_t = sin(theta);
    let cos_t = cos(theta);
    let basis = frosted_orthonormal_basis(n);
    let dir = basis.t * (r * cos_t) + basis.b * (r * sin_t) + n * sqrt(max(1.0 - u1, 0.0));
    return normalize_or_zero(dir);
}

// The (new_dir, new_inside_gem, has_extraordinary_update, extraordinary_update) tuple
// `apply_frosted_bounce` returns, encoded for WGSL (which has no `Option<bool>`):
// `has_extraordinary_update == 0u` is the CPU's `None` (the TIR-forced and reflect
// arms); `!= 0u` is `Some(extraordinary_update != 0u)` (only reachable from the
// transmit arm's `entering_anisotropic` branch, mirroring
// optics::raytracer::apply_frosted_bounce's `entering_anisotropic.then_some(..)`).
struct FrostedBounceResult {
    new_dir: vec3<f32>,
    new_inside_gem: u32,
    has_extraordinary_update: u32,
    extraordinary_update: u32,
}

// optics::raytracer::apply_frosted_bounce -- the CPU signature takes `&RayMaterialContext`
// / `&BounceRefractionGeometry`; this WGSL translation flattens exactly the fields that
// function actually reads out of them (`ctx.is_anisotropic` and
// `geo.{sin2_t,n1,n2,cos_i}` -- see the CPU function's own doc comment) into explicit
// scalar/vector parameters, the same flattening convention every other ported function
// in this file already uses for its CPU struct-based counterpart (e.g.
// `theta_c_for_bounce` above). `stokes`/`path_pdf` are `ptr<function, ...>` so this
// mutates the caller's own local arrays in place, mirroring the CPU's `&mut
// [StokesVector; NUM_CHANNELS]` / `&mut [f32; NUM_CHANNELS]` out-parameters exactly.
fn apply_frosted_bounce(
    is_anisotropic: bool,
    sin2_t: f32,
    n1: f32,
    n2: f32,
    cos_i: f32,
    normal: vec3<f32>,
    inside_gem: bool,
    is_extraordinary: bool,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
) -> FrostedBounceResult {
    let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_DIR_U_STREAM))) / 4294967295.0;
    let u2 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_DIR_V_STREAM))) / 4294967295.0;

    var result: FrostedBounceResult;

    if (sin2_t > 1.0) {
        // Forced reflect (TIR), probability 1 -- no draw, no pdf division, mirroring
        // optics::raytracer::apply_tir_bounce's identical reasoning for the polished
        // path.
        let new_dir = cosine_weighted_hemisphere(u1, u2, normal);
        for (var k: u32 = 0u; k < 8u; k = k + 1u) {
            let intensity = max((*stokes)[k].x, 0.0);
            (*stokes)[k] = vec4<f32>(intensity, 0.0, 0.0, 0.0);
        }
        result.new_dir = new_dir;
        result.new_inside_gem = select(0u, 1u, inside_gem);
        result.has_extraordinary_update = 0u;
        result.extraordinary_update = 0u;
        return result;
    }

    let cos_t = sqrt(max(1.0 - sin2_t, 0.0));
    let r_s = fma(n2, -cos_t, n1 * cos_i) / fma(n2, cos_t, n1 * cos_i);
    let r_p = fma(n1, -cos_t, n2 * cos_i) / fma(n1, cos_t, n2 * cos_i);
    let r_unpol = clamp(0.5 * fma(r_p, r_p, r_s * r_s), R_UNPOL_MIN, R_UNPOL_MAX);
    let rng_bounce = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM))) / 4294967295.0;

    if (rng_bounce < r_unpol) {
        let new_dir = cosine_weighted_hemisphere(u1, u2, normal);
        for (var k: u32 = 0u; k < 8u; k = k + 1u) {
            let intensity = max((*stokes)[k].x, 0.0) / r_unpol;
            (*stokes)[k] = vec4<f32>(intensity, 0.0, 0.0, 0.0);
            (*path_pdf)[k] = (*path_pdf)[k] * r_unpol;
        }
        result.new_dir = new_dir;
        result.new_inside_gem = select(0u, 1u, inside_gem);
        result.has_extraordinary_update = 0u;
        result.extraordinary_update = 0u;
        return result;
    }

    let new_dir = cosine_weighted_hemisphere(u1, u2, -normal);
    let entering_anisotropic = (!inside_gem) && is_anisotropic;
    // Mode SELECTION is a stochastic 50/50 draw with no throughput weighting (no
    // `split_pdf` divisor/multiplier): weighting by the split probability would
    // estimate twice the transmitted energy no interface can deliver. See
    // optics::raytracer::apply_frosted_bounce's doc comment for the full energy-share
    // reasoning (same shape as the polished path's entry split in refraction.rs).
    var use_extraordinary = is_extraordinary;
    if (entering_anisotropic) {
        let split_rand = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))) / 4294967295.0;
        use_extraordinary = split_rand < 0.5;
    }
    let t_unpol = 1.0 - r_unpol;
    // No `/ split_pdf` -- see the entering_anisotropic comment above.
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        let intensity = max((*stokes)[k].x, 0.0) / t_unpol;
        (*stokes)[k] = vec4<f32>(intensity, 0.0, 0.0, 0.0);
        // No `* split_pdf` -- scale-invariant under a uniform per-channel factor, was a
        // pure no-op on the MIS weight; see refraction.rs.
        (*path_pdf)[k] = (*path_pdf)[k] * t_unpol;
    }
    result.new_dir = new_dir;
    result.new_inside_gem = select(1u, 0u, inside_gem);
    if (entering_anisotropic) {
        result.has_extraordinary_update = 1u;
        result.extraordinary_update = select(0u, 1u, use_extraordinary);
    } else {
        result.has_extraordinary_update = 0u;
        result.extraordinary_update = 0u;
    }
    return result;
}

