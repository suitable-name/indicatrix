// Phase 1: camera ray-generation kernel -- driven by `renderer::gpu::camera_check`.
//
// Fresh translation of `optics::raytracer::Camera::new` (pose -> screen-space basis)
// and `Camera::generate_ray` (screen coordinates + jitter -> world-space ray), run back
// to back per case exactly as the CPU renderer does per-frame/per-sample. See that
// module's own doc comment: this is a translation of the CURRENT `raytracer.rs`, never
// a repair of the quarantined old shader.
//
// Each case is fully self-contained (no shared uniform state), so a single dispatch can
// sweep a dense, independently-generated grid of `(yaw, pitch, distance, fov_deg,
// screen_x, screen_y, width, height, jitter_x, jitter_y)` tuples -- including adversarial
// ones (near-polar pitch where `Camera::new`'s `world_up` branch flips, screen corners,
// jitter at its `[-0.5, 0.5)` extremes) generated on the CPU side in
// `renderer::gpu::camera_check`.

struct CameraRayCase {
    yaw: f32,
    pitch: f32,
    distance: f32,
    fov_deg: f32,
    screen_x: f32,
    screen_y: f32,
    width: f32,
    height: f32,
    jitter_x: f32,
    jitter_y: f32,
    _pad0: f32,
    _pad1: f32,
}

struct GpuRay {
    origin: vec3<f32>,
    dir: vec3<f32>,
}

@group(0) @binding(0) var<storage, read> cases: array<CameraRayCase>;
@group(0) @binding(1) var<storage, read_write> out_rays: array<GpuRay>;

const PI: f32 = 3.14159265358979323846;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&cases)) {
        return;
    }
    let c = cases[idx];

    // Camera::new
    let cos_p = cos(c.pitch);
    let sin_p = sin(c.pitch);
    let cos_y = cos(c.yaw);
    let sin_y = sin(c.yaw);

    let origin = vec3<f32>(c.distance * cos_p * sin_y, c.distance * sin_p, c.distance * cos_p * cos_y);
    let forward = normalize(-origin);
    var world_up: vec3<f32>;
    if (abs(cos_p) < 1e-4) {
        world_up = vec3<f32>(0.0, 0.0, -1.0);
    } else {
        world_up = vec3<f32>(0.0, 1.0, 0.0);
    }
    let right = normalize(cross(forward, world_up));
    let up = normalize(cross(right, forward));
    let fov_tan = tan((c.fov_deg * PI / 180.0) * 0.5);

    // Camera::generate_ray
    let aspect = c.width / c.height;
    let u = ((c.screen_x + c.jitter_x) / c.width - 0.5) * 2.0 * aspect * fov_tan;
    let v = (0.5 - (c.screen_y + c.jitter_y) / c.height) * 2.0 * fov_tan;
    let dir = normalize(forward + right * u + up * v);

    out_rays[idx].origin = origin;
    out_rays[idx].dir = dir;
}
