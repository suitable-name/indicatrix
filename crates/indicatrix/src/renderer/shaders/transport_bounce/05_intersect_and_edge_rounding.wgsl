// optics::raytracer::intersect_polyhedron

struct HitInfo {
    hit: bool,
    t: f32,
    normal: vec3<f32>,
    // optics::raytracer::HitRecord::facet_idx -- lets the frosted-finish lookup below
    // index `facet_finishes`. Mirrors `near_facet`/`far_facet`'s `.unwrap_or(0)`
    // fallback: `0u` whenever a hit is never reported either.
    facet_idx: u32,
}

fn intersect_ray(origin: vec3<f32>, dir: vec3<f32>) -> HitInfo {
    var t_near: f32 = -1e30;
    var t_far: f32 = 1e30;
    var near_normal = vec3<f32>(0.0, 0.0, 0.0);
    var far_normal = vec3<f32>(0.0, 0.0, 0.0);
    var near_idx: u32 = 0u;
    var far_idx: u32 = 0u;
    var result: HitInfo;
    let num_planes = arrayLength(&planes);
    for (var i: u32 = 0u; i < num_planes; i = i + 1u) {
        let p = plane_at(i, num_planes);
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
            // Ray (near-)parallel to this plane, origin already outside its half-space
            // -- the polyhedron intersection is empty for this ray.
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

// Exit-event spectral splitting: optics::raytracer::refraction::try_split_exit_channel
// -- the one bounded fan-out primitive every exit-event mismatch site below calls.
// Reads the `planes` binding (via `intersect_ray`) and `params`/`material` (via
// `sample_environment_with_rig`), so -- like `intersect_ray`/`shading_normal_near_edge`
// above -- this stays megakernel-local rather than living in the shared
// `transport_physics.wgsl` prelude. `split_radiance` is threaded as a pointer to the
// caller's own local array, like `stokes`/`path_pdf`.

fn try_split_exit_channel(
    split_radiance: ptr<function, array<f32, 8>>,
    hit_point: vec3<f32>,
    k: u32,
    lambda_k: f32,
    dir_k: vec3<f32>,
    transmitted_intensity: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    observer: vec3<f32>,
) {
    let probe = intersect_ray(hit_point + dir_k * 1e-4, dir_k);
    if (probe.hit) {
        // Bounded re-entry: decline to trace further (a pure energy-loss truncation).
        return;
    }
    let env_spectral = sample_environment_with_rig(dir_k, lambda_k, key_dir, fill_dir, sin_lp, observer);
    (*split_radiance)[k] = fma(max(transmitted_intensity, 0.0), env_spectral, (*split_radiance)[k]);
}

// optics::raytracer::shading_normal_near_edge -- reads the `planes` storage binding
// directly (like `intersect_ray` above), so stays megakernel-local rather than living
// in the shared `transport_physics.wgsl` prelude. `shaders/shading_normal.wgsl` has its
// own standalone copy with its own `planes` binding for
// `renderer::gpu::transport_check::run_shading_normal_near_edge`'s Tier 2 self-test;
// both are unmodified line-for-line translations of the same CPU function.

fn shading_normal_near_edge(hit_point: vec3<f32>, hit_facet_idx: u32, hit_normal: vec3<f32>, rounding_radius: f32) -> vec3<f32> {
    if (rounding_radius <= 0.0) {
        return hit_normal;
    }
    var nearest_dist: f32 = 1e30;
    var nearest_normal = hit_normal;
    let num_planes = arrayLength(&planes);
    for (var i: u32 = 0u; i < num_planes; i = i + 1u) {
        if (i == hit_facet_idx) {
            continue;
        }
        let p = planes[i];
        let dist = -(p.d + dot(p.normal, hit_point));
        if (dist < nearest_dist) {
            nearest_dist = dist;
            nearest_normal = p.normal;
        }
    }
    if (nearest_dist >= rounding_radius) {
        return hit_normal;
    }
    let t = clamp(1.0 - nearest_dist / rounding_radius, 0.0, 1.0);
    let smooth_t = t * t * fma(-2.0, t, 3.0);
    let bisector = normalize_or_zero(hit_normal + nearest_normal);
    return normalize_or_zero(hit_normal * (1.0 - smooth_t) + bisector * smooth_t);
}

