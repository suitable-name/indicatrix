// Extend phase: MIS carry, intersect_ray, miss branch including environment sampling.
fn bounce_extend(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    let phase_pdf_this_check = (*s).pending_light_mis;
    let phase_dir_this_check = (*s).pending_light_mis_dir;
    (*h).phase_pdf_this_check = phase_pdf_this_check;
    (*h).phase_dir_this_check = phase_dir_this_check;
    (*s).pending_light_mis = 0.0;

    let hit = intersect_ray((*s).origin, (*s).dir);
    (*h).hit = hit;
    // Concave stones only (`side` is `HIT_SIDE_NONE` for a stone without tools, so the
    // planar path is untouched): the stretch of ray that ends at this hit is in the
    // material exactly when the hit is an exit. The toggle in the arms agrees with that
    // except when the `1e-4` restart stepped over a second surface within `1e-4` of the one
    // it crossed (a tool's rim on a facet, the crease of two tools): the carve skipped that
    // boundary as the ray's own origin, so the ray is in the other medium and the
    // intersection's own state is the one to trace by. Twin of `run_bounce`'s
    // `self.inside_gem = !entering`; it runs here, before the scatter and surface phases
    // read `inside_gem`, in the megakernel, the old wavefront and `ew_extend` alike (the
    // event wavefront stores the adopted flag with the ray).
    if (hit.hit && hit.side != HIT_SIDE_NONE) {
        (*s).inside_gem = hit.side == HIT_SIDE_EXIT;
    }
    if (!hit.hit) {
        // A primary miss (bounce 0) and an escape at any later bounce take the same
        // environment lookup below, so the backdrop tray (`params.backdrop`, read by the
        // studio arms) looks identical around the stone and through its windows.
        var mis_weight: f32 = 1.0;
        if (phase_pdf_this_check > 0.0 && nee_light_available()) {
            let light_pdf = nee_light_pdf(phase_dir_this_check, rc.studio_key_dir);
            mis_weight = balance_heuristic(phase_pdf_this_check, light_pdf);
        }
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            let env = sample_environment_with_rig((*s).dir, (*s).lambdas[k], rc.studio_key_dir, rc.studio_fill_dir, rc.studio_sin_lp, observer);
            // `max((*stokes)[k].x, 0.0)` clamps `I` to >= 0 before the environment
            // lookup, matching `accumulate_miss_radiance`'s `StokesVector::intensity`
            // on the CPU side -- negative `I` is unphysical on either side.
            (*s).radiance[k] = fma(max((*s).stokes[k].x, 0.0) * mis_weight, env, (*s).radiance[k]);
        }
        // The only site that sets this: staged exit-split contributions commit
        // only when the shared/hero path itself reaches this environment lookup.
        (*s).path_escaped = true;
        return PHASE_TERMINATE;
    }
    return PHASE_NEXT;
}

// Scatter phase, composed of four shared pieces so that the megakernel, the old wavefront and the
// event wavefront run the SAME physics. The event wavefront runs the pieces across two kernels
// when the HG NEE needs the HDR buffers (`ew_scatter` then `ew_nee_hg`), keeping this order:
//   alphas -> sample (Beer-Lambert + scatter decision) -> HG NEE -> commit (new direction + RR).

// One interior scattering decision: whether the free flight ended in a scatter, where, and the
// phase-sampled outgoing direction.
struct BounceScatterEvent {
    scattered: u32,
    scatter_point: vec3<f32>,
    new_dir: vec3<f32>,
}

// True when the ray is inside a scattering medium and so takes the scatter phase at all.
fn bounce_scatter_applies(s: ptr<function, BounceRay>) -> bool {
    return (*s).inside_gem && material.scattering_sigma_s > 0.0;
}

// The assigned eigenmode's absorption coefficient of channel `k` at wave normal `k_hat`, for
// an anisotropic material (`rc.is_anisotropic`). `bounce_scatter_alphas` evaluates it at the
// ray's own direction for the segment it travels; `nee_contribution_hg_scatter` evaluates it
// at the SHADOW direction, which is what a phase-sampled continuation in that direction
// pays (`optics::raytracer::scattering::hg_nee::ShadowAlphas::AssignedMode`).
fn scatter_alpha_anisotropic(rc: RayConstants, k: u32, k_hat: vec3<f32>, is_extraordinary: bool) -> f32 {
    if (rc.is_biaxial && material.has_beta_ray != 0u) {
        return assigned_mode_alpha_biaxial(
            rc.alpha_o_hoisted[k], rc.alpha_beta_hoisted[k], rc.alpha_e_hoisted[k],
            rc.n_alpha_hero, rc.n_beta_hero, rc.n_gamma_hero, rc.biax_ax0, rc.biax_ax1, rc.biax_ax2,
            rc.c_axis, k_hat, is_extraordinary,
        );
    }
    return assigned_mode_alpha_uniaxial(
        rc.alpha_o_hoisted[k], rc.alpha_e_hoisted[k], rc.c_axis, k_hat, is_extraordinary,
        rc.n_o_hero_seed, rc.n_e_hero_seed,
    );
}

// The per-channel absorption alphas for this ray's current direction and mode.
fn bounce_scatter_alphas(
    s: ptr<function, BounceRay>,
    rc: RayConstants,
    alphas: ptr<function, array<f32, 8>>,
) {
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        (*alphas)[k] = 0.0;
    }
    if (rc.is_anisotropic) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            (*alphas)[k] = scatter_alpha_anisotropic(rc, k, (*s).k, (*s).is_extraordinary);
        }
    } else {
        let eigen_a = ordinary_eigen_polarization((*s).k, rc.c_axis);
        let eigen_b = extraordinary_eigen_polarization((*s).k, rc.c_axis);
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            (*alphas)[k] = isotropic_channel_alpha(
                rc.alpha_o_hoisted[k], rc.alpha_e_hoisted[k], rc.c_axis, eigen_a, eigen_b,
            );
        }
    }
}

// Beer-Lambert survival and the scatter decision. Updates stokes and path_pdf; leaves origin,
// dir and k untouched (the commit piece moves them).
fn bounce_scatter_sample(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    bounce: u32,
    seed0: u32,
    alphas: array<f32, 8>,
) -> BounceScatterEvent {
    var local_stokes = (*s).stokes;
    var local_path_pdf = (*s).path_pdf;
    let sc = maybe_scatter_or_extinguish(
        alphas, material.scattering_sigma_s, material.scattering_g, (*s).dir, (*h).hit.t,
        material.absorption_path_scale, seed0, bounce, &local_stokes, &local_path_pdf,
    );
    (*s).stokes = local_stokes;
    (*s).path_pdf = local_path_pdf;
    return BounceScatterEvent(sc.scattered, (*s).origin + sc.t_free * (*s).dir, sc.new_dir);
}

// Henyey-Greenstein NEE at the scatter point and the MIS carry for the phase-sampled
// continuation. HDR environments only. Runs BEFORE the commit, on the pre-roulette stokes.
// `alphas` are this segment's own (evaluated at `(*s).k`, the direction the ray ARRIVED
// along); the shadow ray uses them as they are only for an isotropic material, and
// re-evaluates an anisotropic material's at its own direction (see
// `nee_contribution_hg_scatter`).
fn bounce_scatter_nee(
    s: ptr<function, BounceRay>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    ev: BounceScatterEvent,
    alphas: array<f32, 8>,
) {
    let old_dir = (*s).dir;
    var hg_lambdas = (*s).lambdas;
    var hg_stokes = (*s).stokes;
    var hg_path_pdf = (*s).path_pdf;
    var hg_compat = (*s).compat;
    var hg_nee = (*s).nee_xyz;
    nee_contribution_hg_scatter(
        &hg_lambdas, rc.n_o_hero_seed, ev.scatter_point, old_dir,
        material.scattering_g, seed0, bounce, &hg_stokes, &hg_path_pdf, &hg_compat,
        &hg_nee, alphas, rc, (*s).is_extraordinary,
    );
    (*s).stokes = hg_stokes;
    (*s).path_pdf = hg_path_pdf;
    (*s).compat = hg_compat;
    (*s).nee_xyz = hg_nee;
    (*s).pending_light_mis = henyey_greenstein_phase(dot(ev.new_dir, old_dir), material.scattering_g);
    (*s).pending_light_mis_dir = ev.new_dir;
}

// Moves the ray to the scatter point along the sampled direction, then Russian roulette.
fn bounce_scatter_commit(
    s: ptr<function, BounceRay>,
    bounce: u32,
    seed0: u32,
    ev: BounceScatterEvent,
) -> u32 {
    (*s).origin = ev.scatter_point;
    (*s).dir = ev.new_dir;
    (*s).k = ev.new_dir;
    (*s).have_prev_plane_normal = false;

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
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            (*s).stokes[k] = (*s).stokes[k] * (1.0 / q);
            (*s).split_radiance[k] = (*s).split_radiance[k] / q;
        }
    }
    return PHASE_END_STEP;
}

// Scatter phase: interior scatter (HG NEE + RR).
fn bounce_scatter(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    if (bounce_scatter_applies(s)) {
        var alphas: array<f32, 8>;
        bounce_scatter_alphas(s, rc, &alphas);
        let ev = bounce_scatter_sample(s, h, bounce, seed0, alphas);
        if (ev.scattered != 0u) {
            if (params.env_mode == 2u) {
                bounce_scatter_nee(s, rc, bounce, seed0, ev, alphas);
            }
            return bounce_scatter_commit(s, bounce, seed0, ev);
        }
    }
    return PHASE_NEXT;
}

