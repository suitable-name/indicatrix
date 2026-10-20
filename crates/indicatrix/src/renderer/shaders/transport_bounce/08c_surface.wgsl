// Surface phase: shading normal, frame rotation, interior absorption, facet finish lookup, and arm selection.
fn bounce_surface(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    let hit = (*h).hit;
    var local_origin = (*s).origin;
    let current_origin = &local_origin;
    var local_dir = (*s).dir;
    let current_dir = &local_dir;
    var local_is_extraordinary = (*s).is_extraordinary;
    let is_extraordinary = &local_is_extraordinary;
    var local_stokes = (*s).stokes;
    let stokes = &local_stokes;
    var local_lambdas = (*s).lambdas;
    let lambdas = &local_lambdas;
    let is_anisotropic = rc.is_anisotropic;
    let is_biaxial = rc.is_biaxial;
    let c_axis = rc.c_axis;
    let n_alpha_hero = rc.n_alpha_hero;
    let n_beta_hero = rc.n_beta_hero;
    let n_gamma_hero = rc.n_gamma_hero;
    let biax_ax0 = rc.biax_ax0;
    let biax_ax1 = rc.biax_ax1;
    let biax_ax2 = rc.biax_ax2;
    let n_o_hero_seed = rc.n_o_hero_seed;
    let n_e_hero_seed = rc.n_e_hero_seed;
    let alpha_o_hoisted = rc.alpha_o_hoisted;
    let alpha_e_hoisted = rc.alpha_e_hoisted;
    let alpha_beta_hoisted = rc.alpha_beta_hoisted;

    let hit_point = (*current_origin) + hit.t * (*current_dir);
    // See shading_normal_near_edge's own doc comment. `shading_normal_for_hit` is that
    // function unchanged for a planar stone and the concave-stone rule otherwise.
    var normal = shading_normal_for_hit(hit_point, hit.facet_idx, hit.normal, material.edge_rounding_radius);
    // `wave_dir_at_bounce` is the wave normal `k` (== (*current_k)), used for
    // EVERY index lookup, cos_i/sin_i, Snell/Fresnel evaluation, TIR decision, and
    // the Stokes plane-of-incidence frame below -- see
    // optics::raytracer::refraction's "wave normal vs Poynting direction" design
    // note. The Poynting/energy direction `S` (== (*current_dir)) is used directly
    // (geometric origin-advance/intersection only) where still needed.
    // `wave_dir_at_bounce == (*current_dir)` trivially outside the crystal and for the
    // uniaxial ordinary eigenmode, so every such case is bit-identical to the plain
    // all-`(*current_dir)` code.
    let wave_dir_at_bounce = (*s).k;

    // Plane-of-incidence frame rotation (signed psi via atan2).
    let cpn_raw = cross(wave_dir_at_bounce, normal);
    let cpn_len2 = dot(cpn_raw, cpn_raw);
    var current_plane_normal: vec3<f32>;
    if (cpn_len2 > 0.0) {
        current_plane_normal = cpn_raw / sqrt(cpn_len2);
    } else {
        current_plane_normal = vec3<f32>(0.0, 0.0, 0.0);
    }
    if ((*s).have_prev_plane_normal && cpn_len2 > 1e-6 && dot((*s).prev_plane_normal, (*s).prev_plane_normal) > 1e-6) {
        let psi = signed_frame_rotation_psi((*s).prev_plane_normal, current_plane_normal, wave_dir_at_bounce);
        let rot = mueller_frame_rotation(psi);
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            local_stokes[k] = rot * local_stokes[k];
            (*s).stokes[k] = local_stokes[k];
        }
    }
    (*s).prev_plane_normal = current_plane_normal;
    (*s).have_prev_plane_normal = true;

    if ((*s).inside_gem) {
        normal = -normal;
        // A scattering-active material's extinction for this segment was
        // already applied above (in the `maybe_scatter_or_extinguish` no-scatter
        // branch) -- applying the plain absorption loop here too would charge
        // this segment's absorption TWICE. `material.scattering_sigma_s <= 0.0`
        // is exactly the pre-Task-1 case (including every existing scene), where
        // the block above never ran and this is the ONLY absorption
        // application, matching the CPU's identical guard.
        if (material.scattering_sigma_s <= 0.0) {
            // P1 (assigned-mode absorption): optics::raytracer::absorption::
            // channel_absorption_alphas_assigned. `(*is_extraordinary)` names which
            // eigenmode this path was assigned to at its most recent air->crystal
            // entry -- computed fresh from the CURRENT wave normal
            // (`wave_dir_at_bounce`) every bounce, not read off `(*stokes)[k]`'s
            // (possibly azimuth-drifted) Stokes state. The isotropic branch is the
            // Stokes-independent eigenmode midpoint -- see the scatter block's own
            // comment above.
            var alphas_interior: array<f32, 8>;
            for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                alphas_interior[k] = 0.0;
            }
            if (is_anisotropic) {
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    if (is_biaxial && material.has_beta_ray != 0u) {
                        alphas_interior[k] = assigned_mode_alpha_biaxial(
                            alpha_o_hoisted[k], alpha_beta_hoisted[k], alpha_e_hoisted[k],
                            n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2,
                            c_axis, wave_dir_at_bounce, (*is_extraordinary),
                        );
                    } else {
                        alphas_interior[k] = assigned_mode_alpha_uniaxial(
                            alpha_o_hoisted[k], alpha_e_hoisted[k], c_axis, wave_dir_at_bounce, (*is_extraordinary),
                            n_o_hero_seed, n_e_hero_seed,
                        );
                    }
                }
            } else {
                let eigen_a = ordinary_eigen_polarization(wave_dir_at_bounce, c_axis);
                let eigen_b = extraordinary_eigen_polarization(wave_dir_at_bounce, c_axis);
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    alphas_interior[k] = isotropic_channel_alpha(
                        alpha_o_hoisted[k], alpha_e_hoisted[k], c_axis, eigen_a, eigen_b,
                    );
                }
            }
            for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                // (absorption path scale): mirrors
                // optics::raytracer::absorption::apply_absorption's own
                // `path_len * ctx.material.absorption_path_scale` multiply exactly.
                let scaled_hit_t = hit.t * material.absorption_path_scale;
                // `exp_poly`, not the `exp()` builtin --
                // `apply_absorption` calls `crate::simd::exp_f32x8`, not `f32::exp`.
                let trans_factor = exp_poly(-alphas_interior[k] * scaled_hit_t);
                local_stokes[k] = local_stokes[k] * trans_factor;
            }
        }
        (*s).stokes = local_stokes;
    }

    // Which specular/diffuse treatment
    // this facet gets. Bounds-checked (mirrors optics::raytracer::trace_spectral_ray_inner's
    // `facet_finishes.get(hit_rec.facet_idx).copied().unwrap_or_default()`): an
    // index past the end of a shorter-than-`planes` buffer defaults to
    // `facet_finish::POLISHED`, exactly as the CPU's `Default` does.
    // `select`'s two value arguments are BOTH evaluated in WGSL (no short-circuit),
    // so an `if` guard is used instead of `select` here -- indexing
    // `facet_finishes[hit.facet_idx]` unconditionally would attempt an
    // out-of-bounds storage-buffer read whenever `facet_idx >= arrayLength(...)`
    // (harmless per WGSL's robustness guarantees, but its RESULT is
    // implementation-defined, so this must never be allowed to influence `finish`).
    var finish: u32 = 0u;
    if (hit.facet_idx < arrayLength(&facet_finishes)) {
        finish = facet_finishes[hit.facet_idx];
    }
    // Captured before any of the three bounce-dispatch arms below run -- see
    // optics::raytracer::dispatch_bounce's doc comment for why `was_internal_reflection`
    // is always computed against the PRE-bounce `(*inside_gem)`. The TIR/reflect/refract
    // arms below never need this explicitly (they either never touch `(*inside_gem)` at
    // all, or only flip it once at their very end, after which nothing else in that
    // arm reads it this bounce) -- only the frosted arm, which can update `(*inside_gem)`
    // itself, needs the pre-bounce value spelled out separately.
    let pre_bounce_inside_gem = (*s).inside_gem;

    (*h).hit_point = hit_point;
    (*h).shading_normal = normal;
    (*h).wave_dir_at_bounce = wave_dir_at_bounce;
    (*h).finish = finish;
    (*h).pre_bounce_inside_gem = pre_bounce_inside_gem;

    let idx = bounce_index_setup(s, h, rc);

    if (finish == FACET_FINISH_FROSTED) {
        (*h).arm = ARM_FROSTED;
    } else if (idx.sin2_t > 1.0) {
        (*h).arm = ARM_TIR;
    } else if (idx.uniaxial_nondegenerate) {
        if (!(*s).inside_gem) {
            (*h).arm = ARM_UNI_ENTRY;
        } else {
            (*h).arm = ARM_UNI_INTERNAL;
        }
    } else {
        (*h).arm = ARM_GENERIC;
    }

    return PHASE_NEXT;
}

