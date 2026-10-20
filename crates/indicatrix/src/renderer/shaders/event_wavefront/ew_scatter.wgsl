// Interior scatter decision. With an HDR environment the scatter-point NEE needs five more storage
// buffers than this kernel may bind (plan 4.3: at most 8), so the event is parked on the ray and
// `ew_nee_hg` finishes it: NEE first, then the same commit (new direction + roulette) the
// megakernel runs. Parked state: `prev_plane_normal` holds the scatter point and
// `pending_mis_dir.xyz` the sampled outgoing direction (both are dead until the commit rewrites
// them: `have_prev_plane_normal` is cleared there, and the MIS carry is set by the NEE).
@compute @workgroup_size(64)
fn ew_scatter(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    ew_fill_planes_shared(local_index);
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }
    let core = ray_core[i];
    if (core.ray_meta.z != TAG_SCATTER) {
        return;
    }

    var s = ew_load(i);
    var h = ew_hit_load(i);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let seed0 = core.ray_meta.y;

    var tag: u32 = TAG_SURFACE;
    if (bounce_scatter_applies(&s)) {
        var alphas: array<f32, 8>;
        bounce_scatter_alphas(&s, rc, &alphas);
        let ev = bounce_scatter_sample(&s, &h, ew.bounce, seed0, alphas);
        if (ev.scattered != 0u) {
            if (params.env_mode == 2u) {
                s.prev_plane_normal = ev.scatter_point;
                s.pending_light_mis_dir = ev.new_dir;
                tag = TAG_NEE_HG;
            } else {
                let r = bounce_scatter_commit(&s, ew.bounce, seed0, ev);
                tag = select(TAG_EXTEND, TAG_FINALIZE, r == PHASE_TERMINATE);
            }
        }
    }
    ew_store(i, s);
    ew_hit_store(i, h);
    ray_core[i].ray_meta.z = tag;
}
