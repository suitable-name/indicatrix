const RING_LIGHT_COUNT: u32 = 16u;

fn studio_rig_key_dir(light_yaw: f32, light_pitch: f32) -> vec3<f32> {
    let cos_lp = cos(light_pitch);
    let sin_lp = sin(light_pitch);
    let cos_ly = cos(light_yaw);
    let sin_ly = sin(light_yaw);
    return normalize(vec3<f32>(cos_lp * sin_ly, sin_lp, cos_lp * cos_ly));
}

fn studio_rig_fill_dir(light_yaw: f32, light_pitch: f32) -> vec3<f32> {
    let fill_yaw = fma(PI, 0.78, light_yaw);
    let fill_pitch = clamp(light_pitch * 0.65, 0.15, 1.2);
    return normalize(vec3<f32>(cos(fill_pitch) * sin(fill_yaw), sin(fill_pitch), cos(fill_pitch) * cos(fill_yaw)));
}

fn studio_rig_ring_dir(i: u32, light_yaw: f32, sin_lp: f32) -> vec3<f32> {
    let angle = fma(f32(i), PI * 2.0 / f32(RING_LIGHT_COUNT), light_yaw);
    return normalize(vec3<f32>(cos(angle) * 0.75, sin_lp * 0.8, sin(angle) * 0.75));
}

// optics::raytracer::environment::sample_light_tent -- needs `studio_rig_ring_dir` for
// the three black cards on ring slots 4/8/12, so it lives here rather than in the shared
// prelude with the other lit models.
fn sample_light_tent(
    d: vec3<f32>,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    light_yaw: f32,
    observer: vec3<f32>,
) -> f32 {
    let horizon = horizon_blend(d);
    var walls = fma(0.08, max(d.y, 0.0), 0.14);
    var card: f32 = 0.0;
    for (var slot: u32 = 4u; slot < RING_LIGHT_COUNT; slot = slot + 4u) {
        let card_dir = studio_rig_ring_dir(slot, light_yaw, sin_lp);
        card = max(card, smoothstep_f32(CARD_OUTER_COS, CARD_INNER_COS, dot(d, card_dir)));
    }
    walls = walls * fma(card, -0.9, 1.0);
    let key = smoothstep_f32(TENT_KEY_OUTER_COS, TENT_KEY_INNER_COS, dot(d, key_dir)) * (1.4 * spot_mult);
    let spark = smoothstep_f32(SPARK_OUTER_COS, SPARK_INNER_COS, dot(d, fill_dir)) * (5.0 * spot_mult);
    let above = ((walls + key) + spark) * (horizon * observer_visibility(d, observer));
    let ground = 0.02 * (1.0 - horizon);
    return (above + ground) * (spec_power * exposure);
}

fn sample_studio_rig(
    d: vec3<f32>,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    light_yaw: f32,
) -> f32 {
    let bg_val = max(fma(0.012, fma(d.y, 0.5, 0.5), 0.015), 0.005) * exposure;
    var radiance = bg_val * spec_power;

    let key_dot = max(dot(d, key_dir), 0.0);
    if (key_dot > 0.0) {
        let softbox = powi_u(key_dot, 28u) * 12.0 * spot_mult * exposure;
        radiance = fma(softbox, spec_power, radiance);
    }

    let fill_dot = max(dot(d, fill_dir), 0.0);
    if (fill_dot > 0.0) {
        let fill = powi_u(fill_dot, 18u) * 4.5 * exposure;
        radiance = fma(fill, spec_power, radiance);
    }

    for (var i: u32 = 0u; i < RING_LIGHT_COUNT; i = i + 1u) {
        let ring_dir = studio_rig_ring_dir(i, light_yaw, sin_lp);
        let ring_dot = max(dot(d, ring_dir), 0.0);
        if (ring_dot > 0.96) {
            let spark = (ring_dot - 0.96) / 0.04;
            let intensity = powi_u(spark, 6u) * 22.0 * spot_mult * exposure;
            radiance = fma(intensity, spec_power, radiance);
        }
    }

    return radiance;
}

fn studio_dispatch(
    model: u32,
    d: vec3<f32>,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    light_yaw: f32,
    observer: vec3<f32>,
) -> f32 {
    switch (model) {
        case 1u: {
            return sample_iso_hemisphere(d, spec_power, exposure, observer);
        }
        case 2u: {
            return sample_light_tent(d, spec_power, spot_mult, exposure, key_dir, fill_dir, sin_lp, light_yaw, observer);
        }
        case 3u: {
            return sample_daylight_dome(d, spec_power, exposure, key_dir, observer);
        }
        default: {
            return sample_studio_rig(d, spec_power, spot_mult, exposure, key_dir, fill_dir, sin_lp, light_yaw);
        }
    }
}

// `LightingPreset::spectral_power`: the tabulated CIE D65 curve for the D65 presets,
// else a Planckian fit at `studio_temp_k`.
fn studio_spectral_power(lambda_nm: f32) -> f32 {
    if (params.studio_use_d65 != 0u) {
        return d65_relative_spectral_power(lambda_nm);
    }
    return blackbody_spectrum(lambda_nm, params.studio_temp_k);
}

// `key_dir`/`fill_dir`/`sin_lp` (the `StudioRig`-equivalent quantities) are constant
// across an entire ray, so the caller (`transport_main`'s miss branch) computes them
// once before its `NUM_CHANNELS` loop and passes them in, rather than this function
// recomputing them on every per-channel call -- mirrors
// `optics::raytracer::accumulate_miss_radiance` building them once per ray.
fn sample_studio_environment_with_rig(
    dir_in: vec3<f32>,
    lambda_nm: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    observer: vec3<f32>,
) -> f32 {
    let d = normalize(dir_in);
    let spec_power = studio_spectral_power(lambda_nm);

    return studio_dispatch(
        params.studio_model,
        d,
        spec_power,
        params.studio_spot_mult,
        params.studio_exposure,
        key_dir,
        fill_dir,
        sin_lp,
        params.studio_light_yaw,
        observer,
    );
}

// renderer::env_map::EnvironmentMap::{direction_to_uv, sample_bilinear,
// radiance_at} -- ported op-for-op, including the exact `mul_add`/`fma` chains, so this
// stays within `environment_check`'s ULP budget for `hdr_env_radiance_at` (see that
// module's standalone `env_map_radiance_main` self-test kernel, a duplicate of this same
// logic exercised independently of the megakernel, matching this file's existing
// convention of not sharing code across WGSL modules -- see e.g. `blackbody_spectrum`).

// hdr_wrap_x, hdr_clamp_y, and hdr_direction_to_uv are defined in transport_physics.wgsl.

fn hdr_texel(x: u32, y: u32, width: u32) -> vec3<f32> {
    return hdr_texels[y * width + x].xyz;
}

// EnvironmentMap::sample_bilinear -- same `fx`/`fy` half-texel offset, same wrap/clamp
// neighbour selection, same per-component `mul_add` interpolation order.
fn hdr_env_sample_bilinear(u_in: f32, v_in: f32) -> vec3<f32> {
    let width = hdr_env_dims.width;
    let height = hdr_env_dims.height;
    let width_i = i32(width);
    let height_i = i32(height);

    let u_wrapped = fract(u_in);
    let v_clamped = clamp(v_in, 0.0, 1.0);
    let fx = fma(u_wrapped, f32(width), -0.5);
    let fy = fma(v_clamped, f32(height), -0.5);

    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;

    let x0i = hdr_wrap_x(i32(x0), width_i);
    let x1i = hdr_wrap_x(i32(x0) + 1, width_i);
    let y0i = hdr_clamp_y(i32(y0), height_i);
    let y1i = hdr_clamp_y(i32(y0) + 1, height_i);

    let p00 = hdr_texel(x0i, y0i, width);
    let p10 = hdr_texel(x1i, y0i, width);
    let p01 = hdr_texel(x0i, y1i, width);
    let p11 = hdr_texel(x1i, y1i, width);

    let top = fma(p10, vec3<f32>(tx), p00 * (1.0 - tx));
    let bottom = fma(p11, vec3<f32>(tx), p01 * (1.0 - tx));
    return fma(bottom, vec3<f32>(ty), top * (1.0 - ty));
}

// EnvironmentMap::radiance_at -- bilinear RGB lookup, then the same `rgb_to_spectral_radiance`
// spectral lift the uniform-furnace branch above already uses.
fn hdr_env_radiance_at(dir: vec3<f32>, lambda_nm: f32) -> f32 {
    let uv = hdr_direction_to_uv(dir);
    let rgb = hdr_env_sample_bilinear(uv.x, uv.y);
    return rgb_to_spectral_radiance(rgb.x, rgb.y, rgb.z, lambda_nm);
}

fn sample_environment_with_rig(
    dir: vec3<f32>,
    lambda_nm: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    observer: vec3<f32>,
) -> f32 {
    if (params.env_mode == 0u) {
        return rgb_to_spectral_radiance(params.l0, params.l0, params.l0, lambda_nm);
    } else if (params.env_mode == 2u) {
        return hdr_env_radiance_at(dir, lambda_nm);
    }
    return sample_studio_environment_with_rig(dir, lambda_nm, key_dir, fill_dir, sin_lp, observer);
}

// `dispersion_evaluate`, `spectral_absorption`, the four `mueller_*` Mueller-matrix
// constructors, `tir_phase_delta`, `normalize_or_zero`, `signed_frame_rotation_psi`,
// `degree_of_polarization`, `polarization_azimuth`, `arbitrary_perpendicular`,
// `electric_field_direction`, `stable_orthonormal_basis_t`,
// `ordinary_eigen_polarization`, `extraordinary_eigen_polarization`, `quadratic_form`,
// and `pleochroic_channel_alpha` all live in `shaders/transport_physics.wgsl`, the
// shared source `build.rs` concatenates ahead of this file. Look there, not here.

