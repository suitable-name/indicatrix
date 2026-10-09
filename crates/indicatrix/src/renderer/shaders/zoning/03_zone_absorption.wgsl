// ---------------------------------------------------------------------------------
// Zoned absorption, unit 3 of 3: the zoned interior segment. `zoning` feature only.
//
// WGSL twin of `optics::raytracer::zoned::segment_optical_depths` plus the exp attenuation
// of `absorption::apply_segment_absorption`. It replaces the homogeneous
// `exp(-alpha * path)` of one interior segment by
//
//     exp(-sum_z alpha_z * len_z)        (alpha_z per channel, len_z in mm)
//
// `build.rs` makes `transport_bounce_step` call it (instead of the homogeneous absorption
// block) when `material.zones.zone_count > 0`; see the anchor patches in `build.rs`.
//
// Frame / unit chain (identical to the CPU, see the header of `raytracer/zoned.rs`):
//   model units --x absorption_path_scale--> stone mm --zone frame inverse (inside
//   `zone_lengths`)--> zone frame. The kernel's lengths are used as FRACTIONS of the
//   segment: len_z = scaled * (raw_z / sum(raw)) with scaled = hit_t * absorption_path_scale,
//   so a segment inside one zone gets exactly `scaled`, bit for bit the unzoned product.
//
// Per-zone alphas follow `channel_absorption_alphas_assigned` exactly: the assigned
// eigenmode on an anisotropic host, the eigenmode midpoint on an isotropic-by-symmetry one.
// Zone 0 (the base zone) uses the per-ray hoisted arrays, which `GpuGemMaterial::encode`
// filled from the zoning's base tensor; zones 1..=4 evaluate their own band sets here.
// ---------------------------------------------------------------------------------

// Per-zone path length in mm of the segment origin + dir * t, t in [0, hit_t] (model
// units): `ZonedCache::lengths_mm` with t0 = 0.
fn zone_lengths_mm(origin: vec3<f32>, dir: vec3<f32>, hit_t: f32, path_scale: f32) -> array<f32, 5> {
    var out: array<f32, 5>;
    let scaled = hit_t * path_scale;
    if (!(scaled > 0.0)) {
        return out;
    }
    let seg_a = origin * path_scale;
    let seg_b = (origin + dir * hit_t) * path_scale;
    let raw = zone_lengths(seg_a, seg_b);
    let total = raw[0] + raw[1] + raw[2] + raw[3] + raw[4];
    if (!(total > 0.0)) {
        // A degenerate kernel answer: charge the base zone rather than lose the path.
        out[0] = scaled;
        return out;
    }
    for (var i: u32 = 0u; i < 5u; i = i + 1u) {
        out[i] = scaled * (raw[i] / total);
    }
    return out;
}

// One zone's absorption coefficient for one channel: the same three-way branch the
// homogeneous absorption block uses (`transport_bounce_step`), parameterised by the zone's
// own principal-direction coefficients.
fn zone_channel_alpha(
    alpha_o: f32,
    alpha_e: f32,
    alpha_beta: f32,
    has_beta: bool,
    is_anisotropic: bool,
    is_biaxial: bool,
    c_axis: vec3<f32>,
    k_hat: vec3<f32>,
    is_extraordinary: bool,
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    biax_ax0: vec3<f32>,
    biax_ax1: vec3<f32>,
    biax_ax2: vec3<f32>,
    n_o_hero_seed: f32,
    n_e_hero_seed: f32,
    eigen_a: vec3<f32>,
    eigen_b: vec3<f32>,
) -> f32 {
    if (is_anisotropic) {
        if (is_biaxial && has_beta) {
            return assigned_mode_alpha_biaxial(
                alpha_o, alpha_beta, alpha_e,
                n_alpha, n_beta, n_gamma, biax_ax0, biax_ax1, biax_ax2,
                c_axis, k_hat, is_extraordinary,
            );
        }
        return assigned_mode_alpha_uniaxial(
            alpha_o, alpha_e, c_axis, k_hat, is_extraordinary, n_o_hero_seed, n_e_hero_seed,
        );
    }
    return isotropic_channel_alpha(alpha_o, alpha_e, c_axis, eigen_a, eigen_b);
}

// Applies the zoned absorption of one interior segment to `stokes`.
fn zoned_interior_absorption(
    hit_t: f32,
    seg_origin: vec3<f32>,
    seg_dir: vec3<f32>,
    k_hat: vec3<f32>,
    lambdas: ptr<function, array<f32, 8>>,
    is_anisotropic: bool,
    is_biaxial: bool,
    c_axis: vec3<f32>,
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    biax_ax0: vec3<f32>,
    biax_ax1: vec3<f32>,
    biax_ax2: vec3<f32>,
    n_o_hero_seed: f32,
    n_e_hero_seed: f32,
    is_extraordinary: bool,
    alpha_o_hoisted: array<f32, 8>,
    alpha_e_hoisted: array<f32, 8>,
    alpha_beta_hoisted: array<f32, 8>,
    stokes: ptr<function, array<vec4<f32>, 8>>,
) {
    let lens = zone_lengths_mm(seg_origin, seg_dir, hit_t, material.absorption_path_scale);
    let count = material.zones.header.zone_count;

    // The isotropic-by-symmetry branch's eigen directions depend on the ray only.
    var eigen_a = vec3<f32>(0.0, 0.0, 0.0);
    var eigen_b = vec3<f32>(0.0, 0.0, 0.0);
    if (!is_anisotropic) {
        eigen_a = ordinary_eigen_polarization(k_hat, c_axis);
        eigen_b = extraordinary_eigen_polarization(k_hat, c_axis);
    }

    // sum_z alpha_z * len_z per channel, accumulated with fma in zone order like the CPU's
    // `optical_depths`. A zone with no path contributes alpha * 0 = 0 exactly, so it is skipped.
    var depth: array<f32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        depth[k] = 0.0;
    }
    if (lens[0] > 0.0) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            let alpha = zone_channel_alpha(
                alpha_o_hoisted[k], alpha_e_hoisted[k], alpha_beta_hoisted[k],
                material.has_beta_ray != 0u,
                is_anisotropic, is_biaxial, c_axis, k_hat, is_extraordinary,
                n_alpha, n_beta, n_gamma, biax_ax0, biax_ax1, biax_ax2,
                n_o_hero_seed, n_e_hero_seed, eigen_a, eigen_b,
            );
            depth[k] = fma(alpha, lens[0], depth[k]);
        }
    }
    for (var zi: u32 = 0u; zi < count; zi = zi + 1u) {
        let len_z = lens[zi + 1u];
        if (!(len_z > 0.0)) {
            continue;
        }
        let has_beta = is_biaxial && material.zones.absorption[zi].has_beta_ray != 0u;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            let lambda = (*lambdas)[k];
            let a_o = spectral_absorption(
                material.zones.absorption[zi].o_ray_bands,
                material.zones.absorption[zi].o_ray_band_count,
                lambda,
            );
            let a_e = spectral_absorption(
                material.zones.absorption[zi].e_ray_bands,
                material.zones.absorption[zi].e_ray_band_count,
                lambda,
            );
            var a_beta: f32 = 0.0;
            if (has_beta) {
                a_beta = spectral_absorption(
                    material.zones.absorption[zi].beta_ray_bands,
                    material.zones.absorption[zi].beta_ray_band_count,
                    lambda,
                );
            }
            let alpha = zone_channel_alpha(
                a_o, a_e, a_beta, has_beta,
                is_anisotropic, is_biaxial, c_axis, k_hat, is_extraordinary,
                n_alpha, n_beta, n_gamma, biax_ax0, biax_ax1, biax_ax2,
                n_o_hero_seed, n_e_hero_seed, eigen_a, eigen_b,
            );
            depth[k] = fma(alpha, len_z, depth[k]);
        }
    }

    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        // `exp_poly`, not the `exp()` builtin: the CPU's `apply_absorption` calls
        // `crate::simd::exp_f32x8`.
        (*stokes)[k] = (*stokes)[k] * exp_poly(-depth[k]);
    }
}
