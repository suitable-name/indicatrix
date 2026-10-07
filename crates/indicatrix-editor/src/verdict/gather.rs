//! Reading the verdict's inputs off a design and the solve that goes with it.
//!
//! [`gather`] does no solve of its own: the caller hands in the mast list it already has
//! (or `None` when the design did not solve). What it does build is geometry -- the planes,
//! the solid mesh and the manufacturability checks -- which takes a few milliseconds, so a
//! desktop caller runs it on a worker thread with a clone of the design.

use super::{
    ProportionFact, SolveFacts, TierWindowing, VerdictInputs,
    fix::{has_table, live_planes, steep_target_deg, vanished_positions_in},
};
use crate::view_model::{
    row_format::representative_crown_and_pavilion_angles_deg, yield_report::proportion_verdicts,
};
use indicatrix::geometry::{
    meet_solver::{Block, SolvedTier, classify_blocks},
    stone_metrics::{SolidStatus, StoneProportions, build_solid_mesh, measure_solid},
};
use indicatrix_cut_core::{
    DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, Design, ManufacturabilityWarning, Risk,
    critical_angle_deg, crown_window_margin_deg, crown_windowing_risk,
    design::hinge::tier_plane_ranges,
    manufacturability::check_manufacturability_available,
    optics_hints::{MAX_RETARGET_ANGLE_DEG, is_horizontal_angle_deg},
    tier_margin_deg, windowing_risk,
};

/// Said when the design does not solve and the caller has no better sentence.
const GENERIC_FAILURE: &str = "the facets could not be solved.";

/// Said when the mast list does not belong to the design (it changed under the caller).
const OUT_OF_DATE: &str = "the design changed while it was being checked.";

/// The verdict's inputs for `design`.
///
/// `solved` is the mast list of `design`'s solve, or `None` when it did not solve (then
/// `failure` is the solver's sentence, if the caller has one). `n_d` is the design's
/// effective refractive index (the value the tier table's margins use). The optical figures
/// are left out ([`VerdictInputs::optics`] is `None`); a caller measures them separately and
/// adds them with [`VerdictInputs::with_optics`].
#[must_use]
pub fn gather(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    n_d: f64,
    failure: Option<&str>,
) -> VerdictInputs {
    if design.tiers.is_empty() {
        return VerdictInputs::default();
    }
    // `planes_from_solved` panics on a list that is not one entry per tier; a stale list is
    // treated as no solve.
    let (solved, failure) = match solved {
        Some(list) if list.len() == design.tiers.len() => (Some(list), failure),
        Some(_) => (None, Some(OUT_OF_DATE)),
        None => (None, failure),
    };
    let blocks = classify_blocks(&design.meet_tier_inputs());
    let mut inputs = VerdictInputs {
        warnings: check_manufacturability_available(
            design,
            solved,
            DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
        ),
        windowing: windowing_of(design, &blocks, n_d),
        missing_table: missing_table(design, &blocks),
        tier_names: crate::retarget::plan::tier_display_names(design),
        ..VerdictInputs::default()
    };
    let Some(solved) = solved else {
        inputs.solve = SolveFacts::DoesNotSolve(failure.unwrap_or(GENERIC_FAILURE).to_string());
        return inputs;
    };

    let planes = design.planes_from_solved(solved);
    let mesh = match build_solid_mesh(&planes) {
        SolidStatus::Closed(mesh) => mesh,
        SolidStatus::Unbounded { escaping } => {
            inputs.solve = SolveFacts::NotClosed(format!(
                "{} of the facets run off without closing the stone.",
                escaping.len()
            ));
            return inputs;
        }
        SolidStatus::Degenerate { .. } => {
            inputs.solve =
                SolveFacts::NotClosed("the facets enclose no usable volume.".to_string());
            return inputs;
        }
    };
    inputs.solve = SolveFacts::Closed;

    let ranges = tier_plane_ranges(design, solved);
    let live = live_planes(&mesh);
    for warning in &inputs.warnings {
        if let ManufacturabilityWarning::VanishingFacet {
            tier_index,
            vanished,
            total,
            ..
        } = warning
            && vanished < total
            && vanished_positions_in(design, &ranges, &live, *tier_index).is_some()
        {
            inputs.partial_vanish_fixable.insert(*tier_index);
        }
    }

    if let Some(metrics) = measure_solid(&planes) {
        let proportions = StoneProportions::from_solid(&metrics, &mesh, &planes);
        let verdicts = proportion_verdicts(design, &proportions, n_d);
        inputs.proportions = [
            ("The table width", verdicts.table_pct),
            ("The crown angle", verdicts.crown_angle),
            ("The pavilion angle", verdicts.pavilion_angle),
            ("The total depth", verdicts.total_depth_pct),
            ("The girdle thickness", verdicts.girdle_pct),
        ]
        .into_iter()
        .map(|(label, verdict)| ProportionFact {
            label,
            level: verdict.level,
            reason: verdict.reason,
        })
        .collect();
    }
    inputs
}

/// The tiers that let light out, for a material of index `n_d`.
///
/// A pavilion tier counts when its angle is below the critical angle (the tier table's
/// "Windows" badge); a crown tier when the crown-aware estimate against the design's main
/// pavilion angle says it does. Flat facets (the table, the culet) and girdle facets are not
/// judged.
fn windowing_of(design: &Design, blocks: &[Block], n_d: f64) -> Vec<TierWindowing> {
    if !(n_d.is_finite() && n_d > 1.0) {
        return Vec::new();
    }
    let critical = critical_angle_deg(n_d);
    let (_, pavilion_partner) = representative_crown_and_pavilion_angles_deg(design);
    let steepest_fixable = steep_target_deg(n_d) <= MAX_RETARGET_ANGLE_DEG;
    let mut found = Vec::new();
    for (index, (tier, block)) in design.tiers.iter().zip(blocks).enumerate() {
        if is_horizontal_angle_deg(tier.angle_deg) {
            continue;
        }
        match block {
            Block::Pavilion if windowing_risk(tier.angle_deg, n_d) == Risk::Windows => {
                found.push(TierWindowing {
                    tier: index,
                    name: tier.name.clone(),
                    angle_deg: tier.angle_deg.abs(),
                    critical_deg: critical,
                    margin_deg: tier_margin_deg(tier.angle_deg, n_d),
                    crown_estimate: false,
                    fixable: steepest_fixable && !design.is_tier_driven(index),
                });
            }
            Block::Crown => {
                let Some(partner) = pavilion_partner else {
                    continue;
                };
                if crown_windowing_risk(partner, tier.angle_deg, n_d) == Risk::Windows {
                    found.push(TierWindowing {
                        tier: index,
                        name: tier.name.clone(),
                        angle_deg: tier.angle_deg.abs(),
                        critical_deg: critical,
                        margin_deg: crown_window_margin_deg(partner, tier.angle_deg, n_d),
                        crown_estimate: true,
                        fixable: false,
                    });
                }
            }
            Block::Pavilion | Block::Girdle => {}
        }
    }
    found
}

/// Whether the design has a crown (a sloping crown-block tier) but no table.
fn missing_table(design: &Design, blocks: &[Block]) -> bool {
    let has_crown =
        design.tiers.iter().zip(blocks).any(|(tier, block)| {
            *block == Block::Crown && !is_horizontal_angle_deg(tier.angle_deg)
        });
    has_crown && !has_table(design)
}
