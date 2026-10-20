//! A render Worker's concave scenes: `ToWorker::SetGeometryScene` caches the stone with its
//! tools, traces and denoises it like the desktop does, and refuses a bad tool list whole.

use glam::Vec3;
use indicatrix::geometry::tool::{MAX_TOOL_PRIMITIVES, ToolPrimitive};

use super::{
    tests::{init, spec, ticking_clock},
    *,
};
use crate::{render::handle_trace_chunk, scene::GeometrySceneSpec};

/// A frame big enough for a dimple in the table to change pixels at one sample.
fn frame() -> SceneSpec {
    SceneSpec {
        width: 24,
        height: 24,
        ..spec(None)
    }
}

fn dimple() -> ToolPrimitive {
    ToolPrimitive::ball(Vec3::new(0.0, 0.32, 0.0), 0.45)
}

fn whole_frame_chunk(scene_id: u64) -> ToWorker {
    ToWorker::TraceChunk {
        scene_id,
        first_pixel: 0,
        stride: 1,
        sample_offset: 0,
        spp: 2,
    }
}

fn sums_of(reply: Option<FromWorker>) -> Vec<[f32; 3]> {
    match reply {
        Some(FromWorker::ChunkResult { sums, .. }) => sums,
        other => panic!("expected a chunk result, got {other:?}"),
    }
}

/// The Worker traces exactly what `GeometrySceneSpec::build` + `handle_trace_chunk` trace, and
/// that is not the flat stone.
#[test]
fn a_geometry_scene_is_traced_with_its_tools() {
    let mut handler = init(WorkerRole::Render);
    let clock = ticking_clock();
    let tools = vec![dimple()];

    assert_eq!(
        handler.handle(
            ToWorker::SetGeometryScene {
                scene_id: 1,
                spec: frame(),
                tools: tools.clone(),
            },
            &clock
        ),
        None
    );
    let cut = sums_of(handler.handle(whole_frame_chunk(1), &clock));

    let built = GeometrySceneSpec {
        scene: frame(),
        tools,
    }
    .build(None)
    .expect("builds");
    assert_eq!(built.tools().len(), 1);
    let direct: Vec<[f32; 3]> = handle_trace_chunk(&built, built.plane_soa(), 0, 1, 0, 2)
        .into_iter()
        .map(<[f32; 3]>::from)
        .collect();
    assert_eq!(
        cut, direct,
        "the Worker must trace the stone with its tools"
    );

    // The same frame as a planar scene is a different picture: the tools reached the tracer.
    assert_eq!(
        handler.handle(
            ToWorker::SetScene {
                scene_id: 2,
                spec: frame()
            },
            &clock
        ),
        None
    );
    let flat = sums_of(handler.handle(whole_frame_chunk(2), &clock));
    assert_ne!(cut, flat, "a dimple in the table must change the picture");
}

/// Replacing a concave scene by a planar one (`SetScene`) drops the tools: the Worker holds
/// one scene, never the old tools with the new planes.
#[test]
fn a_planar_scene_replaces_a_concave_one_completely() {
    let mut handler = init(WorkerRole::Render);
    let clock = ticking_clock();
    for message in [
        ToWorker::SetGeometryScene {
            scene_id: 1,
            spec: frame(),
            tools: vec![dimple()],
        },
        ToWorker::SetScene {
            scene_id: 2,
            spec: frame(),
        },
    ] {
        assert_eq!(handler.handle(message, &clock), None);
    }
    let after = sums_of(handler.handle(whole_frame_chunk(2), &clock));

    let mut fresh = init(WorkerRole::Render);
    assert_eq!(
        fresh.handle(
            ToWorker::SetScene {
                scene_id: 2,
                spec: frame()
            },
            &clock
        ),
        None
    );
    assert_eq!(after, sums_of(fresh.handle(whole_frame_chunk(2), &clock)));
    // And the concave scene's id is gone with it.
    assert!(matches!(
        handler.handle(whole_frame_chunk(1), &clock),
        Some(FromWorker::ChunkDropped { scene_id: 1, .. })
    ));
}

/// An invalid tool, or more than `MAX_TOOL_PRIMITIVES`, is a `SceneError` and leaves no scene
/// (not a flat one): a half-resolved stone must never be traced.
#[test]
fn a_bad_tool_list_is_refused_whole() {
    let mut handler = init(WorkerRole::Render);
    let clock = ticking_clock();
    let mut broken = dimple();
    broken.origin[3] = f32::NAN;
    let too_many = vec![dimple(); MAX_TOOL_PRIMITIVES + 1];
    for (scene_id, tools) in [(1, vec![broken]), (2, too_many)] {
        let reply = handler.handle(
            ToWorker::SetGeometryScene {
                scene_id,
                spec: frame(),
                tools,
            },
            &clock,
        );
        assert!(
            matches!(reply, Some(FromWorker::SceneError { scene_id: id, .. }) if id == scene_id),
            "{reply:?}"
        );
        assert!(matches!(
            handler.handle(whole_frame_chunk(scene_id), &clock),
            Some(FromWorker::ChunkDropped { .. })
        ));
    }
}

/// A geometry scene is a render Worker's message, like `SetScene`.
#[test]
fn only_a_render_worker_takes_a_geometry_scene() {
    let clock = ticking_clock();
    for role in [WorkerRole::Solve, WorkerRole::Gpu] {
        let mut handler = init(role);
        let reply = handler.handle(
            ToWorker::SetGeometryScene {
                scene_id: 1,
                spec: frame(),
                tools: vec![dimple()],
            },
            &clock,
        );
        assert!(matches!(reply, Some(FromWorker::Error { .. })), "{role:?}");
    }
}

/// The denoiser's guide pass sees the tools: a picture of the concave scene is asked for with
/// the tools in the frame (`DenoiseFrame::tools`), as on the desktop.
#[test]
fn a_geometry_scene_makes_pictures_with_its_tools_in_the_frame() {
    let mut handler = init(WorkerRole::Render);
    let clock = ticking_clock();
    assert_eq!(
        handler.handle(
            ToWorker::SetGeometryScene {
                scene_id: 3,
                spec: frame(),
                tools: vec![dimple()],
            },
            &clock
        ),
        None
    );
    let sums: Vec<[f32; 3]> = (0..24 * 24)
        .map(|i| [i as f32 * 0.01, 0.5, ((i % 7) as f32).mul_add(0.1, 1.0)])
        .collect();
    let reply = handler.handle(
        ToWorker::Picture {
            scene_id: 3,
            sample_count: 4,
            sums: sums.clone(),
            kind: PictureKind::DenoisedLive,
        },
        &clock,
    );
    let Some(FromWorker::Picture { bytes, .. }) = reply else {
        panic!("expected a picture, got {reply:?}");
    };

    let scene = GeometrySceneSpec {
        scene: frame(),
        tools: vec![dimple()],
    }
    .build(None)
    .expect("builds");
    let sum: Vec<Vec3> = sums.iter().copied().map(Vec3::from_array).collect();
    let frame = DenoiseFrame {
        guide_key: 3,
        width: 24,
        height: 24,
        camera: scene.camera(),
        planes: scene.planes(),
        tools: scene.tools(),
        n_d: scene.material().dispersion.n_d(),
        sample_count: 4,
        sum: &sum,
    };
    assert_eq!(bytes, Denoiser::new().denoised_rgba(&frame));
}
