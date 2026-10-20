@compute @workgroup_size(64)
fn ew_extend(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    ew_fill_planes_shared(local_index);
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }
    let core = ray_core[i];
    if (core.ray_meta.z != TAG_EXTEND) {
        return;
    }

    var s = ew_load(i);
    var h = bounce_hit_blank();
    let rc = transport_hoist_ray_constants(s.lambdas);
    let seed0 = core.ray_meta.y;
    let observer = core.observer.xyz;

    let r = bounce_extend(&s, &h, rc, ew.bounce, seed0, observer);
    ew_store(i, s);
    ew_hit_store(i, h);

    if (r == PHASE_TERMINATE) {
        ray_core[i].ray_meta.z = TAG_FINALIZE;
    } else if (ew.has_scatter != 0u) {
        ray_core[i].ray_meta.z = TAG_SCATTER;
    } else {
        ray_core[i].ray_meta.z = TAG_SURFACE;
    }
}
