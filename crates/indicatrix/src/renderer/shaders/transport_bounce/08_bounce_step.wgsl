// See this file's header comment for the calling convention. Parameter order: `bounce`
// (the loop counter) and the per-ray RNG seed first, then every read-only per-ray
// constant in the same order the megakernel's own prologue computed them (`observer`,
// the unit direction back towards the eye for the lit lighting models' head shadow,
// last), then every mutable per-bounce state pointer in the same order the
// megakernel's own prologue declared them.
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
    // The running total of every Henyey-Greenstein scattering-point NEE deposit
    // so far, each already integrated to XYZ at ITS OWN moment's `path_pdf`/`compat` --
    // see `nee_contribution_hg_scatter`'s own doc comment. Summed into the final XYZ by
    // `transport_finalize_ray`, unconditionally, mirroring
    // `trace_spectral_ray_inner`'s identical `nee_xyz` accumulator on the CPU side.
    nee_xyz: ptr<function, vec3<f32>>,
    path_escaped: ptr<function, bool>,
    pending_light_mis: ptr<function, f32>,
    // The interior direction `pending_light_mis`'s phase pdf was evaluated
    // at -- paired with it exactly like the CPU's `Option<(f32, Vec3)>` carry, and
    // consumed the same way (`dist2d_pdf` below, instead of `(*current_dir)`, which by
    // the time a transmit-out carry reaches here is the refracted EXTERIOR direction).
    pending_light_mis_dir: ptr<function, vec3<f32>>,
) -> u32 {
        let phase_pdf_this_check = (*pending_light_mis);
        let phase_dir_this_check = (*pending_light_mis_dir);
        (*pending_light_mis) = 0.0;

        let hit = intersect_ray((*current_origin), (*current_dir));
        if (!hit.hit) {
            // The camera ray sees the backdrop card, if the scene has one -- see
            // `optics::raytracer::environment::fill_backdrop`.
            if (bounce == 0u && params.env_mode == 1u && params.backdrop > 0.0) {
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    (*radiance)[k] = params.backdrop * studio_spectral_power((*lambdas)[k]);
                }
                (*path_escaped) = true;
                return BOUNCE_STATUS_TERMINATE;
            }
            var mis_weight: f32 = 1.0;
            if (phase_pdf_this_check > 0.0 && params.env_mode == 2u) {
                let light_pdf = dist2d_pdf(phase_dir_this_check);
                mis_weight = balance_heuristic(phase_pdf_this_check, light_pdf);
            }
            for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                let env = sample_environment_with_rig((*current_dir), (*lambdas)[k], studio_key_dir, studio_fill_dir, studio_sin_lp, observer);
                // `max((*stokes)[k].x, 0.0)` clamps `I` to >= 0 before the environment
                // lookup, matching `accumulate_miss_radiance`'s `StokesVector::intensity`
                // on the CPU side -- negative `I` is unphysical on either side.
                (*radiance)[k] = fma(max((*stokes)[k].x, 0.0) * mis_weight, env, (*radiance)[k]);
            }
            // The only site that sets this: staged exit-split contributions commit
            // only when the shared/hero path itself reaches this environment lookup.
            (*path_escaped) = true;
            return BOUNCE_STATUS_TERMINATE;
        }

        // Mirrors optics::raytracer::trace_spectral_ray_inner's restructured bounce
        // loop: attempt a Henyey-Greenstein scattering event somewhere along this
        // segment before the plane-of-incidence rotation / facet processing below.
        // Gated on `material.scattering_sigma_s > 0.0` -- every scene with it `<= 0.0`
        // skips this block entirely, matching the CPU's default-off bit-identity
        // guarantee.
        if ((*inside_gem) && material.scattering_sigma_s > 0.0) {
            // P1 (assigned-mode absorption): mirrors the absorption block's
            // `is_anisotropic`/`is_biaxial` branching below -- `try_scatter_step` feeds
            // the same `channel_absorption_alphas_assigned` the absorption block uses.
            // The propagation direction fed to every alpha function here is the WAVE
            // NORMAL `k`, not the Poynting direction `S` -- (*current_k), not (*current_dir).
            var alphas: array<f32, 8>;
            if (is_anisotropic) {
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    // alpha_o/alpha_e/alpha_beta hoisted above (per-ray, not per-bounce).
                    if (is_biaxial && material.has_beta_ray != 0u) {
                        alphas[k] = assigned_mode_alpha_biaxial(
                            alpha_o_hoisted[k], alpha_beta_hoisted[k], alpha_e_hoisted[k],
                            n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2,
                            c_axis, (*current_k), (*is_extraordinary),
                        );
                    } else {
                        alphas[k] = assigned_mode_alpha_uniaxial(
                            alpha_o_hoisted[k], alpha_e_hoisted[k], c_axis, (*current_k), (*is_extraordinary),
                            n_o_hero_seed, n_e_hero_seed,
                        );
                    }
                }
            } else {
                // Isotropic-by-symmetry material: the plain midpoint of the two
                // eigenmode quadratic forms, independent of the Stokes state, exactly as
                // optics::raytracer::absorption::channel_absorption_alphas_assigned
                // computes it. A dichroic tensor on an isotropic material therefore gets
                // the same unpolarized average on both sides.
                let eigen_a = ordinary_eigen_polarization((*current_k), c_axis);
                let eigen_b = extraordinary_eigen_polarization((*current_k), c_axis);
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    alphas[k] = isotropic_channel_alpha(
                        alpha_o_hoisted[k], alpha_e_hoisted[k], c_axis, eigen_a, eigen_b,
                    );
                }
            }
            let sc = maybe_scatter_or_extinguish(
                alphas, material.scattering_sigma_s, material.scattering_g, (*current_dir), hit.t,
                material.absorption_path_scale, seed0, bounce, stokes, path_pdf,
            );
            if (sc.scattered != 0u) {
                let scatter_point = (*current_origin) + sc.t_free * (*current_dir);
                let old_dir = (*current_dir);
                if (params.env_mode == 2u) {
                    nee_contribution_hg_scatter(
                        lambdas, n_o_hero_seed, scatter_point, old_dir,
                        material.scattering_g, seed0, bounce, stokes, path_pdf, compat,
                        nee_xyz, alphas,
                    );
                    (*pending_light_mis) = henyey_greenstein_phase(dot(sc.new_dir, old_dir), material.scattering_g);
                    (*pending_light_mis_dir) = sc.new_dir;
                }
                (*current_origin) = scatter_point;
                (*current_dir) = sc.new_dir;
                // A scattering event depolarizes, so `k` collapses to `S` going forward.
                (*current_k) = sc.new_dir;
                // Scattered Stokes vectors are already depolarized, so the previous
                // plane of incidence is not physically meaningful -- reset it
                // like the pre-first-bounce state.
                (*have_prev_plane_normal) = false;

                if (bounce > 4u) {
                    var max_intensity: f32 = 0.0;
                    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                        max_intensity = max(max_intensity, max((*stokes)[k].x, 0.0));
                    }
                    let q = clamp(max_intensity, RR_FLOOR, 1.0);
                    let rr_rand = f32(hash_u32(seed0 ^ hash_u32(bounce ^ RUSSIAN_ROULETTE_STREAM))) / 4294967295.0;
                    if (rr_rand > q) {
                        return BOUNCE_STATUS_TERMINATE;
                    }
                    // `split_radiance` rides along on the same `1/q` survival rescale
                    // as `stokes`.
                    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                        (*stokes)[k] = (*stokes)[k] * (1.0 / q);
                        (*split_radiance)[k] = (*split_radiance)[k] / q;
                    }
                }
                return BOUNCE_STATUS_CONTINUE;
            }
            // No scatter event fired: the path survived to the facet boundary, and
            // `maybe_scatter_or_extinguish` already applied this segment's full
            // extinction weight (absorption AND the scattering removal probability)
            // to `stokes`/`path_pdf`. Fall through to the facet-dispatch code below,
            // but see the (skipped) absorption block further down for why it is not
            // applied a second time.
        }

        let hit_point = (*current_origin) + hit.t * (*current_dir);
        // See shading_normal_near_edge's own doc comment.
        var normal = shading_normal_near_edge(hit_point, hit.facet_idx, hit.normal, material.edge_rounding_radius);
        // `wave_dir_at_bounce` is the wave normal `k` (== (*current_k)), used for
        // EVERY index lookup, cos_i/sin_i, Snell/Fresnel evaluation, TIR decision, and
        // the Stokes plane-of-incidence frame below -- see
        // optics::raytracer::refraction's "wave normal vs Poynting direction" design
        // note. The Poynting/energy direction `S` (== (*current_dir)) is used directly
        // (geometric origin-advance/intersection only) where still needed.
        // `wave_dir_at_bounce == (*current_dir)` trivially outside the crystal and for the
        // uniaxial ordinary eigenmode, so every such case is bit-identical to the plain
        // all-`(*current_dir)` code.
        let wave_dir_at_bounce = (*current_k);

        // Plane-of-incidence frame rotation (signed psi via atan2).
        let cpn_raw = cross(wave_dir_at_bounce, normal);
        let cpn_len2 = dot(cpn_raw, cpn_raw);
        var current_plane_normal: vec3<f32>;
        if (cpn_len2 > 0.0) {
            current_plane_normal = cpn_raw / sqrt(cpn_len2);
        } else {
            current_plane_normal = vec3<f32>(0.0, 0.0, 0.0);
        }
        if ((*have_prev_plane_normal) && cpn_len2 > 1e-6 && dot((*prev_plane_normal), (*prev_plane_normal)) > 1e-6) {
            let psi = signed_frame_rotation_psi((*prev_plane_normal), current_plane_normal, wave_dir_at_bounce);
            let rot = mueller_frame_rotation(psi);
            for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                (*stokes)[k] = rot * (*stokes)[k];
            }
        }
        (*prev_plane_normal) = current_plane_normal;
        (*have_prev_plane_normal) = true;

        if ((*inside_gem)) {
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
                    (*stokes)[k] = (*stokes)[k] * trans_factor;
                }
            }
        }

        // angle of incidence measured against the WAVE NORMAL `k`
        // (`wave_dir_at_bounce`), not the Poynting/energy direction `S` -- see this
        // block's own design note above.
        let cos_i = clamp(dot(-wave_dir_at_bounce, normal), 0.0, 1.0);
        let sin_i = sqrt(max(fma(-cos_i, cos_i, 1.0), 0.0));

        // theta_c fixed-point iteration (optics::raytracer::theta_c_for_bounce)
        // plus the per-channel ordinary/effective-extraordinary index pair
        // (optics::raytracer::per_channel_uniaxial_indices, called once per channel via
        // per_channel_uniaxial_index -- see transport_physics.wgsl). For a cubic
        // material (is_anisotropic == false) this reduces to n_eff_ch[k] == n_o_ch[k]
        // for every k, reducing to the isotropic-only computation. For a biaxial
        // material this is a DEAD (unused) computation -- see transport_physics.wgsl's
        // `theta_c_for_bounce` doc comment. Fed `wave_dir_at_bounce` (`k`), not
        // `(*current_dir)` (`S`).
        let theta_c = theta_c_for_bounce(normal, wave_dir_at_bounce, cos_i, (*inside_gem), is_anisotropic, c_axis, n_o_hero_seed, n_e_hero_seed);

        // `n_o_ch[k]` is exactly `n_o_hoisted[k]` (both `dispersion_evaluate` on
        // the same `(*lambdas)[k]`, hoisted above) -- only the theta_c-dependent
        // `n_eff_k` half of `per_channel_uniaxial_index` still needs to run every
        // bounce, so that's all this loop does now, reproducing that function's own
        // `n_e_k`/`effective_extraordinary_index` body (see transport_physics.wgsl) on
        // the hoisted `n_o_hoisted[k]` rather than calling the full function (which
        // would redundantly re-evaluate dispersion) -- bit-identical either way.
        //
        // n_e_k mirrors `GemMaterial::extraordinary_index_at` exactly: a genuine
        // wavelength-dependent evaluation of the material's own extraordinary-ray
        // curve when `material.has_extraordinary_dispersion != 0`
        // (Quartz/Amethyst/Citrine), else the constant-offset
        // `n_o_hoisted[k] + birefringence_delta` approximation.
        var n_eff_ch: array<f32, 8>;
        // RAW (not theta_c-projected) per-channel extraordinary index, hoisted here so
        // the uniaxial entry/internal dispatch arms below can read it directly instead
        // of recomputing the dispersion evaluation.
        var n_e_raw_ch: array<f32, 8>;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            var n_e_k: f32;
            if (material.has_extraordinary_dispersion != 0u) {
                n_e_k = extraordinary_dispersion_evaluate(material.extraordinary_model_type, material.extraordinary_param_a, material.extraordinary_param_b, (*lambdas)[k]);
            } else {
                n_e_k = n_o_hoisted[k] + birefringence_delta;
            }
            n_e_raw_ch[k] = n_e_k;
            var n_eff_k = n_o_hoisted[k];
            if (is_anisotropic) {
                n_eff_k = effective_extraordinary_index(n_o_hoisted[k], n_e_k, theta_c);
            }
            n_eff_ch[k] = n_eff_k;
        }
        let n_o_hero = n_o_hoisted[0];
        // Deliberately the constant-offset form, NOT the accurate `n_e_hero_seed`
        // computed once per ray above -- this `n_e_hero` feeds only the
        // walk-off/direction (Poynting) approximation (`extraordinary_poynting_dir`
        // below), where the existing constant-offset behaviour is deliberately kept
        // as-is -- mirrors `BounceRefractionGeometry::n_e_hero`'s identical comment on
        // the CPU side (`optics::raytracer::refraction`, P5's fix site).
        let n_e_hero = n_o_hero + birefringence_delta;

        // Shared local frame + optic-axis direction cosines `uniaxial_fresnel`'s
        // closed-form solver needs (Lekner 1991), built once per bounce -- mirrors
        // `BounceRefractionGeometry::uniaxial_frame`'s "Some only when genuinely
        // uniaxial" contract; harmless to build unconditionally since every use below
        // is gated on `is_anisotropic && !is_biaxial`. `entry_incidence_frame`
        // (`n1 == 1.0` fixed for air->crystal entry) is likewise built once here even
        // though only the entry arm below consumes it.
        let uframe = uniaxial_frame_build(wave_dir_at_bounce, normal, c_axis, cos_i, sin_i);
        let uinc_frame = entry_incidence_frame(1.0, uframe);
        // Mirrors `BounceRefractionGeometry::uniaxial_frame`'s `Some` condition
        // exactly -- the plain uniaxial-vs-biaxial-vs-isotropic material classification,
        // with no degenerate-axis check of its own (see `uniaxial_nondegenerate` below
        // for that).
        let uniaxial_active = is_anisotropic && !is_biaxial;
        // Degenerate wave-normal-parallel-to-optic-axis limit -- mirrors
        // `apply_partial_fresnel_bounce`'s identical cross-product-length guard AND
        // `apply_tir_bounce`'s own identical guard on its uniaxial branch
        // (`refraction/tir.rs`): falling through to the existing scalar-at-`n_o`
        // machinery is exact at this limit, not an approximation, on every dispatch
        // arm below, including the forced-TIR one.
        let uniaxial_nondegenerate = uniaxial_active
            && dot(cross(wave_dir_at_bounce, c_axis), cross(wave_dir_at_bounce, c_axis)) > 1e-6;

        // optics::raytracer::{hero_biaxial_wave_dirs, per_channel_biaxial_indices}.
        // "mode A" is the faster (lower-index) root, "mode B" the slower -- see this
        // file's header comment. Zero-initialized and never consulted downstream unless
        // `is_biaxial` guards the read, exactly mirroring the CPU arrays' own
        // "computed unconditionally, only ever POPULATED when is_biaxial" contract.
        var wave_dir_a_hero = vec3<f32>(0.0, 0.0, 0.0);
        var wave_dir_b_hero = vec3<f32>(0.0, 0.0, 0.0);
        var n_biax_a_ch: array<f32, 8>;
        var n_biax_b_ch: array<f32, 8>;
        var n_alpha_ch: array<f32, 8>;
        var n_gamma_ch: array<f32, 8>;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            n_biax_a_ch[k] = 0.0;
            n_biax_b_ch[k] = 0.0;
            n_alpha_ch[k] = 0.0;
            n_gamma_ch[k] = 0.0;
        }
        if (is_biaxial) {
            if ((*inside_gem)) {
                // both biaxial modes walk off (see optics::raytracer::refraction's
                // hero_biaxial_wave_dirs doc comment) -- k, not S.
                wave_dir_a_hero = wave_dir_at_bounce;
                wave_dir_b_hero = wave_dir_at_bounce;
            } else {
                let res_a = biaxial_resolve_entry_mode(n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2, wave_dir_at_bounce, normal, cos_i, n_o_hero_seed, false);
                let res_b = biaxial_resolve_entry_mode(n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2, wave_dir_at_bounce, normal, cos_i, n_o_hero_seed, true);
                wave_dir_a_hero = res_a.wave_dir;
                wave_dir_b_hero = res_b.wave_dir;
            }
            for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                // optics::materials::GemMaterial::biaxial_indicatrix's per-channel
                // n_beta/n_alpha/n_gamma -- n_beta_k is bit-identical to n_o_hoisted[k]
                // (both `dispersion.evaluate((*lambdas)[k])`, same inputs), so reused
                // rather than recomputed.
                let n_beta_k = n_o_hoisted[k];
                let n_alpha_k = n_beta_k - material.dispersion.biaxial_delta_beta_alpha;
                let n_gamma_k = n_alpha_k + birefringence_delta;
                n_alpha_ch[k] = n_alpha_k;
                n_gamma_ch[k] = n_gamma_k;
                let ni_a = biaxial_wave_indices(n_alpha_k, n_beta_k, n_gamma_k, biax_ax0, biax_ax1, biax_ax2, wave_dir_a_hero);
                let ni_b = biaxial_wave_indices(n_alpha_k, n_beta_k, n_gamma_k, biax_ax0, biax_ax1, biax_ax2, wave_dir_b_hero);
                n_biax_a_ch[k] = ni_a.y;
                n_biax_b_ch[k] = ni_b.x;
            }
        }
        let n_biax_a_hero = n_biax_a_ch[0];
        let n_biax_b_hero = n_biax_b_ch[0];

        // While inside an anisotropic crystal, the medium
        // index this ray is currently in is mode A or mode B depending on which
        // eigenmode `(*is_extraordinary)` selected at the most recent entry; outside the
        // crystal (or for an isotropic material) it is always the mode-B array, which
        // for a cubic material equals n_o_hoisted exactly (see per_channel_uniaxial_index).
        // `is_biaxial` selects which pair of arrays ("mode A"/"mode B") is consulted --
        // the biaxial ones computed just above, or the uniaxial n_o_hoisted/n_eff_ch pair.
        let use_mode_a_medium = is_anisotropic && (*inside_gem) && !(*is_extraordinary);
        var n_medium_ch: array<f32, 8>;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            var mode_a_k: f32;
            var mode_b_k: f32;
            if (is_biaxial) {
                mode_a_k = n_biax_a_ch[k];
                mode_b_k = n_biax_b_ch[k];
            } else {
                mode_a_k = n_o_hoisted[k];
                mode_b_k = n_eff_ch[k];
            }
            n_medium_ch[k] = select(mode_b_k, mode_a_k, use_mode_a_medium);
        }
        let n_medium_hero = n_medium_ch[0];
        let n1 = select(1.0, n_medium_hero, (*inside_gem));
        let n2 = select(n_medium_hero, 1.0, (*inside_gem));
        let eta = n1 / n2;
        let sin2_t = eta * eta * fma(-cos_i, cos_i, 1.0);

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
        let pre_bounce_inside_gem = (*inside_gem);

        if (finish == FACET_FINISH_FROSTED) {
            // optics::raytracer::apply_frosted_bounce (via dispatch_bounce): the
            // REPLACEMENT for the TIR/partial-reflect/refract dispatch below, not an
            // addition to it -- see transport_physics.wgsl's own doc comment for the
            // achromatic-by-design physics this must preserve exactly.
            let fb = apply_frosted_bounce(
                is_anisotropic, sin2_t, n1, n2, cos_i, normal, (*inside_gem), (*is_extraordinary),
                seed0, bounce, stokes, path_pdf,
            );
            if (fb.new_inside_gem == 0u) {
                let ext_normal = select(normal, -normal, pre_bounce_inside_gem);
                if (params.env_mode == 2u) {
                    nee_contribution_frosted_exterior(
                        lambdas, ext_normal, seed0, bounce, stokes, radiance,
                    );
                    (*pending_light_mis) = dot(fb.new_dir, ext_normal) / PI;
                    // `fb.new_dir` is already the true exterior propagation direction
                    // here (a frosted transmit needs no further refraction before a
                    // possible direct escape), mirroring `dispatch_bounce`'s identical
                    // frosted-arm carry on the CPU side.
                    (*pending_light_mis_dir) = fb.new_dir;
                }
            }
            (*current_origin) = hit_point + fb.new_dir * RAY_EPS;
            (*current_dir) = fb.new_dir;
            // a frosted (diffuse) bounce already depolarizes -- k collapses to S,
            // mirroring optics::raytracer::transport::dispatch_bounce's identical
            // treatment of FacetFinish::Frosted.
            (*current_k) = fb.new_dir;
            (*inside_gem) = fb.new_inside_gem != 0u;
            if (fb.has_extraordinary_update != 0u) {
                (*is_extraordinary) = fb.extraordinary_update != 0u;
            }
            // Same was_internal_reflection formula optics::raytracer::dispatch_bounce
            // uses for every bounce kind -- true for the frosted TIR-forced and reflect
            // arms ((*inside_gem) unchanged, no extraordinary update reported), false for
            // the transmit arm ((*inside_gem) always flips there).
            let was_internal_reflection = pre_bounce_inside_gem
                && (fb.has_extraordinary_update == 0u)
                && ((*inside_gem) == pre_bounce_inside_gem);
            if (is_anisotropic && was_internal_reflection) {
                // No stokes/path_pdf scaling here -- see internal_mode_coupling_draw's
                // doc comment: this is a RELABELING of which eigenmode governs the
                // NEXT bounce, not a SPLIT into two rays, so the matching unbiased
                // scale factor is 1.0 (no-op) -- the same 1.0 conclusion the entry
                // split itself reaches, for a different reason (see this file's header
                // comment / `apply_refract_bounce`'s doc comment on the CPU side).
                (*is_extraordinary) = internal_mode_coupling_draw(
                    c_axis, is_biaxial, current_plane_normal, fb.new_dir,
                    (*stokes)[0].x, (*stokes)[0].y, (*stokes)[0].z, false, 0.0, seed0, bounce,
                );
            }
        } else if (sin2_t > 1.0) {
            // Hero forced TIR (probability 1, no pdf division needed).
            // full uniaxial Fresnel (Lekner 1991): the closed-form o<->e-coupled
            // TIR reflectance -- mirrors
            // `optics::raytracer::refraction::apply_tir_bounce`'s own uniaxial branch
            // bit-for-bit, INCLUDING that function's own degenerate
            // wave-normal-parallel-to-optic-axis guard: `uniaxial_nondegenerate`
            // (not the plain `uniaxial_active`, which every OTHER dispatch arm in this
            // function deliberately avoids for exactly this reason -- see that flag's
            // own doc comment) falls through to the scalar branch below at that limit,
            // exactly as the CPU's now-guarded `apply_tir_bounce` does. `tir_exact_p_o`
            // is the hero channel's own `R_o/(R_o+R_e)`, fed to
            // `internal_mode_coupling_draw` below exactly as `apply_tir_bounce`'s own
            // `exact_p_o` return value is on the CPU side.
            var tir_exact_p_o: f32 = 0.5;
            if (uniaxial_nondegenerate) {
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    let n_inc_k = select(1.0, n_medium_ch[k], (*inside_gem));
                    let sol = internal_solve(n_inc_k, n_o_hoisted[k], n_e_raw_ch[k], c_axis, uframe, !(*is_extraordinary));
                    let flux_inc = max(sol.flux_inc, 1e-12);
                    let ro_pow = cplx_norm_sqr(sol.r_o) * sol.flux_ro;
                    let re_pow = cplx_norm_sqr(sol.r_e) * sol.flux_re;
                    let r_total_k = min((ro_pow + re_pow) / flux_inc, 1.0);
                    (*stokes)[k] = (*stokes)[k] * r_total_k;
                    let n2k_dbg = select(n_medium_ch[k], 1.0, (*inside_gem));
                    let etak_dbg = n_inc_k / n2k_dbg;
                    let sin2_t_k = etak_dbg * etak_dbg * fma(-cos_i, cos_i, 1.0);
                    if (sin2_t_k <= 1.0) {
                        (*path_pdf)[k] = (*path_pdf)[k] * clamp(r_total_k, 1e-4, 1.0 - 1e-4);
                    }
                    if (k == 0u) {
                        tir_exact_p_o = ro_pow / max(ro_pow + re_pow, 1e-12);
                    }
                }
            } else {
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    let n1k = select(1.0, n_medium_ch[k], (*inside_gem));
                    let n2k = select(n_medium_ch[k], 1.0, (*inside_gem));
                    let etak = n1k / n2k;
                    let sin2_t_k = etak * etak * fma(-cos_i, cos_i, 1.0);
                    if (sin2_t_k > 1.0) {
                        let delta_k = tir_phase_delta(n1k, cos_i, sin_i);
                        (*stokes)[k] = mueller_tir_retardation(delta_k) * (*stokes)[k];
                    } else {
                        let cos_t_k = sqrt(max(1.0 - sin2_t_k, 0.0));
                        let r_s_k = fma(n2k, -cos_t_k, n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
                        let r_p_k = fma(n1k, -cos_t_k, n2k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
                        (*stokes)[k] = mueller_fresnel_reflection(r_s_k, r_p_k) * (*stokes)[k];
                        let r_unpol_k = clamp(0.5 * fma(r_p_k, r_p_k, r_s_k * r_s_k), R_UNPOL_MIN, R_UNPOL_MAX);
                        (*path_pdf)[k] = (*path_pdf)[k] * r_unpol_k;
                    }
                }
            }
            // reflects the WAVE NORMAL `k` (not `S`), then re-derives `S'` for the
            // reflected `k'` via `poynting_dir_for_mode` -- see this file's own design
            // note above `wave_dir_at_bounce`.
            let k_prime = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;
            let s_prime = poynting_dir_for_mode(
                is_anisotropic, is_biaxial, (*inside_gem), (*is_extraordinary), k_prime, c_axis,
                n_o_hero, n_e_hero, n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2,
            );
            (*current_origin) = hit_point + s_prime * RAY_EPS;
            (*current_dir) = s_prime;
            (*current_k) = k_prime;

            // TIR is always an internal reflection (`n1 > n2` for the hero
            // channel implies `(*inside_gem)`, exactly as on the CPU side).
            if (is_anisotropic) {
                // `has_exact_p_o` must be `uniaxial_nondegenerate`, NOT the plain
                // `uniaxial_active` -- `tir_exact_p_o` only holds a genuinely solved
                // value when the `uniaxial_nondegenerate` branch above ran; at the
                // degenerate wave-normal-parallel-to-optic-axis limit it is still its
                // unset default (`0.5`), and claiming that as an "exact" p_o here would
                // skip `internal_mode_coupling_draw`'s own polarization-weighted
                // `entry_eigenmode_selection` heuristic -- exactly mirroring
                // `optics::raytracer::refraction::apply_tir_bounce`'s `exact_p_o: None`
                // return at this same limit (`transport::bounce`'s
                // `apply_internal_mode_coupling` call site consumes that `None` via
                // the identical heuristic). No stokes/path_pdf scaling either way --
                // relabeling, not a split; see internal_mode_coupling_draw's doc comment.
                (*is_extraordinary) = internal_mode_coupling_draw(
                    c_axis, is_biaxial, current_plane_normal, k_prime,
                    (*stokes)[0].x, (*stokes)[0].y, (*stokes)[0].z, uniaxial_nondegenerate, tir_exact_p_o, seed0, bounce,
                );
            }
        } else if (uniaxial_nondegenerate) {
            // full uniaxial Fresnel (Lekner 1991): entry AND general (non-forced)
            // internal/exit dispatch, in full, self-contained -- mirrors
            // optics::raytracer::refraction::apply_partial_fresnel_bounce's own early
            // returns to apply_uniaxial_entry_bounce/apply_uniaxial_internal_bounce.
            // `n_e_hero`/`n_e_ch` used elsewhere in this kernel for WALK-OFF/DIRECTION
            // purposes are the constant-offset `n_o + birefringence_delta`
            // approximation (matching `BounceRefractionGeometry::n_e_hero` on the CPU
            // side); the closed-form FRESNEL solve below instead needs the material's
            // own genuine extraordinary-ray dispersion curve where one exists (`n_e_
            // raw_ch`/`n_e_hero_solve`, matching `GemMaterial::extraordinary_index_at`
            // -- see `apply_uniaxial_entry_bounce`'s and `apply_uniaxial_internal_
            // bounce`'s own local `n_e_hero` shadowing on the CPU side for why these
            // are deliberately two different values, not a duplicate).
            let n_e_hero_solve = n_e_raw_ch[0];

            if (!(*inside_gem)) {
                // ---- ENTRY: mirrors apply_uniaxial_entry_bounce ----
                let pair_hero = entry_solve_pair_with_incidence(uinc_frame, 1.0, n_o_hero, n_e_hero_solve, c_axis, uframe);
                let sol_s_hero = pair_hero.s_sol;
                let sol_p_hero = pair_hero.p_sol;
                let reflect_mueller_hero = jones_to_mueller(sol_s_hero.r_s, sol_p_hero.r_s, sol_s_hero.r_p, sol_p_hero.r_p);
                let entry_r_branch = clamp(reflect_mueller_hero[0][0], R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
                let entry_inc_flux = 0.5 * uframe.cos_i;
                let p_o_raw = mode_power(sol_s_hero.t_o, sol_p_hero.t_o, sol_s_hero.flux_o, entry_inc_flux, (*stokes)[0]);
                let p_e_raw = mode_power(sol_s_hero.t_e, sol_p_hero.t_e, sol_s_hero.flux_e, entry_inc_flux, (*stokes)[0]);
                let p_o_hero = clamp(p_o_raw / max(p_o_raw + p_e_raw, 1e-12), R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
                let entry_mode_split_rand = f32(hash_u32(seed0 ^ hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))) / 4294967295.0;
                let use_extraordinary_entry = entry_mode_split_rand < (1.0 - p_o_hero);
                var p_mode_hero_frac = p_o_hero;
                if (use_extraordinary_entry) {
                    p_mode_hero_frac = 1.0 - p_o_hero;
                }

                let n2_hero_dir_e = select(n_o_hero, n2, use_extraordinary_entry);
                let eta_dir_e = n1 / n2_hero_dir_e;
                let sin2_t_dir_e = min(eta_dir_e * eta_dir_e * fma(-cos_i, cos_i, 1.0), 1.0);
                let cos_t_dir_e = sqrt(max(1.0 - sin2_t_dir_e, 0.0));
                let refr_wave_dir_e = normalize(eta_dir_e * wave_dir_at_bounce + fma(eta_dir_e, cos_i, -cos_t_dir_e) * normal);
                var final_refr_dir_e = refr_wave_dir_e;
                if (use_extraordinary_entry) {
                    final_refr_dir_e = extraordinary_poynting_dir(refr_wave_dir_e, c_axis, n_o_hero, n_e_hero);
                }
                let p_axis_transmitted = cross(refr_wave_dir_e, uframe.s_axis);

                let entry_rng_bounce = f32(hash_u32(seed0 ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM))) / 4294967295.0;

                if (entry_rng_bounce < entry_r_branch) {
                    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                        let pair_k = entry_solve_pair_with_incidence(uinc_frame, 1.0, n_o_hoisted[k], n_e_raw_ch[k], c_axis, uframe);
                        let mueller_k = jones_to_mueller(pair_k.s_sol.r_s, pair_k.p_sol.r_s, pair_k.s_sol.r_p, pair_k.p_sol.r_p);
                        let r_unpol_k = clamp(mueller_k[0][0], 1e-4, 1.0 - 1e-4);
                        (*stokes)[k] = (mueller_k * (*stokes)[k]) * (1.0 / entry_r_branch);
                        (*path_pdf)[k] = (*path_pdf)[k] * r_unpol_k;
                    }
                    let new_k_e = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;
                    (*current_origin) = hit_point + new_k_e * RAY_EPS;
                    (*current_dir) = new_k_e;
                    (*current_k) = new_k_e;
                } else {
                    // this is an INTERIOR dispersive event (air->crystal entry) --
                    // mismatch keeps `path_pdf` accumulating (via this event's own
                    // `t_unpol_k`) and narrows `compat` below, no split -- see this
                    // file's own header comment and
                    // `apply_uniaxial_entry_transmit_channels`'s CPU-side doc comment.
                    var entry_dirs: array<vec3<f32>, 8>;
                    var entry_dirs_valid: array<bool, 8>;
                    var entry_hero_match: array<bool, 8>;
                    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                        let n_o_k = n_o_hoisted[k];
                        var n2k_dir = n_o_k;
                        if (use_extraordinary_entry) {
                            n2k_dir = n_eff_ch[k];
                        }
                        let eta_dir_k = n1 / n2k_dir;
                        let sin2_t_k = eta_dir_k * eta_dir_k * fma(-cos_i, cos_i, 1.0);
                        if (sin2_t_k > 1.0) {
                            (*stokes)[k] = (*stokes)[k] * 0.0;
                            (*path_pdf)[k] = 0.0;
                            continue;
                        }
                        let cos_t_k = sqrt(max(1.0 - sin2_t_k, 0.0));
                        let refr_wave_dir_k = normalize(eta_dir_k * wave_dir_at_bounce + fma(eta_dir_k, cos_i, -cos_t_k) * normal);
                        var final_dir_k = refr_wave_dir_k;
                        if (use_extraordinary_entry) {
                            let n_e_eff_k = n_o_k + birefringence_delta;
                            final_dir_k = extraordinary_poynting_dir(refr_wave_dir_k, c_axis, n_o_k, n_e_eff_k);
                        }
                        entry_dirs[k] = final_dir_k;
                        entry_dirs_valid[k] = true;
                        let direction_matches_entry = dot(final_dir_k, final_refr_dir_e) >= DIRECTION_MATCH_COS_TOL;
                        entry_hero_match[k] = direction_matches_entry;
                        if (!direction_matches_entry) {
                            (*stokes)[k] = (*stokes)[k] * 0.0;
                            // technique k stays a live member of every compatible
                            // channel's MIS family -- keep its own entry-transmit
                            // density accumulating (the SAME t_unpol_k the matching
                            // branch below folds in); only its radiance ends here.
                            let pair_k_mm = entry_solve_pair_with_incidence(uinc_frame, 1.0, n_o_k, n_e_raw_ch[k], c_axis, uframe);
                            let mueller_k_mm = jones_to_mueller(pair_k_mm.s_sol.r_s, pair_k_mm.p_sol.r_s, pair_k_mm.s_sol.r_p, pair_k_mm.p_sol.r_p);
                            let t_unpol_k_mm = clamp(1.0 - mueller_k_mm[0][0], 1e-4, 1.0 - 1e-4);
                            (*path_pdf)[k] = (*path_pdf)[k] * t_unpol_k_mm;
                            continue;
                        }
                        let pair_k = entry_solve_pair_with_incidence(uinc_frame, 1.0, n_o_k, n_e_raw_ch[k], c_axis, uframe);
                        var t_s_row = pair_k.s_sol.t_o;
                        var t_p_row = pair_k.p_sol.t_o;
                        var flux_k = pair_k.s_sol.flux_o;
                        var mode_dir = pair_k.s_sol.o_hat;
                        if (use_extraordinary_entry) {
                            t_s_row = pair_k.s_sol.t_e;
                            t_p_row = pair_k.p_sol.t_e;
                            flux_k = pair_k.s_sol.flux_e;
                            mode_dir = pair_k.s_sol.e_hat;
                        }
                        let p_mode_k = mode_power(t_s_row, t_p_row, flux_k, entry_inc_flux, (*stokes)[k]);
                        let deposit_i = p_mode_k / p_mode_hero_frac;
                        let azimuth = azimuth2_in_frame(mode_dir, uframe.s_axis, p_axis_transmitted);
                        (*stokes)[k] = vec4<f32>(deposit_i, deposit_i * azimuth.x, deposit_i * azimuth.y, 0.0) * (1.0 / (1.0 - entry_r_branch));

                        let mueller_k = jones_to_mueller(pair_k.s_sol.r_s, pair_k.p_sol.r_s, pair_k.s_sol.r_p, pair_k.p_sol.r_p);
                        let t_unpol_k = clamp(1.0 - mueller_k[0][0], 1e-4, 1.0 - 1e-4);
                        (*path_pdf)[k] = (*path_pdf)[k] * t_unpol_k;
                    }
                    narrow_compat(compat, entry_dirs, entry_dirs_valid, entry_hero_match);
                    (*current_origin) = hit_point + final_refr_dir_e * RAY_EPS;
                    (*current_dir) = final_refr_dir_e;
                    (*current_k) = refr_wave_dir_e;
                    (*inside_gem) = true;
                    (*is_extraordinary) = use_extraordinary_entry;
                }
            } else {
                // ---- INTERNAL: mirrors apply_uniaxial_internal_bounce ----
                let sol_hero_i = internal_solve(n1, n_o_hero, n_e_hero_solve, c_axis, uframe, !(*is_extraordinary));
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
                            sol = internal_solve(n_medium_ch[k], n_o_hoisted[k], n_e_raw_ch[k], c_axis, uframe, !(*is_extraordinary));
                        }
                        let flux_inc_k = max(sol.flux_inc, 1e-12);
                        // `fma` to match `apply_uniaxial_internal_reflect_channels`'s
                        // `sol.r_e.norm_sqr().mul_add(sol.flux_re, sol.r_o.norm_sqr() * sol.flux_ro)`
                        // bit-for-bit (1 ULP per uniaxial internal reflection otherwise).
                        let r_total_k = min(fma(cplx_norm_sqr(sol.r_e), sol.flux_re, cplx_norm_sqr(sol.r_o) * sol.flux_ro) / flux_inc_k, 1.0);
                        (*stokes)[k] = (*stokes)[k] * (r_total_k / internal_r_branch);
                        (*path_pdf)[k] = (*path_pdf)[k] * clamp(r_total_k, 1e-4, 1.0 - 1e-4);
                    }
                    let new_k_i = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;
                    (*current_origin) = hit_point + new_k_i * RAY_EPS;
                    (*current_dir) = new_k_i;
                    (*current_k) = new_k_i;
                    (*is_extraordinary) = internal_mode_coupling_draw(
                        c_axis, is_biaxial, current_plane_normal, new_k_i,
                        (*stokes)[0].x, (*stokes)[0].y, (*stokes)[0].z, true, internal_p_o_exact, seed0, bounce,
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
                    if (phase_pdf_this_check > 0.0 && params.env_mode == 2u) {
                        let light_pdf_i = dist2d_pdf(phase_dir_this_check);
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
                        let original_stokes_i = (*stokes)[k].x;
                        let n_inc_k = n_medium_ch[k];
                        let eta_dir_k = n_inc_k;
                        let sin2_t_k = eta_dir_k * eta_dir_k * fma(-cos_i, cos_i, 1.0);
                        if (sin2_t_k > 1.0) {
                            (*stokes)[k] = (*stokes)[k] * 0.0;
                            (*path_pdf)[k] = 0.0;
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
                            sol = internal_solve(n_inc_k, n_o_hoisted[k], n_e_raw_ch[k], c_axis, uframe, !(*is_extraordinary));
                        }

                        if (!direction_matches_i) {
                            // Chromatic termination.
                            let prefix_path_pdf_k = (*path_pdf)[k];
                            (*stokes)[k] = (*stokes)[k] * 0.0;
                            (*path_pdf)[k] = 0.0;

                            let et_mm = compute_uniaxial_exit_transmission(sol, internal_r_branch, original_stokes_i);
                            let t_unpol_k_mm = clamp(et_mm.i_unit, 1e-4, 1.0 - 1e-4);
                            (*path_pdf)[k] = prefix_path_pdf_k * t_unpol_k_mm;
                            if (original_stokes_i > 0.0) {
                                try_split_exit_channel(
                                    split_radiance, hit_point, k, (*lambdas)[k], refr_wave_dir_k,
                                    et_mm.transmitted.x, studio_key_dir, studio_fill_dir, studio_sin_lp, observer,
                                    split_mis_weight_i,
                                );
                            }
                            continue;
                        }

                        // direction_matches: the matching-branch formula.
                        let et = compute_uniaxial_exit_transmission(sol, internal_r_branch, original_stokes_i);
                        (*stokes)[k] = et.transmitted;
                        let t_unpol_k = clamp(et.i_unit, 1e-4, 1.0 - 1e-4);
                        (*path_pdf)[k] = (*path_pdf)[k] * t_unpol_k;
                    }
                    (*current_origin) = hit_point + hero_refr_dir_i * RAY_EPS;
                    (*current_dir) = hero_refr_dir_i;
                    (*current_k) = hero_refr_dir_i;
                    (*inside_gem) = false;
                    // This uniaxial closed-form branch is reached only when
                    // `(*inside_gem)` was already true (the "---- INTERNAL ----" arm),
                    // so a live incoming carry is always a genuine transmit-out here --
                    // pass it through unchanged, mirroring `dispatch_bounce`'s identical
                    // CPU-side carry-through for the generic (isotropic/biaxial) exit.
                    (*pending_light_mis) = phase_pdf_this_check;
                    (*pending_light_mis_dir) = phase_dir_this_check;
                }
            }
        } else {
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
            let entering_anisotropic = !(*inside_gem) && is_anisotropic;
            var entry_valid = 0u;
            var entry_cos_2psi_o = 0.0;
            var entry_sin_2psi_o = 0.0;
            var p_o = 0.5;
            if (entering_anisotropic && !is_biaxial) {
                let sel = entry_eigenmode_selection(
                    c_axis, current_plane_normal, wave_dir_at_bounce,
                    (*stokes)[0].x, (*stokes)[0].y, (*stokes)[0].z,
                );
                entry_valid = sel.valid;
                entry_cos_2psi_o = sel.cos_2psi_o;
                entry_sin_2psi_o = sel.sin_2psi_o;
                if (sel.valid != 0u) {
                    p_o = sel.p_o;
                }
            }
            let mode_split_rand = f32(hash_u32(seed0 ^ hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))) / 4294967295.0;
            var use_extraordinary = (*is_extraordinary);
            if (entering_anisotropic) {
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
                    let n1k = select(1.0, n_medium_ch[k], (*inside_gem));
                    let n2k = select(n_medium_ch[k], 1.0, (*inside_gem));
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
                        (*path_pdf)[k] = (*path_pdf)[k] * r_unpol_k;
                        refl = mueller_fresnel_reflection(r_s_k, r_p_k);
                    }
                    (*stokes)[k] = (refl * (*stokes)[k]) * (1.0 / r_unpol);
                }
                // reflects the WAVE NORMAL `k` (not `S`), then re-derives `S'` for
                // the reflected `k'` via `poynting_dir_for_mode` -- see this file's own
                // design note above `wave_dir_at_bounce`.
                let k_prime = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;
                let s_prime = poynting_dir_for_mode(
                    is_anisotropic, is_biaxial, (*inside_gem), (*is_extraordinary), k_prime, c_axis,
                    n_o_hero, n_e_hero, n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2,
                );
                (*current_origin) = hit_point + s_prime * RAY_EPS;
                (*current_dir) = s_prime;
                (*current_k) = k_prime;

                // This arm never changes `(*inside_gem)` (unlike the refract arm
                // below, which always flips it), so `(*inside_gem)` here still holds its
                // pre-bounce value -- an internal reflection iff it was already true.
                if (is_anisotropic && (*inside_gem)) {
                    // No stokes/path_pdf scaling -- relabeling, not a split; see
                    // internal_mode_coupling_draw's doc comment. This arm is only
                    // reached for a biaxial material or a degenerate-axis uniaxial one
                    // (the non-degenerate uniaxial case takes the closed-form
                    // `uniaxial_nondegenerate` branch above instead), so no exact p_o
                    // is available here -- falls back to the polarization-projection
                    // heuristic exactly as before.
                    (*is_extraordinary) = internal_mode_coupling_draw(
                        c_axis, is_biaxial, current_plane_normal, k_prime,
                        (*stokes)[0].x, (*stokes)[0].y, (*stokes)[0].z, false, 0.0, seed0, bounce,
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
                let is_exit_event = (*inside_gem);
                // Mirrors `dispatch_bounce`'s CPU-side `exit.split_mis_weight`
                // assignment exactly: `is_exit_event` (this bounce's PRE-flip
                // `inside_gem`) is precisely `pre_bounce_inside_gem`, and this whole
                // per-channel loop's split branch is reachable only from the
                // hero-driven transmit dispatch, so computing the weight from it here,
                // before any per-channel work, is exact rather than an approximation.
                var split_mis_weight: f32 = 1.0;
                if (is_exit_event && phase_pdf_this_check > 0.0 && params.env_mode == 2u) {
                    let light_pdf = dist2d_pdf(phase_dir_this_check);
                    split_mis_weight = balance_heuristic(phase_pdf_this_check, light_pdf);
                }
                var scalar_dirs: array<vec3<f32>, 8>;
                var scalar_dirs_valid: array<bool, 8>;
                var scalar_hero_match: array<bool, 8>;
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    // captured before any mutation below, so the mismatch/split
                    // branch (which needs the ORIGINAL incident value after `(*stokes)[k]`
                    // has already been zeroed by the unchanged chromatic-termination
                    // bookkeeping) can still compute k's own transmission.
                    let original_stokes_k = (*stokes)[k];
                    let n1k = select(1.0, n_medium_ch[k], (*inside_gem));
                    var n2k = select(n_medium_ch[k], 1.0, (*inside_gem));
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
                        (*stokes)[k] = (*stokes)[k] * 0.0;
                        (*path_pdf)[k] = 0.0;
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
                        var incident_k = (*stokes)[k];
                        if (entering_anisotropic && entry_valid != 0u) {
                            let i_k = (*stokes)[k].x;
                            incident_k = vec4<f32>(i_k, i_k * entry_cos_2psi_x, i_k * entry_sin_2psi_x, 0.0);
                        }
                        // No `/ split_pdf` -- see the entering_anisotropic comment above.
                        (*stokes)[k] = (trans * incident_k) * (1.0 / (1.0 - r_unpol));

                        let r_s_k = fma(n2k, -cos_t_k, n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
                        let r_p_k = fma(n1k, -cos_t_k, n2k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
                        let r_unpol_k = clamp(0.5 * fma(r_p_k, r_p_k, r_s_k * r_s_k), R_UNPOL_MIN, R_UNPOL_MAX);
                        // No `* split_pdf` -- scale-invariant under a uniform per-channel
                        // factor, was a pure no-op on the MIS weight; see refraction.rs.
                        (*path_pdf)[k] = (*path_pdf)[k] * (1.0 - r_unpol_k);
                    } else {
                        // chromatic termination -- UNCHANGED radiance handling
                        // (mismatched channel loses its own Stokes contribution
                        // either way), but `(*path_pdf)[k]` keeps accumulating this
                        // event's own transmit factor and, at an EXIT event, the
                        // channel additionally resolves its own transmitted radiance
                        // along its own refracted direction via
                        // `try_split_exit_channel` -- see this file's own header
                        // comment and `apply_refract_channel`'s CPU-side `else` branch.
                        let prefix_path_pdf_k = (*path_pdf)[k];
                        (*stokes)[k] = (*stokes)[k] * 0.0;
                        (*path_pdf)[k] = 0.0;

                        let azimuth_valid_here = entering_anisotropic && entry_valid != 0u;
                        let ct = compute_channel_transmission(
                            n1k, n2k, cos_i, cos_t_k, r_unpol, entering_anisotropic,
                            azimuth_valid_here, entry_cos_2psi_x, entry_sin_2psi_x, original_stokes_k,
                        );
                        (*path_pdf)[k] = prefix_path_pdf_k * (1.0 - ct.r_unpol_k);
                        if (is_exit_event && original_stokes_k.x > 0.0) {
                            try_split_exit_channel(
                                split_radiance, hit_point, k, (*lambdas)[k], refr_wave_dir_k,
                                ct.transmitted.x, studio_key_dir, studio_fill_dir, studio_sin_lp, observer,
                                split_mis_weight,
                            );
                        }
                    }
                }
                // an INTERIOR dispersive event (an entry into the gem) narrows
                // every channel's MIS family; the exit event never does -- see
                // `narrow_compat`'s own comment in `transport_physics.wgsl`.
                if (!(*inside_gem)) {
                    narrow_compat(compat, scalar_dirs, scalar_dirs_valid, scalar_hero_match);
                }
                (*current_origin) = hit_point + final_refr_dir * RAY_EPS;
                (*current_dir) = final_refr_dir;
                (*current_k) = new_k;
                // `(*inside_gem)` still holds its PRE-bounce value here
                // (flipped just below) -- true only for a genuine transmit-out (never
                // true simultaneously with `entering_anisotropic`, which requires
                // `!inside_gem`) -- so pass a live incoming carry through unchanged,
                // mirroring `dispatch_bounce`'s identical CPU-side carry-through for
                // this same generic (isotropic/biaxial) exit arm.
                if ((*inside_gem)) {
                    (*pending_light_mis) = phase_pdf_this_check;
                    (*pending_light_mis_dir) = phase_dir_this_check;
                }
                (*inside_gem) = !(*inside_gem);
                if (entering_anisotropic) {
                    (*is_extraordinary) = use_extraordinary;
                }
            }
        }

        // Surface glare -- mirrors `trace_spectral_ray_inner`'s identical block: the camera
        // path's first event at a polished facet left the ray outside the stone, so it was
        // the specular reflection and the light never entered. Skipped at 1.0 and for a
        // frosted facet, so those cases are bit-identical.
        if (bounce == 0u && !(*inside_gem) && finish != FACET_FINISH_FROSTED && params.surface_glare < 1.0) {
            for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                (*stokes)[k] = (*stokes)[k] * params.surface_glare;
            }
        }

        if (bounce > 4u) {
            var max_intensity: f32 = 0.0;
            for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                max_intensity = max(max_intensity, max((*stokes)[k].x, 0.0));
            }
            let q = clamp(max_intensity, RR_FLOOR, 1.0);
            let rr_rand = f32(hash_u32(seed0 ^ hash_u32(bounce ^ RUSSIAN_ROULETTE_STREAM))) / 4294967295.0;
            if (rr_rand > q) {
                return BOUNCE_STATUS_TERMINATE;
            }
            // `split_radiance` rides along on the same `1/q` survival
            // rescale as `stokes` -- see `apply_russian_roulette`'s CPU-side doc comment.
            for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                (*stokes)[k] = (*stokes)[k] * (1.0 / q);
                (*split_radiance)[k] = (*split_radiance)[k] / q;
            }
        }

    // Natural fall-through of the megakernel's old loop body (no break/continue hit
    // above): survive to the next bounce.
    return BOUNCE_STATUS_CONTINUE;
}
