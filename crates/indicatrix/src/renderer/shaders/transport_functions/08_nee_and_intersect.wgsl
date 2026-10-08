// ---------------------------------------------------------------------------------
// nee_frosted_exterior_main
// ---------------------------------------------------------------------------------

struct NeeFrostedExteriorCase {
    ext_normal: vec3<f32>,
    rng_seed: u32,
    bounce: u32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
    lambdas: array<f32, 8>,
    stokes: array<vec4<f32>, 8>,
    radiance_in: array<f32, 8>,
}

@group(0) @binding(73) var<storage, read> nee_frosted_cases: array<NeeFrostedExteriorCase>;
@group(0) @binding(74) var<storage, read_write> nee_frosted_out: array<f32>;

fn tf_nee_contribution_frosted_exterior(
    lambdas: ptr<function, array<f32, 8>>,
    ext_normal: vec3<f32>,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
) {
    let u0 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_U_STREAM))) / 4294967295.0;
    let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_V_STREAM))) / 4294967295.0;
    let sample = tf_dist2d_sample(u0, u1);
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

@compute @workgroup_size(64)
fn nee_frosted_exterior_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&nee_frosted_cases)) {
        return;
    }
    let c = nee_frosted_cases[idx];
    var lambdas = c.lambdas;
    var stokes = c.stokes;
    var radiance = c.radiance_in;
    tf_nee_contribution_frosted_exterior(
        &lambdas, c.ext_normal, c.rng_seed, c.bounce, &stokes, &radiance,
    );
    let base = idx * 8u;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        nee_frosted_out[base + k] = radiance[k];
    }
}

// ---------------------------------------------------------------------------------
// nee_hg_scatter_main
// ---------------------------------------------------------------------------------

struct FacetPlane {
    normal: vec3<f32>,
    d: f32,
}

struct HitInfo {
    hit: bool,
    t: f32,
    normal: vec3<f32>,
    facet_idx: u32,
}

@group(0) @binding(75) var<storage, read> tf_planes: array<FacetPlane>;

fn tf_intersect_ray(origin: vec3<f32>, dir: vec3<f32>) -> HitInfo {
    var t_near: f32 = -1e30;
    var t_far: f32 = 1e30;
    var near_normal = vec3<f32>(0.0, 0.0, 0.0);
    var far_normal = vec3<f32>(0.0, 0.0, 0.0);
    var near_idx: u32 = 0u;
    var far_idx: u32 = 0u;
    var result: HitInfo;
    let num_planes = arrayLength(&tf_planes);
    for (var i: u32 = 0u; i < num_planes; i = i + 1u) {
        let p = tf_planes[i];
        let n = p.normal;
        let denom = dot(n, dir);
        let side = p.d + dot(n, origin);
        let numer = -side;
        if (abs(denom) > 1e-7) {
            let t = numer / denom;
            if (denom < 0.0) {
                if (t > t_near) {
                    t_near = t;
                    near_normal = n;
                    near_idx = i;
                }
            } else if (t < t_far) {
                t_far = t;
                far_normal = n;
                far_idx = i;
            }
        } else if (side > 0.0) {
            result.hit = false;
            result.t = 0.0;
            result.normal = vec3<f32>(0.0, 0.0, 0.0);
            result.facet_idx = 0u;
            return result;
        }
    }
    if (t_near > t_far) {
        result.hit = false;
        result.t = 0.0;
        result.normal = vec3<f32>(0.0, 0.0, 0.0);
        result.facet_idx = 0u;
    } else if (t_near > 1e-4) {
        result.hit = true;
        result.t = t_near;
        result.normal = near_normal;
        result.facet_idx = near_idx;
    } else if (t_far > 1e-4) {
        result.hit = true;
        result.t = t_far;
        result.normal = far_normal;
        result.facet_idx = far_idx;
    } else {
        result.hit = false;
        result.t = 0.0;
        result.normal = vec3<f32>(0.0, 0.0, 0.0);
        result.facet_idx = 0u;
    }
    return result;
}


// `sigma_s`/`absorption_path_scale`/`alphas` and `frosted_exit` mirror
// `optics::raytracer::scattering::nee_contribution_hg_scatter`'s own extra parameters
// -- the standalone harness has no scene-wide `material`/`facet_finishes` bindings the
// megakernel (`transport_bounce.wgsl`) reads those from, so each case carries its own
// copy instead (see the CPU-side `NeeHgScatterCase` struct's doc comments for how
// `run_nee_hg_scatter` drives them).
struct NeeHgScatterCase {
    scatter_point: vec3<f32>,
    n_inside_hero: f32,
    scatter_dir_in: vec3<f32>,
    g: f32,
    rng_seed: u32,
    bounce: u32,
    sigma_s: f32,
    absorption_path_scale: f32,
    frosted_exit: u32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
    alphas: array<f32, 8>,
    lambdas: array<f32, 8>,
    stokes: array<vec4<f32>, 8>,
    path_pdf: array<f32, 8>,
    compat: array<u32, 8>,
}

@group(0) @binding(76) var<storage, read> nee_hg_cases: array<NeeHgScatterCase>;
@group(0) @binding(77) var<storage, read_write> nee_hg_out: array<f32>;

// Operation-for-operation copy of `transport_bounce/06_nee_sampling.wgsl`'s
// `nee_contribution_hg_scatter` (itself the WGSL translation of
// `optics::raytracer::scattering::nee_contribution_hg_scatter` plus the immediate
// family-integration `try_scatter_step` applies to its deposit), with the same signature
// for the spectral inputs (`path_pdf`, `compat`, `nee_xyz`), adapted only in how it
// reaches the medium/finish inputs the megakernel
// reads off the shared `material`/`facet_finishes` bindings: this standalone twin takes
// them as explicit per-case parameters instead (`alphas`, `sigma_s`,
// `absorption_path_scale`, `frosted_exit`), and samples the HDR environment through this
// file's own `tf_hdr_env_sample_bilinear`/`tf_dist2d_sample` (bindings 62-66) rather than
// the megakernel's scene-wide texture bindings.
fn tf_nee_contribution_hg_scatter(
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
    sigma_s: f32,
    absorption_path_scale: f32,
    frosted_exit: u32,
) {
    let u0 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_U_STREAM))) / 4294967295.0;
    let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_V_STREAM))) / 4294967295.0;
    let sample = tf_dist2d_sample(u0, u1);
    if (sample.pdf <= 0.0) {
        return;
    }

    let probe_origin = scatter_point + sample.dir * 1e-4;
    let hit = tf_intersect_ray(probe_origin, sample.dir);
    if (!hit.hit) {
        return;
    }
    // Mirrors `nee_contribution_hg_scatter`'s `facet_finishes` check -- the
    // standalone harness has no scene-wide facet-finish buffer to index by `hit.facet_idx`,
    // so the CPU driver marks the whole test cube Frosted or Polished up front and passes
    // that verdict directly (see `NeeHgScatterCase::frosted_exit`'s doc comment).
    if (frosted_exit != 0u) {
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
    // same Snell's-law form as the megakernel twin; `sample.pdf` stays in the INTERIOR
    // (pre-refraction) measure `tf_dist2d_sample` sampled in, only the radiance LOOKUP
    // moves to the refracted direction.
    let refracted_dir = normalize(n_inside_hero * sample.dir - fma(n_inside_hero, cos_i, -cos_t) * hit.normal);
    let refracted_uv = hdr_direction_to_uv(refracted_dir);
    let env_rgb = tf_hdr_env_sample_bilinear(refracted_uv.x, refracted_uv.y);

    // The medium transmittance a phase-sampled continuation reaching this
    // same boundary would have paid -- the same per-channel
    // `exp_poly(-(alphas[k]+sigma_s)*hit.t*path_scale)` the megakernel twin and the CPU
    // function both apply (`exp_poly`, not the `exp()` builtin -- see
    // that function's own doc comment, `transport_physics.wgsl`).
    let hit_t_scaled = hit.t * absorption_path_scale;
    // `sigma_s` is per model unit (size-independent): converted to absorption-length units
    // exactly as the CPU twin does (`sigma_s / absorption_path_scale`).
    let sigma_s_abs = sigma_s / absorption_path_scale;

    let nee_common = t_unpol * phase_val * mis_weight / sample.pdf;
    var nee_deposit: array<f32, 8>;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        let transmittance_k = exp_poly(-(alphas[k] + sigma_s_abs) * hit_t_scaled);
        let env_k = rgb_to_spectral_radiance(env_rgb.x, env_rgb.y, env_rgb.z, (*lambdas)[k]);
        nee_deposit[k] = fma((*stokes)[k].x * transmittance_k * nee_common * env_k, 1.0, 0.0);
    }
    (*nee_xyz) = (*nee_xyz) + integrate_channels_to_xyz_family(nee_deposit, *lambdas, *path_pdf, *compat);
}

@compute @workgroup_size(64)
fn nee_hg_scatter_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&nee_hg_cases)) {
        return;
    }
    let c = nee_hg_cases[idx];
    var lambdas = c.lambdas;
    var stokes = c.stokes;
    var path_pdf = c.path_pdf;
    var compat = c.compat;
    var nee_xyz = vec3<f32>(0.0, 0.0, 0.0);
    tf_nee_contribution_hg_scatter(
        &lambdas, c.n_inside_hero, c.scatter_point, c.scatter_dir_in,
        c.g, c.rng_seed, c.bounce, &stokes, &path_pdf, &compat, &nee_xyz,
        c.alphas, c.sigma_s, c.absorption_path_scale, c.frosted_exit,
    );
    nee_hg_out[idx * 3u + 0u] = nee_xyz.x;
    nee_hg_out[idx * 3u + 1u] = nee_xyz.y;
    nee_hg_out[idx * 3u + 2u] = nee_xyz.z;
}
