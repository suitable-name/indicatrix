// ---------------------------------------------------------------------------------
// Test environment & distribution buffers for Tier 2 NEE kernels
// ---------------------------------------------------------------------------------

@group(0) @binding(62) var<storage, read> dist_test_func: array<f32>;
@group(0) @binding(63) var<storage, read> dist_test_cdf: array<f32>;
@group(0) @binding(64) var<uniform> dist_test_dims: GpuDistDims;
@group(0) @binding(65) var<storage, read> dist_test_hdr_texels: array<vec4<f32>>;
@group(0) @binding(66) var<uniform> dist_test_hdr_dims: HdrEnvDims;

fn tf_dist1d_find_bucket(cdf_start: u32, count: u32, u: f32) -> u32 {
    var first: u32 = 0u;
    var len: u32 = count + 1u;
    while (len > 0u) {
        let half = len >> 1u;
        let middle = first + half;
        if (dist_test_cdf[cdf_start + middle] <= u) {
            first = middle + 1u;
            len = len - (half + 1u);
        } else {
            len = half;
        }
    }
    // `first` saturates at 0 here (NOT `clamp(first - 1u, 0u, count - 1u)`,
    // which wraps to `u32::MAX` then clamps to `count - 1u` -- the WORST bucket,
    // exactly backwards -- whenever `first == 0u`), matching
    // `renderer::env_map_distribution::Distribution1D::find_bucket`'s own
    // `first.saturating_sub(1).min(self.n() - 1)` and the megakernel's own
    // `dist1d_find_bucket` (`transport_bounce/06_nee_sampling.wgsl`), which already
    // guards this with `if (first > 0u) { offset = first - 1u; }` before its `min`.
    var offset: u32 = 0u;
    if (first > 0u) {
        offset = first - 1u;
    }
    return min(offset, count - 1u);
}

fn tf_dist1d_bucket_pdf(func_start: u32, func_int: f32, offset: u32) -> f32 {
    if (func_int > 0.0) {
        return max(dist_test_func[func_start + offset], 0.0) / func_int;
    }
    return 1.0;
}

fn tf_dist1d_sample_continuous(
    func_start: u32,
    cdf_start: u32,
    count: u32,
    func_int: f32,
    u_in: f32,
) -> Dist1dSample {
    let u = clamp(u_in, 0.0, 0.99999994);
    let offset = tf_dist1d_find_bucket(cdf_start, count, u);
    let span = dist_test_cdf[cdf_start + offset + 1u] - dist_test_cdf[cdf_start + offset];
    var du: f32 = 0.0;
    if (span > 0.0) {
        du = (u - dist_test_cdf[cdf_start + offset]) / span;
    }
    let sample = clamp((f32(offset) + du) / f32(count), 0.0, 0.99999994);
    let pdf = tf_dist1d_bucket_pdf(func_start, func_int, offset);

    var res: Dist1dSample;
    res.sample = sample;
    res.pdf = pdf;
    res.offset = offset;
    return res;
}

fn tf_dist1d_pdf(func_start: u32, count: u32, func_int: f32, x_in: f32) -> f32 {
    let x = clamp(x_in, 0.0, 0.99999994);
    let offset = min(u32(x * f32(count)), count - 1u);
    return tf_dist1d_bucket_pdf(func_start, func_int, offset);
}

fn tf_hdr_env_sample_bilinear(u_in: f32, v_in: f32) -> vec3<f32> {
    let width = dist_test_hdr_dims.width;
    let height = dist_test_hdr_dims.height;
    let width_i = i32(width);
    let height_i = i32(height);

    let u_wrapped = fract(u_in);
    let v_clamped = clamp(v_in, 0.0, 1.0);
    let fx = fma(u_wrapped, f32(width), -0.5);
    let fy = fma(v_clamped, f32(height), -0.5);

    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;

    let x0i = hdr_wrap_x(i32(x0), width_i);
    let x1i = hdr_wrap_x(i32(x0) + 1, width_i);
    let y0i = hdr_clamp_y(i32(y0), height_i);
    let y1i = hdr_clamp_y(i32(y0) + 1, height_i);

    let p00 = dist_test_hdr_texels[u32(y0i) * width + u32(x0i)].xyz;
    let p10 = dist_test_hdr_texels[u32(y0i) * width + u32(x1i)].xyz;
    let p01 = dist_test_hdr_texels[u32(y1i) * width + u32(x0i)].xyz;
    let p11 = dist_test_hdr_texels[u32(y1i) * width + u32(x1i)].xyz;

    // Same blend order as `hdr_env_sample_bilinear` in transport_bounce.wgsl and
    // `EnvironmentMap::radiance_at`; a different formula here would hide a real
    // production mismatch behind an unrelated harness one.
    let top = fma(p10, vec3<f32>(tx), p00 * (1.0 - tx));
    let bottom = fma(p11, vec3<f32>(tx), p01 * (1.0 - tx));
    return fma(bottom, vec3<f32>(ty), top * (1.0 - ty));
}

fn tf_dist2d_sample(u0: f32, u1: f32) -> Dist2dSample {
    let width = dist_test_dims.width;
    let height = dist_test_dims.height;
    let marginal_func_start = width * height;
    let marginal_cdf_start = height * (width + 1u);
    let marginal_func_int = dist_test_dims.marginal_func_int;

    let v_sample = tf_dist1d_sample_continuous(marginal_func_start, marginal_cdf_start, height, marginal_func_int, u1);
    let row = v_sample.offset;

    let cond_func_start = row * width;
    let cond_cdf_start = row * (width + 1u);
    let cond_func_int = dist_test_func[marginal_func_start + row];
    let u_sample = tf_dist1d_sample_continuous(cond_func_start, cond_cdf_start, width, cond_func_int, u0);

    let dir = hdr_uv_to_direction(u_sample.sample, v_sample.sample);
    let rgb = tf_hdr_env_sample_bilinear(u_sample.sample, v_sample.sample);
    let pdf_uv = u_sample.pdf * v_sample.pdf;
    let pdf = pdf_uv_to_solid_angle(pdf_uv, v_sample.sample);

    var res: Dist2dSample;
    res.dir = dir;
    res.rgb = rgb;
    res.pdf = pdf;
    return res;
}

fn tf_dist2d_pdf_uv(u: f32, v: f32) -> f32 {
    let width = dist_test_dims.width;
    let height = dist_test_dims.height;
    let marginal_func_start = width * height;
    let marginal_func_int = dist_test_dims.marginal_func_int;

    let row = min(u32(clamp(v, 0.0, 0.99999994) * f32(height)), height - 1u);
    let pdf_v = tf_dist1d_pdf(marginal_func_start, height, marginal_func_int, v);

    let cond_func_start = row * width;
    let cond_func_int = dist_test_func[marginal_func_start + row];
    let pdf_u = tf_dist1d_pdf(cond_func_start, width, cond_func_int, u);

    return pdf_u * pdf_v;
}

// Same `sin(theta)`-off-the-direction form as transport_bounce.wgsl's `dist2d_pdf` and
// `EnvironmentMap::pdf` -- see the latter's own comment.
fn tf_dist2d_pdf(dir: vec3<f32>) -> f32 {
    let uv = hdr_direction_to_uv(dir);
    let pdf_uv = tf_dist2d_pdf_uv(uv.x, uv.y);
    let d = normalize(dir);
    let sin_theta = length(vec2<f32>(d.x, d.z));
    return pdf_uv_to_solid_angle_from_sin(pdf_uv, sin_theta);
}

// ---------------------------------------------------------------------------------
// dist1d_find_bucket_main
// ---------------------------------------------------------------------------------

struct Dist1dFindBucketCase {
    cdf_start: u32,
    count: u32,
    u: f32,
    _pad0: f32,
}

@group(0) @binding(67) var<storage, read> dist1d_find_bucket_cases: array<Dist1dFindBucketCase>;
@group(0) @binding(68) var<storage, read_write> dist1d_find_bucket_out: array<u32>;

@compute @workgroup_size(64)
fn dist1d_find_bucket_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&dist1d_find_bucket_cases)) {
        return;
    }
    let c = dist1d_find_bucket_cases[idx];
    dist1d_find_bucket_out[idx] = tf_dist1d_find_bucket(c.cdf_start, c.count, c.u);
}

// ---------------------------------------------------------------------------------
// dist2d_sample_main
// ---------------------------------------------------------------------------------

struct Dist2dSampleCase {
    u0: f32,
    u1: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(69) var<storage, read> dist2d_sample_cases: array<Dist2dSampleCase>;
@group(0) @binding(70) var<storage, read_write> dist2d_sample_out: array<f32>;

@compute @workgroup_size(64)
fn dist2d_sample_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&dist2d_sample_cases)) {
        return;
    }
    let c = dist2d_sample_cases[idx];
    let sample = tf_dist2d_sample(c.u0, c.u1);
    let base = idx * 7u;
    dist2d_sample_out[base + 0u] = sample.dir.x;
    dist2d_sample_out[base + 1u] = sample.dir.y;
    dist2d_sample_out[base + 2u] = sample.dir.z;
    dist2d_sample_out[base + 3u] = sample.rgb.x;
    dist2d_sample_out[base + 4u] = sample.rgb.y;
    dist2d_sample_out[base + 5u] = sample.rgb.z;
    dist2d_sample_out[base + 6u] = sample.pdf;
}

// ---------------------------------------------------------------------------------
// dist2d_pdf_main
// ---------------------------------------------------------------------------------

struct Dist2dPdfCase {
    dir: vec3<f32>,
    _pad0: f32,
}

@group(0) @binding(71) var<storage, read> dist2d_pdf_cases: array<Dist2dPdfCase>;
@group(0) @binding(72) var<storage, read_write> dist2d_pdf_out: array<f32>;

@compute @workgroup_size(64)
fn dist2d_pdf_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&dist2d_pdf_cases)) {
        return;
    }
    let c = dist2d_pdf_cases[idx];
    dist2d_pdf_out[idx] = tf_dist2d_pdf(c.dir);
}

