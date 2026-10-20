@compute @workgroup_size(64)
fn ew_arm_frosted(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    ew_fill_planes_shared(local_index);
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }
    let core = ray_core[i];
    if (core.ray_meta.z != TAG_ARM_BASE + ARM_FROSTED) {
        return;
    }

    var s = ew_load(i);
    var h = ew_hit_load(i);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let seed0 = core.ray_meta.y;
    let observer = core.observer.xyz;

    let r = bounce_arm_frosted(&s, &h, rc, ew.bounce, seed0, observer);
    ew_store(i, s);
    ew_hit_store(i, h);

    if (r == PHASE_TERMINATE) {
        ray_core[i].ray_meta.z = TAG_FINALIZE;
    } else if (h.split_mask != 0u) {
        ray_core[i].ray_meta.z = TAG_SPLIT;
    } else {
        ray_core[i].ray_meta.z = TAG_TAIL;
    }
}
