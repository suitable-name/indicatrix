//! Input cleaning and Pareto pruning of the candidate designs.
//!
//! Normalise each design to `(1, l, h, f)` (see [`super::piece`]). Design `e`
//! dominates `d` iff `l_e <= l_d`, `h_e <= h_d` and `f_e >= f_d`, with at least
//! one strict (or all equal and `e` has the lower `entry_id`). For every piece
//! `p` and assignment the finished width is
//! `s = min(p_W, p_L / l, p_H / h)`, which is monotone non-increasing in `l`
//! and `h`, and the volume `f * s^3` is monotone in `f` and `s`; the `w_min`
//! test is `s >= w_min`. So a dominated design never strictly beats its
//! dominator on any piece, and the mixed DP may run on the front alone. The
//! argument holds bit for bit under IEEE arithmetic, because division,
//! `min` and multiplication of non-negative numbers are all monotone.
//!
//! # With a mesh rough
//!
//! A piece clipped by a non-convex mesh is solved by a heuristic, `solve_in_mesh` of the
//! shaped `PartialSolver` (the LP of the convex region plus up to six rounds of cutting
//! planes), so the argument above needs a second look. It holds for the TRUE problem: if a dominated
//! design `d` has a stone of width `s` at a centre `t` that the mesh accepts (no triangle
//! meets its caliper box, the centre is in the material), the dominator `e` has, for the
//! same assignment, a caliper box that is no larger on any axis (`l_e <= l_d`, `h_e <= h_d`,
//! the width is the same `s`), so its box at `t` lies inside `d`'s box: no triangle meets it
//! either, the centre is the same point, and its volume `f_e s^3 >= f_d s^3`. The feasible
//! set of the dominator contains the dominated design's, so pruning never discards the
//! optimum.
//!
//! What it can cost is the heuristic's miss: near a notch the solver may fail for `e`
//! (it starts from the convex optimum of `e`, which can press into the notch beyond what
//! six rounds clear) where it would have succeeded for `d`, and `d` is no longer in the pool
//! to try. The loss is a smaller or missing stone in one piece, never a wrong placement
//! (every accepted stone is checked against the mesh), and it is the "near a notch it can be
//! a little more cautious" the manual states. Keeping the dominated designs of a mesh rough
//! instead would multiply the pool (the front is tens of designs, the catalogue thousands),
//! and every table, DP and shortlist cost with it, to recover a few stones the refinement
//! recovers in part anyway.

use std::cmp::Ordering;

use super::{piece::Norm, types::CandidateDesign};

/// Drop invalid candidates (any non-finite or non-positive field), swap a
/// reversed width/length pair, sort by `entry_id` (then by the measured
/// fields, so equal ids still order deterministically) and keep one of any
/// run of candidates with the same `entry_id` and the same shape (all four
/// measured fields equal once width/length are ordered).
///
/// Two candidates that share an `entry_id` but differ in shape both stay: the
/// caller owns the ids, and the planner cannot tell which measurement is right.
pub fn sanitize(designs: &[CandidateDesign]) -> Vec<CandidateDesign> {
    let mut out: Vec<CandidateDesign> = designs
        .iter()
        .filter(|d| {
            [d.width, d.length, d.height, d.volume]
                .iter()
                .all(|v| v.is_finite() && *v > 0.0)
        })
        .map(|d| {
            let (width, length) = if d.width <= d.length {
                (d.width, d.length)
            } else {
                (d.length, d.width)
            };
            CandidateDesign {
                width,
                length,
                ..*d
            }
        })
        .collect();
    out.sort_by(|a, b| {
        a.entry_id
            .cmp(&b.entry_id)
            .then(a.width.total_cmp(&b.width))
            .then(a.length.total_cmp(&b.length))
            .then(a.height.total_cmp(&b.height))
            .then(a.volume.total_cmp(&b.volume))
    });
    out.dedup();
    out
}

/// Whether the design `(ne, id_e)` dominates `(nd, id_d)`.
fn dominates(ne: &Norm, id_e: i64, nd: &Norm, id_d: i64) -> bool {
    let no_worse = ne.l <= nd.l && ne.h <= nd.h && ne.f >= nd.f;
    if !no_worse {
        return false;
    }
    let strict = ne.l < nd.l || ne.h < nd.h || ne.f > nd.f;
    strict || id_e < id_d
}

/// The sweep order: `l` ascending, `h` ascending, `f` descending, then
/// `entry_id` and position ascending. Every dominator of a design sorts before it.
fn sweep_order(clean: &[CandidateDesign], norms: &[Norm], a: usize, b: usize) -> Ordering {
    norms[a]
        .l
        .total_cmp(&norms[b].l)
        .then(norms[a].h.total_cmp(&norms[b].h))
        .then(norms[b].f.total_cmp(&norms[a].f))
        .then(clean[a].entry_id.cmp(&clean[b].entry_id))
        .then(a.cmp(&b))
}

/// The non-dominated designs, sorted by `entry_id`.
///
/// Invalid candidates are dropped first, see [`CandidateDesign`]. Exact
/// duplicates (equal `l`, `h`, `f`) keep the lower `entry_id`.
///
/// One sort and one sweep: the designs are visited in an order in which every
/// dominator precedes the designs it dominates, and a design is kept only when
/// no design kept so far dominates it. Dominance is transitive, so a dominator
/// that was itself dropped is always covered by a kept one, and the sweep is
/// exact. The cost is `O(n log n + n * front)`.
#[must_use]
pub fn pareto_front(designs: &[CandidateDesign]) -> Vec<CandidateDesign> {
    let clean = sanitize(designs);
    let norms: Vec<Norm> = clean.iter().map(Norm::of).collect();
    let mut order: Vec<usize> = (0..clean.len()).collect();
    order.sort_by(|&a, &b| sweep_order(&clean, &norms, a, b));
    let mut kept: Vec<usize> = Vec::new();
    for i in order {
        let covered = kept
            .iter()
            .any(|&k| dominates(&norms[k], clean[k].entry_id, &norms[i], clean[i].entry_id));
        if !covered {
            kept.push(i);
        }
    }
    kept.sort_unstable();
    kept.into_iter().map(|i| clean[i]).collect()
}
