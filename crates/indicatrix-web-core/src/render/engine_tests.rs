use indicatrix::renderer::{
    engine::{EngineSpec, RayOptions, RenderEngine},
    whitted::whitted_pixels_interleaved_geom,
};

use crate::{
    gpu_admission::self_check_scene,
    protocol::{FromWorker, PROTOCOL_VERSION, ToWorker, WorkerRole},
    scene::OwnedScene,
    settings::{ExportSettings, RenderSettings, SessionPayload},
    worker::WorkerHandler,
};

fn ray(taps: u8) -> EngineSpec {
    EngineSpec {
        engine: RenderEngine::RayTraced,
        ray: RayOptions {
            aa_taps: taps,
            fire: true,
        },
    }
}

#[test]
fn numeric_engine_settings_survive_downgrades_and_targets_bypass_path_minima() {
    let restored =
        SessionPayload::from_json(r#"{"render":{"engine":817,"ray_taps":3,"ray_fire":false}}"#)
            .unwrap();
    let settings = restored.render.sanitized();
    assert!(!settings.engine_spec().is_ray());
    assert_eq!(settings.ray_taps, 4);
    let settings = RenderSettings {
        engine: 1,
        ray_taps: 1,
        export: ExportSettings {
            ray_taps: 1,
            ..ExportSettings::default()
        },
        ..RenderSettings::default()
    };
    assert_eq!(settings.live_target(), 1);
    assert_eq!(settings.export_target(), 1);
    let payload = SessionPayload {
        render: settings,
        ..SessionPayload::default()
    };
    assert_eq!(
        SessionPayload::from_json(&payload.to_json().unwrap()),
        Ok(payload)
    );
}

#[test]
fn live_and_export_taps_have_independent_defaults_and_round_trip() {
    let mut settings = SessionPayload::from_json(r#"{"render":{"engine":1}}"#)
        .unwrap()
        .render;
    assert_eq!(settings.live_target(), 4);
    assert_eq!(settings.export_target(), 16);
    settings.export.ray_taps = 7;
    settings = settings.sanitized();
    assert_eq!(settings.live_target(), 4);
    assert_eq!(settings.export_target(), 8);
    settings.ray_taps = 1;
    assert_eq!(settings.export_target(), 8);
    let payload = SessionPayload {
        render: settings,
        ..SessionPayload::default()
    };
    assert_eq!(
        SessionPayload::from_json(&payload.to_json().unwrap()),
        Ok(payload)
    );
}

#[test]
fn ray_chunks_equal_the_core_for_absolute_taps_and_partitions() {
    let mut scene = OwnedScene::build(&self_check_scene(), None).unwrap();
    scene.set_engine(ray(8)).unwrap();
    let prep = scene.whitted_prep().unwrap();
    assert!(
        std::ptr::eq(prep, scene.whitted_prep().unwrap()),
        "prep is cached"
    );
    for stride in [1, 3, 7] {
        for first in 0..stride {
            let expected = whitted_pixels_interleaved_geom(
                &scene.frame_scene(),
                scene.tools(),
                scene.plane_soa(),
                prep,
                first,
                stride,
                2,
                3,
            );
            let actual = super::handle_trace_chunk(&scene, scene.plane_soa(), first, stride, 2, 3);
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn worker_engine_messages_are_scoped_to_their_scene() {
    let mut worker = WorkerHandler::new();
    worker.handle(
        ToWorker::Init {
            protocol_version: PROTOCOL_VERSION,
            role: WorkerRole::Render,
            worker_index: 0,
        },
        &|| 0.0,
    );
    worker.handle(
        ToWorker::SetScene {
            scene_id: 5,
            spec: self_check_scene(),
        },
        &|| 0.0,
    );
    assert!(
        worker
            .handle(
                ToWorker::SetEngine {
                    scene_id: 5,
                    engine: ray(4)
                },
                &|| 0.0
            )
            .is_none()
    );
    assert!(matches!(
        worker.handle(
            ToWorker::SetEngine {
                scene_id: 4,
                engine: ray(16)
            },
            &|| 0.0
        ),
        Some(FromWorker::SceneError { scene_id: 4, .. })
    ));
    let mut expected = OwnedScene::build(&self_check_scene(), None).unwrap();
    expected.set_engine(ray(4)).unwrap();
    let sums = super::handle_trace_chunk(&expected, expected.plane_soa(), 0, 1, 0, 1);
    let result = worker.handle(
        ToWorker::TraceChunk {
            scene_id: 5,
            first_pixel: 0,
            stride: 1,
            sample_offset: 0,
            spp: 1,
        },
        &|| 0.0,
    );
    let Some(FromWorker::ChunkResult { sums: actual, .. }) = result else {
        panic!("missing chunk");
    };
    assert_eq!(
        actual,
        sums.iter().map(glam::Vec3::to_array).collect::<Vec<_>>()
    );
}

#[test]
fn ray_uv_decline_drops_the_worker_scene_instead_of_counting_blank_taps() {
    let mut worker = WorkerHandler::new();
    worker.handle(
        ToWorker::Init {
            protocol_version: PROTOCOL_VERSION,
            role: WorkerRole::Render,
            worker_index: 0,
        },
        &|| 0.0,
    );
    let mut spec = self_check_scene();
    spec.lighting.preset_index = indicatrix::optics::raytracer::LightingPreset::UvLamp365.index();
    worker.handle(ToWorker::SetScene { scene_id: 5, spec }, &|| 0.0);
    assert!(matches!(
        worker.handle(
            ToWorker::SetEngine {
                scene_id: 5,
                engine: ray(4)
            },
            &|| 0.0
        ),
        Some(FromWorker::SceneError { .. })
    ));
    assert!(matches!(
        worker.handle(
            ToWorker::TraceChunk {
                scene_id: 5,
                first_pixel: 0,
                stride: 1,
                sample_offset: 0,
                spp: 1
            },
            &|| 0.0
        ),
        Some(FromWorker::ChunkDropped { .. })
    ));
}

#[test]
fn gpu_passes_stop_at_each_ray_tap_target() {
    for taps in [1, 4, 8, 16] {
        let engine = ray(taps);
        let target = engine.target_samples(4096);
        let mut planner = super::ChunkPlanner::new(3, 256, target);
        let mut next = 0;
        while let Some((_, offset, spp)) = planner.next_gpu_pass() {
            assert_eq!(offset, next);
            assert!(spp > 0 && offset + spp <= target);
            next += spp;
        }
        assert_eq!(next, u32::from(taps));
    }
}
