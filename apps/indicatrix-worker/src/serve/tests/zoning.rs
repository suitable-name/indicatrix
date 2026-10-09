//! The zoning wire extension end to end through the real connection handler (`zoning`
//! builds): the capability marker, a zoned render and a zoned batch that equal the local CPU
//! render of the same scene, and the refusals for a payload that does not fit.

use super::fixtures::{DuplexHalf, heavier_scene, read_stream_until_done, test_db, tiny_scene};
use crate::{render_core, serve::handle_connection};
use indicatrix::{
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::{AbsorptionUnit, GemMaterial},
        zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
    },
    renderer::gpu_backend::GpuBackend,
};
use indicatrix_net::{
    SceneState,
    client::{Accumulator, send_render_request},
    framing, handshake,
    messages::{
        BatchItem, BatchRenderRequest, BatchReply, ClientMessage, RenderRequest, RequestIntent,
        StreamEvent, Welcome, ZONING_TAIL, ZoningPayload, error_codes, split_welcome_tail,
        write_hello_message, write_message, write_zoning_payload,
    },
};
use std::io::Cursor;

/// A bicolour stone: the base absorbs red, a half space (+X side) absorbs blue, both strong
/// enough that the picture differs plainly from the unzoned stone.
fn zoned_material() -> GemMaterial {
    let band = |center: f32| {
        ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
            center, 60.0, 1.0,
        )]))
    };
    let mut zoned = ZonedAbsorption::new(band(640.0));
    zoned.zones.push(Zone {
        shape: ZoneShape::HalfSpace {
            normal: glam::DVec3::X,
            offset: 0.0,
        },
        absorption: band(450.0),
    });
    GemMaterial::diamond().with_zoning(zoned)
}

fn zoned_scene(base: SceneState) -> SceneState {
    SceneState {
        material: zoned_material(),
        ..base
    }
}

/// `HELLO` (with the zoning marker when `zoning`), then `build` writes the rest.
fn run(zoning: bool, build: impl FnOnce(&mut Vec<u8>)) -> Cursor<Vec<u8>> {
    let mut input = Vec::new();
    if zoning {
        write_hello_message(&mut input, &handshake::local_hello_zoning()).unwrap();
    } else {
        write_message(&mut input, &handshake::local_hello()).unwrap();
    }
    build(&mut input);
    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 2, &test_db()).unwrap();
    Cursor::new(duplex.out)
}

const fn render_request(request_id: u32, scene: SceneState, samples: u32) -> RenderRequest {
    RenderRequest {
        request_id,
        scene,
        first_sample: 0,
        samples,
        stream: super::fixtures::final_only(0),
        intent: RequestIntent::Batch,
    }
}

/// The first reply frame, split into the `WELCOME` and whether the marker followed it.
fn read_welcome(out: &mut Cursor<Vec<u8>>) -> (Welcome, bool) {
    let frame = framing::read_frame(out).unwrap();
    let (body, tail) = split_welcome_tail(frame);
    let welcome: Welcome = postcard::from_bytes(&body).unwrap();
    (welcome, tail)
}

#[test]
fn only_a_viewer_that_sent_the_marker_is_answered_with_one() {
    // A zoning viewer: the reply carries the marker, and the decoded bit is set.
    let mut out = run(true, |_| {});
    let (_, tail) = read_welcome(&mut out);
    assert!(
        tail,
        "a zoning HELLO is answered with the capability marker"
    );

    // A default viewer (no marker): the reply is byte-for-byte the plain encoding, with no
    // trailing byte of any kind -- exactly what a default worker would send.
    let mut out = run(false, |_| {});
    let frame = framing::read_frame(&mut out).unwrap();
    let (welcome, rest) = postcard::take_from_bytes::<Welcome>(&frame).unwrap();
    assert!(rest.is_empty(), "a default viewer must not see extra bytes");
    assert!(!welcome.zoning);
    assert_eq!(frame, postcard::to_allocvec(&welcome).unwrap());
    assert!(!frame.ends_with(&ZONING_TAIL));
}

#[test]
fn a_zoned_render_with_its_payload_equals_the_local_cpu_render() {
    let local_scene = zoned_scene(heavier_scene());
    let samples = 16;
    let local = render_core::trace_samples(&local_scene, 0, samples, 2);

    let mut out = run(true, |input| {
        // `send_render_request` writes the payload first, then the request; the scene on the
        // wire carries no zones.
        send_render_request(input, &render_request(31, local_scene.clone(), samples)).unwrap();
    });
    let (welcome, tail) = read_welcome(&mut out);
    assert!(tail);
    assert!(welcome.render.is_some());
    let events = read_stream_until_done(&mut out);
    assert!(
        matches!(events.last().unwrap().0, StreamEvent::Done(d) if !d.cancelled),
        "{:?}",
        events.last()
    );

    let (w, h) = (local_scene.width, local_scene.height);
    let mut acc = Accumulator::new(w, h);
    acc.begin_request_for_range(31, 0, samples);
    for (event, payload) in &events {
        acc.apply(event, payload.as_deref()).unwrap();
    }
    assert_eq!(acc.samples_done(), samples);
    for (a, b) in acc.buffer().iter().zip(&local) {
        let scale = a.abs().max(b.abs()).max(glam::Vec3::splat(1e-6));
        assert!(
            ((*a - *b).abs() / scale).max_element() < 1e-3,
            "worker={a:?} local={b:?}"
        );
    }

    // Control: the same wire scene WITHOUT the zones renders differently, so the payload
    // really was applied.
    let mut stripped = local_scene;
    stripped.material.zoning = None;
    let base_only = render_core::trace_samples(&stripped, 0, samples, 2);
    let l1_difference: f32 = acc
        .buffer()
        .iter()
        .zip(&base_only)
        .map(|(a, b)| (*a - *b).abs().element_sum())
        .sum();
    let l1_base: f32 = base_only.iter().map(|b| b.abs().element_sum()).sum();
    assert!(
        l1_difference > 0.02 * l1_base,
        "zoned vs base-only differ by only {l1_difference} of {l1_base}: the zones changed nothing"
    );
}

#[test]
fn a_payload_for_a_material_that_is_not_per_mm_is_refused() {
    let mut scene = zoned_scene(tiny_scene());
    let payload = ZoningPayload::for_scene(41, &scene).unwrap();
    scene.material.absorption_unit = AbsorptionUnit::ModelUnit;
    let mut out = run(true, |input| {
        write_zoning_payload(input, &payload).unwrap();
        write_message(
            input,
            &ClientMessage::RenderRequest(Box::new(render_request(41, scene, 4))),
        )
        .unwrap();
    });
    let (_, tail) = read_welcome(&mut out);
    assert!(tail);
    let events = read_stream_until_done(&mut out);
    let StreamEvent::Error(error) = &events.last().unwrap().0 else {
        panic!("expected an error, got {:?}", events.last());
    };
    assert_eq!(error.code, error_codes::VALIDATION_FAILED);
    assert_eq!(error.request_id, Some(41));
    assert!(
        error.message.contains("per-millimetre"),
        "{}",
        error.message
    );
}

#[test]
fn a_request_without_a_payload_renders_as_an_ordinary_scene() {
    // The default-viewer path on a zoning worker: nothing is stashed, nothing attached.
    let scene = tiny_scene();
    let direct = render_core::trace_samples(&scene, 0, 4, 2);
    let mut out = run(false, |input| {
        write_message(
            input,
            &ClientMessage::RenderRequest(Box::new(render_request(5, scene.clone(), 4))),
        )
        .unwrap();
    });
    let (_, tail) = read_welcome(&mut out);
    assert!(!tail);
    let events = read_stream_until_done(&mut out);
    let mut acc = Accumulator::new(scene.width, scene.height);
    acc.begin_request_for_range(5, 0, 4);
    for (event, payload) in &events {
        acc.apply(event, payload.as_deref()).unwrap();
    }
    for (a, b) in acc.buffer().iter().zip(&direct) {
        let scale = a.abs().max(b.abs()).max(glam::Vec3::splat(1e-6));
        assert!(((*a - *b).abs() / scale).max_element() < 1e-3);
    }
}

const fn batch_item(item_id: u32, scene: SceneState) -> BatchItem {
    BatchItem {
        item_id,
        width: scene.width,
        height: scene.height,
        scene,
        first_sample: 0,
        samples: 4,
    }
}

/// A batch of one zoned and one plain item: the zoned item's PNG equals the local preview of
/// the zoned scene, the plain one is unaffected.
#[test]
fn a_zoned_batch_item_equals_the_local_preview_of_the_same_scene() {
    let items = vec![
        batch_item(1, zoned_scene(tiny_scene())),
        batch_item(2, tiny_scene()),
    ];
    let request = BatchRenderRequest {
        request_id: 51,
        reply: BatchReply::FinalPng,
        items: items.clone(),
    };
    let payload = ZoningPayload::for_batch(51, &items).unwrap();
    let mut out = run(true, |input| {
        write_zoning_payload(input, &payload).unwrap();
        write_message(input, &ClientMessage::BatchRenderRequest(Box::new(request))).unwrap();
    });
    let (_, tail) = read_welcome(&mut out);
    assert!(tail);
    let mut events = Vec::new();
    while usize::try_from(out.position()).unwrap() < out.get_ref().len() {
        events.push(indicatrix_net::messages::read_stream_event(&mut out).unwrap());
    }
    for item in &items {
        let expected = {
            let accum = render_core::trace_samples_with_gpu(
                &GpuBackend::disabled(),
                &item.scene,
                item.first_sample,
                item.samples,
                2,
            );
            indicatrix::render_setup::encode_preview_png(
                item.width,
                item.height,
                &accum,
                item.samples,
            )
            .unwrap()
        };
        let got = events
            .iter()
            .find_map(|(event, payload)| match event {
                StreamEvent::BatchItemDone(done)
                    if done.request_id == 51 && done.item_id == item.item_id =>
                {
                    payload.clone()
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("item {} was not answered", item.item_id));
        assert_eq!(got, expected, "item {}", item.item_id);
    }
}

#[test]
fn a_batch_payload_naming_a_missing_item_refuses_the_whole_batch() {
    let items = vec![batch_item(1, zoned_scene(tiny_scene()))];
    let mut payload = ZoningPayload::for_batch(52, &items).unwrap();
    payload.materials[0].item_id = Some(99);
    let request = BatchRenderRequest {
        request_id: 52,
        reply: BatchReply::FinalPng,
        items,
    };
    let mut out = run(true, |input| {
        write_zoning_payload(input, &payload).unwrap();
        write_message(input, &ClientMessage::BatchRenderRequest(Box::new(request))).unwrap();
    });
    let _ = read_welcome(&mut out);
    let (event, _) = indicatrix_net::messages::read_stream_event(&mut out).unwrap();
    let StreamEvent::Error(error) = event else {
        panic!("expected the batch to be refused, got {event:?}");
    };
    assert_eq!(error.code, error_codes::VALIDATION_FAILED);
    assert_eq!(error.request_id, Some(52));
    assert!(error.message.contains("99"), "{}", error.message);
}
