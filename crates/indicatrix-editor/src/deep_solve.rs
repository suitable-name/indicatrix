//! Shared Deep Solve availability and diagnostic presentation. No UI or mutation.
use indicatrix::geometry::{
    meet_solver::{MeetConstraint, SolvedTier, VerifiedSolveReport},
    stone_metrics::ExternalProportions,
};
use indicatrix_cut_core::Design;

/// Only externally supplied targets can verify a design. Concave surfaces are
/// excluded because the verified solver measures the planar solve.
pub fn availability(design: &Design, targets: Option<&ExternalProportions>) -> (bool, String) {
    let Some(targets) = targets else {
        return (false, "Deep Solve needs this design's printed proportions. Enter the figures from its published source.".into());
    };
    let values = [
        targets.vol_w3,
        targets.lw,
        targets.cw,
        targets.pw,
        targets.hw,
    ];
    if values.iter().all(Option::is_none)
        || values
            .iter()
            .flatten()
            .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return (
            false,
            "Enter at least one finite positive printed proportion from the source.".into(),
        );
    }
    if !design.concave_tiers.is_empty() {
        return (
            false,
            "Deep Solve does not yet verify concave tool geometry.".into(),
        );
    }
    if design.meta.gear_teeth == 0 || design.tiers.is_empty() {
        return (
            false,
            "Add valid tiers and a positive index gear before Deep Solve.".into(),
        );
    }
    if !design
        .tiers
        .iter()
        .any(|tier| !matches!(tier.constraint, MeetConstraint::ScaleReference(_)))
    {
        return (false, "Every tier is pinned. Use Adopt on an imported meet before Deep Solve has anything to repair.".into());
    }
    (true, "Slow: runs locally off the UI thread and can be cancelled. Verification never edits constraints.".into())
}

/// One tier's mast disagreement between a plain solve and Deep Solve's externally
/// verified repair.
///
/// This is the per-tier breakdown behind
/// [`VerifiedSolveReport`]'s aggregate verdict. Display-only, like the report itself:
/// nothing here is ever written back to a `Design`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TierMastDelta {
    /// This tier's index in `Design::tiers` (and in `Design::solve`'s parallel
    /// output).
    pub tier_index: usize,
    /// The plain solve's mast for this tier, before verification.
    pub before_mast: f64,
    /// Deep Solve's verified mast for this tier.
    pub after_mast: f64,
}

impl TierMastDelta {
    /// The signed movement `after_mast - before_mast` -- positive means Deep Solve
    /// pushed the mast further from the girdle, negative means closer.
    #[must_use]
    pub fn delta(&self) -> f64 {
        self.after_mast - self.before_mast
    }
}

/// A mast movement below this (in the same units as `SolvedTier::mast`, i.e. mast
/// order) is treated as "unchanged" -- filters out the floating-point noise a tier
/// Deep Solve genuinely left alone would otherwise show as a spurious sub-millitier
/// disagreement.
const MAST_DELTA_EPSILON: f64 = 1e-6;

/// Zips `current` (a plain [`indicatrix_cut_core::Design::solve`]) against `verified`
/// (a Deep Solve's own [`DeepSolveOutcome::Completed::solved`]) tier by tier.
///
/// Keeps only the tiers whose mast actually moved by more than [`MAST_DELTA_EPSILON`] --
/// the per-tier breakdown [`DeepSolveOutcome`]'s own doc comment describes. Both
/// slices are expected in the same tier order (`Design::meet_tier_inputs`'s own
/// convention); this pairs them positionally and silently stops at the shorter of the
/// two lengths rather than panicking, since a design edited between the plain solve
/// and Deep Solve's completion can change tier count -- the caller already flags that
/// case `stale` independently (`EditorState::generation`).
#[must_use]
pub fn tier_mast_deltas(current: &[SolvedTier], verified: &[SolvedTier]) -> Vec<TierMastDelta> {
    current
        .iter()
        .zip(verified.iter())
        .enumerate()
        .filter_map(|(tier_index, (before, after))| {
            let delta = after.mast - before.mast;
            (delta.abs() > MAST_DELTA_EPSILON).then_some(TierMastDelta {
                tier_index,
                before_mast: before.mast,
                after_mast: after.mast,
            })
        })
        .collect()
}

/// Renders [`tier_mast_deltas`]'s result as a status-line suffix naming which tiers
/// Deep Solve's verified repair actually disagreed with the plain solve on.
///
/// It also gives by how much -- the minimal consumer [`DeepSolveOutcome`]'s own doc comment describes,
/// so this per-tier data is never dead: `format_deep_solve_report`
/// (`gui::editor::view`) still owns the aggregate verdict line; this is appended
/// after it. Returns `""` when `deltas` is empty (report already reads as full
/// agreement, and an empty sentence would just be noise).
#[must_use]
pub fn format_tier_mast_deltas(deltas: &[TierMastDelta]) -> String {
    if deltas.is_empty() {
        return String::new();
    }
    let tiers: Vec<String> = deltas
        .iter()
        .map(|d| format!("tier {} ({:+.4})", d.tier_index, d.delta()))
        .collect();
    format!(" Disagrees with the last solve on: {}.", tiers.join(", "))
}

#[must_use]
pub fn format_deep_solve_report(report: &VerifiedSolveReport, stale: bool) -> String {
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
