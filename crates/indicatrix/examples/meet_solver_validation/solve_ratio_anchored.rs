//! Report B/D: the production ratio-anchoring path -- anchors each block from the
//! design's own printed `C/W`/`P/W` proportions instead of bootstrapping from the
//! file's real tier-0 mast, which is what actually reflects production usability
//! (most catalogued designs have no `.asc` file, hence no recorded masts to
//! bootstrap from).

use indicatrix::geometry::{
    meet_solver::{
        MeetConstraint, apply_ratio_anchors, meet_tier_inputs_from_asc, solve_meet_points,
        solve_meet_points_verified,
    },
    stone_metrics::ExternalProportions,
};
use indicatrix_formats::asc;

use crate::{
    scoring::{classify_named_resolution, score_solved_tiers},
    types::{AscRow, ConstraintKind, DesignResult},
};

/// The production ratio-anchoring path: like [`solve_one`], but anchors each block
/// from the design's own printed `C/W`/`P/W` proportions
/// ([`apply_ratio_anchors`]) instead of bootstrapping from the file's real tier-0
/// mast. This produces the measurement that matters for "is the solver usable in
/// production": `solve_one`'s tier-0-real-mast bootstrap is a harness-only crutch
/// (real usage, especially the ~2,700 catalogued designs with no `.asc` file at all,
/// has no recorded masts to bootstrap from), so a bootstrap-free measurement is the
/// number that actually reflects production usability.
///
/// A block whose ratio is `None` (a partial diagram) or that `apply_ratio_anchors`
/// otherwise couldn't cover still falls back to its own tier-0 real mast, exactly
/// like `solve_one`, so every design remains solvable and comparable -- but that
/// fallback is recorded (`DesignResult::fully_ratio_anchored`) and every anchor
/// tier, whichever path supplied it, is excluded from scoring below: an anchor is
/// *given*, not *solved*, so counting it as a free zero-error tier would flatter the
/// numbers.
pub fn solve_one_ratio_anchored(row: &AscRow) -> DesignResult {
    solve_one_ratio_anchored_impl(row, false)
}

/// Report D: the production path end-to-end -- printed-ratio anchors like
/// [`solve_one_ratio_anchored`], but solved via [`solve_meet_points_verified`]
/// with the ratio-derived crown/pavilion anchors marked *adjustable* (the
/// search calibrates them against the printed figures) and the printed
/// proportions as the repair/verification targets. Anchoring and scoring
/// conventions are otherwise identical to Report B, so the two are directly
/// comparable.
pub fn solve_one_ratio_anchored_verified(row: &AscRow) -> DesignResult {
    solve_one_ratio_anchored_impl(row, true)
}

fn solve_one_ratio_anchored_impl(row: &AscRow, verified: bool) -> DesignResult {
    let mut out = DesignResult::default();

    let text = String::from_utf8_lossy(&row.content);
    let Ok(schedule) = asc::parse_asc(&text) else {
        return out; // parse_ok stays false
    };
    out.parse_ok = true;
    if schedule.tiers.is_empty() {
        return out;
    }

    let mut tiers = meet_tier_inputs_from_asc(&schedule);
    let original_kinds: Vec<ConstraintKind> = tiers
        .iter()
        .map(|t| match &t.constraint {
            MeetConstraint::ScaleReference(_) => ConstraintKind::ScaleReference,
            MeetConstraint::MeetNamed(_) => ConstraintKind::MeetNamed,
            MeetConstraint::MeetExisting => ConstraintKind::MeetExisting,
        })
        .collect();
    let original_named_names: Vec<Option<Vec<String>>> = tiers
        .iter()
        .map(|t| match &t.constraint {
            MeetConstraint::MeetNamed(names) => Some(names.clone()),
            _ => None,
        })
        .collect();
    out.has_meet_named = original_kinds.contains(&ConstraintKind::MeetNamed);
    out.has_meet_existing = original_kinds.contains(&ConstraintKind::MeetExisting);
    out.has_scale_reference = original_kinds.contains(&ConstraintKind::ScaleReference);

    apply_ratio_anchors(&mut tiers, row.cw_ratio, row.pw_ratio);

    // Anything the ratio path newly turned into a ScaleReference (i.e. wasn't
    // already one before the call) is a ratio-derived anchor -- given, not solved.
    let ratio_anchored: Vec<bool> = (0..tiers.len())
        .map(|i| {
            original_kinds[i] != ConstraintKind::ScaleReference
                && matches!(tiers[i].constraint, MeetConstraint::ScaleReference(_))
        })
        .collect();

    // Any block that still has no anchor at all (its ratio was `None`, or it had no
    // tiers for `apply_ratio_anchors` to pick from) falls back to its own tier-0
    // real mast, same convention as `solve_one`, so the design stays solvable.
    let mut fallback_anchored: Vec<bool> = vec![false; tiers.len()];
    let mut fully_ratio_anchored = true;
    {
        use indicatrix::geometry::meet_solver::classify_blocks;
        let blocks = classify_blocks(&tiers);
        for block in [
            indicatrix::geometry::meet_solver::Block::Crown,
            indicatrix::geometry::meet_solver::Block::Pavilion,
            indicatrix::geometry::meet_solver::Block::Girdle,
        ] {
            let members: Vec<usize> = (0..tiers.len()).filter(|&i| blocks[i] == block).collect();
            let has_anchor = members
                .iter()
                .any(|&i| matches!(tiers[i].constraint, MeetConstraint::ScaleReference(_)));
            if let (false, Some(&first)) = (has_anchor, members.first()) {
                tiers[first].constraint =
                    MeetConstraint::ScaleReference(schedule.tiers[first].mast);
                fallback_anchored[first] = true;
                fully_ratio_anchored = false;
            }
        }
    }
    out.fully_ratio_anchored = fully_ratio_anchored;

    let named_resolution = classify_named_resolution(&tiers, &original_named_names);

    let solved = if verified {
        let targets = ExternalProportions {
            vol_w3: row.volume,
            lw: row.lw_ratio,
            cw: row.cw_ratio,
            pw: row.pw_ratio,
            hw: row.hw_ratio,
        };
        // Ratio-derived crown/pavilion anchors are estimates the search may
        // calibrate; the girdle reference is pure unit choice (every target is
        // scale-invariant), and real-mast fallback anchors are exact.
        let blocks = indicatrix::geometry::meet_solver::classify_blocks(&tiers);
        let adjustable: Vec<usize> = (0..tiers.len())
            .filter(|&i| {
                ratio_anchored[i] && blocks[i] != indicatrix::geometry::meet_solver::Block::Girdle
            })
            .collect();
        let (solved, report) =
            solve_meet_points_verified(schedule.gear_teeth_abs(), &tiers, &targets, &adjustable);
        out.verify = Some(report);
        solved
    } else {
        solve_meet_points(schedule.gear_teeth_abs(), &tiers)
    };
    if solved.len() != schedule.tiers.len() {
        return out;
    }

    score_solved_tiers(
        &schedule,
        &solved,
        &original_kinds,
        &named_resolution,
        |i| {
            original_kinds[i] == ConstraintKind::ScaleReference
                || ratio_anchored[i]
                || fallback_anchored[i]
        },
        &mut out,
    );
    out
}
