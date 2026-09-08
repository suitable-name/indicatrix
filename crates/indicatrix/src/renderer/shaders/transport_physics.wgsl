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
// P6 exit-event spectral splitting (2026-09-07): the CPU-side physics this pass added
// (`compute_channel_transmission`/`compute_uniaxial_exit_transmission` in
// `optics::raytracer::refraction`) IS ported here -- see the two functions of the same
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
// Task 2 GPU port (frosted girdle finish) moved these here from
// `spectral_transport.wgsl` (which used to define its own copy, the only consumer
// until now) so `apply_frosted_bounce` below -- shared verbatim between the megakernel
// and Tier 2's `transport_functions.wgsl` -- has them in scope in EITHER concatenated
// file without a second copy of the constants themselves. A pure move, not a value
// change: `spectral_transport.wgsl` no longer defines these (see that file's own
// comment at the old location) so there is exactly one definition per concatenated
// module, never two (WGSL rejects a duplicate top-level identifier).
const FRESNEL_BRANCH_STREAM: u32 = 0x9e3779b1u;
const RUSSIAN_ROULETTE_STREAM: u32 = 0x517cc1b7u;
const BIREFRINGENT_SPLIT_STREAM: u32 = 0x2545f491u;
const MODE_COUPLING_STREAM: u32 = 0xcc9e2d51u;
// Task 2 (girdle finish): the 2D cosine-weighted-hemisphere direction draw at a frosted
// bounce -- two independent streams for (u, v), mirroring
// optics::raytracer::{FROSTED_DIR_U_STREAM, FROSTED_DIR_V_STREAM}.
const FROSTED_DIR_U_STREAM: u32 = 0x27d4eb2fu;
const FROSTED_DIR_V_STREAM: u32 = 0x165667b1u;
// Finding G7/G8: next-event estimation environment direction draws.
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

// ---------------------------------------------------------------------------------
// Phase 4: optics::birefringence::BiaxialIndicatrix -- the genuinely biaxial
// (three-distinct-principal-index) generalization of the uniaxial machinery above.
// Every function here is a direct, line-for-line port of the corresponding
// `BiaxialIndicatrix` method or free function, taking the indicatrix's three fields
// (`n_alpha`, `n_beta`, `n_gamma`, `axes` -- flattened to `ax0`/`ax1`/`ax2`, the three
// world-space principal-axis columns, alpha/beta/gamma respectively, matching
// `Mat3::from_cols(a1, a2, g)` in `BiaxialIndicatrix::from_gamma_axis`) as explicit
// parameters rather than reading a struct binding, mirroring `dispersion_evaluate`'s
// "one shared body, two different binding shapes" convention above -- the megakernel
// builds `ax0`/`ax1`/`ax2` once per ray via `biaxial_axes_from_gamma` (since they depend
// only on `c_axis`, constant across a ray's bounces) and Tier 2's per-case kernels build
// them once per case the same way.
// ---------------------------------------------------------------------------------

struct BiaxialAxes {
    ax0: vec3<f32>,
    ax1: vec3<f32>,
    ax2: vec3<f32>,
}

// optics::birefringence::BiaxialIndicatrix::from_gamma_axis's axis-frame construction
// (`stable_orthonormal_basis(gamma_axis)` completed to a right-handed orthonormal
// triple) -- pulled out on its own since every ported function below needs it and it
// depends only on `c_axis`/`gamma_axis`, never on wavelength or wave normal.
fn biaxial_axes_from_gamma(gamma_axis: vec3<f32>) -> BiaxialAxes {
    let g = normalize_or_zero(gamma_axis);
    let a1 = stable_orthonormal_basis_t(g);
    let a2 = cross(g, a1);
    var result: BiaxialAxes;
    result.ax0 = a1;
    result.ax1 = a2;
    result.ax2 = g;
    return result;
}

fn biaxial_b_coeffs(n_alpha: f32, n_beta: f32, n_gamma: f32) -> vec3<f32> {
    return vec3<f32>(1.0 / (n_alpha * n_alpha), 1.0 / (n_beta * n_beta), 1.0 / (n_gamma * n_gamma));
}

// optics::birefringence::BiaxialIndicatrix::indices_are_degenerate
fn biaxial_indices_degenerate(a: f32, b: f32) -> bool {
    let scale = max(max(abs(a), abs(b)), 1.0);
    return abs(a - b) <= sqrt(1.1920929e-7) * scale;
}

// optics::birefringence::BiaxialIndicatrix::uniaxial_wave_indices
fn biaxial_uniaxial_wave_indices(k_hat: vec3<f32>, axis: vec3<f32>, n_o: f32, n_e: f32) -> vec2<f32> {
    let theta = acos(abs(clamp(dot(k_hat, axis), -1.0, 1.0)));
    let n_eff = effective_extraordinary_index(n_o, n_e, theta);
    return vec2<f32>(max(n_o, n_eff), min(n_o, n_eff));
}

// optics::birefringence::BiaxialIndicatrix::wave_indices -- returns (n_slow, n_fast).
fn biaxial_wave_indices(
    n_alpha: f32, n_beta: f32, n_gamma: f32,
    ax0: vec3<f32>, ax1: vec3<f32>, ax2: vec3<f32>,
    wave_normal: vec3<f32>,
) -> vec2<f32> {
    let k = normalize_or_zero(wave_normal);

    let ab_degen = biaxial_indices_degenerate(n_alpha, n_beta);
    let bg_degen = biaxial_indices_degenerate(n_beta, n_gamma);

    if (ab_degen && bg_degen) {
        let n = (n_alpha + n_beta + n_gamma) / 3.0;
        return vec2<f32>(n, n);
    }
    if (ab_degen) {
        return biaxial_uniaxial_wave_indices(k, ax2, 0.5 * (n_alpha + n_beta), n_gamma);
    }
    if (bg_degen) {
        return biaxial_uniaxial_wave_indices(k, ax0, 0.5 * (n_beta + n_gamma), n_alpha);
    }

    let local = vec3<f32>(dot(ax0, k), dot(ax1, k), dot(ax2, k));
    let a2 = local.x * local.x;
    let b2 = local.y * local.y;
    let g2 = local.z * local.z;
    let bc = biaxial_b_coeffs(n_alpha, n_beta, n_gamma);

    let big_b = fma(a2, bc.y + bc.z, fma(b2, bc.x + bc.z, g2 * (bc.x + bc.y)));
    let big_c = fma(a2, bc.y * bc.z, fma(b2, bc.x * bc.z, g2 * bc.x * bc.y));
    let disc = sqrt(max(fma(big_b, big_b, -4.0 * big_c), 0.0));

    let x_lo = 0.5 * (big_b - disc);
    let x_hi = 0.5 * (big_b + disc);

    let n_slow = 1.0 / sqrt(max(x_lo, 1e-12));
    let n_fast = 1.0 / sqrt(max(x_hi, 1e-12));
    return vec2<f32>(n_slow, n_fast);
}

// Mirrors optics::birefringence::cross_fma / dot_fma op-for-op: explicit `fma` (not
// plain `*`/`-`/`dot`/`cross`) so this rounds identically to the CPU side -- see
// BiaxialIndicatrix::eigenvector_world's doc comment for why bit-parity matters here.
fn cross_fma(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        fma(a.y, b.z, -(a.z * b.y)),
        fma(a.z, b.x, -(a.x * b.z)),
        fma(a.x, b.y, -(a.y * b.x)),
    );
}

fn dot_fma(a: vec3<f32>, b: vec3<f32>) -> f32 {
    return fma(a.x, b.x, fma(a.y, b.y, a.z * b.z));
}

// Mirrors optics::birefringence::canonicalize_eigenvector_sign op-for-op -- see that
// function's doc comment for the tie-break convention (x, then y, then z priority).
fn canonicalize_eigenvector_sign(v: vec3<f32>) -> vec3<f32> {
    let ax = abs(v.x);
    let ay = abs(v.y);
    let az = abs(v.z);
    var largest: f32;
    if (ax >= ay && ax >= az) {
        largest = v.x;
    } else if (ay >= az) {
        largest = v.y;
    } else {
        largest = v.z;
    }
    if (largest < 0.0) {
        return -v;
    }
    return v;
}

// optics::birefringence::BiaxialIndicatrix::eigenvector_world -- see that Rust
// function's doc comment for the "transverse impermeability" (Gamma = P.B.P) matrix
// construction and the sign-aligned row-pair-cross-product null-vector extraction this
// mirrors op-for-op (reformulated 2026-09-02 for numerical conditioning; replaces the
// previous "cleared-denominator" polynomial form, and then again replaces a first
// largest-of-three-magnitude version of this construction that still measured up to
// ~640K ULP against the CPU side -- see the Rust doc comment for why the discrete
// argmax branch itself was the remaining problem).
//
// Mirrors optics::birefringence::BiaxialIndicatrix::precise_root_near op-for-op: an
// algebraically-exact discriminant reformulation that replaces `B^2 - 4C` (a
// subtraction of two ~1-magnitude sums) with direct, Sterbenz-exact differences of the
// principal `1/n^2` values -- see the Rust doc comment for the full derivation.
fn precise_root_near(local: vec3<f32>, b: vec3<f32>, x: f32) -> f32 {
    let a = local.x * local.x;
    let bb = local.y * local.y;
    let cc = local.z * local.z;
    let big_b = fma(a, b.y + b.z, fma(bb, b.x + b.z, cc * (b.x + b.y)));

    let xdiff = b.x - b.y;
    let ydiff = b.z - b.x;

    let a_plus_c = a + cc;
    let a_plus_bb = a + bb;
    let two_a_minus_bc = 2.0 * fma(bb, -cc, a);

    let disc_sq = fma(
        a_plus_c * a_plus_c,
        xdiff * xdiff,
        fma(a_plus_bb * a_plus_bb, ydiff * ydiff, two_a_minus_bc * xdiff * ydiff),
    );
    let disc = sqrt(max(disc_sq, 0.0));

    let x_lo = 0.5 * (big_b - disc);
    let x_hi = 0.5 * (big_b + disc);

    if (abs(x - x_lo) <= abs(x - x_hi)) {
        return x_lo;
    }
    return x_hi;
}

fn biaxial_eigenvector_world(
    ax0: vec3<f32>, ax1: vec3<f32>, ax2: vec3<f32>,
    local: vec3<f32>, b: vec3<f32>, x_in: f32, k_hat: vec3<f32>,
) -> vec3<f32> {
    let x = precise_root_near(local, b, x_in);

    let s = fma(b.x, local.x * local.x, fma(b.y, local.y * local.y, b.z * local.z * local.z));

    let m00 = fma(local.x * local.x, fma(-2.0, b.x, s), b.x - x);
    let m11 = fma(local.y * local.y, fma(-2.0, b.y, s), b.y - x);
    let m22 = fma(local.z * local.z, fma(-2.0, b.z, s), b.z - x);
    let m01 = local.x * local.y * (s - b.x - b.y);
    let m02 = local.x * local.z * (s - b.x - b.z);
    let m12 = local.y * local.z * (s - b.y - b.z);

    let row0 = vec3<f32>(m00, m01, m02);
    let row1 = vec3<f32>(m01, m11, m12);
    let row2 = vec3<f32>(m02, m12, m22);

    let c01 = cross_fma(row0, row1);
    let c02 = cross_fma(row0, row2);
    let c12 = cross_fma(row1, row2);

    var sign02 = 1.0;
    if (dot_fma(c01, c02) < 0.0) {
        sign02 = -1.0;
    }
    var sign12 = 1.0;
    if (dot_fma(c01, c12) < 0.0) {
        sign12 = -1.0;
    }
    let v_local = c01 + c02 * sign02 + c12 * sign12;

    let v_world = ax0 * v_local.x + ax1 * v_local.y + ax2 * v_local.z;
    if (dot(v_world, v_world) > 1e-12) {
        return normalize(canonicalize_eigenvector_sign(v_world));
    }
    return stable_orthonormal_basis_t(k_hat);
}

struct BiaxialEigenResult {
    d_slow: vec3<f32>,
    d_fast: vec3<f32>,
}

// optics::birefringence::BiaxialIndicatrix::eigen_polarizations
fn biaxial_eigen_polarizations(
    n_alpha: f32, n_beta: f32, n_gamma: f32,
    ax0: vec3<f32>, ax1: vec3<f32>, ax2: vec3<f32>,
    wave_normal: vec3<f32>,
) -> BiaxialEigenResult {
    let k = normalize_or_zero(wave_normal);
    let local = vec3<f32>(dot(ax0, k), dot(ax1, k), dot(ax2, k));
    let bc = biaxial_b_coeffs(n_alpha, n_beta, n_gamma);
    let ni = biaxial_wave_indices(n_alpha, n_beta, n_gamma, ax0, ax1, ax2, wave_normal);
    let x_slow = 1.0 / (ni.x * ni.x);
    let x_fast = 1.0 / (ni.y * ni.y);

    var result: BiaxialEigenResult;
    result.d_slow = biaxial_eigenvector_world(ax0, ax1, ax2, local, bc, x_slow, k);
    result.d_fast = biaxial_eigenvector_world(ax0, ax1, ax2, local, bc, x_fast, k);
    return result;
}

// optics::birefringence::BiaxialIndicatrix::d_to_e_direction
fn biaxial_d_to_e_direction(
    n_alpha: f32, n_beta: f32, n_gamma: f32,
    ax0: vec3<f32>, ax1: vec3<f32>, ax2: vec3<f32>,
    d_hat: vec3<f32>,
) -> vec3<f32> {
    let d_local = vec3<f32>(dot(ax0, d_hat), dot(ax1, d_hat), dot(ax2, d_hat));
    let e_local = vec3<f32>(
        d_local.x / (n_alpha * n_alpha),
        d_local.y / (n_beta * n_beta),
        d_local.z / (n_gamma * n_gamma),
    );
    return normalize_or_zero(ax0 * e_local.x + ax1 * e_local.y + ax2 * e_local.z);
}

// optics::birefringence::poynting_direction -- the general (uniaxial-or-biaxial)
// Poynting/walk-off direction from a wave normal and world-space E-field direction.
fn poynting_direction(wave_normal: vec3<f32>, e_field_hat: vec3<f32>) -> vec3<f32> {
    let k_hat = normalize_or_zero(wave_normal);
    let e_hat = normalize_or_zero(e_field_hat);
    let perp = k_hat - e_hat * dot(e_hat, k_hat);
    let len2 = dot(perp, perp);
    if (len2 > 1e-10) {
        return perp / sqrt(len2);
    }
    return k_hat;
}

// optics::birefringence::BiaxialIndicatrix::mode_poynting_dir
fn biaxial_mode_poynting_dir(
    n_alpha: f32, n_beta: f32, n_gamma: f32,
    ax0: vec3<f32>, ax1: vec3<f32>, ax2: vec3<f32>,
    wave_normal: vec3<f32>, want_slow: bool,
) -> vec3<f32> {
    let eig = biaxial_eigen_polarizations(n_alpha, n_beta, n_gamma, ax0, ax1, ax2, wave_normal);
    var d_hat: vec3<f32>;
    if (want_slow) {
        d_hat = eig.d_slow;
    } else {
        d_hat = eig.d_fast;
    }
    let e_hat = biaxial_d_to_e_direction(n_alpha, n_beta, n_gamma, ax0, ax1, ax2, d_hat);
    return poynting_direction(wave_normal, e_hat);
}

// optics::raytracer::refraction::poynting_dir_for_mode -- recovers the mode's Poynting
// (energy/ray) direction S for a freshly-reflected wave normal k. Returns k unchanged
// (S == k) outside the crystal, for an isotropic material, and for the uniaxial
// ORDINARY eigenmode -- see that function's own doc comment for the full rationale
// (bit-identity in every one of those cases). Hero-level scalars
// (n_alpha_hero/n_beta_hero/n_gamma_hero/biax_ax0/biax_ax1/biax_ax2, n_o_hero/n_e_hero,
// c_axis) are passed explicitly rather than read from a struct binding, mirroring this
// file's established "one shared body, explicit scalar parameters" convention.
fn poynting_dir_for_mode(
    is_anisotropic: bool,
    is_biaxial: bool,
    inside_gem: bool,
    is_extraordinary: bool,
    k: vec3<f32>,
    c_axis: vec3<f32>,
    n_o_hero: f32,
    n_e_hero: f32,
    n_alpha_hero: f32,
    n_beta_hero: f32,
    n_gamma_hero: f32,
    biax_ax0: vec3<f32>,
    biax_ax1: vec3<f32>,
    biax_ax2: vec3<f32>,
) -> vec3<f32> {
    if (!inside_gem || !is_anisotropic) {
        return k;
    }
    if (is_biaxial) {
        return biaxial_mode_poynting_dir(n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2, k, is_extraordinary);
    }
    if (is_extraordinary) {
        return extraordinary_poynting_dir(k, c_axis, n_o_hero, n_e_hero);
    }
    return k;
}

struct BiaxialResolveResult {
    n: f32,
    wave_dir: vec3<f32>,
}

// optics::birefringence::BiaxialIndicatrix::resolve_entry_mode -- the two-iteration
// fixed point resolving a biaxial mode's refracted wave-normal direction at an
// air->crystal entry.
fn biaxial_resolve_entry_mode(
    n_alpha: f32, n_beta: f32, n_gamma: f32,
    ax0: vec3<f32>, ax1: vec3<f32>, ax2: vec3<f32>,
    incident_dir: vec3<f32>, normal: vec3<f32>, cos_i: f32, n_seed: f32, want_slow: bool,
) -> BiaxialResolveResult {
    var n_guess = n_seed;
    var wave_dir = incident_dir;
    for (var i: u32 = 0u; i < 2u; i = i + 1u) {
        let eta_guess = 1.0 / n_guess;
        let sin2_t_guess = eta_guess * eta_guess * fma(-cos_i, cos_i, 1.0);
        if (sin2_t_guess > 1.0) {
            break;
        }
        let cos_t_guess = sqrt(max(1.0 - sin2_t_guess, 0.0));
        wave_dir = normalize(eta_guess * incident_dir + fma(eta_guess, cos_i, -cos_t_guess) * normal);
        let ni = biaxial_wave_indices(n_alpha, n_beta, n_gamma, ax0, ax1, ax2, wave_dir);
        if (want_slow) {
            n_guess = ni.x;
        } else {
            n_guess = ni.y;
        }
    }
    var result: BiaxialResolveResult;
    result.n = n_guess;
    result.wave_dir = wave_dir;
    return result;
}

// optics::birefringence::AbsorptionTensor3::biaxial + AbsorptionTensor3::quadratic_form
// -- the three-independent-principal-coefficient generalization of `quadratic_form`
// above. Reconstructs its axis frame fresh from `c_axis` (never reuses a caller's
// `ax0`/`ax1`/`ax2`), exactly mirroring how the uniaxial `quadratic_form`/
// `pleochroic_channel_alpha` pair above independently rebuild theirs -- both
// constructions are bit-identical to `biaxial_axes_from_gamma` on the same input (see
// `birefringence::biaxial_reduction_tests::absorption_frame_is_bit_identical_to_index_frame`
// on the CPU side), so which call site does the rebuilding is a style choice, not a
// correctness one.
fn quadratic_form3(alpha: f32, beta: f32, gamma: f32, a1: vec3<f32>, a2: vec3<f32>, c: vec3<f32>, e_hat: vec3<f32>) -> f32 {
    let l0 = dot(a1, e_hat);
    let l1 = dot(a2, e_hat);
    let l2 = dot(c, e_hat);
    return alpha * (l0 * l0) + beta * (l1 * l1) + gamma * (l2 * l2);
}

// optics::birefringence::pleochroic_channel_alpha with `alpha_beta = Some(alpha_beta)`
// -- the genuinely biaxial (trichroic) three-coefficient absorption path.
fn pleochroic_channel_alpha_biaxial(
    alpha_o: f32,
    alpha_beta: f32,
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
    let alpha_polarized = quadratic_form3(alpha_o, alpha_beta, alpha_e, a1, a2, c, e_hat);
    let alpha_unpolarized = 0.5 * (quadratic_form3(alpha_o, alpha_beta, alpha_e, a1, a2, c, eigen_a) + quadratic_form3(alpha_o, alpha_beta, alpha_e, a1, a2, c, eigen_b));
    let p = clamp(degree_of_polarization(s), 0.0, 1.0);
    return fma(p, alpha_polarized - alpha_unpolarized, alpha_unpolarized);
}

// ---------------------------------------------------------------------------------
// P1 (assigned-mode absorption): optics::birefringence::{assigned_mode_alpha,
// assigned_mode_e_field_uniaxial, BiaxialIndicatrix::assigned_mode_e_field}. Replaces
// pleochroic_channel_alpha(_biaxial) above on the ANISOTROPIC branch of the interior
// transport path -- see optics::raytracer::absorption::channel_absorption_alphas_assigned's
// doc comment for the full rationale (a camera-traced path's Stokes vector is importance,
// not the light's true state, and its azimuth drifts from the assigned mode's own axis
// after internal reflections; the assigned mode's E-field is exact geometry instead, and
// these functions read NO Stokes vector at all). The isotropic branch keeps calling
// pleochroic_channel_alpha(_biaxial) above, unchanged -- see the megakernel call site's
// own comment for why that is a deliberate, numerically-inert divergence from the CPU
// side's isotropic branch (which no longer has a Stokes vector available to it there).
// ---------------------------------------------------------------------------------

// optics::birefringence::assigned_mode_e_field_uniaxial
fn assigned_mode_e_field_uniaxial(
    k_hat: vec3<f32>,
    c_axis: vec3<f32>,
    is_extraordinary: bool,
    n_o_hero: f32,
    n_e_hero: f32,
) -> vec3<f32> {
    if (!is_extraordinary) {
        return ordinary_eigen_polarization(k_hat, c_axis);
    }
    let k_cross_c = cross(k_hat, c_axis);
    if (dot(k_cross_c, k_cross_c) < 1e-8) {
        return extraordinary_eigen_polarization(k_hat, c_axis);
    }
    let s = extraordinary_poynting_dir(k_hat, c_axis, n_o_hero, n_e_hero);
    return normalize(cross(k_cross_c, s));
}

// optics::birefringence::assigned_mode_alpha, specialised to the uniaxial tensor
// (mirrors optics::raytracer::absorption::channel_absorption_alphas_assigned's uniaxial
// branch: AbsorptionTensor3::uniaxial(alpha_o, alpha_e, c_axis).quadratic_form(e_mode_hat),
// with the tensor's (a1, a2, c) axis frame rebuilt the same way pleochroic_channel_alpha
// above does).
fn assigned_mode_alpha_uniaxial(
    alpha_o: f32,
    alpha_e: f32,
    c_axis: vec3<f32>,
    k: vec3<f32>,
    is_extraordinary: bool,
    n_o_hero: f32,
    n_e_hero: f32,
) -> f32 {
    let c = normalize_or_zero(c_axis);
    let a1 = stable_orthonormal_basis_t(c);
    let a2 = cross(c, a1);
    let e_hat = assigned_mode_e_field_uniaxial(k, c_axis, is_extraordinary, n_o_hero, n_e_hero);
    return quadratic_form(alpha_o, alpha_e, a1, a2, c, e_hat);
}

// optics::birefringence::BiaxialIndicatrix::assigned_mode_e_field
fn biaxial_assigned_mode_e_field(
    n_alpha: f32, n_beta: f32, n_gamma: f32,
    ax0: vec3<f32>, ax1: vec3<f32>, ax2: vec3<f32>,
    wave_normal: vec3<f32>, is_extraordinary: bool,
) -> vec3<f32> {
    let eig = biaxial_eigen_polarizations(n_alpha, n_beta, n_gamma, ax0, ax1, ax2, wave_normal);
    var d_hat: vec3<f32>;
    if (is_extraordinary) {
        d_hat = eig.d_slow;
    } else {
        d_hat = eig.d_fast;
    }
    return biaxial_d_to_e_direction(n_alpha, n_beta, n_gamma, ax0, ax1, ax2, d_hat);
}

// optics::birefringence::assigned_mode_alpha, specialised to the biaxial
// (three-independent-principal-coefficient) tensor -- the assigned-mode counterpart of
// pleochroic_channel_alpha_biaxial above.
fn assigned_mode_alpha_biaxial(
    alpha_o: f32,
    alpha_beta: f32,
    alpha_e: f32,
    n_alpha: f32, n_beta: f32, n_gamma: f32,
    ax0: vec3<f32>, ax1: vec3<f32>, ax2: vec3<f32>,
    c_axis: vec3<f32>,
    k: vec3<f32>,
    is_extraordinary: bool,
) -> f32 {
    let c = normalize_or_zero(c_axis);
    let a1 = stable_orthonormal_basis_t(c);
    let a2 = cross(c, a1);
    let e_hat = biaxial_assigned_mode_e_field(n_alpha, n_beta, n_gamma, ax0, ax1, ax2, k, is_extraordinary);
    return quadratic_form3(alpha_o, alpha_beta, alpha_e, a1, a2, c, e_hat);
}

// ---------------------------------------------------------------------------------
// optics::dispersion::DispersionModel::evaluate -- takes the dispersion params as
// explicit arguments (rather than reading a `material: GpuGemMaterial` binding
// directly) so the exact same function body is callable both from the megakernel
// (which has that binding) and from Tier 2's `dispersion_main` (which reads per-case
// values out of its own `DispersionCase` storage buffer instead).
// ---------------------------------------------------------------------------------

fn dispersion_evaluate(model_type: u32, param_a: vec4<f32>, param_b: vec4<f32>, lambda_nm: f32) -> f32 {
    let lambda_um = lambda_nm * 1e-3;
    let l2 = lambda_um * lambda_um;
    if (model_type == 0u) {
        let n2 = 1.0 + (param_a.x * l2) / (l2 - param_b.x);
        return sqrt(max(n2, 1.0));
    } else if (model_type == 1u) {
        var n2: f32 = 1.0;
        n2 = n2 + (param_a.x * l2) / (l2 - param_b.x);
        n2 = n2 + (param_a.y * l2) / (l2 - param_b.y);
        n2 = n2 + (param_a.z * l2) / (l2 - param_b.z);
        return sqrt(max(n2, 1.0));
    } else {
        let l4 = l2 * l2;
        return param_a.x + (param_a.y / l2) + (param_a.z / l4);
    }
}

// P3 (extraordinary-ray dispersion GPU port): optics::dispersion::DispersionModel::
// evaluate, evaluated against a material's OPTIONAL independent extraordinary-ray
// curve (`GpuGemMaterial::has_extraordinary_dispersion`/`extraordinary_model_type`/
// `extraordinary_param_a`/`extraordinary_param_b` -- see `renderer::buffers::
// GpuGemMaterial`'s own doc comment). A separate function from `dispersion_evaluate`
// above -- rather than that function reused as-is -- because the CPU's
// `DispersionModel::evaluate` floors EVERY variant at `n >= 1.0`
// (`sqrt(max(n2, 1.0))` on both Sellmeier branches, an explicit `.max(1.0)` on the
// Cauchy branch -- see that function's own doc comment on why an out-of-fit-range
// extrapolation must never produce a physically-impossible index), while
// `dispersion_evaluate` -- ported only for the ORDINARY-ray curve, whose Cauchy fits
// this crate's built-ins only ever evaluate well within their validated range -- has
// never needed the Cauchy floor and omits it. Every built-in extraordinary-ray curve
// today (Quartz/Amethyst/Citrine) is Sellmeier3, whose floor `dispersion_evaluate`
// already applies identically, but this function stays a faithful, complete port of
// `DispersionModel::evaluate` (Cauchy floor included) rather than one that happens to
// agree only for the variant currently in use.
fn extraordinary_dispersion_evaluate(model_type: u32, param_a: vec4<f32>, param_b: vec4<f32>, lambda_nm: f32) -> f32 {
    let lambda_um = lambda_nm * 1e-3;
    let l2 = lambda_um * lambda_um;
    if (model_type == 0u) {
        let n2 = 1.0 + (param_a.x * l2) / (l2 - param_b.x);
        return sqrt(max(n2, 1.0));
    } else if (model_type == 1u) {
        var n2: f32 = 1.0;
        n2 = n2 + (param_a.x * l2) / (l2 - param_b.x);
        n2 = n2 + (param_a.y * l2) / (l2 - param_b.y);
        n2 = n2 + (param_a.z * l2) / (l2 - param_b.z);
        return sqrt(max(n2, 1.0));
    } else {
        let l4 = l2 * l2;
        return max(param_a.x + (param_a.y / l2) + (param_a.z / l4), 1.0);
    }
}

// optics::raytracer::per_channel_uniaxial_indices -- one channel's (n_o, n_eff) pair;
// see the Phase 3 section header comment above for why the CPU's internal
// NUM_CHANNELS loop is the WGSL caller's responsibility instead of this function's.
// Placed after `dispersion_evaluate` (which it calls) rather than up in the Phase 3
// section above, purely so every function here is defined after everything it calls.
fn per_channel_uniaxial_index(
    model_type: u32,
    param_a: vec4<f32>,
    param_b: vec4<f32>,
    lambda_nm: f32,
    birefringence_delta: f32,
    is_anisotropic: bool,
    theta_c: f32,
) -> vec2<f32> {
    let n_o_k = dispersion_evaluate(model_type, param_a, param_b, lambda_nm);
    let n_e_k = n_o_k + birefringence_delta;
    var n_eff_k = n_o_k;
    if (is_anisotropic) {
        n_eff_k = effective_extraordinary_index(n_o_k, n_e_k, theta_c);
    }
    return vec2<f32>(n_o_k, n_eff_k);
}

// ---------------------------------------------------------------------------------
// optics::raytracer::spectral_absorption -- takes the band array/count as explicit
// arguments (rather than reading `material.o_ray_bands`/`material.e_ray_bands`
// directly) for the same reason as `dispersion_evaluate` above: one shared body, two
// different binding shapes at the call sites. The megakernel calls this once per
// eigenmode (`material.o_ray_bands`/`o_ray_band_count`, then `e_ray_bands`/
// `e_ray_band_count`) where it previously had two near-identical `_o`/`_e` copies of
// this function.
// ---------------------------------------------------------------------------------

// P6: `shape` selects which domain this band is Gaussian in (0 = GaussianWavelength,
// 1 = GaussianEnergy) -- see `optics::absorption::BandShape` and
// `renderer::buffers::band_shape`. Mirrors `renderer::buffers::GpuAbsorptionBand`
// field-for-field, echoed and size-asserted by `renderer::gpu::layout_check` against
// `layout_echo.wgsl`.
struct AbsorptionBand {
    center_nm: f32,
    width_nm: f32,
    peak: f32,
    shape: u32,
}

// optics::raytracer::spectral_absorption / optics::absorption::AbsorptionBand::evaluate.
// The `shape == 0u` (GaussianWavelength) branch keeps the exact same f32 op order as
// before `shape` was added, so every existing wavelength-domain material's GPU result is
// byte-identical to before this function grew a shape branch.
fn spectral_absorption(bands: array<AbsorptionBand, 8>, band_count: u32, lambda_nm: f32) -> f32 {
    var sum: f32 = 0.0;
    for (var i: u32 = 0u; i < band_count; i = i + 1u) {
        let band = bands[i];
        if (band.shape == 1u) {
            // Wavenumber in cm^-1: nu = 1e7 / lambda_nm -- see
            // optics::absorption::AbsorptionBand::evaluate's own comment for why 1e7.
            let nu = 1.0e7 / lambda_nm;
            let nu0 = 1.0e7 / band.center_nm;
            let t = (nu - nu0) / band.width_nm;
            sum = sum + band.peak * exp(-0.5 * t * t);
        } else {
            let t = (lambda_nm - band.center_nm) / band.width_nm;
            sum = sum + band.peak * exp(-0.5 * t * t);
        }
    }
    return sum;
}

// ---------------------------------------------------------------------------------
// Task 2 GPU port: optics::raytracer::{frosted_orthonormal_basis,
// cosine_weighted_hemisphere, apply_frosted_bounce} -- the diffuse (bruted/frosted
// girdle facet) bounce, ported here (not into `spectral_transport.wgsl` or
// `transport_functions.wgsl` separately) so the shipped megakernel and Tier 2's
// standalone `frosted_bounce_main`/`cosine_hemisphere_main` kernels call the exact same
// function object, never two texts that could drift -- see this file's own header
// comment for why that property matters (a duplicate-vs-shipped-code fault was
// previously caught only by luck, see `renderer::gpu::transport_check`'s module doc
// comment).
//
// # The one deliberate simplification, preserved exactly
//
// `apply_frosted_bounce` is achromatic BY DESIGN: every spectral channel shares the ONE
// direction drawn below (not a per-channel direction) and the ONE broadband
// reflect/transmit split `r_unpol` (computed from the HERO channel's `n1`/`n2`/`cos_i`
// only -- never a per-channel `r_unpol_k`). That is what lets a frosted bounce compose
// with the existing per-channel `path_pdf` bookkeeping and the final
// `spectral_mis_weight`/MIS combination with NO chromatic-termination guard: a smooth,
// finite-support hemisphere BSDF assigns strictly positive density to the realized
// direction under every channel's own hypothetical hero-driven technique (unlike a
// delta BSDF, whose density is exactly zero off its one wavelength-dependent
// direction), so there is no measure-zero mismatch to drop to zero. See
// `optics::raytracer::apply_frosted_bounce`'s own doc comment for the full derivation
// -- this WGSL translation must never diverge from it: no per-channel direction, no
// per-channel `r_unpol_k`, no extra `path_pdf` division (the cosine-weighted-hemisphere
// pdf already exactly cancels the assumed Lambertian `albedo = 1.0` BRDF/BTDF, folded
// into the `1.0 / r_unpol` / `1.0 / t_unpol` throughput scale below).
// ---------------------------------------------------------------------------------

struct FrostedBasis {
    t: vec3<f32>,
    b: vec3<f32>,
}

// optics::raytracer::frosted_orthonormal_basis
fn frosted_orthonormal_basis(n: vec3<f32>) -> FrostedBasis {
    var a: vec3<f32>;
    if (abs(n.x) > 0.9) {
        a = vec3<f32>(0.0, 1.0, 0.0);
    } else {
        a = vec3<f32>(1.0, 0.0, 0.0);
    }
    let t = normalize_or_zero(a - n * dot(n, a));
    let b = cross(n, t);
    var result: FrostedBasis;
    result.t = t;
    result.b = b;
    return result;
}

// optics::raytracer::cosine_weighted_hemisphere -- Malley's method (polar mapping, not
// the concentric-disk variant), so this is a direct line-for-line translation.
fn cosine_weighted_hemisphere(u1: f32, u2: f32, n: vec3<f32>) -> vec3<f32> {
    let r = sqrt(u1);
    let theta = 2.0 * PI * u2;
    let sin_t = sin(theta);
    let cos_t = cos(theta);
    let basis = frosted_orthonormal_basis(n);
    let dir = basis.t * (r * cos_t) + basis.b * (r * sin_t) + n * sqrt(max(1.0 - u1, 0.0));
    return normalize_or_zero(dir);
}

// The (new_dir, new_inside_gem, has_extraordinary_update, extraordinary_update) tuple
// `apply_frosted_bounce` returns, encoded for WGSL (which has no `Option<bool>`):
// `has_extraordinary_update == 0u` is the CPU's `None` (the TIR-forced and reflect
// arms); `!= 0u` is `Some(extraordinary_update != 0u)` (only reachable from the
// transmit arm's `entering_anisotropic` branch, mirroring
// optics::raytracer::apply_frosted_bounce's `entering_anisotropic.then_some(..)`).
struct FrostedBounceResult {
    new_dir: vec3<f32>,
    new_inside_gem: u32,
    has_extraordinary_update: u32,
    extraordinary_update: u32,
}

// optics::raytracer::apply_frosted_bounce -- the CPU signature takes `&RayMaterialContext`
// / `&BounceRefractionGeometry`; this WGSL translation flattens exactly the fields that
// function actually reads out of them (`ctx.is_anisotropic` and
// `geo.{sin2_t,n1,n2,cos_i}` -- see the CPU function's own doc comment) into explicit
// scalar/vector parameters, the same flattening convention every other ported function
// in this file already uses for its CPU struct-based counterpart (e.g.
// `theta_c_for_bounce` above). `stokes`/`path_pdf` are `ptr<function, ...>` so this
// mutates the caller's own local arrays in place, mirroring the CPU's `&mut
// [StokesVector; NUM_CHANNELS]` / `&mut [f32; NUM_CHANNELS]` out-parameters exactly.
fn apply_frosted_bounce(
    is_anisotropic: bool,
    sin2_t: f32,
    n1: f32,
    n2: f32,
    cos_i: f32,
    normal: vec3<f32>,
    inside_gem: bool,
    is_extraordinary: bool,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
) -> FrostedBounceResult {
    let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_DIR_U_STREAM))) / 4294967295.0;
    let u2 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_DIR_V_STREAM))) / 4294967295.0;

    var result: FrostedBounceResult;

    if (sin2_t > 1.0) {
        // Forced reflect (TIR), probability 1 -- no draw, no pdf division, mirroring
        // optics::raytracer::apply_tir_bounce's identical reasoning for the polished
        // path.
        let new_dir = cosine_weighted_hemisphere(u1, u2, normal);
        for (var k: u32 = 0u; k < 8u; k = k + 1u) {
            let intensity = max((*stokes)[k].x, 0.0);
            (*stokes)[k] = vec4<f32>(intensity, 0.0, 0.0, 0.0);
        }
        result.new_dir = new_dir;
        result.new_inside_gem = select(0u, 1u, inside_gem);
        result.has_extraordinary_update = 0u;
        result.extraordinary_update = 0u;
        return result;
    }

    let cos_t = sqrt(max(1.0 - sin2_t, 0.0));
    let r_s = fma(n2, -cos_t, n1 * cos_i) / fma(n2, cos_t, n1 * cos_i);
    let r_p = fma(n1, -cos_t, n2 * cos_i) / fma(n1, cos_t, n2 * cos_i);
    let r_unpol = clamp(0.5 * fma(r_p, r_p, r_s * r_s), R_UNPOL_MIN, R_UNPOL_MAX);
    let rng_bounce = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM))) / 4294967295.0;

    if (rng_bounce < r_unpol) {
        let new_dir = cosine_weighted_hemisphere(u1, u2, normal);
        for (var k: u32 = 0u; k < 8u; k = k + 1u) {
            let intensity = max((*stokes)[k].x, 0.0) / r_unpol;
            (*stokes)[k] = vec4<f32>(intensity, 0.0, 0.0, 0.0);
            (*path_pdf)[k] = (*path_pdf)[k] * r_unpol;
        }
        result.new_dir = new_dir;
        result.new_inside_gem = select(0u, 1u, inside_gem);
        result.has_extraordinary_update = 0u;
        result.extraordinary_update = 0u;
        return result;
    }

    let new_dir = cosine_weighted_hemisphere(u1, u2, -normal);
    let entering_anisotropic = (!inside_gem) && is_anisotropic;
    // Mode SELECTION is still a stochastic 50/50 draw -- only the throughput weighting
    // that used to accompany it (a `split_pdf` divisor/multiplier) is gone, since it
    // estimated twice the transmitted energy no interface can deliver. See
    // optics::raytracer::apply_frosted_bounce's doc comment for the full energy-share
    // reasoning (same shape as the polished path's entry split in refraction.rs).
    var use_extraordinary = is_extraordinary;
    if (entering_anisotropic) {
        let split_rand = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))) / 4294967295.0;
        use_extraordinary = split_rand < 0.5;
    }
    let t_unpol = 1.0 - r_unpol;
    // No `/ split_pdf` -- see the entering_anisotropic comment above.
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        let intensity = max((*stokes)[k].x, 0.0) / t_unpol;
        (*stokes)[k] = vec4<f32>(intensity, 0.0, 0.0, 0.0);
        // No `* split_pdf` -- scale-invariant under a uniform per-channel factor, was a
        // pure no-op on the MIS weight; see refraction.rs.
        (*path_pdf)[k] = (*path_pdf)[k] * t_unpol;
    }
    result.new_dir = new_dir;
    result.new_inside_gem = select(1u, 0u, inside_gem);
    if (entering_anisotropic) {
        result.has_extraordinary_update = 1u;
        result.extraordinary_update = select(0u, 1u, use_extraordinary);
    } else {
        result.has_extraordinary_update = 0u;
        result.extraordinary_update = 0u;
    }
    return result;
}

// ---------------------------------------------------------------------------------
// Physics review, Task 1 GPU port: inclusion/subsurface scattering
// (optics::raytracer::{henyey_greenstein_phase, sample_henyey_greenstein_direction,
// maybe_scatter_or_extinguish}) -- ported here (not into `spectral_transport.wgsl` or
// `transport_functions.wgsl` separately) for the exact same reason `apply_frosted_bounce`
// lives here: the shipped megakernel and Tier 2's standalone kernels must call the SAME
// function object, never two texts that could drift (see this file's own header
// comment).
//
// `maybe_scatter_or_extinguish` takes the per-channel absorption coefficients
// (`alphas`) as an explicit `array<f32, 8>` argument rather than recomputing them from
// band data itself, mirroring `dispersion_evaluate`'s "one shared body, two different
// binding shapes at the call sites" convention above: the megakernel already computes
// this exact per-channel array inline (the pre-Task-1 absorption block,
// `spectral_absorption` + `pleochroic_channel_alpha`), so this function starts from
// that array rather than re-deriving it -- see `optics::raytracer::maybe_scatter_or_extinguish`'s
// doc comment for the full estimator derivation (hazards 1-5); this is a line-for-line
// translation of that function's body, `channel_absorption_alphas`'s work already done
// by the caller.
// ---------------------------------------------------------------------------------

// optics::raytracer::{DISTANCE_SAMPLE_STREAM, PHASE_DIR_U_STREAM, PHASE_DIR_V_STREAM}.
const DISTANCE_SAMPLE_STREAM: u32 = 0xa24baed4u;
const PHASE_DIR_U_STREAM: u32 = 0x9fb21c65u;
const PHASE_DIR_V_STREAM: u32 = 0x1ce4e5b9u;

// optics::raytracer::henyey_greenstein_phase
fn henyey_greenstein_phase(cos_theta: f32, g: f32) -> f32 {
    let g2 = g * g;
    let denom = pow(max(fma(2.0 * g, -cos_theta, 1.0 + g2), 1e-6), 1.5);
    return (1.0 - g2) / (4.0 * PI * denom);
}

// optics::raytracer::sample_henyey_greenstein_direction
fn sample_henyey_greenstein_direction(u1: f32, u2: f32, g: f32, forward: vec3<f32>) -> vec3<f32> {
    var cos_theta: f32;
    if (abs(g) < 1e-3) {
        cos_theta = fma(-2.0, u1, 1.0);
    } else {
        let one_minus_g2 = fma(-g, g, 1.0);
        let denom = fma(2.0 * g, u1, 1.0 - g);
        let sq = one_minus_g2 / denom;
        cos_theta = fma(-sq, sq, fma(g, g, 1.0)) / (2.0 * g);
    }
    cos_theta = clamp(cos_theta, -1.0, 1.0);
    let sin_theta = sqrt(max(fma(cos_theta, -cos_theta, 1.0), 0.0));
    let phi = 2.0 * PI * u2;
    let sin_p = sin(phi);
    let cos_p = cos(phi);
    let basis = frosted_orthonormal_basis(forward);
    let dir = basis.t * (sin_theta * cos_p) + basis.b * (sin_theta * sin_p) + forward * cos_theta;
    return normalize_or_zero(dir);
}

// The `Option<(f32, Vec3)>` `maybe_scatter_or_extinguish` returns, encoded for WGSL:
// `scattered == 0u` is the CPU's `None` (`t_free`/`new_dir` unset, ignored by the
// caller); `!= 0u` is `Some((t_free, new_dir))`.
struct ScatterOrExtinguishResult {
    scattered: u32,
    t_free: f32,
    new_dir: vec3<f32>,
}

// optics::raytracer::maybe_scatter_or_extinguish. `stokes`/`path_pdf` are
// `ptr<function, ...>`, mirroring `apply_frosted_bounce`'s identical in-place-mutation
// convention above.
fn maybe_scatter_or_extinguish(
    alphas: array<f32, 8>,
    sigma_s: f32,
    g: f32,
    ray_dir: vec3<f32>,
    hit_t: f32,
    path_scale: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
) -> ScatterOrExtinguishResult {
    let sigma_t_hero = alphas[0] + sigma_s;

    let dist_rand = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ DISTANCE_SAMPLE_STREAM))) / 4294967295.0;
    let one_minus_u = max(1.0 - dist_rand, 1e-7);
    let t_free = -(log(one_minus_u)) / sigma_t_hero;

    // P1 (absorption path scale): model units -> absorption-length units. See
    // optics::materials::GemMaterial::absorption_path_scale and the CPU
    // maybe_scatter_or_extinguish's own doc comment. `path_scale == 1.0` (every
    // built-in) is an exact no-op.
    let hit_t_scaled = hit_t * path_scale;

    var result: ScatterOrExtinguishResult;

    if (t_free < hit_t_scaled) {
        let pdf_hero = sigma_t_hero * one_minus_u;
        for (var k: u32 = 0u; k < 8u; k = k + 1u) {
            let sigma_t_k = alphas[k] + sigma_s;
            let tr_k = exp(-sigma_t_k * t_free);
            let weight = tr_k * sigma_s / pdf_hero;
            let intensity = max((*stokes)[k].x, 0.0) * weight;
            (*stokes)[k] = vec4<f32>(intensity, 0.0, 0.0, 0.0);
            (*path_pdf)[k] = (*path_pdf)[k] * (sigma_t_k * tr_k);
        }
        let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ PHASE_DIR_U_STREAM))) / 4294967295.0;
        let u2 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ PHASE_DIR_V_STREAM))) / 4294967295.0;
        result.scattered = 1u;
        // Convert the sampled free-path distance back to MODEL units -- see the CPU
        // function's own doc comment for why this unit conversion preserves the
        // estimator's unbiasedness. `path_scale == 1.0` is an exact no-op division.
        result.t_free = t_free / path_scale;
        result.new_dir = sample_henyey_greenstein_direction(u1, u2, g, ray_dir);
        return result;
    }

    let survive_hero = exp(-sigma_t_hero * hit_t_scaled);
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        let sigma_t_k = alphas[k] + sigma_s;
        let survive_k = exp(-sigma_t_k * hit_t_scaled);
        (*stokes)[k] = (*stokes)[k] * (survive_k / max(survive_hero, 1e-30));
        (*path_pdf)[k] = (*path_pdf)[k] * survive_k;
    }
    result.scattered = 0u;
    result.t_free = 0.0;
    result.new_dir = vec3<f32>(0.0, 0.0, 0.0);
    return result;
}

// ---------------------------------------------------------------------------------
// Finding G7/G8: next-event estimation -- optics::raytracer::scattering::balance_heuristic.
//
// The direction-sampling (dist1d/dist2d binary search over the HDR importance
// distribution) and shadow-ray-dependent NEE contribution functions
// (`nee_contribution_hg_scatter`/`nee_contribution_frosted_exterior`) are NOT here:
// they need the `planes`/`dist_func`/`dist_cdf`/`dist_dims`/`hdr_texels` storage/uniform
// bindings, which (like `hdr_env_radiance_at`/`intersect_ray`/`try_split_exit_channel`/
// `shading_normal_near_edge` above them) are declared per-file, not in this shared
// prelude -- a `@group`/`@binding` declared here would collide with
// `transport_functions.wgsl`'s own binding numbers once `build.rs` concatenates this
// file ahead of both `spectral_transport.wgsl` AND `transport_functions.wgsl` (which
// already uses bindings 12-15 for its own Tier 2 case banks). So those five functions
// live directly in `spectral_transport.wgsl` (megakernel-local), with
// `transport_functions.wgsl` carrying its own standalone copies for
// `renderer::gpu::transport_check`'s Tier 2 self-tests -- the same "own standalone copy
// with its own binding" convention `shading_normal_near_edge`'s doc comment already
// establishes for `shaders/shading_normal.wgsl`. `balance_heuristic` itself is a pure
// function of two scalars (no binding needed), so it alone lives here, shared bit-for-bit
// by every copy of those five.
// ---------------------------------------------------------------------------------

// optics::raytracer::scattering::balance_heuristic
fn balance_heuristic(pdf_a: f32, pdf_b: f32) -> f32 {
    let denom = pdf_a + pdf_b;
    if (denom > 1e-12) {
        return pdf_a / denom;
    }
    return 0.0;
}

// ---------------------------------------------------------------------------------
// Finding G6/G7: Environment map spherical direction and UV conversions, solid-angle
// Jacobian, and spectral conversion.
// Mirrors optics::raytracer::environment::EnvironmentMap::{uv_to_direction, direction_to_uv, pdf_uv_to_solid_angle}.
// ---------------------------------------------------------------------------------

fn hdr_direction_to_uv(dir: vec3<f32>) -> vec2<f32> {
    let d = normalize(dir);
    let theta = acos(clamp(d.y, -1.0, 1.0));
    let phi = atan2(d.x, d.z);
    let v = theta / PI;
    let u = fract(phi / (2.0 * PI));
    return vec2<f32>(u, v);
}

fn hdr_uv_to_direction(u_in: f32, v_in: f32) -> vec3<f32> {
    let u = fract(u_in);
    let v = clamp(v_in, 0.0, 1.0);
    let theta = v * PI;
    let phi = u * 2.0 * PI;
    let sin_theta = sin(theta);
    let cos_theta = cos(theta);
    let sin_phi = sin(phi);
    let cos_phi = cos(phi);
    return vec3<f32>(sin_theta * sin_phi, cos_theta, sin_theta * cos_phi);
}

fn pdf_uv_to_solid_angle(pdf_uv: f32, v: f32) -> f32 {
    let theta = v * PI;
    let sin_theta = sin(theta);
    if (sin_theta <= 1e-6) {
        return 0.0;
    }
    return pdf_uv / (2.0 * PI * PI * sin_theta);
}

fn hdr_wrap_x(x: i32, width: i32) -> u32 {
    return u32(((x % width) + width) % width);
}

fn hdr_clamp_y(y: i32, height: i32) -> u32 {
    return u32(clamp(y, 0, height - 1));
}

fn asymmetric_gaussian(x: f32, mu: f32, sigma_lo: f32, sigma_hi: f32) -> f32 {
    var sigma: f32;
    if (x < mu) {
        sigma = sigma_lo;
    } else {
        sigma = sigma_hi;
    }
    let t = (x - mu) / sigma;
    return exp(-0.5 * t * t);
}

fn rgb_to_spectral_radiance(r: f32, g: f32, b: f32, lambda_nm: f32) -> f32 {
    let rc = max(r, 0.0);
    let gc = max(g, 0.0);
    let bc = max(b, 0.0);
    return fma(
        rc, asymmetric_gaussian(lambda_nm, 615.0, 45.0, 65.0),
        fma(
            gc, asymmetric_gaussian(lambda_nm, 545.0, 45.0, 45.0),
            bc * asymmetric_gaussian(lambda_nm, 465.0, 40.0, 45.0),
        ),
    );
}

struct HdrEnvDims {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

struct GpuDistDims {
    width: u32,
    height: u32,
    marginal_func_int: f32,
    _pad0: f32,
}

struct Dist1dSample {
    sample: f32,
    pdf: f32,
    offset: u32,
}

struct Dist2dSample {
    dir: vec3<f32>,
    rgb: vec3<f32>,
    pdf: f32,
}

// ---------------------------------------------------------------------------------
// P2 full uniaxial Fresnel (Lekner 1991) -- GPU mirror of
// `optics::raytracer::uniaxial_fresnel` (see that module's own Rust doc comment for
// the physics/derivation; this section is a direct, op-for-op translation, the same
// convention every other section of this file already follows -- e.g. the Phase 4
// `Biaxial*` functions above). Deliberately entirely self-contained (no `Cplx`/complex
// arithmetic exists anywhere else in this shader tree) since the CPU module itself
// states explicitly why: "the WGSL mirror needs the exact same explicit re/im
// arithmetic -- a `vec2<f32>`-based complex type there, hand-written
// `add`/`mul`/`div`/`sqrt`, exactly mirrors this" (see `Cplx`'s own Rust doc comment).
// A plain two-field struct is used here instead of `vec2<f32>` purely so field access
// reads `.re`/`.im` (matching the Rust field names exactly) rather than `.x`/`.y`.
//
// Wired into `spectral_transport.wgsl`'s uniaxial entry AND internal-reflection/exit
// dispatch -- see that file's own P2-full section for the branch structure this
// mirrors (`apply_uniaxial_entry_bounce`/`apply_uniaxial_internal_bounce` on the CPU
// side), including the same `k_hat` x `c_axis` degenerate-axis guard.
//
// Verified against the CPU implementation by `renderer::gpu::transport_check`'s
// `run_entry_solve_pair`/`run_internal_solve` Tier 2 ULP checks (feeds IDENTICAL
// `(n1, n_o, n_e, c_axis, frame)` inputs to both `uniaxial_fresnel::entry_solve_pair`/
// `internal_solve` and this section's `entry_solve_pair_with_incidence`/
// `internal_solve`, compares every output field within a ULP budget) and Tier 3 image
// comparison for Zircon/Tourmaline/Quartz/Rutile.
// ---------------------------------------------------------------------------------

struct Cplx {
    re: f32,
    im: f32,
}

fn cplx_re(re: f32) -> Cplx {
    var c: Cplx;
    c.re = re;
    c.im = 0.0;
    return c;
}

fn cplx_zero() -> Cplx {
    return cplx_re(0.0);
}

fn cplx_add(a: Cplx, b: Cplx) -> Cplx {
    var c: Cplx;
    c.re = a.re + b.re;
    c.im = a.im + b.im;
    return c;
}

fn cplx_sub(a: Cplx, b: Cplx) -> Cplx {
    var c: Cplx;
    c.re = a.re - b.re;
    c.im = a.im - b.im;
    return c;
}

fn cplx_mul(a: Cplx, b: Cplx) -> Cplx {
    var c: Cplx;
    c.re = a.re * b.re - a.im * b.im;
    c.im = a.re * b.im + a.im * b.re;
    return c;
}

fn cplx_scale(a: Cplx, s: f32) -> Cplx {
    var c: Cplx;
    c.re = a.re * s;
    c.im = a.im * s;
    return c;
}

fn cplx_conj(a: Cplx) -> Cplx {
    var c: Cplx;
    c.re = a.re;
    c.im = -a.im;
    return c;
}

fn cplx_norm_sqr(a: Cplx) -> f32 {
    return fma(a.im, a.im, a.re * a.re);
}

// optics::raytracer::uniaxial_fresnel::Cplx::div
fn cplx_div(a: Cplx, b: Cplx) -> Cplx {
    let denom = max(fma(b.im, b.im, b.re * b.re), 1e-20);
    var c: Cplx;
    c.re = fma(a.im, b.im, a.re * b.re) / denom;
    c.im = fma(a.re, -b.im, a.im * b.re) / denom;
    return c;
}

// optics::raytracer::uniaxial_fresnel::Cplx::sqrt_forward_branch
fn cplx_sqrt_forward_branch(a: Cplx) -> Cplx {
    // GPU-parity fix (P2 exit wiring, 2026-09-07): mirrors the CPU function's own
    // identical fix -- see that function's doc comment for the full derivation. A
    // pure negative-real input makes `theta` land exactly on `pi/2`, where
    // `cos(theta)`'s sign is rounding noise this hardware's trig unit resolves
    // differently from Rust's `atan2`/`cos`; bypassing that round-trip for this one
    // input shape makes both platforms compute the identical, contract-correct
    // (`im > 0`) result.
    if (a.im == 0.0 && a.re < 0.0) {
        var out0: Cplx;
        out0.re = 0.0;
        out0.im = sqrt(-a.re);
        return out0;
    }
    let r = sqrt(sqrt(cplx_norm_sqr(a)));
    let theta = atan2(a.im, a.re) * 0.5;
    var out: Cplx;
    out.re = r * cos(theta);
    out.im = r * sin(theta);
    if (out.re < 0.0) {
        out.re = -out.re;
        out.im = -out.im;
    }
    if (abs(out.re) < 1e-9 && out.im < 0.0) {
        out.re = -out.re;
        out.im = -out.im;
    }
    return out;
}

// optics::raytracer::uniaxial_fresnel::CVec3
struct CVec3 {
    re: vec3<f32>,
    im: vec3<f32>,
}

fn cvec3_from_real(v: vec3<f32>) -> CVec3 {
    var c: CVec3;
    c.re = v;
    c.im = vec3<f32>(0.0, 0.0, 0.0);
    return c;
}

fn cvec3_wavevector(that: vec3<f32>, zhat: vec3<f32>, tangential: f32, q: Cplx) -> CVec3 {
    var c: CVec3;
    c.re = that * tangential + zhat * q.re;
    c.im = zhat * q.im;
    return c;
}

fn cvec3_add(a: CVec3, b: CVec3) -> CVec3 {
    var c: CVec3;
    c.re = a.re + b.re;
    c.im = a.im + b.im;
    return c;
}

fn cvec3_scale_real(a: CVec3, s: f32) -> CVec3 {
    var c: CVec3;
    c.re = a.re * s;
    c.im = a.im * s;
    return c;
}

fn cvec3_scale_complex(a: CVec3, s: Cplx) -> CVec3 {
    var c: CVec3;
    c.re = a.re * s.re - a.im * s.im;
    c.im = a.re * s.im + a.im * s.re;
    return c;
}

fn cvec3_cross_real(a: CVec3, r: vec3<f32>) -> CVec3 {
    var c: CVec3;
    c.re = cross(a.re, r);
    c.im = cross(a.im, r);
    return c;
}

fn cvec3_cross(a: CVec3, b: CVec3) -> CVec3 {
    var c: CVec3;
    c.re = cross(a.re, b.re) - cross(a.im, b.im);
    c.im = cross(a.re, b.im) + cross(a.im, b.re);
    return c;
}

fn cvec3_dot_real(a: CVec3, r: vec3<f32>) -> Cplx {
    var c: Cplx;
    c.re = dot(a.re, r);
    c.im = dot(a.im, r);
    return c;
}

fn cvec3_conj(a: CVec3) -> CVec3 {
    var c: CVec3;
    c.re = a.re;
    c.im = -a.im;
    return c;
}

// optics::raytracer::uniaxial_fresnel::UniaxialFrame::build
struct UniaxialFrameW {
    that: vec3<f32>,
    zhat: vec3<f32>,
    s_axis: vec3<f32>,
    p_axis: vec3<f32>,
    cos_i: f32,
    sin_i: f32,
    alpha: f32,
    beta: f32,
    gamma: f32,
}

fn uniaxial_frame_build(k_hat: vec3<f32>, normal: vec3<f32>, c_axis: vec3<f32>, cos_i: f32, sin_i: f32) -> UniaxialFrameW {
    var f: UniaxialFrameW;
    f.zhat = -normal;
    var that: vec3<f32>;
    if (sin_i > 1e-5) {
        that = normalize_or_zero((k_hat + normal * cos_i) / sin_i);
    } else {
        var fallback: vec3<f32>;
        if (abs(f.zhat.x) < 0.9) {
            fallback = vec3<f32>(1.0, 0.0, 0.0);
        } else {
            fallback = vec3<f32>(0.0, 1.0, 0.0);
        }
        that = normalize_or_zero(fallback - f.zhat * dot(f.zhat, fallback));
    }
    f.that = that;
    var s_axis = normalize_or_zero(cross(k_hat, normal));
    if (dot(s_axis, s_axis) <= 1e-8) {
        s_axis = normalize_or_zero(cross(f.zhat, that));
    }
    f.s_axis = s_axis;
    f.p_axis = cross(k_hat, s_axis);
    f.cos_i = cos_i;
    f.sin_i = sin_i;
    f.alpha = dot(c_axis, that);
    f.beta = dot(c_axis, s_axis);
    f.gamma = dot(c_axis, f.zhat);
    return f;
}

// optics::raytracer::uniaxial_fresnel::uniaxial_q_roots
struct QRootsW {
    qo_plus: Cplx,
    qo_minus: Cplx,
    qe_plus: Cplx,
    qe_minus: Cplx,
}

fn uniaxial_q_roots(n_o: f32, n_e: f32, frame: UniaxialFrameW, tangential: f32) -> QRootsW {
    let n_o2 = n_o * n_o;
    let k2 = tangential * tangential;

    let qo2 = cplx_re(n_o2 - k2);
    let qo_plus = cplx_sqrt_forward_branch(qo2);
    let qo_minus = cplx_sub(cplx_zero(), qo_plus);

    var result: QRootsW;
    result.qo_plus = qo_plus;
    result.qo_minus = qo_minus;

    // Performance (requirement 7): exact isotropic limit, mirrors the CPU's own
    // `uniaxial_q_roots` fast path exactly -- see that function's doc comment for the
    // algebraic derivation (both extraordinary roots collapse to the ordinary ones
    // exactly at `n_o == n_e`).
    if (n_o == n_e) {
        result.qe_plus = qo_plus;
        result.qe_minus = qo_minus;
        return result;
    }

    let n_e2 = n_e * n_e;
    let deleps = n_e2 - n_o2;
    let denom = fma(frame.gamma * frame.gamma, deleps, n_o2);
    let a_term = fma(frame.beta * frame.beta, -deleps, n_e2);
    let b_term = n_e2 * fma(frame.gamma * frame.gamma, deleps, n_o2);
    let d_val = n_o2 * fma(a_term, -k2, b_term);
    let sqrt_d = cplx_sqrt_forward_branch(cplx_re(d_val));
    let shift = cplx_re(frame.alpha * frame.gamma * tangential * deleps);
    let denom_c = cplx_re(max(denom, 1e-12));
    result.qe_plus = cplx_div(cplx_sub(sqrt_d, shift), denom_c);
    result.qe_minus = cplx_div(cplx_sub(cplx_sub(cplx_zero(), sqrt_d), shift), denom_c);
    return result;
}

// optics::raytracer::uniaxial_fresnel::ModeFields / ordinary_mode_fields /
// extraordinary_mode_fields
struct ModeFieldsW {
    k: CVec3,
    e: CVec3,
    h: CVec3,
}

fn ordinary_mode_fields(n_o: f32, c_axis: vec3<f32>, that: vec3<f32>, zhat: vec3<f32>, tangential: f32, q: Cplx) -> ModeFieldsW {
    var m: ModeFieldsW;
    m.k = cvec3_wavevector(that, zhat, tangential, q);
    let d_o = cvec3_cross_real(m.k, c_axis);
    m.e = cvec3_scale_real(d_o, 1.0 / (n_o * n_o));
    m.h = cvec3_cross(m.k, m.e);
    return m;
}

fn extraordinary_mode_fields(n_o: f32, n_e: f32, c_axis: vec3<f32>, that: vec3<f32>, zhat: vec3<f32>, tangential: f32, q: Cplx) -> ModeFieldsW {
    let n_o2 = n_o * n_o;
    let n_e2 = n_e * n_e;
    let deleps = n_e2 - n_o2;
    var m: ModeFieldsW;
    m.k = cvec3_wavevector(that, zhat, tangential, q);
    let d_o = cvec3_cross_real(m.k, c_axis);
    let d_e = cvec3_cross(m.k, d_o);
    let c_dot_de = cvec3_dot_real(d_e, c_axis);
    let term1 = cvec3_scale_real(d_e, 1.0 / n_o2);
    let term2 = cvec3_scale_complex(cvec3_from_real(c_axis), cplx_scale(c_dot_de, -(deleps / (n_o2 * n_e2))));
    m.e = cvec3_add(term1, term2);
    m.h = cvec3_cross(m.k, m.e);
    return m;
}

// optics::raytracer::uniaxial_fresnel::poynting_z
fn poynting_z(fields: ModeFieldsW, amp: Cplx, zhat: vec3<f32>) -> f32 {
    let e = cvec3_scale_complex(fields.e, amp);
    let h = cvec3_scale_complex(fields.h, amp);
    let hc = cvec3_conj(h);
    let s = cvec3_cross(e, hc);
    return 0.5 * cvec3_dot_real(s, zhat).re;
}

// optics::raytracer::uniaxial_fresnel::tangential_components
fn tangential_components(fields: ModeFieldsW, that: vec3<f32>, s_axis: vec3<f32>) -> array<Cplx, 4> {
    var out: array<Cplx, 4>;
    out[0] = cvec3_dot_real(fields.e, that);
    out[1] = cvec3_dot_real(fields.e, s_axis);
    out[2] = cvec3_dot_real(fields.h, that);
    out[3] = cvec3_dot_real(fields.h, s_axis);
    return out;
}

// optics::raytracer::uniaxial_fresnel::solve4 -- Gauss-Jordan, single RHS.
fn solve4_single(a_in: array<array<Cplx, 4>, 4>, b_in: array<Cplx, 4>) -> array<Cplx, 4> {
    var a = a_in;
    var b = b_in;
    for (var col: u32 = 0u; col < 4u; col = col + 1u) {
        var piv = col;
        var piv_mag = cplx_norm_sqr(a[col][col]);
        for (var row: u32 = col + 1u; row < 4u; row = row + 1u) {
            let mag = cplx_norm_sqr(a[row][col]);
            if (mag > piv_mag) {
                piv = row;
                piv_mag = mag;
            }
        }
        for (var j: u32 = 0u; j < 4u; j = j + 1u) {
            let tmp = a[col][j];
            a[col][j] = a[piv][j];
            a[piv][j] = tmp;
        }
        let tmpb = b[col];
        b[col] = b[piv];
        b[piv] = tmpb;

        let pivot = a[col][col];
        for (var j: u32 = col; j < 4u; j = j + 1u) {
            a[col][j] = cplx_div(a[col][j], pivot);
        }
        b[col] = cplx_div(b[col], pivot);
        for (var row: u32 = 0u; row < 4u; row = row + 1u) {
            if (row == col) {
                continue;
            }
            let factor = a[row][col];
            if (cplx_norm_sqr(factor) == 0.0) {
                continue;
            }
            for (var j: u32 = col; j < 4u; j = j + 1u) {
                a[row][j] = cplx_sub(a[row][j], cplx_mul(factor, a[col][j]));
            }
            b[row] = cplx_sub(b[row], cplx_mul(factor, b[col]));
        }
    }
    return b;
}

// optics::raytracer::uniaxial_fresnel::solve4_two_rhs
struct Solve4TwoRhsResult {
    b1: array<Cplx, 4>,
    b2: array<Cplx, 4>,
}

fn solve4_two_rhs(a_in: array<array<Cplx, 4>, 4>, b1_in: array<Cplx, 4>, b2_in: array<Cplx, 4>) -> Solve4TwoRhsResult {
    var a = a_in;
    var b1 = b1_in;
    var b2 = b2_in;
    for (var col: u32 = 0u; col < 4u; col = col + 1u) {
        var piv = col;
        var piv_mag = cplx_norm_sqr(a[col][col]);
        for (var row: u32 = col + 1u; row < 4u; row = row + 1u) {
            let mag = cplx_norm_sqr(a[row][col]);
            if (mag > piv_mag) {
                piv = row;
                piv_mag = mag;
            }
        }
        for (var j: u32 = 0u; j < 4u; j = j + 1u) {
            let tmp = a[col][j];
            a[col][j] = a[piv][j];
            a[piv][j] = tmp;
        }
        let tmpb1 = b1[col];
        b1[col] = b1[piv];
        b1[piv] = tmpb1;
        let tmpb2 = b2[col];
        b2[col] = b2[piv];
        b2[piv] = tmpb2;

        let pivot = a[col][col];
        for (var j: u32 = col; j < 4u; j = j + 1u) {
            a[col][j] = cplx_div(a[col][j], pivot);
        }
        b1[col] = cplx_div(b1[col], pivot);
        b2[col] = cplx_div(b2[col], pivot);
        for (var row: u32 = 0u; row < 4u; row = row + 1u) {
            if (row == col) {
                continue;
            }
            let factor = a[row][col];
            if (cplx_norm_sqr(factor) == 0.0) {
                continue;
            }
            for (var j: u32 = col; j < 4u; j = j + 1u) {
                a[row][j] = cplx_sub(a[row][j], cplx_mul(factor, a[col][j]));
            }
            b1[row] = cplx_sub(b1[row], cplx_mul(factor, b1[col]));
            b2[row] = cplx_sub(b2[row], cplx_mul(factor, b2[col]));
        }
    }
    var result: Solve4TwoRhsResult;
    result.b1 = b1;
    result.b2 = b2;
    return result;
}

// optics::raytracer::uniaxial_fresnel::EntryIncidenceFrame / entry_incidence_frame --
// performance (requirement 7): built once per bounce by the caller (`n1 == 1.0` for
// every channel at an air->crystal entry), shared across every channel's own
// `entry_solve_pair_with_incidence` call -- see the CPU type's own doc comment.
struct EntryIncidenceFrameW {
    incident_s: array<Cplx, 4>,
    incident_p: array<Cplx, 4>,
    reflected_s: array<Cplx, 4>,
    reflected_p: array<Cplx, 4>,
}

fn entry_incidence_frame(n1: f32, frame: UniaxialFrameW) -> EntryIncidenceFrameW {
    let k_tan = n1 * frame.sin_i;
    let k_fwd = cvec3_wavevector(frame.that, frame.zhat, k_tan, cplx_re(n1 * frame.cos_i));
    let k_bwd = cvec3_wavevector(frame.that, frame.zhat, k_tan, cplx_re(-n1 * frame.cos_i));
    let s_axis_c = cvec3_from_real(frame.s_axis);
    let es_fwd = s_axis_c;
    let hs_fwd = cvec3_cross(k_fwd, es_fwd);
    let ep_bwd = cvec3_scale_real(cvec3_cross(k_bwd, s_axis_c), -1.0 / (n1 * n1));
    let ep_fwd = cvec3_scale_real(cvec3_cross(k_fwd, s_axis_c), -1.0 / (n1 * n1));

    var inc_s: ModeFieldsW;
    inc_s.k = k_fwd;
    inc_s.e = es_fwd;
    inc_s.h = hs_fwd;

    var inc_p: ModeFieldsW;
    inc_p.k = k_fwd;
    inc_p.e = ep_fwd;
    inc_p.h = cvec3_cross(k_fwd, ep_fwd);

    var refl_s: ModeFieldsW;
    refl_s.k = k_bwd;
    refl_s.e = s_axis_c;
    refl_s.h = cvec3_cross(k_bwd, s_axis_c);

    var refl_p: ModeFieldsW;
    refl_p.k = k_bwd;
    refl_p.e = ep_bwd;
    refl_p.h = cvec3_cross(k_bwd, ep_bwd);

    var result: EntryIncidenceFrameW;
    result.incident_s = tangential_components(inc_s, frame.that, frame.s_axis);
    result.incident_p = tangential_components(inc_p, frame.that, frame.s_axis);
    result.reflected_s = tangential_components(refl_s, frame.that, frame.s_axis);
    result.reflected_p = tangential_components(refl_p, frame.that, frame.s_axis);
    return result;
}

// optics::raytracer::uniaxial_fresnel::EntryPolarizationSolution /
// entry_solve_pair_with_incidence
struct EntrySolW {
    r_s: Cplx,
    r_p: Cplx,
    t_o: Cplx,
    t_e: Cplx,
    flux_o: f32,
    flux_e: f32,
    o_hat: vec3<f32>,
    e_hat: vec3<f32>,
}

struct EntryPairResultW {
    s_sol: EntrySolW,
    p_sol: EntrySolW,
}

fn entry_solve_pair_with_incidence(
    inc: EntryIncidenceFrameW,
    n1: f32,
    n_o: f32,
    n_e: f32,
    c_axis: vec3<f32>,
    frame: UniaxialFrameW,
) -> EntryPairResultW {
    let k_tan = n1 * frame.sin_i;
    let roots = uniaxial_q_roots(n_o, n_e, frame, k_tan);
    let fo = ordinary_mode_fields(n_o, c_axis, frame.that, frame.zhat, k_tan, roots.qo_plus);
    let fe = extraordinary_mode_fields(n_o, n_e, c_axis, frame.that, frame.zhat, k_tan, roots.qe_plus);

    let eo_c = tangential_components(fo, frame.that, frame.s_axis);
    let ee_c = tangential_components(fe, frame.that, frame.s_axis);

    var a: array<array<Cplx, 4>, 4>;
    var b_s: array<Cplx, 4>;
    var b_p: array<Cplx, 4>;
    for (var i: u32 = 0u; i < 4u; i = i + 1u) {
        a[i][0] = inc.reflected_s[i];
        a[i][1] = inc.reflected_p[i];
        a[i][2] = cplx_sub(cplx_zero(), eo_c[i]);
        a[i][3] = cplx_sub(cplx_zero(), ee_c[i]);
        b_s[i] = cplx_sub(cplx_zero(), inc.incident_s[i]);
        b_p[i] = cplx_sub(cplx_zero(), inc.incident_p[i]);
    }
    let sol = solve4_two_rhs(a, b_s, b_p);

    let flux_o = abs(poynting_z(fo, cplx_re(1.0), frame.zhat));
    let flux_e = abs(poynting_z(fe, cplx_re(1.0), frame.zhat));
    let o_hat = normalize_or_zero(fo.e.re);
    let e_hat = normalize_or_zero(fe.e.re);

    var s_sol: EntrySolW;
    s_sol.r_s = sol.b1[0];
    s_sol.r_p = sol.b1[1];
    s_sol.t_o = sol.b1[2];
    s_sol.t_e = sol.b1[3];
    s_sol.flux_o = flux_o;
    s_sol.flux_e = flux_e;
    s_sol.o_hat = o_hat;
    s_sol.e_hat = e_hat;

    var p_sol: EntrySolW;
    p_sol.r_s = sol.b2[0];
    p_sol.r_p = sol.b2[1];
    p_sol.t_o = sol.b2[2];
    p_sol.t_e = sol.b2[3];
    p_sol.flux_o = flux_o;
    p_sol.flux_e = flux_e;
    p_sol.o_hat = o_hat;
    p_sol.e_hat = e_hat;

    var result: EntryPairResultW;
    result.s_sol = s_sol;
    result.p_sol = p_sol;
    return result;
}

// optics::raytracer::uniaxial_fresnel::entry_solve_pair -- thin wrapper, kept for the
// Tier 2 check's single-call convenience (bit-identical to calling
// `entry_incidence_frame` + `entry_solve_pair_with_incidence` directly, which is what
// every production megakernel call site does instead, sharing one `EntryIncidenceFrameW`
// across all 8 channels of a bounce -- see this section's own header comment).
fn entry_solve_pair(n1: f32, n_o: f32, n_e: f32, c_axis: vec3<f32>, frame: UniaxialFrameW) -> EntryPairResultW {
    let inc = entry_incidence_frame(n1, frame);
    return entry_solve_pair_with_incidence(inc, n1, n_o, n_e, c_axis, frame);
}

// optics::raytracer::uniaxial_fresnel::InternalPolarizationSolution / internal_solve
struct InternalSolW {
    r_o: Cplx,
    r_e: Cplx,
    t_s: Cplx,
    t_p: Cplx,
    flux_ro: f32,
    flux_re: f32,
    flux_ts: f32,
    flux_tp: f32,
    flux_inc: f32,
    o_hat: vec3<f32>,
    e_hat: vec3<f32>,
}

fn internal_solve(
    n_mode_inc: f32,
    n_o: f32,
    n_e: f32,
    c_axis: vec3<f32>,
    frame: UniaxialFrameW,
    incident_is_ordinary: bool,
) -> InternalSolW {
    let k_tan = n_mode_inc * frame.sin_i;
    let roots = uniaxial_q_roots(n_o, n_e, frame, k_tan);

    var f_inc: ModeFieldsW;
    if (incident_is_ordinary) {
        f_inc = ordinary_mode_fields(n_o, c_axis, frame.that, frame.zhat, k_tan, roots.qo_plus);
    } else {
        f_inc = extraordinary_mode_fields(n_o, n_e, c_axis, frame.that, frame.zhat, k_tan, roots.qe_plus);
    }
    let f_o_bwd = ordinary_mode_fields(n_o, c_axis, frame.that, frame.zhat, k_tan, roots.qo_minus);
    let f_e_bwd = extraordinary_mode_fields(n_o, n_e, c_axis, frame.that, frame.zhat, k_tan, roots.qe_minus);

    let n2 = 1.0;
    let zeta = k_tan / n2;
    var cos_t: Cplx;
    if (abs(zeta) <= 1.0) {
        cos_t = cplx_re(sqrt(max(fma(zeta, -zeta, 1.0), 0.0)));
    } else {
        var ct: Cplx;
        ct.re = 0.0;
        ct.im = sqrt(fma(zeta, zeta, -1.0));
        cos_t = ct;
    }
    let k_iso = cvec3_wavevector(frame.that, frame.zhat, k_tan, cplx_scale(cos_t, n2));
    let s_axis_c = cvec3_from_real(frame.s_axis);
    let es_t = s_axis_c;
    let hs_t = cvec3_cross(k_iso, es_t);
    let ep_t = cvec3_scale_real(cvec3_cross(k_iso, s_axis_c), -1.0 / (n2 * n2));
    let hp_t = cvec3_cross(k_iso, ep_t);

    var f_s_t: ModeFieldsW;
    f_s_t.k = k_iso;
    f_s_t.e = es_t;
    f_s_t.h = hs_t;

    var f_p_t: ModeFieldsW;
    f_p_t.k = k_iso;
    f_p_t.e = ep_t;
    f_p_t.h = hp_t;

    let inc_c = tangential_components(f_inc, frame.that, frame.s_axis);
    let eo_c = tangential_components(f_o_bwd, frame.that, frame.s_axis);
    let ee_c = tangential_components(f_e_bwd, frame.that, frame.s_axis);
    let es_c = tangential_components(f_s_t, frame.that, frame.s_axis);
    let ep_c = tangential_components(f_p_t, frame.that, frame.s_axis);

    var a: array<array<Cplx, 4>, 4>;
    var b: array<Cplx, 4>;
    for (var i: u32 = 0u; i < 4u; i = i + 1u) {
        a[i][0] = eo_c[i];
        a[i][1] = ee_c[i];
        a[i][2] = cplx_sub(cplx_zero(), es_c[i]);
        a[i][3] = cplx_sub(cplx_zero(), ep_c[i]);
        b[i] = cplx_sub(cplx_zero(), inc_c[i]);
    }
    let sol = solve4_single(a, b);

    var result: InternalSolW;
    result.r_o = sol[0];
    result.r_e = sol[1];
    result.t_s = sol[2];
    result.t_p = sol[3];
    result.flux_ro = abs(poynting_z(f_o_bwd, cplx_re(1.0), frame.zhat));
    result.flux_re = abs(poynting_z(f_e_bwd, cplx_re(1.0), frame.zhat));
    result.flux_ts = abs(poynting_z(f_s_t, cplx_re(1.0), frame.zhat));
    result.flux_tp = abs(poynting_z(f_p_t, cplx_re(1.0), frame.zhat));
    result.flux_inc = abs(poynting_z(f_inc, cplx_re(1.0), frame.zhat));
    result.o_hat = normalize_or_zero(f_o_bwd.e.re);
    result.e_hat = normalize_or_zero(f_e_bwd.e.re);
    return result;
}

// optics::raytracer::uniaxial_fresnel::azimuth2_in_frame
fn azimuth2_in_frame(dir: vec3<f32>, s_axis: vec3<f32>, p_axis: vec3<f32>) -> vec2<f32> {
    let s_comp = dot(dir, s_axis);
    let p_comp = dot(dir, p_axis);
    let norm = max(fma(p_comp, p_comp, s_comp * s_comp), 1e-12);
    let cos_2psi = fma(p_comp, -p_comp, s_comp * s_comp) / norm;
    let sin_2psi = 2.0 * s_comp * p_comp / norm;
    return vec2<f32>(cos_2psi, sin_2psi);
}

// optics::raytracer::uniaxial_fresnel::jones_to_mueller
fn jones_to_mueller(j_ss: Cplx, j_sp: Cplx, j_ps: Cplx, j_pp: Cplx) -> mat4x4<f32> {
    let m_ss = cplx_norm_sqr(j_ss);
    let m_sp = cplx_norm_sqr(j_sp);
    let m_ps = cplx_norm_sqr(j_ps);
    let m_pp = cplx_norm_sqr(j_pp);

    let a_ssp = cplx_mul(j_ss, cplx_conj(j_sp));
    let a_pep = cplx_mul(j_ps, cplx_conj(j_pp));
    let a_ssp_plus = cplx_add(a_ssp, a_pep);
    let a_ssp_minus = cplx_sub(a_ssp, a_pep);

    let b_sp = cplx_mul(j_ss, cplx_conj(j_ps));
    let b_pp = cplx_mul(j_sp, cplx_conj(j_pp));

    let c_sspp = cplx_mul(j_ss, cplx_conj(j_pp));
    let c_spps = cplx_mul(j_sp, cplx_conj(j_ps));

    let i_i = 0.5 * (m_ss + m_ps + m_sp + m_pp);
    let i_q = 0.5 * (m_ss + m_ps - m_sp - m_pp);
    let i_u = a_ssp_plus.re;
    let i_v = a_ssp_plus.im;

    let q_i = 0.5 * (m_ss - m_ps + m_sp - m_pp);
    let q_q = 0.5 * ((m_ss - m_ps - m_sp) + m_pp);
    let q_u = a_ssp_minus.re;
    let q_v = a_ssp_minus.im;

    let u_i = b_sp.re + b_pp.re;
    let u_q = b_sp.re - b_pp.re;
    let u_u = c_sspp.re + c_spps.re;
    let u_v = c_sspp.im - c_spps.im;

    let v_i = -(b_sp.im + b_pp.im);
    let v_q = -(b_sp.im) + b_pp.im;
    let v_u = -(c_sspp.im + c_spps.im);
    let v_v = c_sspp.re - c_spps.re;

    return mat4x4<f32>(
        vec4<f32>(i_i, q_i, u_i, v_i),
        vec4<f32>(i_q, q_q, u_q, v_q),
        vec4<f32>(i_u, q_u, u_u, v_u),
        vec4<f32>(i_v, q_v, u_v, v_v),
    );
}

// optics::raytracer::uniaxial_fresnel::mode_power. `stokes` is this shader tree's usual
// `vec4<f32>(I, Q, U, V)` convention (`.x`/`.y`/`.z`/`.w`).
fn mode_power(t_s: Cplx, t_p: Cplx, mode_flux: f32, inc_flux: f32, stokes: vec4<f32>) -> f32 {
    let m_s = cplx_norm_sqr(t_s);
    let m_p = cplx_norm_sqr(t_p);
    let cross_term = cplx_mul(t_s, cplx_conj(t_p));
    let inner = 0.5 * fma(m_s, stokes.x + stokes.y, m_p * (stokes.x - stokes.y));
    let raw = fma(cross_term.im, stokes.w, fma(cross_term.re, stokes.z, inner));
    return (mode_flux / max(inc_flux, 1e-12)) * raw;
}

// ---------------------------------------------------------------------------------
// P6 exit-event spectral splitting (2026-09-07, final): the three pure (no-binding)
// per-channel helpers optics::raytracer::refraction's own top-of-file "Exit-event
// spectral splitting" doc comment names -- `compute_channel_transmission`,
// `compute_uniaxial_exit_transmission`, and `narrow_compat`. Every operation, and its
// order, is a direct transcription of the CPU function it mirrors (`f32::mul_add` ->
// `fma`, same clamp bounds, same intermediate names where WGSL's lack of tuple returns
// allows it) -- see each function's own comment for the exact CPU counterpart.
//
// `compute_channel_transmission`/`compute_uniaxial_exit_transmission` are private
// (module-private, not even `pub(super)`) inside `refraction.rs`, and `narrow_compat`
// is `pub(super)` (visible only within `optics::raytracer`, not from this crate's
// `renderer` tree) -- `refraction.rs` is on this task's protected/coordinator-owned
// list, so its visibility cannot be widened to import these directly the way
// `p2_uniaxial_fresnel.rs` imports the (already `pub(crate)`) `entry_solve_pair`/
// `internal_solve`. `renderer::gpu::transport_check::p6_exit_splitting`'s own CPU
// reference functions are therefore verbatim transcriptions of the real CPU source
// (cross-referenced by file/line in that module's doc comment) rather than a call
// through the module boundary -- the one deliberate exception to this file tree's
// usual "always call the real CPU function" rule, forced by that protection boundary,
// not a design choice.
//
// Constants duplicated locally (`8u` in place of `NUM_CHANNELS`, an inlined `1.0 -
// 1e-6` in place of `DIRECTION_MATCH_COS_TOL`) rather than referencing
// `spectral_transport.wgsl`'s own copies: `build.rs` concatenates THIS file ahead of
// both `spectral_transport.wgsl` and `transport_functions.wgsl` (see this file's own
// header comment), so a name defined only in one of those two files is not yet in
// scope here -- exactly the same reason `R_UNPOL_MIN`/`R_UNPOL_MAX` above are this
// file's own copies rather than a reference to `spectral_transport.wgsl`'s
// `R_UNPOL_SELECT_MIN`/`MAX`.
// ---------------------------------------------------------------------------------

// optics::raytracer::refraction::compute_channel_transmission. WGSL has no `Option`, so
// `azimuth_valid == false` stands in for the Rust `None` case (same convention as
// `entry_eigenmode_selection`'s own `valid` field above) -- callers pass
// `false`/`0.0`/`0.0` for `azimuth_valid`/`cos_2psi_x`/`sin_2psi_x` whenever
// `entering_anisotropic` is false or the entry mode-selection draw was invalid.
struct ChannelTransmissionW {
    transmitted: vec4<f32>,
    r_unpol_k: f32,
}

fn compute_channel_transmission(
    n1k: f32,
    n2k: f32,
    cos_i: f32,
    cos_t_k: f32,
    r_unpol: f32,
    entering_anisotropic: bool,
    azimuth_valid: bool,
    cos_2psi_x: f32,
    sin_2psi_x: f32,
    incident_stokes_k: vec4<f32>,
) -> ChannelTransmissionW {
    let t_s_k = (2.0 * n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
    let t_p_k = (2.0 * n1k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
    let trans_matrix_k = mueller_fresnel_transmission(n1k, n2k, cos_i, cos_t_k, t_s_k, t_p_k);
    var incident_k = incident_stokes_k;
    if (entering_anisotropic && azimuth_valid) {
        let i_k = incident_stokes_k.x;
        incident_k = vec4<f32>(i_k, i_k * cos_2psi_x, i_k * sin_2psi_x, 0.0);
    }
    let transmitted = (trans_matrix_k * incident_k) * (1.0 / (1.0 - r_unpol));
    let r_s_k = fma(n2k, -cos_t_k, n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
    let r_p_k = fma(n1k, -cos_t_k, n2k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
    let r_unpol_k = clamp(0.5 * fma(r_p_k, r_p_k, r_s_k * r_s_k), 1e-4, 1.0 - 1e-4);
    var result: ChannelTransmissionW;
    result.transmitted = transmitted;
    result.r_unpol_k = r_unpol_k;
    return result;
}

// optics::raytracer::refraction::compute_uniaxial_exit_transmission.
struct UniaxialExitTransmissionW {
    transmitted: vec4<f32>,
    i_unit: f32,
}

fn compute_uniaxial_exit_transmission(
    sol: InternalSolW,
    r_branch: f32,
    incident_i: f32,
) -> UniaxialExitTransmissionW {
    let flux_inc = max(sol.flux_inc, 1e-12);
    let ts_n = cplx_scale(sol.t_s, sqrt(sol.flux_ts / flux_inc));
    let tp_n = cplx_scale(sol.t_p, sqrt(sol.flux_tp / flux_inc));
    let i_unit = cplx_norm_sqr(ts_n) + cplx_norm_sqr(tp_n);
    let q_unit = cplx_norm_sqr(ts_n) - cplx_norm_sqr(tp_n);
    let cross_st = cplx_mul(ts_n, cplx_conj(tp_n));
    let u_unit = 2.0 * cross_st.re;
    let v_unit = -2.0 * cross_st.im;
    let transmitted = vec4<f32>(
        incident_i * i_unit, incident_i * q_unit, incident_i * u_unit, incident_i * v_unit,
    ) * (1.0 / (1.0 - r_branch));
    var result: UniaxialExitTransmissionW;
    result.transmitted = transmitted;
    result.i_unit = i_unit;
    return result;
}

// optics::raytracer::refraction::narrow_compat. `compat[c]` bit `j` set means channels
// `c`/`j` have refracted within the direction-match tolerance of each other at every
// interior dispersive event so far -- see `ExitSplitCtx::compat`'s own CPU-side doc
// comment. Hero is always channel 0 in this kernel's convention (`path_pdf[0]`
// throughout `spectral_transport.wgsl`), so this is specialized to that fixed hero
// index rather than taking one as a parameter, unlike the CPU function's `hero: usize`.
// `dirs_valid[k] == false` stands in for the CPU `dirs[k]: Option<Vec3> == None` case.
fn narrow_compat(
    compat: ptr<function, array<u32, 8>>,
    dirs: array<vec3<f32>, 8>,
    dirs_valid: array<bool, 8>,
    hero_match: array<bool, 8>,
) {
    let direction_match_cos_tol = 1.0 - 1e-6;
    for (var a: u32 = 0u; a < 8u; a = a + 1u) {
        for (var b: u32 = a + 1u; b < 8u; b = b + 1u) {
            var matches_ab: bool;
            if (a == 0u) {
                matches_ab = hero_match[b];
            } else if (b == 0u) {
                matches_ab = hero_match[a];
            } else if (dirs_valid[a] && dirs_valid[b]) {
                matches_ab = dot(dirs[a], dirs[b]) >= direction_match_cos_tol;
            } else {
                matches_ab = true;
            }
            if (!matches_ab) {
                (*compat)[a] = (*compat)[a] & ~(1u << b);
                (*compat)[b] = (*compat)[b] & ~(1u << a);
            }
        }
    }
}
