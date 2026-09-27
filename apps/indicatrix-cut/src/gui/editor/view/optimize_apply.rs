//! The Optimize result SUMMARY table ([`optimize_result_rows`]/[`optimize_status_text`]),
//! the "Preview" toggle's ghost-preview candidate ([`build_optimize_preview_design`]/
//! [`submit_design_ghost_preview`]), and the Optimize weight-form parser
//! ([`parse_optimize_weights`]). See [`super::solve_results`] for the per-tier
//! result tables and the Deep Solve/Optimize availability hints this file does
//! not cover.

use super::viewport::scaled_viewport_size;
use crate::{
    MainWindow, OptimizeResultRow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::auto_solve,
        solid_preview::preview_state::{CameraPose, SolidPreviewState},
    },
};
use indicatrix_cut_core::{Design, ObjectiveWeights, OptimizeOutcome};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// Formats one objective component's "after" cell as the raw value plus a signed
/// delta and a plain-English verdict -- "9.25% (-3.25%,
/// better)" rather than a bare number the cutter has to subtract by hand and
/// remember the polarity of. `higher_is_better` distinguishes tilt brilliance
/// (higher is better) from every other component/the blended score (lower is
/// better, see [`ObjectiveWeights::score`]'s own doc comment).
///
/// # Handoff
/// `OptimizeResultRow` (`ui/types.slint`) has only
/// `label`/`before`/`after` -- the delta/verdict below is folded into `after`'s
/// own string rather than added as new `delta`/`improved: bool` fields (and
/// coloured emerald/ruby per row, `ui/components/editor_inspector.slint`).
/// Adding those two fields plus the row colouring is still open.
#[must_use]
fn after_with_delta(before: f32, after: f32, higher_is_better: bool, unit: &str) -> (String, i32) {
    let delta = after - before;
    // `delta.abs() < f32::EPSILON` rather than `delta == 0.0` -- clippy's
    // `float_cmp` lint (pedantic) flags exact float equality even here, where
    // `delta` is a plain subtraction of two already-rounded measurements.
    let direction = if delta.abs() < f32::EPSILON {
        0
    } else if (higher_is_better && delta > 0.0) || (!higher_is_better && delta < 0.0) {
        1
    } else {
        -1
    };
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

/// Builds the rows `EditorView`'s Optimize result table needs from a completed
/// or cancelled run's [`OptimizeOutcome`] -- windowing, extinction, tilt
/// brilliance, and yield loss each get their OWN row, and the blended score
/// comes last as a clearly-separate row: an optimizer that improved windowing by
/// wrecking extinction must be visibly doing that, never collapsed into a single
/// figure. Each row's `after` cell also names its own signed delta and direction
/// via [`after_with_delta`], so a cutter reads which metric
/// moved and by how much without doing the subtraction (or remembering which way
/// is good) themselves.
///
/// The "Yield loss" row is shown
/// UNCONDITIONALLY, even at the Optimize tab's default `yield_weight == 0.0` --
/// `before_yield_loss_pct`/`after_yield_loss_pct` are real measurements of the
/// starting/final design either way (see [`OptimizeOutcome::before_yield_loss_pct`]'s
/// own doc comment), and a cutter who left the slider at its default still
/// benefits from seeing whether Optimize's angle changes happened to help or hurt
/// yield, even though the search itself never weighed it.
pub(in crate::gui::editor) fn optimize_result_rows(
    outcome: &OptimizeOutcome,
) -> Vec<OptimizeResultRow> {
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

/// One row of [`optimize_result_rows`] -- the before figure, the after figure with
/// its own delta and verdict, and the direction the row is coloured by.
///
/// `higher_is_better` is the metric's own polarity, not a property of the numbers:
/// windowing and extinction going DOWN is an improvement, tilt brilliance going up
/// is. Getting that backwards would colour a real improvement red, which is why it
/// is stated per call rather than inferred.
fn metric_row(
    label: &str,
    before: f32,
    after: f32,
    higher_is_better: bool,
    unit: &str,
) -> OptimizeResultRow {
    let (after_text, direction) = after_with_delta(before, after, higher_is_better, unit);
    OptimizeResultRow {
        label: label.into(),
        before: format!("{before:.2}{unit}").into(),
        after: after_text.into(),
        direction,
    }
}

/// The one-line summary shown above [`optimize_result_rows`]'s table -- how many
/// tiers changed and how many candidate evaluations it took, plus (only when true)
/// the cancellation note and (only when the polish stage actually ran) how much of
/// the final score it is responsible for -- without this note,
/// `polish_evaluations`/`polish_improvement` would have no reader anywhere in
/// this crate. The per-component before/after numbers themselves live only in the
/// rows table, never duplicated here.
pub(in crate::gui::editor) fn optimize_status_text(outcome: &OptimizeOutcome) -> String {
    let cancelled_note = if outcome.cancelled {
        " (cancelled -- showing the best partial result found before the checkpoint \
         fired)"
    } else {
        ""
    };
    let polish_note = if outcome.polish_evaluations > 0 {
        format!(
            " (polish: +{:.2} in {} evaluation(s))",
            outcome.polish_improvement, outcome.polish_evaluations
        )
    } else {
        String::new()
    };
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

/// A clone of `design` with every one of
/// `outcome`'s [`indicatrix_cut_core::AngleChange`]s already applied -- the
/// candidate a "Preview" toggle shows in the viewport BEFORE the cutter commits to
/// Apply. Never touches `History`/`Edit` at all: `ConstraintTier::angle_deg` is a
/// plain public field, and this is a display-only candidate, never something an
/// Undo could need to unwind.
#[must_use]
pub(in crate::gui::editor) fn build_optimize_preview_design(
    design: &Design,
    outcome: &OptimizeOutcome,
) -> Design {
    let mut preview = design.clone();
    for change in &outcome.changes {
        if let Some(tier) = preview.tiers.get_mut(change.index) {
            tier.angle_deg = change.to_deg;
        }
    }
    preview
}

/// Solves `design` and, on success, redraws the shared solid-preview viewport with
/// its planes at the CURRENT camera pose -- a raw, generation-independent reproject
/// (`SolidPreviewState::request_redraw_with_gear`, the same call
/// `gui::render::camera_lighting::resubmit_at_current_pose` uses for a camera
/// drag), deliberately NOT [`super::viewport::submit_preview_replan`]'s
/// worker-queued `ReplanRequest` path: a ghost preview must never stamp
/// `solid_last_solved`/the design-generation stash with a CANDIDATE design's own
/// solved masts, which would corrupt the next real edit's `resolve_dirty`
/// baseline and the next landed worker frame's tier-table push
/// (`super::viewport::push_solved_preview`) into showing the ghost's numbers
/// instead of the live design's.
///
/// Returns whether `design` actually solved (and so was shown) -- `false` leaves
/// the viewport showing whatever it already had, since there is no honest
/// candidate geometry to draw for a design that does not close.
///
/// Used by both Optimize's own "Preview" toggle (via
/// [`build_optimize_preview_design`]) and Retarget's live ghost overlay
/// (`callbacks::retarget_actions`), so the two features share one implementation of
/// "show this candidate, without disturbing anything the REAL design's next edit
/// depends on."
#[must_use]
pub(in crate::gui::editor) fn submit_design_ghost_preview(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    design: &Design,
) -> bool {
    let Ok(solved) = design.solve() else {
        return false;
    };
    let planes_gpu = auto_solve::design_to_gpu_planes_from_solved(design, &solved);
    let planes: Vec<(glam::Vec3, f32)> = planes_gpu
        .iter()
        .map(|p| (glam::Vec3::from(p.normal), -p.d))
        .collect();
    let ctx = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let camera = CameraPose {
        yaw: ctx.yaw,
        pitch: ctx.pitch,
        distance: ctx.distance,
    };
    let design_gear = ctx.design_gear;
    let view_mode = ui.global::<crate::SolidPreviewModel>().get_view_mode() as u8;
    let size = crate::gui::render::camera_lighting::contained_request_size(
        view_mode,
        scaled_viewport_size(ui),
        (ctx.width, ctx.height),
    );
    drop(ctx);
    preview_state.request_redraw_with_gear(planes, camera, size, view_mode, design_gear);
    true
}

/// Parses the Optimize weight form's three text fields (plus the yield slider's
/// own already-numeric `0..1` value) into an [`ObjectiveWeights`] -- the three
/// text fields must parse and be finite, but also reject negative values: only
/// the RATIOS between them matter, so a negative one would silently invert that
/// component's polarity (rewarding more windowing, say) rather than merely
/// weighting it oddly. `yield_weight` needs no such validation: it comes
/// straight from `EditorModel.optimize_weight_yield`
/// (`ui/components/editor_inspector.slint`'s `Slider`, `minimum: 0.0, maximum:
/// 1.0`), which cannot produce a non-finite or out-of-range value in the first
/// place.
pub(in crate::gui::editor) fn parse_optimize_weights(
    windowing: &str,
    extinction: &str,
    tilt_brilliance: &str,
    yield_weight: f32,
) -> Result<ObjectiveWeights, String> {
    fn parse_weight(label: &str, text: &str) -> Result<f32, String> {
        let value: f32 = text
            .trim()
            .parse()
            .map_err(|_| format!("{label} weight must be a number."))?;
        if !value.is_finite() || value < 0.0 {
            return Err(format!(
                "{label} weight must be a non-negative, finite number."
            ));
        }
        Ok(value)
    }
    Ok(ObjectiveWeights {
        windowing: parse_weight("Windowing", windowing)?,
        extinction: parse_weight("Extinction", extinction)?,
        tilt_brilliance: parse_weight("Tilt brilliance", tilt_brilliance)?,
        yield_weight,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{AngleChange, ConstraintTier, ObjectiveComponents};

    fn scale_reference_tier(value: f64) -> ConstraintTier {
        ConstraintTier {
            angle_deg: 0.0,
            name: "T".to_string(),
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(value),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    /// A hand-built [`OptimizeOutcome`] whose `after` deliberately makes extinction
    /// WORSE while windowing and tilt brilliance improve -- proving
    /// [`optimize_result_rows`] reports every component's real number rather than
    /// only the still-improved blended score.
    fn sample_outcome(changed: bool, cancelled: bool) -> OptimizeOutcome {
        OptimizeOutcome {
            before: ObjectiveComponents {
                windowing_pct: 12.5,
                extinction_pct: 8.0,
                tilt_brilliance_pct: 60.0,
            },
            before_score: 20.0,
            before_yield_loss_pct: 30.0,
            after: ObjectiveComponents {
                windowing_pct: 9.25,
                extinction_pct: 11.0,
                tilt_brilliance_pct: 65.0,
            },
            after_score: 15.0,
            after_yield_loss_pct: 25.0,
            evaluations: 42,
            changes: if changed {
                vec![AngleChange {
                    index: 3,
                    from_deg: -40.0,
                    to_deg: -41.5,
                }]
            } else {
                Vec::new()
            },
            cancelled,
            polish_evaluations: 0,
            polish_improvement: 0.0,
        }
    }

    #[test]
    fn optimize_result_rows_reports_each_component_separately_never_collapsed() {
        let outcome = sample_outcome(true, false);
        let rows = optimize_result_rows(&outcome);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].label.as_str(), "Windowing");
        assert_eq!(rows[0].before.as_str(), "12.50%");
        // Lower windowing is better -- a negative delta reads as "better".
        assert_eq!(rows[0].after.as_str(), "9.25% (-3.25%, better)");
        // Extinction got WORSE -- shown honestly, not hidden by the improved score.
        assert_eq!(rows[1].label.as_str(), "Extinction");
        assert_eq!(rows[1].before.as_str(), "8.00%");
        assert_eq!(rows[1].after.as_str(), "11.00% (+3.00%, worse)");
        assert_eq!(rows[2].label.as_str(), "Tilt brilliance");
        assert_eq!(rows[2].before.as_str(), "60.00%");
        // Higher tilt brilliance is better -- a positive delta reads as "better".
        assert_eq!(rows[2].after.as_str(), "65.00% (+5.00%, better)");
        // Yield loss went DOWN (less preform thrown away) -- reads as "better",
        // same polarity as windowing/extinction, even though this fixture's
        // `ObjectiveWeights` (implicit -- `OptimizeOutcome` carries no weights of
        // its own) never actually weighed it into the blended score.
        assert_eq!(rows[3].label.as_str(), "Yield loss");
        assert_eq!(rows[3].before.as_str(), "30.00%");
        assert_eq!(rows[3].after.as_str(), "25.00% (-5.00%, better)");
        assert_eq!(rows[4].label.as_str(), "Blended score");
        assert_eq!(rows[4].before.as_str(), "20.00");
        assert_eq!(rows[4].after.as_str(), "15.00 (-5.00, better)");
    }

    #[test]
    fn build_optimize_preview_design_moves_only_the_changed_tiers_angle() {
        // The ghost-preview candidate must apply every
        // `AngleChange` to the right tier and leave every other tier's angle (and
        // every other field) untouched.
        let mut design = Design::new(
            indicatrix_cut_core::PreformSpec::block(2.0, 1.0, 2.0),
            indicatrix_cut_core::ScheduleMeta::default(),
            vec![
                scale_reference_tier(0.5),
                scale_reference_tier(0.6),
                scale_reference_tier(0.7),
                ConstraintTier {
                    angle_deg: -40.0,
                    name: "P1".to_string(),
                    indices: vec![0.0, 24.0],
                    constraint: MeetConstraint::ScaleReference(0.8),
                    imported_meet: None,
                    original_notes: None,
                    detached: Vec::new(),
                },
            ],
        );
        design.tiers[3].angle_deg = -40.0;
        let outcome = sample_outcome(true, false); // changes tier index 3 to -41.5
        let preview = build_optimize_preview_design(&design, &outcome);
        assert!((preview.tiers[3].angle_deg - (-41.5)).abs() < 1e-9);
        // Every other tier is untouched.
        for i in 0..3 {
            assert!((preview.tiers[i].angle_deg - design.tiers[i].angle_deg).abs() < 1e-9);
        }
        // The original design is never mutated.
        assert!((design.tiers[3].angle_deg - (-40.0)).abs() < 1e-9);
    }

    #[test]
    fn build_optimize_preview_design_ignores_an_out_of_range_change_index() {
        // A design edited between when Optimize ran and when the preview toggle is
        // flipped can shrink the tier list out from under a stale outcome -- this
        // must degrade gracefully (skip that change), never panic.
        let design = Design::new(
            indicatrix_cut_core::PreformSpec::block(2.0, 1.0, 2.0),
            indicatrix_cut_core::ScheduleMeta::default(),
            vec![scale_reference_tier(0.5)],
        );
        let outcome = sample_outcome(true, false); // names tier index 3, out of range
        let preview = build_optimize_preview_design(&design, &outcome);
        assert_eq!(preview.tiers.len(), 1);
    }

    #[test]
    fn after_with_delta_reports_unchanged_when_the_value_did_not_move() {
        assert_eq!(
            after_with_delta(5.0, 5.0, false, "%"),
            ("5.00% (+0.00%, unchanged)".to_string(), 0)
        );
    }

    #[test]
    fn optimize_status_text_reports_the_change_and_evaluation_count() {
        let text = optimize_status_text(&sample_outcome(true, false));
        assert!(text.contains("1 tier(s)"));
        assert!(text.contains("42 evaluation(s)"));
        assert!(!text.contains("cancelled"));
    }

    #[test]
    fn optimize_status_text_reports_no_improving_move_when_nothing_changed() {
        let text = optimize_status_text(&sample_outcome(false, false));
        assert!(text.contains("no improving move"));
    }

    #[test]
    fn optimize_status_text_notes_cancellation_without_hiding_the_partial_result() {
        // `after`/`changes` still reflect the best REAL partial result found, never
        // discarded -- the cancellation note must be additive, not replace the summary.
        let text = optimize_status_text(&sample_outcome(true, true));
        assert!(text.contains("cancelled"));
        assert!(text.contains("1 tier(s)"));
    }

    #[test]
    fn optimize_status_text_names_the_polish_stages_own_contribution_when_it_ran() {
        // Without this reader, `polish_evaluations`/`polish_improvement` would
        // have no consumer anywhere in this crate -- whether the ridge-following
        // polish stage did anything at all would be invisible to a cutter.
        let mut outcome = sample_outcome(true, false);
        outcome.polish_evaluations = 31;
        outcome.polish_improvement = 0.42;
        let text = optimize_status_text(&outcome);
        assert!(text.contains("polish: +0.42 in 31 evaluation(s)"));
    }

    #[test]
    fn optimize_status_text_omits_the_polish_note_when_the_stage_never_ran() {
        let text = optimize_status_text(&sample_outcome(true, false));
        assert!(!text.contains("polish"));
    }

    #[test]
    fn parse_optimize_weights_accepts_well_formed_input() {
        let weights = parse_optimize_weights("1.0", "2.5", "0", 0.0).unwrap();
        assert_eq!(weights.windowing, 1.0);
        assert_eq!(weights.extinction, 2.5);
        assert_eq!(weights.tilt_brilliance, 0.0);
        assert_eq!(weights.yield_weight, 0.0);
    }

    #[test]
    fn parse_optimize_weights_rejects_a_non_numeric_field() {
        let err = parse_optimize_weights("not-a-number", "1.0", "1.0", 0.0).unwrap_err();
        assert!(err.contains("Windowing"));
    }

    #[test]
    fn parse_optimize_weights_rejects_a_negative_weight() {
        // A negative weight is not merely out of range -- it would invert that
        // component's polarity -- so this is checked separately from finiteness.
        let err = parse_optimize_weights("1.0", "-0.5", "1.0", 0.0).unwrap_err();
        assert!(err.contains("Extinction"));
    }

    #[test]
    fn parse_optimize_weights_rejects_non_finite_values() {
        assert!(parse_optimize_weights("NaN", "1.0", "1.0", 0.0).is_err());
        assert!(parse_optimize_weights("1.0", "inf", "1.0", 0.0).is_err());
    }

    /// `yield_weight` comes
    /// straight from the Optimize tab's `0..1` slider, not a parsed text field --
    /// it passes through into `ObjectiveWeights` untouched, whatever value it is
    /// (the slider itself is what keeps it in range).
    #[test]
    fn parse_optimize_weights_carries_the_yield_slider_value_through_untouched() {
        let weights = parse_optimize_weights("1.0", "1.0", "1.0", 0.4).unwrap();
        assert_eq!(weights.yield_weight, 0.4);
    }
}
