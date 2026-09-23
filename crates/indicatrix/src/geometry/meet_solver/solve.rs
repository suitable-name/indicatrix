//! The three-phase solve pipeline: [`SolveContext`] precomputes everything a
//! design's solve shares (normals, block classification, resolved named
//! references, anchors, priors) once, and [`SolveContext::run_pipeline`] runs
//! the constructive pass / block estimate / refinement sweeps described in the
//! module docs any number of times with different decision overrides -- the
//! reuse [`solve_meet_points_verified`](super::solve_meet_points_verified)'s
//! repair search depends on. [`solve_meet_points`] is the plain, no-override
//! entry point.

use super::{
    BLANK_DOMINATION_FACTOR, DEFAULT_PLAUSIBLE_SCALE, EPS_INCIDENT, LEVEL_TOL,
    MAX_CONSTRUCTIVE_SWEEPS, MAX_PLANES, MAX_REFINE_SWEEPS, MeetConstraint, MeetTierInput,
    SolveControl, SolveError, SolvePhase, SolveProgress, SolveStrategy, SolvedTier,
    blocks::{Block, classify_blocks, tier_sides},
    candidates::{
        CandidateVertex, SolvePlane, blank_planes, enumerate_candidate_vertices_cancellable,
        filter_levels_by_instance_support, group_levels, tier_normals,
    },
    names::MeetNameResolver,
    phase1_cache::{CachedCandidate, Phase1Cache},
};
use crate::geometry::stone_metrics::{ExternalProportions, measure_solid};
use glam::DVec3;
use std::collections::BTreeMap;

// NOTE: tried an annihilation guard ("a pick must not erase any other facet's
// corners", physically justified since every tier has positive final-stone area):
// it presumes the surrounding facets are already near-correct, so from a wrong
// intermediate configuration it just entrenches the wrong values; rejected.

/// How a tier's current value was last established, for strategy/detail reporting.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// Untouched (still at the scale prior).
    Unset,
    Anchor,
    /// Constructive pass, named-reference incidence.
    ConstructiveNamed,
    /// Constructive pass, rank-1 prior.
    ConstructiveRank1,
    /// Per-block estimate (phase 2), not yet snapped to a vertex.
    Estimated,
    /// Estimate subsequently snapped to a vertex level by the refinement sweeps.
    Refined,
}

/// Per-block least-squares fit of `mast ~ a*cos(theta) + b*sin(theta)` over the
/// already-settled tiers of one block, evaluated for a target tier. `theta` is
/// recovered per tier from its first instance normal. Falls back to a pure
/// `b*sin(theta)` fit, then to `fallback`, as data thins out.
fn block_estimate(
    members: &[(f64, f64, f64)], // (cos_theta, sin_theta, mast) of settled same-block tiers
    cos_t: f64,
    sin_t: f64,
    fallback: f64,
) -> f64 {
    // 2x2 normal equations for [a, b].
    if members.len() >= 2 {
        let (mut cc, mut cs, mut ss, mut cm, mut sm) = (0.0_f64, 0.0, 0.0, 0.0, 0.0);
        for &(c, s, m) in members {
            cc = c.mul_add(c, cc);
            cs = c.mul_add(s, cs);
            ss = s.mul_add(s, ss);
            cm = c.mul_add(m, cm);
            sm = s.mul_add(m, sm);
        }
        // Determinant of [[cc, cs], [cs, ss]] -- `cs * cs` is deliberate.
        let det = cs.mul_add(-cs, cc * ss);
        if det.abs() > 1e-9 {
            let a = sm.mul_add(-cs, cm * ss) / det;
            let b = cm.mul_add(-cs, sm * cc) / det;
            let est = a.mul_add(cos_t, b * sin_t);
            if est.is_finite() && est > 1e-6 {
                return est;
            }
        }
    }
    // b-only fit through the origin of the cos axis.
    let usable: Vec<&(f64, f64, f64)> = members.iter().filter(|(_, s, _)| *s > 0.1).collect();
    if !usable.is_empty() {
        let b = usable.iter().map(|(_, s, m)| m / s).sum::<f64>() / usable.len() as f64;
        let est = b * sin_t;
        if est.is_finite() && est > 1e-6 {
            return est;
        }
    }
    fallback
}

/// Everything one full pipeline run (phases 1-3) produces, before it is mapped to
/// per-tier [`SolvedTier`]s.
pub(super) struct PipelineResult {
    pub(super) mast: Vec<f64>,
    origin: Vec<Origin>,
    pub(super) last_pick: Vec<Option<(usize, usize)>>,
    named_cause: Vec<Option<&'static str>>,
    refine_sweeps: usize,
    converged: bool,
}

/// Solves every tier's mast distance.
///
/// See the module docs for the model (vertex incidence), what the caller must
/// anchor (one scale reference per crown/pavilion/girdle block), and the
/// three-phase algorithm.
///
/// `gear_teeth_abs` is the index wheel's tooth count (see
/// [`indicatrix_formats::asc::AscSchedule::gear_teeth_abs`]). `tiers` should be in the
/// schedule's own file order (needed only to resolve an unsigned-zero angle's
/// crown/pavilion side); the solve itself is order-independent.
///
/// Deterministic: two calls with identical inputs produce identical outputs. No
/// hashed iteration and no convex-hull library anywhere in this path.
///
/// Latency: `O(P^3)` per refinement sweep in the plane count `P`; see the
/// module docs' "Cost envelope" section for the measured figures. Never
/// cancels and reports no progress -- see [`solve_meet_points_with`] for
/// that; this is a thin wrapper passing a no-op [`SolveControl`], and above
/// [`MAX_PLANES`] it keeps the legacy behavior of returning an all-
/// [`SolveStrategy::Failed`] result instead of an error.
#[must_use]
pub fn solve_meet_points(gear_teeth_abs: u32, tiers: &[MeetTierInput]) -> Vec<SolvedTier> {
    match solve_meet_points_with(gear_teeth_abs, tiers, &SolveControl::default()) {
        Ok(solved) => solved,
        Err(SolveError::TooManyPlanes { .. }) => {
            SolveContext::new(gear_teeth_abs, tiers).failed_solved()
        }
        Err(SolveError::Cancelled) => {
            unreachable!("SolveControl::default() never sets cancel")
        }
    }
}

/// Cancellable, progress-reporting sibling of [`solve_meet_points`].
///
/// Same algorithm and same determinism guarantee: an unused `control`, i.e.
/// [`SolveControl::default`], reproduces [`solve_meet_points`] exactly, bit
/// for bit. Checks `control` at every cancel point (see the module docs,
/// "Cancellation and progress") and reports a [`SolveProgress`] at every
/// point that has one.
///
/// # Errors
///
/// [`SolveError::TooManyPlanes`] when the design has more than [`MAX_PLANES`]
/// facet-plane instances (checked before any solving starts, so this returns
/// immediately); [`SolveError::Cancelled`] the first time `control`'s cancel
/// flag is observed set.
pub fn solve_meet_points_with(
    gear_teeth_abs: u32,
    tiers: &[MeetTierInput],
    control: &SolveControl<'_>,
) -> Result<Vec<SolvedTier>, SolveError> {
    let ctx = SolveContext::new(gear_teeth_abs, tiers);
    if ctx.total_planes > MAX_PLANES {
        return Err(SolveError::TooManyPlanes {
            planes: ctx.total_planes,
            max: MAX_PLANES,
        });
    }
    if control.is_cancelled() {
        return Err(SolveError::Cancelled);
    }
    let result = ctx.run_pipeline(&BTreeMap::new(), &BTreeMap::new(), control)?;
    Ok(ctx.to_solved(&result))
}

/// Precomputed, immutable per-design solve state: everything every pipeline run
/// shares (normals, block classification, name resolution, anchors, priors).
///
/// Built once per design; [`Self::run_pipeline`] can then run any number of
/// times -- with different decision overrides -- without re-deriving any of it.
/// That reuse is what makes
/// [`solve_meet_points_verified`](super::solve_meet_points_verified)'s
/// externally-scored repair search affordable.
pub(super) struct SolveContext<'a> {
    pub(super) tiers: &'a [MeetTierInput],
    normals: Vec<Vec<DVec3>>,
    blocks: Vec<Block>,
    resolved_named: Vec<Vec<usize>>,
    pub(super) is_anchor: Vec<bool>,
    scale_prior: f64,
    domination_limit: f64,
    pub(super) total_planes: usize,
}

impl<'a> SolveContext<'a> {
    pub(super) fn new(gear_teeth_abs: u32, tiers: &'a [MeetTierInput]) -> Self {
        let sides = tier_sides(tiers);
        let normals: Vec<Vec<DVec3>> = tiers
            .iter()
            .zip(&sides)
            .map(|(t, &crown)| tier_normals(gear_teeth_abs, t.angle_deg, &t.indices, crown))
            .collect();

        let blocks = classify_blocks(tiers);

        // Resolve every MeetNamed tier's references up front via the shared
        // [`MeetNameResolver`] rule set (see its doc comment).
        let resolver = MeetNameResolver::new(tiers);
        let resolved_named: Vec<Vec<usize>> = tiers
            .iter()
            .enumerate()
            .map(|(i, t)| match &t.constraint {
                MeetConstraint::MeetNamed(names) => {
                    let mut refs = resolver.resolve_names(names).refs;
                    refs.retain(|&r| r != i);
                    refs
                }
                _ => Vec::new(),
            })
            .collect();

        let is_anchor: Vec<bool> = tiers
            .iter()
            .map(|t| matches!(t.constraint, MeetConstraint::ScaleReference(_)))
            .collect();
        let scale_prior = tiers
            .iter()
            .filter_map(|t| match &t.constraint {
                MeetConstraint::ScaleReference(v) => Some(v.abs()),
                _ => None,
            })
            .fold(0.0_f64, f64::max);
        let scale_prior = if scale_prior > 1e-9 {
            scale_prior
        } else {
            DEFAULT_PLAUSIBLE_SCALE
        };
        let domination_limit = BLANK_DOMINATION_FACTOR * scale_prior;

        let total_planes = 6 + normals.iter().map(Vec::len).sum::<usize>();
        Self {
            tiers,
            normals,
            blocks,
            resolved_named,
            is_anchor,
            scale_prior,
            domination_limit,
            total_planes,
        }
    }

    /// The over-[`MAX_PLANES`] early-out result: anchors keep their given masts,
    /// everything else is a flagged placeholder.
    pub(super) fn failed_solved(&self) -> Vec<SolvedTier> {
        let total_planes = self.total_planes;
        self.tiers
            .iter()
            .map(|t| match &t.constraint {
                MeetConstraint::ScaleReference(v) => SolvedTier {
                    mast: v.abs(),
                    strategy: SolveStrategy::ScaleReference,
                    detail: "given (scale reference)".to_string(),
                },
                _ => SolvedTier {
                    mast: self.scale_prior,
                    strategy: SolveStrategy::Failed,
                    detail: format!(
                        "design has {total_planes} planes, above the {MAX_PLANES}-plane cap \
                         for candidate-vertex enumeration"
                    ),
                },
            })
            .collect()
    }

    // The three-phase pipeline. The constructive pass honors strict file order
    // (Gauss-Seidel): `.asc` file order is overwhelmingly cutting order, and the
    // prefix arrangement is exactly what a cutter's meet points existed on. Tried
    // a free-running variant (any tier that can settle in a sweep does): trades a
    // slightly better blended median for a much worse per-design success rate;
    // rejected.
    //
    // `overrides` (tier index -> vertex-level index) forces specific phase-1
    // picks, bypassing the named rule and rank-1 prior; phase 3 then refines an
    // overridden tier by nearest-level only. Empty overrides reproduce the plain
    // solve exactly -- the knob
    // [`solve_meet_points_verified`](super::solve_meet_points_verified)'s repair
    // search turns.
    // `anchor_values` (anchor tier index -> mast) substitutes a different value
    // for a [`MeetConstraint::ScaleReference`] anchor without mutating the tier
    // list -- the knob the calibrated search turns to adjust an estimated anchor.
    // Empty maps reproduce the plain solve exactly.
    //
    // `control` (see the module docs, "Cancellation and progress") is checked
    // once per constructive-pass sweep, once per candidate-enumeration chunk
    // inside phase 3's `enumerate_candidate_vertices_cancellable` call (the
    // measured long pole), and once per refinement sweep; an unused
    // (`SolveControl::default`) control never observes cancel and this
    // reproduces the old infallible pipeline's result exactly.
    #[expect(
        clippy::too_many_lines,
        reason = "three sequential phases of one algorithm (constructive pass, \
                  per-block estimate, nearest-level refinement -- see the module docs' \
                  walkthrough) sharing one set of per-tier working vectors (mast, \
                  origin, settled, last_pick); splitting the phases into separate \
                  functions would just turn those shared locals into a too-many-\
                  arguments (or a bespoke context struct) problem at each call boundary"
    )]
    pub(super) fn run_pipeline(
        &self,
        overrides: &BTreeMap<usize, usize>,
        anchor_values: &BTreeMap<usize, f64>,
        control: &SolveControl<'_>,
    ) -> Result<PipelineResult, SolveError> {
        let tiers: &[MeetTierInput] = self.tiers;
        let n = tiers.len();
        let normals = &self.normals;
        let blocks = &self.blocks;
        let resolved_named = &self.resolved_named;
        let is_anchor = &self.is_anchor;
        let scale_prior = self.scale_prior;
        let domination_limit = self.domination_limit;
        let mut mast: Vec<f64> = tiers
            .iter()
            .enumerate()
            .map(|(i, t)| match &t.constraint {
                MeetConstraint::ScaleReference(v) => {
                    anchor_values.get(&i).copied().unwrap_or_else(|| v.abs())
                }
                _ => scale_prior,
            })
            .collect();
        let mut origin: Vec<Origin> = is_anchor
            .iter()
            .map(|&a| if a { Origin::Anchor } else { Origin::Unset })
            .collect();
        let mut last_pick: Vec<Option<(usize, usize)>> = vec![None; n];
        // Why a tier with resolved named refs nevertheless settled on the rank-1
        // prior in phase 1, for the report's detail string.
        let mut named_cause: Vec<Option<&'static str>> = vec![None; n];

        // ---- Phase 1: constructive pass. ----
        let mut settled: Vec<bool> = is_anchor.clone();
        let mut named_release = false;
        // Incremental candidate-vertex cache for phase 1 only (see `Phase1Cache`'s
        // doc comment); seeded with whatever tiers start out settled (anchors).
        let mut phase1_cache = Phase1Cache::new();
        for (j, &s) in settled.iter().enumerate() {
            if s {
                phase1_cache.add_tier(j, &normals[j], mast[j]);
            }
        }
        let mut last_constructive_sweep = 1u32;
        for pass_idx in 0..MAX_CONSTRUCTIVE_SWEEPS {
            if control.is_cancelled() {
                return Err(SolveError::Cancelled);
            }
            last_constructive_sweep = (pass_idx + 1) as u32;
            control.report(SolveProgress {
                phase: SolvePhase::Constructive,
                sweep: last_constructive_sweep,
                max_sweeps: MAX_CONSTRUCTIVE_SWEEPS as u32,
                blocks_done: settled.iter().filter(|&&s| s).count() as u32,
                blocks_total: n as u32,
            });
            let mut progress = false;
            for i in 0..n {
                if settled[i] {
                    continue;
                }
                // Also checked per unsettled tier, not just once per pass:
                // measured necessary on the real 103-tier fixture in an
                // unoptimized build -- a single pass's cumulative
                // `phase1_cache` filtering cost across every still-unsettled
                // tier can itself exceed a caller's cancel-latency budget
                // even though phase 1 as a whole is the cheap, non-cubic
                // path (see the module docs, "Cancellation and progress").
                if control.is_cancelled() {
                    return Err(SolveError::Cancelled);
                }
                let refs = &resolved_named[i];
                let refs_settled: Vec<usize> =
                    refs.iter().copied().filter(|&t| settled[t]).collect();
                // A named tier waits for its references to settle (they usually
                // do, on a later pass); after a pass with no progress,
                // `named_release` lets it settle from geometry alone.
                if !refs.is_empty() && refs_settled.len() < refs.len() && !named_release {
                    continue;
                }

                // `phase1_cache` mirrors a fresh enumeration of blanks + every
                // settled tier's planes, kept incrementally (see its doc comment).
                let n0 = normals[i][0];
                let feasible: Vec<&CachedCandidate> = phase1_cache
                    .candidates
                    .iter()
                    .filter(|c| c.violated.is_none())
                    .collect();
                let vals: Vec<f64> = feasible.iter().map(|c| n0.dot(c.v)).collect();
                let levels = group_levels(vals.iter().copied().filter(|v| *v > 1e-9).collect());
                if levels.is_empty() {
                    continue;
                }

                // Named rule: shallowest level with a candidate incident to every
                // settled reference. Deliberately all-or-nothing: tried a
                // best-partial-incidence variant (most refs incident wins,
                // shallowest breaking ties), which measured worse (MeetNamed-
                // resolved median rel. err 0.0828 -> 0.1076) -- a weak partial
                // match is evidence the constraint isn't really satisfied there,
                // not a lead worth following; rejected.
                // Stores the *index* into `levels`, not the value, so `last_pick`
                // can be recovered by indexing once at the end instead of an
                // exact-value re-search.
                let mut choice: Option<(usize, Origin)> = None;
                if let Some(&forced) = overrides.get(&i) {
                    // Repair-search override: force this tier's pick to the given
                    // level, bypassing both the named rule and the rank-1 prior.
                    choice = Some((forced.min(levels.len() - 1), Origin::ConstructiveRank1));
                } else if !refs_settled.is_empty() {
                    let found = levels.iter().position(|&head| {
                        feasible.iter().zip(&vals).any(|(c, &val)| {
                            (val - head).abs() <= LEVEL_TOL
                                && refs_settled.iter().all(|&t| {
                                    normals[t]
                                        .iter()
                                        .any(|&nr| (nr.dot(c.v) - mast[t]).abs() < EPS_INCIDENT)
                                })
                        })
                    });
                    if let Some(idx) = found {
                        choice = Some((idx, Origin::ConstructiveNamed));
                    }
                    // Tried two "rescue" variants for the fallback case (refs all
                    // settled, no feasible level incident to them all): accepting
                    // the shallowest candidate incident to all refs that violates
                    // at most one other tier measured worse (MeetNamed-resolved
                    // median rel. err 0.0828 -> 0.0862); building the corner
                    // directly from ref-plane triples under a feasibility slack
                    // measured worse still (0.0897). Same pattern as the
                    // annihilation-guard finding above: weakening the acceptance
                    // test admits self-consistent wrong corners; both rejected.
                }
                let (li, orig) = choice
                    .unwrap_or_else(|| (usize::from(levels.len() >= 2), Origin::ConstructiveRank1));
                let value = levels[li];
                if value.is_finite() && value > 1e-9 && value <= domination_limit {
                    if orig == Origin::ConstructiveRank1
                        && !refs.is_empty()
                        && !overrides.contains_key(&i)
                    {
                        named_cause[i] = Some(if refs_settled.len() < refs.len() {
                            "refs not yet settled at release"
                        } else {
                            "no feasible level incident to all settled refs"
                        });
                    }
                    mast[i] = value;
                    origin[i] = orig;
                    last_pick[i] = Some((li, levels.len()));
                    settled[i] = true;
                    phase1_cache.add_tier(i, &normals[i], mast[i]);
                    progress = true;
                }
            }

            if !progress {
                if named_release {
                    break;
                }
                named_release = true;
            } else if settled.iter().all(|&s| s) {
                break;
            }
        }
        // Final constructive-phase report: the loop above can `break` as soon
        // as every tier settles, mid-pass, without another iteration's
        // pre-pass report ever reflecting that -- so report the actual final
        // settled count once more here (a no-op duplicate of the last
        // in-loop report when the loop instead ran out its full sweep cap
        // without settling everything).
        control.report(SolveProgress {
            phase: SolvePhase::Constructive,
            sweep: last_constructive_sweep,
            max_sweeps: MAX_CONSTRUCTIVE_SWEEPS as u32,
            blocks_done: settled.iter().filter(|&&s| s).count() as u32,
            blocks_total: n as u32,
        });

        // ---- Phase 2: per-block estimates for everything still unsettled. ----
        if control.is_cancelled() {
            return Err(SolveError::Cancelled);
        }
        control.report(SolveProgress {
            phase: SolvePhase::LeastSquares,
            sweep: 1,
            max_sweeps: 1,
            blocks_done: 0,
            blocks_total: n as u32,
        });
        for i in 0..n {
            if settled[i] {
                continue;
            }
            let members: Vec<(f64, f64, f64)> = (0..n)
                .filter(|&j| settled[j] && blocks[j] == blocks[i])
                .map(|j| {
                    let y = normals[j][0].y.abs();
                    (y, y.mul_add(-y, 1.0).max(0.0).sqrt(), mast[j])
                })
                .collect();
            let y = normals[i][0].y.abs();
            mast[i] = block_estimate(&members, y, y.mul_add(-y, 1.0).max(0.0).sqrt(), scale_prior);
            origin[i] = Origin::Estimated;
        }

        // ---- Phase 3: nearest-level refinement sweeps over the full arrangement. ----
        let mut refine_sweeps = 0usize;
        let mut converged = false;
        for sweep_idx in 0..MAX_REFINE_SWEEPS {
            if control.is_cancelled() {
                return Err(SolveError::Cancelled);
            }
            control.report(SolveProgress {
                phase: SolvePhase::Refine,
                sweep: (sweep_idx + 1) as u32,
                max_sweeps: MAX_REFINE_SWEEPS as u32,
                blocks_done: 0,
                blocks_total: n as u32,
            });
            refine_sweeps += 1;
            // Re-enumerated from scratch every sweep: caching the triples (either
            // just their inverses, or the whole candidate evaluation) was tried
            // and rejected -- see the NOTEs at the top of `candidates.rs`.
            let mut planes = blank_planes();
            for (i, ns) in normals.iter().enumerate() {
                for &nv in ns {
                    planes.push(SolvePlane {
                        n: nv,
                        m: mast[i],
                        owner: i,
                    });
                }
            }
            // The measured long pole (see the module docs): checks `control`
            // once per outer-plane chunk internally, so a cancel lands well
            // before this whole `O(P^3)` scan finishes.
            let Some(cands) = enumerate_candidate_vertices_cancellable(&planes, 6, control.cancel)
            else {
                return Err(SolveError::Cancelled);
            };
            let mut new_mast = mast.clone();

            for i in 0..n {
                if is_anchor[i] {
                    continue;
                }
                // Reserved up front: on a large design nearly every candidate is
                // usable for nearly every tier (~10^5 entries), and growing the
                // vector by doubling measured as a visible share of per-tier cost.
                let mut usable: Vec<&CandidateVertex> = Vec::with_capacity(cands.len());
                usable.extend(cands.iter().filter(|c| {
                    !c.owners.contains(&i) && (c.violated.is_none() || c.violated == Some(i))
                }));
                if usable.is_empty() {
                    continue;
                }
                let n0 = normals[i][0];
                let vals: Vec<f64> = usable.iter().map(|c| n0.dot(c.v)).collect();
                let levels = group_levels(vals.iter().copied().filter(|v| *v > 1e-9).collect());
                let levels = filter_levels_by_instance_support(levels, &normals[i], &usable);
                if levels.is_empty() {
                    continue;
                }

                // Prefer the *shallowest* level incident to every resolved named
                // reference (same rule as phase 1's, and the oracle-measured best
                // pick -- "nearest the current value" would keep a wrong
                // self-consistent value in place); else the level nearest the
                // current value. No annihilation guard (see the NOTE above).
                let refs = &resolved_named[i];
                // Index into `levels`, not the value -- see phase 1's named rule
                // comment for why.
                let nearest_of = || -> Option<usize> {
                    levels
                        .iter()
                        .enumerate()
                        .min_by(|&(_, a), &(_, b)| {
                            (a - mast[i])
                                .abs()
                                .partial_cmp(&(b - mast[i]).abs())
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .map(|(idx, _)| idx)
                };
                // All-or-nothing incidence, for the same measured reason as phase
                // 1's named rule (see the comment there). An overridden tier
                // refines by nearest-level only, so the sweeps polish the forced
                // pick instead of yanking it back to the named level it was
                // deliberately steered away from.
                let named_pool: Vec<usize> = if refs.is_empty() || overrides.contains_key(&i) {
                    Vec::new()
                } else {
                    levels
                        .iter()
                        .enumerate()
                        .filter(|&(_, &head)| {
                            usable.iter().zip(&vals).any(|(c, &val)| {
                                (val - head).abs() <= LEVEL_TOL
                                    && refs.iter().all(|&t| {
                                        normals[t]
                                            .iter()
                                            .any(|&nr| (nr.dot(c.v) - mast[t]).abs() < EPS_INCIDENT)
                                    })
                            })
                        })
                        .map(|(idx, _)| idx)
                        .collect()
                };
                let pick = named_pool.first().copied().or_else(nearest_of);
                if let Some(li) = pick {
                    let v = levels[li];
                    if v.is_finite() && v > 1e-9 && v <= domination_limit {
                        new_mast[i] = v;
                        last_pick[i] = Some((li, levels.len()));
                        if origin[i] == Origin::Estimated {
                            origin[i] = Origin::Refined;
                        }
                    }
                }
            }
            control.report(SolveProgress {
                phase: SolvePhase::Refine,
                sweep: (sweep_idx + 1) as u32,
                max_sweeps: MAX_REFINE_SWEEPS as u32,
                blocks_done: n as u32,
                blocks_total: n as u32,
            });

            let max_rel_change = mast
                .iter()
                .zip(&new_mast)
                .map(|(&old, &new)| (new - old).abs() / old.abs().max(1e-9))
                .fold(0.0_f64, f64::max);
            mast = new_mast;
            if max_rel_change < 1e-12 {
                converged = true;
                break;
            }
        }

        Ok(PipelineResult {
            mast,
            origin,
            last_pick,
            named_cause,
            refine_sweeps,
            converged,
        })
    }

    /// Maps one pipeline run's raw result to the per-tier [`SolvedTier`] reports.
    pub(super) fn to_solved(&self, result: &PipelineResult) -> Vec<SolvedTier> {
        let variant = "file-order";
        let refine_sweeps = result.refine_sweeps;
        let conv = if result.converged {
            "converged"
        } else {
            "sweep cap"
        };
        (0..self.tiers.len())
            .map(|i| {
                let pick_str = result.last_pick[i].map_or_else(
                    || "no vertex pick".to_string(),
                    |(li, nl)| format!("level {li} of {nl}"),
                );
                match result.origin[i] {
                    Origin::Anchor => SolvedTier {
                        mast: result.mast[i],
                        strategy: SolveStrategy::ScaleReference,
                        detail: "given (scale reference)".to_string(),
                    },
                    Origin::ConstructiveNamed => SolvedTier {
                        mast: result.mast[i],
                        strategy: SolveStrategy::DependencyOrder,
                        detail: format!(
                            "constructive vertex incidence ({variant} pipeline) with {} resolved \
                             named reference(s), then {pick_str} after {refine_sweeps} refinement \
                             sweep(s) ({conv})",
                            self.resolved_named[i].len()
                        ),
                    },
                    Origin::ConstructiveRank1 => SolvedTier {
                        mast: result.mast[i],
                        strategy: SolveStrategy::DependencyOrder,
                        detail: {
                            let cause = result.named_cause[i]
                                .map_or_else(String::new, |c| format!(" [named fallback: {c}]"));
                            format!(
                                "constructive vertex incidence ({variant} pipeline, rank-1 \
                                 prior{cause}), then {pick_str} after {refine_sweeps} refinement \
                                 sweep(s) ({conv})"
                            )
                        },
                    },
                    Origin::Refined => SolvedTier {
                        mast: result.mast[i],
                        strategy: SolveStrategy::JointGroup,
                        detail: format!(
                            "mutually-dependent remainder settled by nearest-level refinement \
                            ({variant} pipeline): {pick_str} after {refine_sweeps} sweep(s) ({conv})"
                        ),
                    },
                    Origin::Estimated => SolvedTier {
                        mast: result.mast[i],
                        strategy: SolveStrategy::LeastSquaresFallback,
                        detail: "no usable candidate vertex; mast is the per-block \
                                 a*cos(theta)+b*sin(theta) estimate"
                            .to_string(),
                    },
                    Origin::Unset => SolvedTier {
                        mast: result.mast[i],
                        strategy: SolveStrategy::Failed,
                        detail: "no vertex and no estimate; mast is the scale prior".to_string(),
                    },
                }
            })
            .collect()
    }

    /// External combined-deviation score of one mast configuration against the
    /// design's printed proportions: [`measure_solid`] over the configuration's
    /// full plane arrangement, then
    /// [`ExternalProportions::combined_deviation`]. `INFINITY` when the solid is
    /// unbounded/degenerate or no printed figure overlaps the measurement --
    /// such a configuration can never be verified.
    pub(super) fn config_score(&self, masts: &[f64], targets: &ExternalProportions) -> f64 {
        let planes: Vec<(DVec3, f64)> = self
            .normals
            .iter()
            .zip(masts)
            .flat_map(|(ns, &m)| ns.iter().map(move |&n| (n, m)))
            .collect();
        measure_solid(&planes)
            .and_then(|m| targets.combined_deviation(&m))
            .unwrap_or(f64::INFINITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-verifiable case: a square girdle wall plus flat table/culet (all three
    /// given as scale references) fully cap a box `[-1,1] x [-0.6,0.6] x [-1,1]`. A
    /// fourth facet at 45 degrees, azimuth 45 degrees (pointed straight at one of
    /// the box's corner edges) must land on a *vertex level* of the box's corner
    /// arrangement. Under the rank-1 prior that is the second-highest corner value
    /// in its normal direction: for unit normal `n = (0.5, 1/sqrt2, 0.5)` the box
    /// corners give values `+-0.5 +- 0.6/sqrt2 +- 0.5`, whose distinct levels are
    /// `1 + 0.6/sqrt2` (first touch), then `1 - 0.6/sqrt2` (the rank-1 level this
    /// solver must select), then lower ones.
    #[test]
    fn selects_the_rank1_vertex_level_against_a_capped_box() {
        let gear = 4;
        let tiers = vec![
            MeetTierInput {
                angle_deg: 90.0,
                indices: vec![0.0, 1.0, 2.0, 3.0],
                constraint: MeetConstraint::ScaleReference(1.0),
                names: vec![],
            },
            MeetTierInput {
                angle_deg: 0.0,
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(0.6),
                names: vec![],
            },
            MeetTierInput {
                angle_deg: -0.0,
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(0.6),
                names: vec![],
            },
            MeetTierInput {
                angle_deg: 45.0,
                indices: vec![0.5],
                constraint: MeetConstraint::MeetExisting,
                names: vec![],
            },
        ];

        let solved = solve_meet_points(gear, &tiers);
        assert_eq!(solved.len(), 4);
        for s in &solved[..3] {
            assert_eq!(s.strategy, SolveStrategy::ScaleReference);
        }
        assert_eq!(
            solved[3].strategy,
            SolveStrategy::DependencyOrder,
            "detail: {}",
            solved[3].detail
        );
        let expected = std::f64::consts::FRAC_1_SQRT_2.mul_add(-0.6, 1.0);
        assert!(
            (solved[3].mast - expected).abs() < 1e-6,
            "expected the rank-1 vertex level {expected}, solver produced {} (detail: {})",
            solved[3].mast,
            solved[3].detail
        );
    }

    /// A stated `"Meet <names>"` reference must override the rank-1 prior: the tier
    /// must land exactly on the vertex where its named references meet, even when
    /// that is not the rank-1 level of its own arrangement.
    #[test]
    fn a_stated_named_meet_overrides_the_rank1_prior() {
        // Box as above, plus Y (a 60-degree facet solved by rank-1 first), plus X,
        // which names Y + girdle + table: X must pass through the vertex where those
        // three planes meet.
        let gear = 4;
        let tiers = vec![
            MeetTierInput {
                angle_deg: 90.0,
                indices: vec![0.0, 1.0, 2.0, 3.0],
                constraint: MeetConstraint::ScaleReference(1.0),
                names: vec!["girdle".to_string()],
            },
            MeetTierInput {
                angle_deg: 0.0,
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(0.6),
                names: vec!["table".to_string()],
            },
            MeetTierInput {
                angle_deg: -0.0,
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(0.6),
                names: vec!["culet".to_string()],
            },
            MeetTierInput {
                angle_deg: 45.0,
                indices: vec![0.5],
                constraint: MeetConstraint::MeetNamed(vec![
                    "Y".to_string(),
                    "girdle".to_string(),
                    "table".to_string(),
                ]),
                names: vec!["X".to_string()],
            },
            MeetTierInput {
                angle_deg: 60.0,
                indices: vec![0.5],
                constraint: MeetConstraint::MeetExisting,
                names: vec!["Y".to_string()],
            },
        ];

        let solved = solve_meet_points(gear, &tiers);
        assert_eq!(solved.len(), 5);
        let x = &solved[3];
        assert_eq!(
            x.strategy,
            SolveStrategy::DependencyOrder,
            "X should have solved exactly, got {:?}: {}",
            x.strategy,
            x.detail
        );
        assert!(
            x.detail.contains("named reference"),
            "X should have used its stated named references, detail: {}",
            x.detail
        );

        // Verify the incidence directly: X's plane must pass through a point that
        // also lies on Y's plane, a girdle plane, and the table plane.
        let y = &solved[4];
        let theta_x = 45.0_f64.to_radians();
        let theta_y = 60.0_f64.to_radians();
        let phi = std::f64::consts::FRAC_PI_4; // index 0.5 on a 4-tooth gear
        let n_x = DVec3::new(
            theta_x.sin() * phi.cos(),
            theta_x.cos(),
            theta_x.sin() * phi.sin(),
        );
        let n_y = DVec3::new(
            theta_y.sin() * phi.cos(),
            theta_y.cos(),
            theta_y.sin() * phi.sin(),
        );
        // Vertex of {Y, girdle at azimuth 0 (n = +x), table (n = +y)}:
        let m = glam::DMat3::from_cols(n_y, DVec3::X, DVec3::Y).transpose();
        let v = m.inverse() * DVec3::new(y.mast, 1.0, 0.6);
        assert!(
            (n_x.dot(v) - x.mast).abs() < 1e-6,
            "X's mast {} should equal n_x . v = {} at the named meet vertex",
            x.mast,
            n_x.dot(v)
        );
    }

    /// Two identical calls must produce byte-identical results (the old solver's
    /// convex-hull library was seeded per-process and made the whole pipeline
    /// nondeterministic; this pins the fix).
    #[test]
    fn solving_is_deterministic() {
        let schedule = indicatrix_formats::asc::parse_asc(
            "GemCad 5.0\n\
             g 96 0.0\n\
             y 6 y\n\
             I 1.72\n\
             H PC 45.149  Round Trichecker-12\n\
             a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
             a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
             a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
             a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
             a 10.000000 0.48799664 96 n C 16 32 48 64 80\n",
        )
        .expect("must parse");
        let mut tiers = super::super::meet_tier_inputs_from_asc(&schedule);
        // No stated scale references in this design: anchor one tier per block the
        // same way the corpus harness does (pavilion tier 0, crown tier 2, and the
        // girdle tier 1).
        tiers[0].constraint = MeetConstraint::ScaleReference(schedule.tiers[0].mast);
        tiers[1].constraint = MeetConstraint::ScaleReference(schedule.tiers[1].mast);
        tiers[2].constraint = MeetConstraint::ScaleReference(schedule.tiers[2].mast);

        let a = solve_meet_points(schedule.gear_teeth_abs(), &tiers);
        let b = solve_meet_points(schedule.gear_teeth_abs(), &tiers);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert!(
                x.mast.to_bits() == y.mast.to_bits(),
                "nondeterministic mast"
            );
            assert_eq!(x.strategy, y.strategy);
            assert_eq!(x.detail, y.detail);
        }
    }

    /// The named-meet fixture above, run through [`solve_meet_points_with`]
    /// with a plain [`SolveControl::default`], must reproduce
    /// [`solve_meet_points`] bit for bit -- an unused control must change
    /// nothing about the result.
    #[test]
    fn solve_meet_points_with_a_default_control_matches_solve_meet_points_bitwise() {
        let gear = 4;
        let tiers = vec![
            MeetTierInput {
                angle_deg: 90.0,
                indices: vec![0.0, 1.0, 2.0, 3.0],
                constraint: MeetConstraint::ScaleReference(1.0),
                names: vec![],
            },
            MeetTierInput {
                angle_deg: 0.0,
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(0.6),
                names: vec![],
            },
            MeetTierInput {
                angle_deg: -0.0,
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(0.6),
                names: vec![],
            },
            MeetTierInput {
                angle_deg: 45.0,
                indices: vec![0.5],
                constraint: MeetConstraint::MeetExisting,
                names: vec![],
            },
        ];
        let plain = solve_meet_points(gear, &tiers);
        let via_with = solve_meet_points_with(gear, &tiers, &SolveControl::default())
            .expect("a default control never cancels and this fixture is under MAX_PLANES");
        assert_eq!(plain.len(), via_with.len());
        for (a, b) in plain.iter().zip(&via_with) {
            assert_eq!(a.mast.to_bits(), b.mast.to_bits());
            assert_eq!(a.strategy, b.strategy);
            assert_eq!(a.detail, b.detail);
        }
    }

    /// A control whose cancel flag is already set before the solve starts
    /// must return `Err(SolveCancelled)` (i.e. [`SolveError::Cancelled`])
    /// immediately, without needing a slow fixture or a background thread --
    /// the deterministic half of the cancellation contract. The
    /// wall-clock-latency half (cancelling mid-solve, on the real 103-tier
    /// fixture) is proven at the `indicatrix-cut-core` level, where that
    /// fixture lives -- see `design::tests::cancel_stops_a_large_real_solve_quickly`.
    #[test]
    fn solve_meet_points_with_a_precancelled_control_returns_cancelled() {
        let gear = 4;
        let tiers = vec![
            MeetTierInput {
                angle_deg: 90.0,
                indices: vec![0.0, 1.0, 2.0, 3.0],
                constraint: MeetConstraint::ScaleReference(1.0),
                names: vec![],
            },
            MeetTierInput {
                angle_deg: 45.0,
                indices: vec![0.5],
                constraint: MeetConstraint::MeetExisting,
                names: vec![],
            },
        ];
        let cancel = std::sync::atomic::AtomicBool::new(true);
        let control = SolveControl::with_cancel(&cancel);
        let result = solve_meet_points_with(gear, &tiers, &control);
        assert!(matches!(result, Err(SolveError::Cancelled)), "{result:?}");
    }

    /// Above [`MAX_PLANES`], `solve_meet_points_with` must return a distinct
    /// [`SolveError::TooManyPlanes`] naming the actual plane count and the
    /// cap, while the legacy `solve_meet_points` keeps its old silent
    /// behavior (every tier `SolveStrategy::Failed`, except the anchor,
    /// which keeps its given mast) -- the two must never diverge on what
    /// counts as "too many planes".
    #[test]
    fn solve_meet_points_with_reports_too_many_planes_above_the_cap() {
        let gear = 400;
        // One tier with 401 index instances -- one plane instance over
        // MAX_PLANES all by itself (`total_planes = 6 blank + 401`).
        let indices: Vec<f64> = (0..401).map(f64::from).collect();
        let tiers = vec![MeetTierInput {
            angle_deg: 90.0,
            indices,
            constraint: MeetConstraint::ScaleReference(1.0),
            names: vec![],
        }];

        let via_with = solve_meet_points_with(gear, &tiers, &SolveControl::default());
        match via_with {
            Err(SolveError::TooManyPlanes { planes, max }) => {
                assert_eq!(max, MAX_PLANES);
                assert!(planes > MAX_PLANES, "planes: {planes}");
            }
            other => panic!("expected SolveError::TooManyPlanes, got {other:?}"),
        }

        let legacy = solve_meet_points(gear, &tiers);
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0].strategy, SolveStrategy::ScaleReference);
        assert!((legacy[0].mast - 1.0).abs() < 1e-12);
    }

    /// The progress callback must report [`SolvePhase::Constructive`] before
    /// [`SolvePhase::LeastSquares`] before [`SolvePhase::Refine`] (the fixed
    /// phase order the module docs describe), `sweep` must never decrease
    /// within one phase's own run of reports, and every phase must reach
    /// `blocks_done == blocks_total` at least once (each phase fully
    /// processes every tier, or reports zero-work-to-do, before moving on).
    #[test]
    fn solve_meet_points_with_reports_monotonic_progress_reaching_every_phases_total() {
        let schedule = indicatrix_formats::asc::parse_asc(
            "GemCad 5.0\n\
             g 96 0.0\n\
             y 6 y\n\
             I 1.72\n\
             H PC 45.149  Round Trichecker-12\n\
             a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
             a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
             a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
             a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
             a 10.000000 0.48799664 96 n C 16 32 48 64 80\n",
        )
        .expect("must parse");
        let mut tiers = super::super::meet_tier_inputs_from_asc(&schedule);
        tiers[0].constraint = MeetConstraint::ScaleReference(schedule.tiers[0].mast);
        tiers[1].constraint = MeetConstraint::ScaleReference(schedule.tiers[1].mast);
        tiers[2].constraint = MeetConstraint::ScaleReference(schedule.tiers[2].mast);

        let reports: std::cell::RefCell<Vec<SolveProgress>> = std::cell::RefCell::new(Vec::new());
        let record = |p: SolveProgress| reports.borrow_mut().push(p);
        let control = SolveControl::default().reporting(&record);
        let _ = solve_meet_points_with(schedule.gear_teeth_abs(), &tiers, &control)
            .expect("under MAX_PLANES, never cancelled");

        let reports = reports.into_inner();
        assert!(!reports.is_empty(), "no progress reported at all");

        // Fixed phase order, no phase revisited once left.
        let phase_rank = |p: SolvePhase| match p {
            SolvePhase::Constructive => 0,
            SolvePhase::LeastSquares => 1,
            SolvePhase::Refine => 2,
        };
        let mut last_rank = 0;
        let mut last_sweep_in_phase = 0u32;
        let mut reached_total: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
        for r in &reports {
            let rank = phase_rank(r.phase);
            assert!(
                rank >= last_rank,
                "phase went backwards: {:?} after rank {last_rank}",
                r.phase
            );
            if rank != last_rank {
                last_sweep_in_phase = 0;
            }
            assert!(
                r.sweep >= last_sweep_in_phase,
                "sweep decreased within {:?}: {} after {last_sweep_in_phase}",
                r.phase,
                r.sweep
            );
            last_sweep_in_phase = r.sweep;
            last_rank = rank;
            assert!(
                r.blocks_done <= r.blocks_total,
                "blocks_done exceeded blocks_total: {r:?}"
            );
            assert_eq!(r.blocks_total, tiers.len() as u32);
            if r.blocks_done == r.blocks_total {
                reached_total.insert(rank);
            }
        }
        assert!(
            reached_total.contains(&0),
            "Constructive phase never reported blocks_done == blocks_total: {reports:?}"
        );
        assert!(
            reached_total.contains(&2),
            "Refine phase never reported blocks_done == blocks_total: {reports:?}"
        );
    }
}
