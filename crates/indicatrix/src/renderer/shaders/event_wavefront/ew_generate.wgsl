@compute @workgroup_size(64)
fn ew_generate(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }

    let gen = transport_generate_ray(i);

    var s: BounceRay;
    s.origin = gen.origin;
    s.dir = gen.dir;
    s.k = gen.dir;
    s.prev_plane_normal = vec3<f32>(0.0, 0.0, 0.0);
    s.pending_light_mis_dir = vec3<f32>(0.0, 0.0, 0.0);
    s.nee_xyz = vec3<f32>(0.0, 0.0, 0.0);
    s.pending_light_mis = 0.0;
    s.inside_gem = false;
    s.is_extraordinary = false;
    s.have_prev_plane_normal = false;
    s.path_escaped = false;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        s.lambdas[k] = gen.lambdas[k];
        s.stokes[k] = vec4<f32>(1.0, 0.0, 0.0, 0.0);
        s.radiance[k] = 0.0;
        s.path_pdf[k] = 1.0;
        s.split_radiance[k] = 0.0;
        s.compat[k] = 0xFFu;
    }

    ew_store(i, s);

    ray_core[i].observer = vec4<f32>(-gen.dir, 0.0);
    ray_core[i].ray_meta.y = gen.seed0;
    ray_core[i].ray_meta.z = TAG_EXTEND;
    ray_core[i].ray_meta.w = i;
}
