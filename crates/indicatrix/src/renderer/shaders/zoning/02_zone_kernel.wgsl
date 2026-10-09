// ---------------------------------------------------------------------------------
// Zoned absorption, unit 2 of 3: the path-length kernel. `zoning` feature only.
//
// A function-by-function port of `k32` in `optics/zoning/kernels.rs` (the f32 kernel, itself
// the same source as the f64 reference): same operation order, same breakpoint array sizes,
// same insertion sort, same 8-point Gauss-Legendre constants, same smoothstep. Where the
// Rust uses `a * b + c` this uses `a * b + c` too (no explicit `fma`); the shader compiler
// may contract it, which is the one place a GPU result can differ from `k32` by an ulp.
//
// What it computes: how much of the straight segment `from -> to` lies in each zone. Index 0 is
// the base zone, 1..=4 the shaped zones in order; later zones override earlier ones; the
// lengths partition the segment. Sharp boundaries are cut analytically per shape; soft
// boundaries (`softness > 0`) integrate a smoothstep weight with Gauss-Legendre pieces.
//
// MeshShell zones do not exist here: the host declines such a material (`gpu_zoning_decline`).
//
// Reads `material.zones` (declared in 01_zone_table.wgsl, bound as part of `material`).
// ---------------------------------------------------------------------------------

const ZONE_TWO_PI: f32 = 6.2831855;
// The "no limit" sentinel of `k32` (f32::MAX).
const ZONE_F32_MAX: f32 = 3.4028234e38;
// Breakpoint capacity: BP_SOFT = 2 + MAX_ZONES * (2 * SPAN_CAP * 2 + 1) = 38 (>= BP_SHARP = 18).
const ZONE_BP_CAP: u32 = 38u;

// 8-point Gauss-Legendre nodes on [-1, 1] (ascending) and weights (sum 2), as in `k32`.
var<private> ZONE_GL_X: array<f32, 8> = array<f32, 8>(
    -0.9602898564975363,
    -0.7966664774136267,
    -0.5255324099163290,
    -0.1834346424956498,
    0.1834346424956498,
    0.5255324099163290,
    0.7966664774136267,
    0.9602898564975363,
);
var<private> ZONE_GL_W: array<f32, 8> = array<f32, 8>(
    0.1012285362903763,
    0.2223810344533745,
    0.3137066458778873,
    0.3626837833783620,
    0.3626837833783620,
    0.3137066458778873,
    0.2223810344533745,
    0.1012285362903763,
);

// ---- spans: up to two sorted, disjoint spans of the segment parameter t in [0, 1] ----

struct ZSpans {
    n: u32,
    a: vec2<f32>,
    b: vec2<f32>,
}

fn zs_empty() -> ZSpans {
    return ZSpans(0u, vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 0.0));
}

fn zs_get(s: ZSpans, i: u32) -> vec2<f32> {
    if (i == 0u) {
        return s.a;
    }
    return s.b;
}

fn zs_push(s: ptr<function, ZSpans>, lo: f32, hi: f32) {
    if ((*s).n < 2u) {
        if ((*s).n == 0u) {
            (*s).a = vec2<f32>(lo, hi);
        } else {
            (*s).b = vec2<f32>(lo, hi);
        }
        (*s).n = (*s).n + 1u;
    }
}

// The span [lo, hi] clipped to [0, 1]; empty when it has no length.
fn zs_one(lo_in: f32, hi_in: f32) -> ZSpans {
    let lo = max(lo_in, 0.0);
    let hi = min(hi_in, 1.0);
    var out = zs_empty();
    if (hi > lo) {
        out.a = vec2<f32>(lo, hi);
        out.n = 1u;
    }
    return out;
}

fn zs_full() -> ZSpans {
    return zs_one(0.0, 1.0);
}

fn zs_intersect(x: ZSpans, y: ZSpans) -> ZSpans {
    var out = zs_empty();
    for (var i: u32 = 0u; i < x.n; i = i + 1u) {
        let xi = zs_get(x, i);
        for (var j: u32 = 0u; j < y.n; j = j + 1u) {
            let yj = zs_get(y, j);
            let lo = max(xi.x, yj.x);
            let hi = min(xi.y, yj.y);
            if (hi > lo) {
                zs_push(&out, lo, hi);
            }
        }
    }
    return out;
}

fn zs_subtract_one(x: ZSpans, lo: f32, hi: f32) -> ZSpans {
    if (hi <= lo) {
        return x;
    }
    var out = zs_empty();
    for (var i: u32 = 0u; i < x.n; i = i + 1u) {
        let xi = zs_get(x, i);
        let a = xi.x;
        let b = xi.y;
        let left_hi = min(b, lo);
        if (left_hi > a) {
            zs_push(&out, a, left_hi);
        }
        let right_lo = max(a, hi);
        if (b > right_lo) {
            zs_push(&out, right_lo, b);
        }
    }
    return out;
}

fn zs_subtract(x: ZSpans, y: ZSpans) -> ZSpans {
    var cur = x;
    for (var j: u32 = 0u; j < y.n; j = j + 1u) {
        let yj = zs_get(y, j);
        cur = zs_subtract_one(cur, yj.x, yj.y);
    }
    return cur;
}

fn zs_contains(x: ZSpans, t: f32) -> bool {
    for (var i: u32 = 0u; i < x.n; i = i + 1u) {
        let xi = zs_get(x, i);
        if (t >= xi.x && t <= xi.y) {
            return true;
        }
    }
    return false;
}

// { t in [0, 1] : a + b t >= 0 }
fn zone_ge0(a: f32, b: f32) -> ZSpans {
    if (b == 0.0) {
        if (a >= 0.0) {
            return zs_full();
        }
        return zs_empty();
    }
    let t = -a / b;
    if (b > 0.0) {
        return zs_one(t, 1.0);
    }
    return zs_one(0.0, t);
}

// { t : |q0 + t qd| <= r }, solved about the closest approach (no cancellation).
fn zone_disc(q0: vec2<f32>, qd: vec2<f32>, r: f32) -> ZSpans {
    if (r <= 0.0) {
        return zs_empty();
    }
    let a = qd.x * qd.x + qd.y * qd.y;
    if (a == 0.0) {
        let c = q0.x * q0.x + q0.y * q0.y;
        if (c <= r * r) {
            return zs_full();
        }
        return zs_empty();
    }
    let tc = -(q0.x * qd.x + q0.y * qd.y) / a;
    let cx = q0.x + tc * qd.x;
    let cy = q0.y + tc * qd.y;
    let rem = r * r - (cx * cx + cy * cy);
    if (rem <= 0.0) {
        return zs_empty();
    }
    let half_len = sqrt(rem / a);
    return zs_one(tc - half_len, tc + half_len);
}

// { t : gauge(q0 + t qd) <= r } for the regular polygon of apothem r.
fn zone_polygon(q0: vec2<f32>, qd: vec2<f32>, n_sides: u32, phase: f32, r: f32) -> ZSpans {
    if (r <= 0.0) {
        return zs_empty();
    }
    var s = zs_full();
    for (var k: u32 = 0u; k < n_sides; k = k + 1u) {
        let ang = phase + ZONE_TWO_PI * f32(k) / f32(n_sides);
        let sa = sin(ang);
        let ca = cos(ang);
        let m0 = ca * q0.x + sa * q0.y;
        let md = ca * qd.x + sa * qd.y;
        s = zs_intersect(s, zone_ge0(r - m0, -md));
    }
    return s;
}

fn zone_polygon_gauge(q: vec2<f32>, n_sides: u32, phase: f32) -> f32 {
    var g = -ZONE_F32_MAX;
    for (var k: u32 = 0u; k < n_sides; k = k + 1u) {
        let ang = phase + ZONE_TWO_PI * f32(k) / f32(n_sides);
        let m = cos(ang) * q.x + sin(ang) * q.y;
        if (m > g) {
            g = m;
        }
    }
    return g;
}

fn zone_cross2(a: vec2<f32>, b: vec2<f32>) -> f32 {
    return a.x * b.y - a.y * b.x;
}

struct ZoneQ {
    q0: vec2<f32>,
    qd: vec2<f32>,
}

// The segment's 2-D position about the zone axis: q(t) = q0 + t qd.
fn zone_q_of(z: GpuZoneShape, p0: vec3<f32>, dv: vec3<f32>) -> ZoneQ {
    let r0 = p0 - z.p.xyz;
    return ZoneQ(
        vec2<f32>(dot(r0, z.u.xyz), dot(r0, z.v.xyz)),
        vec2<f32>(dot(dv, z.u.xyz), dot(dv, z.v.xyz)),
    );
}

// { t in [0, 1] : depth_z(p(t)) >= c }; c = 0 is the sharp zone.
fn zone_spans(z: GpuZoneShape, p0: vec3<f32>, dv: vec3<f32>, c: f32) -> ZSpans {
    let x0 = z.x.x;
    let x1 = z.x.y;
    if (z.kind == ZONE_KIND_HALF) {
        return zone_ge0(dot(z.d.xyz, p0) - x0 - c, dot(z.d.xyz, dv));
    }
    if (z.kind == ZONE_KIND_SLAB) {
        let np = dot(z.d.xyz, p0);
        let nd = dot(z.d.xyz, dv);
        return zs_intersect(zone_ge0(np - x0 - c, nd), zone_ge0(x1 - np - c, -nd));
    }
    if (z.kind == ZONE_KIND_CYL || z.kind == ZONE_KIND_PRISM) {
        let qq = zone_q_of(z, p0, dv);
        var outer: ZSpans;
        if (z.kind == ZONE_KIND_CYL) {
            outer = zone_disc(qq.q0, qq.qd, x1 - c);
        } else {
            outer = zone_polygon(qq.q0, qq.qd, z.n_sides, z.x.z, x1 - c);
        }
        if (x0 > 0.0 && x0 + c > 0.0) {
            var inner: ZSpans;
            if (z.kind == ZONE_KIND_CYL) {
                inner = zone_disc(qq.q0, qq.qd, x0 + c);
            } else {
                inner = zone_polygon(qq.q0, qq.qd, z.n_sides, z.x.z, x0 + c);
            }
            return zs_subtract(outer, inner);
        }
        return outer;
    }
    if (z.kind == ZONE_KIND_SECTOR) {
        if ((z.flags & ZONE_FLAG_FULL) != 0u) {
            return zs_full();
        }
        let qq = zone_q_of(z, p0, dv);
        let e0 = z.e.xy;
        let e1 = z.e.zw;
        if ((z.flags & ZONE_FLAG_WIDE) != 0u) {
            // Complement of the convex wedge from angle_to round to angle_from.
            let w = zs_intersect(
                zone_ge0(zone_cross2(e1, qq.q0) + c, zone_cross2(e1, qq.qd)),
                zone_ge0(zone_cross2(qq.q0, e0) + c, zone_cross2(qq.qd, e0)),
            );
            return zs_subtract(zs_full(), w);
        }
        return zs_intersect(
            zone_ge0(zone_cross2(e0, qq.q0) - c, zone_cross2(e0, qq.qd)),
            zone_ge0(zone_cross2(qq.q0, e1) - c, zone_cross2(qq.qd, e1)),
        );
    }
    return zs_empty();
}

// Signed depth into the zone at p(t) (positive inside, zero on the boundary).
fn zone_depth(z: GpuZoneShape, p0: vec3<f32>, dv: vec3<f32>, t: f32) -> f32 {
    let x0 = z.x.x;
    let x1 = z.x.y;
    if (z.kind == ZONE_KIND_HALF) {
        let p = vec3<f32>(p0.x + t * dv.x, p0.y + t * dv.y, p0.z + t * dv.z);
        return dot(z.d.xyz, p) - x0;
    }
    if (z.kind == ZONE_KIND_SLAB) {
        let p = vec3<f32>(p0.x + t * dv.x, p0.y + t * dv.y, p0.z + t * dv.z);
        let s = dot(z.d.xyz, p);
        return min(s - x0, x1 - s);
    }
    if (z.kind == ZONE_KIND_CYL || z.kind == ZONE_KIND_PRISM) {
        let qq = zone_q_of(z, p0, dv);
        let q = vec2<f32>(qq.q0.x + t * qq.qd.x, qq.q0.y + t * qq.qd.y);
        var g: f32;
        if (z.kind == ZONE_KIND_CYL) {
            g = sqrt(q.x * q.x + q.y * q.y);
        } else {
            g = zone_polygon_gauge(q, z.n_sides, z.x.z);
        }
        let outer = x1 - g;
        if (x0 > 0.0) {
            return min(outer, g - x0);
        }
        return outer;
    }
    if (z.kind == ZONE_KIND_SECTOR) {
        if ((z.flags & ZONE_FLAG_FULL) != 0u) {
            return ZONE_F32_MAX;
        }
        let qq = zone_q_of(z, p0, dv);
        let q = vec2<f32>(qq.q0.x + t * qq.qd.x, qq.q0.y + t * qq.qd.y);
        let e0 = z.e.xy;
        let e1 = z.e.zw;
        if ((z.flags & ZONE_FLAG_WIDE) != 0u) {
            return -min(zone_cross2(e1, q), zone_cross2(q, e0));
        }
        return min(zone_cross2(e0, q), zone_cross2(q, e1));
    }
    return -ZONE_F32_MAX;
}

fn zone_smoothstep_weight(sd: f32, inv_width: f32) -> f32 {
    // The clamp keeps the F32_MAX sentinels (full sector / unused slot) from overflowing
    // the product; it changes nothing for any depth below a kilometre.
    let sdc = clamp(sd, -1.0e6, 1.0e6);
    let x = clamp(sdc * inv_width + 0.5, 0.0, 1.0);
    return x * x * (3.0 - 2.0 * x);
}

// Insertion sort of the first nb entries, ascending (the fixed sort of `k32`).
fn zone_sort_bp(bp: ptr<function, array<f32, 38>>, nb: u32) {
    for (var i: u32 = 1u; i < nb; i = i + 1u) {
        var j: u32 = i;
        loop {
            if (j == 0u) {
                break;
            }
            let lo = (*bp)[j - 1u];
            let hi = (*bp)[j];
            if (!(lo > hi)) {
                break;
            }
            (*bp)[j - 1u] = hi;
            (*bp)[j] = lo;
            j = j - 1u;
        }
    }
}

// Sharp lengths as fractions of the segment, in the zone frame.
fn zone_fractions_sharp(p0: vec3<f32>, dv: vec3<f32>, count: u32) -> array<f32, 5> {
    var spans: array<ZSpans, 4>;
    var bp: array<f32, 38>;
    bp[1] = 1.0;
    var nb: u32 = 2u;
    for (var j: u32 = 0u; j < count; j = j + 1u) {
        spans[j] = zone_spans(material.zones.shapes[j], p0, dv, 0.0);
        for (var i: u32 = 0u; i < spans[j].n; i = i + 1u) {
            let sp = zs_get(spans[j], i);
            bp[nb] = sp.x;
            bp[nb + 1u] = sp.y;
            nb = nb + 2u;
        }
    }
    zone_sort_bp(&bp, nb);
    var out: array<f32, 5>;
    for (var i: u32 = 0u; i + 1u < nb; i = i + 1u) {
        let a = bp[i];
        let b = bp[i + 1u];
        if (b <= a) {
            continue;
        }
        let mid = 0.5 * (a + b);
        var idx: u32 = 0u;
        for (var j: u32 = 0u; j < count; j = j + 1u) {
            if (zs_contains(spans[j], mid)) {
                idx = j + 1u;
            }
        }
        out[idx] = out[idx] + (b - a);
    }
    return out;
}

// Soft lengths as fractions of the segment, in the zone frame.
fn zone_fractions_soft(p0: vec3<f32>, dv: vec3<f32>, count: u32) -> array<f32, 5> {
    let softness = material.zones.header.softness;
    let half_w = 0.5 * softness;
    let inv_width = 1.0 / softness;
    var bp: array<f32, 38>;
    bp[1] = 1.0;
    var nb: u32 = 2u;
    for (var j: u32 = 0u; j < count; j = j + 1u) {
        let z = material.zones.shapes[j];
        for (var level: u32 = 0u; level < 2u; level = level + 1u) {
            var c = -half_w;
            if (level == 1u) {
                c = half_w;
            }
            let s = zone_spans(z, p0, dv, c);
            for (var i: u32 = 0u; i < s.n; i = i + 1u) {
                let sp = zs_get(s, i);
                bp[nb] = sp.x;
                bp[nb + 1u] = sp.y;
                nb = nb + 2u;
            }
        }
        // Kinks of the signed depth.
        if (z.kind == ZONE_KIND_SLAB) {
            let nd = dot(z.d.xyz, dv);
            if (nd != 0.0) {
                let t = (0.5 * (z.x.x + z.x.y) - dot(z.d.xyz, p0)) / nd;
                if (t > 0.0 && t < 1.0) {
                    bp[nb] = t;
                    nb = nb + 1u;
                }
            }
        } else if (z.kind == ZONE_KIND_CYL) {
            let qq = zone_q_of(z, p0, dv);
            let a = qq.qd.x * qq.qd.x + qq.qd.y * qq.qd.y;
            if (a > 0.0) {
                let t = -(qq.q0.x * qq.qd.x + qq.q0.y * qq.qd.y) / a;
                if (t > 0.0 && t < 1.0) {
                    bp[nb] = t;
                    nb = nb + 1u;
                }
            }
        }
    }
    zone_sort_bp(&bp, nb);

    let subdiv = max(material.zones.header.soft_subdiv, 1u);
    var acc: array<f32, 5>;
    for (var i: u32 = 0u; i + 1u < nb; i = i + 1u) {
        let a = bp[i];
        let b = bp[i + 1u];
        if (b <= a) {
            continue;
        }
        let mid = 0.5 * (a + b);
        var banded = false;
        for (var j: u32 = 0u; j < count; j = j + 1u) {
            if (abs(zone_depth(material.zones.shapes[j], p0, dv, mid)) < half_w) {
                banded = true;
            }
        }
        var parts: u32 = 1u;
        if (banded) {
            parts = subdiv;
        }
        let h = (b - a) / f32(parts);
        for (var part: u32 = 0u; part < parts; part = part + 1u) {
            let centre = a + h * f32(part) + 0.5 * h;
            for (var g: u32 = 0u; g < 8u; g = g + 1u) {
                let t = centre + 0.5 * h * ZONE_GL_X[g];
                let wgt = 0.5 * h * ZONE_GL_W[g];
                var w: array<f32, 4>;
                for (var j: u32 = 0u; j < count; j = j + 1u) {
                    w[j] = zone_smoothstep_weight(
                        zone_depth(material.zones.shapes[j], p0, dv, t),
                        inv_width,
                    );
                }
                var rest: f32 = 1.0;
                for (var jj: u32 = count; jj > 0u; jj = jj - 1u) {
                    let j = jj - 1u;
                    acc[j + 1u] = acc[j + 1u] + wgt * w[j] * rest;
                    rest = rest * max(1.0 - w[j], 0.0);
                }
                acc[0] = acc[0] + wgt * rest;
            }
        }
    }
    return acc;
}

// Per-zone lengths of the segment p0 -> p0 + dv, both in the zone frame.
fn zone_lengths_local(p0: vec3<f32>, dv: vec3<f32>) -> array<f32, 5> {
    var out: array<f32, 5>;
    let len = sqrt(dot(dv, dv));
    if (!(len > 0.0)) {
        return out;
    }
    let count = material.zones.header.zone_count;
    var frac: array<f32, 5>;
    if (material.zones.header.softness > 0.0) {
        frac = zone_fractions_soft(p0, dv, count);
    } else {
        frac = zone_fractions_sharp(p0, dv, count);
    }
    for (var i: u32 = 0u; i < 5u; i = i + 1u) {
        out[i] = frac[i] * len;
    }
    return out;
}

fn zone_to_local(p: vec3<f32>) -> vec3<f32> {
    let h = material.zones.header;
    let r = p - h.origin.xyz;
    return vec3<f32>(dot(h.row0.xyz, r), dot(h.row1.xyz, r), dot(h.row2.xyz, r));
}

// `ZoneKernelF32::lengths`: per-zone lengths of the segment seg_a -> seg_b given in the
// frame the zone frame is relative to (stone millimetres).
fn zone_lengths(seg_a: vec3<f32>, seg_b: vec3<f32>) -> array<f32, 5> {
    let a = zone_to_local(seg_a);
    let b = zone_to_local(seg_b);
    return zone_lengths_local(a, vec3<f32>(b.x - a.x, b.y - a.y, b.z - a.z));
}
