//! Helpers for the job-execution tests: a test viewer that collects one request's
//! events, real CPU `join` workers, and a fake worker that dies on its first chunk.

use super::fixtures::{cpu_worker_setup, tls_client};
use crate::join::{JoinTarget, join_once};
use glam::Vec3;
use indicatrix_net::{
    SceneState,
    client::handshake_with_hello,
    handshake,
    messages::{
        ClientMessage, DisplayFrameHeader, Done, ErrorMsg, FinalImageHeader, RenderCapability,
        RenderRequest, RequestIntent, StreamConfig, StreamEvent, TransferMode, Welcome,
    },
    radiance::PayloadDecoder,
};
use std::{
    net::{Shutdown, SocketAddr, TcpStream},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    thread,
    time::Duration,
};

/// A test viewer's TLS stream.
pub type ViewerStream = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

/// A viewer handshake; panics if refused.
pub fn viewer(addr: SocketAddr, bundle_dir: &Path) -> (Welcome, ViewerStream) {
    let mut client = tls_client(addr, bundle_dir);
    let welcome = handshake_with_hello(&mut client, &handshake::local_hello()).unwrap();
    (welcome, client)
}

/// Sends `message` to the coordinator.
pub fn send(client: &mut ViewerStream, message: &ClientMessage) {
    indicatrix_net::messages::write_message(client, message).unwrap();
}

/// A `width x height` diamond scene.
pub fn scene(width: u32, height: u32) -> SceneState {
    SceneState {
        width,
        height,
        surface_glare: 1.0,
        tools: Vec::new(),
        ..super::fixtures::tiny_scene()
    }
}

/// A `RenderRequest` message.
pub fn render(
    request_id: u32,
    scene: SceneState,
    (first_sample, samples): (u32, u32),
    transfer_mode: TransferMode,
    intent: RequestIntent,
) -> ClientMessage {
    ClientMessage::RenderRequest(Box::new(RenderRequest {
        request_id,
        scene,
        first_sample,
        samples,
        stream: StreamConfig {
            transfer_mode,
            cadence_ms: 100,
            preview: None,
        },
        intent,
    }))
}

/// Everything one request produced, up to and including its `DONE` or `ERROR`.
#[derive(Default)]
pub struct Transcript {
    /// The sum of every `FRAME` delta.
    pub sum: Vec<Vec3>,
    /// Samples the `FRAME`s declared.
    pub frame_samples: u32,
    /// `PROGRESS` events seen.
    pub progress: u32,
    /// The last `DISPLAY_FRAME` and its payload.
    pub display: Option<(DisplayFrameHeader, Vec<u8>)>,
    /// `DISPLAY_FRAME`s seen.
    pub display_frames: u32,
    /// The `FINAL_IMAGE` and its payload.
    pub final_image: Option<(FinalImageHeader, Vec<u8>)>,
    /// The terminal `DONE`, if the request ended with one.
    pub done: Option<Done>,
    /// The terminal `ERROR`, if the request ended with one.
    pub error: Option<ErrorMsg>,
}

/// Reads events for a `width x height` request until its `DONE` or an `ERROR`.
/// `on_progress` sees every `PROGRESS` sample count (e.g. to cancel mid-job).
pub fn collect(
    client: &mut ViewerStream,
    (width, height): (u32, u32),
    mut on_progress: impl FnMut(&mut ViewerStream, u32),
) -> Transcript {
    let mut transcript = Transcript {
        sum: vec![Vec3::ZERO; width as usize * height as usize],
        ..Transcript::default()
    };
    let mut decoder = PayloadDecoder::new();
    loop {
        let (event, payload) = indicatrix_net::messages::read_stream_event(client).unwrap();
        match event {
            StreamEvent::Frame(h) => {
                assert!(
                    transcript.final_image.is_none(),
                    "no FRAME after FINAL_IMAGE"
                );
                decoder
                    .decode_and_add(
                        h.encoding,
                        h.raw_len,
                        &payload.unwrap(),
                        width,
                        height,
                        &mut transcript.sum,
                    )
                    .unwrap();
                transcript.frame_samples += h.samples;
            }
            StreamEvent::Progress(p) => {
                transcript.progress += 1;
                on_progress(client, p.samples_done);
            }
            StreamEvent::DisplayFrame(h) => {
                transcript.display_frames += 1;
                transcript.display = Some((h, payload.unwrap()));
            }
            StreamEvent::FinalImage(h) => transcript.final_image = Some((h, payload.unwrap())),
            StreamEvent::Done(done) => {
                transcript.done = Some(done);
                return transcript;
            }
            StreamEvent::Error(error) => {
                transcript.error = Some(error);
                return transcript;
            }
            _ => {}
        }
    }
}

/// Runs a real CPU `join` worker (2 threads) against `worker_addr` on a background
/// thread; it serves until the coordinator goes away.
pub fn spawn_worker(worker_addr: SocketAddr, bundle_dir: &Path) {
    let target = JoinTarget::from_bundle(&worker_addr.to_string(), bundle_dir).unwrap();
    let setup = cpu_worker_setup();
    thread::spawn(move || {
        let _ = join_once(&target, &setup);
    });
}

/// A fake worker reporting `capability` that answers `PING`s and dies (closes its
/// connection) on the first `RenderRequest`, counting it in `renders`.
pub fn spawn_dying_worker(
    worker_addr: SocketAddr,
    bundle_dir: &Path,
    capability: RenderCapability,
    renders: &Arc<AtomicU32>,
) {
    let mut tls = tls_client(worker_addr, bundle_dir);
    let renders = Arc::clone(renders);
    thread::spawn(move || {
        if handshake_with_hello(&mut tls, &handshake::local_worker_hello(capability)).is_err() {
            return;
        }
        let _ = tls.sock.set_read_timeout(Some(Duration::from_secs(300)));
        loop {
            match indicatrix_net::messages::read_message::<_, ClientMessage>(&mut tls) {
                Ok(ClientMessage::Ping { nonce }) => {
                    let _ = indicatrix_net::messages::write_stream_event(
                        &mut tls,
                        &StreamEvent::Pong { nonce },
                        None,
                    );
                }
                Ok(ClientMessage::RenderRequest(_)) => {
                    renders.fetch_add(1, Ordering::SeqCst);
                    let _ = tls.sock.shutdown(Shutdown::Both);
                    return;
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    });
}

/// Whether `a` and `b` agree to 1e-5 relative (plus a tiny absolute floor for values
/// near zero).
pub fn close(a: &[Vec3], b: &[Vec3]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            (x.to_array().iter().zip(y.to_array()))
                .all(|(p, q)| (p - q).abs() <= 1e-5f32.mul_add(p.abs().max(q.abs()), 1e-7))
        })
}
