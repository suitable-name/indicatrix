
// optics::raytracer::color::integrate_channels_to_xyz_families, plus this ray's staged
// exit-split commit, the final white-balance/clamp, and the four output-buffer writes
// (`out_xyz` unconditionally, the three per-channel debug buffers plus `out_compat`
// when `params.write_debug_buffers != 0u`) -- the megakernel's entire per-ray tail from
// the end of the bounce loop onward, factored out (see this file's header comment) so
// BOTH `transport_main` (the megakernel, at the natural end of its bounce loop -- a ray
// that survives every bounce without escaping or dying still reaches this exactly as
// before) and `wavefront_bounce`/`wavefront_finalize_survivors`
// (`wavefront_transport.wgsl`) finalize a ray identically, writing into the SAME
// `out_xyz`/`out_radiance`/`out_lambdas`/`out_path_pdf`/`out_compat` bindings either
// way -- `idx` means exactly the same thing in both callers (dispatch-local
// `pixel_in_chunk * camera.num_samples + sample_in_chunk`), so no separate
// wavefront-only output buffer is needed.
//
// Mutates `*radiance` in place (folding in `split_radiance` when `path_escaped`) rather
// than a local copy -- the debug-buffer write below reads the POST-commit `radiance`,
// exactly as `transport_main` always has.
fn transport_finalize_ray(
    idx: u32,
    lambdas: array<f32, 8>,
    radiance: ptr<function, array<f32, 8>>,
    split_radiance: array<f32, 8>,
    path_pdf: array<f32, 8>,
    compat: array<u32, 8>,
    path_escaped: bool,
) {
    // commit the staged exit-split contributions into the REAL `radiance` array ONLY if
    // the shared/hero path itself reached its own environment lookup -- see
    // `split_radiance`'s own comment in `transport_bounce_step` above and
    // `ExitSplitCtx::split_radiance`'s CPU-side doc comment for why this all-or-nothing
    // gate is required. A no-op whenever nothing ever split, or every split channel's
    // own probe declined.
    if (path_escaped) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            (*radiance)[k] = (*radiance)[k] + split_radiance[k];
        }
    }

    // Every channel that is alive at the exit (matching or split) contributes its own
    // radiance, so each channel's balance-heuristic weight is normalised over exactly
    // the techniques (hero choices) under which THAT channel would have stayed alive on
    // this same geometric path -- its own family, `compat[k]` -- rather than the
    // hero's.
    var xyz = vec3<f32>(0.0, 0.0, 0.0);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        var family_pdf: f32 = 0.0;
        for (var j: u32 = 0u; j < NUM_CHANNELS; j = j + 1u) {
            if ((compat[k] & (1u << j)) != 0u) {
                family_pdf = family_pdf + path_pdf[j];
            }
        }
        // Same "should not happen" fallback as the shared `spectral_mis_weight`.
        var weight_k: f32 = 1.0;
        if (family_pdf > 1e-12) {
            weight_k = f32(NUM_CHANNELS) * path_pdf[0] / family_pdf;
        }
        let cmf = cie_1931_cmf(lambdas[k]);
        let weighted = (*radiance)[k] * weight_k;
        xyz = xyz + cmf * (weighted * NORM_FACTOR);
    }
    if (params.env_mode == 1u) {
        // Bradford-LMS-space von Kries adaptation, not a raw XYZ scale -- see
        // `apply_von_kries_white_balance`'s doc comment above and
        // `optics::raytracer::apply_von_kries_white_balance` on the CPU side.
        xyz = apply_von_kries_white_balance(xyz, params.white_balance);
    }
    // Mirrors `transport::trace_spectral_ray_inner`'s unconditional `.max(Vec3::ZERO)`.
    // Applied outside the `env_mode` branch exactly as the CPU does; pre-white-balance
    // `xyz` is a sum of non-negative terms, so the clamp is a no-op there.
    xyz = max(xyz, vec3<f32>(0.0));

    out_xyz[idx * 3u + 0u] = xyz.x;
    out_xyz[idx * 3u + 1u] = xyz.y;
    out_xyz[idx * 3u + 2u] = xyz.z;
    // `out_xyz` above is the only buffer a production dispatch
    // (`GpuFrameRenderer::accumulate`) ever reads back -- the four per-channel debug
    // buffers below exist for Tier 2/spectral-debug self-tests only. Guarding their
    // writes on `params.write_debug_buffers` (nonzero for every self-test) lets a
    // production dispatch bind tiny fixed-size dummy buffers for them instead of
    // buffers sized like `out_xyz` -- 9x less write traffic and 9x more samples per
    // chunk-budget dispatch.
    if (params.write_debug_buffers != 0u) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            out_radiance[idx * 8u + k] = (*radiance)[k];
            out_lambdas[idx * 8u + k] = lambdas[k];
            out_path_pdf[idx * 8u + k] = path_pdf[k];
            out_compat[idx * 8u + k] = compat[k];
        }
    }
}

// This ray's deterministic camera-ray/hero-wavelength setup -- the megakernel's old
// per-thread prologue up through the `lambdas` derivation, factored out (see this
// file's header comment) so `wavefront_generate` (`wavefront_transport.wgsl`) draws
// EXACTLY the same RNG sequence from the same `idx` that `transport_main` always has.
// `idx` is this dispatch's LOCAL `(pixel_in_chunk, sample_in_chunk)` tuple index --
// `params.pixel_offset` is added here to recover the GLOBAL pixel index camera-ray
// generation and the per-pixel Cranley-Patterson rotations need, exactly as
// `transport_main` always did; output/ray-state slots stay indexed by the caller's own
// `idx`.
//
// Deliberately does NOT compute the per-ray hoisted dispersion/absorption arrays
// (`n_o_hoisted` and friends) or the biaxial axis frame/studio-rig directions: those
// depend only on `lambdas`/`material`/`params` (never on anything ELSE per-ray), so
// `transport_bounce_step`'s callers recompute them fresh from this function's
// `lambdas` output on every bounce instead of paying to store them in the wavefront
// ray-state buffers -- see `wavefront_transport.wgsl`'s module doc comment for the
// buffer-traffic trade-off this makes.
struct GeneratedRay {
    origin: vec3<f32>,
    dir: vec3<f32>,
    lambdas: array<f32, 8>,
    seed0: u32,
}

fn transport_generate_ray(idx: u32) -> GeneratedRay {
    let pixel = idx / camera.num_samples + params.pixel_offset;
    let local_sample = idx % camera.num_samples;
    let sample_num = local_sample + params.sample_offset;

    let seed0 = hash_u32((pixel * 0x9e3779b9u) ^ (sample_num * 0x85ebca6bu));

    // Stratified pixel jitter and hero wavelength (optics::raytracer::
    // {low_discrepancy_base2, cranley_patterson_rotate}), not an unstratified
    // hash-uniform. `seed0` above still seeds every per-bounce draw in
    // `transport_bounce_step` (Fresnel branch, Russian roulette, birefringent split);
    // only jx/jy/hero_rand come from this construction.
    let rot_jx = low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_X_ROTATION_STREAM));
    let rot_jy = low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_Y_ROTATION_STREAM));
    let rot_hero = low_discrepancy_base2(hash_u32(pixel ^ HERO_WAVELENGTH_ROTATION_STREAM));
    let jx = cranley_patterson_rotate(low_discrepancy_base2(sample_num), rot_jx) - 0.5;
    let jy = cranley_patterson_rotate(radical_inverse_base(sample_num, 3u), rot_jy) - 0.5;
    let hero_rand = cranley_patterson_rotate(radical_inverse_base(sample_num, 5u), rot_hero);
    let raygen = generate_camera_ray(pixel, jx, jy);

    let channel_width = SPECTRUM_SPAN / f32(NUM_CHANNELS);
    let lambda_hero = fma(hero_rand, SPECTRUM_SPAN, SPECTRUM_MIN);
    var lambdas: array<f32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        let offset = fma(f32(k), channel_width, lambda_hero - SPECTRUM_MIN);
        lambdas[k] = SPECTRUM_MIN + (offset % SPECTRUM_SPAN);
    }

    var result: GeneratedRay;
    result.origin = raygen.origin;
    result.dir = raygen.dir;
    result.lambdas = lambdas;
    result.seed0 = seed0;
    return result;
}

// This ray's material-derived "hoisted" constants -- everything `transport_bounce_step`
// needs that depends only on `lambdas`/`material`/`MATERIAL_CLASS`/`params`, never on
// anything that varies bounce-to-bounce. Factored out (see this file's header comment)
// so `transport_main`'s prologue and `wavefront_bounce` (`wavefront_transport.wgsl`)
// compute it identically -- `transport_main` calls this ONCE per ray, exactly as its
// own inline prologue always did; `wavefront_bounce` calls it fresh on EVERY bounce
// dispatch instead of storing the result in the wavefront ray-state buffers -- see
// `wavefront_transport.wgsl`'s module doc comment for why that trade-off is bit-identical
// (a pure function of this ray's own already-stored `lambdas`) and what it costs.
struct RayConstants {
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
}

fn transport_hoist_ray_constants(lambdas: array<f32, 8>) -> RayConstants {
    let c_axis = material.dispersion.c_axis_and_birefringence.xyz;
    let birefringence_delta = material.dispersion.c_axis_and_birefringence.w;
    // ANDed with the pipeline-overridable MATERIAL_CLASS -- a no-op for
    // MATERIAL_CLASS_GENERIC, forced `false` for MATERIAL_CLASS_ISOTROPIC regardless of
    // the material buffer's own flag.
    let is_anisotropic = (material.dispersion.is_anisotropic != 0u)
        && (MATERIAL_CLASS != MATERIAL_CLASS_ISOTROPIC);
    // Whether this material's anisotropy is genuinely biaxial (three distinct principal
    // indices) rather than the uniaxial ordinary/extraordinary approximation -- hoisted
    // per-ray, constant across every bounce, like `c_axis` above. Never true for
    // MATERIAL_CLASS_ISOTROPIC/UNIAXIAL's routed materials anyway (see
    // `renderer::gpu::frame::classify_material`), so ANDing with MATERIAL_CLASS only
    // removes dead-by-construction reachability.
    let is_biaxial = (material.dispersion.has_biaxial_delta != 0u)
        && (MATERIAL_CLASS == MATERIAL_CLASS_GENERIC || MATERIAL_CLASS == MATERIAL_CLASS_BIAXIAL);
    // The biaxial principal-axis frame (alpha, beta, gamma world directions) depends
    // only on `c_axis`, so it is computed once per ray rather than every bounce.
    let biax_axes = biaxial_axes_from_gamma(c_axis);
    let biax_ax0 = biax_axes.ax0;
    let biax_ax1 = biax_axes.ax1;
    let biax_ax2 = biax_axes.ax2;
    // The hero channel's base dispersion value and, from it, the hero indicatrix's
    // three principal indices (`optics::materials::GemMaterial::biaxial_indicatrix`'s
    // convention: `n_beta := dispersion.evaluate(lambda)`, `n_alpha := n_beta -
    // biaxial_delta_beta_alpha`, `n_gamma := n_alpha + birefringence_delta`). Computed
    // unconditionally: harmless when `!is_biaxial` since `biaxial_delta_beta_alpha` is
    // `0.0` for every non-biaxial material, so `n_alpha_hero == n_beta_hero ==
    // n_o_hero_seed`, and no `biaxial_*` function is called with these values unless
    // `is_biaxial` guards it.
    let n_o_hero_seed = dispersion_evaluate(material.dispersion.model_type, material.dispersion.param_a, material.dispersion.param_b, lambdas[0]);
    let n_beta_hero = n_o_hero_seed;
    let n_alpha_hero = n_beta_hero - material.dispersion.biaxial_delta_beta_alpha;
    let n_gamma_hero = n_alpha_hero + birefringence_delta;
    // P1/P5: the hero channel's ACCURATE extraordinary index -- a genuine independent
    // e-ray dispersion curve evaluation when the material carries one
    // (Quartz/Amethyst/Citrine/Rutile), else the constant-offset `n_o_hero_seed +
    // birefringence_delta` fallback -- mirroring `GemMaterial::extraordinary_index_at`
    // exactly.
    var n_e_hero_seed: f32;
    if (material.has_extraordinary_dispersion != 0u) {
        n_e_hero_seed = extraordinary_dispersion_evaluate(material.extraordinary_model_type, material.extraordinary_param_a, material.extraordinary_param_b, lambdas[0]);
    } else {
        n_e_hero_seed = n_o_hero_seed + birefringence_delta;
    }

    // `n_o_ch[k]` and the per-channel pleochroic absorption coefficients depend only on
    // `lambdas[k]` (fixed for the whole ray) and the material's dispersion/band data,
    // never on anything that varies per bounce.
    var n_o_hoisted: array<f32, 8>;
    var alpha_o_hoisted: array<f32, 8>;
    var alpha_e_hoisted: array<f32, 8>;
    var alpha_beta_hoisted: array<f32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        n_o_hoisted[k] = dispersion_evaluate(material.dispersion.model_type, material.dispersion.param_a, material.dispersion.param_b, lambdas[k]);
        alpha_o_hoisted[k] = spectral_absorption(material.o_ray_bands, material.o_ray_band_count, lambdas[k]);
        alpha_e_hoisted[k] = spectral_absorption(material.e_ray_bands, material.e_ray_band_count, lambdas[k]);
        alpha_beta_hoisted[k] = 0.0;
    }
    if (is_biaxial) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            alpha_beta_hoisted[k] = spectral_absorption(material.beta_ray_bands, material.beta_ray_band_count, lambdas[k]);
        }
    }

    // Studio rig key/fill/ring directions: a pure function of `params` alone. Unused
    // when `params.env_mode != transport_env_mode::STUDIO_RIG`.
    let studio_key_dir = studio_rig_key_dir(params.studio_light_yaw, params.studio_light_pitch);
    let studio_fill_dir = studio_rig_fill_dir(params.studio_light_yaw, params.studio_light_pitch);
    let studio_sin_lp = sin(params.studio_light_pitch);

    var result: RayConstants;
    result.c_axis = c_axis;
    result.birefringence_delta = birefringence_delta;
    result.is_anisotropic = is_anisotropic;
    result.is_biaxial = is_biaxial;
    result.biax_ax0 = biax_ax0;
    result.biax_ax1 = biax_ax1;
    result.biax_ax2 = biax_ax2;
    result.n_o_hero_seed = n_o_hero_seed;
    result.n_beta_hero = n_beta_hero;
    result.n_alpha_hero = n_alpha_hero;
    result.n_gamma_hero = n_gamma_hero;
    result.n_e_hero_seed = n_e_hero_seed;
    result.n_o_hoisted = n_o_hoisted;
    result.alpha_o_hoisted = alpha_o_hoisted;
    result.alpha_e_hoisted = alpha_e_hoisted;
    result.alpha_beta_hoisted = alpha_beta_hoisted;
    result.studio_key_dir = studio_key_dir;
    result.studio_fill_dir = studio_fill_dir;
    result.studio_sin_lp = studio_sin_lp;
    return result;
}
