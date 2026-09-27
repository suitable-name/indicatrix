//! Real-socket, manual (`#[ignore]`d) repro: does the emitter block on a slow reader?

use super::fixtures::tiny_scene;
use crate::serve::handle_connection;
use indicatrix_net::{
    SceneState, handshake,
    messages::{Cancel, ClientMessage, RenderRequest, StreamEvent},
};
use std::{net::TcpListener, thread};

/// Manual repro (real `TcpStream`, real OS socket buffers -- `super::fixtures::DuplexHalf`'s
/// unbounded `Vec` can never model this): starts a real `serve` connection, sends a
/// `RenderRequest` big enough that `FRAME` payloads fill the OS send buffer if the peer
/// never reads, then never reads for a while before sending `CANCEL`. If the emitter
/// thread blocks inside `write_all` once the socket backs up -- the same thread that
/// polls for `CANCEL` -- then `CANCEL` sits unread until this test drains the read
/// side again.
///
/// `WRITE_TIMEOUT` bounds any single blocked write, including the cancelled `DONE`
/// write and the `StreamEvent::Progress` heartbeat while waiting for the tracer to
/// stop, so against a peer that truly never drains, expect `DONE{cancelled:true}` (or
/// a transport error) within roughly one to two `WRITE_TIMEOUT`s of `CANCEL` being
/// sent, never unbounded.
#[test]
#[ignore = "manual repro: real sockets + sleeps, prints timing to demonstrate the write-blocks-the-poll-loop theory"]
fn repro_slow_reader_blocks_the_emitter_and_delays_cancel() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_connection(stream, 0, &super::fixtures::test_db())
    });

    let mut client = std::net::TcpStream::connect(addr).unwrap();
    indicatrix_net::messages::write_message(&mut client, &handshake::local_hello()).unwrap();
    let _welcome: indicatrix_net::messages::Welcome =
        indicatrix_net::messages::read_message(&mut client).unwrap();

    // Large enough that LiveProgressive FRAME deltas add up to several MB before the
    // request finishes -- past a default OS socket-buffer size if nothing reads them.
    let scene = SceneState {
        width: 1200,
        height: 1200,
        max_bounces: 2,
        ..tiny_scene()
    };
    let request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 99,
        scene,
        first_sample: 0,
        samples: 65_536,
        stream: indicatrix_net::messages::StreamConfig {
            transfer_mode: indicatrix_net::messages::TransferMode::LiveProgressive,
            cadence_ms: 100,
            preview: None,
        },
    };
    indicatrix_net::messages::write_message(
        &mut client,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    // Deliberately never read. Give the tracer/emitter time to produce and attempt
    // to write several cadence ticks' worth of FRAME data.
    eprintln!("not reading for 8s -- letting FRAME writes pile up unread");
    thread::sleep(std::time::Duration::from_secs(8));

    eprintln!("sending CANCEL now, still not reading FRAME payload");
    let cancel_sent_at = std::time::Instant::now();
    indicatrix_net::messages::write_message(
        &mut client,
        &ClientMessage::Cancel(Cancel { request_id: 99 }),
    )
    .unwrap();

    // Now start draining and time how long DONE{cancelled: true} takes to show up.
    // If the emitter were never blocked, this arrives promptly (~100-200ms); a long
    // delay confirms the write-blocks-the-poll-loop theory.
    client
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let drain_start = std::time::Instant::now();
    let mut frame_count = 0u32;
    let mut progress_count = 0u32;
    loop {
        let event: Result<(StreamEvent, Option<Vec<u8>>), _> =
            indicatrix_net::messages::read_stream_event(&mut client);
        match event {
            Ok((StreamEvent::Done(done), _)) => {
                eprintln!(
                    "DONE{{cancelled={}, samples_done={}, effective_cadence_ms={}}} arrived \
                     {:?} after CANCEL was sent ({:?} to drain from when reading resumed); \
                     saw {frame_count} FRAMEs and {progress_count} PROGRESSes total",
                    done.cancelled,
                    done.stats.samples_done,
                    done.stats.effective_cadence_ms,
                    cancel_sent_at.elapsed(),
                    drain_start.elapsed()
                );
                break;
            }
            Ok((StreamEvent::Frame(_), _)) => frame_count += 1,
            Ok((StreamEvent::Progress(_), _)) => progress_count += 1,
            Ok(_) => {}
            Err(e) => {
                eprintln!("stream ended before DONE: {e:?}");
                break;
            }
        }
    }

    drop(client);
    let _ = server.join();
}
