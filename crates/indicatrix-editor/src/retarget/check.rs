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
//!    are alive, the girdle-side edge of every flat crown or pavilion tier).
//! 2. Builds the candidate with every moving, pinned tier re-anchored to its edge
//!    ([`anchored_candidate`]) and judges it with [`judge`].
//! 3. If the original had a table or culet of its own whose size changed, builds a second
//!    candidate with their heights refitted ([`refit_flats`]) and prefers it when it is valid.
//! 4. Measures the three optical columns ([`RetargetMetrics`]) and reports the silhouette
//!    figures (total depth, crown-to-pavilion ratio) next to the verdict.
//!
//! If the ORIGINAL design cannot be analysed (it does not solve, or does not close), nothing
//! can be compared: the check reports [`ValidityStatus::Unchecked`] with no anchors, and the
//! caller applies the plain angle edit, as it always did.

use super::{
    anchors::{
        AnchorChange, anchored_candidate_for, anchored_candidate_split, scale_reference_mast,
    },
    metrics::{MetricColumn, RetargetMetrics, measure_column},
    plan::RetargetPlan,
    refit::refit_flats,
    validity::{
        InvalidReason, MIN_GIRDLE_FRACTION, RetargetStrategy, RetargetValidity, StoneAnalysis,
        analyze, judge,
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

impl Attempt {
    fn new(
        original_design: &Design,
        original: &StoneAnalysis,
        design: Design,
        anchors: Vec<AnchorChange>,
        strategy: RetargetStrategy,
    ) -> Self {
        let analysis = analyze(&design, false);
        let reasons = judge(original_design, original, &analysis);
        Self {
            design,
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
            anchors: Vec::new(),
            strategy: RetargetStrategy::AnglesOnly,
            analysis: Ok(original.clone()),
            reasons: Vec::new(),
            girdle_step: None,
        }
    }
}

/// The refitted attempt built on `base`, if any flat needed refitting. `live` is the live
/// design the retarget starts from.
fn refit_attempt(
    live: &Design,
    original: &StoneAnalysis,
    base: &Attempt,
    should_stop: &dyn Fn() -> bool,
) -> Option<Attempt> {
    let Ok(candidate) = &base.analysis else {
        return None;
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
        design,
        anchors,
        RetargetStrategy::AnchoredRefit,
    ))
}

/// The best candidate for `moves` (`(tier_index, new_angle)` of every tier that moves on its
/// own): every moving pinned tier turns about its girdle-side edge, the tiers that follow a
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
    let mut best = Attempt::new(live, original, hinge_design, hinge_anchors, base_strategy);
    if should_stop() {
        return Ok(None);
    }
    if !original.flats.is_empty() {
        if let Some(refitted) = refit_attempt(live, original, &best, should_stop) {
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
    let column = |planes: &[(glam::DVec3, f64)], gem: &GemMaterial| -> MetricColumn {
        measure_column(planes, gem, inputs.lighting)
    };
    RetargetMetrics {
        current_in_current: inputs.current_gem.map(|gem| column(&original.planes, gem)),
        current_in_target: Some(column(&original.planes, target)),
        retargeted_in_target: candidate
            .as_ref()
            .ok()
            .map(|analysis| column(&analysis.planes, target)),
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
    let original = cached.unwrap_or_else(|| Arc::new(analyze(inputs.design, true)));
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
