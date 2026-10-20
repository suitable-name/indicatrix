//! Deep Solve's availability hint, status text and per-tier result table
//! ([`deep_solve_hint`]/[`format_deep_solve_report`]/[`deep_solve_tier_rows`] --
//! Deep Solve is desktop-only), and the Slint adapters over
//! `indicatrix_editor::optimize_view`'s Optimize hint and change rows (shared with
//! the web app, re-exported here at their old paths). See
//! [`super::optimize_apply`] for the Optimize result SUMMARY table.

use super::state::EditorState;
use crate::{DeepSolveTierRow, OptimizeChangeRow, gui::editor::deep_solve::TierMastDelta};
use indicatrix_cut_core::{Design, OptimizeOutcome};
use indicatrix_editor::optimize_view::tier_name_for_row;

pub(in crate::gui::editor) use indicatrix_editor::optimize_view::facet_count_from_solved;

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
    indicatrix_editor::deep_solve::availability(&state.design, state.printed_proportions.as_ref())
}

/// Renders a completed Deep Solve's
/// [`VerifiedSolveReport`](indicatrix::geometry::meet_solver::VerifiedSolveReport) as the status banner
/// text -- honestly: always shows the actual score movement and run count, never
/// just a pass/fail badge, and never claims `accepted` proves the geometry is right.
pub(in crate::gui::editor) use indicatrix_editor::deep_solve::format_deep_solve_report;

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

/// [`indicatrix_editor::optimize_view::optimize_change_rows`], mapped to the
/// Optimize tab's "Tiers this would change" Slint rows.
///
/// # Call site
/// `callbacks::solve_actions::handle_optimize_outcome` calls this alongside
/// [`super::optimize_apply::optimize_result_rows`] and pushes the result to
/// `EditorModel.optimize_change_rows`.
#[must_use]
pub(in crate::gui::editor) fn optimize_change_rows(
    outcome: &OptimizeOutcome,
    design: &Design,
) -> Vec<OptimizeChangeRow> {
    indicatrix_editor::optimize_view::optimize_change_rows(outcome, design)
        .into_iter()
        .map(|line| OptimizeChangeRow {
            tier_number: line.tier_number.into(),
            name: line.name.into(),
            from_angle: line.from_angle.into(),
            to_angle: line.to_angle.into(),
            delta: line.delta.into(),
        })
        .collect()
}

/// Whether Optimize is available for `state`'s current design, and the hint shown
/// next to its button either way -- see
/// [`indicatrix_editor::optimize_view::optimize_hint`]. `max_evaluations` is the
/// CONFIGURED budget (`super::panel_stale::configured_optimize_max_evaluations`).
///
/// `pub(super)`, not private: `panel_stale.rs` (a sibling file) calls this from
/// its own availability push.
pub(super) fn optimize_hint(state: &EditorState, max_evaluations: usize) -> (bool, String) {
    indicatrix_editor::optimize_view::optimize_hint(&state.design, max_evaluations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::ConstraintTier;

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
        state.session.design
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
}
