// ---------------------------------------------------------------------------------
// Concave tools: the WGSL twin of `geometry::tool` (pieces, signed distance, outward
// normal), `geometry::tool::interval` (closed-form ray intervals) and the carve of
// `optics::raytracer::intersect_stone`. The CPU code is the reference; this file is
// operation-for-operation where the arithmetic matters (`fma` where Rust writes
// `mul_add`) and differs only where WGSL forces it:
//
//   * No arrays. A tool decomposes into at most six convex pieces; `ct_piece(tool, i)`
//     rebuilds piece `i` from the tool record on demand, in the same order as
//     `ToolPrimitive::pieces`, so there is no function-scope array to spill or to
//     crash a driver that dislikes them.
//   * No sort. The CPU `carve` sorts every boundary of `[a, b] \ union([c_k, d_k])`.
//     The GPU instead scans the tools once per pass for "is `pos` inside a tool, where
//     does that tool union end, where is the next tool entry" (`CtScan`) and walks
//     `pos` forward (`ct_carve_step`). The result is the same first change of the
//     "inside the stone" state past `CT_T_FLOOR`; only an exact tie between two
//     boundaries (measure zero) can resolve differently.
//
// Everything here is pure (no bindings). The scene records live in the `planes` storage
// binding after the planes (see `02_scene_bindings.wgsl`), five `vec4`s per tool:
//   v0 = (kind, sweep_kind, 0, 0)   as exact small floats, not bit-cast integers
//   v1 = (origin.xyz, rim radius)
//   v2 = (axis.xyz, half length)
//   v3 = (r_neg, r_pos, half stroke, 0)
//   v4 = (sweep_dir.xyz, 0)
// `renderer::gpu::frame::tools::pack_tools` is the Rust producer.
// ---------------------------------------------------------------------------------

const CT_RECORD_VECS: u32 = 5u;
// `intersect_stone::T_FLOOR`: hits at or before this parameter are the ray's own origin.
const CT_T_FLOOR: f32 = 1e-4;
// `interval::TANGENT_EPS` / `PARALLEL_EPS`.
const CT_TANGENT_EPS: f32 = 1e-7;
const CT_PARALLEL_EPS: f32 = 1e-7;
const CT_BIG: f32 = 1e30;
// Stands in for `f32::INFINITY` in `ct_tool_outward_normal`'s running minimum.
const CT_INF: f32 = 3.0e38;

const CT_STATE_SCAN: u32 = 0u;
const CT_STATE_MISS: u32 = 1u;
const CT_STATE_HIT_NEAR: u32 = 2u;
const CT_STATE_HIT_FAR: u32 = 3u;
const CT_STATE_HIT_TOOL: u32 = 4u;

struct CtTool {
    kind: u32,
    sweep_kind: u32,
    origin: vec3<f32>,
    rim: f32,
    axis: vec3<f32>,
    half_len: f32,
    r_neg: f32,
    r_pos: f32,
    stroke: f32,
    sweep_dir: vec3<f32>,
}

// One convex piece: shape 0 ball (`c`, radius in `r0`), 1 frustum (`c`, `a`, `hl`, `r0`,
// `r1`), 2 wedge (`c`, `a`, `d`, `hl`, `s`, `r0`, `r1`).
struct CtPiece {
    shape: u32,
    c: vec3<f32>,
    a: vec3<f32>,
    d: vec3<f32>,
    hl: f32,
    s: f32,
    r0: f32,
    r1: f32,
}

struct CtInterval {
    hit: bool,
    lo: f32,
    hi: f32,
}

struct CtSd {
    sd: f32,
    n: vec3<f32>,
}

struct CtScan {
    covered: bool,
    cover_end: f32,
    cover_k: u32,
    next_c: f32,
    next_k: u32,
}

struct CtCarve {
    state: u32,
    pos: f32,
    start_kind: u32,
    start_k: u32,
    hit_t: f32,
    hit_k: u32,
    // Valid in a `CT_STATE_HIT_*` state: true when the hit is an entry into the material
    // (the stretch of ray before it is in air or in a cavity), false when it is an exit. It
    // is the `now_inside` the CPU `carve` returns with its hit, the state the intersection
    // itself walked to, which the bounce adopts instead of toggling `inside_gem` (see
    // `bounce_extend`).
    entering: bool,
}

fn ct_tool_from_vecs(v0: vec4<f32>, v1: vec4<f32>, v2: vec4<f32>, v3: vec4<f32>, v4: vec4<f32>) -> CtTool {
    var t: CtTool;
    t.kind = u32(v0.x);
    t.sweep_kind = u32(v0.y);
    t.origin = v1.xyz;
    t.rim = v1.w;
    t.axis = v2.xyz;
    t.half_len = v2.w;
    t.r_neg = v3.x;
    t.r_pos = v3.y;
    t.stroke = v3.z;
    t.sweep_dir = v4.xyz;
    return t;
}

fn ct_copysign(x: f32, s: f32) -> f32 {
    return bitcast<f32>((bitcast<u32>(x) & 0x7fffffffu) | (bitcast<u32>(s) & 0x80000000u));
}

fn ct_none() -> CtInterval {
    var r: CtInterval;
    r.hit = false;
    r.lo = 0.0;
    r.hi = 0.0;
    return r;
}

fn ct_some(lo: f32, hi: f32) -> CtInterval {
    var r: CtInterval;
    r.hit = true;
    r.lo = lo;
    r.hi = hi;
    return r;
}

// ---- decomposition: `ToolPrimitive::pieces` ----

fn ct_is_swept(t: CtTool) -> bool {
    return t.sweep_kind != 0u && t.stroke > 0.0;
}

fn ct_piece_count(t: CtTool) -> u32 {
    if (t.kind > 3u || t.sweep_kind > 2u) {
        return 0u;
    }
    let swept = ct_is_swept(t);
    if (t.kind == 0u) {
        if (swept) {
            return 3u;
        }
        return 1u;
    }
    var nseg = 1u;
    if (t.kind == 3u) {
        nseg = 2u;
    }
    if (!swept) {
        return nseg;
    }
    if (t.sweep_kind == 1u) {
        return nseg + 1u;
    }
    return nseg * 3u;
}

// `ToolPrimitive::segments`: segment `si` of the radius profile as
// `(z_lo, z_hi, r_lo, r_hi)`. A cylinder and a frustum have one; a bicone has two.
fn ct_segment(t: CtTool, si: u32) -> vec4<f32> {
    let hl = t.half_len;
    if (t.kind == 2u) {
        return vec4<f32>(-hl, hl, t.r_neg, t.r_pos);
    }
    if (t.kind == 3u) {
        if (si == 0u) {
            return vec4<f32>(-hl, 0.0, t.r_neg, t.rim);
        }
        return vec4<f32>(0.0, hl, t.rim, t.r_pos);
    }
    return vec4<f32>(-hl, hl, t.rim, t.rim);
}

// The peak vertex of the profile `(axial, radius)`, where the plateau of an along-axis sweep
// sits.
fn ct_peak(t: CtTool) -> vec2<f32> {
    if (t.kind == 2u) {
        if (t.r_pos >= t.r_neg) {
            return vec2<f32>(t.half_len, t.r_pos);
        }
        return vec2<f32>(-t.half_len, t.r_neg);
    }
    if (t.kind == 3u) {
        return vec2<f32>(0.0, t.rim);
    }
    return vec2<f32>(t.half_len, t.rim);
}

fn ct_piece(t: CtTool, i: u32) -> CtPiece {
    var p: CtPiece;
    p.shape = 0u;
    p.c = t.origin;
    p.a = t.axis;
    p.d = vec3<f32>(0.0, 0.0, 0.0);
    p.hl = 0.0;
    p.s = 0.0;
    p.r0 = 0.0;
    p.r1 = 0.0;
    let swept = ct_is_swept(t);
    if (t.kind == 0u) {
        let r = t.rim;
        if (swept) {
            var dir = t.axis;
            if (t.sweep_kind == 2u) {
                dir = t.sweep_dir;
            }
            if (i == 0u) {
                p.c = t.origin - dir * t.stroke;
                p.r0 = r;
            } else if (i == 1u) {
                p.c = t.origin + dir * t.stroke;
                p.r0 = r;
            } else {
                p.shape = 1u;
                p.c = t.origin;
                p.a = dir;
                p.hl = t.stroke;
                p.r0 = r;
                p.r1 = r;
            }
        } else {
            p.r0 = r;
        }
        return p;
    }
    var nseg = 1u;
    if (t.kind == 3u) {
        nseg = 2u;
    }
    // The plateau of an along-axis sweep follows every segment piece.
    if (swept && t.sweep_kind == 1u && i >= nseg) {
        let peak = ct_peak(t);
        p.shape = 1u;
        p.c = t.origin + t.axis * peak.x;
        p.hl = t.stroke;
        p.r0 = peak.y;
        p.r1 = peak.y;
        return p;
    }
    var seg_index = i;
    var sub = 0u;
    if (swept && t.sweep_kind == 2u) {
        seg_index = i / 3u;
        sub = i % 3u;
    }
    let sg = ct_segment(t, seg_index);
    let base = t.origin + t.axis * (0.5 * (sg.x + sg.y));
    p.a = t.axis;
    p.hl = 0.5 * (sg.y - sg.x);
    p.r0 = sg.z;
    p.r1 = sg.w;
    p.shape = 1u;
    p.c = base;
    if (!swept) {
        return p;
    }
    if (t.sweep_kind == 1u) {
        if (sg.w >= sg.z) {
            p.c = base - t.axis * t.stroke;
        } else {
            p.c = base + t.axis * t.stroke;
        }
        return p;
    }
    if (sub == 0u) {
        p.c = base - t.sweep_dir * t.stroke;
    } else if (sub == 1u) {
        p.c = base + t.sweep_dir * t.stroke;
    } else {
        p.shape = 2u;
        p.d = t.sweep_dir;
        p.s = t.stroke;
    }
    return p;
}

// ---- ray intervals: `geometry::tool::interval` ----

fn ct_quadratic_roots(a: f32, b: f32, c: f32) -> CtInterval {
    let disc = fma(b, b, -(a * c));
    if (disc <= CT_TANGENT_EPS * fma(b, b, abs(a * c))) {
        return ct_none();
    }
    let sq = sqrt(disc);
    let q = -(b + ct_copysign(sq, b));
    let r1 = q / a;
    let r2 = c / q;
    if (r1 <= r2) {
        return ct_some(r1, r2);
    }
    return ct_some(r2, r1);
}

fn ct_ball_interval(o_world: vec3<f32>, dir: vec3<f32>, c: vec3<f32>, r: f32) -> CtInterval {
    let o = o_world - c;
    let a = dot(dir, dir);
    let b = dot(o, dir);
    let cc = fma(r, -r, dot(o, o));
    return ct_quadratic_roots(a, b, cc);
}

fn ct_cap_range(z0: f32, dz: f32, hl: f32) -> CtInterval {
    if (abs(dz) > CT_PARALLEL_EPS) {
        let t1 = (-hl - z0) / dz;
        let t2 = (hl - z0) / dz;
        if (t1 <= t2) {
            return ct_some(t1, t2);
        }
        return ct_some(t2, t1);
    }
    if (abs(z0) <= hl) {
        return ct_some(-CT_BIG, CT_BIG);
    }
    return ct_none();
}

fn ct_clip(lo: f32, hi: f32, s0: f32, s1: f32) -> CtInterval {
    let l = max(lo, s0);
    let h = min(hi, s1);
    if (l <= h) {
        return ct_some(l, h);
    }
    return ct_none();
}

fn ct_frustum_interval(o_world: vec3<f32>, dir: vec3<f32>, c: vec3<f32>, a: vec3<f32>, hl: f32, r0: f32, r1: f32) -> CtInterval {
    let o = o_world - c;
    let z0 = dot(o, a);
    let dz = dot(dir, a);
    let caps = ct_cap_range(z0, dz, hl);
    if (!caps.hit) {
        return ct_none();
    }
    let s0 = caps.lo;
    let s1 = caps.hi;
    let rm = 0.5 * (r0 + r1);
    let k = (r1 - r0) / (2.0 * hl);
    let p = o - a * z0;
    let q = dir - a * dz;
    let rz0 = fma(k, z0, rm);
    let rate = k * dz;
    let qa = fma(rate, -rate, dot(q, q));
    let qb = fma(-rate, rz0, dot(p, q));
    let qc = fma(rz0, -rz0, dot(p, p));
    if (abs(qa) <= CT_PARALLEL_EPS) {
        // Ray parallel to a generator: the radius condition is linear in t.
        if (abs(qb) <= CT_PARALLEL_EPS) {
            if (qc <= 0.0) {
                return ct_some(s0, s1);
            }
            return ct_none();
        }
        let t = -qc / (2.0 * qb);
        if (qb > 0.0) {
            return ct_clip(-CT_BIG, t, s0, s1);
        }
        return ct_clip(t, CT_BIG, s0, s1);
    }
    let roots = ct_quadratic_roots(qa, qb, qc);
    if (!roots.hit) {
        return ct_none();
    }
    if (qa > 0.0) {
        return ct_clip(roots.lo, roots.hi, s0, s1);
    }
    // Steeper than the cone: `Q <= 0` outside the roots. The mirror nappe lies beyond the
    // caps, so at most one side survives clipping; the hull keeps an interval if rounding
    // lets both through.
    let x = ct_clip(-CT_BIG, roots.lo, s0, s1);
    let y = ct_clip(roots.hi, CT_BIG, s0, s1);
    if (x.hit && y.hit) {
        return ct_some(min(x.lo, y.lo), max(x.hi, y.hi));
    }
    if (x.hit) {
        return x;
    }
    return y;
}

// One face of the wedge's slab test: `normal . (x - c) + offset <= 0` inside. Returns false
// when the ray is parallel to the face and outside it (the wedge is then missed).
fn ct_wedge_face(n: vec3<f32>, e: f32, o: vec3<f32>, dir: vec3<f32>, t_near: ptr<function, f32>, t_far: ptr<function, f32>) -> bool {
    let denom = dot(n, dir);
    let side = dot(n, o) + e;
    if (abs(denom) > CT_PARALLEL_EPS) {
        let t = -side / denom;
        if (denom < 0.0) {
            *t_near = max(*t_near, t);
        } else {
            *t_far = min(*t_far, t);
        }
    } else if (side > 0.0) {
        return false;
    }
    return true;
}

fn ct_wedge_interval(o_world: vec3<f32>, dir: vec3<f32>, c: vec3<f32>, a: vec3<f32>, d: vec3<f32>, hl: f32, s: f32, r0: f32, r1: f32) -> CtInterval {
    let w = cross(a, d);
    let k = (r1 - r0) / (2.0 * hl);
    let rm = 0.5 * (r0 + r1);
    let o = o_world - c;
    var t_near: f32 = -CT_BIG;
    var t_far: f32 = CT_BIG;
    // The six faces, in the CPU order. Normals are deliberately not normalised: the slab
    // method only needs ratios.
    if (!ct_wedge_face(a, -hl, o, dir, &t_near, &t_far)) { return ct_none(); }
    if (!ct_wedge_face(-a, -hl, o, dir, &t_near, &t_far)) { return ct_none(); }
    if (!ct_wedge_face(d, -s, o, dir, &t_near, &t_far)) { return ct_none(); }
    if (!ct_wedge_face(-d, -s, o, dir, &t_near, &t_far)) { return ct_none(); }
    if (!ct_wedge_face(w - a * k, -rm, o, dir, &t_near, &t_far)) { return ct_none(); }
    if (!ct_wedge_face(-w - a * k, -rm, o, dir, &t_near, &t_far)) { return ct_none(); }
    if (t_near <= t_far) {
        return ct_some(t_near, t_far);
    }
    return ct_none();
}

fn ct_piece_interval(pc: CtPiece, o: vec3<f32>, dir: vec3<f32>) -> CtInterval {
    if (pc.shape == 0u) {
        return ct_ball_interval(o, dir, pc.c, pc.r0);
    }
    if (pc.shape == 1u) {
        return ct_frustum_interval(o, dir, pc.c, pc.a, pc.hl, pc.r0, pc.r1);
    }
    return ct_wedge_interval(o, dir, pc.c, pc.a, pc.d, pc.hl, pc.s, pc.r0, pc.r1);
}

// ---- signed distance estimate and normal: `Piece::sd_normal` ----

// glam's `Vec3::any_orthonormal_vector`.
fn ct_any_orthonormal(v: vec3<f32>) -> vec3<f32> {
    let sgn = ct_copysign(1.0, v.z);
    let a = -1.0 / (sgn + v.z);
    let b = v.x * v.y * a;
    return vec3<f32>(b, sgn + v.x * v.x * a, -v.x);
}

fn ct_piece_sd_normal(pc: CtPiece, p: vec3<f32>) -> CtSd {
    var out: CtSd;
    out.sd = 0.0;
    out.n = vec3<f32>(0.0, 0.0, 1.0);
    if (pc.shape == 0u) {
        let q = p - pc.c;
        let len = length(q);
        out.sd = len - pc.r0;
        if (len > 1e-12) {
            out.n = q / len;
        }
        return out;
    }
    if (pc.shape == 1u) {
        let q = p - pc.c;
        let z = dot(q, pc.a);
        let perp = q - pc.a * z;
        let rho = length(perp);
        var rho_hat = ct_any_orthonormal(pc.a);
        if (rho > 1e-12) {
            rho_hat = perp / rho;
        }
        var cap_sd = z - pc.hl;
        var cap_n = pc.a;
        if (z < 0.0) {
            cap_sd = -z - pc.hl;
            cap_n = -pc.a;
        }
        let dr = pc.r1 - pc.r0;
        let len = sqrt(fma(4.0 * pc.hl, pc.hl, dr * dr));
        let rm = 0.5 * (pc.r0 + pc.r1);
        let side_sd = fma(z, -dr, (rho - rm) * 2.0 * pc.hl) / len;
        let side_n = rho_hat * (2.0 * pc.hl / len) - pc.a * (dr / len);
        if (side_sd > cap_sd) {
            out.sd = side_sd;
            out.n = side_n;
        } else {
            out.sd = cap_sd;
            out.n = cap_n;
        }
        return out;
    }
    let q = p - pc.c;
    let w = cross(pc.a, pc.d);
    let z = dot(q, pc.a);
    let x = dot(q, pc.d);
    let y = dot(q, w);
    let k = (pc.r1 - pc.r0) / (2.0 * pc.hl);
    let rm = 0.5 * (pc.r0 + pc.r1);
    let kappa = 1.0 / sqrt(fma(k, k, 1.0));
    var best_sd = z - pc.hl;
    var best_n = pc.a;
    if (z < 0.0) {
        best_sd = -z - pc.hl;
        best_n = -pc.a;
    }
    var along_sd = x - pc.s;
    var along_n = pc.d;
    if (x < 0.0) {
        along_sd = -x - pc.s;
        along_n = -pc.d;
    }
    if (along_sd > best_sd) {
        best_sd = along_sd;
        best_n = along_n;
    }
    let slant_pos_sd = (y - rm - k * z) * kappa;
    if (slant_pos_sd > best_sd) {
        best_sd = slant_pos_sd;
        best_n = (w - pc.a * k) * kappa;
    }
    let slant_neg_sd = (-y - rm - k * z) * kappa;
    if (slant_neg_sd > best_sd) {
        best_sd = slant_neg_sd;
        best_n = (-w - pc.a * k) * kappa;
    }
    out.sd = best_sd;
    out.n = best_n;
    return out;
}

// ---- whole-tool queries ----

// `ToolPrimitive::ray_interval`: `[min c, max d]` over the pieces.
fn ct_tool_ray_interval(t: CtTool, o: vec3<f32>, dir: vec3<f32>) -> CtInterval {
    var out = ct_none();
    let n = ct_piece_count(t);
    for (var i: u32 = 0u; i < n; i = i + 1u) {
        let iv = ct_piece_interval(ct_piece(t, i), o, dir);
        if (iv.hit) {
            if (!out.hit) {
                out = iv;
            } else {
                out.lo = min(out.lo, iv.lo);
                out.hi = max(out.hi, iv.hi);
            }
        }
    }
    return out;
}

// `ToolPrimitive::outward_normal`: the normal of the piece whose surface `p` is closest to;
// the first piece wins a tie.
fn ct_tool_outward_normal(t: CtTool, p: vec3<f32>) -> vec3<f32> {
    var best_abs: f32 = CT_INF;
    var best_n = vec3<f32>(0.0, 0.0, 1.0);
    let n = ct_piece_count(t);
    for (var i: u32 = 0u; i < n; i = i + 1u) {
        let s = ct_piece_sd_normal(ct_piece(t, i), p);
        if (abs(s.sd) < best_abs) {
            best_abs = abs(s.sd);
            best_n = s.n;
        }
    }
    return best_n;
}

// `ToolPrimitive::contains`.
fn ct_tool_contains(t: CtTool, p: vec3<f32>) -> bool {
    let n = ct_piece_count(t);
    for (var i: u32 = 0u; i < n; i = i + 1u) {
        if (ct_piece_sd_normal(ct_piece(t, i), p).sd <= 0.0) {
            return true;
        }
    }
    return false;
}

// Radius of a sphere about `origin` that holds the whole (swept) tool, with a little slack.
// Every piece lies within `half_len + stroke` along the axis and `max radius + stroke`
// across it (an along-axis sweep shifts a piece by `stroke` along the axis, an across-axis
// one by `stroke` across it).
fn ct_reach(t: CtTool) -> f32 {
    let rmax = max(t.rim, max(t.r_neg, t.r_pos));
    let ext_a = t.half_len + t.stroke;
    let ext_r = rmax + t.stroke;
    return sqrt(fma(ext_a, ext_a, ext_r * ext_r)) * 1.001 + 1e-4;
}

// A conservative reject: `false` only when the whole line misses the bounding sphere, so
// no piece interval can exist. Skips the piece math for most tools on most rays.
fn ct_ray_may_hit(t: CtTool, o: vec3<f32>, dir: vec3<f32>) -> bool {
    if (ct_piece_count(t) == 0u) {
        return false;
    }
    let reach = ct_reach(t);
    let w = cross(o - t.origin, dir);
    return dot(w, w) <= reach * reach * dot(dir, dir);
}

// A conservative reject for a point: `false` only when `p` is outside the bounding sphere.
fn ct_point_may_be_inside(t: CtTool, p: vec3<f32>) -> bool {
    if (ct_piece_count(t) == 0u) {
        return false;
    }
    let reach = ct_reach(t);
    let q = p - t.origin;
    return dot(q, q) <= reach * reach;
}

// ---- the carve ----
//
// The stone is `[a, b]` (the polyhedron's slab interval along the ray) minus the union of
// the tool intervals. `ct_carve_begin` starts at `CT_T_FLOOR` (or at `a` when the ray
// starts outside the slab); each `ct_carve_step` takes one scan of the tools at `pos` and
// either settles the answer or moves `pos` to the end of the tool union covering it.

fn ct_scan_init() -> CtScan {
    var s: CtScan;
    s.covered = false;
    s.cover_end = -CT_BIG;
    s.cover_k = 0u;
    s.next_c = CT_BIG;
    s.next_k = 0u;
    return s;
}

// Folds tool `k`'s interval into the scan at position `pos`. A tool covers `pos` when
// `lo <= pos < hi`; the covering tool with the largest `hi` ends the blocked stretch. A
// tool starting after `pos` is a candidate for the next entry; the earliest wins.
fn ct_scan_add(s_in: CtScan, k: u32, iv: CtInterval, pos: f32) -> CtScan {
    var s = s_in;
    if (!iv.hit) {
        return s;
    }
    if (iv.lo <= pos && pos < iv.hi) {
        if (!s.covered || iv.hi > s.cover_end) {
            s.cover_end = iv.hi;
            s.cover_k = k;
        }
        s.covered = true;
    } else if (iv.lo > pos) {
        if (iv.lo < s.next_c) {
            s.next_c = iv.lo;
            s.next_k = k;
        }
    }
    return s;
}

fn ct_carve_begin(a: f32, b: f32) -> CtCarve {
    var st: CtCarve;
    st.state = CT_STATE_SCAN;
    st.pos = CT_T_FLOOR;
    st.start_kind = 0u;
    st.start_k = 0u;
    st.hit_t = 0.0;
    st.hit_k = 0u;
    st.entering = false;
    if (st.pos < a) {
        // Outside the slab: the stone can only begin at the near plane or later.
        st.pos = a;
        st.start_kind = 1u;
    } else if (st.pos >= b) {
        st.state = CT_STATE_MISS;
    }
    return st;
}

fn ct_carve_step(st_in: CtCarve, acc: CtScan, b: f32) -> CtCarve {
    var st = st_in;
    if (acc.covered) {
        // Blocked by a tool: the stone resumes where the tool union ends.
        st.pos = acc.cover_end;
        st.start_kind = 2u;
        st.start_k = acc.cover_k;
        if (st.pos >= b) {
            st.state = CT_STATE_MISS;
        }
        return st;
    }
    if (st.start_kind == 1u) {
        // Air to material at the near plane.
        st.state = CT_STATE_HIT_NEAR;
        st.hit_t = st.pos;
        st.entering = true;
        return st;
    }
    if (st.start_kind == 2u) {
        st.state = CT_STATE_HIT_TOOL;
        st.hit_t = st.pos;
        st.hit_k = st.start_k;
        // Cavity to material where the tool union ends.
        st.entering = true;
        return st;
    }
    // The ray starts inside the material: it leaves at the earlier of the far plane and the
    // next tool entry.
    if (acc.next_c < b) {
        st.state = CT_STATE_HIT_TOOL;
        st.hit_t = acc.next_c;
        st.hit_k = acc.next_k;
        // Material to cavity at the next tool entry.
        st.entering = false;
        return st;
    }
    st.state = CT_STATE_HIT_FAR;
    st.hit_t = b;
    st.entering = false;
    return st;
}
