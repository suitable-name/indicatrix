//! A failed preservation projection must not block the flat-facet repair that it needs.
use super::*;
use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};

fn swallowed_culet() -> (Design, StoneAnalysis, Attempt, usize) {
    let live = Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    let original = analyze(&live, true).expect("original geometry and meet network");
    let culet = live
        .tiers
        .iter()
        .position(|tier| tier.name == "Culet")
        .unwrap();
    assert!(original.flats.iter().any(|flat| flat.tier_index == culet));
    let mut design = live.clone();
    // The same intermediate state as a pavilion apex moving above the old culet:
    // the stone still closes, but this flat no longer reaches its boundary.
    design.tiers[culet].constraint = MeetConstraint::ScaleReference(1.2);
    let raw = analyze(&design, false).expect("proposal is closed despite its missing culet");
    assert!(raw.flats.iter().all(|flat| flat.tier_index != culet));
    let violation = original
        .preservation
        .as_ref()
        .unwrap()
        .validate(
            &design,
            indicatrix_cut_core::meet_preservation::PreservationPolicy::RETARGET,
        )
        .expect_err("a vanished original flat must be refused");
    let reason = InvalidReason::Preservation(violation.to_string());
    let base = Attempt {
        anchors: super::super::anchors::mast_differences(&live, &design),
        requested: design.clone(),
        design,
        strategy: RetargetStrategy::Anchored,
        analysis: Err(reason.clone()),
        reasons: vec![reason],
        girdle_step: None,
    };
    (live, original, base, culet)
}

#[test]
fn rejected_projection_can_refit_a_swallowed_culet() {
    let (live, original, base, culet) = swallowed_culet();
    let repaired = refit_attempt(&live, &original, &base, &[], &|| false)
        .expect("the failed first projection must not suppress the refit");
    assert!(repaired.reasons.is_empty(), "{:?}", repaired.reasons);
    assert!(
        repaired
            .analysis
            .as_ref()
            .unwrap()
            .flats
            .iter()
            .any(|flat| flat.tier_index == culet)
    );
    assert_eq!(
        repaired.design.tiers[culet].angle_deg.to_bits(),
        live.tiers[culet].angle_deg.to_bits()
    );
    original
        .preservation
        .as_ref()
        .unwrap()
        .validate(
            &repaired.design,
            indicatrix_cut_core::meet_preservation::PreservationPolicy::RETARGET,
        )
        .expect("repair must preserve the original network");
}

#[test]
fn geometry_only_refit_cannot_bypass_the_authoring_gate() {
    let (live, original, mut base, _) = swallowed_culet();
    base.design.tiers[0].name = "Unrelated authoring change".into();
    let repaired = refit_attempt(&live, &original, &base, &[], &|| false)
        .expect("geometry-only analysis still produces a refit proposal");
    assert!(repaired.analysis.is_err());
    assert!(
        !repaired.reasons.is_empty(),
        "the refit is not permission to apply"
    );
}
