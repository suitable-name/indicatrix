// Phase 1: `intersect_polyhedron` kernel -- driven by `renderer::gpu::polyhedron_check`.
//
// Fresh translation of `optics::raytracer::intersect_polyhedron` as it exists in
// `raytracer.rs` today. The exit branch (an origin-inside-the-solid ray reports the FAR
// facet, not a miss) is the critical part this port must get right -- the quarantined
// old shader had only an entry test, so an interior ray (e.g. after a refraction, in a
// future phase) would report a miss and escape to the environment. Both branches are
// ported here, in the same order, with the same `1e-4`/`1e-7` epsilons as the CPU.

struct FacetPlane {
    normal: vec3<f32>,
    d: f32,
}

struct GpuRay {
    origin: vec3<f32>,
    dir: vec3<f32>,
}

struct GpuHitRecord {
    t: f32,
    facet_idx: i32,
    hit: u32,
    normal: vec3<f32>,
}

@group(0) @binding(0) var<storage, read> planes: array<FacetPlane>;
@group(0) @binding(1) var<storage, read> rays: array<GpuRay>;
@group(0) @binding(2) var<storage, read_write> out_hits: array<GpuHitRecord>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&rays)) {
        return;
    }
    let ray = rays[idx];

    var t_near: f32 = -1e30;
    var t_far: f32 = 1e30;
    var near_facet: i32 = -1;
    var near_normal: vec3<f32> = vec3<f32>(0.0, 0.0, 0.0);
    var far_facet: i32 = -1;
    var far_normal: vec3<f32> = vec3<f32>(0.0, 0.0, 0.0);

    var result: GpuHitRecord;
    let num_planes = arrayLength(&planes);
    for (var i: u32 = 0u; i < num_planes; i = i + 1u) {
        let p = planes[i];
        let n = p.normal;
        let denom = dot(n, ray.dir);
        let side = p.d + dot(n, ray.origin);
        let numer = -side;

        if (abs(denom) > 1e-7) {
            let t = numer / denom;
            if (denom < 0.0) {
                if (t > t_near) {
                    t_near = t;
                    near_facet = i32(i);
                    near_normal = n;
                }
            } else if (t < t_far) {
                t_far = t;
                far_facet = i32(i);
                far_normal = n;
            }
        } else if (side > 0.0) {
            // Fix 3: ray (near-)parallel to this plane, origin already outside its
            // half-space -- the polyhedron intersection is empty for this ray. See
            // optics::raytracer::intersect_polyhedron's matching comment.
            result.hit = 0u;
            result.t = 0.0;
            result.facet_idx = -1;
            result.normal = vec3<f32>(0.0, 0.0, 0.0);
            out_hits[idx] = result;
            return;
        }
    }

    if (t_near > t_far) {
        // Ray direction is entirely outside the solid's half-space intersection.
        result.hit = 0u;
        result.t = 0.0;
        result.facet_idx = -1;
        result.normal = vec3<f32>(0.0, 0.0, 0.0);
    } else if (t_near > 1e-4) {
        // Origin is outside the solid: the ray enters through the near (entry) plane.
        result.hit = 1u;
        result.t = t_near;
        result.facet_idx = select(0, near_facet, near_facet >= 0);
        result.normal = near_normal;
    } else if (t_far > 1e-4) {
        // Origin is inside the solid (every entry plane lies behind the ray): the ray
        // exits through the far (exit) plane. This is the critical branch: without it,
        // any ray currently inside the gem would never find its exit facet and this
        // kernel would incorrectly report a miss.
        result.hit = 1u;
        result.t = t_far;
        result.facet_idx = select(0, far_facet, far_facet >= 0);
        result.normal = far_normal;
    } else {
        result.hit = 0u;
        result.t = 0.0;
        result.facet_idx = -1;
        result.normal = vec3<f32>(0.0, 0.0, 0.0);
    }

    out_hits[idx] = result;
}
