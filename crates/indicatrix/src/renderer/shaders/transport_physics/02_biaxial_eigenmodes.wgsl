
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
// mirrors op-for-op. This formulation (rather than a cleared-denominator polynomial
// form, or a largest-of-three-magnitude selection) is used for numerical conditioning:
// a discrete argmax branch over the three candidate magnitudes is the mechanism that
// otherwise degrades precision -- see the Rust doc comment for the full derivation.
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

// optics::birefringence::BiaxialIndicatrix::d_to_e_direction -- see quadratic_form3's
// doc comment above for why the per-axis dot products are spelled out as explicit
// non-fused scalar sums rather than the `dot()` builtin.
fn biaxial_d_to_e_direction(
    n_alpha: f32, n_beta: f32, n_gamma: f32,
    ax0: vec3<f32>, ax1: vec3<f32>, ax2: vec3<f32>,
    d_hat: vec3<f32>,
) -> vec3<f32> {
    let d_local = vec3<f32>(
        ax0.x * d_hat.x + ax0.y * d_hat.y + ax0.z * d_hat.z,
        ax1.x * d_hat.x + ax1.y * d_hat.y + ax1.z * d_hat.z,
        ax2.x * d_hat.x + ax2.y * d_hat.y + ax2.z * d_hat.z,
    );
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
// See eigenmodes_biaxial.rs's ASSIGNED_MODE_ALPHA_BIAXIAL_ULP_BUDGET doc comment. The
// three per-axis dot products are spelled out as explicit non-fused scalar sums here,
// matching optics::birefringence::AbsorptionTensor3::quadratic_form's exact
// `axes.transpose() * e_hat` accumulation order bit-for-bit (glam's Mat3::mul_vec3
// SAXPY expansion reduces algebraically to this same left-to-right x+y+z grouping --
// see that CPU function's doc comment). WGSL's `dot()` builtin is not specified to
// lower this way (a driver's shader compiler may contract it into an FMA chain), so
// this avoids relying on that unspecified rounding. This does not by itself close the
// harness's `assigned_mode_alpha_biaxial` ULP gap between `cpu` and `gpu` bit patterns
// -- see `eigenmodes_biaxial.rs`'s `ASSIGNED_MODE_ALPHA_BIAXIAL_ULP_BUDGET` doc comment
// for the full measurement and why the residual traces to hardware `/`/`sqrt`
// precision in the shared eigenvector solve instead, not this function's op order.
// Kept anyway: removing the dependency on `dot()`'s unspecified lowering is still
// worth having.
fn quadratic_form3(alpha: f32, beta: f32, gamma: f32, a1: vec3<f32>, a2: vec3<f32>, c: vec3<f32>, e_hat: vec3<f32>) -> f32 {
    let l0 = a1.x * e_hat.x + a1.y * e_hat.y + a1.z * e_hat.z;
    let l1 = a2.x * e_hat.x + a2.y * e_hat.y + a2.z * e_hat.z;
    let l2 = c.x * e_hat.x + c.y * e_hat.y + c.z * e_hat.z;
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
// side's isotropic branch, which has no Stokes vector available to it there.
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
