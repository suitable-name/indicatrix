//! The part of a retarget that needs a solved stone: re-anchoring, refitting, the validity
//! gate and the optical numbers.
//!
//! [`run_check`] is built to run on a worker thread (the editor crate has none of its own):
//! it takes everything by reference, reads a `should_stop` flag between its steps so a newer
//! request can abandon an older one, and lets the caller keep the expensive analysis of the
//! ORIGINAL stone between requests (it depends on the design, not on the plan).
//!
//! # What it does
//!
//! 1. Analyses the original stone once (solve, mesh, girdle and table figures, which facets
//!    are alive, the girdle corners of every flat crown or pavilion tier) and captures its
//!    meet network ([`MeetPreservationBaseline`]).
//! 2. Seeds the candidate with every moving, pinned tier re-anchored to its edge
//!    ([`anchored_candidate_for`]), PROJECTS that seed against the original network (the
//!    masts and angles are adjusted until every original junction is met again, within the
//!    Retarget policy) and judges the projected stone with [`judge`]. A seed that cannot be
//!    projected is a refusal, not a silently angle-only candidate. The anchored seed is the
//!    retarget's REQUEST: the gate's turn allowance is measured on it, not on anything the
//!    projection repairs ([`Attempt::requested`]).
//! 3. If the original had a table or culet of its own whose size changed, builds a second
//!    candidate with their heights refitted ([`refit_flats`]), projects it the same way
//!    (judged by the same request, not by the projected state it is built on), and prefers
//!    it when it is valid.
//! 4. Measures the three optical columns ([`RetargetMetrics`]) and reports the silhouette
//!    figures (total depth, crown-to-pavilion ratio) next to the verdict.
//!
//! [`MeetPreservationBaseline`]: indicatrix_cut_core::meet_preservation::MeetPreservationBaseline
//!
//! If the ORIGINAL design cannot be analysed (it does not solve, or does not close), nothing
//! can be compared: the check reports [`ValidityStatus::Unchecked`] with no anchors,
//! and Apply remains blocked until a valid candidate is available.

#[cfg(test)]
use super::validity::analyze;
use super::{
    anchors::{
        AnchorChange, anchored_candidate_for, anchored_candidate_split, scale_reference_mast,
    },
    metrics::{MetricColumn, RetargetMetrics, measure_analysis_column},
    plan::RetargetPlan,
    refit::refit_flats,
    validity::{
        InvalidReason, MIN_GIRDLE_FRACTION, RetargetStrategy, RetargetValidity, StoneAnalysis,
        judge,
    },
};
use indicatrix::{
    geometry::meet_solver::MeetConstraint,
    optics::{LightingPreset, materials::GemMaterial},
};
use indicatrix_cut_core::{Design, design::RelationError};
use std::sync::Arc;

/// What analysing the original stone gave: the analysis, or why there is none.
pub type AnalysisResult = Result<StoneAnalysis, InvalidReason>;

/// How far a retarget may thicken the girdle band to keep its corners (decision O5): thickness
/// only, thicker only, in two steps tried only after a girdle-type failure.
///
/// Thickening translates the crown half up and the pavilion half down by half the change each,
/// so the plan view, the table and culet size and the crown-to-pavilion ratio are kept exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GirdleAllowance {
    /// The largest thickening, as a fraction of the original band thickness (0.10 = +10 %).
    pub max_fraction: f64,
}

impl GirdleAllowance {
    /// The dialog's allowance: up to +10 %.
    pub const DEFAULT_MAX_FRACTION: f64 = 0.10;

    /// The allowance the dialog offers, switched on.
    #[must_use]
    pub const fn standard() -> Self {
        Self {
            max_fraction: Self::DEFAULT_MAX_FRACTION,
        }
    }

    /// The ladder after rung 0 (no change): half the maximum, then the maximum.
    #[must_use]
    pub fn rungs(self) -> [f64; 2] {
        let max = self.max_fraction.max(0.0);
        [0.5 * max, max]
    }
}

/// What a check needs to know.
#[derive(Debug, Clone, Copy)]
pub struct CheckInputs<'a> {
    /// The girdle allowance, or `None` to keep the band as it is.
    pub girdle: Option<GirdleAllowance>,
    /// The live design.
    pub design: &'a Design,
    /// The plan to check.
    pub plan: &'a RetargetPlan,
    /// The design's current material, if it names one (its metrics column needs it).
    pub current_gem: Option<&'a GemMaterial>,
    /// The lighting the metrics are measured under. The desktop dialog always passes
    /// `CANONICAL_LIGHTING_PRESET` (the grading tray), never the viewport's preset.
    pub lighting: LightingPreset,
}

/// The result of one check.
#[derive(Debug, Clone, PartialEq)]
pub struct RetargetCheck {
    /// Exact projected authored state, shared by preview and Apply.
    pub candidate: Option<Design>,
    /// The operation-start network and source identity.
    pub baseline: Option<Arc<indicatrix_cut_core::meet_preservation::MeetPreservationBaseline>>,
    /// The verdict and its reasons.
    pub validity: RetargetValidity,
    /// The `ScaleReference` masts the retarget changes (re-anchored and refitted). Apply
    /// them with the angles, in one undo step.
    ///
    /// The table and the culet are listed only when their HEIGHT is refitted so they keep
    /// their size ([`refit_flats`]); a flat tier is never a hinge anchor and its angle never
    /// changes.
    pub anchors: Vec<AnchorChange>,
    /// The optical comparison.
    pub metrics: RetargetMetrics,
}

impl RetargetCheck {
    /// Revalidate the exact delivered state and return its atomic History edit.
    ///
    /// # Errors
    ///
    /// A message when no baseline or candidate is available, the design changed, the
    /// verdict forbids Apply, or the geometry gate refuses the candidate.
    pub fn validated_edit(&self, original: &Design) -> Result<indicatrix_cut_core::Edit, String> {
        let baseline = self
            .baseline
            .as_ref()
            .ok_or("No original meet validation; recompute the candidate.")?;
        if baseline.original() != original || !baseline.original().tier_ids_eq(original) {
            return Err("The design changed; recompute the candidate.".into());
        }
        let candidate = self
            .candidate
            .as_ref()
            .ok_or("No validated geometry is available.")?;
        if !self.allows_apply() {
            return Err(self.validity.headline());
        }
        baseline
            .validate(
                candidate,
                indicatrix_cut_core::meet_preservation::PreservationPolicy::RETARGET,
            )
            .map_err(|e| e.to_string())?;
        Ok(indicatrix_cut_core::Edit::Batch(
            candidate
                .tiers
                .iter()
                .zip(&original.tiers)
                .enumerate()
                .filter(|(_, (now, before))| now != before)
                .map(|(index, (tier, _))| indicatrix_cut_core::Edit::ModifyTier {
                    index,
                    tier: tier.clone(),
                })
                .collect(),
        ))
    }

    /// `true` when Apply may go ahead.
    #[must_use]
    pub fn allows_apply(&self) -> bool {
        self.validity.allows_apply()
    }
}

/// One candidate and what the gate says about it.
pub(super) struct Attempt {
    /// The candidate design: the new angles and masts, relations followed.
    pub(super) design: Design,
    /// The anchored candidate before any projection: the retarget's REQUEST, whose facet
    /// turn the gate measures its allowance by (`PreservationPolicy::bounds_follow_turn`).
    /// A refit inherits it from the attempt it is built on, so the first projection's repair
    /// cannot enlarge the allowance the second is judged by (`OptimizeOptions::turn_request`).
    pub(super) requested: Design,
    /// The masts that differ from the live design.
    pub(super) anchors: Vec<AnchorChange>,
    /// How the masts were chosen.
    pub(super) strategy: RetargetStrategy,
    /// The candidate's own analysis (or why it has none).
    pub(super) analysis: AnalysisResult,
    /// Every reason the gate refuses it (empty means valid).
    pub(super) reasons: Vec<InvalidReason>,
    /// The fraction of the girdle band the candidate thickened it by (`None`: not thickened).
    pub(super) girdle_step: Option<f64>,
}

/// What an [`Attempt`] is built from.
pub(super) struct Proposal {
    /// The candidate to project.
    pub(super) design: Design,
    /// The masts that differ from the live design.
    pub(super) anchors: Vec<AnchorChange>,
    /// How the masts were chosen.
    pub(super) strategy: RetargetStrategy,
    /// The request the candidate is judged by (see [`Attempt::requested`]): the candidate
    /// itself for an anchored seed, the base attempt's request for a refit.
    pub(super) requested: Design,
}

/// The options a Retarget attempt projects its anchored seed with.
///
/// Shared with the CLI census so the two cannot drift. `requested` is the anchored candidate
/// before projection; `moves` lists the tiers the Shift policy lets move.
#[must_use]
pub fn retarget_projection_options(
    original_design: &Design,
    original: &StoneAnalysis,
    requested: &Design,
    moves: &[(usize, f64)],
) -> indicatrix_cut_core::OptimizeOptions {
    indicatrix_cut_core::OptimizeOptions {
        preservation_baseline: original.preservation.clone(),
        preservation_policy: indicatrix_cut_core::meet_preservation::PreservationPolicy::RETARGET,
        vary_anchored: true,
        anchor_hinges: original.hinges.iter().map(|(&i, h)| (i, h.point)).collect(),
        // Projection may couple the requested moves, but must not unlock a
        // crown angle the Shift policy explicitly held fixed.
        angle_bounds: original_design
            .tiers
            .iter()
            .enumerate()
            .filter(|(i, _)| !moves.iter().any(|(moving, _)| moving == i))
            .map(|(i, tier)| (i, (tier.angle_deg, tier.angle_deg)))
            .collect(),
        // The turn allowance is the REQUEST's (owner decision 2026-10-19): the anchored
        // candidate's, for this projection and for the refit projected after it, so
        // neither projection is judged by angles a repair turned further.
        turn_request: Some(
            indicatrix_cut_core::meet_preservation::tangent_changes(original_design, requested)
                .into(),
        ),
        ..Default::default()
    }
}

impl Attempt {
    fn new(
        original_design: &Design,
        original: &StoneAnalysis,
        proposal: Proposal,
        moves: &[(usize, f64)],
        should_stop: &dyn Fn() -> bool,
    ) -> Self {
        let Proposal {
            mut design,
            mut anchors,
            strategy,
            requested,
        } = proposal;
        let options = retarget_projection_options(original_design, original, &requested, moves);
        let projected = indicatrix_cut_core::optimize::project_preserved_design_controlled(
            original_design,
            &design,
            &options,
            &indicatrix::geometry::meet_solver::SolveControl::with_stop(should_stop),
        );
        let analysis = match projected {
            Ok(preserved) => {
                design = preserved;
                anchors = super::anchors::mast_differences(original_design, &design);
                super::validity::analyze_controlled(
                    &design,
                    false,
                    &indicatrix::geometry::meet_solver::SolveControl::with_stop(should_stop),
                )
            }
            Err(error) => Err(InvalidReason::Preservation(error.to_string())),
        };
        let reasons = judge(original_design, original, &analysis);
        Self {
            design,
            requested,
            anchors,
            strategy,
            analysis,
            reasons,
            girdle_step: None,
        }
    }

    /// The live design itself as an attempt that changes nothing (valid by definition).
    pub(super) fn unchanged(design: &Design, original: &StoneAnalysis) -> Self {
        Self {
            design: design.clone(),
            requested: design.clone(),
            anchors: Vec::new(),
            strategy: RetargetStrategy::AnglesOnly,
            analysis: Ok(original.clone()),
            reasons: Vec::new(),
            girdle_step: None,
        }
    }
}

/// The refitted attempt built on `base`, if any flat needed refitting. `live` is the live
/// design the retarget starts from. A failed projection must not prevent repairing a
/// flat that the raw angle shift swallowed: geometry analysis can seed that repair,
/// but the repaired proposal must still pass projection and the full validity gate.
fn refit_attempt(
    live: &Design,
    original: &StoneAnalysis,
    base: &Attempt,
    moves: &[(usize, f64)],
    should_stop: &dyn Fn() -> bool,
) -> Option<Attempt> {
    let raw_analysis;
    let candidate = if let Ok(candidate) = &base.analysis {
        candidate
    } else {
        raw_analysis = super::validity::analyze_controlled(
            &base.design,
            false,
            &indicatrix::geometry::meet_solver::SolveControl::with_stop(should_stop),
        )
        .ok()?;
        &raw_analysis
    };
    let refits = refit_flats(
        &base.design,
        &candidate.solved,
        &candidate.flats,
        original,
        should_stop,
    );
    if refits.is_empty() || should_stop() {
        return None;
    }
    let mut design = base.design.clone();
    let mut anchors = base.anchors.clone();
    for refit in &refits {
        let Some(old_mast) = scale_reference_mast(live, refit.tier_index) else {
            continue;
        };
        design.tiers[refit.tier_index].constraint = MeetConstraint::ScaleReference(refit.new_mast);
        anchors.retain(|anchor| anchor.tier_index != refit.tier_index);
        anchors.push(AnchorChange {
            tier_index: refit.tier_index,
            old_mast,
            new_mast: refit.new_mast,
        });
    }
    Some(Attempt::new(
        live,
        original,
        Proposal {
            design,
            anchors,
            strategy: RetargetStrategy::AnchoredRefit,
            // The refit is judged by the retarget's request, not by the projected state it
            // is built on.
            requested: base.requested.clone(),
        },
        moves,
        should_stop,
    ))
}

/// The best candidate for `moves` (`(tier_index, new_angle)` of every tier that moves on its
/// own): every moving pinned tier turns about its girdle corners, the tiers that follow a
/// relation follow it, and the table and culet heights are refitted when that gives a valid
/// stone.
///
/// `Ok(None)` when `should_stop` turned `true` before it finished.
///
/// # Errors
///
/// The relation engine's error when the relations cannot be satisfied after the move.
///
/// With a `girdle` allowance and a plain result the gate refuses for a GIRDLE reason (too thin,
/// thin at its corners, gone), the band is thickened in steps (see [`GirdleAllowance`]) and the
/// first step that gives a valid stone wins. Any other failure, and a plain result that is
/// valid, never climb the ladder, so without a girdle failure the result is exactly the
/// allowance-free one.
pub(super) fn best_attempt(
    live: &Design,
    moves: &[(usize, f64)],
    original: &StoneAnalysis,
    girdle: Option<GirdleAllowance>,
    should_stop: &dyn Fn() -> bool,
) -> Result<Option<Attempt>, RelationError> {
    let Some(plain) = attempt_at(live, moves, original, None, should_stop)? else {
        return Ok(None);
    };
    let Some(allowance) = girdle else {
        return Ok(Some(plain));
    };
    if !plain
        .reasons
        .iter()
        .any(|reason| thickening_can_cure(reason, allowance))
    {
        return Ok(Some(plain));
    }
    let Some((low, high)) = original.girdle_band else {
        return Ok(Some(plain));
    };
    let band = high - low;
    if !band.is_finite() || band <= 0.0 {
        return Ok(Some(plain));
    }
    for fraction in allowance.rungs() {
        if should_stop() {
            return Ok(None);
        }
        let Some(mut attempt) = attempt_at(
            live,
            moves,
            original,
            Some((fraction, fraction * band)),
            should_stop,
        )?
        else {
            return Ok(None);
        };
        if attempt.reasons.is_empty() {
            attempt.girdle_step = Some(fraction);
            return Ok(Some(attempt));
        }
    }
    Ok(Some(plain))
}

/// `true` for the reasons a thicker girdle can cure with `allowance`.
///
/// The corner and gone cases always qualify. The overall `GirdleTooThin` qualifies only when
/// the largest thickening can lift it back over `MIN_GIRDLE_FRACTION` of what it was: the
/// translation adds at most `max_fraction` of the original band, so a band that lost more than
/// that can never pass and the ladder's solve, mesh and refit attempts would be wasted.
fn thickening_can_cure(reason: &InvalidReason, allowance: GirdleAllowance) -> bool {
    match reason {
        InvalidReason::GirdleTooThin {
            was_percent,
            now_percent,
        } => {
            allowance
                .max_fraction
                .max(0.0)
                .mul_add(*was_percent, *now_percent)
                >= MIN_GIRDLE_FRACTION * was_percent
        }
        InvalidReason::GirdleThinAtCorners { .. } | InvalidReason::GirdleGone => true,
        _ => false,
    }
}

/// One rung of [`best_attempt`]: the anchored, refitted candidate, with the girdle band
/// thickened by `thicken` (`(fraction, delta)`) when given.
fn attempt_at(
    live: &Design,
    moves: &[(usize, f64)],
    original: &StoneAnalysis,
    thicken: Option<(f64, f64)>,
    should_stop: &dyn Fn() -> bool,
) -> Result<Option<Attempt>, RelationError> {
    let (hinge_design, hinge_anchors) = match thicken {
        None => anchored_candidate_for(live, moves, original)?,
        Some((_, delta)) => {
            let split = original.split_at_girdle(delta);
            anchored_candidate_split(live, moves, &split, 0.5 * delta)?
        }
    };
    let base_strategy = if hinge_anchors.is_empty() {
        RetargetStrategy::AnglesOnly
    } else {
        RetargetStrategy::Anchored
    };
    let mut best = Attempt::new(
        live,
        original,
        Proposal {
            design: hinge_design.clone(),
            anchors: hinge_anchors,
            strategy: base_strategy,
            requested: hinge_design,
        },
        moves,
        should_stop,
    );
    if should_stop() {
        return Ok(None);
    }
    if !original.flats.is_empty() {
        if let Some(refitted) = refit_attempt(live, original, &best, moves, should_stop) {
            // The refit is the intended result; keep the plain one only when the refit is
            // invalid and the plain one is not.
            if refitted.reasons.is_empty() || !best.reasons.is_empty() {
                best = refitted;
            }
        }
        if should_stop() {
            return Ok(None);
        }
    }
    Ok(Some(best))
}

/// The optical columns for the original stone and the chosen candidate.
fn measure(
    inputs: &CheckInputs<'_>,
    original: &StoneAnalysis,
    candidate: &AnalysisResult,
) -> RetargetMetrics {
    let target = &inputs.plan.target.gem;
    // Each column measures the stone with its concave tools, as the Compare window does.
    let column = |analysis: &StoneAnalysis, gem: &GemMaterial| -> MetricColumn {
        measure_analysis_column(analysis, gem, inputs.lighting)
    };
    RetargetMetrics {
        current_in_current: inputs.current_gem.map(|gem| column(original, gem)),
        current_in_target: Some(column(original, target)),
        retargeted_in_target: candidate
            .as_ref()
            .ok()
            .map(|analysis| column(analysis, target)),
    }
}

fn check_against(
    inputs: &CheckInputs<'_>,
    original: &StoneAnalysis,
    should_stop: &dyn Fn() -> bool,
) -> Option<RetargetCheck> {
    let moves = inputs.plan.moving_angles();
    let best = match best_attempt(inputs.design, &moves, original, inputs.girdle, should_stop) {
        Ok(Some(best)) => best,
        Ok(None) => return None,
        Err(error) => return Some(relation_failure(inputs, original, &error)),
    };
    let validity = RetargetValidity::judged(original, &best.analysis, best.reasons, best.strategy)
        .with_girdle_thickened(best.girdle_step);
    Some(RetargetCheck {
        candidate: Some(best.design.clone()),
        baseline: original.preservation.clone(),
        validity,
        anchors: best.anchors,
        metrics: measure(inputs, original, &best.analysis),
    })
}

/// The check for a plan whose relations cannot be satisfied: not valid, nothing to apply.
fn relation_failure(
    inputs: &CheckInputs<'_>,
    original: &StoneAnalysis,
    error: &RelationError,
) -> RetargetCheck {
    let reason = InvalidReason::RelationFails(error.to_string());
    let none: AnalysisResult = Err(reason.clone());
    RetargetCheck {
        candidate: None,
        baseline: None,
        validity: RetargetValidity::judged(
            original,
            &none,
            vec![reason],
            RetargetStrategy::AnglesOnly,
        ),
        anchors: Vec::new(),
        metrics: measure(inputs, original, &none),
    }
}

/// The check for a design whose original stone could not be analysed.
fn unchecked(reason: &InvalidReason) -> RetargetCheck {
    RetargetCheck {
        candidate: None,
        baseline: None,
        validity: RetargetValidity::unchecked(reason),
        anchors: Vec::new(),
        metrics: RetargetMetrics::default(),
    }
}

/// Runs the whole check.
///
/// `cached` is the analysis of the original stone from an earlier call for the SAME design
/// (pass `None` the first time); the analysis used is returned so the caller can keep it.
/// `should_stop` is read between steps: when it turns `true` the check is abandoned and
/// `None` comes back.
#[must_use]
pub fn run_check(
    inputs: &CheckInputs<'_>,
    cached: Option<Arc<AnalysisResult>>,
    should_stop: &dyn Fn() -> bool,
) -> Option<(Arc<AnalysisResult>, RetargetCheck)> {
    let original = cached.unwrap_or_else(|| {
        Arc::new(super::validity::analyze_controlled(
            inputs.design,
            true,
            &indicatrix::geometry::meet_solver::SolveControl::with_stop(should_stop),
        ))
    });
    if should_stop() {
        return None;
    }
    let check = match original.as_ref() {
        Ok(analysis) => check_against(inputs, analysis, should_stop)?,
        Err(reason) => unchecked(reason),
    };
    Some((original, check))
}

/// [`run_check`] with no cache and no stop flag -- the whole check in one call.
#[must_use]
pub fn check_retarget(inputs: &CheckInputs<'_>) -> RetargetCheck {
    run_check(inputs, None, &|| false)
        .map_or_else(|| unchecked(&InvalidReason::NotClosed), |(_, check)| check)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod refit_projection_tests;
