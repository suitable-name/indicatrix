// ---------------------------------------------------------------------------------
// Next-event estimation -- optics::raytracer::scattering::balance_heuristic.
//
// The direction-sampling (dist1d/dist2d binary search over the HDR importance
// distribution) and shadow-ray-dependent NEE contribution functions
// (`nee_contribution_hg_scatter`/`nee_contribution_frosted_exterior`) are NOT here:
// they need the `planes`/`dist_func`/`dist_cdf`/`dist_dims`/`hdr_texels` storage/uniform
// bindings, which (like `hdr_env_radiance_at`/`intersect_ray`/`try_split_exit_channel`/
// `shading_normal_near_edge` above them) are declared per-file, not in this shared
// prelude -- a `@group`/`@binding` declared here would collide with
// `transport_functions.wgsl`'s own binding numbers once `build.rs` concatenates this
// file ahead of both `spectral_transport.wgsl` AND `transport_functions.wgsl` (which
// already uses bindings 12-15 for its own Tier 2 case banks). So those five functions
// live directly in `spectral_transport.wgsl` (megakernel-local), with
// `transport_functions.wgsl` carrying its own standalone copies for
// `renderer::gpu::transport_check`'s Tier 2 self-tests -- the same "own standalone copy
// with its own binding" convention `shading_normal_near_edge`'s doc comment already
// establishes for `shaders/shading_normal.wgsl`. `balance_heuristic` itself is a pure
// function of two scalars (no binding needed), so it alone lives here, shared bit-for-bit
// by every copy of those five.
// ---------------------------------------------------------------------------------

// optics::raytracer::scattering::balance_heuristic
fn balance_heuristic(pdf_a: f32, pdf_b: f32) -> f32 {
    let denom = pdf_a + pdf_b;
    if (denom > 1e-12) {
        return pdf_a / denom;
    }
    return 0.0;
}

// ---------------------------------------------------------------------------------
// Environment map spherical direction and UV conversions, solid-angle
// Jacobian, and spectral conversion.
// Mirrors optics::raytracer::environment::EnvironmentMap::{uv_to_direction, direction_to_uv, pdf_uv_to_solid_angle}.
// ---------------------------------------------------------------------------------

fn hdr_direction_to_uv(dir: vec3<f32>) -> vec2<f32> {
    let d = normalize(dir);
    let theta = acos(clamp(d.y, -1.0, 1.0));
    let phi = atan2(d.x, d.z);
    let v = theta / PI;
    let u = fract(phi / (2.0 * PI));
    return vec2<f32>(u, v);
}

fn hdr_uv_to_direction(u_in: f32, v_in: f32) -> vec3<f32> {
    let u = fract(u_in);
    let v = clamp(v_in, 0.0, 1.0);
    let theta = v * PI;
    let phi = u * 2.0 * PI;
    // `sin` of the reflected argument, `cos` of the direct one -- exactly as
    // `EnvironmentMap::uv_to_direction`; see `pdf_uv_to_solid_angle` below for why.
    let sin_theta = sin(min(v, 1.0 - v) * PI);
    let cos_theta = cos(theta);
    let sin_phi = sin(phi);
    let cos_phi = cos(phi);
    return vec3<f32>(sin_theta * sin_phi, cos_theta, sin_theta * cos_phi);
}

// `min(v, 1.0 - v)`: the reflection `sin(PI - x) == sin(x)` folded in BEFORE the
// multiply, exactly as `renderer::env_map::pdf_uv_to_solid_angle` does -- see that
// function's own doc comment ("Why `sin(min(v, 1-v) * PI)`") for the measured
// near-south-pole error the plain `sin(v * PI)` form carries on both CPU and GPU.
fn pdf_uv_to_solid_angle(pdf_uv: f32, v: f32) -> f32 {
    let theta = min(v, 1.0 - v) * PI;
    return pdf_uv_to_solid_angle_from_sin(pdf_uv, sin(theta));
}

// renderer::env_map::pdf_uv_to_solid_angle_from_sin -- the Jacobian division with
// `sin(theta)` supplied by the caller (`dist2d_pdf` takes it straight off the direction,
// see `EnvironmentMap::pdf`'s own comment for the `acos` conditioning argument).
fn pdf_uv_to_solid_angle_from_sin(pdf_uv: f32, sin_theta: f32) -> f32 {
    if (sin_theta <= 1e-6) {
        return 0.0;
    }
    return pdf_uv / (2.0 * PI * PI * sin_theta);
}

fn hdr_wrap_x(x: i32, width: i32) -> u32 {
    return u32(((x % width) + width) % width);
}

fn hdr_clamp_y(y: i32, height: i32) -> u32 {
    return u32(clamp(y, 0, height - 1));
}

fn asymmetric_gaussian(x: f32, mu: f32, sigma_lo: f32, sigma_hi: f32) -> f32 {
    var sigma: f32;
    if (x < mu) {
        sigma = sigma_lo;
    } else {
        sigma = sigma_hi;
    }
    let t = (x - mu) / sigma;
    return exp(-0.5 * t * t);
}

fn rgb_to_spectral_radiance(r: f32, g: f32, b: f32, lambda_nm: f32) -> f32 {
    let rc = max(r, 0.0);
    let gc = max(g, 0.0);
    let bc = max(b, 0.0);
    return fma(
        rc, asymmetric_gaussian(lambda_nm, 615.0, 45.0, 65.0),
        fma(
            gc, asymmetric_gaussian(lambda_nm, 545.0, 45.0, 45.0),
            bc * asymmetric_gaussian(lambda_nm, 465.0, 40.0, 45.0),
        ),
    );
}

struct HdrEnvDims {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

struct GpuDistDims {
    width: u32,
    height: u32,
    marginal_func_int: f32,
    _pad0: f32,
}

struct Dist1dSample {
    sample: f32,
    pdf: f32,
    offset: u32,
}

struct Dist2dSample {
    dir: vec3<f32>,
    rgb: vec3<f32>,
    pdf: f32,
}

