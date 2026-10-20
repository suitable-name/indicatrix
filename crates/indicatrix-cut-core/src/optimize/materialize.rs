//! One materialization path for preview and Apply, including relation evaluation.
use super::{options::MastChange, search::AngleChange};
use crate::{
    design::Design,
    edit::{Edit, EditError},
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy},
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// Detailed materialization failure. Legacy `EditError` adapters retain their old signature.
#[derive(Debug, Clone)]
pub enum MaterializeError {
    /// An editable parameter was stale or invalid.
    Edit(EditError),
    /// A geometry failure with its original diagnostic.
    Preservation(crate::meet_preservation::PreservationViolation),
}
impl MaterializeError {
    /// Compatibility index for older editors; geometric failures have no single tier.
    #[must_use]
    pub const fn index(&self) -> usize {
        match self {
            Self::Edit(e) => e.index,
            Self::Preservation(_) => 0,
        }
    }
}
impl std::fmt::Display for MaterializeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Edit(e) => write!(f, "{e}"),
            Self::Preservation(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for MaterializeError {}
impl From<MaterializeError> for EditError {
    fn from(error: MaterializeError) -> Self {
        match error {
            MaterializeError::Edit(e) => e,
            MaterializeError::Preservation(_) => Self {
                index: 0,
                tier_count: 0,
            },
        }
    }
}

/// Apply all editable parameters on a scratch design and validate its real solve.
/// Returns exactly the batch to commit through History; no hidden solved-mast state.
///
/// # Errors
/// [`MaterializeError::Edit`] for a stale or invalid parameter, and
/// [`MaterializeError::Preservation`] when the baseline is stale or the geometry gate refuses.
pub fn materialize_preserved_changes(
    design: &Design,
    angles: &[AngleChange],
    masts: &[MastChange],
    baseline: Option<&MeetPreservationBaseline>,
    policy: PreservationPolicy,
) -> Result<(Design, Edit, usize), MaterializeError> {
    if let Some(baseline) = baseline
        && (baseline.original() != design || !baseline.original().tier_ids_eq(design))
    {
        return Err(MaterializeError::Preservation(
            crate::meet_preservation::PreservationViolation::StaleCandidate,
        ));
    }
    materialize_from_source(design, angles, masts, baseline, policy)
}

/// Caller has separately authenticated the parameter source. A Retarget seed may
/// differ from the immutable pre-Shift network, which remains the geometry gate.
pub(super) fn materialize_from_source(
    design: &Design,
    angles: &[AngleChange],
    masts: &[MastChange],
    baseline: Option<&MeetPreservationBaseline>,
    policy: PreservationPolicy,
) -> Result<(Design, Edit, usize), MaterializeError> {
    preflight(design, angles, masts)?;
    let error = |index| {
        MaterializeError::Edit(EditError {
            index,
            tier_count: design.tiers.len(),
        })
    };
    let captured;
    let baseline = if let Some(baseline) = baseline {
        baseline
    } else {
        captured =
            MeetPreservationBaseline::capture(design).map_err(MaterializeError::Preservation)?;
        &captured
    };
    let mut trial = design.clone();
    for change in angles {
        if design.is_tier_driven(change.index) {
            return Err(error(change.index));
        }
        let tier = trial
            .tiers
            .get_mut(change.index)
            .ok_or_else(|| error(change.index))?;
        if tier.angle_deg.to_bits() != change.from_deg.to_bits() {
            return Err(error(change.index));
        }
        tier.angle_deg = change.to_deg;
    }
    for change in masts {
        if design.tier_target(change.index).is_some() {
            return Err(error(change.index));
        }
        let tier = trial
            .tiers
            .get_mut(change.index)
            .ok_or_else(|| error(change.index))?;
        let MeetConstraint::ScaleReference(mast) = tier.constraint else {
            return Err(error(change.index));
        };
        if mast.to_bits() != change.from_mast.to_bits() {
            return Err(error(change.index));
        }
        tier.constraint = MeetConstraint::ScaleReference(change.to_mast);
    }
    let relations = trial.evaluate_relations().map_err(|_| error(0))?;
    for (index, angle) in relations {
        trial
            .tiers
            .get_mut(index)
            .ok_or_else(|| error(index))?
            .angle_deg = angle;
    }
    baseline
        .validate(&trial, policy)
        .map_err(MaterializeError::Preservation)?;
    let edits: Vec<_> = trial
        .tiers
        .iter()
        .zip(&design.tiers)
        .enumerate()
        .filter(|(_, (now, before))| now != before)
        .map(|(index, (tier, _))| Edit::ModifyTier {
            index,
            tier: tier.clone(),
        })
        .collect();
    let count = edits.len();
    Ok((trial, Edit::Batch(edits), count))
}

/// Preserve precise parameter diagnostics before checking operation identity.
pub(super) fn preflight(
    design: &Design,
    angles: &[AngleChange],
    masts: &[MastChange],
) -> Result<(), MaterializeError> {
    let error = |index| {
        MaterializeError::Edit(EditError {
            index,
            tier_count: design.tiers.len(),
        })
    };
    for change in angles {
        let tier = design
            .tiers
            .get(change.index)
            .ok_or_else(|| error(change.index))?;
        if tier.angle_deg.to_bits() != change.from_deg.to_bits() || !change.to_deg.is_finite() {
            return Err(error(change.index));
        }
        if design.is_tier_driven(change.index) {
            return Err(MaterializeError::Preservation(
                crate::meet_preservation::PreservationViolation::Relation(format!(
                    "Tier {} follows a relation and cannot be changed directly",
                    tier.name
                )),
            ));
        }
    }
    for change in masts {
        let tier = design
            .tiers
            .get(change.index)
            .ok_or_else(|| error(change.index))?;
        let MeetConstraint::ScaleReference(mast) = tier.constraint else {
            return Err(error(change.index));
        };
        if mast.to_bits() != change.from_mast.to_bits()
            || !change.to_mast.is_finite()
            || change.to_mast <= 0.0
            || design.tier_target(change.index).is_some()
        {
            return Err(error(change.index));
        }
    }
    Ok(())
}
