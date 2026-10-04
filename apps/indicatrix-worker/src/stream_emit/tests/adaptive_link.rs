//! Per-connection adaptive compression through the real emitter: one [`PeerLink`] shared
//! by consecutive requests on a "connection" whose writes are throttled (or not). A slow
//! peer must push the sender to the slow-link codec from the next frame on; a fast peer
//! must stay on its tier; a pinned link must not move at all.

use super::fixtures::tiny_scene;
use crate::stream_emit::{
    Output, ProducerOutcome, ProducerSink, StreamOutcome, StreamSpec, TimeoutRead, TimeoutWrite,
    run_stream_with,
};
use glam::Vec3;
use indicatrix_net::{
    SceneState,
    messages::{
        PayloadEncoding, RenderRequest, RequestIntent, StreamConfig, StreamEvent, TransferMode,
        adaptive::{BandwidthTier, PayloadChoice, PeerLink},
        read_stream_event,
    },
};
use std::{
    io::{Read, Write},
    sync::Arc,
    time::Duration,
};

const WIDTH: u32 = 512;
const HEIGHT: u32 = 256;

/// A connection double that never has anything to read and, when `mbps` is set, makes
/// every payload-sized write take as long as that link would. Everything written is kept.
struct Wire {
    mbps: Option<f64>,
    written: Vec<u8>,
}

impl Wire {
    fn new(mbps: Option<f64>) -> Self {
        Self {
            mbps,
            written: Vec::new(),
        }
    }
}

impl Read for Wire {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "nothing pending",
        ))
    }
}

impl Write for Wire {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Only payload-sized writes are slowed: a sleep per tiny header write would be
        // rounded up to the platform's timer granularity and distort the measurement.
        if let Some(mbps) = self.mbps
            && buf.len() >= 4096
        {
            std::thread::sleep(Duration::from_secs_f64(
                buf.len() as f64 * 8.0 / (mbps * 1.0e6),
            ));
        }
        self.written.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TimeoutRead for Wire {
    fn set_read_timeout(&mut self, _duration: Option<Duration>) -> std::io::Result<()> {
        Ok(())
    }
}

impl TimeoutWrite for Wire {
    fn set_write_timeout(&mut self, _duration: Option<Duration>) -> std::io::Result<()> {
        Ok(())
    }
}

fn scene() -> SceneState {
    SceneState {
        width: WIDTH,
        height: HEIGHT,
        surface_glare: 1.0,
        tools: Vec::new(),
        ..tiny_scene()
    }
}

/// A radiance sum of the test frame size that compresses to about half (a smooth gradient
/// with random low mantissa bits), so even compressed it stays far above the estimator's
/// minimum sample size.
fn smooth_sum() -> Vec<Vec3> {
    let mut state = 0x9e37_79b9_u32;
    let mut noisy = |base: f32| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        f32::from_bits(base.to_bits() ^ (state & 0xffff))
    };
    (0..WIDTH * HEIGHT)
        .map(|i| {
            let base = 1.0 + (i % 97) as f32 / 97.0;
            Vec3::new(noisy(base), noisy(base * 0.5), noisy(base * 0.25))
        })
        .collect()
}

/// Streams one `FinalOnly` request (one `FRAME` of the whole picture) over `wire` through
/// `link` and returns the encoding that frame's header named.
fn stream_one_frame(wire: &mut Wire, link: &Arc<PeerLink>, request_id: u32) -> PayloadEncoding {
    let request = RenderRequest {
        intent: RequestIntent::Batch,
        request_id,
        scene: scene(),
        first_sample: 0,
        samples: 1,
        stream: StreamConfig {
            transfer_mode: TransferMode::FinalOnly,
            cadence_ms: 100,
            preview: None,
        },
    };
    let spec = StreamSpec {
        request: &request,
        payload_encoding: PayloadEncoding::Raw,
        link: Some(link),
        output: Output::Radiance,
        contribution: None,
        stall_timeout: None,
    };
    let data = smooth_sum();
    let before = wire.written.len();
    let (outcome, _) = run_stream_with(wire, &spec, move |sink: ProducerSink| {
        sink.add_chunk(1, &data);
        sink.finish(ProducerOutcome::Complete {
            final_total: None,
            reclaimed_samples: 0,
        });
    })
    .expect("the stream ends cleanly");
    assert_eq!(outcome, StreamOutcome::Completed);

    let mut cursor = &wire.written[before..];
    while !cursor.is_empty() {
        let (event, _payload) = read_stream_event(&mut cursor).expect("well-formed event");
        if let StreamEvent::Frame(header) = event {
            return header.encoding;
        }
    }
    panic!("request {request_id} wrote no FRAME");
}

fn adaptive_link(peer: &str) -> Arc<PeerLink> {
    Arc::new(PeerLink::new(
        peer,
        PayloadChoice::Auto.policy(&PayloadEncoding::default_accept_list(), false),
    ))
}

#[test]
fn a_slow_peer_moves_the_sender_to_the_slow_link_codec_from_the_next_frame() {
    let link = adaptive_link("adaptive-link-test:slow");
    assert_eq!(link.tier(), BandwidthTier::DEFAULT);
    let mut wire = Wire::new(Some(20.0));

    let first = stream_one_frame(&mut wire, &link, 1);
    let second = stream_one_frame(&mut wire, &link, 2);
    let third = stream_one_frame(&mut wire, &link, 3);

    // Before anything is measured the default tier (300 Mbit/s) uses zstd level 1; one
    // 20 Mbit/s write later the connection sits on the slowest tier (zstd level 3).
    assert_eq!(first, PayloadEncoding::ShuffleZstd { level: 1 });
    assert_eq!(link.tier(), BandwidthTier::LOWEST);
    assert_eq!(second, PayloadEncoding::ShuffleZstd { level: 3 });
    assert_eq!(third, second, "the tier stays put on a steady slow link");
    let estimate = link.estimate().expect("the writes were measured");
    assert!(
        estimate < 50.0,
        "estimated {estimate} Mbit/s for a 20 Mbit/s wire"
    );
}

#[test]
fn a_fast_peer_goes_raw_after_the_first_measurement_and_stays_there() {
    let link = adaptive_link("adaptive-link-test:fast");
    let mut wire = Wire::new(None);

    let first = stream_one_frame(&mut wire, &link, 1);
    let second = stream_one_frame(&mut wire, &link, 2);
    let third = stream_one_frame(&mut wire, &link, 3);

    assert_eq!(first, PayloadEncoding::ShuffleZstd { level: 1 });
    assert_eq!(link.tier(), BandwidthTier::HIGHEST);
    assert_eq!(second, PayloadEncoding::Raw);
    assert_eq!(third, PayloadEncoding::Raw);
    assert_eq!(
        link.tier_switches(),
        0,
        "no flapping after the first placement"
    );
}

#[test]
fn a_pinned_link_never_adapts_whatever_the_wire_does() {
    let link = Arc::new(PeerLink::new(
        "adaptive-link-test:pinned",
        PayloadChoice::Fixed(PayloadEncoding::ShuffleLz4)
            .policy(&PayloadEncoding::default_accept_list(), false),
    ));
    let mut wire = Wire::new(Some(20.0));

    let encodings: Vec<_> = (1..=3)
        .map(|id| stream_one_frame(&mut wire, &link, id))
        .collect();

    assert!(
        encodings
            .iter()
            .all(|e| *e == PayloadEncoding::ShuffleLz4
                || !PayloadEncoding::ShuffleLz4.is_supported()),
        "{encodings:?}"
    );
    assert!(!link.is_tracking());
    assert_eq!(link.estimate(), None);
    assert_eq!(link.tier(), BandwidthTier::DEFAULT);
}

#[test]
fn a_loopback_peer_gets_raw_frames_under_auto() {
    let link = Arc::new(PeerLink::new(
        "adaptive-link-test:loopback",
        PayloadChoice::Auto.policy(&PayloadEncoding::default_accept_list(), true),
    ));
    let mut wire = Wire::new(None);
    for id in 1..=2 {
        assert_eq!(stream_one_frame(&mut wire, &link, id), PayloadEncoding::Raw);
    }
    assert_eq!(link.estimate(), None);
}
