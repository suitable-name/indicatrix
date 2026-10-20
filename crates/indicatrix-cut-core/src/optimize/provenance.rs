//! Portable operation identity: worker messages retain source, score inputs and full depths.
use super::{
    MastChange, OptimizeConfig, OptimizeOptions, OptimizeOutcome, materialize::MaterializeError,
};
use crate::{
    Design,
    meet_preservation::{PreservationPolicy, PreservationViolation, SearchDiagnostics},
};
use indicatrix::optics::materials::GemMaterial;

/// Exact native authoring snapshots, not a lossy hash or row-angle freshness check.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OptimizationProvenance {
    /// Canonical native source state and stable identities.
    pub source: String,
    /// Exact accepted authored state; detects mutation or truncation of proposed changes.
    pub candidate: String,
    /// Material, objective, lighting, search settings, locks and bounds used to score,
    /// without the execution knobs (`starts`, `max_lanes`) and an inactive shape target.
    pub scoring: String,
    /// Hard shape policy used during the operation.
    pub policy: PreservationPolicy,
    /// Full depth changes carried even by the legacy outcome interface.
    pub masts: Vec<(usize, f64, f64)>,
    /// Observational measurements, excluded from logical equality and from the wire:
    /// timings differ between two identical runs, so serialising them would make
    /// identical results differ byte for byte. A deserialised value reads as zeros.
    #[serde(skip)]
    pub diagnostics: SearchDiagnostics,
    /// Geometry and physical accuracy facts for the actual accepted state.
    pub preservation: crate::meet_preservation::PreservationFacts,
}
impl PartialEq for OptimizationProvenance {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
            && self.candidate == other.candidate
            && self.scoring == other.scoring
            && self.policy == other.policy
            && self.masts == other.masts
    }
}
/// Canonical JSON sorts map keys; explicit raw IDs also distinguish legacy empty-ID inputs.
///
/// # Errors
/// [`PreservationViolation::InvalidBaseline`] when the design cannot be serialised.
pub fn authoring_identity(design: &Design) -> Result<String, PreservationViolation> {
    let file = crate::native::design_to_file(design, None, &crate::native::DesignExtras::default());
    let value = serde_json::to_value(file)
        .map_err(|e| PreservationViolation::InvalidBaseline(e.to_string()))?;
    Ok(format!("{}|{:?}", value, design.tier_ids))
}
/// What a result was scored under. Two runs that are the same search read the same.
///
/// Only the search itself takes part. `max_lanes` is a pure execution knob (the result never
/// depends on it), and `starts` is a request the budget may cut back (see
/// [`super::effective_starts`]); what ran is reported by the result's own `starts_run` and
/// `best_start`, so a run that asked for more starts than it could afford reads like the
/// single search it was. A shape target that cannot change a score counts as none.
pub(super) fn scoring_identity(
    material: &GemMaterial,
    config: &OptimizeConfig,
    options: &OptimizeOptions,
) -> String {
    let config = OptimizeConfig {
        starts: 1,
        max_lanes: 0,
        ..*config
    };
    format!(
        "{material:?}|{config:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{}",
        options.only_tiers,
        options.angle_bounds,
        options.anchor_hinges,
        options.active_shape_target(),
        options.preservation_policy,
        options.vary_anchored
    )
}
impl OptimizationProvenance {
    #[expect(
        clippy::too_many_arguments,
        reason = "internal constructor whose inputs are the operation's independent identity parts; its two callers each hold them as separate values"
    )]
    pub(super) fn capture(
        original: &Design,
        candidate: &Design,
        material: &GemMaterial,
        config: &OptimizeConfig,
        options: &OptimizeOptions,
        masts: &[MastChange],
        diagnostics: SearchDiagnostics,
        preservation: crate::meet_preservation::PreservationFacts,
    ) -> Result<Self, PreservationViolation> {
        Ok(Self {
            source: authoring_identity(original)?,
            candidate: authoring_identity(candidate)?,
            scoring: scoring_identity(material, config, options),
            policy: options.preservation_policy,
            masts: masts
                .iter()
                .map(|m| (m.index, m.from_mast, m.to_mast))
                .collect(),
            diagnostics,
            preservation,
        })
    }
    /// Front ends can reject a score after changing its material, lighting or objective.
    ///
    /// The execution knobs are not part of the key: a result matches a `config` that differs
    /// only in `starts` or `max_lanes` (see `scoring_identity`). A caller that wants a
    /// multi-start result invalidated when the start count changes compares
    /// [`super::OptimizeResult::starts_run`] with [`super::effective_starts`] itself.
    #[must_use]
    pub fn matches_scoring(
        &self,
        material: &GemMaterial,
        config: &OptimizeConfig,
        options: &OptimizeOptions,
    ) -> bool {
        self.scoring == scoring_identity(material, config, options)
    }
    /// No original input, including an untouched tier or material, may silently drift.
    ///
    /// # Errors
    /// [`PreservationViolation::StaleCandidate`] when `design` no longer matches the
    /// captured source, or an invalid-baseline violation when it cannot be serialised.
    pub fn check_source(&self, design: &Design) -> Result<(), PreservationViolation> {
        if authoring_identity(design)? != self.source {
            return Err(PreservationViolation::StaleCandidate);
        }
        Ok(())
    }
}
/// Shared legacy-outcome materialization; missing operation identity fails closed.
///
/// # Errors
/// [`MaterializeError`] when the outcome carries no provenance, the design or the
/// materialized state no longer matches it, or the geometry gate refuses.
pub fn materialize_outcome(
    design: &Design,
    outcome: &OptimizeOutcome,
) -> Result<(Design, crate::Edit, usize), MaterializeError> {
    super::materialize::preflight(design, &outcome.changes, &[])?;
    let provenance = outcome
        .provenance
        .as_ref()
        .ok_or(MaterializeError::Preservation(
            PreservationViolation::StaleCandidate,
        ))?;
    provenance
        .check_source(design)
        .map_err(MaterializeError::Preservation)?;
    let masts: Vec<_> = provenance
        .masts
        .iter()
        .map(|&(index, from_mast, to_mast)| MastChange {
            index,
            from_mast,
            to_mast,
        })
        .collect();
    let result = super::materialize::materialize_preserved_changes(
        design,
        &outcome.changes,
        &masts,
        None,
        provenance.policy,
    )?;
    if authoring_identity(&result.0).map_err(MaterializeError::Preservation)?
        != provenance.candidate
    {
        return Err(MaterializeError::Preservation(
            PreservationViolation::StaleCandidate,
        ));
    }
    Ok(result)
}

/// Stamp only after final scoring; preserve the source the public changes are relative to.
///
/// Every candidate already passed the gate once (`score_state`); the rebuild from its
/// `changes` and `mast_changes` here must reproduce that state. A candidate that no
/// longer validates is dropped on its own, and the result's best entry (`outcome`,
/// `mast_changes`) is rebuilt from the first survivor, so one failing rival never
/// discards a valid winner or turns a finished search into a solver error.
pub(super) fn stamp_result(
    original: &Design,
    material: &GemMaterial,
    config: &OptimizeConfig,
    options: &OptimizeOptions,
    baseline: &crate::meet_preservation::MeetPreservationBaseline,
    result: &mut super::OptimizeResult,
) -> Result<(), crate::DesignSolveError> {
    let mut stamped = Vec::with_capacity(result.candidates.len());
    for candidate in &result.candidates {
        // Changes refer to the search start; preservation refers to the immutable
        // operation original, which can precede a Retarget Shift seed.
        let mut state = original.clone();
        for change in &candidate.changes {
            state.tiers[change.index].angle_deg = change.to_deg;
        }
        for change in &candidate.mast_changes {
            state.tiers[change.index].constraint =
                indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(change.to_mast);
        }
        if !super::space::settle_relations(&mut state) {
            stamped.push(None);
            continue;
        }
        let Ok(facts) = baseline.validate(&state, options.preservation_policy) else {
            stamped.push(None);
            continue;
        };
        stamped.push(Some(
            OptimizationProvenance::capture(
                original,
                &state,
                material,
                config,
                options,
                &candidate.mast_changes,
                baseline.diagnostics(),
                facts,
            )
            .map_err(crate::DesignSolveError::Preservation)?,
        ));
    }
    let dropped = stamped.iter().any(Option::is_none);
    let mut stamped = stamped.into_iter();
    result
        .candidates
        .retain_mut(|candidate| match stamped.next().flatten() {
            Some(provenance) => {
                candidate.provenance = Some(provenance);
                true
            }
            None => false,
        });
    if dropped {
        rebuild_best(result);
    }
    result.outcome.provenance = match result.candidates.first() {
        Some(candidate) => candidate.provenance.clone(),
        None => Some(
            OptimizationProvenance::capture(
                original,
                original,
                material,
                config,
                options,
                &[],
                baseline.diagnostics(),
                baseline
                    .validate(original, options.preservation_policy)
                    .map_err(crate::DesignSolveError::Preservation)?,
            )
            .map_err(crate::DesignSolveError::Preservation)?,
        ),
    };
    Ok(())
}

/// Make `outcome` and `mast_changes` describe the first remaining candidate again, or
/// report no change when none remains.
fn rebuild_best(result: &mut super::OptimizeResult) {
    if let Some(best) = result.candidates.first() {
        result.outcome.after = best.after;
        result.outcome.after_score = best.score;
        result.outcome.after_yield_loss_pct = best.yield_loss_pct;
        result.outcome.changes.clone_from(&best.changes);
        result.mast_changes.clone_from(&best.mast_changes);
    } else {
        result.outcome.after = result.outcome.before;
        result.outcome.after_score = result.outcome.before_score;
        result.outcome.after_yield_loss_pct = result.outcome.before_yield_loss_pct;
        result.outcome.changes.clear();
        result.outcome.polish_improvement = 0.0;
        result.mast_changes.clear();
    }
}

/// Shared rich-candidate gate, including its original policy and exact accepted state.
///
/// # Errors
/// As [`materialize_outcome`], for a candidate's own provenance.
pub fn materialize_candidate(
    design: &Design,
    candidate: &super::OptimizeCandidate,
) -> Result<(Design, crate::Edit, usize), MaterializeError> {
    super::materialize::preflight(design, &candidate.changes, &candidate.mast_changes)?;
    let p = candidate
        .provenance
        .as_ref()
        .ok_or(MaterializeError::Preservation(
            PreservationViolation::StaleCandidate,
        ))?;
    p.check_source(design)
        .map_err(MaterializeError::Preservation)?;
    let result = super::materialize::materialize_from_source(
        design,
        &candidate.changes,
        &candidate.mast_changes,
        candidate.preservation_baseline.as_deref(),
        p.policy,
    )?;
    if authoring_identity(&result.0).map_err(MaterializeError::Preservation)? != p.candidate {
        return Err(MaterializeError::Preservation(
            PreservationViolation::StaleCandidate,
        ));
    }
    Ok(result)
}
