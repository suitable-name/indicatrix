//! Branch diagnosis (follow-up plan 2026-10-19, section 8.1). Written without a toolchain:
//! the owner runs them.

use super::*;
use crate::meet_preservation::BranchDiagnosis;
use indicatrix::geometry::meet_solver::SolveControl;

fn diagnose(baseline: &MeetPreservationBaseline, candidate: &Design) -> BranchDiagnosis {
    baseline.diagnose_refusal(candidate, &SolveControl::default())
}

/// The octahedron whose last crown facet (`C3`) is derived from the others' meet instead of
/// pinned: the solver picks a vertex level for it, not the apex the original passes through.
fn with_derived_crown_facet(design: &Design) -> Design {
    let mut derived = design.clone();
    derived.tiers[3].constraint = MeetConstraint::MeetExisting;
    derived
}

#[test]
fn a_split_apex_with_nothing_derived_is_infeasible() {
    let design = octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).expect("original network");
    let mut damaged = design;
    damaged.tiers[0].angle_deg += 1.0;
    assert!(
        matches!(
            diagnose(&baseline, &damaged),
            BranchDiagnosis::Infeasible { .. }
        ),
        "a split four-plane apex cannot be repaired by any mast"
    );
}

/// Not runnable on the octahedron: the solver's rank-1 level for the derived `C3` is the
/// original apex (levels 1.5 then 0.5; `pipeline.rs` picks index 1), so the premise below
/// fails and the diagnosis is `Unresolved`. A mismatch needs a fixture whose derived tier has
/// a shallower feasible vertex level than its original mast, and whose junctions still hold
/// three fixed planes each (pinning `C3` off the apex leaves only two, so the ideal mast
/// cannot be formed). The real-stone mismatch is measured by the census instead.
#[test]
#[ignore = "needs a fixture where the meet solver picks a different vertex; see FABLE_V0 F1"]
fn a_derived_tier_that_picked_another_vertex_is_a_branch_mismatch() {
    let design = octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).expect("original network");
    let candidate = with_derived_crown_facet(&design);
    let solved = candidate.solve().expect("the derived design solves");
    assert!(
        (solved[3].mast - 0.5).abs() > 1e-6,
        "premise: the solver's own pick for the derived facet is not the original apex"
    );
    match diagnose(&baseline, &candidate) {
        BranchDiagnosis::BranchMismatch {
            tiers,
            ideal_spread,
        } => {
            assert_eq!(tiers, vec![3]);
            assert!(ideal_spread < 1e-5, "{ideal_spread}");
        }
        other => panic!("expected a branch mismatch, got {other:?}"),
    }
}

#[test]
fn a_candidate_with_other_surfaces_is_unresolved() {
    let design = octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).expect("original network");
    let mut other = design;
    other.tiers[0].indices = vec![0.0, 24.0];
    assert!(matches!(
        diagnose(&baseline, &other),
        BranchDiagnosis::Unresolved { .. }
    ));
}

#[test]
fn a_candidate_the_gate_would_accept_has_nothing_to_diagnose() {
    let design = octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).expect("original network");
    assert!(matches!(
        diagnose(&baseline, &design),
        BranchDiagnosis::Unresolved { .. }
    ));
}

#[test]
fn a_cancelled_diagnosis_is_unresolved() {
    let design = octahedron();
    let baseline = MeetPreservationBaseline::capture(&design).expect("original network");
    let flag = std::sync::atomic::AtomicBool::new(true);
    let control = SolveControl::with_cancel(&flag);
    assert!(matches!(
        baseline.diagnose_refusal(&design, &control),
        BranchDiagnosis::Unresolved { .. }
    ));
}
