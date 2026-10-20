// ---------------------------------------------------------------------------------
// Event Wavefront WGSL Common Definitions and Bindings.
//
// Tags, packed buffer structs, bindings (40..43), load/store helpers,
// and workgroup-shared plane cache cooperative fill.
// ---------------------------------------------------------------------------------

const TAG_DONE: u32 = 0u;
const TAG_EXTEND: u32 = 1u;
const TAG_SCATTER: u32 = 2u;
const TAG_SURFACE: u32 = 3u;
const TAG_ARM_BASE: u32 = 3u;
const TAG_SPLIT: u32 = 9u;
const TAG_TAIL: u32 = 10u;
const TAG_FINALIZE: u32 = 11u;
const TAG_NEE_HG: u32 = 12u;     // scatter event parked for ew_nee_hg (HDR environments only)

const RAY_FLAG_INSIDE_GEM: u32 = 1u;
const RAY_FLAG_IS_EXTRAORDINARY: u32 = 2u;
const RAY_FLAG_HAVE_PREV_PLANE_NORMAL: u32 = 4u;
const RAY_FLAG_PATH_ESCAPED: u32 = 8u;

struct EwParams {            // 32 bytes
    chunk_rays: u32,
    bounce: u32,             // round index; equals the megakernel's loop variable
    finalize_all: u32,       // 1 only on the final ew_finalize dispatch
    has_scatter: u32,
    pad0: u32, pad1: u32, pad2: u32, pad3: u32,
}

struct EwRayCore {           // 128 bytes
    origin: vec4<f32>,               // w unused (0)
    dir: vec4<f32>,                  // w unused (0)
    k: vec4<f32>,                    // w unused (0)
    prev_plane_normal: vec4<f32>,    // w unused (0)
    pending_mis_dir: vec4<f32>,      // w = pending_light_mis
    nee_xyz: vec4<f32>,              // w unused (0)
    observer: vec4<f32>,             // w unused (0)
    ray_meta: vec4<u32>,             // x flags, y seed0, z event tag, w tuple index (pixel*spp+sample)
}

struct EwRayChan {           // 288 bytes
    stokes: array<vec4<f32>, 8>,
    radiance: array<f32, 8>,
    path_pdf: array<f32, 8>,
    split_radiance: array<f32, 8>,
    lambdas: array<f32, 8>,
    compat: array<u32, 8>,
}

struct EwHit {               // 240 bytes
    hit_point: vec4<f32>,            // w = hit.t
    hit_normal: vec4<f32>,           // w unused (0)
    shading_normal: vec4<f32>,
    wave_dir_at_bounce: vec4<f32>,
    phase_dir_this_check: vec4<f32>, // w = phase_pdf_this_check
    misc: vec4<u32>,                 // x hit.hit (0/1), y pre_bounce_inside_gem, z finish, w arm
    split: vec4<u32>,                // x split_mask, y bitcast<u32>(split_mis_weight), z hit.facet_idx, w 0
    split_dir: array<vec4<f32>, 8>,
}

@group(0) @binding(40) var<uniform> ew: EwParams;
@group(0) @binding(41) var<storage, read_write> ray_core: array<EwRayCore>;
@group(0) @binding(42) var<storage, read_write> ray_chan: array<EwRayChan>;
@group(0) @binding(43) var<storage, read_write> ray_hit: array<EwHit>;

fn ew_load(i: u32) -> BounceRay {
    var s: BounceRay;
    let core = ray_core[i];
    s.origin = core.origin.xyz;
    s.dir = core.dir.xyz;
    s.k = core.k.xyz;
    s.prev_plane_normal = core.prev_plane_normal.xyz;
    s.pending_light_mis_dir = core.pending_mis_dir.xyz;
    s.pending_light_mis = core.pending_mis_dir.w;
    s.nee_xyz = core.nee_xyz.xyz;
    let flags = core.ray_meta.x;
    s.inside_gem = (flags & RAY_FLAG_INSIDE_GEM) != 0u;
    s.is_extraordinary = (flags & RAY_FLAG_IS_EXTRAORDINARY) != 0u;
    s.have_prev_plane_normal = (flags & RAY_FLAG_HAVE_PREV_PLANE_NORMAL) != 0u;
    s.path_escaped = (flags & RAY_FLAG_PATH_ESCAPED) != 0u;

    let chan = ray_chan[i];
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        s.lambdas[k] = chan.lambdas[k];
        s.stokes[k] = chan.stokes[k];
        s.radiance[k] = chan.radiance[k];
        s.path_pdf[k] = chan.path_pdf[k];
        s.split_radiance[k] = chan.split_radiance[k];
        s.compat[k] = chan.compat[k];
    }
    return s;
}

fn ew_store(i: u32, s: BounceRay) {
    var flags: u32 = 0u;
    if (s.inside_gem) { flags = flags | RAY_FLAG_INSIDE_GEM; }
    if (s.is_extraordinary) { flags = flags | RAY_FLAG_IS_EXTRAORDINARY; }
    if (s.have_prev_plane_normal) { flags = flags | RAY_FLAG_HAVE_PREV_PLANE_NORMAL; }
    if (s.path_escaped) { flags = flags | RAY_FLAG_PATH_ESCAPED; }

    ray_core[i].origin = vec4<f32>(s.origin, 0.0);
    ray_core[i].dir = vec4<f32>(s.dir, 0.0);
    ray_core[i].k = vec4<f32>(s.k, 0.0);
    ray_core[i].prev_plane_normal = vec4<f32>(s.prev_plane_normal, 0.0);
    ray_core[i].pending_mis_dir = vec4<f32>(s.pending_light_mis_dir, s.pending_light_mis);
    ray_core[i].nee_xyz = vec4<f32>(s.nee_xyz, 0.0);
    ray_core[i].ray_meta.x = flags;

    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        ray_chan[i].lambdas[k] = s.lambdas[k];
        ray_chan[i].stokes[k] = s.stokes[k];
        ray_chan[i].radiance[k] = s.radiance[k];
        ray_chan[i].path_pdf[k] = s.path_pdf[k];
        ray_chan[i].split_radiance[k] = s.split_radiance[k];
        ray_chan[i].compat[k] = s.compat[k];
    }
}

fn ew_hit_load(i: u32) -> BounceHit {
    var h = bounce_hit_blank();
    let raw = ray_hit[i];
    h.hit.hit = (raw.misc.x != 0u);
    h.hit.t = raw.hit_point.w;
    h.hit.normal = raw.hit_normal.xyz;
    h.hit.facet_idx = raw.split.z;

    h.hit_point = raw.hit_point.xyz;
    h.shading_normal = raw.shading_normal.xyz;
    h.wave_dir_at_bounce = raw.wave_dir_at_bounce.xyz;
    h.phase_dir_this_check = raw.phase_dir_this_check.xyz;
    h.phase_pdf_this_check = raw.phase_dir_this_check.w;

    h.pre_bounce_inside_gem = (raw.misc.y != 0u);
    h.finish = raw.misc.z;
    h.arm = raw.misc.w;

    h.split_mask = raw.split.x;
    h.split_mis_weight = bitcast<f32>(raw.split.y);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        h.split_dir[k] = raw.split_dir[k];
    }
    return h;
}

fn ew_hit_store(i: u32, h: BounceHit) {
    ray_hit[i].hit_point = vec4<f32>(h.hit_point, h.hit.t);
    ray_hit[i].hit_normal = vec4<f32>(h.hit.normal, 0.0);
    ray_hit[i].shading_normal = vec4<f32>(h.shading_normal, 0.0);
    ray_hit[i].wave_dir_at_bounce = vec4<f32>(h.wave_dir_at_bounce, 0.0);
    ray_hit[i].phase_dir_this_check = vec4<f32>(h.phase_dir_this_check, h.phase_pdf_this_check);
    ray_hit[i].misc = vec4<u32>(
        select(0u, 1u, h.hit.hit),
        select(0u, 1u, h.pre_bounce_inside_gem),
        h.finish,
        h.arm,
    );
    ray_hit[i].split = vec4<u32>(
        h.split_mask,
        bitcast<u32>(h.split_mis_weight),
        h.hit.facet_idx,
        0u,
    );
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        ray_hit[i].split_dir[k] = h.split_dir[k];
    }
}

fn ew_fill_planes_shared(local_index: u32) {
    // The real planes only: with concave tools the `planes` binding also holds
    // `params.num_tools` packed tool records after them (the host packs them with
    // `geometry::tool::pack_planes_and_tools`), which `scene_plane_count` leaves out. The
    // tool records are read straight from storage by `scene_tool`, never via this cache.
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
}
