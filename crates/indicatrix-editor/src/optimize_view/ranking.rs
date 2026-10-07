//! The Optimize tab's ranked candidate list: one row per candidate, the starting stone as
//! a row of its own, the outcome a selected candidate stands for, and the sentence that
//! sums a run up.

use super::{cancelled_note, direction_of, polish_note};
use indicatrix::color::metrics::FaceUpTone;
use indicatrix_cut_core::{OptimizeCandidate, OptimizeOutcome, OptimizeResult, ToneGoal};

/// One row of the candidate list, every cell already formatted.
///
/// The `*_direction` fields are `1` when the figure is better than the starting stone's,
/// `-1` when it is worse and `0` when it is the same (or the row is the starting stone
/// itself), so a front end can colour a cell without parsing the text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CandidateLine {
    /// `"Start"` for the starting stone, else the rank: `"1"` is the best candidate.
    pub rank: String,
    /// The combined score, lower is better.
    pub score: String,
    /// Windowing, as a percentage; lower is better.
    pub windowing: String,
    /// Tilt brilliance, as a percentage; higher is better.
    pub brilliance: String,
    /// Extinction, as a percentage; lower is better.
    pub extinction: String,
    /// Yield loss, as a percentage; lower is better.
    pub yield_loss: String,
    /// How many tiers the candidate changes; `"-"` for the starting stone.
    pub changed: String,
    /// Direction of the score against the starting stone.
    pub score_direction: i32,
    /// Direction of the windowing figure.
    pub windowing_direction: i32,
    /// Direction of the brilliance figure.
    pub brilliance_direction: i32,
    /// Direction of the extinction figure.
    pub extinction_direction: i32,
    /// Direction of the yield loss figure.
    pub yield_direction: i32,
    /// The face-up lightness, `"L* 62.3"`; `"-"` when the run did not measure the tone.
    pub tone: String,
    /// Direction of the tone against the starting stone: judged on L* for a lighter goal and
    /// on chroma for a deeper one, `0` when the tone was not weighted or for the start row.
    pub tone_direction: i32,
    /// The swatch of the face-up colour, display sRGB; `None` without a tone.
    pub tone_srgb: Option<[u8; 3]>,
}

/// The tone cell: `"L* 62.3"`, or `"-"` without a tone.
fn tone_text(tone: Option<&FaceUpTone>) -> String {
    tone.map_or_else(|| "-".to_string(), |t| format!("L* {:.1}", t.l_star))
}

/// A percentage cell: one decimal and a percent sign.
fn percent(value: f32) -> String {
    format!("{value:.1}%")
}

/// The row of the starting stone: the figures before any change.
#[must_use]
pub fn baseline_line(result: &OptimizeResult) -> CandidateLine {
    let outcome = &result.outcome;
    CandidateLine {
        tone: tone_text(result.tone_before.as_ref()),
        tone_srgb: result.tone_before.map(|t| t.srgb),
        rank: "Start".to_string(),
        score: format!("{:.2}", outcome.before_score),
        windowing: percent(outcome.before.windowing_pct),
        brilliance: percent(outcome.before.tilt_brilliance_pct),
        extinction: percent(outcome.before.extinction_pct),
        yield_loss: percent(outcome.before_yield_loss_pct),
        changed: "-".to_string(),
        ..CandidateLine::default()
    }
}

/// The candidate rows of `result`, best first, each compared with the starting stone.
#[must_use]
pub fn candidate_lines(result: &OptimizeResult) -> Vec<CandidateLine> {
    let before = &result.outcome;
    let tone_direction = |candidate: &OptimizeCandidate| match (
        result.tone_goal,
        result.tone_before,
        candidate.tone,
    ) {
        (Some(ToneGoal::Lighter), Some(start), Some(now)) => {
            direction_of(now.l_star - start.l_star, true)
        }
        (Some(ToneGoal::Deeper), Some(start), Some(now)) => {
            direction_of(now.chroma - start.chroma, true)
        }
        _ => 0,
    };
    result
        .candidates
        .iter()
        .enumerate()
        .map(|(position, candidate)| CandidateLine {
            rank: (position + 1).to_string(),
            score: format!("{:.2}", candidate.score),
            windowing: percent(candidate.after.windowing_pct),
            brilliance: percent(candidate.after.tilt_brilliance_pct),
            extinction: percent(candidate.after.extinction_pct),
            yield_loss: percent(candidate.yield_loss_pct),
            changed: candidate.changes.len().to_string(),
            score_direction: direction_of(candidate.score - before.before_score, false),
            windowing_direction: direction_of(
                candidate.after.windowing_pct - before.before.windowing_pct,
                false,
            ),
            brilliance_direction: direction_of(
                candidate.after.tilt_brilliance_pct - before.before.tilt_brilliance_pct,
                true,
            ),
            extinction_direction: direction_of(
                candidate.after.extinction_pct - before.before.extinction_pct,
                false,
            ),
            yield_direction: direction_of(
                candidate.yield_loss_pct - before.before_yield_loss_pct,
                false,
            ),
            tone: tone_text(candidate.tone.as_ref()),
            tone_direction: tone_direction(candidate),
            tone_srgb: candidate.tone.map(|t| t.srgb),
        })
        .collect()
}

/// The outcome that selecting `candidate` stands for.
///
/// It is `base` with the candidate's changes and figures in place of the best result's.
/// This is what the preview, the compare window and the table of changed tiers read, all
/// of which take an outcome.
#[must_use]
pub fn candidate_outcome(base: &OptimizeOutcome, candidate: &OptimizeCandidate) -> OptimizeOutcome {
    OptimizeOutcome {
        changes: candidate.changes.clone(),
        after: candidate.after,
        after_score: candidate.score,
        after_yield_loss_pct: candidate.yield_loss_pct,
        ..base.clone()
    }
}

/// The sentence above the candidate list: what the run found, in plain words.
///
/// A run that found nothing better says so plainly and says nothing was changed.
/// `elapsed_secs` is the wall time of the run, when known.
#[must_use]
pub fn optimize_run_status(result: &OptimizeResult, elapsed_secs: Option<f32>) -> String {
    let outcome = &result.outcome;
    let time = elapsed_secs.map_or_else(String::new, |secs| format!(", {secs:.1} s"));
    let notes = format!("{}{}", cancelled_note(outcome), polish_note(outcome));
    match result.candidates.first() {
        None if outcome.evaluations == 0 => format!(
            "Nothing could be changed with these settings: no tier was free to move. Check \
             \"Only selected tiers\" and the angle ranges.{notes}"
        ),
        None => format!(
            "No better arrangement found in {} evaluation(s){}{time}. The tiers it was allowed to \
             change are already at, or very near, the best for this weighting, so nothing was \
             changed.{notes}",
            outcome.evaluations,
            starts_note(result, false)
        ),
        Some(best) => format!(
            "Found {} candidate(s) in {} evaluation(s){}{time}. The best changes {} tier(s).{notes}",
            result.candidates.len(),
            outcome.evaluations,
            starts_note(result, true),
            best.changes.len()
        ),
    }
}

/// " across 8 starts (the best came from start 4)" for a run that tried several starting
/// arrangements, nothing for a single start. Start 1 is the design's own descent.
/// `with_origin` adds the clause naming where the best came from.
fn starts_note(result: &OptimizeResult, with_origin: bool) -> String {
    if result.starts_run <= 1 {
        return String::new();
    }
    if with_origin {
        format!(
            " across {} starts (the best came from start {})",
            result.starts_run,
            result.best_start + 1
        )
    } else {
        format!(" across {} starts", result.starts_run)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::{AngleChange, ObjectiveComponents};

    fn components(windowing: f32, extinction: f32, brilliance: f32) -> ObjectiveComponents {
        ObjectiveComponents {
            windowing_pct: windowing,
            extinction_pct: extinction,
            tilt_brilliance_pct: brilliance,
        }
    }

    fn change(index: usize, from_deg: f64, to_deg: f64) -> AngleChange {
        AngleChange {
            index,
            from_deg,
            to_deg,
        }
    }

    fn candidate(
        after: ObjectiveComponents,
        score: f32,
        yield_loss_pct: f32,
        changes: Vec<AngleChange>,
    ) -> OptimizeCandidate {
        OptimizeCandidate {
            changes,
            mast_changes: Vec::new(),
            after,
            score,
            yield_loss_pct,
            tone: None,
        }
    }

    fn tone(l_star: f32, chroma: f32) -> FaceUpTone {
        FaceUpTone {
            l_star,
            chroma,
            srgb: [10, 20, 30],
            ..FaceUpTone::NONE
        }
    }

    /// `sample_result` with tones: the start at L* 50 / C* 20; the best is lighter and
    /// weaker, the second darker and stronger.
    fn toned_result(goal: Option<ToneGoal>) -> OptimizeResult {
        let mut result = sample_result();
        result.tone_before = Some(tone(50.0, 20.0));
        result.tone_goal = goal;
        result.candidates[0].tone = Some(tone(62.3, 15.0));
        result.candidates[1].tone = Some(tone(40.0, 30.0));
        result
    }

    /// A start of 12.5 / 8 / 60 (score 20, yield loss 30) and two candidates: the first
    /// trades extinction for windowing and brilliance, the second changes nothing but yield.
    fn sample_result() -> OptimizeResult {
        let best = candidate(
            components(9.3, 11.0, 65.0),
            15.0,
            25.0,
            vec![change(3, -40.0, -41.5), change(5, 38.0, 39.0)],
        );
        let second = candidate(
            components(12.5, 8.0, 60.0),
            19.0,
            28.0,
            vec![change(3, -40.0, -40.5)],
        );
        OptimizeResult {
            outcome: OptimizeOutcome {
                before: components(12.5, 8.0, 60.0),
                before_score: 20.0,
                before_yield_loss_pct: 30.0,
                after: best.after,
                after_score: best.score,
                after_yield_loss_pct: best.yield_loss_pct,
                evaluations: 42,
                changes: best.changes.clone(),
                cancelled: false,
                polish_evaluations: 0,
                polish_improvement: 0.0,
            },
            mast_changes: Vec::new(),
            candidates: vec![best, second],
            tone_before: None,
            tone_goal: None,
            lighting: indicatrix_cut_core::CANONICAL_LIGHTING_PRESET,
            starts_run: 1,
            best_start: 0,
        }
    }

    #[test]
    fn the_starting_stone_has_a_row_of_its_own() {
        let line = baseline_line(&sample_result());
        assert_eq!(line.rank, "Start");
        assert_eq!(line.tone, "-", "no tone measured shows a dash");
        assert_eq!(line.tone_srgb, None);
        assert_eq!(line.score, "20.00");
        assert_eq!(line.windowing, "12.5%");
        assert_eq!(line.brilliance, "60.0%");
        assert_eq!(line.extinction, "8.0%");
        assert_eq!(line.yield_loss, "30.0%");
        assert_eq!(line.changed, "-");
        assert_eq!(line.score_direction, 0);
        assert_eq!(line.windowing_direction, 0);
    }

    #[test]
    fn a_candidate_row_shows_rank_figures_and_the_number_of_changed_tiers() {
        let lines = candidate_lines(&sample_result());
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].rank, "1");
        assert_eq!(lines[0].score, "15.00");
        assert_eq!(lines[0].windowing, "9.3%");
        assert_eq!(lines[0].brilliance, "65.0%");
        assert_eq!(lines[0].extinction, "11.0%");
        assert_eq!(lines[0].yield_loss, "25.0%");
        assert_eq!(lines[0].changed, "2");
        assert_eq!(lines[1].rank, "2");
        assert_eq!(lines[1].changed, "1");
    }

    #[test]
    fn each_cell_says_whether_it_beats_the_starting_stone() {
        let lines = candidate_lines(&sample_result());
        let best = &lines[0];
        assert_eq!(best.score_direction, 1, "a lower score is better");
        assert_eq!(best.windowing_direction, 1, "less windowing is better");
        assert_eq!(best.brilliance_direction, 1, "more brilliance is better");
        assert_eq!(best.extinction_direction, -1, "more extinction is worse");
        assert_eq!(best.yield_direction, 1, "less yield loss is better");

        let second = &lines[1];
        assert_eq!(
            second.windowing_direction, 0,
            "the same figure is unchanged"
        );
        assert_eq!(second.brilliance_direction, 0);
        assert_eq!(second.extinction_direction, 0);
        assert_eq!(second.yield_direction, 1);
    }

    #[test]
    fn the_tone_cell_shows_lightness_and_the_swatch() {
        let result = toned_result(None);
        let start = baseline_line(&result);
        assert_eq!(start.tone, "L* 50.0");
        assert_eq!(start.tone_srgb, Some([10, 20, 30]));
        let lines = candidate_lines(&result);
        assert_eq!(lines[0].tone, "L* 62.3");
        assert_eq!(lines[0].tone_srgb, Some([10, 20, 30]));
        assert_eq!(
            lines[0].tone_direction, 0,
            "an unweighted tone is not judged"
        );
        assert_eq!(lines[1].tone_direction, 0);
    }

    #[test]
    fn the_tone_direction_follows_the_goal() {
        let lighter = candidate_lines(&toned_result(Some(ToneGoal::Lighter)));
        assert_eq!(lighter[0].tone_direction, 1, "L* rose");
        assert_eq!(lighter[1].tone_direction, -1, "L* fell");
        let deeper = candidate_lines(&toned_result(Some(ToneGoal::Deeper)));
        assert_eq!(deeper[0].tone_direction, -1, "chroma fell");
        assert_eq!(deeper[1].tone_direction, 1, "chroma rose");
    }

    #[test]
    fn a_run_without_candidates_has_no_rows() {
        let mut result = sample_result();
        result.candidates.clear();
        assert_eq!(candidate_lines(&result), Vec::<CandidateLine>::new());
    }

    #[test]
    fn selecting_a_candidate_gives_the_outcome_with_its_changes_and_figures() {
        let result = sample_result();
        let outcome = candidate_outcome(&result.outcome, &result.candidates[1]);
        assert_eq!(outcome.changes, result.candidates[1].changes);
        assert_eq!(outcome.after, result.candidates[1].after);
        assert_eq!(outcome.after_score, 19.0);
        assert_eq!(outcome.after_yield_loss_pct, 28.0);
        // Everything that describes the start and the run stays.
        assert_eq!(outcome.before, result.outcome.before);
        assert_eq!(outcome.before_score, 20.0);
        assert_eq!(outcome.evaluations, 42);
    }

    #[test]
    fn the_best_candidate_is_the_plain_outcome() {
        let result = sample_result();
        assert_eq!(
            candidate_outcome(&result.outcome, &result.candidates[0]),
            result.outcome
        );
    }

    #[test]
    fn the_status_counts_candidates_and_the_tiers_the_best_changes() {
        let text = optimize_run_status(&sample_result(), Some(12.34));
        assert_eq!(
            text,
            "Found 2 candidate(s) in 42 evaluation(s), 12.3 s. The best changes 2 tier(s)."
        );
        assert!(!optimize_run_status(&sample_result(), None).contains(" s."));
    }

    #[test]
    fn a_run_with_several_starts_says_how_many_and_where_the_best_came_from() {
        let mut result = sample_result();
        result.starts_run = 8;
        result.best_start = 3;
        assert_eq!(
            optimize_run_status(&result, Some(12.34)),
            "Found 2 candidate(s) in 42 evaluation(s) across 8 starts (the best came from \
             start 4), 12.3 s. The best changes 2 tier(s)."
        );
        result.candidates.clear();
        result.outcome.changes.clear();
        let text = optimize_run_status(&result, None);
        assert!(
            text.starts_with("No better arrangement found in 42 evaluation(s) across 8 starts."),
            "{text}"
        );
    }

    #[test]
    fn a_run_that_found_nothing_better_says_so_plainly() {
        let mut result = sample_result();
        result.candidates.clear();
        result.outcome.changes.clear();
        let text = optimize_run_status(&result, None);
        assert!(
            text.starts_with("No better arrangement found in 42 evaluation(s)."),
            "{text}"
        );
        assert!(text.contains("nothing was changed"), "{text}");
    }

    #[test]
    fn a_run_with_no_free_tier_says_nothing_could_move() {
        let mut result = sample_result();
        result.candidates.clear();
        result.outcome.changes.clear();
        result.outcome.evaluations = 0;
        let text = optimize_run_status(&result, None);
        assert!(
            text.starts_with("Nothing could be changed with these settings"),
            "{text}"
        );
        assert!(text.contains("Only selected tiers"), "{text}");
    }

    #[test]
    fn the_status_notes_cancellation_and_the_polish_stage() {
        let mut result = sample_result();
        result.outcome.cancelled = true;
        result.outcome.polish_evaluations = 9;
        result.outcome.polish_improvement = 0.25;
        let text = optimize_run_status(&result, None);
        assert!(text.contains("cancelled"), "{text}");
        assert!(text.contains("polish: +0.25 in 9 evaluation(s)"), "{text}");
    }
}
