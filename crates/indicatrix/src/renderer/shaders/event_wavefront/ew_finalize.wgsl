@compute @workgroup_size(64)
fn ew_finalize(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }
    let tag = ray_core[i].ray_meta.z;
    let should_finalize = (tag == TAG_FINALIZE) || (ew.finalize_all != 0u && tag != TAG_DONE);
    if (!should_finalize) {
        return;
    }

    var s = ew_load(i);
    var local_radiance = s.radiance;
    transport_finalize_ray(
        ray_core[i].ray_meta.w,
        s.lambdas,
        &local_radiance,
        s.split_radiance,
        s.path_pdf,
        s.compat,
        s.path_escaped,
        s.nee_xyz,
    );
    s.radiance = local_radiance;

    ray_core[i].ray_meta.z = TAG_DONE;
}
