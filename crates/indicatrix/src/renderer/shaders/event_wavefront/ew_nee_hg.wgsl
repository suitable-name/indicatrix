// Henyey-Greenstein NEE at a parked scatter event (see `ew_scatter.wgsl`), then the commit.
// Binds no `ray_hit`, which keeps it at 8 storage buffers.
@compute @workgroup_size(64)
fn ew_nee_hg(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    ew_fill_planes_shared(local_index);
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }
    let core = ray_core[i];
    if (core.ray_meta.z != TAG_NEE_HG) {
        return;
    }

    var s = ew_load(i);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let seed0 = core.ray_meta.y;

    let ev = BounceScatterEvent(1u, s.prev_plane_normal, s.pending_light_mis_dir);
    var alphas: array<f32, 8>;
    bounce_scatter_alphas(&s, rc, &alphas);
    bounce_scatter_nee(&s, rc, ew.bounce, seed0, ev, alphas);
    let r = bounce_scatter_commit(&s, ew.bounce, seed0, ev);
    ew_store(i, s);
    ray_core[i].ray_meta.z = select(TAG_EXTEND, TAG_FINALIZE, r == PHASE_TERMINATE);
}
