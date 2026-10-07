//! Turning the Optimize tab's state into a run: the objective preset, what may change,
//! the budget and seed, the number of candidates, and how long it will take.
//!
//! Everything here is a plain function of plain values, so the tab's rules (a preset
//! beats the weight boxes, anchored tiers are varied by default only when nothing else
//! is free, a driven tier is never offered) are tested without a window.

use super::{
    optimize_hint, parse_optimize_weights,
    ranges::{RangeInput, range_bounds, range_rows},
};
use crate::loading::eval_number;
use glam::DVec3;
use indicatrix::{geometry::meet_solver::SolvedTier, optics::LightingPreset};
use indicatrix_cut_core::{
    Design, DesignSolveError, ObjectivePreset, ObjectiveWeights, OptimizeConfig, OptimizeOptions,
    design::tier_hinge_points,
    free_tier_indices,
    optimize::{effective_starts, inclusive_max_evaluations_for},
};
use std::collections::{BTreeMap, BTreeSet};

/// The combo box position of "Custom", after the presets.
pub const CUSTOM_PRESET_INDEX: usize = ObjectivePreset::ALL.len();

/// How many candidates a run keeps unless the cutter changes it.
pub const DEFAULT_CANDIDATES: usize = 3;

/// The most candidates a run keeps. Each one costs another full-quality scoring.
pub const MAX_CANDIDATES: usize = 5;

/// The least a candidate may keep of the starting girdle's thickness when "Keep the
/// girdle" is on: half.
pub const DEFAULT_GIRDLE_FRACTION: f64 = 0.5;

/// The evaluation budget of a run unless the cutter changes it.
pub const DEFAULT_BUDGET: usize = 800;

/// How many starting arrangements a run tries unless the cutter changes it. The search
/// lowers it by itself when the budget or the design is too small to afford them.
pub const DEFAULT_STARTS: usize = 8;

/// The most starting arrangements a run may ask for.
pub const MAX_STARTS: usize = 32;

/// The largest evaluation budget a field may hold; more is a typing slip.
const MAX_BUDGET: usize = 100_000;

/// Two candidates count as different when some angle differs by at least this many
/// degrees. Wider than the search's finest step so the list offers real alternatives and
/// not the same stone four times.
const CANDIDATE_SEPARATION_DEG: f64 = 0.5;

/// A design with this many tiers costs [`SMALL_DESIGN_EVALUATION_MS`] per evaluation.
const SMALL_DESIGN_TIERS: f64 = 12.0;

/// Measured cost of one search evaluation on a small design (12 tiers), in milliseconds.
const SMALL_DESIGN_EVALUATION_MS: f64 = 7.0;

/// Measured cost of one full-quality scoring, in seconds. A run pays it once for the
/// starting stone and once per candidate.
const FULL_SCORING_SECONDS: f64 = 1.3;

/// The names of the combo box entries: the presets, then "Custom".
#[must_use]
pub fn preset_labels() -> Vec<&'static str> {
    ObjectivePreset::ALL
        .into_iter()
        .map(ObjectivePreset::label)
        .chain(["Custom"])
        .collect()
}

/// The one-line description under the combo box for the entry at `index`.
#[must_use]
pub const fn preset_description(index: usize) -> &'static str {
    if index < CUSTOM_PRESET_INDEX {
        ObjectivePreset::from_index(index).description()
    } else {
        "Custom: set the three weights, the yield weight and the tone yourself."
    }
}

/// The weights of the preset at `index`; `None` for "Custom", whose weights are typed.
#[must_use]
pub fn weights_for_preset(index: usize) -> Option<ObjectiveWeights> {
    (index < CUSTOM_PRESET_INDEX).then(|| ObjectivePreset::from_index(index).weights())
}

/// Whether "Vary anchored tiers" starts ticked for `design`.
///
/// It is ticked only when nothing else is free to move, which is every freshly imported
/// design (import pins every tier). A design with free tiers keeps today's behaviour,
/// moving only those.
#[must_use]
pub fn default_vary_anchored(design: &Design) -> bool {
    free_tier_indices(design).is_empty()
}

/// Reads the evaluation budget field. Arithmetic is allowed (`100 * 4`); a blank field
/// means [`DEFAULT_BUDGET`].
///
/// # Errors
///
/// A message when the text is not a number, is below 1, or is above 100000.
pub fn parse_budget(text: &str) -> Result<usize, String> {
    if text.trim().is_empty() {
        return Ok(DEFAULT_BUDGET);
    }
    let value = eval_number(text, None).map_err(|error| error.message("Budget", text))?;
    if !value.is_finite() || value < 0.5 {
        return Err("Budget must be at least 1 evaluation.".to_string());
    }
    if value > MAX_BUDGET as f64 {
        return Err(format!("Budget is at most {MAX_BUDGET} evaluations."));
    }
    Ok(value.round() as usize)
}

/// Reads the starts field: a whole number from 1 to [`MAX_STARTS`]. Arithmetic is allowed
/// (`2 * 4`); a blank field means [`DEFAULT_STARTS`].
///
/// # Errors
///
/// A message when the text is not a number, is not whole, or is outside 1 to 32.
pub fn parse_starts(text: &str) -> Result<usize, String> {
    if text.trim().is_empty() {
        return Ok(DEFAULT_STARTS);
    }
    let value = eval_number(text, None).map_err(|error| error.message("Starts", text))?;
    if !value.is_finite() || value.fract().abs() > 1e-9 || value < 0.5 {
        return Err("Starts must be a whole number, 1 or more.".to_string());
    }
    if value > MAX_STARTS as f64 + 0.5 {
        return Err(format!("Starts is at most {MAX_STARTS}."));
    }
    Ok(value.round() as usize)
}

/// Reads the seed field: a whole number, 0 or more. Arithmetic is allowed; a blank field
/// means 0.
///
/// # Errors
///
/// A message when the text is not a number or not a whole number of 0 or more.
pub fn parse_seed(text: &str) -> Result<u64, String> {
    if text.trim().is_empty() {
        return Ok(0);
    }
    let value = eval_number(text, None).map_err(|error| error.message("Seed", text))?;
    if !value.is_finite() || value < 0.0 || value.fract().abs() > 1e-9 || value > 9.0e15 {
        return Err("Seed must be a whole number, 0 or more.".to_string());
    }
    Ok(value as u64)
}

/// The Optimize tab's state when "Optimize" is clicked, as plain values.
#[derive(Debug, Clone, Copy)]
pub struct RunForm<'a> {
    /// The combo box position: a preset, or [`CUSTOM_PRESET_INDEX`].
    pub preset_index: usize,
    /// The windowing weight box (read only for "Custom").
    pub weight_windowing: &'a str,
    /// The extinction weight box (read only for "Custom").
    pub weight_extinction: &'a str,
    /// The tilt brilliance weight box (read only for "Custom").
    pub weight_tilt_brilliance: &'a str,
    /// The yield weight slider (read only for "Custom").
    pub yield_weight: f32,
    /// The tone slider, signed `-3..=3`: negative lighter, positive deeper, `0` off
    /// (read only for "Custom"; see [`parse_optimize_weights`]).
    pub tone: f32,
    /// "Vary anchored tiers": move tiers that are pinned to a scale value too.
    pub vary_anchored: bool,
    /// "Keep the girdle": reject a candidate that thins the girdle below half.
    pub keep_girdle: bool,
    /// The budget box.
    pub budget_text: &'a str,
    /// "Starts": how many starting arrangements to try, clamped to at least 1 and at most [`MAX_STARTS`].
    pub starts: usize,
    /// The seed box.
    pub seed_text: &'a str,
    /// "Polish".
    pub polish: bool,
    /// "Candidates".
    pub candidates: usize,
    /// The ranges the cutter edited; a tier without an entry uses its default range.
    pub ranges: &'a [RangeInput],
}

/// What a run needs, before the hinges are known.
#[derive(Debug, Clone, PartialEq)]
pub struct RunPlan {
    /// The search knobs: weights, budget, seed, polish.
    pub config: OptimizeConfig,
    /// What may change and what is kept: anchored tiers, ranges, the girdle guard, the
    /// number of candidates. `anchor_hinges` is empty: a hinge needs a solved design, so
    /// [`fill_anchor_hinges`] fills it on the worker.
    pub options: OptimizeOptions,
    /// How many tiers the run may move (the ranges table's rows that follow no
    /// relation).
    pub movable_tiers: usize,
}

/// Builds the run for `form` against `design`.
///
/// A preset beats the weight boxes (they are not even read); "Custom" reads them through
/// [`parse_optimize_weights`]. Every tier the run may move gets an angle range, the
/// default five degrees either side unless the cutter edited it; a tier that follows a
/// relation gets none and is never moved.
///
/// # Errors
///
/// A message naming the first field that cannot be read: a weight, the budget, the seed,
/// or an angle range.
pub fn build_run_plan(
    design: &Design,
    form: &RunForm<'_>,
    lighting: LightingPreset,
) -> Result<RunPlan, String> {
    let weights = match weights_for_preset(form.preset_index) {
        Some(weights) => weights,
        None => parse_optimize_weights(
            form.weight_windowing,
            form.weight_extinction,
            form.weight_tilt_brilliance,
            form.yield_weight,
            form.tone,
        )?,
    };
    let max_evaluations = parse_budget(form.budget_text)?;
    let seed = parse_seed(form.seed_text)?;
    let angle_bounds = range_bounds(design, form.vary_anchored, form.ranges)?;
    let mut config = OptimizeConfig {
        weights,
        seed,
        max_evaluations,
        starts: form.starts.clamp(1, MAX_STARTS),
        lighting,
        ..OptimizeConfig::default()
    };
    if !form.polish {
        config.polish_start_step_deg = None;
    }
    let movable_tiers = range_rows(design, form.vary_anchored)
        .iter()
        .filter(|row| !row.driven)
        .count();
    let options = OptimizeOptions {
        vary_anchored: form.vary_anchored,
        anchor_hinges: BTreeMap::new(),
        angle_bounds,
        min_girdle_fraction: form.keep_girdle.then_some(DEFAULT_GIRDLE_FRACTION),
        // The form has one "keep the girdle" choice: the thinnest point takes the same fraction.
        min_girdle_thinnest_fraction: None,
        keep_candidates: form.candidates.clamp(1, MAX_CANDIDATES),
        candidate_separation_deg: Some(CANDIDATE_SEPARATION_DEG),
        // The Optimize tab has no "keep the look" choice: it ranks by the objective alone.
        shape_target: None,
    };
    Ok(RunPlan {
        config,
        options,
        movable_tiers,
    })
}

/// Puts the hinges a run varies anchored tiers about into `options`.
///
/// `hinges` is `tier_hinge_points` of the solved design. A tier that follows a relation
/// gets none (it is never moved), and with `only` (the tiers selected in the tier table)
/// only those tiers keep one, so "Only selected tiers" also holds for anchored tiers.
pub fn fill_anchor_hinges(
    options: &mut OptimizeOptions,
    design: &Design,
    hinges: &BTreeMap<usize, DVec3>,
    only: Option<&BTreeSet<usize>>,
) {
    options.anchor_hinges = hinges
        .iter()
        .filter(|(index, _)| !design.is_tier_driven(**index))
        .filter(|(index, _)| only.is_none_or(|selected| selected.contains(*index)))
        .map(|(index, hinge)| (*index, *hinge))
        .collect();
}

/// Measures the hinges of the solved `design` and puts them into `options`.
///
/// A hinge is the vertex of each tier's first facet that [`tier_hinge_points`] picks, filtered
/// by [`fill_anchor_hinges`]'s rules (no hinge for a tier that follows a relation; only `only`
/// when it is given).
///
/// This is the one place the desktop's Optimize tab and the command line's `optimize` get
/// their hinges from. It does not look at [`OptimizeOptions::vary_anchored`]; see
/// [`prepare_anchor_hinges`] for the version that does.
pub fn measure_anchor_hinges(
    options: &mut OptimizeOptions,
    design: &Design,
    solved: &[SolvedTier],
    only: Option<&BTreeSet<usize>>,
) {
    let hinges = tier_hinge_points(design, solved);
    fill_anchor_hinges(options, design, &hinges, only);
}

/// Solves `design` and measures the hinges a run varying anchored tiers needs
/// ([`measure_anchor_hinges`]).
///
/// Does nothing, and does not solve, when `options.vary_anchored` is off. A caller that has
/// the solve already calls [`measure_anchor_hinges`] with it instead.
///
/// # Errors
///
/// [`DesignSolveError`] when the design does not solve.
pub fn prepare_anchor_hinges(
    design: &Design,
    options: &mut OptimizeOptions,
    only: Option<&BTreeSet<usize>>,
) -> Result<(), DesignSolveError> {
    if !options.vary_anchored {
        return Ok(());
    }
    let solved = design.solve()?;
    measure_anchor_hinges(options, design, &solved, only);
    Ok(())
}

/// Whether Optimize is available for `design` with the tab's current "Vary anchored tiers"
/// setting, and the hint for its button.
///
/// With the setting off this is [`optimize_hint`] (with one added sentence for a design
/// that has nothing free). With it on, a design is available when some tier can move at
/// all, even though every tier is pinned.
#[must_use]
pub fn optimize_availability(
    design: &Design,
    max_evaluations: usize,
    vary_anchored: bool,
) -> (bool, String) {
    let (available, hint) = optimize_hint(design, max_evaluations);
    if available {
        return (true, hint);
    }
    let movable = range_rows(design, vary_anchored)
        .iter()
        .filter(|row| !row.driven)
        .count();
    if vary_anchored && movable > 0 {
        return (
            true,
            format!(
                "Turns {movable} tier(s) about the edge each keeps on the girdle, scored on \
                 windowing, extinction and tilt brilliance, and the face-up tone when a tone \
                 objective is chosen, under a fixed canonical light pose. It runs in the \
                 background and can be cancelled."
            ),
        );
    }
    (
        false,
        format!(
            "{hint} Or tick \"Vary anchored tiers\" in the Optimize tab to turn them about \
             their girdle edges."
        ),
    )
}

/// How long a run takes, in seconds, as a rough estimate.
///
/// `evaluations` is the whole search budget (both stages), `candidates` the number of
/// candidates kept: the starting stone and every candidate are scored once at full
/// quality (about 1.3 s each on any design). One evaluation costs 7 ms on a small design
/// (12 tiers) and grows with the square of the tier count, which is what the solve
/// costs; `measured_ms_per_evaluation`, the rate of an earlier run on this design,
/// replaces that guess.
///
/// `starts` is the number of starting arrangements asked for. With more than one, the search
/// also spends screening draws and a polish per kept start
/// (`inclusive_max_evaluations_for`), and runs the starts on several threads: the guessed
/// per-evaluation cost is divided by `min(starts, cores / 2)`. `free_tiers` is the number
/// of free (movable) tiers the run searches, which the starts rule needs (a budget under
/// eight sweeps per tier leaves one start); `tier_count` is the whole design and only
/// scales the per-evaluation guess. A `measured_ms_per_evaluation` is already a wall-clock rate (see
/// [`measured_ms_per_evaluation`]), so no thread divisor is applied to it. With one start
/// the figure is the same as before multi-start existed.
#[must_use]
pub fn estimate_run_seconds(
    tier_count: usize,
    free_tiers: usize,
    evaluations: usize,
    candidates: usize,
    starts: usize,
    measured_ms_per_evaluation: Option<f64>,
) -> f64 {
    let scale = (tier_count as f64 / SMALL_DESIGN_TIERS).max(1.0);
    let scorings = (candidates.clamp(1, MAX_CANDIDATES) + 1) as f64;
    let config = OptimizeConfig {
        max_evaluations: evaluations,
        starts,
        ..OptimizeConfig::default()
    };
    let effective = effective_starts(&config, free_tiers);
    let (searched, lanes) = if effective > 1 {
        (
            inclusive_max_evaluations_for(&config, candidates.clamp(1, MAX_CANDIDATES), free_tiers),
            estimate_lanes(effective),
        )
    } else {
        (evaluations, 1)
    };
    let per_evaluation_ms = measured_ms_per_evaluation
        .unwrap_or_else(|| SMALL_DESIGN_EVALUATION_MS * scale * scale / lanes as f64);
    (searched as f64).mul_add(per_evaluation_ms / 1000.0, scorings * FULL_SCORING_SECONDS)
}

/// How many starts run at once: the starts, at most half the logical cores (each start
/// spawns its own pair of solve threads). One on the web, where the lanes run in turn.
fn estimate_lanes(effective_starts: usize) -> usize {
    if cfg!(target_arch = "wasm32") {
        return 1;
    }
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    effective_starts.min(cores / 2).max(1)
}

/// The cost of one evaluation, in milliseconds, worked out from a finished run.
///
/// It is the run's wall time less the full-quality scorings it paid for (the starting
/// stone and each candidate), over its evaluations (all of them: screening, every start's
/// descent and polish, which is `OptimizeResult::evaluations`). With several starts on
/// several threads that is the effective wall-clock rate, not the cost of one evaluation on
/// one thread. `None` when the run was too short to tell (under 20
/// evaluations, or no time left once the scorings are taken off), so one odd run does not
/// replace a better figure.
#[must_use]
pub fn measured_ms_per_evaluation(
    elapsed_secs: f64,
    evaluations: usize,
    candidates_found: usize,
) -> Option<f64> {
    if evaluations < 20 || !elapsed_secs.is_finite() {
        return None;
    }
    let scorings = (candidates_found.max(1) + 1) as f64 * FULL_SCORING_SECONDS;
    let searching = elapsed_secs - scorings;
    (searching > 0.0).then_some(searching / evaluations as f64 * 1000.0)
}

/// A duration in plain words: "about 40 seconds", "about 3 minutes".
#[must_use]
pub fn format_duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "unknown".to_string();
    }
    if seconds < 2.0 {
        "a second or two".to_string()
    } else if seconds < 60.0 {
        let rounded = if seconds < 10.0 {
            seconds.round()
        } else {
            (seconds / 5.0).round() * 5.0
        };
        format!("about {rounded:.0} seconds")
    } else if seconds < 5400.0 {
        let minutes = (seconds / 60.0).round();
        if minutes < 1.5 {
            "about 1 minute".to_string()
        } else {
            format!("about {minutes:.0} minutes")
        }
    } else {
        format!("about {:.1} hours", seconds / 3600.0)
    }
}

/// The line under the budget box: the estimate, and whether it comes from a measured
/// rate or from the design size alone.
#[must_use]
pub fn estimate_text(seconds: f64, measured: bool) -> String {
    let duration = format_duration(seconds);
    if measured {
        format!("Estimated time: {duration} (from your last run on this design).")
    } else {
        format!(
            "Estimated time: {duration} (a rough guess from the design size; the next run is estimated from this one)."
        )
    }
}

#[cfg(test)]
mod tests;
