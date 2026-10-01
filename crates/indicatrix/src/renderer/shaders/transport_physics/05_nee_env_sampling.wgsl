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
    // Neutral part min(r, g, b) on the wide bumps; chroma remainder on the narrow bumps.
    let w = min(min(rc, gc), bc);
    let cr = rc - w;
    let cg = gc - w;
    let cb = bc - w;
    // 3x3 correction so the wide bump basis reproduces the neutral part under the CMFs.
    let n_r = fma(0.823989868, w, fma(-0.305067778, w, 0.263915569 * w));
    let n_g = fma(-0.297157794, w, fma(1.25289488, w, -0.461147606 * w));
    let n_b = fma(0.0788375437, w, fma(-0.122806296, w, 1.23178256 * w));
    let neutral = fma(
        n_r, asymmetric_gaussian(lambda_nm, 615.0, 45.0, 65.0),
        fma(
            n_g, asymmetric_gaussian(lambda_nm, 545.0, 45.0, 45.0),
            n_b * asymmetric_gaussian(lambda_nm, 465.0, 40.0, 45.0),
        ),
    );
    // Entrywise-positive 3x3 for the narrow bumps: non-negative chroma gives
    // non-negative coefficients, so the chroma spectrum is exact.
    let k_r = fma(1.0995291, cr, fma(0.08711245, cg, 0.01771266 * cb));
    let k_g = fma(0.007455747, cr, fma(1.3345385, cg, 0.014020192 * cb));
    let k_b = fma(0.02487475, cr, fma(0.058899768, cg, 1.2609048 * cb));
    let saturated = fma(
        k_r, asymmetric_gaussian(lambda_nm, 635.0, 28.0, 39.2),
        fma(
            k_g, asymmetric_gaussian(lambda_nm, 540.0, 28.0, 28.0),
            k_b * asymmetric_gaussian(lambda_nm, 450.0, 25.2, 28.0),
        ),
    );
    return max(0.0, neutral + saturated);
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


// ---------------------------------------------------------------------------------
// Spectral-to-XYZ integration shared by the megakernel, the wavefront pipeline and the
// standalone transport-function kernels.
// ---------------------------------------------------------------------------------

const NUM_CHANNELS: u32 = 8u;
const NORM_FACTOR: f32 = (400.0 / 8.0) / 106.856;

// cie_1931_cmf -- color::cie1931::cie_1931_cmf: CIE 1931 2-degree observer, tabulated at
// 5nm (380-780nm, CIE_15_2004_CMF_TABLE) and linearly interpolated -- ported identically
// to shaders/environment.wgsl / shaders/furnace.wgsl. See that Rust function's own doc
// comment for why a WGSL port must stay bit-identical: plain f32 arithmetic in a fixed
// order (floor, fraction, `lo + (hi - lo) * t`), no mul_add/fma, no f64 intermediate
// anywhere. This uses the real tabulated observer rather than a Wyman/Sloan/Shirley
// Gaussian-lobe fit, which carries 1-3% XYZ error against it (worst in the x_bar trough
// around 495-510nm) -- see `color::cie1931`'s module doc comment.

const CIE_15_2004_CMF_START_NM: f32 = 380.0;
const CIE_15_2004_CMF_STEP_NM: f32 = 5.0;
const CIE_15_2004_CMF_LAST_INDEX: u32 = 80u;
// CIE_15_2004_CMF_START_NM + 80.0 * CIE_15_2004_CMF_STEP_NM, precomputed since WGSL
// `const` initializers can't call CIE_15_2004_CMF_TABLE.length() the way Rust's
// `CIE_1931_TABLE.len()` can.
const CIE_15_2004_CMF_END_NM: f32 = 780.0;

// CIE 15:2004 Table T.4 / CIE 1931 2-degree observer, 5nm, 380-780nm (81 entries) --
// SAME values as `color::cie1931::CIE_1931_TABLE`, transcribed by hand from that array
// (not generated), so a future edit to one must be mirrored into the other by hand too.
const CIE_15_2004_CMF_TABLE: array<vec3<f32>, 81> = array<vec3<f32>, 81>(
    vec3<f32>(0.0014, 0.0000, 0.0065), // 380nm
    vec3<f32>(0.0022, 0.0001, 0.0105), // 385nm
    vec3<f32>(0.0042, 0.0001, 0.0201), // 390nm
    vec3<f32>(0.0076, 0.0002, 0.0362), // 395nm
    vec3<f32>(0.0143, 0.0004, 0.0679), // 400nm
    vec3<f32>(0.0232, 0.0006, 0.1102), // 405nm
    vec3<f32>(0.0435, 0.0012, 0.2074), // 410nm
    vec3<f32>(0.0776, 0.0022, 0.3713), // 415nm
    vec3<f32>(0.1344, 0.0040, 0.6456), // 420nm
    vec3<f32>(0.2148, 0.0073, 1.0391), // 425nm
    vec3<f32>(0.2839, 0.0116, 1.3856), // 430nm
    vec3<f32>(0.3285, 0.0168, 1.6230), // 435nm
    vec3<f32>(0.3483, 0.0230, 1.7471), // 440nm
    vec3<f32>(0.3481, 0.0298, 1.7826), // 445nm
    vec3<f32>(0.3362, 0.0380, 1.7721), // 450nm
    vec3<f32>(0.3187, 0.0480, 1.7441), // 455nm
    vec3<f32>(0.2908, 0.0600, 1.6692), // 460nm
    vec3<f32>(0.2511, 0.0739, 1.5281), // 465nm
    vec3<f32>(0.1954, 0.0910, 1.2876), // 470nm
    vec3<f32>(0.1421, 0.1126, 1.0419), // 475nm
    vec3<f32>(0.0956, 0.1390, 0.8130), // 480nm
    vec3<f32>(0.0580, 0.1693, 0.6162), // 485nm
    vec3<f32>(0.0320, 0.2080, 0.4652), // 490nm
    vec3<f32>(0.0147, 0.2586, 0.3533), // 495nm
    vec3<f32>(0.0049, 0.3230, 0.2720), // 500nm
    vec3<f32>(0.0024, 0.4073, 0.2123), // 505nm
    vec3<f32>(0.0093, 0.5030, 0.1582), // 510nm
    vec3<f32>(0.0291, 0.6082, 0.1117), // 515nm
    vec3<f32>(0.0633, 0.7100, 0.0782), // 520nm
    vec3<f32>(0.1096, 0.7932, 0.0573), // 525nm
    vec3<f32>(0.1655, 0.8620, 0.0422), // 530nm
    vec3<f32>(0.2257, 0.9149, 0.0298), // 535nm
    vec3<f32>(0.2904, 0.9540, 0.0203), // 540nm
    vec3<f32>(0.3597, 0.9803, 0.0134), // 545nm
    vec3<f32>(0.4334, 0.9950, 0.0087), // 550nm
    vec3<f32>(0.5121, 1.0000, 0.0057), // 555nm
    vec3<f32>(0.5945, 0.9950, 0.0039), // 560nm
    vec3<f32>(0.6784, 0.9786, 0.0027), // 565nm
    vec3<f32>(0.7621, 0.9520, 0.0021), // 570nm
    vec3<f32>(0.8425, 0.9154, 0.0018), // 575nm
    vec3<f32>(0.9163, 0.8700, 0.0017), // 580nm
    vec3<f32>(0.9786, 0.8163, 0.0014), // 585nm
    vec3<f32>(1.0263, 0.7570, 0.0011), // 590nm
    vec3<f32>(1.0567, 0.6949, 0.0010), // 595nm
    vec3<f32>(1.0622, 0.6310, 0.0008), // 600nm
    vec3<f32>(1.0456, 0.5668, 0.0006), // 605nm
    vec3<f32>(1.0026, 0.5030, 0.0003), // 610nm
    vec3<f32>(0.9384, 0.4412, 0.0002), // 615nm
    vec3<f32>(0.8544, 0.3810, 0.0002), // 620nm
    vec3<f32>(0.7514, 0.3210, 0.0001), // 625nm
    vec3<f32>(0.6424, 0.2650, 0.0000), // 630nm
    vec3<f32>(0.5419, 0.2170, 0.0000), // 635nm
    vec3<f32>(0.4479, 0.1750, 0.0000), // 640nm
    vec3<f32>(0.3608, 0.1382, 0.0000), // 645nm
    vec3<f32>(0.2835, 0.1070, 0.0000), // 650nm
    vec3<f32>(0.2187, 0.0816, 0.0000), // 655nm
    vec3<f32>(0.1649, 0.0610, 0.0000), // 660nm
    vec3<f32>(0.1212, 0.0446, 0.0000), // 665nm
    vec3<f32>(0.0874, 0.0320, 0.0000), // 670nm
    vec3<f32>(0.0636, 0.0232, 0.0000), // 675nm
    vec3<f32>(0.0468, 0.0170, 0.0000), // 680nm
    vec3<f32>(0.0329, 0.0119, 0.0000), // 685nm
    vec3<f32>(0.0227, 0.0082, 0.0000), // 690nm
    vec3<f32>(0.0158, 0.0057, 0.0000), // 695nm
    vec3<f32>(0.0114, 0.0041, 0.0000), // 700nm
    vec3<f32>(0.0081, 0.0029, 0.0000), // 705nm
    vec3<f32>(0.0058, 0.0021, 0.0000), // 710nm
    vec3<f32>(0.0041, 0.0015, 0.0000), // 715nm
    vec3<f32>(0.0029, 0.0010, 0.0000), // 720nm
    vec3<f32>(0.0020, 0.0007, 0.0000), // 725nm
    vec3<f32>(0.0014, 0.0005, 0.0000), // 730nm
    vec3<f32>(0.0010, 0.0004, 0.0000), // 735nm
    vec3<f32>(0.0007, 0.0002, 0.0000), // 740nm
    vec3<f32>(0.0005, 0.0002, 0.0000), // 745nm
    vec3<f32>(0.0003, 0.0001, 0.0000), // 750nm
    vec3<f32>(0.0002, 0.0001, 0.0000), // 755nm
    vec3<f32>(0.0002, 0.0001, 0.0000), // 760nm
    vec3<f32>(0.0001, 0.0000, 0.0000), // 765nm
    vec3<f32>(0.0001, 0.0000, 0.0000), // 770nm
    vec3<f32>(0.0001, 0.0000, 0.0000), // 775nm
    vec3<f32>(0.0000, 0.0000, 0.0000), // 780nm
);

fn cie_1931_cmf(l: f32) -> vec3<f32> {
    if (!(CIE_15_2004_CMF_START_NM <= l && l <= CIE_15_2004_CMF_END_NM)) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    let position = (l - CIE_15_2004_CMF_START_NM) / CIE_15_2004_CMF_STEP_NM;
    let index0 = min(u32(floor(position)), CIE_15_2004_CMF_LAST_INDEX - 1u);
    let index1 = index0 + 1u;
    let t = position - f32(index0);
    let lo = CIE_15_2004_CMF_TABLE[index0];
    let hi = CIE_15_2004_CMF_TABLE[index1];
    return lo + (hi - lo) * t;
}

// optics::raytracer::color::integrate_channels_to_xyz_families -- each channel `k`
// weighted by the balance heuristic over exactly its own MIS *family* (`compat[k]`, the
// hero choices under which channel `k` would have stayed alive on this same geometric
// path), not the hero's own family. Factored out here (rather than left inline at its
// one-time-only call site) so `nee_contribution_hg_scatter` (a scattering-point
// NEE deposit integrated to XYZ immediately, using THAT moment's own `path_pdf`/
// `compat`, not the trace's final one) and `transport_finalize_ray`'s own end-of-trace
// integration share one definition instead of two independently-maintained copies.
fn integrate_channels_to_xyz_family(radiance: array<f32, 8>, lambdas: array<f32, 8>, path_pdf: array<f32, 8>, compat: array<u32, 8>) -> vec3<f32> {
    var xyz = vec3<f32>(0.0, 0.0, 0.0);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        var family_pdf: f32 = 0.0;
        for (var j: u32 = 0u; j < NUM_CHANNELS; j = j + 1u) {
            if ((compat[k] & (1u << j)) != 0u) {
                family_pdf = family_pdf + path_pdf[j];
            }
        }
        // Same "should not happen" fallback as the shared `spectral_mis_weight`.
        var weight_k: f32 = 1.0;
        if (family_pdf > 1e-12) {
            weight_k = f32(NUM_CHANNELS) * path_pdf[0] / family_pdf;
        }
        let cmf = cie_1931_cmf(lambdas[k]);
        let weighted = radiance[k] * weight_k;
        xyz = xyz + cmf * (weighted * NORM_FACTOR);
    }
    return xyz;
}
