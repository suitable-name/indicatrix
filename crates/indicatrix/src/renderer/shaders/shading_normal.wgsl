// Physics review, Task 2 (facet edge rounding): `shading_normal_near_edge` Tier 2
// kernel -- driven by `renderer::gpu::shading_normal_check`. A standalone file with its
// own `planes` binding (bound to the SAME real 57-facet Standard Round Brilliant plane
// set `intersect_polyhedron.wgsl`/`polyhedron_check` use), mirroring that file's own
// pattern for a function whose only extra input beyond scalars/vectors is the
// polyhedron's plane list. A direct, unmodified line-for-line translation of
// `optics::raytracer::shading_normal_near_edge` -- the SAME translation
// `shaders/spectral_transport.wgsl`'s megakernel-local copy is (both read `planes` via
// their own file's binding, so neither can share a `transport_physics.wgsl` function
// for it -- see that file's own doc comment for why a binding-touching function stays
// per-file).

struct FacetPlane {
    normal: vec3<f32>,
    d: f32,
}

struct ShadingNormalCase {
    hit_point: vec3<f32>,
    hit_facet_idx: u32,
    hit_normal: vec3<f32>,
    rounding_radius: f32,
}

@group(0) @binding(0) var<storage, read> planes: array<FacetPlane>;
@group(0) @binding(1) var<storage, read> cases: array<ShadingNormalCase>;
@group(0) @binding(2) var<storage, read_write> out_normals: array<f32>;

fn normalize_or_zero(v: vec3<f32>) -> vec3<f32> {
    let l2 = dot(v, v);
    if (l2 > 1e-30) {
        return v / sqrt(l2);
    }
    return vec3<f32>(0.0, 0.0, 0.0);
}

// optics::raytracer::shading_normal_near_edge
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

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&cases)) {
        return;
    }
    let c = cases[idx];
    let n = shading_normal_near_edge(c.hit_point, c.hit_facet_idx, c.hit_normal, c.rounding_radius);
    out_normals[idx * 3u + 0u] = n.x;
    out_normals[idx * 3u + 1u] = n.y;
    out_normals[idx * 3u + 2u] = n.z;
}
