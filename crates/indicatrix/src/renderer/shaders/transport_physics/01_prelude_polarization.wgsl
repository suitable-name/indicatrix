// Phase 2 shared physics prelude -- the SINGLE definition of every ported function
// consumed by BOTH `spectral_transport.wgsl` (the megakernel, the actually-shipped
// path) and `transport_functions.wgsl` (the standalone kernels Tier 2's per-function
// ULP checks exercise, driven by `renderer::gpu::transport_check`).
//
// # Why this file exists
//
// Before it existed, this physics was hand-copied into both shader files (WGSL has no
// `#include`). The copies could drift -- and once, demonstrably, did: an injected fault
// in `transport_functions.wgsl`'s copy was caught precisely by Tier 2 with an exact
// argmax diagnostic, while the SAME fault in the megakernel's own copy was caught only
// marginally by Tier 3 (a handful of isolated-singleton pixels that would have
// independently passed at that sample budget). Tier 2 was validating a duplicate, not
// the shipped code. See `renderer::gpu::transport_check`'s module doc comment for the
// full story, and the fault-injection re-run recorded there proving this file closes
// the gap.
//
// # How it's wired in
//
// `build.rs` concatenates this file's text ahead of `spectral_transport.wgsl` and
// `transport_functions.wgsl` at build time into `$OUT_DIR/*.generated.wgsl` -- those
// generated files, not the checked-in `.wgsl` files directly, are what
// `renderer::gpu::estimator_check` and `renderer::gpu::transport_check` `include_str!`.
// Neither `spectral_transport.wgsl` nor `transport_functions.wgsl` is valid WGSL in
// isolation any more: both assume every symbol defined here is already in scope, the
// same way a Rust module assumes its `use` imports resolve.
//
// `build.rs`'s `INDICATRIX_BUILD_ID` content hash walks `src/**/*.wgsl` on disk (this file
// included, since it lives under `src/renderer/shaders/`) -- NOT the generated
// `$OUT_DIR` output -- so the hash still fingerprints exactly the physics text that
// ships, just spread across one fewer duplicate copy than before.
//
// # Rules for editing this file
//
// Only functions/types genuinely shared VERBATIM between the megakernel and the Tier 2
// kernels belong here. Do not add bindings (`@group`/`@binding`) or entry points
// (`@compute`) -- those are necessarily per-file (the megakernel's bindings and the
// Tier 2 kernels' per-case bindings are entirely different shapes). A function that
// only one of the two files needs stays local to that file.
//
// P6 exit-event spectral splitting: the CPU-side physics
// (`compute_channel_transmission`/`compute_uniaxial_exit_transmission` in
// `optics::raytracer::refraction`) is ported here -- see the two functions of the same
// name further down this file (~line 1965) -- and wired into `spectral_transport.wgsl`'s
// megakernel and verified by `renderer::gpu::transport_check::p6_exit_splitting`'s Tier 2
// checks.

const PI: f32 = 3.14159265358979323846;

// ---------------------------------------------------------------------------------
// optics::raytracer::hash_u32 plus the per-bounce stream salts
// (FRESNEL_BRANCH_STREAM/RUSSIAN_ROULETTE_STREAM/BIREFRINGENT_SPLIT_STREAM/
// MODE_COUPLING_STREAM/FROSTED_DIR_U_STREAM/FROSTED_DIR_V_STREAM) and the frosted
// r_unpol clamp bounds (R_UNPOL_MIN/R_UNPOL_MAX).
//
// These constants (frosted girdle finish) live here rather than in
// `spectral_transport.wgsl` so `apply_frosted_bounce` below -- shared verbatim between
// the megakernel and Tier 2's `transport_functions.wgsl` -- has them in scope in EITHER
// concatenated file without a second copy of the constants themselves: there is exactly
// one definition per concatenated module, never two (WGSL rejects a duplicate
// top-level identifier).
const FRESNEL_BRANCH_STREAM: u32 = 0x9e3779b1u;
const RUSSIAN_ROULETTE_STREAM: u32 = 0x517cc1b7u;
const BIREFRINGENT_SPLIT_STREAM: u32 = 0x2545f491u;
const MODE_COUPLING_STREAM: u32 = 0xcc9e2d51u;
// Frosted girdle finish: the 2D cosine-weighted-hemisphere direction draw at a frosted
// bounce -- two independent streams for (u, v), mirroring
// optics::raytracer::{FROSTED_DIR_U_STREAM, FROSTED_DIR_V_STREAM}.
const FROSTED_DIR_U_STREAM: u32 = 0x27d4eb2fu;
const FROSTED_DIR_V_STREAM: u32 = 0x165667b1u;
// Next-event estimation environment direction draws.
// Mirrors optics::raytracer::sampling::{NEE_ENV_DIR_U_STREAM, NEE_ENV_DIR_V_STREAM,
// FROSTED_NEE_ENV_DIR_U_STREAM, FROSTED_NEE_ENV_DIR_V_STREAM}.
const NEE_ENV_DIR_U_STREAM: u32 = 0x4b725c19u;
const NEE_ENV_DIR_V_STREAM: u32 = 0x2f1e8a4du;
const FROSTED_NEE_ENV_DIR_U_STREAM: u32 = 0x6a09e667u;
const FROSTED_NEE_ENV_DIR_V_STREAM: u32 = 0xbb67ae85u;

const R_UNPOL_MIN: f32 = 1e-4;
const R_UNPOL_MAX: f32 = 1.0 - 1e-4;
const RAY_EPS: f32 = 1e-4;

fn hash_u32(x_in: u32) -> u32 {
    var x = x_in;
    x = x * 0x85ebca6bu;
    x = x ^ (x >> 13u);
    x = x * 0xc2b2ae35u;
    x = x ^ (x >> 16u);
    return x;
}

// ---------------------------------------------------------------------------------
// optics::polarization -- Stokes/Mueller matrix constructors. `StokesVector::
// apply_matrix` itself has no separate function to share: every call site below (in
// both consuming files) applies a matrix via the WGSL builtin `mat4x4<f32> *
// vec4<f32>` operator, so there is nothing hand-written to duplicate or dedupe there.
// ---------------------------------------------------------------------------------

// Fix 2 (see optics::polarization::MuellerMatrix::frame_rotation's doc comment): the
// CPU array was written row-major but fed to a column-major constructor, realizing
// R(-psi) instead of R(psi). WGSL's `mat4x4<f32>(col0, col1, col2, col3)` constructor
// is likewise column-major, so this mirrors the same fix -- column 1 gets `-s2` and
// column 2 gets `s2` (swapped from before) to build the textbook R(psi).
fn mueller_frame_rotation(psi: f32) -> mat4x4<f32> {
    let c2 = cos(2.0 * psi);
    let s2 = sin(2.0 * psi);
    return mat4x4<f32>(
        vec4<f32>(1.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, c2, -s2, 0.0),
        vec4<f32>(0.0, s2, c2, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 1.0),
    );
}

fn mueller_fresnel_reflection(r_s: f32, r_p: f32) -> mat4x4<f32> {
    let rs2 = r_s * r_s;
    let rp2 = r_p * r_p;
    let a = 0.5 * (rs2 + rp2);
    let b = 0.5 * (rs2 - rp2);
    let c = r_s * r_p;
    return mat4x4<f32>(
        vec4<f32>(a, b, 0.0, 0.0),
        vec4<f32>(b, a, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, c, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, c),
    );
}

fn mueller_fresnel_transmission(n1: f32, n2: f32, cos_i: f32, cos_t: f32, t_s: f32, t_p: f32) -> mat4x4<f32> {
    let factor = (n2 * cos_t) / max(n1 * cos_i, 1e-6);
    let ts2 = t_s * t_s * factor;
    let tp2 = t_p * t_p * factor;
    let a = 0.5 * (ts2 + tp2);
    let b = 0.5 * (ts2 - tp2);
    let c = t_s * t_p * factor;
    return mat4x4<f32>(
        vec4<f32>(a, b, 0.0, 0.0),
        vec4<f32>(b, a, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, c, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, c),
    );
}

fn mueller_tir_retardation(delta: f32) -> mat4x4<f32> {
    let cos_d = cos(delta);
    let sin_d = sin(delta);
    return mat4x4<f32>(
        vec4<f32>(1.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 1.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, cos_d, -sin_d),
        vec4<f32>(0.0, 0.0, sin_d, cos_d),
    );
}

// optics::raytracer::tir_phase_delta -- TIR phase retardation delta = delta_p - delta_s.
fn tir_phase_delta(n1k: f32, cos_i: f32, sin_i: f32) -> f32 {
    let a = n1k * n1k * sin_i;
    let inner = max(fma(a, sin_i, -1.0), 0.0);
    let tan_half_delta_k = (cos_i * sqrt(inner)) / max(n1k * sin_i * sin_i, 1e-6);
    return 2.0 * atan(tan_half_delta_k);
}

fn normalize_or_zero(v: vec3<f32>) -> vec3<f32> {
    let l2 = dot(v, v);
    if (l2 > 1e-30) {
        return v / sqrt(l2);
    }
    return vec3<f32>(0.0, 0.0, 0.0);
}

// optics::raytracer::signed_frame_rotation_psi -- signed plane-of-incidence rotation
// angle between consecutive bounces, via atan2 (Fix 1 in the megakernel's own doc
// comment).
fn signed_frame_rotation_psi(prev: vec3<f32>, curr: vec3<f32>, axis: vec3<f32>) -> f32 {
    let cos_psi = clamp(dot(prev, curr), -1.0, 1.0);
    let sin_psi = dot(cross(prev, curr), normalize_or_zero(axis));
    return atan2(sin_psi, cos_psi);
}

fn degree_of_polarization(s: vec4<f32>) -> f32 {
    if (s.x <= 1e-7) {
        return 0.0;
    }
    let mag = sqrt(fma(s.w, s.w, fma(s.z, s.z, s.y * s.y)));
    return clamp(mag / s.x, 0.0, 1.0);
}

fn polarization_azimuth(s: vec4<f32>) -> f32 {
    return 0.5 * atan2(s.z, s.y);
}

fn arbitrary_perpendicular(n: vec3<f32>) -> vec3<f32> {
    var a: vec3<f32>;
    if (abs(n.x) > 0.9) {
        a = vec3<f32>(0.0, 1.0, 0.0);
    } else {
        a = vec3<f32>(1.0, 0.0, 0.0);
    }
    return normalize_or_zero(a - n * dot(n, a));
}

// optics::polarization::electric_field_direction
fn electric_field_direction(s: vec4<f32>, s_axis: vec3<f32>, propagation_dir: vec3<f32>) -> vec3<f32> {
    let k_hat = normalize_or_zero(propagation_dir);
    let s_raw = s_axis - k_hat * dot(k_hat, s_axis);
    var s_hat: vec3<f32>;
    if (dot(s_raw, s_raw) > 1e-8) {
        s_hat = normalize(s_raw);
    } else {
        s_hat = arbitrary_perpendicular(k_hat);
    }
    let p_hat = cross(k_hat, s_hat);
    let psi = polarization_azimuth(s);
    let e = cos(psi) * s_hat + sin(psi) * p_hat;
    if (dot(e, e) > 1e-8) {
        return normalize(e);
    }
    return s_hat;
}

// ---------------------------------------------------------------------------------
// optics::birefringence -- the uniaxial pleochroic absorption path (exercised even
// for an isotropic material -- see `spectral_transport.wgsl`'s header comment).
// ---------------------------------------------------------------------------------

// `arbitrary_perpendicular` and this are two CPU-side names for the identical
// construction (a stable orthonormal vector perpendicular to `n`); kept as distinct
// WGSL functions here, as in both files before this dedup, to mirror the CPU naming
// each call site is ported from.
fn stable_orthonormal_basis_t(n: vec3<f32>) -> vec3<f32> {
    var a: vec3<f32>;
    if (abs(n.x) > 0.9) {
        a = vec3<f32>(0.0, 1.0, 0.0);
    } else {
        a = vec3<f32>(1.0, 0.0, 0.0);
    }
    return normalize_or_zero(a - n * dot(n, a));
}

fn ordinary_eigen_polarization(wave_normal: vec3<f32>, c_axis: vec3<f32>) -> vec3<f32> {
    let crs = cross(wave_normal, c_axis);
    if (dot(crs, crs) > 1e-8) {
        return normalize(crs);
    }
    return stable_orthonormal_basis_t(normalize_or_zero(wave_normal));
}

fn extraordinary_eigen_polarization(wave_normal: vec3<f32>, c_axis: vec3<f32>) -> vec3<f32> {
    let o_hat = ordinary_eigen_polarization(wave_normal, c_axis);
    return normalize_or_zero(cross(wave_normal, o_hat));
}

fn quadratic_form(alpha_o: f32, alpha_e: f32, a1: vec3<f32>, a2: vec3<f32>, c: vec3<f32>, e_hat: vec3<f32>) -> f32 {
    let l0 = dot(a1, e_hat);
    let l1 = dot(a2, e_hat);
    let l2 = dot(c, e_hat);
    return alpha_o * (l0 * l0) + alpha_o * (l1 * l1) + alpha_e * (l2 * l2);
}

fn pleochroic_channel_alpha(
    alpha_o: f32,
    alpha_e: f32,
    c_axis: vec3<f32>,
    s_axis: vec3<f32>,
    propagation_dir: vec3<f32>,
    eigen_a: vec3<f32>,
    eigen_b: vec3<f32>,
    s: vec4<f32>,
) -> f32 {
    let c = normalize_or_zero(c_axis);
    let a1 = stable_orthonormal_basis_t(c);
    let a2 = cross(c, a1);
    let e_hat = electric_field_direction(s, s_axis, propagation_dir);
    let alpha_polarized = quadratic_form(alpha_o, alpha_e, a1, a2, c, e_hat);
    let alpha_unpolarized = 0.5 * (quadratic_form(alpha_o, alpha_e, a1, a2, c, eigen_a) + quadratic_form(alpha_o, alpha_e, a1, a2, c, eigen_b));
    let p = clamp(degree_of_polarization(s), 0.0, 1.0);
    return fma(p, alpha_polarized - alpha_unpolarized, alpha_unpolarized);
}

// ---------------------------------------------------------------------------------
// Phase 3 -- optics::birefringence::BirefringenceParams::{effective_extraordinary_index,
// walk_off_angle, extraordinary_poynting_dir} plus
// optics::raytracer::{theta_c_for_bounce, per_channel_uniaxial_indices} (the latter as a
// PER-CHANNEL function, `per_channel_uniaxial_index`, called once per channel by the
// caller -- see this file's own doc comment: the CPU original loops over
// `NUM_CHANNELS` internally, WGSL callers do that looping themselves and call this once
// per iteration, so the shared body stays the single source of truth for one channel's
// computation either way).
//
// `theta_c_for_bounce` below still omits the CPU function's `is_biaxial` parameter and
// always takes the `!inside_gem && is_anisotropic` branch: for a genuinely biaxial
// material (see the Phase 4 section further down for `BiaxialIndicatrix`'s own port)
// its output -- and `per_channel_uniaxial_index`'s below -- is a provably DEAD value.
// `spectral_transport.wgsl`'s `transport_main` selects the medium index for a bounce
// from the biaxial mode-A/mode-B arrays instead whenever `is_biaxial` is true, so the
// uniaxial `n_o_ch`/`n_eff_ch` this function's result feeds into is computed but never
// read for a biaxial material -- exactly the same "harmless, unused, still cheap"
// pattern this crate's CPU side documents elsewhere (see e.g.
// `BounceRefractionGeometry`'s uniaxial/biaxial field pairs). Taking the CPU's
// `!is_biaxial`-aware branch here would compute a DIFFERENT (also-unused) theta, but
// never a NaN/Inf one -- `n_o_hero_seed`/`birefringence_delta` are always finite,
// well-scaled index values regardless of crystal system -- so this simplification costs
// nothing observable in rendered output while avoiding a signature change to an
// already-verified, already-ULP-checked function.
// ---------------------------------------------------------------------------------

fn effective_extraordinary_index(n_o: f32, n_e: f32, theta: f32) -> f32 {
    if (abs(n_o - n_e) < 1e-5) {
        return n_o;
    }
    let sin_t = sin(theta);
    let cos_t = cos(theta);
    let ne_cos = n_e * cos_t;
    let no_sin = n_o * sin_t;
    let denom_sq = fma(ne_cos, ne_cos, no_sin * no_sin);
    if (denom_sq <= 1e-8) {
        return n_o;
    }
    return (n_o * n_e) / sqrt(denom_sq);
}

fn walk_off_angle(n_o: f32, n_e: f32, theta: f32) -> f32 {
    if (abs(n_o - n_e) < 1e-5) {
        return 0.0;
    }
    let n_o2 = n_o * n_o;
    let n_e2 = n_e * n_e;
    let sin_t = sin(theta);
    let cos_t = cos(theta);
    let numer = (n_o2 - n_e2) * sin_t * cos_t;
    let denom = max(fma(n_e2 * cos_t, cos_t, n_o2 * sin_t * sin_t), 1e-6);
    let tan_rho = numer / denom;
    return atan(tan_rho);
}

// Fix 1 (see optics::birefringence::BirefringenceParams::extraordinary_poynting_dir's
// doc comment): the optic axis is a director (S(c) == S(-c)), so fold `c_axis` onto the
// wave normal's own hemisphere via `sign` and negate the tilt on that branch to
// compensate -- `theta`/`delta` already use the unsigned `|cos_theta|` and so are
// branch-independent, but `c_proj` is built from the SIGNED `cos_theta` and flips sign
// under `c_axis -> -c_axis` while `delta` does not.
fn extraordinary_poynting_dir(wave_normal: vec3<f32>, c_axis: vec3<f32>, n_o: f32, n_e: f32) -> vec3<f32> {
    let cos_theta = clamp(dot(wave_normal, c_axis), -1.0, 1.0);
    let theta = acos(abs(cos_theta));
    let delta = walk_off_angle(n_o, n_e, theta);

    if (abs(delta) < 1e-5) {
        return wave_normal;
    }

    let c_proj = normalize_or_zero(c_axis - cos_theta * wave_normal);
    if (dot(c_proj, c_proj) < 1e-6) {
        return wave_normal;
    }
    var sign: f32 = 1.0;
    if (cos_theta < 0.0) {
        sign = -1.0;
    }

    return normalize(wave_normal * cos(delta) - sign * c_proj * sin(delta));
}

// optics::raytracer::theta_c_for_bounce -- see this section's header comment for why
// `is_biaxial` is omitted (always false for any material the GPU ever sees).
//
// P5: takes the precomputed hero e-index `n_e_hero_seed` directly (computed once per ray
// by the caller via `extraordinary_dispersion_evaluate` when the material carries a
// genuine independent e-ray curve, else the constant-offset `n_o_hero_seed +
// birefringence_delta` -- mirroring `GemMaterial::extraordinary_index_at`) rather than
// hardcoding the constant-offset form itself, matching the CPU-side fix in
// `optics::raytracer::refraction::theta_c_for_bounce`.
fn theta_c_for_bounce(
    normal: vec3<f32>,
    ray_dir: vec3<f32>,
    cos_i: f32,
    inside_gem: bool,
    is_anisotropic: bool,
    c_axis: vec3<f32>,
    n_o_hero_seed: f32,
    n_e_hero_seed: f32,
) -> f32 {
    if (!inside_gem && is_anisotropic) {
        var n_guess = n_o_hero_seed;
        var theta: f32 = 0.0;
        for (var i: u32 = 0u; i < 2u; i = i + 1u) {
            let eta_guess = 1.0 / n_guess;
            let sin2_t_guess = eta_guess * eta_guess * fma(-cos_i, cos_i, 1.0);
            if (sin2_t_guess > 1.0) {
                break;
            }
            let cos_t_guess = sqrt(max(1.0 - sin2_t_guess, 0.0));
            let wave_dir_guess = normalize(eta_guess * ray_dir + fma(eta_guess, cos_i, -cos_t_guess) * normal);
            let cos_theta_wave = abs(clamp(dot(wave_dir_guess, c_axis), -1.0, 1.0));
            theta = acos(cos_theta_wave);
            n_guess = effective_extraordinary_index(n_o_hero_seed, n_e_hero_seed, theta);
        }
        return theta;
    } else {
        return acos(abs(clamp(dot(ray_dir, c_axis), -1.0, 1.0)));
    }
}
