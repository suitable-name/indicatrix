// ---------------------------------------------------------------------------------
// GPU Next-Event Estimation & Multiple Importance Sampling
// ---------------------------------------------------------------------------------

fn dist1d_find_bucket(cdf_start: u32, n: u32, u: f32) -> u32 {
    var first: u32 = 0u;
    var len: u32 = n + 1u;
    while (len > 0u) {
        let half = len / 2u;
        let middle = first + half;
        if (dist_cdf[cdf_start + middle] <= u) {
            first = middle + 1u;
            len = len - (half + 1u);
        } else {
            len = half;
        }
    }
    var offset: u32 = 0u;
    if (first > 0u) {
        offset = first - 1u;
    }
    return min(offset, n - 1u);
}

fn dist1d_bucket_pdf(func_start: u32, offset: u32, func_int: f32) -> f32 {
    if (func_int > 0.0) {
        return max(dist_func[func_start + offset], 0.0) / func_int;
    }
    return 1.0;
}

fn dist1d_sample_continuous(cdf_start: u32, func_start: u32, n: u32, func_int: f32, u_in: f32) -> Dist1dSample {
    let u = clamp(u_in, 0.0, 0.99999994);
    let offset = dist1d_find_bucket(cdf_start, n, u);
    let cdf0 = dist_cdf[cdf_start + offset];
    let cdf1 = dist_cdf[cdf_start + offset + 1u];
    let span = cdf1 - cdf0;
    var du: f32 = 0.0;
    if (span > 0.0) {
        du = (u - cdf0) / span;
    }
    let sample = clamp((f32(offset) + du) / f32(n), 0.0, 0.99999994);
    let pdf = dist1d_bucket_pdf(func_start, offset, func_int);
    var res: Dist1dSample;
    res.sample = sample;
    res.pdf = pdf;
    res.offset = offset;
    return res;
}

fn dist1d_pdf(func_start: u32, n: u32, func_int: f32, x: f32) -> f32 {
    let offset = min(u32(clamp(x, 0.0, 0.99999994) * f32(n)), n - 1u);
    return dist1d_bucket_pdf(func_start, offset, func_int);
}

fn dist2d_sample(u0: f32, u1: f32) -> Dist2dSample {
    let width = dist_dims.width;
    let height = dist_dims.height;
    let marginal_cdf_start = height * (width + 1u);
    let marginal_func_start = width * height;
    let marginal_func_int = dist_dims.marginal_func_int;

    let s_v = dist1d_sample_continuous(marginal_cdf_start, marginal_func_start, height, marginal_func_int, u1);
    let row = s_v.offset;
    let v = s_v.sample;
    let pdf_v = s_v.pdf;

    let cond_cdf_start = row * (width + 1u);
    let cond_func_start = row * width;
    let cond_func_int = dist_func[marginal_func_start + row];

    let s_u = dist1d_sample_continuous(cond_cdf_start, cond_func_start, width, cond_func_int, u0);
    let u = s_u.sample;
    let pdf_u = s_u.pdf;

    let dir = hdr_uv_to_direction(u, v);
    let rgb = hdr_env_sample_bilinear(u, v);
    let pdf = pdf_uv_to_solid_angle(pdf_u * pdf_v, v);

    var res: Dist2dSample;
    res.dir = dir;
    res.rgb = rgb;
    res.pdf = pdf;
    return res;
}

fn dist2d_pdf_uv(u: f32, v: f32) -> f32 {
    let width = dist_dims.width;
    let height = dist_dims.height;
    let marginal_func_start = width * height;
    let marginal_func_int = dist_dims.marginal_func_int;

    let row = min(u32(clamp(v, 0.0, 0.99999994) * f32(height)), height - 1u);
    let pdf_v = dist1d_pdf(marginal_func_start, height, marginal_func_int, v);

    let cond_func_start = row * width;
    let cond_func_int = dist_func[marginal_func_start + row];
    let pdf_u = dist1d_pdf(cond_func_start, width, cond_func_int, u);

    return pdf_u * pdf_v;
}

// renderer::env_map::EnvironmentMap::pdf -- `sin(theta)` off the unit direction
// (`length(vec2(x, z))`, the CPU's `x.hypot(z)`), never `sin(acos(y))`; see that
// function's own comment.
fn dist2d_pdf(dir: vec3<f32>) -> f32 {
    let uv = hdr_direction_to_uv(dir);
    let pdf_uv = dist2d_pdf_uv(uv.x, uv.y);
    let d = normalize(dir);
    let sin_theta = length(vec2<f32>(d.x, d.z));
    return pdf_uv_to_solid_angle_from_sin(pdf_uv, sin_theta);
}

// optics::raytracer::scattering::nee_contribution_hg_scatter. `alphas` mirrors that
// function's own explicit parameter; `sigma_s`/`absorption_path_scale`
// and `facet_finishes` are read directly off the global
// `material`/`facet_finishes` bindings instead, exactly as the megakernel's own scatter
// call site above (`transport_bounce_step`) already does for the SAME quantities.
fn nee_contribution_hg_scatter(
    lambdas: ptr<function, array<f32, 8>>,
    n_inside_hero: f32,
    scatter_point: vec3<f32>,
    scatter_dir_in: vec3<f32>,
    g: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
    alphas: array<f32, 8>,
) {
    let u0 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_U_STREAM))) / 4294967295.0;
    let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_V_STREAM))) / 4294967295.0;
    let sample = dist2d_sample(u0, u1);
    if (sample.pdf <= 0.0) {
        return;
    }

    let probe_origin = scatter_point + sample.dir * 1e-4;
    let hit = intersect_ray(probe_origin, sample.dir);
    if (!hit.hit) {
        return;
    }
    // A frosted exit facet has no well-defined specular Fresnel/refraction
    // for this shadow ray to use -- `nee_contribution_frosted_exterior` already handles
    // NEE for a frosted exit's own diffusely-sampled surface point.
    if (hit.facet_idx < arrayLength(&facet_finishes) && facet_finishes[hit.facet_idx] == FACET_FINISH_FROSTED) {
        return;
    }

    let cos_i = clamp(dot(sample.dir, hit.normal), 0.0, 1.0);
    let sin2_t = min(n_inside_hero * n_inside_hero * fma(-cos_i, cos_i, 1.0), 1.0);
    if (sin2_t >= 1.0) {
        return;
    }
    let cos_t = sqrt(max(1.0 - sin2_t, 0.0));
    let r_s = fma(n_inside_hero, cos_i, -cos_t) / fma(n_inside_hero, cos_i, cos_t);
    let r_p = fma(n_inside_hero, -cos_t, cos_i) / fma(n_inside_hero, cos_t, cos_i);
    let r_unpol = clamp(0.5 * fma(r_p, r_p, r_s * r_s), 0.0, 1.0);
    let t_unpol = 1.0 - r_unpol;

    let phase_cos = dot(sample.dir, scatter_dir_in);
    let phase_val = henyey_greenstein_phase(phase_cos, g);
    let mis_weight = balance_heuristic(sample.pdf, phase_val);
    if (mis_weight <= 0.0) {
        return;
    }

    // The exterior direction this light sample actually leaves along --
    // Snell's law at the exit facet, mirroring `refraction.wgsl`'s own
    // `eta*k_hat + (eta*cos_i - cos_t)*normal` vector form with `sample.dir` playing
    // the incident-direction role and (the inward-flipped) `-hit.normal` the surface
    // normal. `sample.pdf` itself stays in the INTERIOR (pre-refraction) measure
    // `dist2d_sample` sampled in -- only the radiance LOOKUP moves to the refracted
    // direction.
    let refracted_dir = normalize(n_inside_hero * sample.dir - fma(n_inside_hero, cos_i, -cos_t) * hit.normal);
    let refracted_uv = hdr_direction_to_uv(refracted_dir);
    let env_rgb = hdr_env_sample_bilinear(refracted_uv.x, refracted_uv.y);

    // The medium transmittance a phase-sampled continuation reaching this
    // same boundary would have paid -- the same per-channel `exp_poly(-(alphas[k]+sigma_s)*
    // hit.t*path_scale)` `maybe_scatter_or_extinguish`'s survive branch applies
    // (`exp_poly`, not the `exp()` builtin -- see that function's own doc comment,
    // `transport_physics.wgsl`).
    let hit_t_scaled = hit.t * material.absorption_path_scale;

    let nee_common = t_unpol * phase_val * mis_weight / sample.pdf;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        let transmittance_k = exp_poly(-(alphas[k] + material.scattering_sigma_s) * hit_t_scaled);
        let env_k = rgb_to_spectral_radiance(env_rgb.x, env_rgb.y, env_rgb.z, (*lambdas)[k]);
        (*radiance)[k] = fma((*stokes)[k].x * transmittance_k * nee_common * env_k, 1.0, (*radiance)[k]);
    }
}

fn nee_contribution_frosted_exterior(
    lambdas: ptr<function, array<f32, 8>>,
    ext_normal: vec3<f32>,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
) {
    let u0 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_U_STREAM))) / 4294967295.0;
    let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_V_STREAM))) / 4294967295.0;
    let sample = dist2d_sample(u0, u1);
    if (sample.pdf <= 0.0) {
        return;
    }

    let cos_light = dot(sample.dir, ext_normal);
    if (cos_light <= 0.0) {
        return;
    }

    let brdf_pdf = cos_light / PI;
    let mis_weight = balance_heuristic(sample.pdf, brdf_pdf);
    if (mis_weight <= 0.0) {
        return;
    }

    let nee_common = brdf_pdf * mis_weight / sample.pdf;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        let env_k = rgb_to_spectral_radiance(sample.rgb.x, sample.rgb.y, sample.rgb.z, (*lambdas)[k]);
        (*radiance)[k] = fma((*stokes)[k].x * nee_common * env_k, 1.0, (*radiance)[k]);
    }
}

