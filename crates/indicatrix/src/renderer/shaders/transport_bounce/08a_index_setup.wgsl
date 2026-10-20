// ---------------------------------------------------------------------------------
// Per-bounce phase functions and transport_bounce_step composition.
//
// Refactored per plan_web_gpu_2026-10-16.md §4.1: splits the single monolithic
// bounce step into pure index setup, extend, scatter, surface, 5 interaction arms,
// deferred exit-split resolution, and tail phases.
// ---------------------------------------------------------------------------------

struct IndexSetup {
    cos_i: f32,
    sin_i: f32,
    theta_c: f32,
    n_eff_ch: array<f32, 8>,
    n_e_raw_ch: array<f32, 8>,
    n_o_hero: f32,
    n_e_hero: f32,
    uframe: UniaxialFrameW,
    uinc_frame: EntryIncidenceFrameW,
    uniaxial_active: bool,
    uniaxial_nondegenerate: bool,
    wave_dir_a_hero: vec3<f32>,
    wave_dir_b_hero: vec3<f32>,
    n_biax_a_ch: array<f32, 8>,
    n_biax_b_ch: array<f32, 8>,
    n_alpha_ch: array<f32, 8>,
    n_gamma_ch: array<f32, 8>,
    n_biax_a_hero: f32,
    n_biax_b_hero: f32,
    use_mode_a_medium: bool,
    n_medium_ch: array<f32, 8>,
    n_medium_hero: f32,
    n1: f32,
    n2: f32,
    eta: f32,
    sin2_t: f32,
}

// Pure per-bounce optical and index setup. Reads ray and hit state; writes nothing.
fn bounce_index_setup(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
) -> IndexSetup {
    let wave_dir_at_bounce = (*h).wave_dir_at_bounce;
    let normal = (*h).shading_normal;
    let is_anisotropic = rc.is_anisotropic;
    let is_biaxial = rc.is_biaxial;
    let c_axis = rc.c_axis;
    let n_o_hero_seed = rc.n_o_hero_seed;
    let n_e_hero_seed = rc.n_e_hero_seed;
    let n_o_hoisted = rc.n_o_hoisted;
    let birefringence_delta = rc.birefringence_delta;
    let biax_ax0 = rc.biax_ax0;
    let biax_ax1 = rc.biax_ax1;
    let biax_ax2 = rc.biax_ax2;
    let n_alpha_hero = rc.n_alpha_hero;
    let n_beta_hero = rc.n_beta_hero;
    let n_gamma_hero = rc.n_gamma_hero;

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
    let theta_c = theta_c_for_bounce(normal, wave_dir_at_bounce, cos_i, (*s).inside_gem, is_anisotropic, c_axis, n_o_hero_seed, n_e_hero_seed);

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
        n_eff_ch[k] = 0.0;
        n_e_raw_ch[k] = 0.0;
    }
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        var n_eff_k = n_o_hoisted[k];
        if (is_anisotropic) {
            // Gated on the anisotropy flag (PERF-9): every read of `n_e_raw_ch` below
            // sits under `uniaxial_nondegenerate` (which implies `is_anisotropic`);
            // the isotropic path keeps zero-initialised entries it never reads.
            var n_e_k: f32;
            if (material.has_extraordinary_dispersion != 0u) {
                n_e_k = extraordinary_dispersion_evaluate(material.extraordinary_model_type, material.extraordinary_param_a, material.extraordinary_param_b, (*s).lambdas[k]);
            } else {
                n_e_k = n_o_hoisted[k] + birefringence_delta;
            }
            n_e_raw_ch[k] = n_e_k;
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
    // uniaxial" contract. PERF-11: built only under `uniaxial_nondegenerate` (below),
    // because every read of `uframe`/`uinc_frame` sits inside that guard; the
    // zero-initialised values are never read otherwise. `entry_incidence_frame`
    // (`n1 == 1.0` fixed for air->crystal entry) is built with it though only the
    // entry arm consumes it.
    var uframe: UniaxialFrameW;
    uframe.that = vec3<f32>(0.0, 0.0, 0.0);
    uframe.zhat = vec3<f32>(0.0, 0.0, 0.0);
    uframe.s_axis = vec3<f32>(0.0, 0.0, 0.0);
    uframe.p_axis = vec3<f32>(0.0, 0.0, 0.0);
    uframe.cos_i = 0.0;
    uframe.sin_i = 0.0;
    uframe.alpha = 0.0;
    uframe.beta = 0.0;
    uframe.gamma = 0.0;

    var uinc_frame: EntryIncidenceFrameW;
    for (var i: u32 = 0u; i < 4u; i = i + 1u) {
        uinc_frame.incident_s[i] = cplx_zero();
        uinc_frame.incident_p[i] = cplx_zero();
        uinc_frame.reflected_s[i] = cplx_zero();
        uinc_frame.reflected_p[i] = cplx_zero();
    }

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
    if (uniaxial_nondegenerate) {
        uframe = uniaxial_frame_build(wave_dir_at_bounce, normal, c_axis, cos_i, sin_i);
        uinc_frame = entry_incidence_frame(1.0, uframe);
    }

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
        if ((*s).inside_gem) {
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
    let use_mode_a_medium = is_anisotropic && (*s).inside_gem && !(*s).is_extraordinary;
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
    let n1 = select(1.0, n_medium_hero, (*s).inside_gem);
    let n2 = select(n_medium_hero, 1.0, (*s).inside_gem);
    let eta = n1 / n2;
    let sin2_t = eta * eta * fma(-cos_i, cos_i, 1.0);

    var idx: IndexSetup;
    idx.cos_i = cos_i;
    idx.sin_i = sin_i;
    idx.theta_c = theta_c;
    idx.n_eff_ch = n_eff_ch;
    idx.n_e_raw_ch = n_e_raw_ch;
    idx.n_o_hero = n_o_hero;
    idx.n_e_hero = n_e_hero;
    idx.uframe = uframe;
    idx.uinc_frame = uinc_frame;
    idx.uniaxial_active = uniaxial_active;
    idx.uniaxial_nondegenerate = uniaxial_nondegenerate;
    idx.wave_dir_a_hero = wave_dir_a_hero;
    idx.wave_dir_b_hero = wave_dir_b_hero;
    idx.n_biax_a_ch = n_biax_a_ch;
    idx.n_biax_b_ch = n_biax_b_ch;
    idx.n_alpha_ch = n_alpha_ch;
    idx.n_gamma_ch = n_gamma_ch;
    idx.n_biax_a_hero = n_biax_a_hero;
    idx.n_biax_b_hero = n_biax_b_hero;
    idx.use_mode_a_medium = use_mode_a_medium;
    idx.n_medium_ch = n_medium_ch;
    idx.n_medium_hero = n_medium_hero;
    idx.n1 = n1;
    idx.n2 = n2;
    idx.eta = eta;
    idx.sin2_t = sin2_t;
    return idx;
}

