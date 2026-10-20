// Arm 4: Uniaxial internal/exit bounce.
fn bounce_arm_uni_internal(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    let idx = bounce_index_setup(s, h, rc);
    let hit_point = (*h).hit_point;
    let normal = (*h).shading_normal;
    let wave_dir_at_bounce = (*h).wave_dir_at_bounce;
    let c_axis = rc.c_axis;
    let is_biaxial = rc.is_biaxial;
    let n_o_hoisted = rc.n_o_hoisted;
    let studio_key_dir = rc.studio_key_dir;
    let current_plane_normal = (*s).prev_plane_normal;
    let phase_pdf_this_check = (*h).phase_pdf_this_check;
    let phase_dir_this_check = (*h).phase_dir_this_check;
    let cos_i = idx.cos_i;
    let uframe = idx.uframe;
    let n_o_hero = idx.n_o_hero;
    let n_e_hero_solve = idx.n_e_raw_ch[0];
    let n_e_raw_ch = idx.n_e_raw_ch;
    let n_medium_ch = idx.n_medium_ch;
    let n1 = idx.n1;
    let n2 = idx.n2;

    // ---- INTERNAL: mirrors apply_uniaxial_internal_bounce ----
    let sol_hero_i = internal_solve(n1, n_o_hero, n_e_hero_solve, c_axis, uframe, !(*s).is_extraordinary);
    let flux_inc_hero_i = max(sol_hero_i.flux_inc, 1e-12);
    let ro_pow_hero = cplx_norm_sqr(sol_hero_i.r_o) * sol_hero_i.flux_ro;
    let re_pow_hero = cplx_norm_sqr(sol_hero_i.r_e) * sol_hero_i.flux_re;
    let internal_r_branch = clamp((ro_pow_hero + re_pow_hero) / flux_inc_hero_i, R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
    let internal_p_o_exact = ro_pow_hero / max(ro_pow_hero + re_pow_hero, 1e-12);
    let internal_rng_bounce = f32(hash_u32(seed0 ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM))) / 4294967295.0;

    if (internal_rng_bounce < internal_r_branch) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            var sol = sol_hero_i;
            if (k != 0u) {
                sol = internal_solve(n_medium_ch[k], n_o_hoisted[k], n_e_raw_ch[k], c_axis, uframe, !(*s).is_extraordinary);
            }
            let flux_inc_k = max(sol.flux_inc, 1e-12);
            // `fma` to match `apply_uniaxial_internal_reflect_channels`'s
            // `sol.r_e.norm_sqr().mul_add(sol.flux_re, sol.r_o.norm_sqr() * sol.flux_ro)`
            // bit-for-bit (1 ULP per uniaxial internal reflection otherwise).
            let r_total_k = min(fma(cplx_norm_sqr(sol.r_e), sol.flux_re, cplx_norm_sqr(sol.r_o) * sol.flux_ro) / flux_inc_k, 1.0);
            (*s).stokes[k] = (*s).stokes[k] * (r_total_k / internal_r_branch);
            (*s).path_pdf[k] = (*s).path_pdf[k] * clamp(r_total_k, 1e-4, 1.0 - 1e-4);
        }
        let new_k_i = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;
        (*s).origin = hit_point + new_k_i * RAY_EPS;
        (*s).dir = new_k_i;
        (*s).k = new_k_i;
        (*s).is_extraordinary = internal_mode_coupling_draw(
            c_axis, is_biaxial, current_plane_normal, new_k_i,
            (*s).stokes[0].x, (*s).stokes[0].y, (*s).stokes[0].z, true, internal_p_o_exact, seed0, bounce,
        );
    } else {
        let eta_dir_i = n1 / n2;
        let sin2_t_dir_i = min(eta_dir_i * eta_dir_i * fma(-cos_i, cos_i, 1.0), 1.0);
        let cos_t_dir_i = sqrt(max(1.0 - sin2_t_dir_i, 0.0));
        let hero_refr_dir_i = normalize(eta_dir_i * wave_dir_at_bounce + fma(eta_dir_i, cos_i, -cos_t_dir_i) * normal);

        // This uniaxial internal branch is reached only while `(*inside_gem)`
        // was already true (see the header comment at this `else`'s own
        // opening brace above), so this transmit sub-branch is always a
        // genuine transmit-out -- mirrors `dispatch_bounce`'s CPU-side
        // `exit.split_mis_weight` assignment exactly (same balance heuristic,
        // same `dist2d_pdf` at the carried INTERIOR direction).
        var split_mis_weight_i: f32 = 1.0;
        if (phase_pdf_this_check > 0.0 && nee_light_available()) {
            let light_pdf_i = nee_light_pdf(phase_dir_this_check, studio_key_dir);
            split_mis_weight_i = balance_heuristic(phase_pdf_this_check, light_pdf_i);
        }

        // this IS the uniaxial exact EXIT event (crystal -> air) --
        // mismatch resolves its own split (point 1) and keeps `path_pdf`
        // accumulating via this event's own exit factor (point 2), but
        // never narrows `compat` -- see
        // `apply_uniaxial_internal_transmit_channels`'s CPU-side doc
        // comment and this file's own header comment.
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            // captured before any mutation below, so the split branch
            // (which needs the ORIGINAL incident value after `(*stokes)[k]`
            // has already been zeroed by the unchanged chromatic-
            // termination bookkeeping) can still compute k's own
            // transmission.
            let original_stokes_i = (*s).stokes[k].x;
            let n_inc_k = n_medium_ch[k];
            let eta_dir_k = n_inc_k;
            let sin2_t_k = eta_dir_k * eta_dir_k * fma(-cos_i, cos_i, 1.0);
            if (sin2_t_k > 1.0) {
                (*s).stokes[k] = (*s).stokes[k] * 0.0;
                (*s).path_pdf[k] = 0.0;
                continue;
            }
            let cos_t_k = sqrt(max(1.0 - sin2_t_k, 0.0));
            let refr_wave_dir_k = normalize(eta_dir_k * wave_dir_at_bounce + fma(eta_dir_k, cos_i, -cos_t_k) * normal);
            let direction_matches_i = dot(refr_wave_dir_k, hero_refr_dir_i) >= DIRECTION_MATCH_COS_TOL;

            // Performance: reuse the caller's already-solved hero channel
            // instead of re-solving the identical boundary system -- needed
            // by both branches below (matching directly; split via
            // `compute_uniaxial_exit_transmission`).
            var sol = sol_hero_i;
            if (k != 0u) {
                sol = internal_solve(n_inc_k, n_o_hoisted[k], n_e_raw_ch[k], c_axis, uframe, !(*s).is_extraordinary);
            }

            if (!direction_matches_i) {
                // Chromatic termination.
                let prefix_path_pdf_k = (*s).path_pdf[k];
                (*s).stokes[k] = (*s).stokes[k] * 0.0;
                (*s).path_pdf[k] = 0.0;

                let et_mm = compute_uniaxial_exit_transmission(sol, internal_r_branch, original_stokes_i);
                let t_unpol_k_mm = clamp(et_mm.i_unit, 1e-4, 1.0 - 1e-4);
                (*s).path_pdf[k] = prefix_path_pdf_k * t_unpol_k_mm;
                if (original_stokes_i > 0.0) {
                    (*h).split_mask = (*h).split_mask | (1u << k);
                    (*h).split_dir[k] = vec4<f32>(refr_wave_dir_k, et_mm.transmitted.x);
                    (*h).split_mis_weight = split_mis_weight_i;
                }
                continue;
            }

            // direction_matches: the matching-branch formula.
            let et = compute_uniaxial_exit_transmission(sol, internal_r_branch, original_stokes_i);
            (*s).stokes[k] = et.transmitted;
            let t_unpol_k = clamp(et.i_unit, 1e-4, 1.0 - 1e-4);
            (*s).path_pdf[k] = (*s).path_pdf[k] * t_unpol_k;
        }
        (*s).origin = hit_point + hero_refr_dir_i * RAY_EPS;
        (*s).dir = hero_refr_dir_i;
        (*s).k = hero_refr_dir_i;
        (*s).inside_gem = false;
        // This uniaxial closed-form branch is reached only when
        // `(*inside_gem)` was already true (the "---- INTERNAL ----" arm),
        // so a live incoming carry is always a genuine transmit-out here --
        // pass it through unchanged, mirroring `dispatch_bounce`'s identical
        // CPU-side carry-through for the generic (isotropic/biaxial) exit.
        (*s).pending_light_mis = phase_pdf_this_check;
        (*s).pending_light_mis_dir = phase_dir_this_check;
    }
    return PHASE_NEXT;
}

// Arm 5: Generic (isotropic/biaxial/degenerate-uniaxial) bounce.
fn bounce_arm_generic(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    let idx = bounce_index_setup(s, h, rc);
    let hit_point = (*h).hit_point;
    let normal = (*h).shading_normal;
    let wave_dir_at_bounce = (*h).wave_dir_at_bounce;
    let is_anisotropic = rc.is_anisotropic;
    let is_biaxial = rc.is_biaxial;
    let c_axis = rc.c_axis;
    let n_o_hoisted = rc.n_o_hoisted;
    let birefringence_delta = rc.birefringence_delta;
    let studio_key_dir = rc.studio_key_dir;
    let current_plane_normal = (*s).prev_plane_normal;
    let phase_pdf_this_check = (*h).phase_pdf_this_check;
    let phase_dir_this_check = (*h).phase_dir_this_check;
    let n_alpha_hero = rc.n_alpha_hero;
    let n_beta_hero = rc.n_beta_hero;
    let n_gamma_hero = rc.n_gamma_hero;
    let biax_ax0 = rc.biax_ax0;
    let biax_ax1 = rc.biax_ax1;
    let biax_ax2 = rc.biax_ax2;
    let cos_i = idx.cos_i;
    let sin_i = idx.sin_i;
    let n_o_hero = idx.n_o_hero;
    let n_e_hero = idx.n_e_hero;
    let n_medium_ch = idx.n_medium_ch;
    let n_biax_a_hero = idx.n_biax_a_hero;
    let n_biax_b_hero = idx.n_biax_b_hero;
    let n_biax_a_ch = idx.n_biax_a_ch;
    let n_alpha_ch = idx.n_alpha_ch;
    let n_gamma_ch = idx.n_gamma_ch;
    let n1 = idx.n1;
    let n2 = idx.n2;

    // at an air->crystal entry into an anisotropic material, which
    // eigenmode (ordinary/mode-A vs extraordinary/mode-B) this path's single
    // geometric transmission event represents is decided HERE -- before the
    // reflect-vs-transmit draw below -- rather than inside the REFRACT arm's
    // own transmit branch. See
    // optics::raytracer::refraction::apply_partial_fresnel_bounce's doc
    // comment (CPU side) for the full two-part rationale: (P1) the
    // polarization-weighted selection (`entry_eigenmode_selection`) must be
    // shared by both branches below -- a beam already aligned with the
    // ordinary axis should be MORE likely to reflect at the ordinary index
    // too, not just more likely to transmit as ordinary conditional on
    // transmitting -- and (P2) the REFLECT branch's own Fresnel coefficients
    // must be evaluated at the SAME mode's index the transmit branch uses, so
    // `R + T == 1` for whichever mode this draw actually selects. A biaxial
    // material has no uniaxial "ordinary" eigenmode to weight against, so both
    // the selection and the index correction below are gated on `!is_biaxial`.
    let entering_anisotropic = !(*s).inside_gem && is_anisotropic;
    var entry_valid = 0u;
    var entry_cos_2psi_o = 0.0;
    var entry_sin_2psi_o = 0.0;
    var p_o = 0.5;
    if (entering_anisotropic && !is_biaxial) {
        let sel = entry_eigenmode_selection(
            c_axis, current_plane_normal, wave_dir_at_bounce,
            (*s).stokes[0].x, (*s).stokes[0].y, (*s).stokes[0].z,
        );
        entry_valid = sel.valid;
        entry_cos_2psi_o = sel.cos_2psi_o;
        entry_sin_2psi_o = sel.sin_2psi_o;
        if (sel.valid != 0u) {
            p_o = sel.p_o;
        }
    }
    var use_extraordinary = (*s).is_extraordinary;
    if (entering_anisotropic) {
        // PERF-11: the draw's only reader is the comparison below, so the two
        // hashes are paid only on an anisotropic entry (same expression).
        let mode_split_rand = f32(hash_u32(seed0 ^ hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))) / 4294967295.0;
        // Must match `apply_partial_fresnel_bounce`'s `mode_split_rand < (1.0 -
        // p_o)` EXACTLY (same operator, same sense), not just the same marginal
        // probability: `rand < 1-p_o` and `rand >= p_o` integrate to the same
        // P(extraordinary) but pick different halves of the identical [0,1)
        // draw, so they disagree on which mode a path gets. Previously read
        // `mode_split_rand >= p_o`, silently breaking CPU/GPU parity at every
        // anisotropic entry -- do not "simplify" this back.
        use_extraordinary = mode_split_rand < (1.0 - p_o);
    }
    // The chosen mode's own doubled polarization azimuth, for the per-channel
    // eigenmode projection in the REFRACT arm below -- meaningless unless
    // `entry_valid != 0u`. Extraordinary is perpendicular to ordinary
    // (psi_e == psi_o + 90deg), so its doubled azimuth is the negation of the
    // ordinary one.
    var entry_cos_2psi_x = entry_cos_2psi_o;
    var entry_sin_2psi_x = entry_sin_2psi_o;
    if (use_extraordinary) {
        entry_cos_2psi_x = -entry_cos_2psi_o;
        entry_sin_2psi_x = -entry_sin_2psi_o;
    }

    // use the SELECTED mode's own index for n2 at THIS interface -- `n2`
    // (mode B/extraordinary) unchanged when extraordinary is selected, not
    // entering an anisotropic material at all, or biaxial (`n_o_hero` is a
    // uniaxial-only quantity, not mode A), `n_o_hero` when the ordinary mode
    // is selected instead. Bit-identical to the plain `sqrt(1.0 - sin2_t)` in
    // every case this reduces to (`n2_decision == n2`): both `n1 / n2_decision`
    // and `sin2_t` itself are pure functions of `n1`/`n2`/`cos_i`, so recomputing
    // the ratio here rather than reusing the hero-level `sin2_t` produces the
    // identical bits when the index is unchanged.
    let n2_decision = select(n2, n_o_hero, entering_anisotropic && !is_biaxial && !use_extraordinary);
    let eta_decision = n1 / n2_decision;
    let cos_t = sqrt(max(1.0 - eta_decision * eta_decision * fma(-cos_i, cos_i, 1.0), 0.0));
    let r_s = fma(n2_decision, -cos_t, n1 * cos_i) / fma(n2_decision, cos_t, n1 * cos_i);
    let r_p = fma(n1, -cos_t, n2_decision * cos_i) / fma(n1, cos_t, n2_decision * cos_i);
    let r_unpol_raw = 0.5 * fma(r_p, r_p, r_s * r_s);
    // [0.02, 0.98] here (distinct from the per-channel R_UNPOL_MIN/MAX
    // used only to scale path_pdf, never to divide stokes) caps the
    // `1/r_unpol`/`1/(1-r_unpol)` divisions below at 50x instead of 10,000x at
    // grazing incidence -- still unbiased, just far less firefly-prone.
    let r_unpol = clamp(r_unpol_raw, R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
    let rng_bounce = f32(hash_u32(seed0 ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM))) / 4294967295.0;

    if (rng_bounce < r_unpol) {
        // REFLECT: each channel applies its OWN Fresnel/TIR matrix, divided by
        // the SAME hero selection probability r_unpol (the PDF-division
        // coupling that makes GPU/CPU float divergence harmless -- see this
        // file's header comment).
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            let n1k = select(1.0, n_medium_ch[k], (*s).inside_gem);
            let n2k = select(n_medium_ch[k], 1.0, (*s).inside_gem);
            let etak = n1k / n2k;
            let sin2_t_k = etak * etak * fma(-cos_i, cos_i, 1.0);
            var refl: mat4x4<f32>;
            if (sin2_t_k > 1.0) {
                let delta_k = tir_phase_delta(n1k, cos_i, sin_i);
                refl = mueller_tir_retardation(delta_k);
            } else {
                let cos_t_k = sqrt(max(1.0 - sin2_t_k, 0.0));
                let r_s_k = fma(n2k, -cos_t_k, n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
                let r_p_k = fma(n1k, -cos_t_k, n2k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
                let r_unpol_k = clamp(0.5 * fma(r_p_k, r_p_k, r_s_k * r_s_k), R_UNPOL_MIN, R_UNPOL_MAX);
                (*s).path_pdf[k] = (*s).path_pdf[k] * r_unpol_k;
                refl = mueller_fresnel_reflection(r_s_k, r_p_k);
            }
            (*s).stokes[k] = (refl * (*s).stokes[k]) * (1.0 / r_unpol);
        }
        // reflects the WAVE NORMAL `k` (not `S`), then re-derives `S'` for
        // the reflected `k'` via `poynting_dir_for_mode` -- see this file's own
        // design note above `wave_dir_at_bounce`.
        let k_prime = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;
        let s_prime = poynting_dir_for_mode(
            is_anisotropic, is_biaxial, (*s).inside_gem, (*s).is_extraordinary, k_prime, c_axis,
            n_o_hero, n_e_hero, n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2,
        );
        (*s).origin = hit_point + s_prime * RAY_EPS;
        (*s).dir = s_prime;
        (*s).k = k_prime;

        // This arm never changes `(*inside_gem)` (unlike the refract arm
        // below, which always flips it), so `(*inside_gem)` here still holds its
        // pre-bounce value -- an internal reflection iff it was already true.
        if (is_anisotropic && (*s).inside_gem) {
            // No stokes/path_pdf scaling -- relabeling, not a split; see
            // internal_mode_coupling_draw's doc comment. This arm is only
            // reached for a biaxial material or a degenerate-axis uniaxial one
            // (the non-degenerate uniaxial case takes the closed-form
            // `uniaxial_nondegenerate` branch above instead), so no exact p_o
            // is available here -- falls back to the polarization-projection
            // heuristic exactly as before.
            (*s).is_extraordinary = internal_mode_coupling_draw(
                c_axis, is_biaxial, current_plane_normal, k_prime,
                (*s).stokes[0].x, (*s).stokes[0].y, (*s).stokes[0].z, false, 0.0, seed0, bounce,
            );
        }
    } else {
        // REFRACT -- optics::raytracer::apply_refract_bounce.
        // `entering_anisotropic` and `use_extraordinary` are ALREADY resolved
        // above (shared with the REFLECT arm's own r_unpol computation) -- no
        // fresh draw here. On an air->crystal entry into an anisotropic
        // material, the selected mode's own Fresnel transmittance is applied
        // to that mode's PROJECTED Stokes state (see the per-channel loop
        // below) at its own unscaled intensity: the selection probability is
        // set to match this mode's own true physical energy fraction exactly,
        // so multiplying by that fraction and dividing by the identical
        // selection probability cancel -- not the naive `1/0.5` a disjoint
        // split would need. For a cubic material (or any bounce that is not an
        // anisotropic entry) `entering_anisotropic` is false and
        // `use_extraordinary` keeps whatever `(*is_extraordinary)` already was.

        // Direction: the mode-A eigenmode uses n_mode_a and (uniaxial only) is
        // never walked off; the mode-B eigenmode's ENERGY (Poynting) direction
        // is displaced by the walk-off angle -- computed BEFORE the per-channel
        // loop below so each companion channel's own hypothetical direction can
        // be compared against this SAME hero-driven direction. For a biaxial material entering the crystal,
        // NEITHER mode is a plain constant-index Snell refraction -- BOTH modes
        // walk off via `biaxial_mode_poynting_dir`, using `n_biax_a_hero`/
        // `n_biax_b_hero` (the SAME looked-up scalars the per-channel loop's own
        // `k == hero_idx` iteration uses) for self-consistency.
        // `refr_wave_dir` is the SNELL-REFRACTED WAVE NORMAL `k'` (fed from
        // `wave_dir_at_bounce`, not `(*current_dir)`/`S` -- see this file's
        // own design note above `wave_dir_at_bounce`, and rule 4 in particular:
        // "at exit into air, refract k (not S)"). Captured into `new_k` alongside
        // the Poynting-converted `final_refr_dir` (`S'`), since the caller needs
        // BOTH from here on.
        var new_k: vec3<f32>;
        var final_refr_dir: vec3<f32>;
        if (entering_anisotropic && is_biaxial) {
            let n2_hero_dir = select(n_biax_a_hero, n_biax_b_hero, use_extraordinary);
            let eta_dir = n1 / n2_hero_dir;
            let sin2_t_dir = min(eta_dir * eta_dir * fma(-cos_i, cos_i, 1.0), 1.0);
            let cos_t_dir = sqrt(max(1.0 - sin2_t_dir, 0.0));
            let refr_wave_dir = normalize(
                eta_dir * wave_dir_at_bounce + fma(eta_dir, cos_i, -cos_t_dir) * normal,
            );
            new_k = refr_wave_dir;
            final_refr_dir = biaxial_mode_poynting_dir(n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2, refr_wave_dir, use_extraordinary);
        } else {
            let n2_hero_dir = select(n2, n_o_hero, entering_anisotropic && !use_extraordinary);
            let eta_dir = n1 / n2_hero_dir;
            let sin2_t_dir = min(eta_dir * eta_dir * fma(-cos_i, cos_i, 1.0), 1.0);
            let cos_t_dir = sqrt(max(1.0 - sin2_t_dir, 0.0));
            let refr_wave_dir = normalize(
                eta_dir * wave_dir_at_bounce + fma(eta_dir, cos_i, -cos_t_dir) * normal,
            );
            new_k = refr_wave_dir;
            final_refr_dir = refr_wave_dir;
            if (entering_anisotropic && use_extraordinary) {
                final_refr_dir = extraordinary_poynting_dir(refr_wave_dir, c_axis, n_o_hero, n_e_hero);
            }
        }

        // `is_exit_event` mirrors apply_refract_channel's own
        // `(*inside_gem) && !entering_anisotropic` -- since `entering_anisotropic`
        // is only ever true while `!(*inside_gem)`, this reduces to plain
        // `(*inside_gem)` (still the PRE-flip value here, unchanged until after
        // this loop). Interior (entry) mismatches instead narrow `compat`,
        // never split -- see this file's own header comment.
        let is_exit_event = (*s).inside_gem;
        // Mirrors `dispatch_bounce`'s CPU-side `exit.split_mis_weight`
        // assignment exactly: `is_exit_event` (this bounce's PRE-flip
        // `inside_gem`) is precisely `pre_bounce_inside_gem`, and this whole
        // per-channel loop's split branch is reachable only from the
        // hero-driven transmit dispatch, so computing the weight from it here,
        // before any per-channel work, is exact rather than an approximation.
        var split_mis_weight: f32 = 1.0;
        if (is_exit_event && phase_pdf_this_check > 0.0 && nee_light_available()) {
            let light_pdf = nee_light_pdf(phase_dir_this_check, studio_key_dir);
            split_mis_weight = balance_heuristic(phase_pdf_this_check, light_pdf);
        }
        var scalar_dirs: array<vec3<f32>, 8>;
        var scalar_dirs_valid: array<bool, 8>;
        var scalar_hero_match: array<bool, 8>;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            scalar_dirs[k] = vec3<f32>(0.0, 0.0, 0.0);
            scalar_dirs_valid[k] = false;
            scalar_hero_match[k] = false;
        }
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            // captured before any mutation below, so the mismatch/split
            // branch (which needs the ORIGINAL incident value after `(*stokes)[k]`
            // has already been zeroed by the unchanged chromatic-termination
            // bookkeeping) can still compute k's own transmission.
            let original_stokes_k = (*s).stokes[k];
            let n1k = select(1.0, n_medium_ch[k], (*s).inside_gem);
            var n2k = select(n_medium_ch[k], 1.0, (*s).inside_gem);
            if (entering_anisotropic && !use_extraordinary) {
                if (is_biaxial) {
                    n2k = n_biax_a_ch[k];
                } else {
                    n2k = n_o_hoisted[k];
                }
            }
            let ratio_k = n1k / n2k;
            let sin2_t_k = (ratio_k * ratio_k) * fma(-cos_i, cos_i, 1.0);
            if (sin2_t_k > 1.0) {
                // Chromatic termination: channel k cannot transmit at this
                // angle even though the hero-driven path did. Both the Stokes
                // contribution AND the path_pdf are dropped to exactly 0 --
                // never just down-weighted (see this file's header comment).
                (*s).stokes[k] = (*s).stokes[k] * 0.0;
                (*s).path_pdf[k] = 0.0;
                continue;
            }
            let cos_t_k = sqrt(max(1.0 - sin2_t_k, 0.0));
            let refr_wave_dir_k = normalize(
                ratio_k * wave_dir_at_bounce + fma(ratio_k, cos_i, -cos_t_k) * normal,
            );
            // The direction-match identity trap: channel k's own
            // walk-off, using k's own per-channel indices, compared against the
            // STORED `final_refr_dir` above -- never a second recomputation of
            // the hero's own direction (which would be a few ULP different and
            // chromatically self-terminate the hero channel against itself).
            // channel k's own biaxial walk-off, using k's own
            // (n_alpha_ch[k], n_beta_ch := n_o_hoisted[k], n_gamma_ch[k]) evaluated
            // at k's own single-shot refracted wave direction -- the direct
            // per-channel generalization of the uniaxial extraordinary_poynting_dir
            // call below.
            var final_dir_k = refr_wave_dir_k;
            if (entering_anisotropic && is_biaxial) {
                final_dir_k = biaxial_mode_poynting_dir(n_alpha_ch[k], n_o_hoisted[k], n_gamma_ch[k], biax_ax0, biax_ax1, biax_ax2, refr_wave_dir_k, use_extraordinary);
            } else if (entering_anisotropic && use_extraordinary) {
                let n_e_k = n_o_hoisted[k] + birefringence_delta;
                final_dir_k = extraordinary_poynting_dir(refr_wave_dir_k, c_axis, n_o_hoisted[k], n_e_k);
            }
            scalar_dirs[k] = final_dir_k;
            scalar_dirs_valid[k] = true;
            let direction_matches = dot(final_dir_k, final_refr_dir) >= DIRECTION_MATCH_COS_TOL;
            scalar_hero_match[k] = direction_matches;
            if (direction_matches) {
                let t_s_k = (2.0 * n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
                let t_p_k = (2.0 * n1k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
                let trans = mueller_fresnel_transmission(n1k, n2k, cos_i, cos_t_k, t_s_k, t_p_k);
                // project this channel's incident Stokes state onto the
                // SELECTED eigenmode before transmission -- see
                // entry_eigenmode_selection's doc comment for the full
                // derivation. Fully linear along the mode's own axis (`w`/`V`
                // zeroed), at this channel's OWN unscaled intensity (the
                // mode-selection draw's probability matches this mode's true
                // physical energy fraction exactly, so multiplying by that
                // fraction and dividing by the identical selection probability
                // cancel). `entry_valid == 0u` (biaxial, or negligible linear
                // polarization) leaves `(*stokes)[k]` unprojected.
                var incident_k = (*s).stokes[k];
                if (entering_anisotropic && entry_valid != 0u) {
                    let i_k = (*s).stokes[k].x;
                    incident_k = vec4<f32>(i_k, i_k * entry_cos_2psi_x, i_k * entry_sin_2psi_x, 0.0);
                }
                // No `/ split_pdf` -- see the entering_anisotropic comment above.
                (*s).stokes[k] = (trans * incident_k) * (1.0 / (1.0 - r_unpol));

                let r_s_k = fma(n2k, -cos_t_k, n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
                let r_p_k = fma(n1k, -cos_t_k, n2k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
                let r_unpol_k = clamp(0.5 * fma(r_p_k, r_p_k, r_s_k * r_s_k), R_UNPOL_MIN, R_UNPOL_MAX);
                // No `* split_pdf` -- scale-invariant under a uniform per-channel
                // factor, was a pure no-op on the MIS weight; see refraction.rs.
                (*s).path_pdf[k] = (*s).path_pdf[k] * (1.0 - r_unpol_k);
            } else {
                // chromatic termination -- UNCHANGED radiance handling
                // (mismatched channel loses its own Stokes contribution
                // either way), but `(*path_pdf)[k]` keeps accumulating this
                // event's own transmit factor and, at an EXIT event, the
                // channel additionally resolves its own transmitted radiance
                // along its own refracted direction via
                // `try_split_exit_channel` -- see this file's own header
                // comment and `apply_refract_channel`'s CPU-side `else` branch.
                let prefix_path_pdf_k = (*s).path_pdf[k];
                (*s).stokes[k] = (*s).stokes[k] * 0.0;
                (*s).path_pdf[k] = 0.0;

                let azimuth_valid_here = entering_anisotropic && entry_valid != 0u;
                let ct = compute_channel_transmission(
                    n1k, n2k, cos_i, cos_t_k, r_unpol, entering_anisotropic,
                    azimuth_valid_here, entry_cos_2psi_x, entry_sin_2psi_x, original_stokes_k,
                );
                (*s).path_pdf[k] = prefix_path_pdf_k * (1.0 - ct.r_unpol_k);
                if (is_exit_event && original_stokes_k.x > 0.0) {
                    (*h).split_mask = (*h).split_mask | (1u << k);
                    (*h).split_dir[k] = vec4<f32>(refr_wave_dir_k, ct.transmitted.x);
                    (*h).split_mis_weight = split_mis_weight;
                }
            }
        }
        // an INTERIOR dispersive event (an entry into the gem) narrows
        // every channel's MIS family; the exit event never does -- see
        // `narrow_compat`'s own comment in `transport_physics.wgsl`.
        if (!(*s).inside_gem) {
            var local_compat = (*s).compat;
            narrow_compat(&local_compat, scalar_dirs, scalar_dirs_valid, scalar_hero_match);
            (*s).compat = local_compat;
        }
        (*s).origin = hit_point + final_refr_dir * RAY_EPS;
        (*s).dir = final_refr_dir;
        (*s).k = new_k;
        // `(*inside_gem)` still holds its PRE-bounce value here
        // (flipped just below) -- true only for a genuine transmit-out (never
        // true simultaneously with `entering_anisotropic`, which requires
        // `!inside_gem`) -- so pass a live incoming carry through unchanged,
        // mirroring `dispatch_bounce`'s identical CPU-side carry-through for
        // this same generic (isotropic/biaxial) exit arm.
        if ((*s).inside_gem) {
            (*s).pending_light_mis = phase_pdf_this_check;
            (*s).pending_light_mis_dir = phase_dir_this_check;
        }
        (*s).inside_gem = !(*s).inside_gem;
        if (entering_anisotropic) {
            (*s).is_extraordinary = use_extraordinary;
        }
    }
    return PHASE_NEXT;
}

