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

// ---- concave-stone shadow probe of the Henyey-Greenstein NEE ----
// optics::raytracer::scattering::hg_nee::{exit_transmittance, walk_exits}. Only reached when
// `params.num_tools > 0`; a convex stone has exactly one crossing and keeps the original
// single-exit code in `nee_contribution_hg_scatter`.

// The most boundaries the probe crosses before it gives the sample up
// (`HG_NEE_MAX_CROSSINGS`).
const HG_NEE_MAX_CROSSINGS: u32 = 4u;

struct HgExit {
    ok: bool,
    normal: vec3<f32>,
    t_material: f32,
    cos_i: f32,
    cos_t: f32,
    t_unpol: f32,
}

// Unpolarised exit Fresnel transmittance and refracted-angle cosine for a ray leaving an
// interior of index `n_inside` at `cos_i`, as `(t_unpol, cos_t)`; `x < 0` marks total
// internal reflection (the sample is dropped).
fn hg_exit_transmittance(n_inside: f32, cos_i: f32) -> vec2<f32> {
    let sin2_t = min(n_inside * n_inside * fma(-cos_i, cos_i, 1.0), 1.0);
    if (sin2_t >= 1.0) {
        return vec2<f32>(-1.0, 0.0);
    }
    let cos_t = sqrt(max(1.0 - sin2_t, 0.0));
    let r_s = fma(n_inside, cos_i, -cos_t) / fma(n_inside, cos_i, cos_t);
    let r_p = fma(n_inside, -cos_t, cos_i) / fma(n_inside, cos_t, cos_i);
    let r_unpol = clamp(0.5 * fma(r_p, r_p, r_s * r_s), 0.0, 1.0);
    return vec2<f32>(1.0 - r_unpol, cos_t);
}

// The concave-stone probe: follows the sample direction through up to
// `HG_NEE_MAX_CROSSINGS` boundaries, multiplying in the Fresnel transmittance at each exit
// and at each re-entry through a cavity's far wall, and summing the length of every
// in-material segment for the Beer-Lambert term (each measured from the scattering point
// or the re-entry wall itself: every in-material segment starts `HG_SHADOW_PROBE_OFFSET`
// inside the material, so the offset is added back). Drops the sample above the cap, on
// TIR or a frosted exit at any crossing, and when the walk ends on an entry. The walk
// keeps the interior direction through the air gaps; only the last exit bends it (see the
// CPU twin).
fn hg_walk_exits(probe_origin: vec3<f32>, dir: vec3<f32>, first: HitInfo, n_inside: f32) -> HgExit {
    var out: HgExit;
    out.ok = false;
    out.normal = vec3<f32>(0.0, 0.0, 0.0);
    out.t_material = 0.0;
    out.cos_i = 0.0;
    out.cos_t = 0.0;
    out.t_unpol = 0.0;
    var origin = probe_origin;
    var hit = first;
    var crossings: u32 = 1u;
    var chain: f32 = 1.0;
    var material_len: f32 = 0.0;
    var have_last = false;
    var last_normal = vec3<f32>(0.0, 0.0, 0.0);
    var last_cos_i: f32 = 0.0;
    var last_cos_t: f32 = 0.0;
    for (var guard: u32 = 0u; guard <= HG_NEE_MAX_CROSSINGS; guard = guard + 1u) {
        if (hit.facet_idx < arrayLength(&facet_finishes) && facet_finishes[hit.facet_idx] == FACET_FINISH_FROSTED) {
            return out;
        }
        let cos_d = dot(dir, hit.normal);
        if (cos_d > 0.0) {
            // The segment ending at an exit is in-material, from an origin offset
            // `HG_SHADOW_PROBE_OFFSET` inside it (the probe's own origin, or the restart
            // just past a re-entry).
            material_len = material_len + (hit.t + HG_SHADOW_PROBE_OFFSET);
            let cos_in = clamp(cos_d, 0.0, 1.0);
            let tr = hg_exit_transmittance(n_inside, cos_in);
            if (tr.x < 0.0) {
                return out;
            }
            chain = chain * tr.x;
            have_last = true;
            last_normal = hit.normal;
            last_cos_i = cos_in;
            last_cos_t = tr.y;
        } else {
            // Re-entry through a far cavity wall: the air-gap segment ending here is not
            // absorbing, but the interface transmits only `T`.
            let tr = hg_exit_transmittance(n_inside, clamp(-cos_d, 0.0, 1.0));
            if (tr.x < 0.0) {
                return out;
            }
            chain = chain * tr.x;
            have_last = false;
        }
        origin = origin + hit.t * dir + dir * HG_SHADOW_PROBE_OFFSET;
        let next = intersect_ray(origin, dir);
        if (!next.hit) {
            break;
        }
        crossings = crossings + 1u;
        if (crossings > HG_NEE_MAX_CROSSINGS) {
            return out;
        }
        hit = next;
    }
    if (!have_last) {
        return out;
    }
    out.ok = true;
    out.normal = last_normal;
    out.t_material = material_len;
    out.cos_i = last_cos_i;
    out.cos_t = last_cos_t;
    out.t_unpol = chain;
    return out;
}

// optics::raytracer::scattering::hg_nee::nee_contribution_hg_scatter. `alphas` is the
// segment's own per-channel array (`bounce_scatter_alphas`, evaluated at the direction the
// ray ARRIVED along): used as it is for an isotropic material, whose coefficients do not
// depend on direction (the CPU's `ShadowAlphas::Fixed` arm); for an anisotropic material
// (`rc.is_anisotropic`, the `ShadowAlphas::AssignedMode` arm) the shadow ray's coefficients
// are re-evaluated per channel at the SHADOW direction below, since that is what the
// phase-sampled continuation in that direction pays -- see the CPU `ShadowAlphas` doc
// comment for the NEE-on-darker bias the incoming-direction alphas caused on the ruby.
// `sigma_s`/`absorption_path_scale` and `facet_finishes` are read directly off the global
// `material`/`facet_finishes` bindings instead, exactly as the megakernel's own scatter
// call site above (`transport_bounce_step`) already does for the SAME quantities.
// This deposit is a complete, self-contained direct-lighting sample -- it must
// be combined into XYZ using `path_pdf`/`compat` AS THEY STAND RIGHT NOW (this
// scattering event's own per-channel technique densities), never the FINAL
// `path_pdf`/`compat` the rest of the path happens to end up with (unrelated further
// bounces this same light sample has no bearing on). Integrated locally into `nee_deposit`
// then folded into the caller's own running `*nee_xyz` immediately below, mirroring
// `optics::raytracer::scattering::try_scatter_step`'s identical CPU-side fix.
fn nee_contribution_hg_scatter(
    lambdas: ptr<function, array<f32, 8>>,
    n_inside_hero: f32,
    scatter_point: vec3<f32>,
    scatter_dir_in: vec3<f32>,
    g: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
    compat: ptr<function, array<u32, 8>>,
    nee_xyz: ptr<function, vec3<f32>>,
    alphas: array<f32, 8>,
    rc: RayConstants,
    is_extraordinary: bool,
) {
    let u0 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_U_STREAM))) / 4294967295.0;
    let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_V_STREAM))) / 4294967295.0;
    let sample = dist2d_sample(u0, u1);
    if (sample.pdf <= 0.0) {
        return;
    }

    let probe_origin = scatter_point + sample.dir * HG_SHADOW_PROBE_OFFSET;
    let hit = intersect_ray(probe_origin, sample.dir);
    if (!hit.hit) {
        return;
    }

    // The exit the shadow ray finally leaves through. A convex stone has exactly one
    // crossing (`hit`); a concave one may cross out into a cavity and back in first, so
    // the probe walks its boundaries (`hg_walk_exits`, the twin of `walk_exits`). The
    // in-material length is measured from the scattering point itself: the probe's origin
    // sits `HG_SHADOW_PROBE_OFFSET` inside the material, so `hit.t` alone is that much short.
    var exit_normal = hit.normal;
    var exit_t = hit.t + HG_SHADOW_PROBE_OFFSET;
    var cos_i: f32 = 0.0;
    var cos_t: f32 = 0.0;
    var t_unpol: f32 = 0.0;
    if (params.num_tools == 0u) {
        // A frosted exit facet has no well-defined specular Fresnel/refraction
        // for this shadow ray to use -- `nee_contribution_frosted_exterior` already handles
        // NEE for a frosted exit's own diffusely-sampled surface point.
        if (hit.facet_idx < arrayLength(&facet_finishes) && facet_finishes[hit.facet_idx] == FACET_FINISH_FROSTED) {
            return;
        }

        cos_i = clamp(dot(sample.dir, hit.normal), 0.0, 1.0);
        let sin2_t = min(n_inside_hero * n_inside_hero * fma(-cos_i, cos_i, 1.0), 1.0);
        if (sin2_t >= 1.0) {
            return;
        }
        cos_t = sqrt(max(1.0 - sin2_t, 0.0));
        let r_s = fma(n_inside_hero, cos_i, -cos_t) / fma(n_inside_hero, cos_i, cos_t);
        let r_p = fma(n_inside_hero, -cos_t, cos_i) / fma(n_inside_hero, cos_t, cos_i);
        let r_unpol = clamp(0.5 * fma(r_p, r_p, r_s * r_s), 0.0, 1.0);
        t_unpol = 1.0 - r_unpol;
    } else {
        let walked = hg_walk_exits(probe_origin, sample.dir, hit, n_inside_hero);
        if (!walked.ok) {
            return;
        }
        exit_normal = walked.normal;
        exit_t = walked.t_material;
        cos_i = walked.cos_i;
        cos_t = walked.cos_t;
        t_unpol = walked.t_unpol;
    }

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
    let refracted_dir = normalize(n_inside_hero * sample.dir - fma(n_inside_hero, cos_i, -cos_t) * exit_normal);
    let refracted_uv = hdr_direction_to_uv(refracted_dir);
    let env_rgb = hdr_env_sample_bilinear(refracted_uv.x, refracted_uv.y);

    // The medium transmittance a phase-sampled continuation in this same direction would
    // pay on its way to this same boundary -- the same per-channel
    // `exp_poly(-(alpha_k+sigma_s)*d*path_scale)` `maybe_scatter_or_extinguish`'s survive
    // branch applies (`exp_poly`, not the `exp()` builtin -- see that function's own doc
    // comment, `transport_physics.wgsl`), with `alpha_k` the assigned mode's coefficient AT
    // the shadow direction (see the header comment) and `d` the full distance from the
    // scattering point.
    let hit_t_scaled = exit_t * material.absorption_path_scale;
    // `scattering_sigma_s` is per model unit (size-independent): converted to absorption-length
    // units exactly as the CPU twin does (`sigma_s / absorption_path_scale`).
    let sigma_s_abs = material.scattering_sigma_s / material.absorption_path_scale;

    let nee_common = t_unpol * phase_val * mis_weight / sample.pdf;
    var nee_deposit: array<f32, 8>;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        // The shadow ray's own extinction coefficient: the segment's `alphas[k]` for an
        // isotropic material (direction-independent), the assigned mode's coefficient at
        // the shadow direction for an anisotropic one (`scatter_alpha_anisotropic`, the same
        // evaluation `bounce_scatter_alphas` makes for the ray's own segments).
        var alpha_k: f32 = alphas[k];
        if (rc.is_anisotropic) {
            alpha_k = scatter_alpha_anisotropic(rc, k, sample.dir, is_extraordinary);
        }
        let transmittance_k = exp_poly(-(alpha_k + sigma_s_abs) * hit_t_scaled);
        let env_k = rgb_to_spectral_radiance(env_rgb.x, env_rgb.y, env_rgb.z, (*lambdas)[k]);
        nee_deposit[k] = fma((*stokes)[k].x * transmittance_k * nee_common * env_k, 1.0, 0.0);
    }
    (*nee_xyz) = (*nee_xyz) + integrate_channels_to_xyz_family(nee_deposit, *lambdas, *path_pdf, *compat);
}

// Deliberately still deposits straight into the caller's shared `radiance` (weighted,
// like every other channel's contribution, by the FINAL post-loop `path_pdf` at
// `transport_finalize_ray`), unlike `nee_contribution_hg_scatter` above -- see
// `optics::raytracer::scattering::apply_frosted_bounce`'s "Why the exterior NEE deposit
// needs no separate `nee_xyz` accumulator" doc comment for why that is exact,
// not an oversight: both call sites below are convex-polyhedron surface points sampled
// into their own true-outward half-space, so the very next bounce is guaranteed to
// escape directly with no intervening `path_pdf` update.
//
// Two light-sampling techniques share this body, mirroring `sample_environment_for_nee`:
// the HDR map's importance distribution (`env_mode == 2u`) and the analytic sun of
// `LightingModel::DaylightSun` (`env_mode == 1u`, `studio_model == 5u`: a direction uniform
// over the 0.27 degree disc about `key_dir`, `pdf = 1 / SUN_SOLID_ANGLE`, radiance
// `daylight_sun_factor * (spd * exposure)`). `key_dir` is only read for the sun.
fn nee_contribution_frosted_exterior(
    lambdas: ptr<function, array<f32, 8>>,
    // The surface point of this bounce, the origin of the concave-stone shadow probe.
    origin: vec3<f32>,
    ext_normal: vec3<f32>,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
    key_dir: vec3<f32>,
) {
    let u0 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_U_STREAM))) / 4294967295.0;
    let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_V_STREAM))) / 4294967295.0;
    let sun_mode = params.env_mode != 2u;
    var light_dir = vec3<f32>(0.0, 0.0, 0.0);
    var light_pdf: f32 = 0.0;
    var hdr_rgb = vec3<f32>(0.0, 0.0, 0.0);
    if (sun_mode) {
        light_dir = daylight_sun_cone_direction(key_dir, u0, u1);
        light_pdf = 1.0 / SUN_SOLID_ANGLE;
    } else {
        let sample = dist2d_sample(u0, u1);
        light_dir = sample.dir;
        light_pdf = sample.pdf;
        hdr_rgb = sample.rgb;
    }
    if (light_pdf <= 0.0) {
        return;
    }

    let cos_light = dot(light_dir, ext_normal);
    if (cos_light <= 0.0) {
        return;
    }

    // Concave stone: the sampled direction may run into the far side of a cavity (a point
    // on a groove wall moving along its outward normal can meet the opposite wall), so cast
    // a real shadow probe and deposit only on a miss. Any hit, even one past a cavity, is an
    // occluder: the escaping BSDF-sampled continuation sees the same visibility, so the two
    // MIS techniques stay consistent. The probe runs after the RNG draws, so the stream
    // consumption equals the convex stone's. Optics twin:
    // `nee_contribution_frosted_exterior`'s "Concave stones" section.
    if (params.num_tools != 0u) {
        if (intersect_ray(origin + light_dir * 1e-4, light_dir).hit) {
            return;
        }
    }

    let brdf_pdf = cos_light / PI;
    let mis_weight = balance_heuristic(light_pdf, brdf_pdf);
    if (mis_weight <= 0.0) {
        return;
    }

    let nee_common = brdf_pdf * mis_weight / light_pdf;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        var env_k: f32;
        if (sun_mode) {
            env_k = daylight_sun_factor(key_dir) * (studio_spectral_power((*lambdas)[k]) * params.studio_exposure);
        } else {
            env_k = rgb_to_spectral_radiance(hdr_rgb.x, hdr_rgb.y, hdr_rgb.z, (*lambdas)[k]);
        }
        (*radiance)[k] = fma((*stokes)[k].x * nee_common * env_k, 1.0, (*radiance)[k]);
    }
}

// Whether this scene offers a light-sampling technique at all: an HDR map, or the analytic
// sun (`environment_supports_nee` on the CPU). Gates the frosted-exterior NEE and the
// MIS weight of a frosted continuation's escape. The Henyey-Greenstein scattering NEE stays
// HDR-only (`params.env_mode == 2u`), see `nee_contribution_hg_scatter`'s CPU twin.
fn nee_light_available() -> bool {
    return params.env_mode == 2u || (params.env_mode == 1u && params.studio_model == 5u);
}

// `environment_nee_pdf`: the light-sampling technique's solid-angle pdf at `dir` -- the HDR
// distribution's, or `daylight_sun_nee_pdf`'s (zero outside the disc).
fn nee_light_pdf(dir: vec3<f32>, key_dir: vec3<f32>) -> f32 {
    if (params.env_mode == 2u) {
        return dist2d_pdf(dir);
    }
    return daylight_sun_nee_pdf(normalize(dir), key_dir);
}

