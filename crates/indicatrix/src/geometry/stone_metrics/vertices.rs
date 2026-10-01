//! Enumeration and deduplication of a plane arrangement's vertices: the
//! shared primitive [`measure_solid`](super::measure_solid) and
//! [`build_solid_mesh`](super::build_solid_mesh) both build on.

use glam::DVec3;

use super::{BLANK_HALF_EXTENT, EPS_FEAS, MIN_TRIPLE_DET, VERTEX_DEDUP};
use crate::geometry::cuts::normals_coincide;

/// One deduplicated vertex of the solid.
pub(super) struct SolidVertex {
    pub(super) v: DVec3,
}

/// Accepted vertices, in first-seen order, plus an index over them sorted by
/// `x` so a new candidate's duplicate check only has to scan the vertices
/// that could possibly be within [`VERTEX_DEDUP`] of it on every axis instead
/// of the full accepted set.
///
/// `verts`' order is exactly the push order of [`insert_if_new`](Self::insert_if_new)
/// calls (the divergence-theorem sum in [`measure_solid`](super::measure_solid)
/// depends on it); `by_x` is a lookup structure only, never observed by
/// callers.
#[derive(Default)]
struct VertexAccumulator {
    verts: Vec<SolidVertex>,
    /// Indices into `verts`, kept sorted ascending by `verts[i].v.x`.
    by_x: Vec<usize>,
}

impl VertexAccumulator {
    /// Inserts `v` unless some already-accepted vertex is within
    /// [`VERTEX_DEDUP`] of it on every axis -- identical to a linear scan
    /// testing `(s.v - v).abs().max_element() < VERTEX_DEDUP` against every
    /// prior vertex, just restricted up front to the `x`-sorted window that
    /// could possibly match (any vertex outside `[v.x - VERTEX_DEDUP, v.x +
    /// VERTEX_DEDUP]` fails the `x`-axis check alone, so narrowing to that
    /// window changes no accept/reject decision).
    fn insert_if_new(&mut self, v: DVec3) {
        let verts = &self.verts;
        let lo = self
            .by_x
            .partition_point(|&i| verts[i].v.x < v.x - VERTEX_DEDUP);
        let hi = self
            .by_x
            .partition_point(|&i| verts[i].v.x <= v.x + VERTEX_DEDUP);
        for &i in &self.by_x[lo..hi] {
            if (self.verts[i].v - v).abs().max_element() < VERTEX_DEDUP {
                return;
            }
        }
        let idx = self.verts.len();
        self.verts.push(SolidVertex { v });
        let pos = self.by_x.partition_point(|&i| self.verts[i].v.x < v.x);
        self.by_x.insert(pos, idx);
    }
}

/// Drops duplicate planes so a tier that lists the same index twice can't
/// double-count its face's area. The first occurrence wins.
///
/// Two planes are duplicates when their normals coincide under
/// [`normals_coincide`] (the rule the `.asc` plane builder uses, which holds for
/// `f32`-normalised inputs widened to `f64`) and their offsets differ by less
/// than `1e-9`.
pub(super) fn dedup_planes(planes: &[(DVec3, f64)]) -> Vec<(DVec3, f64)> {
    let mut out: Vec<(DVec3, f64)> = Vec::with_capacity(planes.len());
    for &(n, m) in planes {
        let dup = out
            .iter()
            .any(|&(n2, m2)| normals_coincide(n, n2) && (m - m2).abs() < 1e-9);
        if !dup {
            out.push((n, m));
        }
    }
    out
}

/// Drains one full (or final partial) [`crate::simd::TripleBatch`] solve into
/// `visit`, in ascending lane order. `lanes[i]` holds the plane-index triple
/// pushed into lane `i`. Returns `None` the moment a well-conditioned
/// (`|det| >= MIN_TRIPLE_DET`) vertex escapes to the blank box, propagated by
/// the caller via `?` -- matching the original loop's immediate `return None`.
/// Shared by [`for_each_feasible_triple`]'s batching loop.
///
/// `det_floor` is the smallest `|det|` a lane may have and still be solved and
/// visited. [`feasible_vertices`] passes [`MIN_TRIPLE_DET`] itself, which makes
/// the escape branch's own `det_abs >= MIN_TRIPLE_DET` test always true there,
/// so its behaviour is exactly the pre-walker one. A caller passing a lower
/// floor (the B-rep, which wants to *report* ill-conditioned vertices rather
/// than silently lose them) also sees feasible lanes with
/// `det_floor <= |det| < MIN_TRIPLE_DET`, but such a lane never triggers the
/// blank escape: it is dropped instead, so the escape decision stays exactly
/// the one [`escaping_plane_indices`] replays.
fn flush_solid_batch(
    batch: &crate::simd::TripleBatch,
    lanes: &[[usize; 3]; crate::simd::TRIPLE_LANES],
    soa: &crate::simd::PlanesSoA64,
    det_floor: f64,
    visit: &mut impl FnMut([usize; 3], f64, DVec3),
) -> Option<()> {
    let sol = crate::simd::solve_triple_batch(batch);
    for lane in 0..batch.len {
        // `is_nan() || .. < det_floor` rather than a plain `.. < det_floor`:
        // a NaN determinant (a non-finite mast or index reaching the solve) fails
        // every ordered comparison, so the plain `<` form falls through and hands a
        // NaN-tainted vertex to the accumulator (see the module docs on non-finite
        // inputs). The explicit `is_nan()` check catches it too.
        let det_abs = sol.det[lane].abs();
        if det_abs.is_nan() || det_abs < det_floor {
            continue;
        }
        let v = DVec3::new(sol.vx[lane], sol.vy[lane], sol.vz[lane]);
        if !v.is_finite() {
            continue;
        }
        if v.abs().max_element() > BLANK_HALF_EXTENT + 1.0 {
            continue;
        }
        if crate::simd::any_violation(soa, v, EPS_FEAS) {
            continue;
        }
        // A feasible vertex at the blank box means the real planes never
        // closed the solid up -- there is no finite stone to measure.
        if v.abs().max_element() > BLANK_HALF_EXTENT - 1.0 {
            if det_abs >= MIN_TRIPLE_DET {
                return None;
            }
            continue;
        }
        visit(lanes[lane], sol.det[lane], v);
    }
    Some(())
}

/// Pairs whose normals are closer to parallel than this (`|n_a x n_b|`) are
/// never pruned by [`pair_misses_solid`]: their intersection line is too
/// poorly determined to clip reliably.
const PRUNE_MIN_SIN: f64 = 1e-3;

/// Bound on `|residual| * |det|` for one `solve_triple_batch` lane: the amount
/// by which a triple's solved point can miss its own planes, times the
/// triple's determinant. The cofactor inverse's rounding error is about
/// `4e-13` for unit normals and offsets up to the blank's 65 (derived in
/// [`pair_misses_solid`]); this keeps a 25x safety factor.
const SOLVE_RESIDUAL_NUMERATOR: f64 = 1e-11;

/// Conservative pair prune: `true` only if no triple `(a, b, c)` with
/// `|det| >= det_floor` can pass [`flush_solid_batch`]'s feasibility check.
///
/// Every such triple's point lies on the line where planes `a` and `b` meet,
/// up to its solve residual `r <= SOLVE_RESIDUAL_NUMERATOR / det_floor` in
/// each of the two planes, i.e. within `2 r / |n_a x n_b|` of the line. So if
/// the line, clipped against every other plane widened by [`EPS_FEAS`] plus
/// that distance, is empty, no point near it is feasible and the pair's whole
/// inner loop can be skipped without changing a single visited triple.
/// (Residual derivation: glam's `DMat3::inverse` forms cofactors as cross
/// products of the columns, each entry with rounding error below about
/// `2e-15` for components of unit normals, so `M * inverse(M) * b - b` is at
/// most about `3 * 2e-15 * 65 / |det| ~ 4e-13 / |det|` per row, with
/// `|b|_inf <= 65`.)
fn pair_misses_solid(all: &[(DVec3, f64)], a: usize, b: usize, det_floor: f64) -> bool {
    let (na, ma) = all[a];
    let (nb, mb) = all[b];
    let dir = na.cross(nb);
    let sin = dir.length();
    if sin < PRUNE_MIN_SIN {
        return false;
    }
    // The point on both planes closest to the origin, and the unit direction.
    let origin = (nb.cross(dir) * ma + dir.cross(na) * mb) / (sin * sin);
    let unit = dir / sin;
    let margin = EPS_FEAS + 2.0 * SOLVE_RESIDUAL_NUMERATOR / det_floor / sin;
    let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
    for (c, &(nc, mc)) in all.iter().enumerate() {
        if c == a || c == b {
            continue;
        }
        let slope = nc.dot(unit);
        let room = mc + margin - nc.dot(origin);
        if slope > 0.0 {
            hi = hi.min(room / slope);
        } else if slope < 0.0 {
            lo = lo.max(room / slope);
        } else if room < 0.0 {
            return true;
        }
        if lo > hi {
            return true;
        }
    }
    false
}

/// Calls `visit(triple, det, vertex)` for every feasible plane triple of the
/// arrangement, in lexicographic order.
///
/// Covers every triple `a < b < c` of `planes` plus the six blank-box planes
/// (indices `planes.len()..planes.len() + 6`) whose `|det| >= det_floor` and
/// whose intersection is finite and satisfies every half-space within
/// [`EPS_FEAS`]. Returns `None` when a well-conditioned feasible vertex reaches
/// the bounding blank (the real planes don't bound a finite solid); `visit`
/// may already have been called for earlier triples by then.
///
/// The single arrangement walk behind both [`feasible_vertices`] (which
/// passes `det_floor = MIN_TRIPLE_DET`, `prune_pairs = true`, and
/// deduplicates positions) and `geometry::brep` (which passes a lower floor,
/// prunes, and keeps the triples, so it can recover each vertex's incident
/// planes). Deterministic: plain nested loops, lanes drained in ascending
/// order, no hashing.
///
/// `prune_pairs` skips every pair `(a, b)` that [`pair_misses_solid`] proves
/// cannot take part in a visited triple -- the same triples are visited in the
/// same order, only faster (a dense 600-plane sphere walks about 30x faster;
/// the 205-plane crackotto fixture about 4x). Both callers enable it; `false`
/// is the plain exhaustive walk the pruned one is checked against.
///
/// Batched through `crate::simd`, matching
/// `meet_solver::enumerate_candidate_vertices`: one `PlanesSoA64` built up
/// front (owner is irrelevant to this owner-free scan, so every plane is
/// pushed with owner 0), triples solved via `solve_triple_batch` via
/// [`flush_solid_batch`], and the `any()` feasibility scan replaced by
/// `any_violation` -- bit-identical per lane to the `glam` `DMat3` sequence
/// and scalar scan they replace (see `src/simd.rs`'s determinism contract).
pub(in crate::geometry) fn for_each_feasible_triple(
    planes: &[(DVec3, f64)],
    det_floor: f64,
    prune_pairs: bool,
    mut visit: impl FnMut([usize; 3], f64, DVec3),
) -> Option<()> {
    let mut all: Vec<(DVec3, f64)> = planes.to_vec();
    for n in [
        DVec3::X,
        DVec3::NEG_X,
        DVec3::Y,
        DVec3::NEG_Y,
        DVec3::Z,
        DVec3::NEG_Z,
    ] {
        all.push((n, BLANK_HALF_EXTENT));
    }

    let mut soa = crate::simd::PlanesSoA64::with_capacity(all.len());
    for &(n, m) in &all {
        soa.push(n, m, 0);
    }

    let p = all.len();
    let mut batch = crate::simd::TripleBatch::default();
    let mut lanes = [[0usize; 3]; crate::simd::TRIPLE_LANES];
    for a in 0..p {
        for b in (a + 1)..p {
            if prune_pairs && pair_misses_solid(&all, a, b, det_floor) {
                continue;
            }
            for c in (b + 1)..p {
                let (pa, pb, pc) = (all[a], all[b], all[c]);
                lanes[batch.len] = [a, b, c];
                if batch.push((pa.0, pa.1), (pb.0, pb.1), (pc.0, pc.1)) {
                    flush_solid_batch(&batch, &lanes, &soa, det_floor, &mut visit)?;
                    batch = crate::simd::TripleBatch::default();
                }
            }
        }
    }
    if batch.len > 0 {
        flush_solid_batch(&batch, &lanes, &soa, det_floor, &mut visit)?;
    }
    Some(())
}

/// Enumerates the solid's distinct vertices: every well-conditioned plane triple
/// whose intersection satisfies all half-spaces (within [`EPS_FEAS`]), then
/// deduplicated by position. Returns `None` when any vertex reaches the bounding
/// blank (the real planes don't bound a finite solid).
///
/// A thin wrapper over [`for_each_feasible_triple`] with
/// `det_floor = MIN_TRIPLE_DET` and pair pruning. Lanes are drained in ascending order and
/// triples are generated by the same nested loops in the same order (the prune
/// only skips pairs that provably yield no visited triple), so vertex order and
/// every decision here (determinant check, bounds check, feasibility,
/// blank-escape) match the unbatched scalar version exactly.
pub(super) fn feasible_vertices(planes: &[(DVec3, f64)]) -> Option<Vec<SolidVertex>> {
    let mut acc = VertexAccumulator::default();
    for_each_feasible_triple(planes, MIN_TRIPLE_DET, true, |_, _, v| {
        acc.insert_if_new(v);
    })?;
    Some(acc.verts)
}

/// Maps each plane of `deduped` (in order) back to its index in `original`.
///
/// Valid because [`dedup_planes`] is a pure filter: it copies each retained
/// plane through unchanged (no numeric transform) and never reorders
/// anything, only drops exact repeats -- so `deduped` is, element for
/// element, an order-preserving subsequence of `original`. A single lockstep
/// scan (advancing `original`'s cursor past every plane it consumes, whether
/// kept or skipped) recovers the mapping without re-implementing
/// `dedup_planes`'s own equality test, and without ever revisiting an
/// `original` index already assigned to an earlier `deduped` entry (so two
/// bit-identical original planes are told apart by position, not silently
/// both mapped to the first).
pub(super) fn dedup_origin_indices(
    original: &[(DVec3, f64)],
    deduped: &[(DVec3, f64)],
) -> Vec<usize> {
    let same = |a: (DVec3, f64), b: (DVec3, f64)| {
        a.0.x.to_bits() == b.0.x.to_bits()
            && a.0.y.to_bits() == b.0.y.to_bits()
            && a.0.z.to_bits() == b.0.z.to_bits()
            && a.1.to_bits() == b.1.to_bits()
    };
    let mut out = Vec::with_capacity(deduped.len());
    let mut cursor = 0usize;
    for &d in deduped {
        while cursor < original.len() && !same(original[cursor], d) {
            cursor += 1;
        }
        // `cursor == original.len()` here would mean `deduped` contains a
        // plane `dedup_planes` could not have produced from `original` --
        // an invariant violation, not a real runtime case. Clamping instead
        // of panicking keeps this diagnostic helper infallible even if that
        // invariant is ever broken by a future edit to `dedup_planes`.
        out.push(cursor.min(original.len().saturating_sub(1)));
        cursor += 1;
    }
    out
}

/// Sorted indices of the real planes that take part in a vertex escaping to
/// the blank box.
///
/// Diagnostic re-scan of `planes` (already deduped), used only when
/// [`feasible_vertices`] reports the arrangement unbounded: replays the same
/// augmented-triple enumeration and the same escape test
/// (`flush_solid_batch`'s `v.abs().max_element() > BLANK_HALF_EXTENT - 1.0`),
/// but instead of stopping at the first escaping vertex, visits every triple
/// and collects which of the REAL (non-blank) plane indices participate in
/// at least one.
///
/// Not SIMD-batched, unlike [`feasible_vertices`]: this only runs on an
/// editor's invalid intermediate states, never on the success path measured
/// by any perf budget, so a plain `glam` solve per triple (the same
/// arithmetic `feasible_vertices`'s batches compute, just one triple at a
/// time) is the right tradeoff here -- obviously correct beats fast.
pub(in crate::geometry) fn escaping_plane_indices(planes: &[(DVec3, f64)]) -> Vec<usize> {
    let real_count = planes.len();
    let mut all: Vec<(DVec3, f64)> = planes.to_vec();
    for n in [
        DVec3::X,
        DVec3::NEG_X,
        DVec3::Y,
        DVec3::NEG_Y,
        DVec3::Z,
        DVec3::NEG_Z,
    ] {
        all.push((n, BLANK_HALF_EXTENT));
    }

    let p = all.len();
    let mut escaping: Vec<usize> = Vec::new();
    for a in 0..p {
        for b in (a + 1)..p {
            for c in (b + 1)..p {
                let (na, ma) = all[a];
                let (nb, mb) = all[b];
                let (nc, mc) = all[c];
                let mat = glam::DMat3::from_cols(na, nb, nc).transpose();
                let det = mat.determinant();
                // See `flush_solid_batch`'s matching guard: the explicit `is_nan()`
                // check catches a NaN determinant, which `.abs() < MIN_TRIPLE_DET`
                // alone does not.
                let det_abs = det.abs();
                if det_abs.is_nan() || det_abs < MIN_TRIPLE_DET {
                    continue;
                }
                let v = mat.inverse() * DVec3::new(ma, mb, mc);
                if !v.is_finite() {
                    continue;
                }
                if v.abs().max_element() > BLANK_HALF_EXTENT + 1.0 {
                    continue;
                }
                if all.iter().any(|&(n, m)| n.dot(v) - m > EPS_FEAS) {
                    continue;
                }
                if v.abs().max_element() > BLANK_HALF_EXTENT - 1.0 {
                    for idx in [a, b, c] {
                        if idx < real_count && !escaping.contains(&idx) {
                            escaping.push(idx);
                        }
                    }
                }
            }
        }
    }
    escaping.sort_unstable();
    escaping
}
