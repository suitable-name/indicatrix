//! Timing one search step.
//!
//! A step of the search is a PROJECTED proposal: the proposal is pulled back onto the original
//! meet network by a damped least-squares loop that solves the design several times, and then the
//! stone is measured. The time the dialog quotes per step is therefore the time of one projection
//! plus one fast optical measurement, not of one bare solve.

use crate::retarget::{
    anchors::fold_relations, metrics::measure_analysis_column, validity::analyze,
};
use indicatrix::{
    geometry::meet_solver::MeetConstraint,
    optics::{LightingPreset, materials::GemMaterial},
};
use indicatrix_cut_core::{
    Design, OptimizeOptions,
    design::hinge::tier_hinges,
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy, SearchDiagnostics},
    optimize::project_preserved_design,
};
use std::sync::Arc;

/// The common y stretch the probe's proposal asks for. Small, like the first proposals of the
/// search's axial family, so the projection has the same kind of work to do.
const PROBE_STRETCH: f64 = 1.001;

/// What one probe found.
pub(super) struct Probe {
    /// `true` when the design solves and closes.
    pub(super) solved: bool,
    /// The baseline's counters after the one projection; `None` when no projection could run
    /// (the baseline could not be captured, or the stretched proposal does not settle its
    /// relations).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "only the test that checks a projection ran reads it"
        )
    )]
    pub(super) diagnostics: Option<SearchDiagnostics>,
}

/// `design` stretched by [`PROBE_STRETCH`] along y: every free angle and every pinned depth
/// follows the stretch (the same transform as the search's axial proposals), and the tiers that
/// follow a relation take the angle their relation gives. `None` when the relations cannot be
/// satisfied afterwards.
fn stretched(design: &Design) -> Option<Design> {
    let mut candidate = design.clone();
    for (index, tier) in candidate.tiers.iter_mut().enumerate() {
        let angle = tier.angle_deg.abs().to_radians();
        let norm = angle.sin().hypot(angle.cos() / PROBE_STRETCH);
        if !design.is_tier_driven(index) {
            tier.angle_deg = angle
                .sin()
                .atan2(angle.cos() / PROBE_STRETCH)
                .to_degrees()
                .copysign(tier.angle_deg);
        }
        if design.tier_target(index).is_none()
            && let MeetConstraint::ScaleReference(mast) = &mut tier.constraint
        {
            *mast /= norm;
        }
    }
    fold_relations(&mut candidate).ok()?;
    Some(candidate)
}

/// One probe: solve and measure `design`, project one stretched proposal against its own meet
/// network, and take one fast optical measurement.
///
/// The projection's own result does not matter here (a refused proposal costs its solves all
/// the same); only that it ran.
pub(super) fn run_probe(design: &Design, gem: &GemMaterial, lighting: LightingPreset) -> Probe {
    let Ok(analysis) = analyze(design, false) else {
        return Probe {
            solved: false,
            diagnostics: None,
        };
    };
    let diagnostics = if let Ok(baseline) = MeetPreservationBaseline::capture(design)
        && let Some(proposal) = stretched(design)
    {
        let baseline = Arc::new(baseline);
        let options = OptimizeOptions {
            preservation_baseline: Some(baseline.clone()),
            preservation_policy: PreservationPolicy::RETARGET,
            vary_anchored: true,
            anchor_hinges: tier_hinges(design, &analysis.solved)
                .iter()
                .map(|(&index, hinge)| (index, hinge.point))
                .collect(),
            ..OptimizeOptions::default()
        };
        let _ = project_preserved_design(design, &proposal, &options, None);
        Some(baseline.diagnostics())
    } else {
        None
    };
    let _ = measure_analysis_column(&analysis, gem, lighting);
    Probe {
        solved: true,
        diagnostics,
    }
}

/// Runs one step of the kind a search repeats (a projected proposal and one fast optical
/// measurement) so the caller can time it. `false` when the design does not solve and close.
///
/// The time includes capturing the design's meet network, which a search pays once and not
/// per step, so the estimate it gives errs on the long side. The editor crate keeps no clock
/// of its own: the caller takes the time around this call.
#[must_use]
pub fn probe_evaluation(design: &Design, gem: &GemMaterial, lighting: LightingPreset) -> bool {
    run_probe(design, gem, lighting).solved
}
