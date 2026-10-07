//! Refitting the table and culet heights after the angles move.
//!
//! The table and the culet are flat: their angle never changes, but their HEIGHT decides how
//! big they are. A steeper crown pushes the crown's apex up, so a table left at its old
//! height gets larger; a shallower pavilion pulls the apex up too and can lift it past the
//! culet plane, which then no longer touches the stone. A cutter fixes both by re-polishing
//! the flat at the height that gives the same size again.
//!
//! [`refit_flats`] does that for every pinned flat tier that had its own facet: a bisection on
//! the tier's mast, over the candidate's planes with every other mast held, until the flat's
//! facet has the area it had in the original stone. It never re-solves the design inside the
//! loop (only the plane set is rebuilt), so a refit costs a few milliseconds even on a large
//! design; the caller solves the refitted design once at the end and judges that.
//!
//! The search bracket is cut to the stone's own scale (its largest mast), so a design authored
//! at another size -- a girdle mast of 4 rather than the usual 1 -- is refitted as well.

use super::{
    anchors::scale_reference_mast,
    validity::{FlatRing, StoneAnalysis, polygon_area},
};
use indicatrix::geometry::{
    meet_solver::SolvedTier,
    stone_metrics::{SolidStatus, build_solid_mesh},
};
use indicatrix_cut_core::{Design, design::hinge::tier_plane_ranges};

/// A flat facet whose linear size is within this fraction of the original is left alone.
const REFIT_TOLERANCE: f64 = 0.02;

/// The lowest mast the bisection tries, as a fraction of the stone's scale: a flat just off
/// the girdle plane.
const BRACKET_LOW_FRACTION: f64 = 1e-4;

/// The highest mast the bisection tries, as a multiple of the stone's scale: far above any
/// crown apex or below any culet.
const BRACKET_HIGH_FRACTION: f64 = 3.0;

/// The bisection stops when its bracket is narrower than this fraction of the stone's scale.
const STOP_WIDTH_FRACTION: f64 = 1e-10;

/// Upper bound on bisection steps (the bracket halves every step).
const MAX_STEPS: usize = 60;

/// How often the bisection looks at the stop flag.
const STOP_CHECK_EVERY: usize = 8;

/// The new mast of one flat tier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlatRefit {
    /// The tier's position in `design.tiers`.
    pub tier_index: usize,
    /// The mast that gives the flat facet its original size again.
    pub new_mast: f64,
}

/// The area of the facet on plane `plane_index` when tier `tier_index` has mast `mast`
/// and every other mast stays as it is in `solved`.
fn area_at(
    design: &Design,
    solved: &mut [SolvedTier],
    tier_index: usize,
    plane_index: usize,
    mast: f64,
) -> f64 {
    let previous = solved[tier_index].mast;
    solved[tier_index].mast = mast;
    let planes = design.planes_from_solved(solved);
    solved[tier_index].mast = previous;
    match build_solid_mesh(&planes) {
        SolidStatus::Closed(mesh) => mesh
            .rings
            .iter()
            .filter(|(plane, ring)| *plane == plane_index && ring.len() >= 3)
            .map(|(_, ring)| polygon_area(ring))
            .sum(),
        _ => 0.0,
    }
}

/// The scale of the stone `solved` describes: its largest mast magnitude (in a design
/// normalised the usual way that is the girdle's mast, 1), or 1 when nothing is solved.
///
/// Every mast scales together when a design is authored at another size, so the refit's
/// search bracket is cut to this instead of to fixed heights.
fn stone_scale(solved: &[SolvedTier]) -> f64 {
    let largest = solved
        .iter()
        .map(|tier| tier.mast.abs())
        .filter(|mast| mast.is_finite())
        .fold(0.0_f64, f64::max);
    if largest > 0.0 { largest } else { 1.0 }
}

/// Finds the mast of one flat tier that gives its facet `target` area, or `None` when the
/// bracket (relative to the stone's `scale`) does not enclose it.
fn bisect_mast(
    design: &Design,
    solved: &mut [SolvedTier],
    tier_index: usize,
    plane_index: usize,
    target: f64,
    scale: f64,
    should_stop: &dyn Fn() -> bool,
) -> Option<f64> {
    let (mut low, mut high) = (BRACKET_LOW_FRACTION * scale, BRACKET_HIGH_FRACTION * scale);
    // A smaller mast is a flat nearer the girdle plane: a larger facet.
    if area_at(design, solved, tier_index, plane_index, low) < target
        || area_at(design, solved, tier_index, plane_index, high) > target
    {
        return None;
    }
    for step in 0..MAX_STEPS {
        if step % STOP_CHECK_EVERY == 0 && should_stop() {
            return None;
        }
        let mid = f64::midpoint(low, high);
        if area_at(design, solved, tier_index, plane_index, mid) > target {
            low = mid;
        } else {
            high = mid;
        }
        if high - low < STOP_WIDTH_FRACTION * scale {
            break;
        }
    }
    Some(f64::midpoint(low, high))
}

/// The refits that bring every pinned flat facet of `candidate` back to the size it had in
/// `original`.
///
/// `candidate_solved` is the candidate design's solved masts and `candidate_flats` its flat
/// rings. A flat whose facet is already within [`REFIT_TOLERANCE`] of the original size (in
/// linear size), a flat that is not pinned by a `ScaleReference`, and a flat the bisection
/// cannot bracket are all left out.
#[must_use]
pub fn refit_flats(
    candidate: &Design,
    candidate_solved: &[SolvedTier],
    candidate_flats: &[FlatRing],
    original: &StoneAnalysis,
    should_stop: &dyn Fn() -> bool,
) -> Vec<FlatRefit> {
    let ranges = tier_plane_ranges(candidate, candidate_solved);
    let scale = stone_scale(candidate_solved);
    let mut work = candidate_solved.to_vec();
    let mut refits = Vec::new();
    for flat in &original.flats {
        if should_stop() {
            break;
        }
        if scale_reference_mast(candidate, flat.tier_index).is_none() {
            continue;
        }
        let target = flat.area;
        if target.is_nan() || target <= 0.0 {
            continue;
        }
        let current = candidate_flats
            .iter()
            .find(|other| other.tier_index == flat.tier_index)
            .map_or(0.0, |other| other.area);
        if ((current / target).sqrt() - 1.0).abs() < REFIT_TOLERANCE {
            continue;
        }
        let Some(range) = ranges.get(flat.tier_index) else {
            continue;
        };
        let Some(new_mast) = bisect_mast(
            candidate,
            &mut work,
            flat.tier_index,
            range.start,
            target,
            scale,
            should_stop,
        ) else {
            continue;
        };
        work[flat.tier_index].mast = new_mast;
        refits.push(FlatRefit {
            tier_index: flat.tier_index,
            new_mast,
        });
    }
    refits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retarget::validity::analyze;
    use indicatrix::geometry::meet_solver::{MeetConstraint, SolveStrategy};
    use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};

    fn brilliant() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    fn tier_named(design: &Design, name: &str) -> usize {
        design.tiers.iter().position(|t| t.name == name).unwrap()
    }

    #[test]
    fn an_unchanged_stone_needs_no_refit() {
        let design = brilliant();
        let original = analyze(&design, false).unwrap();
        let refits = refit_flats(
            &design,
            &original.solved,
            &original.flats,
            &original,
            &|| false,
        );
        assert_eq!(refits, Vec::new());
    }

    #[test]
    fn a_table_that_grew_is_raised_back_to_its_original_size() {
        let design = brilliant();
        let original = analyze(&design, false).unwrap();
        let table = tier_named(&design, "Table");
        let original_area = original
            .flats
            .iter()
            .find(|flat| flat.tier_index == table)
            .unwrap()
            .area;

        // A lower table is a bigger table.
        let mut lowered = design;
        lowered.tiers[table].constraint = MeetConstraint::ScaleReference(0.2);
        let candidate = analyze(&lowered, false).unwrap();
        let grown = candidate
            .flats
            .iter()
            .find(|flat| flat.tier_index == table)
            .unwrap()
            .area;
        assert!(grown > original_area * 1.05, "{grown} vs {original_area}");

        let refits = refit_flats(
            &lowered,
            &candidate.solved,
            &candidate.flats,
            &original,
            &|| false,
        );
        let refit = refits
            .iter()
            .find(|refit| refit.tier_index == table)
            .expect("the table must be refitted");
        // Back to the original table height (0.32 in the template).
        assert!((refit.new_mast - 0.32).abs() < 1e-3, "{}", refit.new_mast);

        // And refitting really gives the size back.
        lowered.tiers[table].constraint = MeetConstraint::ScaleReference(refit.new_mast);
        let fitted = analyze(&lowered, false).unwrap();
        let area = fitted
            .flats
            .iter()
            .find(|flat| flat.tier_index == table)
            .unwrap()
            .area;
        assert!(
            (area / original_area - 1.0).abs() < 1e-3,
            "{area} vs {original_area}"
        );
    }

    /// The brilliant authored at another size: every `ScaleReference` mast multiplied by
    /// `scale`, in a preform block big enough to hold it.
    fn scaled_brilliant(scale: f64) -> Design {
        let mut tiers = ConstraintTier::standard_round_brilliant();
        for tier in &mut tiers {
            if let MeetConstraint::ScaleReference(mast) = &mut tier.constraint {
                *mast *= scale;
            }
        }
        Design::new(
            PreformSpec::block(2.0 * scale, 1.0, 4.0 * scale),
            ScheduleMeta::standard_round_brilliant(),
            tiers,
        )
    }

    #[test]
    fn a_stone_authored_at_a_larger_scale_still_gets_its_culet_refitted() {
        // The culet's own mast is 0.88 * 4 = 3.52 here, beyond the fixed 3.0 the bracket used
        // to stop at, so the old absolute bracket found nothing to refit.
        let scale = 4.0;
        let design = scaled_brilliant(scale);
        let original = analyze(&design, false).unwrap();
        let culet = tier_named(&design, "Culet");
        let original_area = original
            .flats
            .iter()
            .find(|flat| flat.tier_index == culet)
            .unwrap()
            .area;

        // A culet nearer the girdle is a bigger culet.
        let mut shallow = design;
        shallow.tiers[culet].constraint = MeetConstraint::ScaleReference(0.75 * scale);
        let candidate = analyze(&shallow, false).unwrap();
        let grown = candidate
            .flats
            .iter()
            .find(|flat| flat.tier_index == culet)
            .unwrap()
            .area;
        assert!(grown > original_area * 1.05, "{grown} vs {original_area}");

        let refits = refit_flats(
            &shallow,
            &candidate.solved,
            &candidate.flats,
            &original,
            &|| false,
        );
        let refit = refits
            .iter()
            .find(|refit| refit.tier_index == culet)
            .expect("the culet of a large-scale stone must be refitted");
        let original_culet_mast = 0.88 * scale;
        assert!(
            (refit.new_mast - original_culet_mast).abs() < 1e-3 * scale,
            "{}",
            refit.new_mast
        );
    }

    #[test]
    fn the_stone_scale_is_the_largest_mast_or_one() {
        let tier = |mast: f64| SolvedTier {
            mast,
            strategy: SolveStrategy::ScaleReference,
            detail: String::new(),
        };
        assert!((stone_scale(&[tier(0.3), tier(-4.0), tier(1.0)]) - 4.0).abs() < 1e-12);
        assert!((stone_scale(&[tier(f64::NAN), tier(0.5)]) - 0.5).abs() < 1e-12);
        assert!((stone_scale(&[]) - 1.0).abs() < 1e-12);
        assert!((stone_scale(&[tier(0.0)]) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_stop_request_ends_the_refit_early() {
        let design = brilliant();
        let original = analyze(&design, false).unwrap();
        let table = tier_named(&design, "Table");
        let mut lowered = design;
        lowered.tiers[table].constraint = MeetConstraint::ScaleReference(0.2);
        let candidate = analyze(&lowered, false).unwrap();
        let refits = refit_flats(
            &lowered,
            &candidate.solved,
            &candidate.flats,
            &original,
            &|| true,
        );
        assert_eq!(refits, Vec::new());
    }
}
