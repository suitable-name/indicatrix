//! Optimize and Retarget accept designs with concave tiers.
//!
//! Both used to decline them, because their scores measured the flat shell. The engine
//! entry points they call now score every candidate on the stone with its tools
//! (`ScoredStone` and the `*_geom_under` objectives in cut-core, `measure_analysis_column`
//! in the editor), so the phone has no reason to refuse: these tests pin that a concave
//! design is captured exactly when the same design without the tool tier is, and that the
//! tool tier is part of the operation's identity.

use crate::{
    app_state::AppState,
    optimization::{self, OptimizationJob},
    platform_mock::MockPlatform,
    retarget::{self, RetargetControls, RetargetJob},
};
use indicatrix_cut_core::{
    CANONICAL_LIGHTING_PRESET,
    design::{ConcaveTier, ConcaveTool, ToolMotion},
    templates::TEMPLATES,
};
use indicatrix_editor::{
    EditorSession,
    optimize_view::{RunForm, RunPlan, build_run_plan},
    retarget::CrownShift,
};
use std::sync::Arc;

fn dimple(diameter_ratio: f64) -> ConcaveTier {
    ConcaveTier {
        name: "Dimple".into(),
        // A tier the form would accept: the rule is 0 < |angle| < 90.
        angle_deg: 40.0,
        indices: vec![0.0],
        instructions: String::new(),
        tool: ConcaveTool::Sphere,
        tool_azimuth_deg: 0.0,
        displacement: [0.0; 3],
        diameter_ratio,
        tool_angle_deg: None,
        motion: ToolMotion::Plunge,
    }
}

/// A state on the first template, with a dimple tier when `concave`.
fn state(concave: bool) -> AppState {
    let mut state = AppState::new(Arc::new(MockPlatform::default()));
    let template = &TEMPLATES[0];
    state.session = EditorSession::from_template(
        template.fresh_spec(template.default_material_selection()),
        1,
    );
    if concave {
        state.session.design.concave_tiers.push(dimple(0.8));
    }
    state
}

fn plan(state: &AppState) -> RunPlan {
    build_run_plan(
        &state.session.design,
        &RunForm {
            preset_index: 0,
            weight_windowing: "",
            weight_extinction: "",
            weight_tilt_brilliance: "",
            yield_weight: 0.0,
            tone: 0.0,
            vary_anchored: false,
            keep_girdle: true,
            budget_text: "200",
            starts: 1,
            seed_text: "1",
            polish: false,
            candidates: 1,
            ranges: &[],
        },
        CANONICAL_LIGHTING_PRESET,
    )
    .expect("the default form is valid")
}

fn controls() -> RetargetControls {
    RetargetControls {
        target: TEMPLATES[0].default_material_selection(),
        crown: CrownShift::default(),
        girdle: None,
        search: None,
    }
}

#[test]
fn optimize_captures_a_concave_design_exactly_when_the_flat_one_is_captured() {
    let mut flat = state(false);
    let mut concave = state(true);
    let flat_plan = plan(&flat);
    let concave_plan = plan(&concave);
    let flat_result = OptimizationJob::capture(&mut flat, flat_plan, None).map(|_| ());
    let concave_result = OptimizationJob::capture(&mut concave, concave_plan, None).map(|_| ());
    assert_eq!(
        concave_result.is_ok(),
        flat_result.is_ok(),
        "flat: {flat_result:?}, concave: {concave_result:?}"
    );
    if let Err(message) = &concave_result {
        assert!(!message.contains("concave"), "{message}");
    }
}

#[test]
fn retarget_captures_a_concave_design_exactly_when_the_flat_one_is_captured() {
    let mut flat = state(false);
    let mut concave = state(true);
    let flat_result = RetargetJob::capture(&mut flat, controls()).map(|_| ());
    let concave_result = RetargetJob::capture(&mut concave, controls()).map(|_| ());
    assert_eq!(
        concave_result.is_ok(),
        flat_result.is_ok(),
        "flat: {flat_result:?}, concave: {concave_result:?}"
    );
    if let Err(message) = &concave_result {
        assert!(!message.contains("concave"), "{message}");
    }
}

#[test]
fn the_tool_tier_is_part_of_both_operations_identity() {
    let flat = state(false);
    let mut concave = state(true);
    let plan_flat = plan(&flat);
    let before = optimization::fingerprint(&concave, &plan_flat, None).unwrap();
    assert_ne!(
        before,
        optimization::fingerprint(&flat, &plan_flat, None).unwrap(),
        "a design with a tool tier is another design for Optimize"
    );
    let retarget_before = retarget::fingerprint(&concave, &controls()).unwrap();
    assert_ne!(
        retarget_before,
        retarget::fingerprint(&flat, &controls()).unwrap()
    );
    // Resizing the tool changes the stone that is scored, so a result computed before is stale.
    concave.session.design.concave_tiers[0].diameter_ratio = 0.5;
    assert_ne!(
        before,
        optimization::fingerprint(&concave, &plan_flat, None).unwrap()
    );
    assert_ne!(
        retarget_before,
        retarget::fingerprint(&concave, &controls()).unwrap()
    );
}
