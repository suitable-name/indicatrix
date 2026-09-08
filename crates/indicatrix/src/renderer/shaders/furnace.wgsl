// Phase 1: furnace-anchor kernel -- driven by `renderer::gpu::furnace_check`.
//
// Glues together every Phase-1 ported function into one end-to-end pipeline (camera ray
// generation with RNG-derived jitter, the hero-wavelength comb, CIE 1931 CMF
// integration) with a deliberately UNIFORM constant-radiance "environment" (independent
// of both direction and wavelength) and, implicitly, zero gemstone facets -- so the
// resulting XYZ is analytically computable from the CMF integral alone, checking this
// pipeline against TRUE values rather than merely against the CPU (a shared porting
// mistake could otherwise self-certify a CPU-vs-GPU-only comparison).
//
// # Why this kernel never calls `intersect_polyhedron`
//
// `optics::raytracer::intersect_polyhedron(ray, &[])` (zero facet planes) does NOT
// return `None` -- its `t_near`/`t_far` sentinels (`-1e30`/`+1e30`) fall through to the
// "origin is inside the solid" EXIT branch (vacuously true: with zero half-space
// constraints, every point satisfies all of them), producing
// `Some(HitRecord { t: 1e30, normal: Vec3::ZERO, facet_idx: 0 })`. See
// `renderer::gpu::furnace_check`'s own doc comment and its
// `empty_planes_intersect_returns_the_sentinel_hit_not_a_miss` test, which pins this
// exact CPU behavior down. Functionally this IS "the ray reaches the environment
// unobstructed" (there is no real geometry at `t = 1e30` to interact with), which is
// the property this kernel's "uniform environment, unconditionally sampled" design
// relies on -- it does not need to branch on the `Option` wrapper at all, because with
// zero planes by construction there is nothing that could ever block a ray.

struct GpuCameraParams {
    origin: vec3<f32>,
    fov_tan: f32,
    forward: vec3<f32>,
    width: f32,
    right: vec3<f32>,
    height: f32,
    up: vec3<f32>,
    num_samples: u32,
}

struct FurnaceExtra {
    l0: f32,
    num_pixels: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> camera: GpuCameraParams;
@group(0) @binding(1) var<uniform> extra: FurnaceExtra;
@group(0) @binding(2) var<storage, read_write> out_persample: array<f32>;
@group(0) @binding(3) var<storage, read_write> out_pixel_sum: array<f32>;

const SPECTRUM_MIN: f32 = 380.0;
const SPECTRUM_SPAN: f32 = 400.0;
const NUM_CHANNELS: u32 = 8u;
// (400.0 / 8) / 106.856 -- must match `optics::raytracer::integrate_channels_to_xyz`'s
// `norm_factor` exactly. Computed via the SAME division WGSL will perform at
// const-evaluation time, deliberately not a hand-rounded decimal literal: an earlier
// version of this constant was hand-computed as 0.4680704632214494 (a transcription
// error -- the correct value is ~0.4679194), which produced a ~0.03% systematic bias
// caught by this module's own per-tuple ULP check (see `furnace_check`'s negative
// control / this bug's own discovery for how that showed up in practice).
const NORM_FACTOR: f32 = (400.0 / 8.0) / 106.856;

fn hash_u32(x_in: u32) -> u32 {
    var x = x_in;
    x = x * 0x85ebca6bu;
    x = x ^ (x >> 13u);
    x = x * 0xc2b2ae35u;
    x = x ^ (x >> 16u);
    return x;
}

// P4: CIE 1931 2-degree observer, tabulated at 5nm (CIE 15:2004 Table T.4, 380-780nm)
// and linearly interpolated -- ported identically to shaders/environment.wgsl and the
// CMF region of shaders/spectral_transport.wgsl (all three copies transcribed by hand
// from `color::cie1931::CIE_1931_TABLE`, so an edit to one must be mirrored into the
// other two by hand too). Replaces the retired Wyman/Sloan/Shirley Gaussian-lobe fit,
// which carried 1-3% XYZ error against the real tabulated observer -- see
// `color::cie1931`'s module doc comment. Deliberately plain f32 arithmetic in the same
// fixed order as that Rust function (floor, fraction, `lo + (hi - lo) * t`), no
// mul_add/fma, no f64 intermediate anywhere, so this reproduces it bit-for-bit modulo
// ordinary driver-level f32 rounding.

const CIE_15_2004_CMF_START_NM: f32 = 380.0;
const CIE_15_2004_CMF_STEP_NM: f32 = 5.0;
const CIE_15_2004_CMF_LAST_INDEX: u32 = 80u;
// CIE_15_2004_CMF_START_NM + 80.0 * CIE_15_2004_CMF_STEP_NM, precomputed since WGSL
// `const` initializers can't call CIE_15_2004_CMF_TABLE.length() the way Rust's
// `CIE_1931_TABLE.len()` can.
const CIE_15_2004_CMF_END_NM: f32 = 780.0;

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

/// One (pixel, sample) tuple's furnace XYZ estimate: camera ray generation (with
/// RNG-derived jitter, bit-exact per Phase 0) + the hero-wavelength comb + CMF
/// integration against the uniform constant-radiance environment. Shared by both entry
/// points below.
fn furnace_sample_xyz(pixel: u32, sample: u32) -> vec3<f32> {
    let x = f32(pixel % u32(camera.width));
    let y = f32(pixel / u32(camera.width));

    let seed = hash_u32((pixel * 0x9e3779b9u) ^ (sample * 0x85ebca6bu));
    let jx = f32(hash_u32(seed) % 10000u) / 10000.0 - 0.5;
    let jy = f32(hash_u32(seed + 0x7feb352du) % 10000u) / 10000.0 - 0.5;

    // Camera::generate_ray -- exercised for pipeline fidelity even though this
    // furnace's environment is direction-independent (see the file header): `dir` is
    // computed exactly as the real ray-generation path would, it just happens not to
    // affect this particular environment's radiance.
    let aspect = camera.width / camera.height;
    let u = ((x + jx) / camera.width - 0.5) * 2.0 * aspect * camera.fov_tan;
    let v = (0.5 - (y + jy) / camera.height) * 2.0 * camera.fov_tan;
    let dir = normalize(camera.forward + camera.right * u + camera.up * v);
    // `dir` deliberately unused beyond this point -- see above.
    _ = dir;

    let hero_hash = hash_u32(seed);
    let hero_rand = f32(hero_hash) / 4294967295.0;
    let channel_width = SPECTRUM_SPAN / f32(NUM_CHANNELS);
    let lambda_hero = fma(hero_rand, SPECTRUM_SPAN, SPECTRUM_MIN);

    var xyz = vec3<f32>(0.0, 0.0, 0.0);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        let offset = fma(f32(k), channel_width, lambda_hero - SPECTRUM_MIN);
        let wrapped = offset % SPECTRUM_SPAN;
        let lambda = SPECTRUM_MIN + wrapped;
        // Uniform environment: `extra.l0` regardless of `lambda`/`dir`. `mis_weight` is
        // exactly 1.0 here (every channel shares the same, uniform path_pdf), matching
        // `integrate_channels_to_xyz`'s `spectral_mis_weight` degenerating to 1.0 when
        // every channel's technique agrees.
        xyz = xyz + cie_1931_cmf(lambda) * (extra.l0 * NORM_FACTOR);
    }
    return xyz;
}

@compute @workgroup_size(64)
fn furnace_samples_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    let total = extra.num_pixels * camera.num_samples;
    if (idx >= total) {
        return;
    }
    let pixel = idx / camera.num_samples;
    let sample = idx % camera.num_samples;
    let xyz = furnace_sample_xyz(pixel, sample);
    out_persample[idx * 3u + 0u] = xyz.x;
    out_persample[idx * 3u + 1u] = xyz.y;
    out_persample[idx * 3u + 2u] = xyz.z;
}

// Strictly sequential, this-thread-only accumulation across `camera.num_samples` --
// exactly `self_determinism.wgsl`'s proven-safe pattern (see that file's own doc
// comment): no atomics, no cross-thread reduction, so the result is bit-for-bit
// reproducible run to run regardless of GPU scheduling.
@compute @workgroup_size(64)
fn furnace_accumulate_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pixel = gid.x;
    if (pixel >= extra.num_pixels) {
        return;
    }
    var sum = vec3<f32>(0.0, 0.0, 0.0);
    for (var s: u32 = 0u; s < camera.num_samples; s = s + 1u) {
        sum = sum + furnace_sample_xyz(pixel, s);
    }
    out_pixel_sum[pixel * 3u + 0u] = sum.x;
    out_pixel_sum[pixel * 3u + 1u] = sum.y;
    out_pixel_sum[pixel * 3u + 2u] = sum.z;
}
