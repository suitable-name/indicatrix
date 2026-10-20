//! Precision diagnostics for rounded schedule formats. Native files retain authoring instead.
use super::{
    MeetPreservationBaseline, PreservationFacts, PreservationPolicy, PreservationViolation,
};
use crate::{Design, PreformSpec};
use indicatrix_formats::asc::AscSchedule;

/// Compare independently parsed output with the exact accepted solved schedule.
/// A format that regroups/reorders authored surfaces is explicitly uncertified.
///
/// # Errors
/// Returns a [`PreservationViolation`] when the export regrouped surfaces, the schedule
/// alone does not enclose the stone, or the reopened geometry detaches a meet.
pub fn check_schedule_precision(
    original: &AscSchedule,
    reopened: &AscSchedule,
) -> Result<PreservationFacts, PreservationViolation> {
    if original.tiers.len() != reopened.tiers.len()
        || original
            .tiers
            .iter()
            .zip(&reopened.tiers)
            .any(|(a, b)| a.name != b.name || a.indices.len() != b.indices.len())
    {
        return Err(PreservationViolation::InvalidBaseline(
            "export reordered/regrouped surfaces; meet precision cannot be certified".into(),
        ));
    }
    let extent = original
        .tiers
        .iter()
        .map(|t| t.mast.abs())
        .fold(1.0, f64::max)
        * 32.0;
    let preform = PreformSpec::block(extent, extent, extent);
    let source = Design::from_asc_schedule(preform.clone(), original);
    let baseline = MeetPreservationBaseline::capture(&source)?;
    if baseline.has_live_preform() {
        return Err(PreservationViolation::InvalidBaseline(
            "schedule alone does not enclose the finished stone".into(),
        ));
    }
    let mut candidate = Design::from_asc_schedule(preform, reopened);
    candidate.tier_ids = source.tier_ids;
    // The reopened pinned instructions may have rounded indices/angles/depths. This is
    // a geometry diagnostic, so the strict editable-authoring check is inapplicable.
    baseline.validate_geometry(
        &candidate,
        PreservationPolicy::RETARGET,
        &indicatrix::geometry::meet_solver::SolveControl::default(),
        false,
    )
}
