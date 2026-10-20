@compute @workgroup_size(64)
fn ew_tail(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }
    let core = ray_core[i];
    if (core.ray_meta.z != TAG_TAIL) {
        return;
    }

    var s = ew_load(i);
    var h = ew_hit_load(i);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let seed0 = core.ray_meta.y;

    let r = bounce_tail(&s, &h, rc, ew.bounce, seed0);
    ew_store(i, s);
    ew_hit_store(i, h);

    if (r == PHASE_TERMINATE) {
        ray_core[i].ray_meta.z = TAG_FINALIZE;
    } else {
        ray_core[i].ray_meta.z = TAG_EXTEND;
    }
}
