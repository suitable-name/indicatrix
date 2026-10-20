//! Branch diagnosis of a refused candidate (follow-up plan 2026-10-19, section 4.3).
//!
//! A meet-derived tier (`MeetExisting` / `MeetNamed`) has no projection variable: its mast is
//! whatever the authoring solve picks, and after the shell moves that pick can flip to another
//! vertex while a vertex that reproduces the original meet still exists. The gate then reports
//! a detached junction and nothing says whether the stone is infeasible or merely unnameable.
//! This module tells the two apart. It is pure measurement: it never changes a verdict and no
//! product path calls it.
//!
//! The test is the "ideal mast". For every original junction that a meet-derived tier passes
//! through, the junction is rebuilt from the three best-conditioned planes of its support that
//! the diagnosed tiers do not own (other tiers and the preform), and the tier's mast is read
//! from that point. If pinning every such tier at its ideal mast makes the candidate pass the
//! gate under the wide census policy, the authored constraints picked another vertex
//! (a branch mismatch); if it still fails, the stone itself cannot keep the meets.
use super::{MeetPreservationBaseline, provenance, topology};
use crate::{
    design::Design,
    meet_preservation::{
        Plane, PreservationPolicy, PreservationViolation, solve_checked_controlled,
    },
};
use glam::DVec3;
use indicatrix::geometry::meet_solver::{MeetConstraint, SolveControl};

/// Why the gate refused a candidate, as far as the meet network can tell.
#[derive(Debug, Clone, PartialEq)]
pub enum BranchDiagnosis {
    /// Substituting every meet-derived tier's ideal mast reproduces the original network:
    /// the authored constraints picked another vertex. `tiers` are the tier indices whose
    /// solved mast differs from the ideal by more than the preservation tolerance, and
    /// `ideal_spread` the largest disagreement between the ideal masts one tier would need at
    /// its several junctions, in units of the stone's extent.
    BranchMismatch {
        tiers: Vec<usize>,
        ideal_spread: f64,
    },
    /// Even the ideal masts do not reproduce the network (or there is no meet-derived tier to
    /// substitute): a real infeasibility. `reason` is the gate's refusal of that state.
    Infeasible { reason: PreservationViolation },
    /// The ideal masts cannot be formed or the question does not apply: a meet-derived tier
    /// no junction fixes, a candidate with other surfaces than the original, a failed solve,
    /// a cancellation, or a candidate whose meets are intact and that only a shape bound
    /// refuses.
    Unresolved { reason: String },
}

/// The point where three planes meet, `None` for a rank-deficient or non-finite triple.
fn intersection(a: Plane, b: Plane, c: Plane) -> Option<DVec3> {
    let det = a.0.dot(b.0.cross(c.0));
    if det.abs() <= 1e-8 {
        return None;
    }
    let point = (b.0.cross(c.0) * a.1 + c.0.cross(a.0) * b.1 + a.0.cross(b.0) * c.1) / det;
    point.is_finite().then_some(point)
}

fn unresolved(reason: impl Into<String>) -> BranchDiagnosis {
    BranchDiagnosis::Unresolved {
        reason: reason.into(),
    }
}

/// What the diagnosis reads of the candidate's solved geometry.
struct Geometry {
    /// The candidate's planes, one per surface of the baseline.
    planes: Vec<Plane>,
    /// The tier index each surface belongs to; the preform's have none.
    tier_of: Vec<Option<usize>>,
    /// Whether each tier is meet-derived and diagnosed.
    derived: Vec<bool>,
}

impl Geometry {
    /// The diagnosed tier a surface belongs to.
    fn derived_tier(&self, surface: usize) -> Option<usize> {
        self.tier_of[surface].filter(|&tier| self.derived[tier])
    }
}

impl MeetPreservationBaseline {
    /// Classify the refusal of `candidate`: a branch mismatch, a real infeasibility, or neither
    /// (see [`BranchDiagnosis`]). For a candidate the gate refused; one it accepts comes back
    /// as [`BranchDiagnosis::Unresolved`].
    ///
    /// Costs a few authoring solves and gate evaluations; it is a census tool, never a step of
    /// the search.
    #[must_use]
    pub fn diagnose_refusal(
        &self,
        candidate: &Design,
        control: &SolveControl<'_>,
    ) -> BranchDiagnosis {
        if control.is_cancelled() {
            return unresolved("the diagnosis was cancelled");
        }
        let (geometry, masts) = match self.candidate_geometry(candidate, control) {
            Ok(read) => read,
            Err(reason) => return unresolved(reason),
        };
        if !geometry.derived.contains(&true) {
            return self.refused_otherwise(candidate, control);
        }
        let ideals = match self.ideal_masts(&geometry, candidate) {
            Ok(ideals) => ideals,
            Err(reason) => return unresolved(reason),
        };

        let mut ideal_design = candidate.clone();
        let mut mismatched = Vec::new();
        let mut spread: f64 = 0.0;
        for (tier, values) in ideals.iter().enumerate().filter(|(_, v)| !v.is_empty()) {
            let mean = values.iter().sum::<f64>() / values.len() as f64;
            let low = values.iter().copied().fold(f64::INFINITY, f64::min);
            let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            spread = spread.max((high - low) / self.scale);
            ideal_design.tiers[tier].constraint =
                MeetConstraint::ScaleReference(mean.copysign(masts[tier]));
            if (masts[tier].abs() - mean).abs() > self.tolerance {
                mismatched.push(tier);
            }
        }
        match self.validate_geometry(&ideal_design, PreservationPolicy::CENSUS, control, false) {
            Ok(_) if mismatched.is_empty() => self.refused_otherwise(candidate, control),
            Ok(_) => BranchDiagnosis::BranchMismatch {
                tiers: mismatched,
                ideal_spread: spread,
            },
            Err(PreservationViolation::Cancelled) => unresolved("the diagnosis was cancelled"),
            Err(reason) => BranchDiagnosis::Infeasible {
                reason: self.annotate(reason),
            },
        }
    }

    /// Solve `candidate` and read its surfaces, tier correspondence and meet-derived tiers,
    /// with the solved mast of every tier. The error is the reason the question has no answer.
    fn candidate_geometry(
        &self,
        candidate: &Design,
        control: &SolveControl<'_>,
    ) -> Result<(Geometry, Vec<f64>), String> {
        if candidate.tiers.len() != self.original.tiers.len() {
            return Err("the candidate has another number of tiers than the original".into());
        }
        let solved =
            solve_checked_controlled(candidate, control).map_err(|e| ToString::to_string(&e))?;
        let planes = candidate.planes_from_solved(&solved);
        let surfaces = provenance::surfaces(candidate, &solved, &planes)
            .map_err(|e| ToString::to_string(&e))?;
        if surfaces.iter().map(|(key, _)| key).ne(self.keys.iter()) {
            return Err("the candidate has other surfaces than the original".into());
        }
        let planes: Vec<Plane> = surfaces.iter().map(|(_, plane)| *plane).collect();
        if planes.iter().any(|(n, d)| !n.is_finite() || !d.is_finite()) {
            return Err("a candidate plane is not finite".into());
        }
        let tier_of = self
            .keys
            .iter()
            .map(|key| match key {
                provenance::SurfaceKey::Preform(_) => None,
                provenance::SurfaceKey::Facet(id, _) => (0..self.original.tiers.len())
                    .find(|&tier| self.original.tier_id_at_or_synthetic(tier) == *id),
            })
            .collect();
        let derived = candidate
            .tiers
            .iter()
            .enumerate()
            .map(|(index, tier)| {
                matches!(
                    tier.constraint,
                    MeetConstraint::MeetExisting | MeetConstraint::MeetNamed(_)
                ) && candidate.tier_target(index).is_none()
            })
            .collect();
        let masts = solved.iter().map(|tier| tier.mast).collect();
        Ok((
            Geometry {
                planes,
                tier_of,
                derived,
            },
            masts,
        ))
    }

    /// Per tier, the ideal masts it would need at each original junction it passes through
    /// (empty for a tier that is not diagnosed or touches no junction).
    ///
    /// The error names a diagnosed tier that passes through junctions but is fixed at none of
    /// them by three planes of other tiers.
    fn ideal_masts(
        &self,
        geometry: &Geometry,
        candidate: &Design,
    ) -> Result<Vec<Vec<f64>>, String> {
        let count = geometry.derived.len();
        let mut ideals: Vec<Vec<f64>> = vec![Vec::new(); count];
        let mut touched = vec![false; count];
        for junction in &self.junctions {
            let owned: Vec<usize> = junction
                .support
                .iter()
                .copied()
                .filter(|&surface| geometry.derived_tier(surface).is_some())
                .collect();
            for tier in owned.iter().filter_map(|&s| geometry.derived_tier(s)) {
                touched[tier] = true;
            }
            let fixed: Vec<usize> = junction
                .support
                .iter()
                .copied()
                .filter(|&surface| geometry.derived_tier(surface).is_none())
                .collect();
            if owned.is_empty() {
                continue;
            }
            let planes = &geometry.planes;
            let Some([a, b, c]) = topology::best_basis(planes, &fixed) else {
                continue;
            };
            let Some(point) = intersection(planes[a], planes[b], planes[c]) else {
                continue;
            };
            for &surface in &owned {
                if let Some(tier) = geometry.derived_tier(surface) {
                    ideals[tier].push(planes[surface].0.dot(point));
                }
            }
        }
        if let Some(tier) = (0..count).find(|&t| touched[t] && ideals[t].is_empty()) {
            return Err(format!(
                "no junction of tier {} ({}) is fixed by three planes of other tiers",
                tier + 1,
                candidate.tiers[tier].name
            ));
        }
        Ok(ideals)
    }

    /// The diagnosis when no ideal mast explains the refusal: the gate's own verdict on the
    /// candidate under the census policy, or `Unresolved` when only a shape bound refuses it.
    fn refused_otherwise(&self, candidate: &Design, control: &SolveControl<'_>) -> BranchDiagnosis {
        match self.validate_geometry(candidate, PreservationPolicy::CENSUS, control, false) {
            Err(PreservationViolation::Cancelled) => unresolved("the diagnosis was cancelled"),
            Err(reason) => BranchDiagnosis::Infeasible {
                reason: self.annotate(reason),
            },
            Ok(_) => {
                unresolved("the candidate keeps every original meet; only a shape bound refuses it")
            }
        }
    }
}
