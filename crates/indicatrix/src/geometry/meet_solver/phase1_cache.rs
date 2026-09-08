//! [`solve`](super::solve)'s incremental candidate-vertex cache for phase 1's
//! constructive pass: as tiers settle one at a time, this keeps the
//! candidate-vertex set updated incrementally instead of re-running
//! `enumerate_candidate_vertices` from scratch on every settle attempt. Kept
//! as its own unit because the soundness and ordering argument below is
//! self-contained and only phase 1 (never phase 3) can use it.

// ====================================================================
// Phase 1 incremental candidate-vertex cache
// ====================================================================
//
// Phase 1's constructive pass used to rebuild the settled-tier plane list
// and call `enumerate_candidate_vertices` (cubic in plane count) at every
// settle attempt of every tier. This is sound to cache because a phase-1
// tier's mast never changes once settled, so the candidate-vertex set for
// "blanks + every settled tier's planes" only ever *grows* -- a candidate
// already classified feasible-or-single-violator stays that way and only
// needs checking against what's new. Phase 3 cannot reuse this: its sweeps
// change every tier's mast every sweep, so it has no stable prefix to cache
// against.
//
// # Ordering
//
// A full enumeration scans plane triples in ascending lexicographic order
// of their `(tier, instance)` keys (blanks never appear in a triple). This
// cache reproduces that order: candidates are tagged with their key triple
// and kept sorted by it; a newly settled tier can have a lower tier index
// than tiers already cached (a named tier can settle out of file order), so
// each new triple's planes are sorted by key before solving
// ([`push_triple`]) and merged into the sorted list by key
// ([`merge_sorted`]).
//
// # Feasibility scan order is safe to append
//
// New candidates are classified against [`Phase1Cache::soa`], which is
// appended to in settle order rather than kept in tier order. That can only
// change *which* of several violating owners a scan meets first, never the
// Dead-vs-`Ok(owner)` outcome (`Dead` and single-owner `Ok` are both facts
// about the *set* of violated planes, not the scan order) -- so an
// append-ordered scan gives bit-identical results to a tier-ordered one.
//
// # Why dead stays dead
//
// A tier only ever adds planes (masts are fixed once settled), and adding
// planes only adds constraints, never removes them -- so a candidate
// already infeasible stays infeasible forever. Dead candidates are never
// stored, and a survivor only needs re-checking against what's new
// ([`revalidate`]), not the whole arrangement.

use super::{BLANK_HALF_EXTENT, EPS_FEAS, MIN_TRIPLE_DET};
use glam::DVec3;

/// One real plane with its canonical `(tier, instance)` identity, used only
/// by [`Phase1Cache`].
struct PlaneRec {
    n: DVec3,
    m: f64,
    owner: usize,
    instance: usize,
}

/// One phase-1 cached candidate vertex, tagged with the `(tier, instance)`
/// key triple of the three planes that formed it (see the module comment on
/// why this key, not just plane owners, keeps the cache in enumeration order).
pub(super) struct CachedCandidate {
    pub(super) v: DVec3,
    pub(super) violated: Option<usize>,
    key: [(usize, usize); 3],
}

/// Incremental candidate-vertex store for phase 1's constructive pass only.
/// See the module comment above for the soundness and ordering argument.
pub(super) struct Phase1Cache {
    /// Every currently-settled tier's real planes, plus the six bounding
    /// blanks, in the order each was added (settle order, *not* tier
    /// order -- safe for feasibility classification only, see above).
    soa: crate::simd::PlanesSoA64,
    /// Every currently-settled tier's real planes (settle order), mirroring
    /// `soa`'s real-plane entries -- kept separately (rather than read back
    /// out of `soa`) so triple generation has each plane's `(tier,
    /// instance)` key and un-scaled `DVec3` normal on hand.
    real_planes: Vec<PlaneRec>,
    /// Every surviving (feasible or single-tier-violator) candidate vertex
    /// of the current arrangement, in canonical `(tier, instance)`-triple
    /// order -- exactly the order a full `enumerate_candidate_vertices`
    /// call would produce.
    pub(super) candidates: Vec<CachedCandidate>,
}

impl Phase1Cache {
    pub(super) fn new() -> Self {
        let mut soa = crate::simd::PlanesSoA64::with_capacity(6);
        for n in [
            DVec3::X,
            DVec3::NEG_X,
            DVec3::Y,
            DVec3::NEG_Y,
            DVec3::Z,
            DVec3::NEG_Z,
        ] {
            soa.push(n, BLANK_HALF_EXTENT, crate::simd::BLANK_OWNER);
        }
        Self {
            soa,
            real_planes: Vec::new(),
            candidates: Vec::new(),
        }
    }

    /// Folds one newly settled tier's planes into the cache: revalidates
    /// existing survivors against only the new planes, enumerates only the
    /// new triples (those with at least one plane from `owner`), and merges
    /// the result into `candidates` at its canonical position -- equivalent
    /// to a full re-enumeration, minus the work already-settled tiers did.
    pub(super) fn add_tier(&mut self, owner: usize, normals_j: &[DVec3], mast: f64) {
        let new_planes: Vec<PlaneRec> = normals_j
            .iter()
            .enumerate()
            .map(|(instance, &n)| PlaneRec {
                n,
                m: mast,
                owner,
                instance,
            })
            .collect();

        revalidate(&mut self.candidates, &new_planes, owner);

        for p in &new_planes {
            self.soa.push(p.n, p.m, owner as u32);
        }

        // The three categories below (all-new, two-new-one-old, one-new-two-old)
        // partition every triple touching `new_planes` without overlap or
        // omission, adding exactly what a full re-enumeration would add.
        let mut new_candidates: Vec<CachedCandidate> = Vec::new();
        let mut batch = crate::simd::TripleBatch::default();
        let mut key_meta = [[(0usize, 0usize); 3]; crate::simd::TRIPLE_LANES];
        let k = new_planes.len();

        for ja in 0..k {
            for jb in (ja + 1)..k {
                for jc in (jb + 1)..k {
                    push_triple(
                        &mut batch,
                        &mut key_meta,
                        &new_planes[ja],
                        &new_planes[jb],
                        &new_planes[jc],
                        &self.soa,
                        &mut new_candidates,
                    );
                }
            }
        }
        for ja in 0..k {
            for jb in (ja + 1)..k {
                for o in &self.real_planes {
                    push_triple(
                        &mut batch,
                        &mut key_meta,
                        &new_planes[ja],
                        &new_planes[jb],
                        o,
                        &self.soa,
                        &mut new_candidates,
                    );
                }
            }
        }
        let old_len = self.real_planes.len();
        for new_plane in &new_planes {
            for ia in 0..old_len {
                for ib in (ia + 1)..old_len {
                    push_triple(
                        &mut batch,
                        &mut key_meta,
                        new_plane,
                        &self.real_planes[ia],
                        &self.real_planes[ib],
                        &self.soa,
                        &mut new_candidates,
                    );
                }
            }
        }
        if batch.len > 0 {
            flush_keyed(&batch, &self.soa, &key_meta, &mut new_candidates);
        }

        new_candidates.sort_by_key(|cc| cc.key);
        self.candidates = merge_sorted(std::mem::take(&mut self.candidates), new_candidates);
        self.real_planes.extend(new_planes);
    }
}

/// Drops any cached candidate that `new_planes` (all owned by `owner`, a
/// tier that just settled) newly violates and that was already recorded as
/// violating some other tier -- and records `owner` as the sole violator of
/// any candidate that had none yet. Never produces `Dead` from this
/// single-owner scan (blank violations and multi-owner violations are the
/// only ways `classify_feasibility` returns `Dead`, and neither can occur
/// scanning one tier's planes in isolation), so the classification only
/// ever needs the two-way `None`/`Some(owner)` split.
fn revalidate(existing: &mut Vec<CachedCandidate>, new_planes: &[PlaneRec], owner: usize) {
    if new_planes.is_empty() {
        return;
    }
    let mut mini = crate::simd::PlanesSoA64::with_capacity(new_planes.len());
    for p in new_planes {
        mini.push(p.n, p.m, owner as u32);
    }
    existing.retain_mut(
        |cc| match crate::simd::classify_feasibility(&mini, cc.v, EPS_FEAS) {
            crate::simd::Feasibility::Dead => {
                debug_assert!(
                    false,
                    "single-owner mini-scan against one tier's planes cannot classify Dead"
                );
                false
            }
            crate::simd::Feasibility::Ok(None) => true,
            crate::simd::Feasibility::Ok(Some(_)) => match cc.violated {
                None => {
                    cc.violated = Some(owner);
                    true
                }
                Some(prior) => prior == owner,
            },
        },
    );
}

/// Sorts three planes by their canonical `(tier, instance)` key (matching
/// the `a < b < c` order a full plane-index scan would assign them) and
/// pushes the triple into `batch`, flushing a full batch through
/// [`flush_keyed`] exactly as a full candidate-vertex enumeration's loop
/// does.
fn push_triple(
    batch: &mut crate::simd::TripleBatch,
    key_meta: &mut [[(usize, usize); 3]; crate::simd::TRIPLE_LANES],
    p0: &PlaneRec,
    p1: &PlaneRec,
    p2: &PlaneRec,
    soa: &crate::simd::PlanesSoA64,
    out: &mut Vec<CachedCandidate>,
) {
    let mut trio = [p0, p1, p2];
    trio.sort_by_key(|p| (p.owner, p.instance));
    let [a, b, c] = trio;
    key_meta[batch.len] = [
        (a.owner, a.instance),
        (b.owner, b.instance),
        (c.owner, c.instance),
    ];
    if batch.push((a.n, a.m), (b.n, b.m), (c.n, c.m)) {
        flush_keyed(batch, soa, key_meta, out);
        *batch = crate::simd::TripleBatch::default();
    }
}

/// Phase 3's candidate-flush twin for the incremental cache: same
/// determinant threshold, bounding-box guard and feasibility
/// classification (against the arrangement's *full* current `soa`, so a new
/// triple is checked against every settled tier, not just the ones it
/// touches), but tags each survivor with its canonical key instead of its
/// plane owners.
fn flush_keyed(
    batch: &crate::simd::TripleBatch,
    soa: &crate::simd::PlanesSoA64,
    key_meta: &[[(usize, usize); 3]; crate::simd::TRIPLE_LANES],
    out: &mut Vec<CachedCandidate>,
) {
    let sol = crate::simd::solve_triple_batch(batch);
    for (lane, key) in key_meta.iter().enumerate().take(batch.len) {
        if sol.det[lane].abs() < MIN_TRIPLE_DET {
            continue;
        }
        let v = DVec3::new(sol.vx[lane], sol.vy[lane], sol.vz[lane]);
        if v.x.abs() > BLANK_HALF_EXTENT + 1.0
            || v.y.abs() > BLANK_HALF_EXTENT + 1.0
            || v.z.abs() > BLANK_HALF_EXTENT + 1.0
        {
            continue;
        }
        match crate::simd::classify_feasibility(soa, v, EPS_FEAS) {
            crate::simd::Feasibility::Dead => {}
            crate::simd::Feasibility::Ok(violated) => {
                out.push(CachedCandidate {
                    v,
                    violated: violated.map(|o| o as usize),
                    key: *key,
                });
            }
        }
    }
}

/// Merges two candidate lists already sorted by `key` into one sorted list,
/// reproducing the order a full enumeration over their combined arrangement
/// would produce (see the module comment on ordering above). Plain
/// sorted-`Vec` merge -- no hashing, matching every other decision path in
/// this module.
fn merge_sorted(a: Vec<CachedCandidate>, b: Vec<CachedCandidate>) -> Vec<CachedCandidate> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let mut a = a.into_iter();
    let mut b = b.into_iter();
    let mut next_a = a.next();
    let mut next_b = b.next();
    loop {
        match (next_a.take(), next_b.take()) {
            (Some(x), Some(y)) => {
                if x.key <= y.key {
                    next_b = Some(y);
                    out.push(x);
                    next_a = a.next();
                } else {
                    next_a = Some(x);
                    out.push(y);
                    next_b = b.next();
                }
            }
            (Some(x), None) => {
                out.push(x);
                out.extend(a);
                break;
            }
            (None, Some(y)) => {
                out.push(y);
                out.extend(b);
                break;
            }
            (None, None) => break,
        }
    }
    out
}
