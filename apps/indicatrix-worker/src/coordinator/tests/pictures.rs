//! "Final picture only", tilt routing and `CAPABILITY_CHANGED` through a real
//! coordinator on loopback with a joined CPU worker.

use super::{
    fixtures::{bundle, coordinator_args, pki_with_server, start, wait_for},
    support::{collect, render, scene, send, spawn_worker, viewer},
};
use crate::{coordinator::LivenessConfig, render_core::trace_samples};
use indicatrix::{
    optics::raytracer::Camera,
    renderer::{
        denoise::AtrousDenoiser,
        frame_denoise::{DenoiseScratch, FirstHitSnapshot, denoise_and_tonemap_frame},
        guide_pass::generate_guide_buffers,
        tonemap::tonemap_accumulation,
    },
};
use indicatrix_net::{
    display,
    messages::{
        Backend, ClientMessage, DisplayEncoding, FinalImageRequest, FinalOutput, PeerRole,
        RequestIntent, StreamEvent, TILT_CURVE_AXIS_COUNT, TiltCurvesRequest, TiltCurvesResponse,
        TransferMode, WireColorSpace,
    },
};
use std::{sync::Arc, time::Duration};

const WAIT: Duration = Duration::from_secs(20);

/// A `FinalImageRequest` answered through a coordinator and a joined
/// worker is byte-identical to the GUI export's own `tonemap_accumulation` of the same
/// float sum, rendered directly on this machine (2 samples: the only chunk and sub-batch
/// partition is 1 + 1 on both paths, so the float sums agree bit for bit). PROGRESS,
/// then exactly one `FINAL_IMAGE`, then `DONE`.
#[test]
fn a_final_image_is_byte_identical_to_the_gui_tonemap_of_the_same_sum() {
    let pki = pki_with_server("pictures-final");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    spawn_worker(handle.worker_addr.unwrap(), &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));

    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    let image = scene(9, 7);
    let sum = trace_samples(&image, 5, 2, 2);
    for (id, color_space) in [(1, WireColorSpace::Srgb), (2, WireColorSpace::DisplayP3)] {
        send(
            &mut client,
            &ClientMessage::FinalImageRequest(Box::new(FinalImageRequest {
                request_id: id,
                scene: image.clone(),
                first_sample: 5,
                samples: 2,
                width: 9,
                height: 7,
                color_space,
                output: FinalOutput::PngRgba8,
            })),
        );
        let transcript = collect(&mut client, (9, 7), |_, _| {});
        let done = transcript.done.expect("DONE after FINAL_IMAGE");
        assert!(!done.cancelled && done.request_id == id && done.stats.samples_done == 2);
        assert_eq!(
            transcript.frame_samples, 0,
            "no FRAME on a final-picture request"
        );
        let (header, png) = transcript.final_image.expect("one FINAL_IMAGE");
        assert_eq!((header.request_id, header.width, header.height), (id, 9, 7));
        assert_eq!(
            (header.samples_done, header.encoding),
            (2, DisplayEncoding::Png)
        );
        let pixels = display::decode_rgba8(DisplayEncoding::Png, 9, 7, &png).unwrap();
        let expected = tonemap_accumulation(9, 7, 2, &sum, color_space.into());
        assert_eq!(
            pixels, expected,
            "{color_space:?}: PNG differs from the GUI tonemap"
        );
    }
}

/// Display-only live view: `DisplayOnly` on a render-less coordinator runs on the
/// single fastest worker and streams DENOISED, tone-mapped `DISPLAY_FRAME`s (no
/// `FRAME`); the last one is the whole request's picture, byte-identical to the GUI's
/// own `denoise_and_tonemap_frame` of the same sum with guides from the same pose and
/// geometry -- and not the plain tone-map.
#[test]
fn display_only_streams_tone_mapped_display_frames() {
    let pki = pki_with_server("pictures-display");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    spawn_worker(handle.worker_addr.unwrap(), &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));

    let (welcome, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    let image = scene(8, 6);
    let mode = TransferMode::DisplayOnly;
    send(
        &mut client,
        &render(3, image.clone(), (0, 2), mode, RequestIntent::Interactive),
    );
    let transcript = collect(&mut client, (8, 6), |_, _| {});
    assert_eq!(transcript.done.expect("DONE").stats.samples_done, 2);
    assert_eq!(transcript.frame_samples, 0, "no FRAME in DisplayOnly");
    assert!(transcript.display_frames >= 1);
    let (header, payload) = transcript.display.expect("a DISPLAY_FRAME");
    assert_eq!((header.request_id, header.samples_done), (3, 2));
    let encoding = DisplayEncoding::for_payload_encoding(welcome.payload_encoding);
    assert_eq!(header.encoding, encoding);
    let pixels = display::decode_rgba8(encoding, 8, 6, &payload).unwrap();
    let sum = trace_samples(&image, 0, 2, 2);
    let camera = Camera::new(image.yaw, image.pitch, image.distance, 42.0);
    let guides = generate_guide_buffers(8, 6, &camera, &image.planes);
    let expected = denoise_and_tonemap_frame(
        FirstHitSnapshot {
            width: 8,
            height: 6,
            current_sample_count: 2,
            accum_buffer: &sum,
            first_hit_depth: &guides.depth,
            first_hit_normal: &guides.normal,
            first_hit_facet_id: &guides.facet_id,
        },
        &mut DenoiseScratch {
            denoiser: &mut AtrousDenoiser::new(),
            avg_color_buf: &mut Vec::new(),
            filtered_buf: &mut Vec::new(),
        },
    );
    assert_eq!(
        pixels, expected,
        "the GUI's denoised picture of the same sum"
    );
    let plain = tonemap_accumulation(8, 6, 2, &sum, indicatrix::color::ColorSpace::Srgb);
    assert_ne!(
        pixels, plain,
        "a display frame is denoised, not just tone-mapped"
    );
}

/// A render-less coordinator forwards `TiltCurvesRequest` whole to one idle worker
/// and relays the curves under the viewer's own `request_id`.
#[test]
fn tilt_curves_are_forwarded_to_a_joined_worker() {
    let pki = pki_with_server("pictures-tilt");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    spawn_worker(handle.worker_addr.unwrap(), &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));

    let (welcome, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    assert!(welcome.tilt_curves);
    // Unoptimised, the worker needs ~35 s for the four axes.
    client
        .sock
        .set_read_timeout(Some(Duration::from_secs(180)))
        .unwrap();
    send(
        &mut client,
        &ClientMessage::TiltCurvesRequest(Box::new(TiltCurvesRequest {
            request_id: 41,
            scene: scene(4, 4),
        })),
    );
    match indicatrix_net::messages::read_message::<_, TiltCurvesResponse>(&mut client).unwrap() {
        TiltCurvesResponse::Curves(result) => {
            assert_eq!(result.request_id, 41);
            assert_eq!(result.axes.len(), TILT_CURVE_AXIS_COUNT);
        }
        other => panic!("expected curves, got {other:?}"),
    }
    assert!(wait_for(WAIT, || registry
        .workers()
        .iter()
        .all(|(_, idle)| *idle)));
}

/// v14 `CAPABILITY_CHANGED`: an idle viewer of a bare coordinator (WELCOME.render
/// `None`) is told as soon as a worker joins, without sending anything.
#[test]
fn an_idle_viewer_learns_about_a_joining_worker() {
    let pki = pki_with_server("pictures-capability");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());

    let (welcome, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    assert_eq!(welcome.render, None);
    spawn_worker(handle.worker_addr.unwrap(), &worker_bundle);
    let (event, _) = indicatrix_net::messages::read_stream_event(&mut client).unwrap();
    let StreamEvent::CapabilityChanged {
        render: Some(render),
    } = event
    else {
        panic!("expected CAPABILITY_CHANGED with a capability, got {event:?}");
    };
    assert_eq!(
        render.backend,
        Backend::Coordinator {
            workers: 1,
            threads: 2,
            gpus: 0
        }
    );
    // The connection keeps serving requests afterwards.
    send(&mut client, &ClientMessage::Ping { nonce: 9 });
    let (event, _) = indicatrix_net::messages::read_stream_event(&mut client).unwrap();
    assert_eq!(event, StreamEvent::Pong { nonce: 9 });
}
