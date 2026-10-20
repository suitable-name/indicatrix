//! Actual tool geometry reaches the tracer, rather than only a capability flag.
use super::{RenderView, scene};
use crate::{
    app_state::AppState,
    design_state::{begin_solve, install, run_solve},
    platform_mock::MockPlatform,
};
use indicatrix_cut_core::{
    design::{ConcaveTier, ConcaveTool, ToolMotion},
    templates::TEMPLATES,
};
use indicatrix_editor::EditorSession;
use indicatrix_web_core::{render::handle_trace_chunk, scene::GeometrySceneSpec};
use std::sync::{Arc, atomic::AtomicBool};

#[test]
fn concave_scene_traces_the_resolved_tools_and_tool_changes_change_identity() {
    let mut state = AppState::new(Arc::new(MockPlatform::default()));
    let template = &TEMPLATES[0];
    state.session = EditorSession::from_template(
        template.fresh_spec(template.default_material_selection()),
        1,
    );
    state.session.design.concave_tiers.push(ConcaveTier {
        name: "Dimple".into(),
        // A crown-side facet angle; the rule is 0 < |angle| < 90, so 0 cannot render.
        angle_deg: 40.0,
        indices: vec![0.0],
        instructions: String::new(),
        tool: ConcaveTool::Sphere,
        tool_azimuth_deg: 0.0,
        displacement: [0.0; 3],
        diameter_ratio: 0.8,
        tool_angle_deg: None,
        motion: ToolMotion::Plunge,
    });
    let job = begin_solve(&mut state).unwrap();
    let answer = run_solve(&job, &AtomicBool::new(false));
    install(
        &mut state,
        job.generation,
        answer,
        std::time::Duration::ZERO,
    );
    let view = RenderView {
        width: 16,
        height: 12,
    };
    let prepared = scene::prepare(&state, view).unwrap();
    assert_ne!(
        prepared.key.tools,
        [] as [indicatrix::geometry::tool::ToolPrimitive; 0]
    );
    // A catalogue material has no physics recipe, so there is nothing to fluoresce from.
    assert_eq!(
        prepared.key.customs,
        [] as [indicatrix_web_core::scene::CustomMaterialSpec; 0]
    );
    let geometry = GeometrySceneSpec {
        scene: prepared.key.spec.clone(),
        tools: prepared.key.tools.clone(),
    };
    let built = geometry.build(prepared.hdr).unwrap();
    let expected = indicatrix::renderer::cpu_frame::trace_pixels_interleaved_geom(
        &built.frame_scene(),
        &geometry.tools,
        built.fluorescence(),
        built.plane_soa(),
        0,
        1,
        0,
        4,
    );
    assert_eq!(
        handle_trace_chunk(&built, built.plane_soa(), 0, 1, 0, 4),
        expected
    );
    let without_tools = indicatrix::renderer::cpu_frame::trace_pixels_interleaved_geom(
        &built.frame_scene(),
        &[],
        built.fluorescence(),
        built.plane_soa(),
        0,
        1,
        0,
        4,
    );
    assert_ne!(
        expected, without_tools,
        "a concave render must differ from the uncut hull"
    );
    state.session.design.concave_tiers[0].diameter_ratio = 0.6;
    let changed = scene::prepare(&state, view).unwrap();
    assert_ne!(changed.key, prepared.key);
}
