// ---------------------------------------------------------------------------------
// Phase-function refactor: BounceRay and BounceHit structs and constants.
//
// Everything one ray carries from bounce to bounce. Mirrors the pointer parameters of
// `transport_bounce_step` one-for-one; same names, same types.
// ---------------------------------------------------------------------------------

struct BounceRay {
    origin: vec3<f32>,
    dir: vec3<f32>,
    k: vec3<f32>,
    prev_plane_normal: vec3<f32>,
    pending_light_mis_dir: vec3<f32>,
    nee_xyz: vec3<f32>,
    pending_light_mis: f32,
    inside_gem: bool,
    is_extraordinary: bool,
    have_prev_plane_normal: bool,
    path_escaped: bool,
    lambdas: array<f32, 8>,
    stokes: array<vec4<f32>, 8>,
    radiance: array<f32, 8>,
    path_pdf: array<f32, 8>,
    split_radiance: array<f32, 8>,
    compat: array<u32, 8>,
}

// What one bounce step hands from one phase to the next. Lives only inside one step.
struct BounceHit {
    hit: HitInfo,                    // from intersect_ray (05_intersect...wgsl)
    hit_point: vec3<f32>,
    shading_normal: vec3<f32>,
    wave_dir_at_bounce: vec3<f32>,   // = k at the start of the surface phase
    phase_pdf_this_check: f32,       // MIS carry taken at extend phase
    phase_dir_this_check: vec3<f32>,
    pre_bounce_inside_gem: bool,
    finish: u32,                     // facet finish looked up in surface phase
    arm: u32,                        // ARM_* below, chosen by the surface phase
    split_mask: u32,                 // bit k set: channel k has a deferred exit split
    split_mis_weight: f32,
    split_dir: array<vec4<f32>, 8>,  // xyz = dir_k, w = transmitted_intensity
}

const ARM_FROSTED: u32 = 1u;
const ARM_TIR: u32 = 2u;
const ARM_UNI_ENTRY: u32 = 3u;
const ARM_UNI_INTERNAL: u32 = 4u;
const ARM_GENERIC: u32 = 5u;

// Phase results.
const PHASE_NEXT: u32 = 0u;        // go on to the next phase in this bounce
const PHASE_END_STEP: u32 = 1u;    // this bounce is over, ray alive (scatter continued)
const PHASE_TERMINATE: u32 = 2u;   // ray dead; finalize it

fn bounce_hit_blank() -> BounceHit {
    var h: BounceHit;
    h.hit.hit = false;
    h.hit.t = 0.0;
    h.hit.normal = vec3<f32>(0.0, 0.0, 0.0);
    h.hit.facet_idx = 0u;
    h.hit_point = vec3<f32>(0.0, 0.0, 0.0);
    h.shading_normal = vec3<f32>(0.0, 0.0, 0.0);
    h.wave_dir_at_bounce = vec3<f32>(0.0, 0.0, 0.0);
    h.phase_pdf_this_check = 0.0;
    h.phase_dir_this_check = vec3<f32>(0.0, 0.0, 0.0);
    h.pre_bounce_inside_gem = false;
    h.finish = 0u;
    h.arm = 0u;
    h.split_mask = 0u;
    h.split_mis_weight = 0.0;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        h.split_dir[k] = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    return h;
}

fn bounce_ray_from_pointers(
    current_origin: ptr<function, vec3<f32>>,
    current_dir: ptr<function, vec3<f32>>,
    current_k: ptr<function, vec3<f32>>,
    prev_plane_normal: ptr<function, vec3<f32>>,
    pending_light_mis_dir: ptr<function, vec3<f32>>,
    nee_xyz: ptr<function, vec3<f32>>,
    pending_light_mis: ptr<function, f32>,
    inside_gem: ptr<function, bool>,
    is_extraordinary: ptr<function, bool>,
    have_prev_plane_normal: ptr<function, bool>,
    path_escaped: ptr<function, bool>,
    lambdas: ptr<function, array<f32, 8>>,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
    split_radiance: ptr<function, array<f32, 8>>,
    compat: ptr<function, array<u32, 8>>,
) -> BounceRay {
    var s: BounceRay;
    s.origin = (*current_origin);
    s.dir = (*current_dir);
    s.k = (*current_k);
    s.prev_plane_normal = (*prev_plane_normal);
    s.pending_light_mis_dir = (*pending_light_mis_dir);
    s.nee_xyz = (*nee_xyz);
    s.pending_light_mis = (*pending_light_mis);
    s.inside_gem = (*inside_gem);
    s.is_extraordinary = (*is_extraordinary);
    s.have_prev_plane_normal = (*have_prev_plane_normal);
    s.path_escaped = (*path_escaped);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        s.lambdas[k] = (*lambdas)[k];
        s.stokes[k] = (*stokes)[k];
        s.radiance[k] = (*radiance)[k];
        s.path_pdf[k] = (*path_pdf)[k];
        s.split_radiance[k] = (*split_radiance)[k];
        s.compat[k] = (*compat)[k];
    }
    return s;
}

fn bounce_ray_to_pointers(
    s: BounceRay,
    current_origin: ptr<function, vec3<f32>>,
    current_dir: ptr<function, vec3<f32>>,
    current_k: ptr<function, vec3<f32>>,
    prev_plane_normal: ptr<function, vec3<f32>>,
    pending_light_mis_dir: ptr<function, vec3<f32>>,
    nee_xyz: ptr<function, vec3<f32>>,
    pending_light_mis: ptr<function, f32>,
    inside_gem: ptr<function, bool>,
    is_extraordinary: ptr<function, bool>,
    have_prev_plane_normal: ptr<function, bool>,
    path_escaped: ptr<function, bool>,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
    split_radiance: ptr<function, array<f32, 8>>,
    compat: ptr<function, array<u32, 8>>,
) {
    (*current_origin) = s.origin;
    (*current_dir) = s.dir;
    (*current_k) = s.k;
    (*prev_plane_normal) = s.prev_plane_normal;
    (*pending_light_mis_dir) = s.pending_light_mis_dir;
    (*nee_xyz) = s.nee_xyz;
    (*pending_light_mis) = s.pending_light_mis;
    (*inside_gem) = s.inside_gem;
    (*is_extraordinary) = s.is_extraordinary;
    (*have_prev_plane_normal) = s.have_prev_plane_normal;
    (*path_escaped) = s.path_escaped;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        (*stokes)[k] = s.stokes[k];
        (*radiance)[k] = s.radiance[k];
        (*path_pdf)[k] = s.path_pdf[k];
        (*split_radiance)[k] = s.split_radiance[k];
        (*compat)[k] = s.compat[k];
    }
}
