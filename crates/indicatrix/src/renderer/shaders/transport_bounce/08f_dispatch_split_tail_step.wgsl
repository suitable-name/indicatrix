// Arm dispatcher based on h.arm selected in surface phase.
fn bounce_run_arm(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    if ((*h).arm == ARM_FROSTED) {
        return bounce_arm_frosted(s, h, rc, bounce, seed0, observer);
    } else if ((*h).arm == ARM_TIR) {
        return bounce_arm_tir(s, h, rc, bounce, seed0, observer);
    } else if ((*h).arm == ARM_UNI_ENTRY) {
        return bounce_arm_uni_entry(s, h, rc, bounce, seed0, observer);
    } else if ((*h).arm == ARM_UNI_INTERNAL) {
        return bounce_arm_uni_internal(s, h, rc, bounce, seed0, observer);
    } else if ((*h).arm == ARM_GENERIC) {
        return bounce_arm_generic(s, h, rc, bounce, seed0, observer);
    }
    return PHASE_NEXT;
}

// Resolve deferred exit splits.
fn bounce_resolve_splits(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    observer: vec3<f32>,
) -> u32 {
    let mask = (*h).split_mask;
    if (mask != 0u) {
        var local_split_radiance = (*s).split_radiance;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            if ((mask & (1u << k)) != 0u) {
                try_split_exit_channel(
                    &local_split_radiance,
                    (*h).hit_point,
                    k,
                    (*s).lambdas[k],
                    (*h).split_dir[k].xyz,
                    (*h).split_dir[k].w,
                    rc.studio_key_dir,
                    rc.studio_fill_dir,
                    rc.studio_sin_lp,
                    observer,
                    (*h).split_mis_weight,
                );
            }
        }
        (*s).split_radiance = local_split_radiance;
    }
    return PHASE_NEXT;
}

// Tail phase: surface glare and Russian roulette.
fn bounce_tail(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
) -> u32 {
    // Surface glare -- mirrors `trace_spectral_ray_inner`'s identical block: the camera
    // path's first event at a polished facet left the ray outside the stone, so it was
    // the specular reflection and the light never entered. Skipped at 1.0 and for a
    // frosted facet, so those cases are bit-identical.
    if (bounce == 0u && !(*s).inside_gem && (*h).finish != FACET_FINISH_FROSTED && params.surface_glare < 1.0) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            (*s).stokes[k] = (*s).stokes[k] * params.surface_glare;
        }
    }

    if (bounce > 4u) {
        var max_intensity: f32 = 0.0;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            max_intensity = max(max_intensity, max((*s).stokes[k].x, 0.0));
        }
        let q = clamp(max_intensity, RR_FLOOR, 1.0);
        let rr_rand = f32(hash_u32(seed0 ^ hash_u32(bounce ^ RUSSIAN_ROULETTE_STREAM))) / 4294967295.0;
        if (rr_rand > q) {
            return PHASE_TERMINATE;
        }
        // `split_radiance` rides along on the same `1/q` survival
        // rescale as `stokes` -- see `apply_russian_roulette`'s CPU-side doc comment.
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            (*s).stokes[k] = (*s).stokes[k] * (1.0 / q);
            (*s).split_radiance[k] = (*s).split_radiance[k] / q;
        }
    }

    return PHASE_NEXT;
}

// One bounce of the shared transport bounce loop. Composition of the phase functions.
fn transport_bounce_step(
    bounce: u32,
    seed0: u32,
    lambdas: ptr<function, array<f32, 8>>,
    c_axis: vec3<f32>,
    birefringence_delta: f32,
    is_anisotropic: bool,
    is_biaxial: bool,
    biax_ax0: vec3<f32>,
    biax_ax1: vec3<f32>,
    biax_ax2: vec3<f32>,
    n_o_hero_seed: f32,
    n_beta_hero: f32,
    n_alpha_hero: f32,
    n_gamma_hero: f32,
    n_e_hero_seed: f32,
    n_o_hoisted: array<f32, 8>,
    alpha_o_hoisted: array<f32, 8>,
    alpha_e_hoisted: array<f32, 8>,
    alpha_beta_hoisted: array<f32, 8>,
    studio_key_dir: vec3<f32>,
    studio_fill_dir: vec3<f32>,
    studio_sin_lp: f32,
    observer: vec3<f32>,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
    current_origin: ptr<function, vec3<f32>>,
    current_dir: ptr<function, vec3<f32>>,
    current_k: ptr<function, vec3<f32>>,
    inside_gem: ptr<function, bool>,
    is_extraordinary: ptr<function, bool>,
    prev_plane_normal: ptr<function, vec3<f32>>,
    have_prev_plane_normal: ptr<function, bool>,
    split_radiance: ptr<function, array<f32, 8>>,
    compat: ptr<function, array<u32, 8>>,
    nee_xyz: ptr<function, vec3<f32>>,
    path_escaped: ptr<function, bool>,
    pending_light_mis: ptr<function, f32>,
    pending_light_mis_dir: ptr<function, vec3<f32>>,
) -> u32 {
    var s = bounce_ray_from_pointers(
        current_origin, current_dir, current_k, prev_plane_normal, pending_light_mis_dir, nee_xyz,
        pending_light_mis, inside_gem, is_extraordinary, have_prev_plane_normal, path_escaped,
        lambdas, stokes, radiance, path_pdf, split_radiance, compat,
    );
    var h = bounce_hit_blank();
    var rc: RayConstants;
    rc.c_axis = c_axis;
    rc.birefringence_delta = birefringence_delta;
    rc.is_anisotropic = is_anisotropic;
    rc.is_biaxial = is_biaxial;
    rc.biax_ax0 = biax_ax0;
    rc.biax_ax1 = biax_ax1;
    rc.biax_ax2 = biax_ax2;
    rc.n_o_hero_seed = n_o_hero_seed;
    rc.n_beta_hero = n_beta_hero;
    rc.n_alpha_hero = n_alpha_hero;
    rc.n_gamma_hero = n_gamma_hero;
    rc.n_e_hero_seed = n_e_hero_seed;
    rc.n_o_hoisted = n_o_hoisted;
    rc.alpha_o_hoisted = alpha_o_hoisted;
    rc.alpha_e_hoisted = alpha_e_hoisted;
    rc.alpha_beta_hoisted = alpha_beta_hoisted;
    rc.studio_key_dir = studio_key_dir;
    rc.studio_fill_dir = studio_fill_dir;
    rc.studio_sin_lp = studio_sin_lp;

    var r = bounce_extend(&s, &h, rc, bounce, seed0, observer);
    if (r == PHASE_NEXT) { r = bounce_scatter(&s, &h, rc, bounce, seed0, observer); }
    if (r == PHASE_NEXT) { r = bounce_surface(&s, &h, rc, bounce, seed0, observer); }
    if (r == PHASE_NEXT) { r = bounce_run_arm(&s, &h, rc, bounce, seed0, observer); }
    if (r == PHASE_NEXT) { r = bounce_resolve_splits(&s, &h, rc, observer); }
    if (r == PHASE_NEXT) { r = bounce_tail(&s, &h, rc, bounce, seed0); }

    bounce_ray_to_pointers(
        s, current_origin, current_dir, current_k, prev_plane_normal, pending_light_mis_dir, nee_xyz,
        pending_light_mis, inside_gem, is_extraordinary, have_prev_plane_normal, path_escaped,
        stokes, radiance, path_pdf, split_radiance, compat,
    );

    if (r == PHASE_TERMINATE) {
        return BOUNCE_STATUS_TERMINATE;
    }
    return BOUNCE_STATUS_CONTINUE;
}
