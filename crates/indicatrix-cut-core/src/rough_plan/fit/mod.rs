//! Exact single-stone fitting in convex rough regions.
//!
//! Fits candidate library designs into modelled rough geometry using their true convex
//! outline, arbitrary 3D rotation, and dense LP translation/scaling. Uses a three-stage
//! search: coarse screening with proxy polyhedra, exact evaluation on full geometry over
//! fine orientations, and local pattern-search polishing.
//!
//! The stages are exposed separately so a caller can spread designs over several lanes and
//! still get exactly the result of one sequential run:
//!
//! 1. [`screen_designs`] scores every design of a chunk against the coarse region;
//! 2. [`shortlist`] picks the designs worth an exact search from the scores of ALL chunks
//!    (the cut-off is global, never per chunk);
//! 3. [`fit_shortlisted`] runs the exact search and the polish on the shortlisted designs of
//!    a chunk;
//! 4. [`merge_fits`] ranks the fits of all chunks and keeps the best.
//!
//! [`fit_single_stones`] is the sequential composition of the four.
//!
//! # Coarse and fine regions
//!
//! Screening runs against the coarse region (a 16-gon cylinder, a 42-plane pebble), the
//! exact stage against the fine one (a 64-gon cylinder, a 162-plane pebble). Both are
//! polytopes inscribed in the true curved solid; the coarse one is smaller (a 16-gon has
//! about 2.5 % less area than a 64-gon) and neither contains the other exactly. Screening is
//! therefore pessimistic on average about what will fit, which is fine for ranking designs
//! but means a coarse score is not an upper bound on the fine fit. The shortlist keeps a
//! generous margin (at least 48 designs) to absorb that.
//!
//! For the same reason the exact stage cannot skip a fine LP because the coarse LP of the
//! same orientation was poor: the coarse planes are not a subset of the fine ones (the
//! offsets differ, and so do the plane counts), so a coarse scale is neither an upper nor a
//! lower bound on the fine one and such a branch-and-bound would change the result.
//!
//! # Basins
//!
//! The best grid orientations of a design usually are neighbours of one optimum. Both the
//! screening stage and the exact stage therefore keep only the best orientation of each
//! basin (two orientations closer than about 15 degrees are one basin), so the polish
//! starts in several different optima instead of polishing one of them three times.

mod orient;
mod support;

pub use orient::{
    EXACT_LEVEL, EXACT_ORIENTATIONS, EXACT_SPINS, SCREEN_LEVEL, SCREEN_ORIENTATIONS, SCREEN_SPINS,
};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_edge;
#[cfg(test)]
mod tests_search;

use glam::DVec3;

use orient::{
    Quat, diagonal_perturbation, generate_orientations, perturbation, screening_orientations,
};
use support::{SupportWorkspace, compute_proxy_vertices, opposite_pairs};

use crate::{
    rough_plan::types::{PlanProgress, PlanSettings},
    yield_metrics::carat_weight,
};

/// Initial step size in radians for pattern search polishing (6 degrees = 0.105 rad).
const POLISH_START_STEP: f64 = 0.105;
/// Lower termination threshold for pattern search step size in radians (about 0.05 degrees).
///
/// The step is halved from [`POLISH_START_STEP`] until it falls below this, so the last step
/// tried is `0.105 / 64 = 0.00164 rad` (0.094 degrees).
const POLISH_MIN_STEP: f64 = 0.000_872_5;
/// Steps below this also try moves about two axes at once. With the halving above these are
/// the last two step sizes, `0.00328` and `0.00164 rad`, where a single-axis search stalls on
/// the ridges of the scale function.
const POLISH_DIAGONAL_BELOW: f64 = 4.0 * POLISH_MIN_STEP;
/// Most evaluations one single-axis step size may spend.
const POLISH_MAX_EVALS_PER_STEP: usize = 40;
/// Most evaluations one step size with diagonal moves may spend (five passes of 18 moves).
const POLISH_MAX_EVALS_DIAGONAL: usize = 90;
/// Smallest number of designs that get an exact search.
const SHORTLIST_MIN: usize = 48;
/// Orientation basins kept per design by the exact stage.
const TOP_KEEP: usize = 4;
/// Orientation basins of the screening stage that are re-scored with the full outline.
const SCREEN_REFINE: usize = 4;
/// Two orientations whose quaternion alignment `|q1 . q2|` is at least this are one basin:
/// `cos(7.5 degrees)`, the half-angle of a 15 degree rotation.
const BASIN_ALIGNMENT: f64 = 0.991_444_861_373_81;
/// The exact stage checks for a cancel every this many orientations of a design.
const POLL_EVERY: usize = 256;
/// Relative slack on the scale bound before an orientation is skipped; covers the LP's own
/// feasibility tolerance and the tolerance on "opposite" normals.
const PRUNE_SLACK: f64 = 1e-6;

type OrientCandidate = (f64, Quat, [f64; 3]);
type ExactCandidate = (usize, Vec<OrientCandidate>);

/// A design's convex outline for exact single-stone fitting.
///
/// Contains outline vertices in the caliper frame, bounding-box centred,
/// in model units, plus the design's volume and caliper width in the same units.
/// Built by the app from the cached hull.
#[derive(Debug, Clone, PartialEq)]
pub struct DesignHull {
    /// The design's library entry ID.
    pub entry_id: i64,
    /// Convex outline vertices in model units, centered at the bounding-box center.
    pub vertices: Vec<[f64; 3]>,
    /// Design volume in model units cubed.
    pub volume: f64,
    /// Caliper width in model units.
    pub width: f64,
}

/// Where one stone sits. `axes[1]` is the table normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StonePose {
    /// Caliper-frame origin in the rough frame in mm.
    pub center_mm: [f64; 3],
    /// Three unit vectors: the caliper frame's x, y, z axes in the rough frame.
    /// `axes[1]` is the table normal.
    pub axes: [[f64; 3]; 3],
    /// Millimetres per model unit.
    pub mm_per_unit: f64,
}

/// One design's best single-stone fit.
#[derive(Debug, Clone, PartialEq)]
pub struct SingleFit {
    /// The design's library entry ID.
    pub entry_id: i64,
    /// Spatial placement of the stone in the rough.
    pub pose: StonePose,
    /// Finished volume in mm^3.
    pub volume_mm3: f64,
    /// Finished weight in carats.
    pub carat: f64,
}

/// Stages of single-stone fitting for progress reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitStage {
    /// Coarse screening of all candidate designs using proxy hulls.
    Screen,
    /// Exact search over fine orientation grid for shortlisted designs.
    Exact,
    /// Local continuous polishing of top orientations.
    Polish,
}

/// Whether `hull` describes a solid the fit can work with: at least one vertex, all
/// coordinates finite, and a finite, positive volume and width.
///
/// A design failing this scores `0.0` in screening and never gets a fit; it is not an
/// error, because a catalogue may well hold a design whose outline could not be measured.
fn usable_hull(hull: &DesignHull) -> bool {
    !hull.vertices.is_empty()
        && hull
            .vertices
            .iter()
            .flatten()
            .all(|coord| coord.is_finite())
        && hull.volume.is_finite()
        && hull.volume > 0.0
        && hull.width.is_finite()
        && hull.width > 0.0
}

/// The best single-stone fit of every design in `hulls` inside `region` (already inset by
/// skin + allowance), best first, at most `keep` results. `None` when cancelled.
///
/// This is [`screen_designs`], [`shortlist`], [`fit_shortlisted`] and [`merge_fits`] run in
/// sequence over all of `hulls`.
pub fn fit_single_stones(
    region: &[(DVec3, f64)],
    coarse_region: &[(DVec3, f64)],
    hulls: &[DesignHull],
    settings: &PlanSettings,
    keep: usize,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<SingleFit>> {
    if hulls.is_empty() || keep == 0 {
        return Some(Vec::new());
    }

    let scores = screen_designs(coarse_region, hulls, settings, on_progress)?;
    let mut chosen = shortlist(&scores, keep);
    chosen.sort_unstable();
    let subset: Vec<DesignHull> = hulls
        .iter()
        .filter(|hull| chosen.binary_search(&hull.entry_id).is_ok())
        .cloned()
        .collect();

    let fits = fit_shortlisted(region, &subset, settings, on_progress)?;
    Some(merge_fits(fits, keep))
}

/// Scores every design in `hulls` against `coarse_region`.
///
/// The score is the volume of the largest copy of the design that the screening found in
/// the coarse region, returned as `(entry_id, volume_mm3)` in the order of `hulls`. A
/// design that does not fit at all, or whose outline is unusable, scores `0.0`. `None` when
/// cancelled.
///
/// The orientations are searched with the design's 26-point proxy (the extreme vertices
/// along 26 fixed directions), which is cheap but sits inside the design and so fits
/// larger than the design does. The best orientation of each of the [`SCREEN_REFINE`]
/// best basins is therefore evaluated again with the full outline, and the score is
/// `k^3 * volume` of the best of those: the volume of a stone that really fits, never the
/// inflated proxy figure. The orientation table is the octahedral grid plus the proxy's own
/// directions ([`screening_orientations`]).
///
/// The score of a design depends only on that design and the region, so the scores of
/// several chunks can simply be concatenated.
pub fn screen_designs(
    coarse_region: &[(DVec3, f64)],
    hulls: &[DesignHull],
    _settings: &PlanSettings,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<(i64, f64)>> {
    let total_hulls = hulls.len();
    if !on_progress(PlanProgress::Fit {
        stage: FitStage::Screen,
        done: 0,
        total: total_hulls,
    }) {
        return None;
    }

    let screen_orients = screening_orientations();
    let mut workspace = SupportWorkspace::new(coarse_region.len());
    let mut scores = Vec::with_capacity(total_hulls);
    for (hull_idx, hull) in hulls.iter().enumerate() {
        let score = screening_score(hull, &screen_orients, coarse_region, &mut workspace);
        scores.push((hull.entry_id, score));

        if !on_progress(PlanProgress::Fit {
            stage: FitStage::Screen,
            done: hull_idx + 1,
            total: total_hulls,
        }) {
            return None;
        }
    }
    Some(scores)
}

/// The screening score of one design: see [`screen_designs`].
fn screening_score(
    hull: &DesignHull,
    orients: &[orient::Orientation],
    coarse_region: &[(DVec3, f64)],
    workspace: &mut SupportWorkspace,
) -> f64 {
    if !usable_hull(hull) {
        return 0.0;
    }
    let proxy_verts = compute_proxy_vertices(&hull.vertices);

    let mut basins: Vec<OrientCandidate> = Vec::with_capacity(SCREEN_REFINE + 1);
    for orient in orients {
        if let Some((k, center)) = workspace.evaluate(&orient.axes, coarse_region, &proxy_verts)
            && k > 0.0
        {
            insert_candidate(&mut basins, (k, orient.quat, center), SCREEN_REFINE);
        }
    }

    let mut best_k = 0.0_f64;
    for (_, quat, _) in &basins {
        if let Some((k, _)) = workspace.evaluate(&quat.columns(), coarse_region, &hull.vertices)
            && k > best_k
        {
            best_k = k;
        }
    }
    best_k * best_k * best_k * hull.volume
}

/// Picks the designs that get an exact search from the scores of ALL designs.
///
/// `scores` are `(entry_id, volume)` pairs. The best `max(48, 4 * keep)` by volume
/// are chosen (descending, ties by ascending `entry_id`), or all of them when there
/// are fewer. Returned best first.
///
/// The cut-off is over the whole candidate set, so a caller that screened in chunks must
/// concatenate every chunk's scores before calling this.
#[must_use]
pub fn shortlist(scores: &[(i64, f64)], keep: usize) -> Vec<i64> {
    if keep == 0 {
        return Vec::new();
    }
    let cap = keep.saturating_mul(4).max(SHORTLIST_MIN).min(scores.len());
    let mut ranked = scores.to_vec();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.into_iter().take(cap).map(|(id, _)| id).collect()
}

/// Runs the exact orientation search and the polish for every design in `hulls` (normally
/// the shortlisted designs of one chunk) inside `region`.
///
/// Returns one [`SingleFit`] per design that fits with at least the minimum width, in the
/// order of `hulls`, not ranked and not truncated; [`merge_fits`] ranks and truncates.
/// `None` when cancelled.
///
/// Besides one [`PlanProgress::Fit`] event per finished design, the exact stage reports
/// one with `done == 0` every 256 orientations of a design (8,256 LPs, tens of
/// milliseconds each in a debug build), so a cancel is seen within a fraction of a design.
/// Such an event does not advance the stage.
pub fn fit_shortlisted(
    region: &[(DVec3, f64)],
    hulls: &[DesignHull],
    settings: &PlanSettings,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<SingleFit>> {
    let exact_orients = generate_orientations(EXACT_LEVEL, EXACT_SPINS);
    let pairs = opposite_pairs(region);
    let mut workspace = SupportWorkspace::new(region.len());

    let exact_candidates = exact_search(
        hulls,
        &exact_orients,
        region,
        &pairs,
        &mut workspace,
        on_progress,
    )?;
    polish_exact_candidates(
        hulls,
        exact_candidates,
        region,
        settings,
        &mut workspace,
        on_progress,
    )
}

/// Ranks `fits` by finished volume (descending, ties by ascending `entry_id`) and keeps the
/// best `keep`.
#[must_use]
pub fn merge_fits(mut fits: Vec<SingleFit>, keep: usize) -> Vec<SingleFit> {
    fits.sort_by(|fit_a, fit_b| {
        fit_b
            .volume_mm3
            .total_cmp(&fit_a.volume_mm3)
            .then(fit_a.entry_id.cmp(&fit_b.entry_id))
    });
    fits.truncate(keep);
    fits
}

fn exact_search(
    hulls: &[DesignHull],
    exact_orients: &[orient::Orientation],
    region: &[(DVec3, f64)],
    pairs: &[(usize, usize)],
    workspace: &mut SupportWorkspace,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<ExactCandidate>> {
    let exact_total = hulls.len();
    if !on_progress(PlanProgress::Fit {
        stage: FitStage::Exact,
        done: 0,
        total: exact_total,
    }) {
        return None;
    }

    let mut exact_candidates = Vec::with_capacity(exact_total);
    for (hull_idx, hull) in hulls.iter().enumerate() {
        let basins = if usable_hull(hull) {
            let mut poll = || {
                on_progress(PlanProgress::Fit {
                    stage: FitStage::Exact,
                    done: 0,
                    total: exact_total,
                })
            };
            let (basins, _solves) =
                search_orientations(hull, exact_orients, region, pairs, workspace, &mut poll)?;
            basins
        } else {
            Vec::new()
        };

        exact_candidates.push((hull_idx, basins));

        if !on_progress(PlanProgress::Fit {
            stage: FitStage::Exact,
            done: hull_idx + 1,
            total: exact_total,
        }) {
            return None;
        }
    }
    Some(exact_candidates)
}

/// The best orientation of each of the best [`TOP_KEEP`] basins of `hull` over `orients`,
/// best first, and the number of LPs solved. `None` when `poll` (asked every
/// [`POLL_EVERY`] orientations) says to stop.
///
/// Once [`TOP_KEEP`] basins are held, an orientation whose scale bound (see
/// [`SupportWorkspace::scale_bound`], with [`PRUNE_SLACK`]) does not exceed the scale of the
/// worst held basin is skipped without solving its LP. The result is bit-identical to
/// solving every LP: such an orientation's scale is at most its bound, and
/// [`insert_candidate`] ignores a candidate that does not beat the worst held basin, whether
/// or not it lies in a held basin. Passing no `pairs` turns the pruning off.
fn search_orientations(
    hull: &DesignHull,
    orients: &[orient::Orientation],
    region: &[(DVec3, f64)],
    pairs: &[(usize, usize)],
    workspace: &mut SupportWorkspace,
    poll: &mut dyn FnMut() -> bool,
) -> Option<(Vec<OrientCandidate>, usize)> {
    let mut basins: Vec<OrientCandidate> = Vec::with_capacity(TOP_KEEP + 1);
    let mut solves = 0_usize;
    for (index, orient) in orients.iter().enumerate() {
        if index > 0 && index % POLL_EVERY == 0 && !poll() {
            return None;
        }
        if !workspace.prepare(&orient.axes, region, &hull.vertices) {
            continue;
        }
        if basins.len() >= TOP_KEEP
            && let Some(worst) = basins.last()
        {
            let bound = workspace.scale_bound(pairs);
            if bound.mul_add(PRUNE_SLACK, bound) <= worst.0 {
                continue;
            }
        }
        solves += 1;
        if let Some((k, center)) = workspace.solve()
            && k > 0.0
        {
            insert_candidate(&mut basins, (k, orient.quat, center), TOP_KEEP);
        }
    }
    Some((basins, solves))
}

fn polish_exact_candidates(
    hulls: &[DesignHull],
    exact_candidates: Vec<ExactCandidate>,
    region: &[(DVec3, f64)],
    settings: &PlanSettings,
    workspace: &mut SupportWorkspace,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<SingleFit>> {
    let polish_total = exact_candidates.len();
    if !on_progress(PlanProgress::Fit {
        stage: FitStage::Polish,
        done: 0,
        total: polish_total,
    }) {
        return None;
    }

    let mut fits = Vec::new();
    for (p_idx, (hull_idx, basins)) in exact_candidates.into_iter().enumerate() {
        let hull = &hulls[hull_idx];
        let mut best_fit: Option<(f64, [f64; 3], Quat)> = None;

        for (k_init, q_init, t_init) in basins {
            let (k_pol, t_pol, q_pol) =
                polish_orientation(q_init, k_init, t_init, &hull.vertices, region, workspace);
            if best_fit.as_ref().is_none_or(|(bk, _, _)| k_pol > *bk) {
                best_fit = Some((k_pol, t_pol, q_pol));
            }
        }

        if let Some((k, center_mm, best_q)) = best_fit {
            let stone_width = k * hull.width;
            if stone_width >= settings.min_width_mm {
                let volume_mm3 = k * k * k * hull.volume;
                let carat = carat_weight(volume_mm3, settings.specific_gravity);
                let axes = best_q.columns();
                fits.push(SingleFit {
                    entry_id: hull.entry_id,
                    pose: StonePose {
                        center_mm,
                        axes,
                        mm_per_unit: k,
                    },
                    volume_mm3,
                    carat,
                });
            }
        }

        if !on_progress(PlanProgress::Fit {
            stage: FitStage::Polish,
            done: p_idx + 1,
            total: polish_total,
        }) {
            return None;
        }
    }
    Some(fits)
}

/// Whether two orientations lie in one basin (closer than about 15 degrees).
fn same_basin(first: Quat, second: Quat) -> bool {
    first.alignment(second) >= BASIN_ALIGNMENT
}

/// Adds `candidate` to `kept`, the best orientation of each basin sorted by scale
/// descending, holding at most `cap` basins.
///
/// A candidate in the basin of a held orientation that is at least as good is dropped; one
/// that is better replaces every held orientation of its basin (a candidate between two
/// held basins can be in both). Otherwise it takes a free slot or, when `kept` is full,
/// the place of the worst held basin if it beats that one. The held orientations are
/// therefore always pairwise in different basins. A candidate that does not beat the worst
/// scale of a full `kept` never changes it.
fn insert_candidate(kept: &mut Vec<OrientCandidate>, candidate: OrientCandidate, cap: usize) {
    if kept.len() >= cap && kept.last().is_some_and(|worst| candidate.0 <= worst.0) {
        return;
    }
    if kept
        .iter()
        .any(|held| held.0 >= candidate.0 && same_basin(held.1, candidate.1))
    {
        return;
    }
    kept.retain(|held| !same_basin(held.1, candidate.1));
    if kept.len() >= cap {
        kept.pop();
    }
    kept.push(candidate);
    kept.sort_by(|item_a, item_b| item_b.0.total_cmp(&item_a.0));
}

/// The scale, centre and orientation a polish is currently at.
struct PolishPose {
    k: f64,
    center: [f64; 3],
    quat: Quat,
}

/// The rotations one polish pass tries at `step`, in order: the six single-axis moves and,
/// when `diagonal`, the twelve moves about two axes at once.
fn polish_moves(step: f64, diagonal: bool) -> Vec<Quat> {
    let mut moves = Vec::with_capacity(18);
    for axis in 0..3 {
        for sign in [1.0, -1.0] {
            moves.push(perturbation(axis, sign, step));
        }
    }
    if diagonal {
        for (first, second) in [(0, 1), (0, 2), (1, 2)] {
            for sign_first in [1.0, -1.0] {
                for sign_second in [1.0, -1.0] {
                    moves.push(diagonal_perturbation(
                        first,
                        second,
                        sign_first,
                        sign_second,
                        step,
                    ));
                }
            }
        }
    }
    moves
}

/// Applies the best strictly improving move of `moves` again and again until none improves
/// or `cap` evaluations are spent.
fn polish_step(
    pose: &mut PolishPose,
    moves: &[Quat],
    cap: usize,
    vertices: &[[f64; 3]],
    region: &[(DVec3, f64)],
    workspace: &mut SupportWorkspace,
) {
    let mut evals = 0;
    while evals < cap {
        let mut best: Option<(f64, Quat, [f64; 3])> = None;
        let mut best_k = pose.k;
        for delta in moves {
            if evals >= cap {
                break;
            }
            evals += 1;

            let cand_q = pose.quat.mul(*delta).normalize();
            let cand_axes = cand_q.columns();
            if let Some((cand_k, cand_t)) = workspace.evaluate(&cand_axes, region, vertices)
                && cand_k > best_k
            {
                best_k = cand_k;
                best = Some((cand_k, cand_q, cand_t));
            }
        }

        let Some((k, quat, center)) = best else {
            break;
        };
        *pose = PolishPose { k, center, quat };
    }
}

/// Refines an orientation using pattern search down to a step of about 0.09 degrees.
///
/// The step starts at [`POLISH_START_STEP`] and is halved while it is at least
/// [`POLISH_MIN_STEP`]; the last two step sizes also try moves about two axes at once.
/// A move is only accepted when it strictly increases the scale, so the returned scale is
/// never below `init_k`.
fn polish_orientation(
    init_quat: Quat,
    init_k: f64,
    init_t: [f64; 3],
    vertices: &[[f64; 3]],
    region: &[(DVec3, f64)],
    workspace: &mut SupportWorkspace,
) -> (f64, [f64; 3], Quat) {
    let mut pose = PolishPose {
        k: init_k,
        center: init_t,
        quat: init_quat,
    };

    let mut step = POLISH_START_STEP;
    loop {
        if step < POLISH_MIN_STEP {
            break;
        }
        let diagonal = step < POLISH_DIAGONAL_BELOW;
        let cap = if diagonal {
            POLISH_MAX_EVALS_DIAGONAL
        } else {
            POLISH_MAX_EVALS_PER_STEP
        };
        let moves = polish_moves(step, diagonal);
        polish_step(&mut pose, &moves, cap, vertices, region, workspace);
        step *= 0.5;
    }

    (pose.k, pose.center, pose.quat)
}
