//! Checks 3 and 4: [`check_gear_quantization`] and [`check_cut_order`], the
//! two that need no solved mast at all -- see the parent module's doc
//! comment. Both read `design`'s raw authored state directly, so they run
//! even on a design that has never been solved.

use super::warning::ManufacturabilityWarning;
use crate::design::Design;
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
#[must_use]
pub fn check_gear_quantization(design: &Design) -> Vec<ManufacturabilityWarning> {
    let gear_teeth = f64::from(design.meta.gear_teeth_abs().max(1));
    let mut warnings = Vec::new();
    for (tier_index, tier) in design.tiers.iter().enumerate() {
        for &requested in &tier.indices {
            let achievable = requested.round();
            let delta = requested - achievable;
            if delta.abs() > GEAR_QUANTIZATION_EPS {
                warnings.push(ManufacturabilityWarning::FractionalIndex {
                    tier_index,
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
                    tier_name: tier.name.clone(),
                    target_tier_index: target,
                    target_tier_name: design.tiers[target].name.clone(),
                });
            }
        }
    }
    warnings
}
