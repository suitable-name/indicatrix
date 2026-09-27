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

