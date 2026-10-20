//! What picking an Optimize option puts in the check slot, without a window.
//!
//! The search already captured the operation-start meet network and validated every option
//! against it (`SearchReport.baseline`), so picking an option reuses that network: nothing
//! is solved on the UI thread. A report without a baseline cannot back an Apply, and says so
//! in the verdict, so the button state and the refusal text agree.

use indicatrix_cut_core::{Design, Edit};
use indicatrix_editor::retarget::{
    check::RetargetCheck,
    search::SearchReport,
    validity::{InvalidReason, ValidityStatus},
};

/// The sentence shown when an option has no original network to be validated against.
pub(super) const MISSING_BASELINE: &str =
    "Original meet validation is missing; recompute this candidate.";

/// The check slot's content for option `index` of `report`: the option's design, masts, metrics
/// and verdict, with the report's own baseline (shared, not captured again). `None` when
/// `index` is out of range.
///
/// A report that carries no baseline gives the option an `Invalid` verdict whose first reason
/// is [`MISSING_BASELINE`], so Apply is blocked with that same sentence.
pub(super) fn candidate_check(report: &SearchReport, index: usize) -> Option<RetargetCheck> {
    let candidate = report.candidates.get(index)?;
    let mut validity = candidate.validity.clone();
    if report.baseline.is_none() {
        validity.status = ValidityStatus::Invalid;
        validity
            .reasons
            .insert(0, InvalidReason::Preservation(MISSING_BASELINE.to_string()));
    }
    Some(RetargetCheck {
        candidate: Some(candidate.design.clone()),
        baseline: report.baseline.clone(),
        validity,
        anchors: candidate.anchors.clone(),
        metrics: candidate.metrics,
    })
}

/// Every option's edit against `design`, validated against the report's baseline, in the
/// report's order; `None` for an option Apply would refuse. Each is a full solve, so this
/// belongs on the search's worker thread.
pub(super) fn prevalidated_edits(report: &SearchReport, design: &Design) -> Vec<Option<Edit>> {
    (0..report.candidates.len())
        .map(|index| {
            let check = candidate_check(report, index)?;
            check.allows_apply().then_some(())?;
            check.validated_edit(design).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::super::check_run::{self, ApplyGate};
    use super::*;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{
        ConstraintTier, Design, PreformSpec, ScheduleMeta,
        meet_preservation::MeetPreservationBaseline,
    };
    use indicatrix_editor::retarget::{
        metrics::RetargetMetrics,
        search::{CandidateKind, CandidateNumbers, SearchCandidate, StartPoint},
        validity::RetargetValidity,
    };
    use std::sync::Arc;

    /// A closed bipyramid: two `ScaleReference` tiers at +/-45 degrees on a block preform.
    fn design() -> Design {
        let tiers = [45.0, -45.0]
            .into_iter()
            .enumerate()
            .map(|(index, angle_deg)| ConstraintTier {
                angle_deg,
                name: format!("Tier {index}"),
                indices: vec![0.0, 24.0, 48.0, 72.0],
                constraint: MeetConstraint::ScaleReference(0.5),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            })
            .collect();
        let meta = ScheduleMeta {
            symmetry_order: 4,
            ..ScheduleMeta::standard_round_brilliant()
        };
        let mut design = Design::new(PreformSpec::block(4.0, 4.0, 4.0), meta, tiers);
        design.ensure_tier_ids();
        design
    }

    fn numbers() -> CandidateNumbers {
        CandidateNumbers {
            score: 8.0,
            windowing_pct: 4.5,
            brilliance_pct: 81.0,
            extinction_pct: 3.0,
            yield_loss_pct: 40.5,
        }
    }

    fn valid() -> RetargetValidity {
        let mut validity = RetargetValidity::unchecked(&InvalidReason::NotClosed);
        validity.status = ValidityStatus::Valid;
        validity.reasons.clear();
        validity
    }

    fn report(baseline: Option<Arc<MeetPreservationBaseline>>) -> SearchReport {
        SearchReport {
            start: StartPoint::Shift,
            shift_validity: valid(),
            start_numbers: numbers(),
            candidates: vec![SearchCandidate {
                preservation: None,
                kind: CandidateKind::Optimized,
                role: None,
                angles: Vec::new(),
                anchors: Vec::new(),
                validity: valid(),
                metrics: RetargetMetrics::default(),
                numbers: numbers(),
                design: design(),
            }],
            dropped: Vec::new(),
            evaluations: 1,
            free_tiers: 1,
            keep_look: false,
            baseline,
        }
    }

    #[test]
    fn a_report_without_a_baseline_blocks_apply_with_the_same_sentence() {
        let check = candidate_check(&report(None), 0).expect("option 0 exists");
        assert!(check.baseline.is_none());
        assert!(!check.allows_apply());
        let headline = check.validity.headline();
        assert!(headline.contains(MISSING_BASELINE), "{headline}");
        // Even if it got past the verdict, the edit itself would be refused without a baseline.
        let refusal = check
            .validated_edit(&design())
            .expect_err("no baseline, no edit");
        assert!(refusal.contains("No original meet validation"), "{refusal}");
        // Installed as the slot Apply reads, the gate is closed with the verdict's own words.
        let generation = 7;
        check_run::set_check_for_test(generation, Some(check));
        match check_run::apply_gate(generation) {
            ApplyGate::Blocked(why) => {
                assert!(why.contains(MISSING_BASELINE), "{why}");
            }
            _ => panic!("a pick without a baseline must block Apply"),
        }
    }

    #[test]
    fn a_report_with_a_baseline_shares_it_and_keeps_the_verdict() {
        let original = design();
        let baseline = MeetPreservationBaseline::capture(&original).expect("closed baseline");
        let baseline = Arc::new(baseline);
        let check = candidate_check(&report(Some(Arc::clone(&baseline))), 0).expect("option 0");
        assert!(Arc::ptr_eq(
            check.baseline.as_ref().expect("shared"),
            &baseline
        ));
        assert!(check.allows_apply());
    }

    #[test]
    fn an_index_past_the_list_is_none() {
        assert!(candidate_check(&report(None), 1).is_none());
    }
}
