// Tier 2 probe for the megakernel's concave-tool path. NOT a standalone shader: the host
// (`renderer::gpu::tool_check`) appends it to the generated megakernel source
// (`spectral_transport.generated.wgsl`), so it calls the REAL `intersect_ray`,
// `scene_any_tool_contains` and `shading_normal_for_hit` against the real bindings
// (`params` = 1, `planes` = 3, which hold the planes followed by the packed tool records)
// instead of a hand-copied twin. It adds two bindings of its own (90, 91).
//
// One case per thread; `mode.x` selects the query:
//   0  intersect_ray(origin, dir)            -> a = (hit, t, facet_idx, side), b = (normal, 0)
//                                                   side = HIT_SIDE_* as an exact float
//   1  any tool contains `origin`            -> a = (contained, 0, 0, 0)
//   2  shading_normal_for_hit(origin = hit point, dir = hit normal, facet = mode.y,
//                             rounding radius = origin.w)        -> b = (shading normal, 0)

struct ToolProbeCase {
    origin: vec4<f32>,
    dir: vec4<f32>,
    mode: vec4<u32>,
}

struct ToolProbeOut {
    a: vec4<f32>,
    b: vec4<f32>,
}

@group(0) @binding(90) var<storage, read> tp_cases: array<ToolProbeCase>;
@group(0) @binding(91) var<storage, read_write> tp_out: array<ToolProbeOut>;

@compute @workgroup_size(64)
fn tool_probe_main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    // The same cooperative plane-cache fill `transport_main` runs, before the early return,
    // so `intersect_ray`'s `plane_at` reads what it reads in production.
    let plane_count = scene_plane_count();
    if (plane_count <= PLANES_SHARED_CAPACITY) {
        var fill_i = local_index;
        loop {
            if (fill_i >= plane_count) {
                break;
            }
            planes_shared[fill_i] = planes[fill_i];
            fill_i = fill_i + 64u;
        }
    }
    workgroupBarrier();

    let idx = gid.x;
    if (idx >= arrayLength(&tp_cases)) {
        return;
    }
    let c = tp_cases[idx];
    var a = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    var b = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    if (c.mode.x == 0u) {
        let h = intersect_ray(c.origin.xyz, c.dir.xyz);
        a = vec4<f32>(select(0.0, 1.0, h.hit), h.t, f32(h.facet_idx), f32(h.side));
        b = vec4<f32>(h.normal, 0.0);
    } else if (c.mode.x == 1u) {
        let inside = scene_any_tool_contains(plane_count, c.origin.xyz);
        a = vec4<f32>(select(0.0, 1.0, inside), 0.0, 0.0, 0.0);
    } else {
        let n = shading_normal_for_hit(c.origin.xyz, c.mode.y, c.dir.xyz, c.origin.w);
        b = vec4<f32>(n, 0.0);
    }
    tp_out[idx] = ToolProbeOut(a, b);
}
