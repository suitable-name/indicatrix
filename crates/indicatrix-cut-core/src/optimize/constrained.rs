//! Bounded incidence and containment projection through the real authoring solver.
//! Derived masts are never stored or pinned: each residual uses `Design::solve_with`.
use super::space::{SearchSpace, settle_relations};
use crate::{
    design::Design,
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy, PreservationViolation},
};
use indicatrix::geometry::meet_solver::{MeetConstraint, SolveControl};
use std::sync::{Arc, atomic::AtomicBool};

#[path = "projection_step.rs"]
mod projection_step;

/// Project a preferred deformation without scoring it. Shift and Optimize share this path.
///
/// # Errors
/// Returns the [`PreservationViolation`] that stopped the baseline capture or the
/// projection (cancelled, stale proposal, or no feasible state within the trust region).
pub fn project_preserved_design(
    original: &Design,
    proposed: &Design,
    options: &super::options::OptimizeOptions,
    cancel: Option<&AtomicBool>,
) -> Result<Design, crate::meet_preservation::PreservationViolation> {
    project_preserved_design_controlled(
        original,
        proposed,
        options,
        &cancel.map_or_else(SolveControl::default, SolveControl::with_cancel),
    )
}

/// Projection with solver-level cancellation for closure-based Retarget workers.
///
/// # Errors
/// As [`project_preserved_design`].
pub fn project_preserved_design_controlled(
    original: &Design,
    proposed: &Design,
    options: &super::options::OptimizeOptions,
    control: &SolveControl<'_>,
) -> Result<Design, crate::meet_preservation::PreservationViolation> {
    project_traced(original, proposed, options, control, &mut Trace::default())
}

/// A projection together with what the plain entry points drop (follow-up plan 2026-10-19,
/// section 4.3).
#[derive(Debug, Clone)]
pub struct ProjectionReport {
    /// Exactly what [`project_preserved_design_controlled`] returns for the same inputs.
    pub result: Result<Design, PreservationViolation>,
    /// The last candidate the gate refused while the projection ran: the seed, a line-search
    /// probe or the final iterate, whichever was assessed last. `None` when the projection
    /// accepted a design, stopped before any assessment (a stale proposal, a failed capture,
    /// a cancellation) or never formed a candidate.
    pub last_probe: Option<Design>,
    /// Damped least-squares corrections started, at most twelve.
    pub iterations: u64,
}

/// [`project_preserved_design_controlled`] that also reports the last refused probe and the
/// iteration count.
///
/// A caller can use them to diagnose a refusal (`BranchDiagnosis`). The projection itself
/// is the same code: `result` is bit-identical to the plain entry point's.
#[must_use]
pub fn project_preserved_design_report(
    original: &Design,
    proposed: &Design,
    options: &super::options::OptimizeOptions,
    control: &SolveControl<'_>,
) -> ProjectionReport {
    let mut trace = Trace {
        record_probes: true,
        ..Trace::default()
    };
    let result = project_traced(original, proposed, options, control, &mut trace);
    ProjectionReport {
        result,
        last_probe: trace.last_probe,
        iterations: trace.iterations,
    }
}

/// What a projection observed on its way, beyond its result.
#[derive(Debug, Default)]
struct Trace {
    /// Whether refused candidates are kept (they are cloned, which the plain entry points skip).
    record_probes: bool,
    last_probe: Option<Design>,
    iterations: u64,
}

impl Trace {
    /// Note a candidate the gate has just refused.
    fn refused(&mut self, candidate: &Design) {
        if self.record_probes {
            self.last_probe = Some(candidate.clone());
        }
    }
}

fn project_traced(
    original: &Design,
    proposed: &Design,
    options: &super::options::OptimizeOptions,
    control: &SolveControl<'_>,
    trace: &mut Trace,
) -> Result<Design, PreservationViolation> {
    let baseline = match &options.preservation_baseline {
        Some(baseline) => baseline.clone(),
        None => Arc::new(MeetPreservationBaseline::capture_controlled(
            original, control,
        )?),
    };
    let solved = crate::meet_preservation::solve_checked_controlled(original, control)?;
    let planes = original.planes_from_solved(&solved);
    let free = super::candidate::free_tier_indices_with(original, options);
    let anchors = super::space::measure_anchors(original, options, &free, &solved, &planes);
    let feasible = FeasibleSpace {
        baseline,
        free,
        masts: movable_masts(original, options),
        policy: options.preservation_policy,
        turn_request: options.turn_request.clone(),
    };
    let space = SearchSpace::new(options, anchors);
    feasible.project_controlled(proposed, &space, control, trace)
}

/// Depth freedom is independent of angle freedom: flat tiers and relation followers
/// can need coordinated depths, but explicit dimensional targets and unselected rows
/// remain locked. Vertical girdle depths keep the authored outline fixed.
pub(super) fn movable_masts(
    design: &Design,
    options: &super::options::OptimizeOptions,
) -> Vec<usize> {
    (0..design.tiers.len())
        .filter(|&i| {
            options.vary_anchored
                && design.tier_target(i).is_none()
                && (options.anchored_hinge(design, i).is_some()
                    || (design.tiers[i].angle_deg.abs() < 1e-8
                        && matches!(
                            design.tiers[i].constraint,
                            MeetConstraint::ScaleReference(_)
                        )))
                && options
                    .only_tiers
                    .as_ref()
                    .is_none_or(|set| set.contains(&i))
                && design.tiers[i].angle_deg.abs() < 89.999
        })
        .collect()
}

#[derive(Debug, Clone)]
pub(super) struct FeasibleSpace {
    pub(super) baseline: Arc<MeetPreservationBaseline>,
    pub(super) free: Vec<usize>,
    pub(super) masts: Vec<usize>,
    pub(super) policy: PreservationPolicy,
    /// The request's per-tier tangent change every assessment is judged by
    /// ([`super::options::OptimizeOptions::turn_request`]); `None` reads it from the seed.
    pub(super) turn_request: Option<Arc<[f64]>>,
}

impl FeasibleSpace {
    /// Damped least-squares corrections reduce the nonlinear meet residuals.
    /// Only the full authored-geometry validator can certify a feasible result.
    pub(super) fn project(
        &self,
        proposed: &Design,
        space: &SearchSpace,
        cancel: Option<&AtomicBool>,
    ) -> Option<Design> {
        self.project_controlled(
            proposed,
            space,
            &cancel.map_or_else(SolveControl::default, SolveControl::with_cancel),
            &mut Trace::default(),
        )
        .ok()
    }

    fn project_controlled(
        &self,
        proposed: &Design,
        space: &SearchSpace,
        control: &SolveControl<'_>,
        trace: &mut Trace,
    ) -> Result<Design, PreservationViolation> {
        let _projection = self.baseline.telemetry.phase(1);
        let mut reason = PreservationViolation::Unrepresentable;
        let mut stage = "authoring and parameter checks";
        let result = self.project_inner(proposed, space, control, &mut reason, &mut stage, trace);
        if result.is_none() {
            self.baseline.telemetry.rejected();
        }
        result.ok_or_else(|| {
            if control.is_cancelled() {
                PreservationViolation::Cancelled
            } else if matches!(reason, PreservationViolation::StaleCandidate) {
                reason
            } else {
                PreservationViolation::ProjectionFailed {
                    stage: stage.into(),
                    reason: reason.to_string(),
                }
            }
        })
    }

    /// The turn the request asks for: the caller's (`OptimizeOptions::turn_request`, the
    /// anchored candidate of a Retarget, which also covers the refit projected after it)
    /// or, without one, read once from the seed. A policy that follows the turn is
    /// widened by this and only this, so no probe is judged by an allowance its own
    /// repair enlarged (`PreservationPolicy::bounds_follow_turn`).
    fn requested_turn(&self, proposed: &Design) -> std::borrow::Cow<'_, [f64]> {
        self.turn_request
            .as_deref()
            .filter(|request| request.len() == proposed.tiers.len())
            .map_or_else(
                || {
                    std::borrow::Cow::Owned(crate::meet_preservation::tangent_changes(
                        self.baseline.original(),
                        proposed,
                    ))
                },
                std::borrow::Cow::Borrowed,
            )
    }

    fn project_inner(
        &self,
        proposed: &Design,
        space: &SearchSpace,
        control: &SolveControl<'_>,
        reason: &mut PreservationViolation,
        stage: &mut &'static str,
        trace: &mut Trace,
    ) -> Option<Design> {
        let cancelled = || control.is_cancelled();
        if cancelled() {
            return None;
        }
        if !self.baseline.same_authoring(proposed) {
            *reason = PreservationViolation::StaleCandidate;
            return None;
        }
        let seed = self.seeded(proposed)?;
        let Some(origin) = self.parameters(&seed) else {
            // A proposal that adopted a pinned depth back to a meet has no mast to move.
            *reason = PreservationViolation::StaleCandidate;
            return None;
        };
        // The already-feasible shortcut must obey the same parameter bounds as projection.
        // In particular, Shift may suggest an angle outside the requested search range.
        let seed = self.materialize(&seed, &origin, space)?;
        let proposed = &seed;
        let turn = self.requested_turn(proposed);
        let turn: &[f64] = &turn;
        // One solve answers both questions: is the seed already feasible and, if it is
        // not, where do its junctions sit. The same holds for every probe below.
        let first = self
            .baseline
            .assess(proposed, self.policy, control, Some(turn));
        let mut residual = match first.verdict {
            Ok(_) => return Some(proposed.clone()),
            Err(error) => {
                trace.refused(proposed);
                *reason = error;
                first.residuals
            }
        };
        let mut x = origin.clone();
        // Degrees and normalized mast units: the same physical scales bound the
        // complete projection and regularize each correction, including weak columns.
        let limits: Vec<_> = (0..x.len())
            .map(|i| if i < self.free.len() { 5.0 } else { 0.1 })
            .collect();
        for _ in 0..12 {
            self.baseline.telemetry.iteration();
            trace.iterations += 1;
            if cancelled() {
                return None;
            }
            // The residual at `x` is already known: the seed's, or the accepted line-search
            // probe's, both formed in the same solve that judged them.
            *stage = "junction residual evaluation";
            let current = residual.take()?;
            *stage = "finite-difference correction";
            let columns = self.jacobian_columns(proposed, &x, &current, space, control)?;
            let mut correction = projection_step::correction(&columns, &current, &limits)?;
            *stage = "bounded correction";
            projection_step::bound_correction(&mut correction, &x, &origin, &limits)?;
            let before: f64 = current.iter().map(|r| r * r).sum();
            let mut accepted = None;
            *stage = "line search (no improving bounded correction)";
            for factor in [1.0, 0.5, 0.25, 0.125] {
                let next: Vec<_> = x
                    .iter()
                    .zip(&correction)
                    .map(|(v, dx)| v + factor * dx)
                    .collect();
                // Explicit local trust region: angles <= 5 degrees, masts <= 10% of unit.
                if next
                    .iter()
                    .zip(&origin)
                    .enumerate()
                    .any(|(i, (a, b))| !a.is_finite() || (a - b).abs() > limits[i])
                {
                    continue;
                }
                let Some(probe) = self.materialize(proposed, &next, space) else {
                    continue;
                };
                let assessment = self
                    .baseline
                    .assess(&probe, self.policy, control, Some(turn));
                match assessment.verdict {
                    // Near numerical precision a valid point need not improve squared
                    // residuals further. The authoritative gate accepts it as it is.
                    Ok(_) => return Some(probe),
                    Err(error) => {
                        trace.refused(&probe);
                        let Some(r) = assessment.residuals else {
                            continue;
                        };
                        let after: f64 = r.iter().map(|r| r * r).sum();
                        if after < before {
                            accepted = Some((next, r, error));
                            break;
                        }
                    }
                }
            }
            let (next, r, error) = accepted?;
            x = next;
            residual = Some(r);
            *reason = error;
        }
        let candidate = self.materialize(proposed, &x, space)?;
        *stage = "iteration limit";
        if let Err(error) = self
            .baseline
            .assess(&candidate, self.policy, control, Some(turn))
            .verdict
        {
            trace.refused(&candidate);
            *reason = error;
            return None;
        }
        Some(candidate)
    }

    /// The proposal with every non-free, non-relation-follower angle and every
    /// non-movable depth reset to the original, with its relations settled.
    fn seeded(&self, proposed: &Design) -> Option<Design> {
        let mut seed = proposed.clone();
        for (index, tier) in seed.tiers.iter_mut().enumerate() {
            let original = &self.baseline.original().tiers[index];
            if !self.free.contains(&index)
                && !seed
                    .tier_relations
                    .contains_key(&self.baseline.original().tier_id_at_or_synthetic(index))
            {
                tier.angle_deg = original.angle_deg;
            }
            if !self.masts.contains(&index)
                && matches!(original.constraint, MeetConstraint::ScaleReference(_))
            {
                tier.constraint = original.constraint.clone();
            }
        }
        settle_relations(&mut seed).then_some(seed)
    }

    /// One finite-difference Jacobian column of the junction residuals per variable.
    /// `None` when cancelled or when a probe's residual count differs from `current`.
    fn jacobian_columns(
        &self,
        proposed: &Design,
        x: &[f64],
        current: &[f64],
        space: &SearchSpace,
        control: &SolveControl<'_>,
    ) -> Option<Vec<Vec<f64>>> {
        let mut columns = Vec::new();
        for variable in 0..x.len() {
            if control.is_cancelled() {
                return None;
            }
            // Production normals pass through f32; smaller differences lose rank.
            let h = if variable < self.free.len() {
                2e-3
            } else {
                2e-5
            };
            // At an upper bound the forward probe is invalid. Use an inward probe
            // instead of rejecting an otherwise feasible coupled direction.
            let mut derivative = None;
            for step in [h, -h] {
                let mut next = x.to_vec();
                next[variable] += step;
                let Some(probe) = self.materialize(proposed, &next, space) else {
                    continue;
                };
                let Some(r) = self.baseline.residuals(&probe, control) else {
                    continue;
                };
                if r.len() != current.len() {
                    return None;
                }
                derivative = Some(r.iter().zip(current).map(|(a, b)| (a - b) / step).collect());
                break;
            }
            // A zero-width bound is a fixed variable, not a failed projection.
            columns.push(derivative.unwrap_or_else(|| vec![0.0; current.len()]));
        }
        Some(columns)
    }

    /// The projection variables of `design`: free angles, then normalized pinned depths.
    /// `None` when a depth variable's tier is no longer a pinned mast, which only a
    /// proposal built against another authoring state can ask for.
    fn parameters(&self, design: &Design) -> Option<Vec<f64>> {
        let mut values: Vec<f64> = self
            .free
            .iter()
            .map(|&i| design.tiers[i].angle_deg)
            .collect();
        for &i in &self.masts {
            let MeetConstraint::ScaleReference(mast) = design.tiers[i].constraint else {
                return None;
            };
            values.push(mast / self.baseline.scale());
        }
        Some(values)
    }

    fn materialize(&self, base: &Design, values: &[f64], space: &SearchSpace) -> Option<Design> {
        let mut candidate = base.clone();
        for (&index, &angle) in self.free.iter().zip(values) {
            if !space.contains(index, angle)
                || !super::candidate::candidate_angle_is_safe(
                    self.baseline.original().tiers[index].angle_deg,
                    angle,
                )
            {
                return None;
            }
            candidate.tiers[index].angle_deg = angle;
        }
        for (&index, &mast) in self.masts.iter().zip(&values[self.free.len()..]) {
            if !mast.is_finite() || mast <= 1e-9 {
                return None;
            }
            // A variable the projection did not move keeps the tier's exact mast: the
            // normalized round trip `(m / s) * s` is not always `m`, and a last-bit
            // difference would list an untouched depth as a change.
            if let MeetConstraint::ScaleReference(current) = candidate.tiers[index].constraint
                && (current / self.baseline.scale()).to_bits() == mast.to_bits()
            {
                continue;
            }
            candidate.tiers[index].constraint =
                MeetConstraint::ScaleReference(mast * self.baseline.scale());
        }
        settle_relations(&mut candidate).then_some(candidate)
    }
}
