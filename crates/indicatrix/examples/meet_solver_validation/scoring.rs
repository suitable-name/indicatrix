//! Scoring and name-resolution classification shared by both solving strategies
//! (real-mast bootstrap and ratio-anchored): scores a solved design's meet-derived
//! tiers against the schedule's own recorded masts, and classifies each tier's
//! named-meet resolution via the solver's own resolver.

use indicatrix_formats::asc;

use crate::types::{ConstraintKind, DesignResult, TierResult};

/// Scores every meet-derived tier of one solved design against the schedule's own
/// real recorded masts, filling `out.tier_results` and `out.worst_err`. Tiers for
/// which `is_given` returns true (stated scale references, bootstrap or ratio
/// anchors) are excluded -- an anchor is *given*, not *solved*, so counting it
/// would flatter the numbers -- as are near-zero-mast tiers (relative error is
/// undefined there).
pub fn score_solved_tiers(
    schedule: &asc::AscSchedule,
    solved: &[indicatrix::geometry::meet_solver::SolvedTier],
    original_kinds: &[ConstraintKind],
    named_resolution: &[Option<bool>],
    is_given: impl Fn(usize) -> bool,
    out: &mut DesignResult,
) {
    let mut worst: Option<f64> = None;
    for (i, sol) in solved.iter().enumerate() {
        let real = schedule.tiers[i].mast.abs();
        if is_given(i) || real < 1e-6 {
            continue;
        }
        let rel_err = (sol.mast - real).abs() / real;
        out.tier_results.push(TierResult {
            strategy: sol.strategy,
            rel_err,
            kind: original_kinds[i],
            named_resolved: named_resolution[i],
            used_named: sol.detail.contains("named reference"),
            fallback_cause: if sol.detail.contains("refs not yet settled") {
                'u'
            } else if sol.detail.contains("no feasible level incident") {
                'n'
            } else {
                ' '
            },
        });
        worst = Some(worst.map_or(rel_err, |w: f64| w.max(rel_err)));
    }
    out.worst_err = worst;
}

/// Classifies each tier's name resolution via the *same* [`MeetNameResolver`] the
/// solver itself uses at solve time (no mirrored logic to drift out of sync).
/// `tiers` must be in the same state (post scale-reference bootstrap) passed to
/// `solve_meet_points`; `original_names` supplies each tier's stated name list
/// from *before* that bootstrap could have overwritten a `MeetNamed` tier 0's
/// constraint.
///
/// Returns `None` for any tier that wasn't originally `MeetNamed`; `Some(true)`
/// iff the stated list fully resolved (every token resolved to a tier or was a
/// recognized non-facet word, and at least one token actually named a tier --
/// see [`indicatrix::geometry::meet_solver::ResolvedNames::fully`]).
pub fn classify_named_resolution(
    tiers: &[indicatrix::geometry::meet_solver::MeetTierInput],
    original_names: &[Option<Vec<String>>],
) -> Vec<Option<bool>> {
    let resolver = indicatrix::geometry::meet_solver::MeetNameResolver::new(tiers);
    original_names
        .iter()
        .map(|names| {
            names
                .as_ref()
                .map(|names| resolver.resolve_names(names).fully)
        })
        .collect()
}
