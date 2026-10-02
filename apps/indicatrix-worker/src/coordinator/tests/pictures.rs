//! "Final picture only", tilt routing and `CAPABILITY_CHANGED` through a real
//! coordinator on loopback with a joined CPU worker.

use super::{
    fixtures::{bundle, coordinator_args, fan_out_config, pki_with_server, start, wait_for},
    support::{collect, render, scene, send, spawn_worker, viewer},
};
use crate::{
    coordinator::{JobConfig, LivenessConfig},
    render_core::trace_samples,
};
use indicatrix::{
    optics::raytracer::{Camera, DEFAULT_FOV_DEG},
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
        Backend, ClientMessage, DisplayEncoding, FinalImageRequest, FinalOutput, PayloadEncoding,
        PeerRole, RequestIntent, StreamEvent, TILT_CURVE_AXIS_COUNT, TiltCurvesRequest,
        TiltCurvesResponse, TransferMode, WireColorSpace, error_codes,
    },
    radiance::PayloadEncoder,
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
                viewer_samples: 0,
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

/// v16: `viewer_samples` reserves the request's tail for the viewer; the server plans
/// only the rest, the viewer uploads its own trace as `CONTRIBUTION`, and the merged
/// `FINAL_IMAGE` is byte-identical to summing both halves directly -- the server's own
/// share and the viewer's, exactly as each was traced.
#[test]
fn a_viewer_contribution_completes_a_byte_identical_final_image() {
    let pki = pki_with_server("pictures-contribution");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    spawn_worker(handle.worker_addr.unwrap(), &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));

    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    let image = scene(9, 7);
    let (first_sample, total_samples, viewer_samples) = (5, 4, 2);
    let server_samples = total_samples - viewer_samples;
    // Bit-identical to what the job itself traces for a 2-sample range (see the sibling
    // test above): the server's own share and the viewer's contribution, computed and
    // summed exactly the same way the production code does.
    let server_sum = trace_samples(&image, first_sample, server_samples, 2);
    let viewer_sum = trace_samples(&image, first_sample + server_samples, viewer_samples, 2);
    let mut expected_sum = server_sum;
    for (e, v) in expected_sum.iter_mut().zip(&viewer_sum) {
        *e += *v;
    }

    send(
        &mut client,
        &ClientMessage::FinalImageRequest(Box::new(FinalImageRequest {
            request_id: 7,
            scene: image,
            first_sample,
            samples: total_samples,
            width: 9,
            height: 7,
            color_space: WireColorSpace::Srgb,
            output: FinalOutput::PngRgba8,
            viewer_samples,
        })),
    );
    // Uploaded before reading anything back -- the server's own lanes take a moment,
    // giving this plenty of time to land before the wait even starts.
    let mut encoder = PayloadEncoder::new(PayloadEncoding::Raw);
    indicatrix_net::client::send_contribution(
        &mut client,
        7,
        (first_sample + server_samples, viewer_samples),
        9,
        7,
        &viewer_sum,
        &mut encoder,
    )
    .unwrap();

    let transcript = collect(&mut client, (9, 7), |_, _| {});
    let done = transcript.done.expect("DONE after FINAL_IMAGE");
    assert!(!done.cancelled && done.request_id == 7);
    assert_eq!(done.stats.samples_done, total_samples);
    assert_eq!(
        done.stats.reclaimed_samples, 0,
        "the contribution arrived in time; nothing should be reclaimed"
    );
    let (header, png) = transcript.final_image.expect("one FINAL_IMAGE");
    assert_eq!(
        (header.samples_done, header.encoding),
        (4, DisplayEncoding::Png)
    );
    let pixels = display::decode_rgba8(DisplayEncoding::Png, 9, 7, &png).unwrap();
    let expected = tonemap_accumulation(
        9,
        7,
        total_samples,
        &expected_sum,
        indicatrix::color::ColorSpace::Srgb,
    );
    assert_eq!(pixels, expected, "PNG differs from the server+viewer sum");
}

/// v16: when the viewer's `CONTRIBUTION` never arrives, the coordinator traces that
/// range itself after `contribution_wait` elapses, reports it in
/// `DONE.stats.reclaimed_samples`, and the export still succeeds -- with a picture
/// equal to tracing the whole request server-side.
#[test]
fn a_missing_contribution_is_reclaimed_and_reported_in_done_stats() {
    let pki = pki_with_server("pictures-reclaim");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    handle
        .coordinator
        .as_ref()
        .unwrap()
        .set_job_config(JobConfig {
            contribution_wait: Duration::from_millis(200),
            ..fan_out_config()
        });
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    spawn_worker(handle.worker_addr.unwrap(), &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));

    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    let image = scene(9, 7);
    let (first_sample, total_samples, viewer_samples) = (5, 4, 2);
    let server_samples = total_samples - viewer_samples;
    let server_sum = trace_samples(&image, first_sample, server_samples, 2);
    let reclaimed_sum = trace_samples(&image, first_sample + server_samples, viewer_samples, 2);
    let mut expected_sum = server_sum;
    for (e, r) in expected_sum.iter_mut().zip(&reclaimed_sum) {
        *e += *r;
    }

    send(
        &mut client,
        &ClientMessage::FinalImageRequest(Box::new(FinalImageRequest {
            request_id: 8,
            scene: image,
            first_sample,
            samples: total_samples,
            width: 9,
            height: 7,
            color_space: WireColorSpace::Srgb,
            output: FinalOutput::PngRgba8,
            viewer_samples,
        })),
    );
    // No CONTRIBUTION ever sent for request 8.
    let transcript = collect(&mut client, (9, 7), |_, _| {});
    let done = transcript.done.expect("DONE after FINAL_IMAGE");
    assert!(!done.cancelled && done.request_id == 8);
    assert_eq!(done.stats.samples_done, total_samples);
    assert_eq!(done.stats.reclaimed_samples, viewer_samples);
    let (_, png) = transcript.final_image.expect("one FINAL_IMAGE");
    let pixels = display::decode_rgba8(DisplayEncoding::Png, 9, 7, &png).unwrap();
    let expected = tonemap_accumulation(
        9,
        7,
        total_samples,
        &expected_sum,
        indicatrix::color::ColorSpace::Srgb,
    );
    assert_eq!(
        pixels, expected,
        "a reclaimed export must equal an all-server render"
    );
}

/// v16: `viewer_samples` over half of `samples` is refused with `VALIDATION_FAILED`,
/// before any route is planned -- no worker or own lane is even needed for this.
#[test]
fn viewer_samples_over_half_is_refused_with_validation_failed() {
    let pki = pki_with_server("pictures-viewer-share");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);
    let image = scene(4, 4);
    send(
        &mut client,
        &ClientMessage::FinalImageRequest(Box::new(FinalImageRequest {
            request_id: 9,
            scene: image,
            first_sample: 0,
            samples: 4,
            width: 4,
            height: 4,
            color_space: WireColorSpace::Srgb,
            output: FinalOutput::PngRgba8,
            viewer_samples: 3, // > samples / 2 == 2
        })),
    );
    let (event, _) = indicatrix_net::messages::read_stream_event(&mut client).unwrap();
    let StreamEvent::Error(err) = event else {
        panic!("expected StreamEvent::Error, got {event:?}");
    };
    assert_eq!(err.code, error_codes::VALIDATION_FAILED);
    assert_eq!(err.request_id, Some(9));
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
    let camera = Camera::new(image.yaw, image.pitch, image.distance, DEFAULT_FOV_DEG);
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
