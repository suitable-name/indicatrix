//! Optimize's view models: the availability hint, the result SUMMARY table
//! ([`optimize_result_rows`]/[`optimize_status_text`]), the per-tier change table
//! ([`optimize_change_rows`]).
//!
//! The "Preview" candidate design ([`build_optimize_preview_design`]), the weight-form
//! parser ([`parse_optimize_weights`]), and the solved design's facet count
//! ([`facet_count_from_solved`]).
//!
//! The front end's pure logic lives in four submodules, re-exported here:
//! [`plan`] (the tab's state turned into a run, defaults, the time estimate),
//! [`ranges`] (the per-tier angle ranges), [`ranking`] (the ranked candidate rows and the
//! run's status sentence) and [`candidate_apply`] (previewing and applying one
//! candidate, relations included).

mod candidate_apply;
mod plan;
mod ranges;
mod ranking;

pub use candidate_apply::{apply_candidate, build_candidate_preview_design, candidate_edits};
pub use plan::{
    CUSTOM_PRESET_INDEX, DEFAULT_BUDGET, DEFAULT_CANDIDATES, DEFAULT_GIRDLE_FRACTION,
    DEFAULT_STARTS, MAX_CANDIDATES, MAX_STARTS, RunForm, RunPlan, build_run_plan,
    default_vary_anchored, estimate_run_seconds, estimate_text, fill_anchor_hinges,
    format_duration, measure_anchor_hinges, measured_ms_per_evaluation, optimize_availability,
    parse_budget, parse_seed, parse_starts, prepare_anchor_hinges, preset_description,
    preset_labels, weights_for_preset,
};
pub use ranges::{
    DEFAULT_RANGE_DEG, MIN_RANGE_ANGLE_DEG, RangeInput, RangeRow, default_range,
    format_range_value, parse_range, range_bounds, range_rows, range_summary,
};
pub use ranking::{
    CandidateLine, baseline_line, candidate_lines, candidate_outcome, optimize_run_status,
};

use crate::loading::{NumberExprError, eval_number};
use indicatrix::{
    color::metrics::ToneIlluminant,
    geometry::meet_solver::{MeetConstraint, SolvedTier},
    optics::{LightingPreset, materials::GemMaterial},
};
use indicatrix_cut_core::{
    Design, MaterialSelection, ObjectiveWeights, OptimizeCandidate, OptimizeOutcome,
    OptimizeResult, ToneGoal, free_tier_indices,
    optimize::{CANONICAL_LIGHT_PITCH, CANONICAL_LIGHT_YAW, SearchStage, StartProgress},
};
use std::collections::BTreeSet;

/// One row of the Optimize result table.
///
/// A metric's before figure, its after figure with a signed delta and a plain-English
/// verdict, and the direction the row is colored by (`1` better, `-1` worse, `0`
/// unchanged).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptimizeResultLine {
    /// The metric's label.
    pub label: String,
    /// The starting figure.
    pub before: String,
    /// The final figure plus its signed delta and verdict.
    pub after: String,
    /// `1` better, `-1` worse, `0` unchanged.
    pub direction: i32,
}

/// One row of the "Tiers this would change" table -- see [`optimize_change_rows`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptimizeChangeLine {
    /// `"#N"`, 1-based.
    pub tier_number: String,
    /// The tier's name, `""` for an index the design no longer has.
    pub name: String,
    /// The starting angle, as a magnitude.
    pub from_angle: String,
    /// The proposed angle, as a magnitude.
    pub to_angle: String,
    /// The signed difference of the two magnitudes: `+` is steeper, `-` is flatter.
    pub delta: String,
}

/// `1` when a change of `delta` is an improvement, `-1` when it is worse and `0` when it
/// is lost in rounding. `higher_is_better` is the metric's own polarity.
///
/// `delta.abs() < f32::EPSILON` rather than `delta == 0.0`: the figures are already
/// rounded measurements, and a difference below one ulp is not a change.
fn direction_of(delta: f32, higher_is_better: bool) -> i32 {
    if delta.abs() < f32::EPSILON {
        0
    } else if (higher_is_better && delta > 0.0) || (!higher_is_better && delta < 0.0) {
        1
    } else {
        -1
    }
}

/// Formats one objective component's "after" cell as the raw value plus a signed
/// delta and a plain-English verdict -- "9.25% (-3.25%, better)" rather than a bare
/// number the cutter has to subtract by hand and remember the polarity of.
/// `higher_is_better` distinguishes tilt brilliance (higher is better) from every
/// other component/the blended score (lower is better, see
/// [`ObjectiveWeights::score`]'s own doc comment).
#[must_use]
fn after_with_delta(before: f32, after: f32, higher_is_better: bool, unit: &str) -> (String, i32) {
    let delta = after - before;
    let direction = direction_of(delta, higher_is_better);
    let verdict = match direction {
        1 => "better",
        -1 => "worse",
        _ => "unchanged",
    };
    (
        format!("{after:.2}{unit} ({delta:+.2}{unit}, {verdict})"),
        direction,
    )
}

/// The Optimize result table's rows from a completed or cancelled run's
/// [`OptimizeOutcome`].
///
/// Windowing, extinction, tilt brilliance and yield loss each get their OWN row, and the
/// blended score comes last as a clearly separate row: an optimizer that improved
/// windowing by wrecking extinction must be visibly doing that, never collapsed into a
/// single figure.
///
/// The "Yield loss" row is shown UNCONDITIONALLY, even at a zero yield weight --
/// `before_yield_loss_pct`/`after_yield_loss_pct` are real measurements of the
/// starting/final design either way.
#[must_use]
pub fn optimize_result_rows(outcome: &OptimizeOutcome) -> Vec<OptimizeResultLine> {
    vec![
        metric_row(
            "Windowing",
            outcome.before.windowing_pct,
            outcome.after.windowing_pct,
            false,
            "%",
        ),
        metric_row(
            "Extinction",
            outcome.before.extinction_pct,
            outcome.after.extinction_pct,
            false,
            "%",
        ),
        metric_row(
            "Tilt brilliance",
            outcome.before.tilt_brilliance_pct,
            outcome.after.tilt_brilliance_pct,
            true,
            "%",
        ),
        metric_row(
            "Yield loss",
            outcome.before_yield_loss_pct,
            outcome.after_yield_loss_pct,
            false,
            "%",
        ),
        metric_row(
            "Blended score",
            outcome.before_score,
            outcome.after_score,
            false,
            "",
        ),
    ]
}

/// One row of [`optimize_result_rows`]. `higher_is_better` is the metric's own
/// polarity, not a property of the numbers: windowing and extinction going DOWN is
/// an improvement, tilt brilliance going up is.
fn metric_row(
    label: &str,
    before: f32,
    after: f32,
    higher_is_better: bool,
    unit: &str,
) -> OptimizeResultLine {
    let (after_text, direction) = after_with_delta(before, after, higher_is_better, unit);
    OptimizeResultLine {
        label: label.to_string(),
        before: format!("{before:.2}{unit}"),
        after: after_text,
        direction,
    }
}

/// The one-line summary shown above [`optimize_result_rows`]'s table.
///
/// How many tiers changed and how many candidate evaluations it took, plus (only when
/// true) the cancellation note and (only when the polish stage actually ran) how much of
/// the final score it is responsible for.
#[must_use]
pub fn optimize_status_text(outcome: &OptimizeOutcome) -> String {
    let cancelled_note = cancelled_note(outcome);
    let polish_note = polish_note(outcome);
    if outcome.changes.is_empty() {
        format!(
            "Optimize found no improving move in {} evaluation(s) -- this design's \
             free tiers were already at (or very near) a local optimum for these \
             weights.{cancelled_note}{polish_note}",
            outcome.evaluations
        )
    } else {
        format!(
            "Optimize changed {} tier(s) in {} evaluation(s).{cancelled_note}{polish_note}",
            outcome.changes.len(),
            outcome.evaluations
        )
    }
}

/// The note a cancelled run's status carries, `""` for a finished one.
const fn cancelled_note(outcome: &OptimizeOutcome) -> &'static str {
    if outcome.cancelled {
        " (cancelled -- showing the best partial result found before the checkpoint \
         fired)"
    } else {
        ""
    }
}

/// The note on what the polish stage added, `""` when it did not run.
fn polish_note(outcome: &OptimizeOutcome) -> String {
    if outcome.polish_evaluations > 0 {
        format!(
            " (polish: +{:.2} in {} evaluation(s))",
            outcome.polish_improvement, outcome.polish_evaluations
        )
    } else {
        String::new()
    }
}

/// A clone of `design` with every one of `outcome`'s angle changes already applied
/// -- the candidate a "Preview" toggle shows BEFORE the cutter commits to Apply.
///
/// Never touches `History`/`Edit` at all: this is a display-only candidate, never
/// something an Undo could need to unwind. A change naming a tier the design no
/// longer has is skipped.
#[must_use]
pub fn build_optimize_preview_design(design: &Design, outcome: &OptimizeOutcome) -> Design {
    let mut preview = design.clone();
    for change in &outcome.changes {
        if let Some(tier) = preview.tiers.get_mut(change.index) {
            tier.angle_deg = change.to_deg;
        }
    }
    preview
}

/// Parses the Optimize weight form's three text fields (plus the yield slider's own
/// already-numeric `0..1` value and the tone slider's signed `-3..=3` value) into an
/// [`ObjectiveWeights`].
///
/// `tone` is signed: negative asks for a lighter stone face-up ([`ToneGoal::Lighter`]),
/// positive for a stronger colour ([`ToneGoal::Deeper`]), the magnitude is the weight and
/// `0` leaves the tone out (bit-identical results). A non-finite value is an error.
///
/// The three text fields
/// must parse, be finite, and be non-negative: only the RATIOS between them matter,
/// so a negative one would silently invert that component's polarity.
///
/// A field may hold arithmetic (`1 + 0.5`, `4 / 2`) like every number field: text the
/// plain `f32` parse accepts is read exactly as before (so the weights, and with them
/// a run's result, stay bit-identical), anything else goes through
/// [`crate::loading::eval_number`].
///
/// # Errors
///
/// A message naming the offending field.
pub fn parse_optimize_weights(
    windowing: &str,
    extinction: &str,
    tilt_brilliance: &str,
    yield_weight: f32,
    tone: f32,
) -> Result<ObjectiveWeights, String> {
    fn parse_weight(label: &str, text: &str) -> Result<f32, String> {
        let trimmed = text.trim();
        let value: f32 = match trimmed.parse::<f32>() {
            Ok(value) => value,
            Err(_) => match eval_number(trimmed, None) {
                Ok(value) => value as f32,
                Err(NumberExprError::NotANumber) => {
                    return Err(format!("{label} weight must be a number."));
                }
                Err(NumberExprError::Invalid(reason)) => {
                    return Err(format!(
                        "{label} weight '{trimmed}' cannot be calculated: {reason}."
                    ));
                }
            },
        };
        if !value.is_finite() || value < 0.0 {
            return Err(format!(
                "{label} weight must be a non-negative, finite number."
            ));
        }
        Ok(value)
    }
    if !tone.is_finite() {
        return Err("Tone must be a number.".to_string());
    }
    Ok(ObjectiveWeights {
        windowing: parse_weight("Windowing", windowing)?,
        extinction: parse_weight("Extinction", extinction)?,
        tilt_brilliance: parse_weight("Tilt brilliance", tilt_brilliance)?,
        yield_weight,
        tone_weight: tone.abs(),
        tone_goal: if tone > 0.0 {
            ToneGoal::Deeper
        } else {
            ToneGoal::Lighter
        },
    })
}

/// The signed tone slider value (`-3..=3`) a set of weights stands for.
///
/// The inverse of [`parse_optimize_weights`]'s `tone` argument. Negative is [`ToneGoal::Lighter`],
/// positive is [`ToneGoal::Deeper`], the magnitude is the weight, and `0.0` is "off".
#[must_use]
pub fn signed_tone(weights: &ObjectiveWeights) -> f32 {
    if weights.tone_weight <= 0.0 || !weights.tone_weight.is_finite() {
        return 0.0;
    }
    match weights.tone_goal {
        ToneGoal::Lighter => -weights.tone_weight,
        ToneGoal::Deeper => weights.tone_weight,
    }
}

/// Whether `material` has any body colour: some absorption band of some set with a peak
/// above zero. A colourless material gives the tone objective nothing to work on.
#[must_use]
pub fn material_has_body_color(material: &GemMaterial) -> bool {
    let tensor = &material.absorption;
    let sets = [
        Some(&tensor.o_ray),
        Some(&tensor.e_ray),
        tensor.beta_ray.as_ref(),
    ];
    sets.into_iter()
        .flatten()
        .any(|bands| bands.iter().any(|band| band.peak > 0.0))
}

/// The sentence under the tone slider: what the tone objective will and will not do for
/// this design, material and lighting.
///
/// A material without body colour gets a refusal ("nothing to work on"). Otherwise the
/// note says how the stone is sized (the design's girdle diameter, or the
/// swatch convention of one model unit per unit of colour strength) and under which light
/// the tone is measured (the lighting preset's own, or daylight where a UV lamp has no
/// visible white).
#[must_use]
pub fn tone_scale_note(
    design: &Design,
    material: &GemMaterial,
    lighting: LightingPreset,
) -> String {
    if !material_has_body_color(material) {
        return "The material has no body colour, so the tone objective has nothing to work on. \
                Pick a colour in Design settings."
            .to_string();
    }
    let size = match design.girdle_diameter_mm {
        Some(mm) if mm > 0.0 && mm.is_finite() => {
            format!("Face-up colour is worked out for a {mm:.1} mm stone (Design settings)")
        }
        _ => "No stone size set: one model unit of path counts as one unit of colour strength, \
              the swatch convention; set the girdle diameter in Design settings to size it"
            .to_string(),
    };
    let environment = lighting.studio(1.0, CANONICAL_LIGHT_YAW, CANONICAL_LIGHT_PITCH);
    let light = if ToneIlluminant::for_environment(environment).uses_preset_light() {
        format!(", under {} (the Live Render's lighting).", lighting.label())
    } else {
        ", in daylight (D65): the UV lamp has no visible white, so the tone uses daylight."
            .to_string()
    };
    format!("{size}{light}")
}

/// The swatch caption for the light the tone was measured under: "under Incandescent
/// (3200K)", or "in daylight (D65) -- UV lamp" where a UV lamp fell back to daylight.
#[must_use]
pub fn tone_lighting_label(lighting: LightingPreset) -> String {
    let environment = lighting.studio(1.0, CANONICAL_LIGHT_YAW, CANONICAL_LIGHT_PITCH);
    if ToneIlluminant::for_environment(environment).uses_preset_light() {
        format!("under {}", lighting.label())
    } else {
        "in daylight (D65) -- UV lamp".to_string()
    }
}

/// The two face-up tone rows of the result table for `candidate` against the start:
/// "Face-up lightness L*" and "Face-up colour strength C*".
///
/// Only the figure the run's goal pulls on is judged better or worse (lighter: L*, deeper:
/// C*); the other row, and both rows of a run whose tone was not weighted, show the change
/// without a verdict (direction `0`). Empty when either tone is missing.
#[must_use]
pub fn tone_result_rows(
    result: &OptimizeResult,
    candidate: &OptimizeCandidate,
) -> Vec<OptimizeResultLine> {
    let (Some(before), Some(after)) = (result.tone_before, candidate.tone) else {
        return Vec::new();
    };
    let (lightness_judged, chroma_judged) = match result.tone_goal {
        Some(ToneGoal::Lighter) => (Some(true), None),
        Some(ToneGoal::Deeper) => (None, Some(true)),
        None => (None, None),
    };
    vec![
        tone_row(
            "Face-up lightness L*",
            before.l_star,
            after.l_star,
            lightness_judged,
        ),
        tone_row(
            "Face-up colour strength C*",
            before.chroma,
            after.chroma,
            chroma_judged,
        ),
    ]
}

/// One tone row: judged like [`metric_row`] when `higher_is_better` is given, else the
/// plain figure and signed change with direction `0`.
fn tone_row(
    label: &str,
    before: f32,
    after: f32,
    higher_is_better: Option<bool>,
) -> OptimizeResultLine {
    higher_is_better.map_or_else(
        || OptimizeResultLine {
            label: label.to_string(),
            before: format!("{before:.2}"),
            after: format!("{after:.2} ({:+.2})", after - before),
            direction: 0,
        },
        |higher| metric_row(label, before, after, higher, ""),
    )
}

/// The tier name to show alongside a tier-index reference in a result table: what the tier
/// table shows, so the tier's own name, or its standard code (`P1`, `C2`) when the name is
/// empty or old-style (`1`, `A`).
///
/// The index is a position in `design.tiers`: Optimize's variables are flat tiers only
/// (a concave tier has no critical-angle margin to optimise), even though its objective
/// is evaluated on the whole stone, tools included.
///
/// `""` (never a placeholder) for an out-of-range index, since a design edited between
/// when a background search started and when its result landed can shrink the tier list
/// out from under a stale row.
#[must_use]
pub fn tier_name_for_row(design: &Design, tier_index: usize) -> String {
    crate::retarget::plan::tier_display_names(design)
        .into_iter()
        .nth(tier_index)
        .unwrap_or_default()
}

/// One [`OptimizeChangeLine`] per angle change a pending Optimize result would apply.
///
/// The per-tier table behind [`optimize_result_rows`]'s aggregate rows, so a cutter can
/// see WHICH tiers move, and by how much, before clicking Apply.
///
/// Both angles are magnitudes (a pavilion tier stored at -40 reads `40.00°`; the side of the
/// girdle is the tier's). The change is the difference of those two numbers with an explicit
/// sign, so `+1.50°` is a tier that got 1.5 degrees steeper whichever side it is on.
#[must_use]
pub fn optimize_change_rows(outcome: &OptimizeOutcome, design: &Design) -> Vec<OptimizeChangeLine> {
    let names = crate::retarget::plan::tier_display_names(design);
    outcome
        .changes
        .iter()
        .map(|change| OptimizeChangeLine {
            tier_number: format!("#{}", change.index + 1),
            name: names.get(change.index).cloned().unwrap_or_default(),
            from_angle: format!("{:.2}\u{b0}", change.from_deg.abs()),
            to_angle: format!("{:.2}\u{b0}", change.to_deg.abs()),
            delta: format!("{:+.2}\u{b0}", change.to_deg.abs() - change.from_deg.abs()),
        })
        .collect()
}

/// The design's total facet count after gear/symmetry expansion: the length of
/// [`Design::planes_from_solved`] minus the preform's own bounding planes (which it
/// always prepends).
///
/// An uncut preform has zero facets, not the preform's plane count.
#[must_use]
pub fn facet_count_from_solved(design: &Design, solved: &[SolvedTier]) -> usize {
    design
        .planes_from_solved(solved)
        .len()
        .saturating_sub(design.preform.planes_offset(design.preform_y_offset).len())
}

/// Defaults an Optimize run's material selection to the design's effective RI.
///
/// `selection` is left unchanged when it already names a material or carries an RI
/// override; otherwise `refractive_index_override` is set to the design's effective RI --
/// otherwise `MaterialSelection::resolve` / `resolved_gem_material` silently fall back to
/// diamond (`n_D` 2.42), scoring the search against the wrong RI with nothing on screen
/// saying so. Returns the defaulted RI only when a default was actually applied, so a caller
/// can fold it into the initial status line via [`optimize_start_status`].
///
/// Reads `design.effective_refractive_index_with(custom)` -- catalogue-aware, so a
/// design on a CUSTOM material defaults to that material's own real `n_D` -- rather than
/// the built-ins-only `Design::effective_refractive_index()`, which silently falls
/// through to the legacy schedule RI for any design naming a custom catalogue material.
///
/// Moved from the desktop's `solve_actions::optimize_run`.
#[must_use]
pub fn default_optimize_material_ri(
    design: &Design,
    selection: &mut MaterialSelection,
    custom: &[GemMaterial],
) -> Option<f64> {
    if selection.name.is_some() || selection.refractive_index_override.is_some() {
        return None;
    }
    let n_d = design.effective_refractive_index_with(custom);
    selection.refractive_index_override = Some(n_d);
    Some(n_d)
}

/// A clone of `design` with every free tier outside `keep_free` pinned at its mast.
///
/// Every FREE tier (`indicatrix_cut_core::free_tier_indices`) NOT in `keep_free` is pinned
/// to a [`MeetConstraint::ScaleReference`] at its own CURRENT solved mast -- so
/// `optimize_design`, which only ever moves a tier `free_tier_indices` names, cannot
/// touch it. Every tier already in `keep_free`, and every tier that was already pinned,
/// is left exactly as it was.
///
/// Never removes or reorders a tier, so the returned design's `AngleChange::index`
/// values from a search run against it stay valid against the CALLER's own original
/// design with no translation needed. `solved` must have one entry per `design.tiers` in
/// the same order (a shorter list leaves the tiers past its end alone, never a panic).
///
/// Moved from the desktop's `solve_actions::optimize_run`.
#[must_use]
pub fn pin_non_selected_free_tiers(
    design: &Design,
    solved: &[SolvedTier],
    keep_free: &BTreeSet<usize>,
) -> Design {
    let mut restricted = design.clone();
    for index in free_tier_indices(design) {
        if keep_free.contains(&index) {
            continue;
        }
        let Some(mast) = solved.get(index).map(|s| s.mast) else {
            continue;
        };
        if let Some(tier) = restricted.tiers.get_mut(index) {
            tier.constraint = MeetConstraint::ScaleReference(mast);
        }
    }
    restricted
}

/// The initial "Optimizing..." status line shown before the first progress tick arrives.
///
/// Names the defaulted RI (see [`default_optimize_material_ri`]) when one was applied so
/// the assumption is visible from the very first frame.
///
/// Moved from the desktop's `solve_actions::optimize_run`.
#[must_use]
pub fn optimize_start_status(defaulted_ri: Option<f64>) -> String {
    defaulted_ri.map_or_else(
        || "Optimizing... 0 evaluations, 0.0s elapsed".to_string(),
        |n_d| {
            format!(
                "Optimizing (no material set -- scored for n_d={n_d:.4})... 0 \
                 evaluations, 0.0s elapsed"
            )
        },
    )
}

/// The running "Optimizing..." status line for one progress tick.
///
/// Names `stage` explicitly instead of always showing an evaluation fraction: the two
/// full-fidelity
/// scorings that bracket every run report zero-progress ticks of their own, which would
/// otherwise leave the counter frozen for over a second each, reading as a hang. The
/// coordinate and polish stages show `max_evaluations` (a single combined figure, the
/// coordinate cap plus the polish stage's own, `inclusive_max_evaluations`) so the polish
/// stage's evaluations climbing does not read as sailing past the run's own stated
/// budget.
///
/// `start` is the last [`StartProgress`] of a run with several starts (`None` for a single
/// start): the coordinate and polish lines then read "start 3 of 8, best 12.41, 412 of ~983
/// evaluations". `max_evaluations` must then be `inclusive_max_evaluations_for`.
///
/// Moved from the desktop's `solve_actions::optimize_run`.
#[must_use]
pub fn optimize_progress_status(
    stage: SearchStage,
    evaluations: usize,
    max_evaluations: usize,
    start: Option<StartProgress>,
    elapsed_secs: f32,
) -> String {
    // "start 3 of 8, best 12.41, " -- the start index is zero-based in the search, one-based here.
    let which = start.map_or_else(String::new, |start| {
        format!(
            "start {} of {}, best {:.2}, ",
            start.index + 1,
            start.count,
            start.best_fast_score
        )
    });
    match stage {
        SearchStage::BaselineFull => {
            format!(
                "Optimizing... scoring the starting point at full fidelity, {elapsed_secs:.1}s elapsed"
            )
        }
        SearchStage::Coordinate => format!(
            "Optimizing... {which}{evaluations} of ~{max_evaluations} evaluations, {elapsed_secs:.1}s elapsed"
        ),
        SearchStage::Polish => format!(
            "Optimizing (polish)... {which}{evaluations} of ~{max_evaluations} evaluations, {elapsed_secs:.1}s elapsed"
        ),
        SearchStage::FinalFull => {
            format!("Optimizing... scoring the result at full fidelity, {elapsed_secs:.1}s elapsed")
        }
        // The draws are `min(64, 4 * (starts - 1))`; a caller that knows the planned number of
        // starts passes it as `start.count` to get the "of" form.
        SearchStage::Screening => start.map_or_else(
            || {
                format!(
                    "Optimizing... screening starting points, {evaluations} drawn, {elapsed_secs:.1}s elapsed"
                )
            },
            |start| {
                format!(
                    "Optimizing... screening {evaluations} of {} starting points, {elapsed_secs:.1}s elapsed",
                    (4 * start.count.saturating_sub(1)).min(64)
                )
            },
        ),
    }
}

/// Whether Optimize is available for `design`, and the explanatory hint shown next
/// to its button either way.
///
/// Genuinely unavailable (not merely caveated) when
/// nothing is free to move: `free_tier_indices` empty means `optimize_design` would
/// return its input unchanged. A design fresh off the catalogue pins EVERY tier on
/// import, which is correct and expected -- the hint says so rather than leaving a
/// disabled button to read as broken.
///
/// `max_evaluations` is the CONFIGURED coordinate-stage budget, quoted so the hint
/// reflects what a run would actually use. The hint also names the fixed canonical
/// light pose every evaluation scores tilt brilliance under -- not the light the
/// viewport shows after the cutter drags it.
#[must_use]
pub fn optimize_hint(design: &Design, max_evaluations: usize) -> (bool, String) {
    let free = free_tier_indices(design);
    if free.is_empty() {
        (
            false,
            "Every tier is currently pinned as a scale reference -- a freshly \
             imported design starts this way, and that is correct, not broken. \
             Optimize has nothing free to move until you adopt at least one tier's \
             real meet constraint (the tier list's Adopt action) or author one by \
             hand."
                .to_string(),
        )
    } else {
        (
            true,
            format!(
                "Coordinate search over {} free tier angle(s), scored on windowing, \
                 extinction, and tilt brilliance, and the face-up tone when a tone \
                 objective is chosen, under a FIXED canonical light pose \
                 (not necessarily the light you see in the viewport right now -- drag \
                 the light and these figures can disagree with the trace/HUD until you \
                 re-run Optimize). Roughly 7 ms per evaluation on a small design, but \
                 up to several seconds each on a large, heavily meet-derived one (a \
                 {max_evaluations}-evaluation budget can then take minutes) -- plus two \
                 fixed full-fidelity scorings (one before, one after the search) that \
                 can each take over a second on their own, so even a fast run has some \
                 up-front and trailing wait beyond the quoted per-evaluation cost. With \
                 several starts the search also tries other starting arrangements inside \
                 your angle ranges, shares the budget between them, and keeps the best; a \
                 design or budget too small to afford them uses one start. Runs \
                 off the UI thread and can be cancelled.",
                free.len()
            ),
        )
    }
}

#[cfg(test)]
mod tests;
