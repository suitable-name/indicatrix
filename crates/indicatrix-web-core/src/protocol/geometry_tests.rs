//! `ToWorker::SetGeometryScene`: a concave stone's scene on the wire.

use glam::Vec3;
use indicatrix::{
    geometry::{cuts::StandardGemCuts, tool::ToolPrimitive},
    optics::raytracer::LightingPreset,
    render_setup::Backdrop,
};

use super::*;
use crate::scene::{CameraSpec, FinishSpec, LightingSpec, MaterialSpec, planes_to_data};

fn spec() -> SceneSpec {
    SceneSpec {
        planes: planes_to_data(&StandardGemCuts::standard_round_brilliant()),
        finishes: FinishSpec::AllPolished,
        material: MaterialSpec::catalogue("Diamond"),
        camera: CameraSpec {
            yaw: 0.6,
            pitch: 0.35,
            distance: 4.2,
        },
        lighting: LightingSpec::new(
            LightingPreset::LightTent,
            1.0,
            0.4,
            0.35,
            Backdrop::Grey,
            16.0,
        ),
        max_bounces: 4,
        width: 6,
        height: 4,
        hdr_id: None,
    }
}

fn tools() -> Vec<ToolPrimitive> {
    vec![
        ToolPrimitive::ball(Vec3::new(0.2, 0.32, 0.1), 0.25),
        ToolPrimitive::cylinder(
            Vec3::new(-0.3, 0.2, -0.2),
            Vec3::new(1.0, 0.0, 0.2).normalize(),
            0.08,
            0.5,
        ),
    ]
}

/// The tools survive postcard bit for bit and retain index 11, after `TraceFullPass` = 10.
#[test]
fn a_geometry_scene_round_trips_and_keeps_its_variant_index() {
    let message = ToWorker::SetGeometryScene {
        scene_id: 9,
        spec: spec(),
        tools: tools(),
    };
    let bytes = encode_to_worker(&message).expect("encodes");
    assert_eq!(bytes[0], 11, "appended after TraceFullPass (variant 10)");
    assert_eq!(decode_to_worker(&bytes).expect("decodes"), message);
}

/// Engine selection is appended beside the unchanged geometry message.
#[test]
fn engine_is_variant_twelve_with_explicit_renderer_taps_and_fire() {
    use indicatrix::renderer::engine::{EngineSpec, RayOptions, RenderEngine};
    let message = ToWorker::SetEngine {
        scene_id: 9,
        engine: EngineSpec {
            engine: RenderEngine::RayTraced,
            ray: RayOptions {
                aa_taps: 8,
                fire: true,
            },
        },
    };
    let bytes = encode_to_worker(&message).expect("encodes");
    assert_eq!(bytes, [12, 9, 1, 8, 1]);
    assert_eq!(decode_to_worker(&bytes).expect("decodes"), message);
}

/// A planar scene still goes out as `SetScene`, byte for byte what it was before concave
/// stones reached the browser's Render tab; only a stone with tools uses the new message.
#[test]
fn a_planar_scene_keeps_the_set_scene_bytes() {
    let planar = ToWorker::scene(9, spec(), &[]);
    assert_eq!(
        planar,
        ToWorker::SetScene {
            scene_id: 9,
            spec: spec()
        }
    );
    let old = encode_to_worker(&ToWorker::SetScene {
        scene_id: 9,
        spec: spec(),
    })
    .expect("encodes");
    assert_eq!(encode_to_worker(&planar).expect("encodes"), old);
    assert_eq!(old[0], 1, "SetScene is variant 1");

    let concave = ToWorker::scene(9, spec(), &tools());
    assert!(
        matches!(&concave, ToWorker::SetGeometryScene { tools: t, .. } if *t == tools()),
        "{concave:?}"
    );
}

/// `SolveRequest::{MetricsWithTools, TiltWithTools}`: the concave stone's metrics and tilt
/// jobs. They are the last two `SolveRequest` variants (6 and 7, after `BodyColor` = 5), the
/// tools survive the wire, and a stone without tools still sends the planar variants with
/// the bytes they always had.
#[test]
fn the_solve_requests_with_tools_round_trip_and_are_the_last_variants() {
    use crate::solve::{MetricsParams, TiltParams};

    let metrics = MetricsParams::from_scene(&spec());
    let tilt = TiltParams::from_scene(&spec());

    for (request, index) in [
        (SolveRequest::metrics(metrics.clone(), &tools()), 6),
        (SolveRequest::tilt(tilt.clone(), &tools()), 7),
    ] {
        let bytes = postcard::to_allocvec(&request).expect("encodes");
        assert_eq!(bytes[0], index, "{request:?}");
        let message = ToWorker::Solve {
            job_id: 5,
            design_toml: String::new(),
            request,
        };
        let wire = encode_to_worker(&message).expect("encodes");
        assert_eq!(decode_to_worker(&wire).expect("decodes"), message);
    }

    for (planar, old, index) in [
        (
            SolveRequest::metrics(metrics.clone(), &[]),
            SolveRequest::Metrics { params: metrics },
            3,
        ),
        (
            SolveRequest::tilt(tilt.clone(), &[]),
            SolveRequest::Tilt { params: tilt },
            4,
        ),
    ] {
        let bytes = postcard::to_allocvec(&planar).expect("encodes");
        assert_eq!(bytes, postcard::to_allocvec(&old).expect("encodes"));
        assert_eq!(bytes[0], index);
    }
}
