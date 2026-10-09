//! Tests for protocol v24 batched preview requests (`serve::connection::batch`): every
//! item of a batch is answered with the PNG a local preview would produce from the same
//! sum, one bad item fails alone, a `CANCEL` ends a batch early, and a second batch sent
//! while the first runs is queued and served on the same connection.

use super::fixtures::{DuplexHalf, heavier_scene, test_db, tiny_scene};
use crate::{render_core, serve::handle_connection};
use indicatrix::{geometry::ToolPrimitive, renderer::gpu_backend::GpuBackend};
use indicatrix_net::{
    SceneState, handshake,
    messages::{
        BatchItem, BatchRenderRequest, BatchReply, Cancel, ClientMessage, StreamEvent, Welcome,
    },
};
use std::{collections::BTreeMap, io::Cursor};

/// A concave scene (one ball tool): `scene_uses_gpu` is false for it, so it is traced on
/// the CPU even where a GPU is present.
fn concave_scene() -> SceneState {
    let mut scene = tiny_scene();
    scene.tools = vec![ToolPrimitive::ball(glam::Vec3::new(0.0, 0.0, 0.4), 0.2)];
    scene
}

const fn item(item_id: u32, scene: SceneState, first_sample: u32, samples: u32) -> BatchItem {
    BatchItem {
        item_id,
        width: scene.width,
        height: scene.height,
        scene,
        first_sample,
        samples,
    }
}

const fn batch(request_id: u32, items: Vec<BatchItem>) -> BatchRenderRequest {
    BatchRenderRequest {
        request_id,
        reply: BatchReply::FinalPng,
        items,
    }
}

/// The PNG a local preview makes for `item`: the CPU sum, tone-mapped and encoded by the
/// shared function.
fn expected_png(item: &BatchItem) -> Vec<u8> {
    let accum = render_core::trace_samples_with_gpu(
        &GpuBackend::disabled(),
        &item.scene,
        item.first_sample,
        item.samples,
        2,
    );
    indicatrix::render_setup::encode_preview_png(item.width, item.height, &accum, item.samples)
        .expect("a CPU sum always encodes")
}

/// Runs a scripted connection: `HELLO`, then `messages`, then silence.
fn run(messages: &[ClientMessage]) -> Cursor<Vec<u8>> {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    for message in messages {
        indicatrix_net::messages::write_message(&mut input, message).unwrap();
    }
    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 2, &test_db()).unwrap();
    let mut out = Cursor::new(duplex.out);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out).unwrap();
    out
}

/// Every event up to the end of the stream.
fn read_all(out: &mut Cursor<Vec<u8>>) -> Vec<(StreamEvent, Option<Vec<u8>>)> {
    let mut events = Vec::new();
    while out.position() < out.get_ref().len() as u64 {
        events.push(indicatrix_net::messages::read_stream_event(out).unwrap());
    }
    events
}

/// `(item_id -> PNG)` of the `BATCH_ITEM_DONE` events of `request_id`, in arrival order.
fn pictures(events: &[(StreamEvent, Option<Vec<u8>>)], request_id: u32) -> Vec<(u32, Vec<u8>)> {
    events
        .iter()
        .filter_map(|(event, payload)| match event {
            StreamEvent::BatchItemDone(done) if done.request_id == request_id => Some((
                done.item_id,
                payload.clone().expect("a picture has a payload"),
            )),
            _ => None,
        })
        .collect()
}

/// The `cancelled` flag of `request_id`'s `BATCH_DONE`, and whether it is the last event
/// that batch produced.
fn batch_done(events: &[(StreamEvent, Option<Vec<u8>>)], request_id: u32) -> (bool, bool) {
    let position = events
        .iter()
        .position(
            |(event, _)| matches!(event, StreamEvent::BatchDone(d) if d.request_id == request_id),
        )
        .unwrap_or_else(|| panic!("no BATCH_DONE for batch {request_id}"));
    let StreamEvent::BatchDone(done) = &events[position].0 else {
        unreachable!()
    };
    let last = events[position + 1..]
        .iter()
        .all(|(event, _)| event.request_id() != Some(request_id));
    (done.cancelled, last)
}

#[test]
fn a_mixed_batch_returns_every_item_with_the_shared_png_bytes() {
    let items = vec![
        item(10, tiny_scene(), 0, 4),
        item(11, concave_scene(), 8, 4),
        item(12, tiny_scene(), 16, 2),
    ];
    let mut out = run(&[ClientMessage::BatchRenderRequest(Box::new(batch(
        31,
        items.clone(),
    )))]);
    let events = read_all(&mut out);

    let got: BTreeMap<u32, Vec<u8>> = pictures(&events, 31).into_iter().collect();
    assert_eq!(got.len(), items.len(), "every item must be answered once");
    for item in &items {
        assert_eq!(
            got[&item.item_id],
            expected_png(item),
            "item {} must equal the local preview of the same sum",
            item.item_id
        );
    }
    // Homogeneous GPU/CPU runs keep the input order.
    let order: Vec<u32> = pictures(&events, 31).iter().map(|(id, _)| *id).collect();
    assert_eq!(order, vec![10, 11, 12]);
    assert_eq!(batch_done(&events, 31), (false, true));
    assert!(
        !events
            .iter()
            .any(|(event, _)| matches!(event, StreamEvent::BatchItemFailed(_))),
        "no item may fail"
    );
}

#[test]
fn a_bad_item_fails_alone_and_the_rest_of_the_batch_is_served() {
    let mut wrong_size = item(2, tiny_scene(), 0, 2);
    wrong_size.width += 1;
    let items = vec![
        item(1, tiny_scene(), 0, 2),
        wrong_size,
        item(3, tiny_scene(), 4, 2),
    ];
    let mut out = run(&[ClientMessage::BatchRenderRequest(Box::new(batch(
        32, items,
    )))]);
    let events = read_all(&mut out);

    let failed: Vec<u32> = events
        .iter()
        .filter_map(|(event, _)| match event {
            StreamEvent::BatchItemFailed(f) if f.request_id == 32 => Some(f.item_id),
            _ => None,
        })
        .collect();
    assert_eq!(failed, vec![2]);
    let done: Vec<u32> = pictures(&events, 32).iter().map(|(id, _)| *id).collect();
    assert_eq!(done, vec![1, 3]);
    assert_eq!(batch_done(&events, 32), (false, true));
}

#[test]
fn a_cancel_mid_batch_ends_it_with_a_cancelled_batch_done() {
    // Heavy enough that the scripted CANCEL is read long before the batch could finish.
    let items: Vec<BatchItem> = (0..6)
        .map(|id| item(id, heavier_scene(), 64 * id, 64))
        .collect();
    let mut out = run(&[
        ClientMessage::BatchRenderRequest(Box::new(batch(33, items.clone()))),
        ClientMessage::Cancel(Cancel { request_id: 33 }),
    ]);
    let events = read_all(&mut out);

    let (cancelled, last) = batch_done(&events, 33);
    assert!(cancelled, "BATCH_DONE must say the batch was cancelled");
    assert!(last, "nothing of the batch may follow BATCH_DONE");
    assert!(
        pictures(&events, 33).len() < items.len(),
        "a cancelled batch leaves the unfinished items unanswered"
    );
}

#[test]
fn a_second_batch_sent_while_the_first_runs_is_served_on_the_same_connection() {
    let first = vec![item(0, tiny_scene(), 0, 4), item(1, concave_scene(), 4, 4)];
    let second = vec![item(0, tiny_scene(), 32, 4), item(1, tiny_scene(), 40, 2)];
    let mut out = run(&[
        ClientMessage::BatchRenderRequest(Box::new(batch(41, first.clone()))),
        ClientMessage::BatchRenderRequest(Box::new(batch(42, second.clone()))),
    ]);
    let events = read_all(&mut out);

    for (request_id, items) in [(41, &first), (42, &second)] {
        let got: BTreeMap<u32, Vec<u8>> = pictures(&events, request_id).into_iter().collect();
        assert_eq!(
            got.len(),
            items.len(),
            "batch {request_id}: every item answered"
        );
        for item in items {
            assert_eq!(got[&item.item_id], expected_png(item), "batch {request_id}");
        }
        assert_eq!(batch_done(&events, request_id), (false, true));
    }
}

#[test]
fn a_batch_with_no_items_is_refused_and_the_connection_stays_usable() {
    let mut out = run(&[
        ClientMessage::BatchRenderRequest(Box::new(batch(51, Vec::new()))),
        ClientMessage::BatchRenderRequest(Box::new(batch(52, vec![item(0, tiny_scene(), 0, 2)]))),
    ]);
    let events = read_all(&mut out);
    assert!(
        events.iter().any(|(event, _)| matches!(
            event,
            StreamEvent::Error(e) if e.request_id == Some(51)
        )),
        "the empty batch gets an ERROR naming it"
    );
    assert_eq!(pictures(&events, 52).len(), 1);
    assert_eq!(batch_done(&events, 52), (false, true));
}
