//! Loopback end-to-end tests over real TLS: a coordinator started with `serve::start`
//! (ephemeral ports), real `join` connections, real enrollment tokens.

use super::fixtures::{
    FAST_LIVENESS, bundle, coordinator_args, cpu_worker_setup, pki_with_server, start, tiny_scene,
    tls_client, unique_temp_dir, wait_for,
};
use crate::{
    coordinator::{Capacity, LivenessConfig},
    join::{JoinError, JoinTarget, join_once},
};
use indicatrix_net::{
    client::{ClientError, handshake_with_hello},
    enroll::EnrollResponse,
    handshake,
    messages::{
        Backend, ClientMessage, PeerRole, RenderRequest, RequestIntent, StreamConfig, StreamEvent,
        TransferMode, Welcome, error_codes,
    },
};
use std::{net::SocketAddr, path::Path, sync::Arc, thread, time::Duration};

const WAIT: Duration = Duration::from_secs(10);

/// Runs `join_once` on a background thread (it blocks while registered).
fn spawn_worker(worker_addr: SocketAddr, bundle_dir: &Path) -> thread::JoinHandle<()> {
    let target = JoinTarget::from_bundle(&worker_addr.to_string(), bundle_dir).unwrap();
    let setup = cpu_worker_setup();
    thread::spawn(move || {
        let _ = join_once(&target, &setup);
    })
}

/// A viewer handshake with the viewer bundle; the `WELCOME` (or the refusal).
fn viewer_welcome(
    addr: SocketAddr,
    bundle_dir: &Path,
) -> (
    Result<Welcome, ClientError>,
    rustls::StreamOwned<rustls::ClientConnection, std::net::TcpStream>,
) {
    let mut client = tls_client(addr, bundle_dir);
    let welcome = handshake_with_hello(&mut client, &handshake::local_hello());
    (welcome, client)
}

fn render_request(request_id: u32) -> ClientMessage {
    ClientMessage::RenderRequest(Box::new(RenderRequest {
        request_id,
        scene: tiny_scene(),
        first_sample: 0,
        samples: 3,
        stream: StreamConfig {
            transfer_mode: TransferMode::FinalOnly,
            cadence_ms: 0,
            preview: None,
        },
        intent: RequestIntent::Batch,
    }))
}

/// A bare coordinator (no `--render`) is a library-only remote until a worker
/// joins; then `WELCOME.render` is `Backend::Coordinator{..}` with the worker's counts,
/// the registry holds the worker's reported capability, and a `RenderRequest` is
/// rendered by the joined worker (one FRAME of all samples, then DONE).
#[test]
fn a_joined_worker_registers_and_turns_a_bare_coordinator_into_a_render_endpoint() {
    let pki = pki_with_server("join");
    let viewer = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());

    let (welcome, _client) = viewer_welcome(handle.viewer_addr, &viewer);
    let welcome = welcome.unwrap();
    assert_eq!(welcome.render, None, "a bare coordinator renders nothing");
    assert!(welcome.library && !welcome.tilt_curves);

    let changes = registry.subscribe();
    let _worker = spawn_worker(handle.worker_addr.unwrap(), &worker);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));
    assert_eq!(
        changes.recv_timeout(WAIT).unwrap(),
        Capacity {
            workers: 1,
            threads: 2,
            gpus: 0,
            max_pixels: crate::validate::MAX_PIXELS,
            hdr_workers: 1,
        }
    );
    let workers = registry.workers();
    assert_eq!(workers[0].0.capability.backend, Backend::Cpu { threads: 2 });
    // Pin identity (`--pin-interactive-worker`): the label comes from the real worker certificate's
    // `worker:box` Common Name, parsed at registration.
    assert_eq!(workers[0].0.label.as_deref(), Some("box"));

    let (welcome, mut client) = viewer_welcome(handle.viewer_addr, &viewer);
    let render = welcome.unwrap().render.unwrap();
    assert_eq!(
        render.backend,
        Backend::Coordinator {
            workers: 1,
            threads: 2,
            gpus: 0
        }
    );

    indicatrix_net::messages::write_message(&mut client, &render_request(7)).unwrap();
    let mut frame_samples = 0;
    loop {
        match indicatrix_net::messages::read_stream_event(&mut client).unwrap() {
            (StreamEvent::Frame(header), Some(_)) => frame_samples += header.samples,
            (StreamEvent::Done(done), _) => {
                assert!(!done.cancelled && done.request_id == 7);
                assert_eq!(done.stats.samples_done, 3);
                break;
            }
            (StreamEvent::Error(e), _) => panic!("render failed: {e:?}"),
            _ => {}
        }
    }
    assert_eq!(frame_samples, 3);
}

/// A viewer certificate dialling the worker port is refused with `ROLE_REFUSED` (and
/// never registered); a worker certificate on the viewer port likewise.
#[test]
fn certificates_of_the_wrong_role_are_refused_on_both_ports() {
    let pki = pki_with_server("roles");
    let viewer = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());

    let target =
        JoinTarget::from_bundle(&handle.worker_addr.unwrap().to_string(), &viewer).unwrap();
    match join_once(&target, &cpu_worker_setup()) {
        Err(JoinError::Refused(e)) => {
            assert_eq!(e.code, error_codes::ROLE_REFUSED);
            assert!(e.message.contains("viewer certificate"), "{}", e.message);
        }
        other => panic!("expected ROLE_REFUSED, got {other:?}"),
    }
    assert_eq!(handle.registry.as_ref().unwrap().capacity().workers, 0);

    let (welcome, _client) = viewer_welcome(handle.viewer_addr, &worker);
    match welcome {
        Err(ClientError::Refused(e)) => {
            assert_eq!(e.code, error_codes::ROLE_REFUSED);
            assert!(e.message.contains("worker certificate"), "{}", e.message);
        }
        other => panic!("expected ROLE_REFUSED, got {other:?}"),
    }
}

/// Worker token enrollment over the worker enrollment port yields a WORKER certificate
/// that joins; a viewer token's certificate is refused on the worker port; a viewer
/// token cannot be claimed on the worker listener, and neither listener mints the other
/// role.
#[test]
fn worker_token_enrollment_yields_a_joinable_certificate_and_viewer_tokens_do_not() {
    let pki = pki_with_server("tokens");
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let ca = pki.join(crate::pki::CA_CERT_FILE);
    let worker_enroll = handle.worker_enroll_addr.unwrap().to_string();
    let viewer_enroll = handle.viewer_enroll_addr.unwrap().to_string();
    let issue =
        |addr: &str, name: &str| match crate::enroll_client::issue_token_over_tls(&ca, addr, name)
            .unwrap()
        {
            EnrollResponse::Issued { token, .. } => Ok(token),
            EnrollResponse::IssueRefused { reason } => Err(reason),
            other => panic!("unexpected {other:?}"),
        };
    let claim_into = |token: &str, addr: &str| {
        let bundle = indicatrix_net::enroll::claim(token, addr)?;
        let dir = unique_temp_dir("claimed");
        crate::enroll_client::write_bundle(&dir, &bundle).unwrap();
        Ok::<_, indicatrix_net::enroll::ClaimError>(dir)
    };

    // Each listener refuses to mint the other role.
    assert!(
        issue(&worker_enroll, "laptop")
            .unwrap_err()
            .contains("--role worker")
    );
    assert!(
        issue(&viewer_enroll, "worker:box")
            .unwrap_err()
            .contains("WORKER enrollment")
    );

    let worker_dir = claim_into(
        &issue(&worker_enroll, "worker:box").unwrap(),
        &worker_enroll,
    )
    .unwrap();
    assert_eq!(
        crate::join::bundle_role(&worker_dir).unwrap(),
        PeerRole::Worker
    );
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    let _worker = spawn_worker(handle.worker_addr.unwrap(), &worker_dir);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));

    let viewer_token = issue(&viewer_enroll, "laptop").unwrap();
    assert!(
        claim_into(&viewer_token, &worker_enroll).is_err(),
        "a viewer token must not claim on the worker enrollment listener"
    );
    let viewer_dir =
        claim_into(&issue(&viewer_enroll, "laptop2").unwrap(), &viewer_enroll).unwrap();
    assert_eq!(
        crate::join::bundle_role(&viewer_dir).unwrap(),
        PeerRole::Viewer
    );
    let target =
        JoinTarget::from_bundle(&handle.worker_addr.unwrap().to_string(), &viewer_dir).unwrap();
    assert!(matches!(
        join_once(&target, &cpu_worker_setup()),
        Err(JoinError::Refused(e)) if e.code == error_codes::ROLE_REFUSED
    ));
    assert_eq!(registry.capacity().workers, 1);
}

/// Liveness: a registered connection that never answers `PING` is dropped after the
/// deadline; a real `join` connection answers and stays.
#[test]
fn a_silent_worker_is_dropped_by_liveness_while_a_live_one_stays() {
    let pki = pki_with_server("liveness");
    let worker = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), FAST_LIVENESS);
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    let worker_addr = handle.worker_addr.unwrap();

    let _live = spawn_worker(worker_addr, &worker);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));
    let live_id = registry.workers()[0].0.worker_id;

    // Registers like a worker, then never reads or answers anything.
    let mut silent = tls_client(worker_addr, &worker);
    let hello = handshake::local_worker_hello(cpu_worker_setup().capability.clone());
    let welcome = handshake_with_hello(&mut silent, &hello).unwrap();
    let silent_id = welcome.registration.unwrap().worker_id;
    assert!(wait_for(WAIT, || registry.capacity().workers == 2));

    assert!(
        wait_for(WAIT, || registry.capacity().workers == 1),
        "the silent worker must be dropped after {:?}",
        FAST_LIVENESS.dead_after
    );
    let remaining: Vec<u32> = registry
        .workers()
        .iter()
        .map(|(w, _)| w.worker_id)
        .collect();
    assert_eq!(remaining, [live_id]);
    assert_ne!(live_id, silent_id);
    // Several more ping rounds: the live worker keeps answering.
    thread::sleep(FAST_LIVENESS.dead_after * 3);
    assert_eq!(registry.capacity().workers, 1);
    drop(silent);
}

/// `serve --render` (here `--only-cpu`) still renders a request end to end exactly like
/// the plain worker it replaces: plain `Backend::Cpu` in `WELCOME`, a full FRAME, DONE.
#[test]
fn serve_with_render_still_renders_a_request_end_to_end() {
    let pki = pki_with_server("render");
    let viewer = bundle(&pki, "laptop", PeerRole::Viewer);
    let mut args = coordinator_args(&pki);
    args.render = true;
    args.compute_mode = crate::cli::ComputeMode::OnlyCpu;
    let handle = start(&args, LivenessConfig::default());

    let (welcome, mut client) = viewer_welcome(handle.viewer_addr, &viewer);
    let welcome = welcome.unwrap();
    assert_eq!(
        welcome.render.as_ref().unwrap().backend,
        Backend::Cpu { threads: 2 }
    );
    assert!(welcome.tilt_curves && welcome.registration.is_none());

    indicatrix_net::messages::write_message(&mut client, &render_request(1)).unwrap();
    let mut frame_samples = 0;
    loop {
        let (event, payload) = indicatrix_net::messages::read_stream_event(&mut client).unwrap();
        match event {
            StreamEvent::Frame(header) => {
                frame_samples += header.samples;
                assert!(payload.is_some());
            }
            StreamEvent::Done(done) => {
                assert_eq!(done.request_id, 1);
                assert!(!done.cancelled);
                break;
            }
            StreamEvent::Error(e) => panic!("render failed: {e:?}"),
            _ => {}
        }
    }
    assert_eq!(frame_samples, 3);
}
