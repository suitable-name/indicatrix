//! The Optimize half of "Retarget for material": a search over the angles the Shift plan
//! only moves by a formula.
//!
//! Shift moves every crown and pavilion angle by the critical-angle shift and stops. Optimize
//! starts from that result and lets every crown and pavilion angle move on its own, inside a
//! range the cutter picks, to score best in the TARGET material. The result is always checked
//! with the same validity gate Shift uses, against the design as it is now.
//!
//! # What the search does
//!
//! 1. The start. The Shift result (angles, girdle-side edges kept, table and culet refitted)
//!    is the start when it is valid. When it is not, the start is the largest part of the
//!    Shift change that is (found by halving), or the design as it is.
//! 2. The search. [`optimize_design_with`] varies every crown and pavilion angle, including
//!    the ones pinned to a mast: a pinned facet turns about the girdle-side edge it has in the
//!    start stone, so the girdle keeps its outline and height. Each angle may move at most
//!    the chosen range either side of its start angle, never across the horizontal and never
//!    past 89 degrees. Table, culet and girdle tiers never tilt (the table and culet HEIGHTS
//!    are refitted on every option, see below). The girdle must keep at least half the
//!    thickness the design has now, both overall and at its thinnest point (the corners
//!    between the walls, which the overall figure cannot see), and the table facet must
//!    survive. A pavilion angle also never drops below [`pavilion_floor_deg`]: the target's
//!    critical angle plus the margin the design has now (at most 2 degrees), so a search
//!    cannot buy brilliance with a window.
//! 3. The gate. Tiers that follow a relation are brought back into line, the table and culet
//!    heights are refitted to the size the design has now ([`refit_flats`], as Shift does),
//!    then each result goes through the validity gate. A result the gate refuses is dropped
//!    and counted.
//!
//! # Keeping the design's look
//!
//! With [`SearchSettings::keep_look`] (the default) every score, the start stone's included,
//! adds a penalty for drifting from the design's own table size and crown-to-pavilion ratio
//! (measured on the live design, not on the start stone; weight [`KEEP_LOOK_WEIGHT`]). Off,
//! the options carry no shape target and are ranked by the optical score and the yield alone.
//!
//! The same inputs give the same candidates: the search is seeded and the code here has no
//! other source of variation.
//!
//! # Tiers that follow a relation
//!
//! They are never varied. After the search each result has its followers evaluated
//! ([`fold_relations`]) and re-anchored, so a relation such as `P2 = P1 + 2` still holds.
//! The proposal leaves the followers out: applying it through the editor session makes them
//! follow again in the same undo step.
//!
//! The optimizer scores a stone whose followers still sit on their start masts and whose table
//! and culet are not refitted, so a result whose followers or flats moved is scored again as
//! the stone that is delivered (re-anchored, refitted), at the same fidelity and with the
//! same shape penalty, before the options are ranked: the order, the "best" mark and the figures
//! shown all describe the stone the cutter gets.

use super::{
    anchors::{angle_differences, fold_relations, mast_differences},
    check::{AnalysisResult, Attempt, GirdleAllowance, best_attempt},
    metrics::{MetricColumn, RetargetMetrics, measure_column},
    plan::RetargetPlan,
    refit::refit_flats,
    retarget_scope,
    validity::{
        InvalidReason, KNIFE_EDGE_PERCENT, MIN_GIRDLE_FRACTION, RetargetStrategy, RetargetValidity,
        StoneAnalysis, analyze, judge,
    },
};
use indicatrix::{
    geometry::meet_solver::{Block, MeetConstraint},
    optics::{LightingPreset, materials::GemMaterial},
};
use indicatrix_cut_core::{
    Design, ObjectivePreset, OptimizeCandidate, OptimizeConfig, OptimizeOptions, SearchHooks,
    ShapeTarget,
    design::hinge::{TierHinge, mast_through, tier_hinges},
    free_tier_indices_with,
    optics_hints::{
        MAX_RETARGET_ANGLE_DEG, MIN_RETARGET_ANGLE_DEG, critical_angle_deg, tier_margin_deg,
    },
    optimize::{
        ObjectiveFidelity, SearchStage, inclusive_max_evaluations_for, score_finished_design,
    },
    optimize_design_with,
};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

mod report;

pub use report::{
    CandidateKind, CandidateNumbers, SearchCandidate, SearchError, SearchReport, StartPoint,
};

/// The angle ranges the dialog offers, in degrees either side of each start angle.
pub const RANGE_CHOICES_DEG: [f64; 4] = [3.0, 6.0, 10.0, 15.0];

/// The range chosen when the dialog opens (an index into [`RANGE_CHOICES_DEG`]): 6 degrees.
pub const DEFAULT_RANGE_INDEX: usize = 1;

/// The search budgets the dialog offers, in evaluations of one candidate stone. The polish
/// stage adds a few more (see [`SearchSettings::total_steps`]).
pub const EFFORT_CHOICES: [usize; 3] = [100, 300, 800];

/// The budget chosen when the dialog opens (an index into [`EFFORT_CHOICES`]): 300.
pub const DEFAULT_EFFORT_INDEX: usize = 1;

/// How many ranked alternatives the search keeps.
pub const CANDIDATES_KEPT: usize = 3;

/// The weight of the shape penalty "Keep the design's look" adds to every score (see
/// [`ShapeTarget`]): 10 % of drift in the table size or the ratio costs 5 score points.
pub const KEEP_LOOK_WEIGHT: f32 = 0.5;

/// The most margin over the target's critical angle a pavilion floor asks for: the floor keeps
/// the design's own margin, but never more than this many degrees of it.
const PAVILION_MARGIN_CAP_DEG: f64 = 2.0;

/// The seed of the search. A constant, so the same inputs give the same candidates.
const SEARCH_SEED: u64 = 0;

/// The steepest angle a search may use: half a degree under the retarget's own limit.
const SEARCH_CEILING_DEG: f64 = MAX_RETARGET_ANGLE_DEG - 0.5;

/// How many times the Shift change is halved while looking for the largest valid part of it.
const PARTIAL_STEPS: usize = 5;

/// The smallest mast a re-anchored follower may get.
const MIN_FOLLOWER_MAST: f64 = 1e-6;

/// What the cutter chose in the dialog.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchSettings {
    /// What the search favours.
    pub preset: ObjectivePreset,
    /// How far each angle may move either side of its start angle, in degrees.
    pub range_deg: f64,
    /// The evaluation budget of the coordinate stage.
    pub evaluations: usize,
    /// The seed of the search.
    pub seed: u64,
    /// How many ranked alternatives to keep.
    pub keep: usize,
    /// Penalise every option for drifting from the design's table size and crown-to-pavilion
    /// ratio ([`KEEP_LOOK_WEIGHT`]). On by default.
    pub keep_look: bool,
    /// May the girdle band thicken (up to +10 %) when its corners would otherwise pinch? On by
    /// default. `None` keeps the band as it is.
    pub girdle: Option<GirdleAllowance>,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self::from_choices(0, DEFAULT_RANGE_INDEX, DEFAULT_EFFORT_INDEX)
    }
}

impl SearchSettings {
    /// The settings for the dialog's three combo boxes (a range or effort index past the end
    /// of its list takes the last entry; an objective index past the end takes Balanced).
    #[must_use]
    pub fn from_choices(objective_index: usize, range_index: usize, effort_index: usize) -> Self {
        Self {
            preset: ObjectivePreset::from_index(objective_index),
            range_deg: RANGE_CHOICES_DEG[range_index.min(RANGE_CHOICES_DEG.len() - 1)],
            evaluations: EFFORT_CHOICES[effort_index.min(EFFORT_CHOICES.len() - 1)],
            seed: SEARCH_SEED,
            keep: CANDIDATES_KEPT,
            keep_look: true,
            girdle: Some(GirdleAllowance::standard()),
        }
    }

    /// The optimizer's configuration for these settings, scored under `lighting`.
    #[must_use]
    pub fn config(&self, lighting: LightingPreset) -> OptimizeConfig {
        OptimizeConfig {
            weights: self.preset.weights(),
            seed: self.seed,
            max_evaluations: self.evaluations,
            lighting,
            ..OptimizeConfig::default()
        }
    }

    /// The most evaluations a search with `free_tiers` free angles may spend: the budget
    /// plus the polish stage's own.
    #[must_use]
    pub fn total_steps(&self, free_tiers: usize) -> usize {
        let config = OptimizeConfig {
            max_evaluations: self.evaluations,
            ..OptimizeConfig::default()
        };
        inclusive_max_evaluations_for(
            &config,
            OptimizeOptions::default().keep_candidates,
            free_tiers,
        )
    }
}

/// One sentence on how long a search of `total_steps` evaluations takes, given the measured
/// cost of one (`step_ms`, milliseconds; `None` before it is known).
#[must_use]
pub fn estimate_text(total_steps: usize, step_ms: Option<f64>) -> String {
    step_ms
        .filter(|ms| ms.is_finite() && *ms > 0.0)
        .map_or_else(
            || {
                format!(
                    "Up to {total_steps} steps. The time per step is measured once the change has been checked."
                )
            },
            |ms| {
                let seconds = total_steps as f64 * ms / 1000.0;
                let time = if seconds < 1.0 {
                    "under a second".to_string()
                } else if seconds < 90.0 {
                    format!("about {seconds:.0} s")
                } else {
                    format!("about {:.0} min", seconds / 60.0)
                };
                format!(
                    "Up to {total_steps} steps, {time} at {ms:.0} ms a step, plus a few seconds to score the results."
                )
            },
        )
}

/// Runs one evaluation of the kind a search repeats (solve, mesh, manufacturing checks and
/// one fast optical measurement) so the caller can time it. `false` when the design does not
/// solve and close.
///
/// The editor crate keeps no clock of its own: the caller takes the time around this call.
#[must_use]
pub fn probe_evaluation(design: &Design, gem: &GemMaterial, lighting: LightingPreset) -> bool {
    analyze(design, false)
        .map(|analysis| measure_column(&analysis.planes, gem, lighting))
        .is_ok()
}

/// Everything a search needs.
#[derive(Debug, Clone, Copy)]
pub struct SearchInputs<'a> {
    /// The live design.
    pub design: &'a Design,
    /// The Shift plan: the target material, the crown policy and the angles the start uses.
    pub plan: &'a RetargetPlan,
    /// The design's current material, if it names one (the first metrics column needs it).
    pub current_gem: Option<&'a GemMaterial>,
    /// The lighting everything is scored under. The desktop dialog always passes
    /// `CANONICAL_LIGHTING_PRESET` (the grading tray), never the viewport's preset.
    pub lighting: LightingPreset,
    /// What the cutter chose.
    pub settings: &'a SearchSettings,
}

/// The angle range one tier may be searched in: `range_deg` either side of `angle_deg`.
///
/// The range is signed like the angle, never closer to the horizontal than the retarget's own
/// minimum and never steeper than [`SEARCH_CEILING_DEG`]. The start angle itself is always
/// inside.
#[must_use]
pub fn angle_bounds(angle_deg: f64, range_deg: f64) -> (f64, f64) {
    let magnitude = angle_deg.abs();
    let low = (magnitude - range_deg).max(MIN_RETARGET_ANGLE_DEG);
    let low = low.min(magnitude);
    let high = (magnitude + range_deg).min(SEARCH_CEILING_DEG);
    let high = high.max(magnitude);
    if angle_deg.is_sign_negative() {
        (-high, -low)
    } else {
        (low, high)
    }
}

/// The least girdle thickness a search result may have, as a fraction of the START stone's:
/// half of the live design's, measured against the start. The gate checks the same half
/// against the live design afterwards.
fn girdle_fraction(original: &StoneAnalysis, start: &StoneAnalysis) -> f64 {
    match (original.girdle_percent, start.girdle_percent) {
        (Some(was), Some(now)) if now > 0.0 => (MIN_GIRDLE_FRACTION * was / now).min(1.0),
        _ => MIN_GIRDLE_FRACTION,
    }
}

/// [`girdle_fraction`] for the girdle's THINNEST point: half of the live design's thinnest
/// point, measured against the start's. A start that is thinner at its corners than the live
/// design (a part of the Shift change) makes the floor a larger share of the start's own
/// figure; one that is thicker, a smaller share.
fn thinnest_fraction(original: &StoneAnalysis, start: &StoneAnalysis) -> f64 {
    match (
        original.girdle_thinnest_percent,
        start.girdle_thinnest_percent,
    ) {
        (Some(was), Some(now)) if now > KNIFE_EDGE_PERCENT => {
            (MIN_GIRDLE_FRACTION * was / now).min(1.0)
        }
        _ => MIN_GIRDLE_FRACTION,
    }
}

/// The two floors the optimizer's guard takes, each applied to its own figure: the overall girdle
/// ([`girdle_fraction`]) and its thinnest point ([`thinnest_fraction`]). The gate holds each to
/// half of the live design's figure, so a result under either would be refused afterwards, and a
/// result over both is never refused by the guard for the other's sake.
fn guard_fractions(original: &StoneAnalysis, start: &StoneAnalysis) -> GuardFractions {
    GuardFractions {
        overall: girdle_fraction(original, start),
        thinnest: thinnest_fraction(original, start),
    }
}

/// What [`guard_fractions`] found: the share of the start's figure a result must keep.
#[derive(Debug, Clone, Copy, PartialEq)]
struct GuardFractions {
    /// For the overall girdle thickness.
    overall: f64,
    /// For the girdle band's thinnest point.
    thinnest: f64,
}

/// The tiers a search may vary: every crown and pavilion tier with a slope, except the ones
/// that follow a relation.
fn free_candidates(design: &Design) -> Vec<usize> {
    retarget_scope(design)
        .0
        .into_iter()
        .filter(|&index| !design.is_tier_driven(index))
        .collect()
}

/// The least magnitude a pavilion angle may take in the target material.
///
/// Its critical angle plus the margin the live angle has over the critical angle of the current material, at most
/// [`PAVILION_MARGIN_CAP_DEG`] of it. A pavilion that was safe stays as safe as that in the
/// target, so a search cannot trade a window for brilliance.
#[must_use]
pub fn pavilion_floor_deg(live_angle_deg: f64, n_from: f64, n_to: f64) -> f64 {
    critical_angle_deg(n_to) + tier_margin_deg(live_angle_deg, n_from).min(PAVILION_MARGIN_CAP_DEG)
}

/// `bounds` (signed like the angle `start_deg`) with the low magnitude raised to `floor_deg`,
/// never above the start angle itself, so the start stays inside. A floor that is not finite
/// changes nothing.
fn raise_floor(bounds: (f64, f64), start_deg: f64, floor_deg: f64) -> (f64, f64) {
    if !floor_deg.is_finite() {
        return bounds;
    }
    let floor = floor_deg.min(start_deg.abs());
    if start_deg.is_sign_negative() {
        (bounds.0, bounds.1.min(-floor))
    } else {
        (bounds.0.max(floor), bounds.1)
    }
}

/// The shape target of "Keep the design's look": the live design's table percent and
/// crown-to-pavilion ratio, or `None` when the setting is off.
fn shape_target(settings: &SearchSettings, original: &StoneAnalysis) -> Option<ShapeTarget> {
    settings.keep_look.then_some(ShapeTarget {
        table_percent: original.table_percent,
        crown_to_pavilion: original.crown_to_pavilion,
        weight: KEEP_LOOK_WEIGHT,
    })
}

/// What to give the optimizer, resolved against the start design.
fn search_options(
    inputs: &SearchInputs<'_>,
    original: &StoneAnalysis,
    start: &Design,
    free: &[usize],
    hinges: &BTreeMap<usize, TierHinge>,
    fractions: GuardFractions,
) -> OptimizeOptions {
    let settings = inputs.settings;
    let mut options = OptimizeOptions {
        vary_anchored: true,
        min_girdle_fraction: Some(fractions.overall),
        min_girdle_thinnest_fraction: Some(fractions.thinnest),
        keep_candidates: settings.keep,
        shape_target: shape_target(settings, original),
        ..OptimizeOptions::default()
    };
    let blocks = retarget_scope(start).1;
    for &index in free {
        let start_deg = start.tiers[index].angle_deg;
        let mut bounds = angle_bounds(start_deg, settings.range_deg);
        if blocks.get(index) == Some(&Block::Pavilion)
            && let Some(live) = inputs.design.tiers.get(index)
        {
            let floor = pavilion_floor_deg(live.angle_deg, inputs.plan.n_from, inputs.plan.n_to);
            bounds = raise_floor(bounds, start_deg, floor);
        }
        options.angle_bounds.insert(index, bounds);
        if let Some(hinge) = hinges.get(&index) {
            options.anchor_hinges.insert(index, hinge.point);
        }
    }
    options
}

/// The two optical columns that do not depend on the candidate.
struct Columns {
    current_in_current: Option<MetricColumn>,
    current_in_target: Option<MetricColumn>,
}

impl Columns {
    fn measure(inputs: &SearchInputs<'_>, original: &StoneAnalysis) -> Self {
        Self {
            current_in_current: inputs
                .current_gem
                .map(|gem| measure_column(&original.planes, gem, inputs.lighting)),
            current_in_target: Some(measure_column(
                &original.planes,
                &inputs.plan.target.gem,
                inputs.lighting,
            )),
        }
    }

    fn metrics(&self, inputs: &SearchInputs<'_>, candidate: &AnalysisResult) -> RetargetMetrics {
        RetargetMetrics {
            current_in_current: self.current_in_current,
            current_in_target: self.current_in_target,
            retargeted_in_target: candidate.as_ref().ok().map(|analysis| {
                measure_column(&analysis.planes, &inputs.plan.target.gem, inputs.lighting)
            }),
        }
    }
}

/// The largest part of the Shift change that passes the gate, found by halving, or the design
/// as it is when not even a small part does.
fn partial_start(
    inputs: &SearchInputs<'_>,
    original: &StoneAnalysis,
    should_stop: &dyn Fn() -> bool,
) -> Result<(Attempt, StartPoint), SearchError> {
    let (mut low, mut high) = (0.0_f64, 1.0_f64);
    let mut best: Option<(f64, Attempt)> = None;
    for _ in 0..PARTIAL_STEPS {
        if should_stop() {
            return Err(SearchError::Cancelled);
        }
        let middle = f64::midpoint(low, high);
        let moves = inputs.plan.moving_angles_at(middle);
        match best_attempt(
            inputs.design,
            &moves,
            original,
            inputs.settings.girdle,
            should_stop,
        ) {
            Ok(None) => return Err(SearchError::Cancelled),
            Ok(Some(attempt)) if attempt.reasons.is_empty() => {
                low = middle;
                best = Some((middle, attempt));
            }
            Ok(Some(_)) | Err(_) => high = middle,
        }
    }
    Ok(match best {
        Some((fraction, attempt)) => (
            attempt,
            StartPoint::Partial {
                percent: (fraction * 100.0).round() as u32,
            },
        ),
        None => (
            Attempt::unchanged(inputs.design, original),
            StartPoint::Unchanged,
        ),
    })
}

/// Picks the start stone: the full Shift result when it is valid, else the largest valid part
/// of it. Also returns the verdict on the full Shift result.
fn choose_start(
    inputs: &SearchInputs<'_>,
    original: &StoneAnalysis,
    should_stop: &dyn Fn() -> bool,
) -> Result<(Attempt, StartPoint, RetargetValidity), SearchError> {
    let moves = inputs.plan.moving_angles();
    let shift_verdict = match best_attempt(
        inputs.design,
        &moves,
        original,
        inputs.settings.girdle,
        should_stop,
    ) {
        Ok(None) => return Err(SearchError::Cancelled),
        Ok(Some(attempt)) => {
            let verdict = RetargetValidity::judged(
                original,
                &attempt.analysis,
                attempt.reasons.clone(),
                attempt.strategy,
            )
            .with_girdle_thickened(attempt.girdle_step);
            if attempt.reasons.is_empty() {
                return Ok((attempt, StartPoint::Shift, verdict));
            }
            verdict
        }
        Err(error) => {
            let reason = InvalidReason::RelationFails(error.to_string());
            let none: AnalysisResult = Err(reason.clone());
            RetargetValidity::judged(original, &none, vec![reason], RetargetStrategy::AnglesOnly)
        }
    };
    let (start, point) = partial_start(inputs, original, should_stop)?;
    Ok((start, point, shift_verdict))
}

/// Gives every tier that follows a relation, and whose angle moved away from `start`'s, the
/// mast that keeps it on its hinge. `true` when at least one follower got a different mast
/// than the optimizer scored the stone with.
fn reanchor_followers(
    design: &mut Design,
    start: &Design,
    hinges: &BTreeMap<usize, TierHinge>,
) -> bool {
    let mut changed = false;
    for index in 0..design.tiers.len() {
        let moved =
            design.tiers[index].angle_deg.to_bits() != start.tiers[index].angle_deg.to_bits();
        if !moved || !design.is_tier_driven(index) {
            continue;
        }
        let Some(hinge) = hinges.get(&index) else {
            continue;
        };
        let MeetConstraint::ScaleReference(before) = design.tiers[index].constraint else {
            continue;
        };
        let mast = mast_through(
            design.tiers[index].angle_deg,
            hinge.azimuth_rad,
            hinge.side,
            hinge.point,
        );
        if mast.is_finite() && mast >= MIN_FOLLOWER_MAST {
            changed |= mast.to_bits() != before.to_bits();
            design.tiers[index].constraint = MeetConstraint::ScaleReference(mast);
        }
    }
    changed
}

/// Refits the table and culet heights of `design` to the size they have in `original`, as
/// Shift does. `true` when at least one flat got a different mast. A design that does not
/// analyse is left as it is (the gate reports it).
fn refit_table_and_culet(design: &mut Design, original: &StoneAnalysis) -> bool {
    let Ok(stone) = analyze(design, false) else {
        return false;
    };
    let refits = refit_flats(design, &stone.solved, &stone.flats, original, &|| false);
    for refit in &refits {
        design.tiers[refit.tier_index].constraint = MeetConstraint::ScaleReference(refit.new_mast);
    }
    !refits.is_empty()
}

/// The numbers of the stone that is delivered: `design` scored the way the optimizer reports
/// its results (full tilt scan, yield blended in, the shape penalty of "Keep the design's
/// look" when it is on), under the settings the search ran with.
fn delivered_numbers(
    inputs: &SearchInputs<'_>,
    original: &StoneAnalysis,
    design: &Design,
    stone: &StoneAnalysis,
) -> CandidateNumbers {
    let weights = inputs.settings.config(inputs.lighting).weights;
    let scored = score_finished_design(
        design,
        &stone.planes,
        &inputs.plan.target.gem,
        &weights,
        ObjectiveFidelity::Full,
        inputs.lighting,
        shape_target(inputs.settings, original),
    );
    CandidateNumbers::new(scored.after, scored.score, scored.yield_loss_pct)
}

/// One result of the optimizer as a candidate, or the reasons the gate refused it.
fn finish_candidate(
    inputs: &SearchInputs<'_>,
    original: &StoneAnalysis,
    start: &Design,
    hinges: &BTreeMap<usize, TierHinge>,
    found: &OptimizeCandidate,
    columns: &Columns,
    girdle_step: Option<f64>,
) -> Result<SearchCandidate, Vec<InvalidReason>> {
    let mut design = start.clone();
    for change in &found.changes {
        if let Some(tier) = design.tiers.get_mut(change.index) {
            tier.angle_deg = change.to_deg;
        }
    }
    for change in &found.mast_changes {
        if let Some(tier) = design.tiers.get_mut(change.index) {
            tier.constraint = MeetConstraint::ScaleReference(change.to_mast);
        }
    }
    fold_relations(&mut design)
        .map_err(|error| vec![InvalidReason::RelationFails(error.to_string())])?;
    let followers_moved = reanchor_followers(&mut design, start, hinges);
    let refit_moved = refit_table_and_culet(&mut design, original);

    let analysis = analyze(&design, false);
    let reasons = judge(inputs.design, original, &analysis);
    if !reasons.is_empty() {
        return Err(reasons);
    }
    // The optimizer scored the stone with its followers on their start masts and its table and
    // culet at their start heights. When a follower was re-anchored or a flat refitted the
    // delivered stone is a different one, so it is scored again; when neither moved (the
    // refit leaves a flat alone that is within tolerance) the optimizer's own numbers are
    // exactly the delivered stone's.
    let numbers = analysis
        .as_ref()
        .ok()
        .filter(|_| followers_moved || refit_moved)
        .map_or_else(
            || CandidateNumbers::new(found.after, found.score, found.yield_loss_pct),
            |stone| delivered_numbers(inputs, original, &design, stone),
        );
    Ok(SearchCandidate {
        kind: CandidateKind::Optimized,
        angles: angle_differences(inputs.design, &design),
        anchors: mast_differences(inputs.design, &design),
        validity: RetargetValidity::judged(
            original,
            &analysis,
            Vec::new(),
            RetargetStrategy::Optimized,
        )
        .with_girdle_thickened(girdle_step),
        metrics: columns.metrics(inputs, &analysis),
        numbers,
        design,
    })
}

/// Runs the whole search.
///
/// `cancel` is read between steps (and handed to the optimizer, which reads it once per tier
/// decision); `on_progress` hears the running evaluation count and the stage. Both are
/// called on the calling thread. The optimizer starts two short-lived threads of its own per
/// decision, so this call is meant for a worker thread.
///
/// # Errors
///
/// [`SearchError::Unusable`] when the live design does not solve and close,
/// [`SearchError::Solve`] when the start stone does not solve, [`SearchError::Cancelled`] when
/// `cancel` was raised.
pub fn run_search(
    inputs: &SearchInputs<'_>,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(usize, SearchStage),
) -> Result<SearchReport, SearchError> {
    let should_stop = || cancel.load(Ordering::Relaxed);
    let original = analyze(inputs.design, true).map_err(SearchError::Unusable)?;
    if should_stop() {
        return Err(SearchError::Cancelled);
    }

    let (start, start_point, shift_validity) = choose_start(inputs, &original, &should_stop)?;
    let start_analysis = start
        .analysis
        .as_ref()
        .map_err(|reason| SearchError::Unusable(reason.clone()))?;
    let hinges = tier_hinges(&start.design, &start_analysis.solved);
    let free = free_candidates(&start.design);
    let options = search_options(
        inputs,
        &original,
        &start.design,
        &free,
        &hinges,
        guard_fractions(&original, start_analysis),
    );
    let free_tiers = free_tier_indices_with(&start.design, &options).len();

    let config = inputs.settings.config(inputs.lighting);
    let hooks = SearchHooks {
        cancel: Some(cancel),
        on_progress: Some(on_progress),
        on_start: None,
    };
    let result = optimize_design_with(
        &start.design,
        &inputs.plan.target.gem,
        &config,
        &options,
        &hooks,
    )
    .map_err(|error| SearchError::Solve(error.to_string()))?;
    if should_stop() {
        return Err(SearchError::Cancelled);
    }

    let columns = Columns::measure(inputs, &original);
    let start_numbers = CandidateNumbers::new(
        result.outcome.before,
        result.outcome.before_score,
        result.outcome.before_yield_loss_pct,
    );
    let mut candidates = Vec::new();
    let start_kind = match start_point {
        StartPoint::Shift => Some(CandidateKind::Shift),
        StartPoint::Partial { percent } => Some(CandidateKind::Partial { percent }),
        StartPoint::Unchanged => None,
    };
    if let Some(kind) = start_kind {
        candidates.push(SearchCandidate {
            kind,
            angles: angle_differences(inputs.design, &start.design),
            anchors: mast_differences(inputs.design, &start.design),
            validity: RetargetValidity::judged(
                &original,
                &start.analysis,
                Vec::new(),
                start.strategy,
            )
            .with_girdle_thickened(start.girdle_step),
            metrics: columns.metrics(inputs, &start.analysis),
            numbers: start_numbers,
            design: start.design.clone(),
        });
    }
    let mut dropped = Vec::new();
    for found in &result.candidates {
        match finish_candidate(
            inputs,
            &original,
            &start.design,
            &hinges,
            found,
            &columns,
            start.girdle_step,
        ) {
            Ok(candidate) => candidates.push(candidate),
            Err(reasons) => dropped.push(reasons),
        }
    }
    candidates.sort_by(|a, b| a.numbers.score.total_cmp(&b.numbers.score));

    Ok(SearchReport {
        start: start_point,
        shift_validity,
        start_numbers,
        candidates,
        dropped,
        evaluations: result.outcome.evaluations,
        free_tiers,
        keep_look: inputs.settings.keep_look,
    })
}

#[cfg(test)]
mod tests;
