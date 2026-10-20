struct WtHit { valid: bool, t: f32, normal: vec3<f32>, facet: u32, side: u32 }
fn wt_tool(k: u32) -> CtTool {
    let base = wt.counts.x + k * CT_RECORD_VECS;
    return ct_tool_from_vecs(wt_planes[base], wt_planes[base + 1u], wt_planes[base + 2u],
        wt_planes[base + 3u], wt_planes[base + 4u]);
}
fn wt_intersect(origin: vec3<f32>, dir: vec3<f32>) -> WtHit {
    var result = WtHit(false, 0.0, vec3<f32>(0.0), 0u, 0u);
    if (wt.counts.x == 0u) { return result; }
    var near: f32 = -1e30; var far: f32 = 1e30;
    var near_n = vec3<f32>(0.0); var far_n = vec3<f32>(0.0);
    var near_i: u32 = 0u; var far_i: u32 = 0u;
    for (var i: u32 = 0u; i < wt.counts.x; i = i + 1u) {
        let plane = wt_planes[i];
        // Intersection retains the existing non-fused geometry dot convention.
        let denominator = dot(plane.xyz, dir);
        let side = plane.w + dot(plane.xyz, origin);
        if (abs(denominator) > 1e-7) {
            let t = -side / denominator;
            if (denominator < 0.0) {
                if (t > near) { near = t; near_n = plane.xyz; near_i = i; }
            } else if (t < far) { far = t; far_n = plane.xyz; far_i = i; }
        } else if (side > 0.0) { return result; }
    }
    if (near > far) { return result; }
    if (wt.counts.y == 0u) {
        if (near > 1e-4) { return WtHit(true, near, near_n, near_i, 0u); }
        if (far > 1e-4) { return WtHit(true, far, far_n, far_i, 0u); }
        return result;
    }
    var state = ct_carve_begin(near, far);
    for (var scan: u32 = 0u; scan <= wt.counts.y; scan = scan + 1u) {
        if (state.state != CT_STATE_SCAN) { break; }
        var acc = ct_scan_init();
        for (var k: u32 = 0u; k < wt.counts.y; k = k + 1u) {
            let tool = wt_tool(k);
            if (ct_ray_may_hit(tool, origin, dir)) {
                acc = ct_scan_add(acc, k, ct_tool_ray_interval(tool, origin, dir), state.pos);
            }
        }
        state = ct_carve_step(state, acc, far);
    }
    if (state.state == CT_STATE_HIT_NEAR) { result = WtHit(true, state.hit_t, near_n, near_i, 0u); }
    if (state.state == CT_STATE_HIT_FAR) { result = WtHit(true, state.hit_t, far_n, far_i, 0u); }
    if (state.state == CT_STATE_HIT_TOOL) {
        let point = origin + dir * state.hit_t;
        result = WtHit(true, state.hit_t, -ct_tool_outward_normal(wt_tool(state.hit_k), point), wt.counts.x + state.hit_k, 0u);
    }
    if (result.valid) { result.side = select(2u, 1u, state.entering); }
    return result;
}
