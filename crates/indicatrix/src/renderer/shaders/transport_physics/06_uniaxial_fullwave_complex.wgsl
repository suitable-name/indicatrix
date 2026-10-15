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
    // Mirrors the CPU function's own identical fix -- see that function's doc comment
    // for the full derivation. A
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

// BEGIN GENERATED solve4 (tools/unroll_solve4.py)
// Constant-index unrolled Gauss-Jordan: same arithmetic, same order, same pivoting as
// the rolled form (strict `>`, ascending rows, whole-row conditional swap, `!= 0.0`
// factor skip). DO NOT EDIT BY HAND: regenerate with
//     python crates/indicatrix/tools/unroll_solve4.py
// (`--check` verifies this region is current.)

// optics::raytracer::uniaxial_fresnel::solve4 -- Gauss-Jordan, single RHS.
fn solve4_single(a_in: array<array<Cplx, 4>, 4>, b_in: array<Cplx, 4>) -> array<Cplx, 4> {
    var a = a_in;
    var b = b_in;
    // column 0: pivot search
    var piv0: u32 = 0u;
    var piv_mag0: f32 = cplx_norm_sqr(a[0][0]);
    let mag0_1 = cplx_norm_sqr(a[1][0]);
    if (mag0_1 > piv_mag0) {
        piv0 = 1u;
        piv_mag0 = mag0_1;
    }
    let mag0_2 = cplx_norm_sqr(a[2][0]);
    if (mag0_2 > piv_mag0) {
        piv0 = 2u;
        piv_mag0 = mag0_2;
    }
    let mag0_3 = cplx_norm_sqr(a[3][0]);
    if (mag0_3 > piv_mag0) {
        piv0 = 3u;
        piv_mag0 = mag0_3;
    }
    if (piv0 == 1u) {
        let t0_1_0 = a[0][0];
        a[0][0] = a[1][0];
        a[1][0] = t0_1_0;
        let t0_1_1 = a[0][1];
        a[0][1] = a[1][1];
        a[1][1] = t0_1_1;
        let t0_1_2 = a[0][2];
        a[0][2] = a[1][2];
        a[1][2] = t0_1_2;
        let t0_1_3 = a[0][3];
        a[0][3] = a[1][3];
        a[1][3] = t0_1_3;
        let tb0_1_b = b[0];
        b[0] = b[1];
        b[1] = tb0_1_b;
    }
    if (piv0 == 2u) {
        let t0_2_0 = a[0][0];
        a[0][0] = a[2][0];
        a[2][0] = t0_2_0;
        let t0_2_1 = a[0][1];
        a[0][1] = a[2][1];
        a[2][1] = t0_2_1;
        let t0_2_2 = a[0][2];
        a[0][2] = a[2][2];
        a[2][2] = t0_2_2;
        let t0_2_3 = a[0][3];
        a[0][3] = a[2][3];
        a[2][3] = t0_2_3;
        let tb0_2_b = b[0];
        b[0] = b[2];
        b[2] = tb0_2_b;
    }
    if (piv0 == 3u) {
        let t0_3_0 = a[0][0];
        a[0][0] = a[3][0];
        a[3][0] = t0_3_0;
        let t0_3_1 = a[0][1];
        a[0][1] = a[3][1];
        a[3][1] = t0_3_1;
        let t0_3_2 = a[0][2];
        a[0][2] = a[3][2];
        a[3][2] = t0_3_2;
        let t0_3_3 = a[0][3];
        a[0][3] = a[3][3];
        a[3][3] = t0_3_3;
        let tb0_3_b = b[0];
        b[0] = b[3];
        b[3] = tb0_3_b;
    }
    let pivot0 = a[0][0];
    a[0][0] = cplx_div(a[0][0], pivot0);
    a[0][1] = cplx_div(a[0][1], pivot0);
    a[0][2] = cplx_div(a[0][2], pivot0);
    a[0][3] = cplx_div(a[0][3], pivot0);
    b[0] = cplx_div(b[0], pivot0);
    let factor0_1 = a[1][0];
    if (cplx_norm_sqr(factor0_1) != 0.0) {
        a[1][0] = cplx_sub(a[1][0], cplx_mul(factor0_1, a[0][0]));
        a[1][1] = cplx_sub(a[1][1], cplx_mul(factor0_1, a[0][1]));
        a[1][2] = cplx_sub(a[1][2], cplx_mul(factor0_1, a[0][2]));
        a[1][3] = cplx_sub(a[1][3], cplx_mul(factor0_1, a[0][3]));
        b[1] = cplx_sub(b[1], cplx_mul(factor0_1, b[0]));
    }
    let factor0_2 = a[2][0];
    if (cplx_norm_sqr(factor0_2) != 0.0) {
        a[2][0] = cplx_sub(a[2][0], cplx_mul(factor0_2, a[0][0]));
        a[2][1] = cplx_sub(a[2][1], cplx_mul(factor0_2, a[0][1]));
        a[2][2] = cplx_sub(a[2][2], cplx_mul(factor0_2, a[0][2]));
        a[2][3] = cplx_sub(a[2][3], cplx_mul(factor0_2, a[0][3]));
        b[2] = cplx_sub(b[2], cplx_mul(factor0_2, b[0]));
    }
    let factor0_3 = a[3][0];
    if (cplx_norm_sqr(factor0_3) != 0.0) {
        a[3][0] = cplx_sub(a[3][0], cplx_mul(factor0_3, a[0][0]));
        a[3][1] = cplx_sub(a[3][1], cplx_mul(factor0_3, a[0][1]));
        a[3][2] = cplx_sub(a[3][2], cplx_mul(factor0_3, a[0][2]));
        a[3][3] = cplx_sub(a[3][3], cplx_mul(factor0_3, a[0][3]));
        b[3] = cplx_sub(b[3], cplx_mul(factor0_3, b[0]));
    }
    // column 1: pivot search
    var piv1: u32 = 1u;
    var piv_mag1: f32 = cplx_norm_sqr(a[1][1]);
    let mag1_2 = cplx_norm_sqr(a[2][1]);
    if (mag1_2 > piv_mag1) {
        piv1 = 2u;
        piv_mag1 = mag1_2;
    }
    let mag1_3 = cplx_norm_sqr(a[3][1]);
    if (mag1_3 > piv_mag1) {
        piv1 = 3u;
        piv_mag1 = mag1_3;
    }
    if (piv1 == 2u) {
        let t1_2_0 = a[1][0];
        a[1][0] = a[2][0];
        a[2][0] = t1_2_0;
        let t1_2_1 = a[1][1];
        a[1][1] = a[2][1];
        a[2][1] = t1_2_1;
        let t1_2_2 = a[1][2];
        a[1][2] = a[2][2];
        a[2][2] = t1_2_2;
        let t1_2_3 = a[1][3];
        a[1][3] = a[2][3];
        a[2][3] = t1_2_3;
        let tb1_2_b = b[1];
        b[1] = b[2];
        b[2] = tb1_2_b;
    }
    if (piv1 == 3u) {
        let t1_3_0 = a[1][0];
        a[1][0] = a[3][0];
        a[3][0] = t1_3_0;
        let t1_3_1 = a[1][1];
        a[1][1] = a[3][1];
        a[3][1] = t1_3_1;
        let t1_3_2 = a[1][2];
        a[1][2] = a[3][2];
        a[3][2] = t1_3_2;
        let t1_3_3 = a[1][3];
        a[1][3] = a[3][3];
        a[3][3] = t1_3_3;
        let tb1_3_b = b[1];
        b[1] = b[3];
        b[3] = tb1_3_b;
    }
    let pivot1 = a[1][1];
    a[1][1] = cplx_div(a[1][1], pivot1);
    a[1][2] = cplx_div(a[1][2], pivot1);
    a[1][3] = cplx_div(a[1][3], pivot1);
    b[1] = cplx_div(b[1], pivot1);
    let factor1_0 = a[0][1];
    if (cplx_norm_sqr(factor1_0) != 0.0) {
        a[0][1] = cplx_sub(a[0][1], cplx_mul(factor1_0, a[1][1]));
        a[0][2] = cplx_sub(a[0][2], cplx_mul(factor1_0, a[1][2]));
        a[0][3] = cplx_sub(a[0][3], cplx_mul(factor1_0, a[1][3]));
        b[0] = cplx_sub(b[0], cplx_mul(factor1_0, b[1]));
    }
    let factor1_2 = a[2][1];
    if (cplx_norm_sqr(factor1_2) != 0.0) {
        a[2][1] = cplx_sub(a[2][1], cplx_mul(factor1_2, a[1][1]));
        a[2][2] = cplx_sub(a[2][2], cplx_mul(factor1_2, a[1][2]));
        a[2][3] = cplx_sub(a[2][3], cplx_mul(factor1_2, a[1][3]));
        b[2] = cplx_sub(b[2], cplx_mul(factor1_2, b[1]));
    }
    let factor1_3 = a[3][1];
    if (cplx_norm_sqr(factor1_3) != 0.0) {
        a[3][1] = cplx_sub(a[3][1], cplx_mul(factor1_3, a[1][1]));
        a[3][2] = cplx_sub(a[3][2], cplx_mul(factor1_3, a[1][2]));
        a[3][3] = cplx_sub(a[3][3], cplx_mul(factor1_3, a[1][3]));
        b[3] = cplx_sub(b[3], cplx_mul(factor1_3, b[1]));
    }
    // column 2: pivot search
    var piv2: u32 = 2u;
    var piv_mag2: f32 = cplx_norm_sqr(a[2][2]);
    let mag2_3 = cplx_norm_sqr(a[3][2]);
    if (mag2_3 > piv_mag2) {
        piv2 = 3u;
        piv_mag2 = mag2_3;
    }
    if (piv2 == 3u) {
        let t2_3_0 = a[2][0];
        a[2][0] = a[3][0];
        a[3][0] = t2_3_0;
        let t2_3_1 = a[2][1];
        a[2][1] = a[3][1];
        a[3][1] = t2_3_1;
        let t2_3_2 = a[2][2];
        a[2][2] = a[3][2];
        a[3][2] = t2_3_2;
        let t2_3_3 = a[2][3];
        a[2][3] = a[3][3];
        a[3][3] = t2_3_3;
        let tb2_3_b = b[2];
        b[2] = b[3];
        b[3] = tb2_3_b;
    }
    let pivot2 = a[2][2];
    a[2][2] = cplx_div(a[2][2], pivot2);
    a[2][3] = cplx_div(a[2][3], pivot2);
    b[2] = cplx_div(b[2], pivot2);
    let factor2_0 = a[0][2];
    if (cplx_norm_sqr(factor2_0) != 0.0) {
        a[0][2] = cplx_sub(a[0][2], cplx_mul(factor2_0, a[2][2]));
        a[0][3] = cplx_sub(a[0][3], cplx_mul(factor2_0, a[2][3]));
        b[0] = cplx_sub(b[0], cplx_mul(factor2_0, b[2]));
    }
    let factor2_1 = a[1][2];
    if (cplx_norm_sqr(factor2_1) != 0.0) {
        a[1][2] = cplx_sub(a[1][2], cplx_mul(factor2_1, a[2][2]));
        a[1][3] = cplx_sub(a[1][3], cplx_mul(factor2_1, a[2][3]));
        b[1] = cplx_sub(b[1], cplx_mul(factor2_1, b[2]));
    }
    let factor2_3 = a[3][2];
    if (cplx_norm_sqr(factor2_3) != 0.0) {
        a[3][2] = cplx_sub(a[3][2], cplx_mul(factor2_3, a[2][2]));
        a[3][3] = cplx_sub(a[3][3], cplx_mul(factor2_3, a[2][3]));
        b[3] = cplx_sub(b[3], cplx_mul(factor2_3, b[2]));
    }
    // column 3: pivot search
    var piv3: u32 = 3u;
    var piv_mag3: f32 = cplx_norm_sqr(a[3][3]);
    let pivot3 = a[3][3];
    a[3][3] = cplx_div(a[3][3], pivot3);
    b[3] = cplx_div(b[3], pivot3);
    let factor3_0 = a[0][3];
    if (cplx_norm_sqr(factor3_0) != 0.0) {
        a[0][3] = cplx_sub(a[0][3], cplx_mul(factor3_0, a[3][3]));
        b[0] = cplx_sub(b[0], cplx_mul(factor3_0, b[3]));
    }
    let factor3_1 = a[1][3];
    if (cplx_norm_sqr(factor3_1) != 0.0) {
        a[1][3] = cplx_sub(a[1][3], cplx_mul(factor3_1, a[3][3]));
        b[1] = cplx_sub(b[1], cplx_mul(factor3_1, b[3]));
    }
    let factor3_2 = a[2][3];
    if (cplx_norm_sqr(factor3_2) != 0.0) {
        a[2][3] = cplx_sub(a[2][3], cplx_mul(factor3_2, a[3][3]));
        b[2] = cplx_sub(b[2], cplx_mul(factor3_2, b[3]));
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
    // column 0: pivot search
    var piv0: u32 = 0u;
    var piv_mag0: f32 = cplx_norm_sqr(a[0][0]);
    let mag0_1 = cplx_norm_sqr(a[1][0]);
    if (mag0_1 > piv_mag0) {
        piv0 = 1u;
        piv_mag0 = mag0_1;
    }
    let mag0_2 = cplx_norm_sqr(a[2][0]);
    if (mag0_2 > piv_mag0) {
        piv0 = 2u;
        piv_mag0 = mag0_2;
    }
    let mag0_3 = cplx_norm_sqr(a[3][0]);
    if (mag0_3 > piv_mag0) {
        piv0 = 3u;
        piv_mag0 = mag0_3;
    }
    if (piv0 == 1u) {
        let t0_1_0 = a[0][0];
        a[0][0] = a[1][0];
        a[1][0] = t0_1_0;
        let t0_1_1 = a[0][1];
        a[0][1] = a[1][1];
        a[1][1] = t0_1_1;
        let t0_1_2 = a[0][2];
        a[0][2] = a[1][2];
        a[1][2] = t0_1_2;
        let t0_1_3 = a[0][3];
        a[0][3] = a[1][3];
        a[1][3] = t0_1_3;
        let tb0_1_b1 = b1[0];
        b1[0] = b1[1];
        b1[1] = tb0_1_b1;
        let tb0_1_b2 = b2[0];
        b2[0] = b2[1];
        b2[1] = tb0_1_b2;
    }
    if (piv0 == 2u) {
        let t0_2_0 = a[0][0];
        a[0][0] = a[2][0];
        a[2][0] = t0_2_0;
        let t0_2_1 = a[0][1];
        a[0][1] = a[2][1];
        a[2][1] = t0_2_1;
        let t0_2_2 = a[0][2];
        a[0][2] = a[2][2];
        a[2][2] = t0_2_2;
        let t0_2_3 = a[0][3];
        a[0][3] = a[2][3];
        a[2][3] = t0_2_3;
        let tb0_2_b1 = b1[0];
        b1[0] = b1[2];
        b1[2] = tb0_2_b1;
        let tb0_2_b2 = b2[0];
        b2[0] = b2[2];
        b2[2] = tb0_2_b2;
    }
    if (piv0 == 3u) {
        let t0_3_0 = a[0][0];
        a[0][0] = a[3][0];
        a[3][0] = t0_3_0;
        let t0_3_1 = a[0][1];
        a[0][1] = a[3][1];
        a[3][1] = t0_3_1;
        let t0_3_2 = a[0][2];
        a[0][2] = a[3][2];
        a[3][2] = t0_3_2;
        let t0_3_3 = a[0][3];
        a[0][3] = a[3][3];
        a[3][3] = t0_3_3;
        let tb0_3_b1 = b1[0];
        b1[0] = b1[3];
        b1[3] = tb0_3_b1;
        let tb0_3_b2 = b2[0];
        b2[0] = b2[3];
        b2[3] = tb0_3_b2;
    }
    let pivot0 = a[0][0];
    a[0][0] = cplx_div(a[0][0], pivot0);
    a[0][1] = cplx_div(a[0][1], pivot0);
    a[0][2] = cplx_div(a[0][2], pivot0);
    a[0][3] = cplx_div(a[0][3], pivot0);
    b1[0] = cplx_div(b1[0], pivot0);
    b2[0] = cplx_div(b2[0], pivot0);
    let factor0_1 = a[1][0];
    if (cplx_norm_sqr(factor0_1) != 0.0) {
        a[1][0] = cplx_sub(a[1][0], cplx_mul(factor0_1, a[0][0]));
        a[1][1] = cplx_sub(a[1][1], cplx_mul(factor0_1, a[0][1]));
        a[1][2] = cplx_sub(a[1][2], cplx_mul(factor0_1, a[0][2]));
        a[1][3] = cplx_sub(a[1][3], cplx_mul(factor0_1, a[0][3]));
        b1[1] = cplx_sub(b1[1], cplx_mul(factor0_1, b1[0]));
        b2[1] = cplx_sub(b2[1], cplx_mul(factor0_1, b2[0]));
    }
    let factor0_2 = a[2][0];
    if (cplx_norm_sqr(factor0_2) != 0.0) {
        a[2][0] = cplx_sub(a[2][0], cplx_mul(factor0_2, a[0][0]));
        a[2][1] = cplx_sub(a[2][1], cplx_mul(factor0_2, a[0][1]));
        a[2][2] = cplx_sub(a[2][2], cplx_mul(factor0_2, a[0][2]));
        a[2][3] = cplx_sub(a[2][3], cplx_mul(factor0_2, a[0][3]));
        b1[2] = cplx_sub(b1[2], cplx_mul(factor0_2, b1[0]));
        b2[2] = cplx_sub(b2[2], cplx_mul(factor0_2, b2[0]));
    }
    let factor0_3 = a[3][0];
    if (cplx_norm_sqr(factor0_3) != 0.0) {
        a[3][0] = cplx_sub(a[3][0], cplx_mul(factor0_3, a[0][0]));
        a[3][1] = cplx_sub(a[3][1], cplx_mul(factor0_3, a[0][1]));
        a[3][2] = cplx_sub(a[3][2], cplx_mul(factor0_3, a[0][2]));
        a[3][3] = cplx_sub(a[3][3], cplx_mul(factor0_3, a[0][3]));
        b1[3] = cplx_sub(b1[3], cplx_mul(factor0_3, b1[0]));
        b2[3] = cplx_sub(b2[3], cplx_mul(factor0_3, b2[0]));
    }
    // column 1: pivot search
    var piv1: u32 = 1u;
    var piv_mag1: f32 = cplx_norm_sqr(a[1][1]);
    let mag1_2 = cplx_norm_sqr(a[2][1]);
    if (mag1_2 > piv_mag1) {
        piv1 = 2u;
        piv_mag1 = mag1_2;
    }
    let mag1_3 = cplx_norm_sqr(a[3][1]);
    if (mag1_3 > piv_mag1) {
        piv1 = 3u;
        piv_mag1 = mag1_3;
    }
    if (piv1 == 2u) {
        let t1_2_0 = a[1][0];
        a[1][0] = a[2][0];
        a[2][0] = t1_2_0;
        let t1_2_1 = a[1][1];
        a[1][1] = a[2][1];
        a[2][1] = t1_2_1;
        let t1_2_2 = a[1][2];
        a[1][2] = a[2][2];
        a[2][2] = t1_2_2;
        let t1_2_3 = a[1][3];
        a[1][3] = a[2][3];
        a[2][3] = t1_2_3;
        let tb1_2_b1 = b1[1];
        b1[1] = b1[2];
        b1[2] = tb1_2_b1;
        let tb1_2_b2 = b2[1];
        b2[1] = b2[2];
        b2[2] = tb1_2_b2;
    }
    if (piv1 == 3u) {
        let t1_3_0 = a[1][0];
        a[1][0] = a[3][0];
        a[3][0] = t1_3_0;
        let t1_3_1 = a[1][1];
        a[1][1] = a[3][1];
        a[3][1] = t1_3_1;
        let t1_3_2 = a[1][2];
        a[1][2] = a[3][2];
        a[3][2] = t1_3_2;
        let t1_3_3 = a[1][3];
        a[1][3] = a[3][3];
        a[3][3] = t1_3_3;
        let tb1_3_b1 = b1[1];
        b1[1] = b1[3];
        b1[3] = tb1_3_b1;
        let tb1_3_b2 = b2[1];
        b2[1] = b2[3];
        b2[3] = tb1_3_b2;
    }
    let pivot1 = a[1][1];
    a[1][1] = cplx_div(a[1][1], pivot1);
    a[1][2] = cplx_div(a[1][2], pivot1);
    a[1][3] = cplx_div(a[1][3], pivot1);
    b1[1] = cplx_div(b1[1], pivot1);
    b2[1] = cplx_div(b2[1], pivot1);
    let factor1_0 = a[0][1];
    if (cplx_norm_sqr(factor1_0) != 0.0) {
        a[0][1] = cplx_sub(a[0][1], cplx_mul(factor1_0, a[1][1]));
        a[0][2] = cplx_sub(a[0][2], cplx_mul(factor1_0, a[1][2]));
        a[0][3] = cplx_sub(a[0][3], cplx_mul(factor1_0, a[1][3]));
        b1[0] = cplx_sub(b1[0], cplx_mul(factor1_0, b1[1]));
        b2[0] = cplx_sub(b2[0], cplx_mul(factor1_0, b2[1]));
    }
    let factor1_2 = a[2][1];
    if (cplx_norm_sqr(factor1_2) != 0.0) {
        a[2][1] = cplx_sub(a[2][1], cplx_mul(factor1_2, a[1][1]));
        a[2][2] = cplx_sub(a[2][2], cplx_mul(factor1_2, a[1][2]));
        a[2][3] = cplx_sub(a[2][3], cplx_mul(factor1_2, a[1][3]));
        b1[2] = cplx_sub(b1[2], cplx_mul(factor1_2, b1[1]));
        b2[2] = cplx_sub(b2[2], cplx_mul(factor1_2, b2[1]));
    }
    let factor1_3 = a[3][1];
    if (cplx_norm_sqr(factor1_3) != 0.0) {
        a[3][1] = cplx_sub(a[3][1], cplx_mul(factor1_3, a[1][1]));
        a[3][2] = cplx_sub(a[3][2], cplx_mul(factor1_3, a[1][2]));
        a[3][3] = cplx_sub(a[3][3], cplx_mul(factor1_3, a[1][3]));
        b1[3] = cplx_sub(b1[3], cplx_mul(factor1_3, b1[1]));
        b2[3] = cplx_sub(b2[3], cplx_mul(factor1_3, b2[1]));
    }
    // column 2: pivot search
    var piv2: u32 = 2u;
    var piv_mag2: f32 = cplx_norm_sqr(a[2][2]);
    let mag2_3 = cplx_norm_sqr(a[3][2]);
    if (mag2_3 > piv_mag2) {
        piv2 = 3u;
        piv_mag2 = mag2_3;
    }
    if (piv2 == 3u) {
        let t2_3_0 = a[2][0];
        a[2][0] = a[3][0];
        a[3][0] = t2_3_0;
        let t2_3_1 = a[2][1];
        a[2][1] = a[3][1];
        a[3][1] = t2_3_1;
        let t2_3_2 = a[2][2];
        a[2][2] = a[3][2];
        a[3][2] = t2_3_2;
        let t2_3_3 = a[2][3];
        a[2][3] = a[3][3];
        a[3][3] = t2_3_3;
        let tb2_3_b1 = b1[2];
        b1[2] = b1[3];
        b1[3] = tb2_3_b1;
        let tb2_3_b2 = b2[2];
        b2[2] = b2[3];
        b2[3] = tb2_3_b2;
    }
    let pivot2 = a[2][2];
    a[2][2] = cplx_div(a[2][2], pivot2);
    a[2][3] = cplx_div(a[2][3], pivot2);
    b1[2] = cplx_div(b1[2], pivot2);
    b2[2] = cplx_div(b2[2], pivot2);
    let factor2_0 = a[0][2];
    if (cplx_norm_sqr(factor2_0) != 0.0) {
        a[0][2] = cplx_sub(a[0][2], cplx_mul(factor2_0, a[2][2]));
        a[0][3] = cplx_sub(a[0][3], cplx_mul(factor2_0, a[2][3]));
        b1[0] = cplx_sub(b1[0], cplx_mul(factor2_0, b1[2]));
        b2[0] = cplx_sub(b2[0], cplx_mul(factor2_0, b2[2]));
    }
    let factor2_1 = a[1][2];
    if (cplx_norm_sqr(factor2_1) != 0.0) {
        a[1][2] = cplx_sub(a[1][2], cplx_mul(factor2_1, a[2][2]));
        a[1][3] = cplx_sub(a[1][3], cplx_mul(factor2_1, a[2][3]));
        b1[1] = cplx_sub(b1[1], cplx_mul(factor2_1, b1[2]));
        b2[1] = cplx_sub(b2[1], cplx_mul(factor2_1, b2[2]));
    }
    let factor2_3 = a[3][2];
    if (cplx_norm_sqr(factor2_3) != 0.0) {
        a[3][2] = cplx_sub(a[3][2], cplx_mul(factor2_3, a[2][2]));
        a[3][3] = cplx_sub(a[3][3], cplx_mul(factor2_3, a[2][3]));
        b1[3] = cplx_sub(b1[3], cplx_mul(factor2_3, b1[2]));
        b2[3] = cplx_sub(b2[3], cplx_mul(factor2_3, b2[2]));
    }
    // column 3: pivot search
    var piv3: u32 = 3u;
    var piv_mag3: f32 = cplx_norm_sqr(a[3][3]);
    let pivot3 = a[3][3];
    a[3][3] = cplx_div(a[3][3], pivot3);
    b1[3] = cplx_div(b1[3], pivot3);
    b2[3] = cplx_div(b2[3], pivot3);
    let factor3_0 = a[0][3];
    if (cplx_norm_sqr(factor3_0) != 0.0) {
        a[0][3] = cplx_sub(a[0][3], cplx_mul(factor3_0, a[3][3]));
        b1[0] = cplx_sub(b1[0], cplx_mul(factor3_0, b1[3]));
        b2[0] = cplx_sub(b2[0], cplx_mul(factor3_0, b2[3]));
    }
    let factor3_1 = a[1][3];
    if (cplx_norm_sqr(factor3_1) != 0.0) {
        a[1][3] = cplx_sub(a[1][3], cplx_mul(factor3_1, a[3][3]));
        b1[1] = cplx_sub(b1[1], cplx_mul(factor3_1, b1[3]));
        b2[1] = cplx_sub(b2[1], cplx_mul(factor3_1, b2[3]));
    }
    let factor3_2 = a[2][3];
    if (cplx_norm_sqr(factor3_2) != 0.0) {
        a[2][3] = cplx_sub(a[2][3], cplx_mul(factor3_2, a[3][3]));
        b1[2] = cplx_sub(b1[2], cplx_mul(factor3_2, b1[3]));
        b2[2] = cplx_sub(b2[2], cplx_mul(factor3_2, b2[3]));
    }
    var result: Solve4TwoRhsResult;
    result.b1 = b1;
    result.b2 = b2;
    return result;
}
// END GENERATED solve4

