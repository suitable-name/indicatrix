// The independent Ray-Traced kernel. Four storage buffers, three uniforms.
const wt_PI: f32 = 3.14159265358979323846;
struct WtCamera {
    origin: vec3<f32>, fov_tan: f32,
    forward: vec3<f32>, width: f32,
    right: vec3<f32>, height: f32,
    up: vec3<f32>, num_samples: u32,
}
struct GpuWhittedParams {
    counts: vec4<u32>, range: vec4<u32>, modes: vec4<u32>,
    values: vec4<f32>, indices: vec4<f32>,
    alpha_o: vec4<f32>, alpha_e: vec4<f32>, axis: vec4<f32>,
    masks: array<vec4<f32>, 3>, rgb_to_xyz: array<vec4<f32>, 3>,
    white_balance: vec4<f32>, env_rgb: vec4<f32>,
    key_dir: vec4<f32>, fill_dir: vec4<f32>, studio: vec4<f32>,
    shadow: vec4<f32>, tent: vec4<f32>, extra: vec4<f32>,
    aa: array<vec4<f32>, 16>, ring_dirs: array<vec4<f32>, 16>,
}
@group(0) @binding(0) var<uniform> wt_camera: WtCamera;
@group(0) @binding(1) var<uniform> wt: GpuWhittedParams;
@group(0) @binding(2) var<storage, read> wt_planes: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> wt_env: array<vec4<f32>>;
@group(0) @binding(4) var<uniform> wt_dims: vec4<u32>;
@group(0) @binding(5) var<storage, read> wt_frost: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> out_xyz: array<f32>;

fn wt_dot(a: vec3<f32>, b: vec3<f32>) -> f32 {
    return fma(a.x, b.x, fma(a.y, b.y, a.z * b.z));
}
fn wt_scale_add(v: vec3<f32>, scale: f32, add: vec3<f32>) -> vec3<f32> {
    return fma(v, vec3<f32>(scale), add);
}
fn wt_reflect(dir: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    return wt_scale_add(normal, -2.0 * wt_dot(dir, normal), dir);
}
struct WtRefracted { dir: vec3<f32>, valid: bool }
fn wt_refract(dir: vec3<f32>, normal: vec3<f32>, eta: f32) -> WtRefracted {
    let ci = clamp(-wt_dot(dir, normal), 0.0, 1.0);
    let sin2 = max(fma(-ci, ci, 1.0), 0.0);
    let k = fma(-(eta * eta), sin2, 1.0);
    if (k < 0.0) { return WtRefracted(vec3<f32>(0.0), false); }
    return WtRefracted(wt_scale_add(normal, fma(eta, ci, -sqrt(k)), dir * eta), true);
}
fn wt_fresnel(ci_in: f32, eta: f32) -> f32 {
    if (eta == 1.0) { return 0.0; }
    let ci = clamp(abs(ci_in), 0.0, 1.0);
    let sin2 = max(fma(-ci, ci, 1.0), 0.0);
    let k = fma(-(eta * eta), sin2, 1.0);
    if (k <= 0.0) { return 1.0; }
    let ct = sqrt(k);
    let eci = eta * ci;
    let ect = eta * ct;
    let rs = (eci - ct) / (eci + ct);
    let rp = (ci - ect) / (ci + ect);
    return clamp(0.5 * fma(rs, rs, rp * rp), 0.0, 1.0);
}
fn wt_alpha(dir: vec3<f32>) -> vec3<f32> {
    if (wt.modes.x == 0u) { return wt.alpha_o.xyz; }
    let cosine = dot(dir, wt.axis.xyz);
    return (wt.alpha_o.xyz + wt.alpha_e.xyz + (wt.alpha_o.xyz - wt.alpha_e.xyz) * (cosine * cosine)) * 0.5;
}
fn wt_max_weight(weight: vec3<f32>) -> f32 { return max(max(weight.x, weight.y), weight.z); }
fn wt_keeps(weight: vec3<f32>, depth: u32) -> bool {
    return depth <= wt.modes.z && wt_max_weight(weight) >= wt.values.y;
}
