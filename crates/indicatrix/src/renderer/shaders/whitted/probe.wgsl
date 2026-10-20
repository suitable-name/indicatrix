// Adapter tests append this to the exact production shader; no copied physics.
@compute @workgroup_size(64)
fn whitted_probe(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= wt.range.y) { return; }
    let a = wt_planes[id.x * 2u];
    let b = wt_planes[id.x * 2u + 1u];
    let r = wt_refract(a.xyz, b.xyz, b.w);
    let base = id.x * 8u;
    out_xyz[base] = wt_fresnel(a.w, b.w);
    out_xyz[base + 1u] = exp_poly(a.w);
    out_xyz[base + 2u] = r.dir.x;
    out_xyz[base + 3u] = r.dir.y;
    out_xyz[base + 4u] = r.dir.z;
    out_xyz[base + 5u] = select(0.0, 1.0, r.valid);
    out_xyz[base + 6u] = wt_studio_g(a.xyz, vec3<f32>(0.0, 0.0, 1.0));
    out_xyz[base + 7u] = 0.0;
}
