//! Deep Solve/Optimize availability hints ([`deep_solve_hint`]/[`optimize_hint`])
//! and the per-tier result tables built from a completed run
//! ([`deep_solve_tier_rows`]/[`optimize_change_rows`]/[`facet_count_from_solved`]).
//! See [`super::optimize_apply`] for the Optimize result SUMMARY table/ghost
//! preview/weight parsing this file does not cover.

use super::state::EditorState;
use crate::{DeepSolveTierRow, OptimizeChangeRow, gui::editor::deep_solve::TierMastDelta};
use indicatrix::geometry::meet_solver::{MeetConstraint, SolvedTier, VerifiedSolveReport};
use indicatrix_cut_core::{Design, OptimizeOutcome, free_tier_indices};

/// Whether Deep Solve is available for `state`'s current design, and the
/// explanatory hint text `EditorView` shows next to its button either way:
///
/// - No printed proportions at all: disabled -- `solve_meet_points_verified` has no
///   external signal to score against.
/// - Printed proportions exist, but every tier is pinned to its recorded mast:
///   **disabled** (a run against an all-pinned design cannot
///   change the verdict, so offering it invites minutes of work for nothing),
///   hinted that there's nothing to repair yet -- the user must first convert a
///   tier to a meet constraint (the tier list's "Adopt" action) before Deep Solve
///   has anything to search over.
/// - Printed proportions exist and at least one tier is meet-derived: enabled,
///   hinted with the cost/cancellability caveat instead.
///
/// `pub(super)`, not private: `panel_stale.rs` (a sibling file) calls this from
/// its own availability push.
pub(super) fn deep_solve_hint(state: &EditorState) -> (bool, String) {
    if state.printed_proportions.is_none() {
        return (
            false,
            "Deep Solve needs this design's printed proportions (Vol/W^3, L/W, C/W, P/W, \
             H/W) from the catalogue to verify against -- unavailable for a new or \
             placeholder-reconstructed design."
                .to_string(),
        );
    }
    let has_repairable = state
        .design
        .tiers
        .iter()
        .any(|t| !matches!(t.constraint, MeetConstraint::ScaleReference(_)));
    if has_repairable {
        (
            true,
            "Slow (a mean of ~68 solves per design on the corpus -- minutes on a large \
             design); runs off the UI thread and can be cancelled."
                .to_string(),
        )
    } else {
        // An all-`ScaleReference` design cannot be repaired --
        // every tier is already pinned to its recorded mast, so a run here would
        // spend minutes unable to change the verdict either way. Disabled, not
        // merely enabled-with-a-caveat like the branch above -- the command bar's
        // Deep Solve button already gates its `enabled` on this same flag
        // (`editor_command_bar.slint`), so no markup change is needed here.
        (
            false,
            "Every tier is currently pinned to its recorded mast, exactly as imported -- \
             Deep Solve has nothing to repair until you convert a tier to a meet \
             constraint (the tier list's Adopt action)."
                .to_string(),
        )
    }
}

/// Renders a completed Deep Solve's [`VerifiedSolveReport`] as the status banner
/// text -- honestly: always shows the actual score movement and run count, never
/// just a pass/fail badge, and never claims `accepted` proves the geometry is right.
pub(in crate::gui::editor) fn format_deep_solve_report(
    report: &VerifiedSolveReport,
    stale: bool,
) -> String {
    let verdict = if report.accepted {
        "ACCEPTED -- reproduces the printed figures to verification accuracy (not a \
         correctness proof, only a strong external signal)"
    } else {
        "not accepted -- still deviates from the printed figures"
    };
    let scores = if report.initial_score.is_finite() {
        format!(
            "combined deviation {:.4} -> {:.4} ({} vertex-level repair(s), {} anchor \
             calibration move(s), {} pipeline run(s))",
            report.initial_score,
            report.final_score,
            report.overrides_applied,
            report.anchor_moves_applied,
            report.pipeline_runs
        )
    } else {
        "none of this design's printed figures overlapped what could be measured -- \
         unverifiable"
            .to_string()
    };
    let stale_note = if stale {
        " NOTE: the design changed while this ran -- re-run Deep Solve for a result that \
         reflects the current schedule."
    } else {
        ""
    };
    format!("Deep Solve: {verdict}. {scores}.{stale_note}")
}

/// The tier name to show alongside a tier-index reference in a result table --
/// `""` (never a placeholder like "(unnamed)") for an out-of-range index, since a
/// design edited between when a background search started and when its result
/// landed can shrink the tier list out from under a stale row (the caller already
/// flags that case `stale` independently; see [`TierMastDelta`]'s own doc comment).
fn tier_name_for_row(design: &Design, tier_index: usize) -> String {
    design
        .tiers
        .get(tier_index)
        .map(|tier| tier.name.clone())
        .unwrap_or_default()
}

/// Builds one [`DeepSolveTierRow`] per [`TierMastDelta`] -- the per-tier table
/// behind Deep Solve's aggregate verdict,
/// additional to (never replacing) [`format_deep_solve_report`]'s status-line
/// summary and `super::super::deep_solve::format_tier_mast_deltas`'s own
/// one-line suffix. `deltas` is expected to already be
/// `super::super::deep_solve::tier_mast_deltas`'s output -- already filtered to
/// only the tiers whose mast actually moved.
///
/// # Call site
/// `callbacks::solve_actions::setup_deep_solve_callback` calls this at its own
/// `DeepSolveOutcome::Completed` arm (see that function's doc comment) and pushes
/// the result to `EditorModel.deep_solve_tier_rows`, rendered by the Log popup's
/// per-tier table.
#[must_use]
pub(in crate::gui::editor) fn deep_solve_tier_rows(
    deltas: &[TierMastDelta],
    design: &Design,
) -> Vec<DeepSolveTierRow> {
    deltas
        .iter()
        .map(|d| DeepSolveTierRow {
            tier_number: format!("#{}", d.tier_index + 1).into(),
            name: tier_name_for_row(design, d.tier_index).into(),
            before_mast: format!("{:.4}", d.before_mast).into(),
            after_mast: format!("{:.4}", d.after_mast).into(),
            delta: format!("{:+.4}", d.delta()).into(),
        })
        .collect()
}

/// Builds one [`OptimizeChangeRow`] per [`indicatrix_cut_core::AngleChange`] a
/// pending Optimize result would apply -- the per-tier table behind
/// [`super::optimize_apply::optimize_result_rows`]'s four aggregate component
/// rows, so a cutter can see WHICH tiers move, and by how much, before clicking
/// Apply.
///
/// # Call site
/// `callbacks::solve_actions::handle_optimize_outcome` calls this alongside
/// [`super::optimize_apply::optimize_result_rows`] and pushes the result to
/// `EditorModel.optimize_change_rows`, rendered as the "Tiers this would change"
/// table in the Optimize tab.
#[must_use]
pub(in crate::gui::editor) fn optimize_change_rows(
    outcome: &OptimizeOutcome,
    design: &Design,
) -> Vec<OptimizeChangeRow> {
    outcome
        .changes
        .iter()
        .map(|change| OptimizeChangeRow {
            tier_number: format!("#{}", change.index + 1).into(),
            name: tier_name_for_row(design, change.index).into(),
            from_angle: format!("{:.2}\u{b0}", change.from_deg).into(),
            to_angle: format!("{:.2}\u{b0}", change.to_deg).into(),
            delta: format!("{:+.2}\u{b0}", change.to_deg - change.from_deg).into(),
        })
        .collect()
}

/// The design's total facet count after gear/symmetry expansion -- the count
/// [`EditorStatusStrip`](crate::gui::editor)'s persistent solver-state segment
/// still lacks (the state dot, tier
/// count and last-solve duration are already live there). One entry in
/// [`Design::planes_from_solved`]'s own output IS one facet, so this is just its
/// length -- no new geometry computation, only a name for a count that already
/// exists.
///
/// # Call site
/// `super::panel::refresh_editor_panel_from_solve` calls this and pushes the
/// result to `EditorModel.facet_count`.
#[must_use]
pub(in crate::gui::editor) fn facet_count_from_solved(
    design: &Design,
    solved: &[SolvedTier],
) -> usize {
    design.planes_from_solved(solved).len()
}

/// Whether Optimize is available for `state`'s current design, and the explanatory
/// hint `EditorView` shows next to its button either way.
///
/// Unlike [`deep_solve_hint`], this button is genuinely **disabled** (not merely
/// enabled-with-a-caveat) when there's nothing free to move: `free_tier_indices`
/// empty means `optimize_design` would return its input unchanged at zero
/// evaluations.
///
/// The one case worth stating loudly: a design fresh off the catalogue pins EVERY
/// tier to `ScaleReference` on import, so it has zero free tiers until the user
/// adopts at least one tier's real meet constraint or authors one by hand. That's
/// correct, expected behaviour for a design nobody has started editing, not a bug --
/// the hint text says so explicitly rather than leaving a disabled button to read as
/// broken.
///
/// `max_evaluations` is the CONFIGURED coordinate-stage budget
/// (`super::panel_stale::configured_optimize_max_evaluations`, read from
/// `EditorModel.optimize_budget_text` -- `200` when unset/unparseable, matching
/// [`indicatrix_cut_core::OptimizeConfig::default`]), quoted here instead of a
/// literal `200` so the hint reflects what a run would actually use rather than a
/// hard-coded figure divorced from it.
///
/// Also names the fixed canonical light pose every Optimize
/// evaluation scores tilt brilliance under
/// ([`indicatrix_cut_core::optimize::CANONICAL_LIGHT_YAW`]/`CANONICAL_LIGHT_PITCH`)
/// -- NOT the light the trace/HUD/tilt dialog show after the cutter drags it. The
/// tilt dialog's own caption already declares this half of the discrepancy
/// (`performance_graph_dialog.slint`); this doc comment states the Optimize-panel
/// half of it.
///
/// `pub(super)`, not private: `panel_stale.rs` (a sibling file) calls this from
/// its own availability push.
pub(super) fn optimize_hint(state: &EditorState, max_evaluations: usize) -> (bool, String) {
    let free = free_tier_indices(&state.design);
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
                 extinction, and tilt brilliance under a FIXED canonical light pose \
                 (not necessarily the light you see in the viewport right now -- drag \
                 the light and these figures can disagree with the trace/HUD until you \
                 re-run Optimize). Roughly 7 ms per evaluation on a small design, but \
                 up to several seconds each on a large, heavily meet-derived one (a \
                 {max_evaluations}-evaluation budget can then take minutes) -- plus two \
                 fixed full-fidelity scorings (one before, one after the search) that \
                 can each take over a second on their own, so even a fast run has some \
                 up-front and trailing wait beyond the quoted per-evaluation cost. Runs \
                 off the UI thread and can be cancelled.",
                free.len()
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::{ConstraintTier, ObjectiveComponents};

    fn some_proportions() -> indicatrix::geometry::stone_metrics::ExternalProportions {
        indicatrix::geometry::stone_metrics::ExternalProportions {
            vol_w3: Some(1.2),
            lw: Some(1.0),
            cw: Some(0.2),
            pw: Some(0.4),
            hw: Some(0.6),
        }
    }

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

    #[test]
    fn deep_solve_hint_is_unavailable_for_a_fresh_design_with_no_printed_proportions() {
        // A brand-new design has `printed_proportions: None` -- nothing to verify
        // against, so this must read as unavailable, not "nothing to repair yet".
        let state = EditorState::fresh();
        let (available, hint) = deep_solve_hint(&state);
        assert!(!available);
        assert!(hint.contains("printed proportions"));
    }

    #[test]
    fn deep_solve_hint_is_unavailable_when_every_tier_is_pinned_with_nothing_to_repair() {
        // Printed proportions exist, but every tier is pinned to a `ScaleReference`
        // -- correct and expected for an untouched import, but a run cannot change
        // the verdict either way, so the button must be DISABLED, with an
        // explanatory hint rather than reading as broken or missing.
        let mut state = EditorState::fresh();
        state.printed_proportions = Some(some_proportions());
        state.design.tiers.push(scale_reference_tier(0.5));
        state.design.tiers.push(scale_reference_tier(0.8));

        let (available, hint) = deep_solve_hint(&state);
        assert!(!available);
        assert!(hint.contains("pinned"));
        assert!(hint.contains("Adopt"));
    }

    #[test]
    fn deep_solve_hint_is_available_with_the_cost_caveat_when_a_tier_is_meet_derived() {
        // At least one tier is not pinned to a recorded mast -- Deep Solve has
        // something to search over, so the hint should be the cost/cancellability
        // caveat, not "nothing to repair".
        let mut state = EditorState::fresh();
        state.printed_proportions = Some(some_proportions());
        state.design.tiers.push(scale_reference_tier(0.5));
        state.design.tiers.push(ConstraintTier {
            angle_deg: -40.0,
            name: "P1".to_string(),
            indices: vec![0.0, 24.0],
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        });

        let (available, hint) = deep_solve_hint(&state);
        assert!(available);
        assert!(!hint.contains("pinned"));
        assert!(hint.to_lowercase().contains("cancel"));
    }

    #[test]
    fn optimize_hint_is_unavailable_when_every_tier_is_pinned() {
        // A design with only `ScaleReference` tiers has zero free tiers. Must read
        // as "nothing to optimize yet, and that's expected," never as broken.
        let mut state = EditorState::fresh();
        state.design.tiers.push(scale_reference_tier(0.5));
        state.design.tiers.push(scale_reference_tier(0.8));

        let (available, hint) = optimize_hint(&state, 200);
        assert!(!available);
        assert!(hint.contains("pinned"));
        assert!(hint.contains("Adopt"));
    }

    #[test]
    fn optimize_hint_is_available_once_a_tier_is_free_to_move() {
        let mut state = EditorState::fresh();
        state.design.tiers.push(scale_reference_tier(0.5));
        state.design.tiers.push(ConstraintTier {
            angle_deg: -40.0,
            name: "P1".to_string(),
            indices: vec![0.0, 24.0],
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        });

        let (available, hint) = optimize_hint(&state, 200);
        assert!(available);
        assert!(
            hint.contains('1'),
            "expected the free-tier count in: {hint}"
        );
        assert!(hint.to_lowercase().contains("cancel"));
    }

    #[test]
    fn optimize_hint_quotes_the_configured_budget_not_a_hardcoded_number() {
        // The hint must reflect whatever budget the caller
        // passes in, not a literal `200` baked into the format string.
        let mut state = EditorState::fresh();
        state.design.tiers.push(scale_reference_tier(0.5));
        state.design.tiers.push(ConstraintTier {
            angle_deg: -40.0,
            name: "P1".to_string(),
            indices: vec![0.0, 24.0],
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        });

        let (_, hint) = optimize_hint(&state, 750);
        assert!(hint.contains("750"), "expected the budget in: {hint}");
        assert!(!hint.contains("200-evaluation"));
    }

    #[test]
    fn optimize_hint_names_the_canonical_light_pose() {
        // The Optimize panel must say its score is measured
        // under a fixed pose, not the user's own light.
        let mut state = EditorState::fresh();
        state.design.tiers.push(scale_reference_tier(0.5));
        state.design.tiers.push(ConstraintTier {
            angle_deg: -40.0,
            name: "P1".to_string(),
            indices: vec![0.0, 24.0],
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        });

        let (_, hint) = optimize_hint(&state, 200);
        assert!(hint.to_lowercase().contains("canonical light pose"));
    }

    fn design_with_named_tiers(names: &[&str]) -> Design {
        let mut state = EditorState::fresh();
        for &name in names {
            state.design.tiers.push(ConstraintTier {
                angle_deg: -40.0,
                name: name.to_string(),
                indices: vec![0.0],
                constraint: MeetConstraint::MeetExisting,
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            });
        }
        state.design
    }

    // --- deep_solve_tier_rows ---

    #[test]
    fn deep_solve_tier_rows_formats_one_row_per_delta_with_the_tiers_own_name() {
        let design = design_with_named_tiers(&["G1", "P1", "P2"]);
        let deltas = [
            TierMastDelta {
                tier_index: 1,
                before_mast: 1.0,
                after_mast: 1.25,
            },
            TierMastDelta {
                tier_index: 2,
                before_mast: 2.0,
                after_mast: 1.9,
            },
        ];
        let rows = deep_solve_tier_rows(&deltas, &design);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].tier_number.as_str(), "#2");
        assert_eq!(rows[0].name.as_str(), "P1");
        assert_eq!(rows[0].before_mast.as_str(), "1.0000");
        assert_eq!(rows[0].after_mast.as_str(), "1.2500");
        assert_eq!(rows[0].delta.as_str(), "+0.2500");
        // A negative movement stays signed, not just "smaller".
        assert_eq!(rows[1].delta.as_str(), "-0.1000");
    }

    #[test]
    fn deep_solve_tier_rows_is_empty_for_no_deltas() {
        let design = design_with_named_tiers(&["G1"]);
        assert_eq!(deep_solve_tier_rows(&[], &design).len(), 0);
    }

    #[test]
    fn deep_solve_tier_rows_names_an_out_of_range_tier_blank_rather_than_panicking() {
        // A design edited between Deep Solve's dispatch and its completion can
        // shrink the tier list out from under a stale delta -- this must degrade
        // gracefully, not panic or fabricate a name.
        let design = design_with_named_tiers(&["G1"]);
        let deltas = [TierMastDelta {
            tier_index: 5,
            before_mast: 1.0,
            after_mast: 1.1,
        }];
        let rows = deep_solve_tier_rows(&deltas, &design);
        assert_eq!(rows[0].name.as_str(), "");
    }

    // --- optimize_change_rows ---

    #[test]
    fn optimize_change_rows_formats_one_row_per_angle_change_with_the_tiers_own_name() {
        let design = design_with_named_tiers(&["G1", "P1"]);
        let outcome = OptimizeOutcome {
            before: ObjectiveComponents {
                windowing_pct: 0.0,
                extinction_pct: 0.0,
                tilt_brilliance_pct: 0.0,
            },
            before_score: 0.0,
            before_yield_loss_pct: 0.0,
            after: ObjectiveComponents {
                windowing_pct: 0.0,
                extinction_pct: 0.0,
                tilt_brilliance_pct: 0.0,
            },
            after_score: 0.0,
            after_yield_loss_pct: 0.0,
            evaluations: 1,
            changes: vec![indicatrix_cut_core::AngleChange {
                index: 1,
                from_deg: -40.0,
                to_deg: -41.5,
            }],
            cancelled: false,
            polish_evaluations: 0,
            polish_improvement: 0.0,
        };
        let rows = optimize_change_rows(&outcome, &design);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tier_number.as_str(), "#2");
        assert_eq!(rows[0].name.as_str(), "P1");
        assert_eq!(rows[0].from_angle.as_str(), "-40.00\u{b0}");
        assert_eq!(rows[0].to_angle.as_str(), "-41.50\u{b0}");
        assert_eq!(rows[0].delta.as_str(), "-1.50\u{b0}");
    }

    // --- facet_count_from_solved ---

    #[test]
    fn facet_count_from_solved_is_the_length_of_the_designs_expanded_planes() {
        let mut design = design_with_named_tiers(&["G1"]);
        design.tiers[0].constraint = MeetConstraint::ScaleReference(1.0);
        let solved = design
            .solve()
            .expect("a single scale-reference tier always solves");
        // A thin wrapper, so this mostly guards against the wrapper drifting from
        // `Design::planes_from_solved`'s own count rather than testing geometry.
        assert_eq!(
            facet_count_from_solved(&design, &solved),
            design.planes_from_solved(&solved).len()
        );
        assert!(
            facet_count_from_solved(&design, &solved) > 0,
            "a solved design always has at least one facet plane"
        );
    }
}
