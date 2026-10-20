use super::*;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use indicatrix_net::{
    SceneState,
    messages::{BatchReply, ClientMessage},
};

fn scene() -> SceneState {
    SceneState {
        engine: indicatrix::renderer::engine::EngineSpec::default(),
        width: 4,
        height: 4,
        yaw: 0.4,
        pitch: 0.3,
        distance: 3.0,
        light_yaw: 0.85,
        light_pitch: 0.95,
        exposure: 1.0,
        max_bounces: 4,
        lighting_preset: LightingPreset::Daylight,
        material: GemMaterial::diamond(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: 0.0,
        environment: indicatrix_net::scene::SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: indicatrix::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: 16.0,
        frosted_planes: Vec::new(),
    }
}

fn item(item_id: u32) -> BatchItem {
    BatchItem {
        item_id,
        scene: scene(),
        first_sample: 0,
        samples: 2,
        width: 4,
        height: 4,
    }
}

fn request(request_id: u32, ids: &[u32]) -> BatchRenderRequest {
    BatchRenderRequest {
        request_id,
        reply: BatchReply::FinalPng,
        items: ids.iter().map(|&id| item(id)).collect(),
    }
}

#[test]
fn a_well_formed_batch_is_admitted() {
    assert!(check_admission(&request(1, &[0, 1, 2]), &[]).is_ok());
    assert!(check_admission(&request(2, &[0]), &[1]).is_ok());
}

#[test]
fn a_third_batch_in_flight_is_refused() {
    let err = check_admission(&request(3, &[0]), &[1, 2]).unwrap_err();
    assert!(err.contains("in flight"), "{err}");
}

#[test]
fn a_duplicate_batch_id_empty_oversized_or_duplicate_items_are_refused() {
    assert!(check_admission(&request(1, &[0]), &[1]).is_err());
    assert!(check_admission(&request(1, &[]), &[]).is_err());
    let too_many: Vec<u32> =
        (0..=u32::try_from(MAX_BATCH_ITEMS).expect("the batch cap is small")).collect();
    assert!(check_admission(&request(1, &too_many), &[]).is_err());
    let at_cap: Vec<u32> =
        (0..u32::try_from(MAX_BATCH_ITEMS).expect("the batch cap is small")).collect();
    assert!(check_admission(&request(1, &at_cap), &[]).is_ok());
    assert!(check_admission(&request(1, &[4, 5, 4]), &[]).is_err());
}

#[test]
fn an_item_with_a_size_mismatch_zero_samples_or_an_hdr_scene_fails_alone() {
    assert!(check_item(&item(0)).is_ok());

    let mut wrong_size = item(0);
    wrong_size.width = 5;
    assert!(check_item(&wrong_size).unwrap_err().contains("must equal"));

    let mut no_samples = item(0);
    no_samples.samples = 0;
    assert!(check_item(&no_samples).is_err());

    let mut hdr = item(0);
    hdr.scene.environment =
        indicatrix_net::scene::SceneEnvironment::Hdr(indicatrix_net::scene::HdrEnvironment {
            content_hash: [1; 32],
            width: 64,
            height: 32,
        });
    assert!(check_item(&hdr).unwrap_err().contains("HDR"));
}

#[test]
fn a_picture_encodes_to_the_shared_preview_png() {
    let accum = vec![Vec3::new(0.4, 0.3, 0.2) * 4.0; 16];
    let (event, payload) = encode_picture(Picture {
        request_id: 5,
        item_id: 9,
        samples: 4,
        width: 4,
        height: 4,
        accum: accum.clone(),
    });
    let StreamEvent::BatchItemDone(header) = event else {
        panic!("expected BatchItemDone");
    };
    let bytes = payload.expect("a finished picture carries its PNG");
    assert_eq!(header.payload_len as usize, bytes.len());
    assert_eq!(
        (header.request_id, header.item_id, header.samples_done),
        (5, 9, 4)
    );
    assert_eq!(
        Some(bytes),
        indicatrix::render_setup::encode_preview_png(4, 4, &accum, 4)
    );
}

#[test]
fn a_picture_of_the_wrong_length_becomes_an_item_failure() {
    let (event, payload) = encode_picture(Picture {
        request_id: 5,
        item_id: 9,
        samples: 4,
        width: 4,
        height: 4,
        accum: vec![Vec3::ZERO; 3],
    });
    assert!(matches!(event, StreamEvent::BatchItemFailed(f) if f.item_id == 9));
    assert!(payload.is_none());
}

#[test]
fn the_batch_message_is_the_one_the_request_loop_dispatches() {
    let message = ClientMessage::BatchRenderRequest(Box::new(request(1, &[0])));
    assert!(matches!(message, ClientMessage::BatchRenderRequest(_)));
}
