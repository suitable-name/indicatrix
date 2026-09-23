//! Checks 3, 4 and 5: [`check_gear_quantization`], [`check_cut_order`] and
//! [`check_meet_name_asc_safety`], the three that need no solved mast at all --
//! see the parent module's doc comment. All three read `design`'s raw authored
//! state directly, so they run even on a design that has never been solved.

use super::warning::ManufacturabilityWarning;
use crate::design::{Design, meet_name_is_asc_safe};
use indicatrix::geometry::meet_solver::{MeetConstraint, MeetNameResolver};

/// An index within this absolute distance of an integer counts as landing on
/// a real gear tooth. Real `.asc` index tokens are written as plain decimal
/// literals (`0`, `24`, `47.5`) that parse to exact or near-exact `f64`
/// values, so this only needs to absorb text/float round-trip noise, not
/// measurement error -- `1e-9` is generous for that while still catching a
/// genuine half-tooth position like `47.5` (error `0.5`, eleven orders of
/// magnitude over the threshold).
const GEAR_QUANTIZATION_EPS: f64 = 1e-9;

/// Check 3: an authored index-wheel position does not land on a real gear
/// tooth -- see the module docs.
///
/// Needs no mast at all, so this runs off `design`'s raw authored `indices`
/// regardless of whether the design has ever been solved.
///
/// Reports nothing at all when `design.meta.gear_teeth_abs() == 0`: a
/// `.max(1)` fallback would compute an `azimuth_error_deg` as if this design had
/// a real one-tooth wheel (`360 * delta`), which is not a gear any lapidary owns
/// and is not a meaningful figure to show -- with no real gear stated, there is
/// nothing to quantize against, so the honest answer is no warning rather than a
/// fabricated one.
#[must_use]
pub fn check_gear_quantization(design: &Design) -> Vec<ManufacturabilityWarning> {
    let gear_teeth_abs = design.meta.gear_teeth_abs();
    if gear_teeth_abs == 0 {
        return Vec::new();
    }
    let gear_teeth = f64::from(gear_teeth_abs);
    let mut warnings = Vec::new();
    for (tier_index, tier) in design.tiers.iter().enumerate() {
        for &requested in &tier.indices {
            let achievable = requested.round();
            let delta = requested - achievable;
            if delta.abs() > GEAR_QUANTIZATION_EPS {
                warnings.push(ManufacturabilityWarning::FractionalIndex {
                    tier_index,
                    tier_id: design.tier_id_at_or_synthetic(tier_index),
                    tier_name: tier.name.clone(),
                    requested,
                    achievable,
                    azimuth_error_deg: 360.0 * delta / gear_teeth,
                });
            }
        }
    }
    warnings
}

/// Check 4: a tier's stated meet target comes at or after the tier itself in
/// the schedule -- see the module docs.
///
/// Needs no mast either: this resolves
/// `design`'s own authored [`MeetConstraint::MeetNamed`] references via the
/// exact same [`MeetNameResolver`] `indicatrix::geometry::meet_solver::solve_meet_points`
/// itself uses (reused, not reimplemented, so this check's notion of "which
/// tier does this name" can never disagree with what the solver actually
/// solved against), and only reports a resolved reference whose target index
/// is `>=` the referencing tier's own index -- a self-reference (index equal
/// to `tier_index`) is included too, since a tier obviously cannot meet a
/// facet that is itself not yet cut.
///
/// An unresolved name, or one the schedule states as [`MeetConstraint::MeetExisting`]/
/// [`MeetConstraint::ScaleReference`], is out of scope here -- this check is
/// specifically about a reference that *did* resolve to a real tier, just the
/// wrong-in-time one.
#[must_use]
pub fn check_cut_order(design: &Design) -> Vec<ManufacturabilityWarning> {
    let inputs = design.meet_tier_inputs();
    let resolver = MeetNameResolver::new(&inputs);
    let mut warnings = Vec::new();
    for (tier_index, tier) in design.tiers.iter().enumerate() {
        let MeetConstraint::MeetNamed(names) = &tier.constraint else {
            continue;
        };
        let resolved = resolver.resolve_names(names);
        for &target in &resolved.refs {
            if target >= tier_index {
                warnings.push(ManufacturabilityWarning::OutOfOrderMeet {
                    tier_index,
                    tier_id: design.tier_id_at_or_synthetic(tier_index),
                    tier_name: tier.name.clone(),
                    target_tier_index: target,
                    target_tier_name: design.tiers[target].name.clone(),
                });
            }
        }
    }
    warnings
}

/// Check 5: a tier's [`MeetConstraint::MeetNamed`] names a target that would not
/// survive a plain `.asc` export/re-import.
///
/// See [`meet_name_is_asc_safe`]'s own doc comment for exactly which names fail
/// and why (`crate::design::export`'s own `"Meet <names>"` text is the export
/// this guards). Needs no mast either, same as [`check_cut_order`]: purely a
/// property of the tier's own authored name list.
#[must_use]
pub fn check_meet_name_asc_safety(design: &Design) -> Vec<ManufacturabilityWarning> {
    let mut warnings = Vec::new();
    for (tier_index, tier) in design.tiers.iter().enumerate() {
        let MeetConstraint::MeetNamed(names) = &tier.constraint else {
            continue;
        };
        let unsafe_names: Vec<String> = names
            .iter()
            .filter(|name| !meet_name_is_asc_safe(name.as_str()))
            .cloned()
            .collect();
        if !unsafe_names.is_empty() {
            warnings.push(ManufacturabilityWarning::MeetNameNotAscSafe {
                tier_index,
                tier_id: design.tier_id_at_or_synthetic(tier_index),
                tier_name: tier.name.clone(),
                unsafe_names,
            });
        }
    }
    warnings
}
