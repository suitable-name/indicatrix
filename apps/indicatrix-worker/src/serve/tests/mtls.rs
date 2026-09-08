// See `super`'s module doc comment: real throwaway CAs and certs, real TLS handshakes,
// nothing mocked at the `rustls` layer.

use crate::serve::{
    handle_connection,
    tls::{Auth, Transport, accept_tls, build_transport},
};
use indicatrix_net::{
    handshake,
    messages::{Backend, ClientMessage, RenderRequest, StreamEvent, Welcome},
};
use rustls::pki_types::ServerName;
use std::{
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};

use super::{final_only, read_stream_until_done, tiny_scene};

fn unique_temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "indicatrix-worker-mtls-test-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn server_config_from(pki_dir: &Path) -> Arc<rustls::ServerConfig> {
    let ca = indicatrix_net::tls::load_ca(&pki_dir.join(crate::pki::CA_CERT_FILE)).unwrap();
    let certs =
        indicatrix_net::tls::load_certs(&pki_dir.join(crate::pki::SERVER_CERT_FILE)).unwrap();
    let key =
        indicatrix_net::tls::load_private_key(&pki_dir.join(crate::pki::SERVER_KEY_FILE)).unwrap();
    indicatrix_net::tls::server_config(ca, certs, key).unwrap()
}

/// Builds a client config that trusts `trust_ca_dir`'s CA and presents the client
/// certificate bundle in `bundle_dir` -- two separate directories, so a test can trust
/// one CA while presenting a certificate signed by a different one (see
/// `client_certificate_signed_by_a_different_ca_is_rejected`).
fn client_config_from(trust_ca_dir: &Path, bundle_dir: &Path) -> Arc<rustls::ClientConfig> {
    let ca = indicatrix_net::tls::load_ca(&trust_ca_dir.join(crate::pki::CA_CERT_FILE)).unwrap();
    let certs =
        indicatrix_net::tls::load_certs(&bundle_dir.join(crate::pki::CLIENT_CERT_FILE)).unwrap();
    let key = indicatrix_net::tls::load_private_key(&bundle_dir.join(crate::pki::CLIENT_KEY_FILE))
        .unwrap();
    indicatrix_net::tls::client_config(ca, certs, key).unwrap()
}

fn loopback_server_name() -> ServerName<'static> {
    ServerName::from(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST))
}

/// Signs a client certificate whose validity window is entirely in the past,
/// bypassing `crate::pki::issue_client`'s fixed lifetime -- there is no public API for
/// issuing a deliberately expired certificate, so this test-only helper builds one
/// directly with `rcgen`.
fn issue_expired_client_bundle(pki_dir: &Path, out: &Path) {
    let ca_issuer = crate::pki::load_ca(pki_dir).unwrap();
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "expired-client");
    params.not_before = time::OffsetDateTime::now_utc() - time::Duration::days(10);
    params.not_after = time::OffsetDateTime::now_utc() - time::Duration::days(1);
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
    let key_pair = rcgen::KeyPair::generate().unwrap();
    let cert = params.signed_by(&key_pair, &ca_issuer).unwrap();

    std::fs::create_dir_all(out).unwrap();
    let ca_pem = std::fs::read_to_string(pki_dir.join(crate::pki::CA_CERT_FILE)).unwrap();
    std::fs::write(out.join(crate::pki::CA_CERT_FILE), ca_pem).unwrap();
    std::fs::write(out.join(crate::pki::CLIENT_CERT_FILE), cert.pem()).unwrap();
    std::fs::write(
        out.join(crate::pki::CLIENT_KEY_FILE),
        key_pair.serialize_pem(),
    )
    .unwrap();
}

/// The full path: `cert init`, `issue-server`, `issue-client`, a real TLS 1.3 handshake
/// over loopback, HELLO/WELCOME, a `RenderRequest`, and a correctly sized radiance
/// buffer back -- proving the whole stack works together, not just each piece in
/// isolation.
#[test]
fn full_round_trip_handshake_and_render_over_loopback() {
    let pki = unique_temp_dir("roundtrip-pki");
    let bundle = unique_temp_dir("roundtrip-bundle");
    crate::pki::init(&pki).unwrap();
    crate::pki::issue_server(
        &pki,
        &["localhost".to_string()],
        &["127.0.0.1".parse().unwrap()],
    )
    .unwrap();
    crate::pki::issue_client(&pki, "test-client", &bundle).unwrap();

    let server_config = server_config_from(&pki);
    let client_cfg = client_config_from(&pki, &bundle);
    let auth = Auth::Allowlist(pki.join(crate::pki::ALLOWLIST_FILE));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let tls_stream = accept_tls(stream, &server_config, &auth, None)
            .expect("handshake and allowlist check should both succeed");
        handle_connection(tls_stream, 2, &super::test_db()).unwrap();
    });

    let tcp = TcpStream::connect(addr).unwrap();
    let conn = rustls::ClientConnection::new(client_cfg, loopback_server_name()).unwrap();
    let mut client = rustls::StreamOwned::new(conn, tcp);

    indicatrix_net::messages::write_message(&mut client, &handshake::local_hello()).unwrap();
    let welcome: Welcome = indicatrix_net::messages::read_message(&mut client).unwrap();
    assert!(matches!(
        welcome.render.as_ref().unwrap().backend,
        Backend::Cpu { .. }
    ));

    let scene = tiny_scene();
    let request = RenderRequest {
        request_id: 1,
        scene: scene.clone(),
        first_sample: 0,
        samples: 3,
        stream: final_only(0),
    };
    indicatrix_net::messages::write_message(
        &mut client,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();
    let events = read_stream_until_done(&mut client);
    let (header, payload) = events
        .iter()
        .find_map(|(e, p)| match e {
            StreamEvent::Frame(h) => Some((h, p.as_ref().unwrap())),
            _ => None,
        })
        .unwrap();
    assert_eq!(header.samples, 3);
    assert_eq!(
        payload.len(),
        scene.width as usize * scene.height as usize * indicatrix_net::radiance::BYTES_PER_PIXEL
    );

    drop(client);
    server.join().unwrap();

    std::fs::remove_dir_all(&pki).ok();
    std::fs::remove_dir_all(&bundle).ok();
}

/// A client certificate signed by a different CA than the server trusts must be
/// rejected before `check_auth` (the allowlist) ever runs -- this test uses
/// `Auth::AnyCaSignedClient` to isolate the CA-chain failure from the allowlist check
/// exercised separately below.
#[test]
fn client_certificate_signed_by_a_different_ca_is_rejected() {
    let pki_a = unique_temp_dir("diffca-a");
    let pki_b = unique_temp_dir("diffca-b");
    let bundle_b = unique_temp_dir("diffca-bundle-b");
    crate::pki::init(&pki_a).unwrap();
    crate::pki::issue_server(&pki_a, &[], &["127.0.0.1".parse().unwrap()]).unwrap();
    crate::pki::init(&pki_b).unwrap();
    crate::pki::issue_client(&pki_b, "intruder", &bundle_b).unwrap();

    let server_config = server_config_from(&pki_a);
    // Trusts CA A (so it accepts the server's own certificate), but presents a
    // client certificate signed by CA B.
    let client_cfg = client_config_from(&pki_a, &bundle_b);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        accept_tls(stream, &server_config, &Auth::AnyCaSignedClient, None)
    });

    let mut tcp = TcpStream::connect(addr).unwrap();
    let mut conn = rustls::ClientConnection::new(client_cfg, loopback_server_name()).unwrap();
    // Not asserted on: in a TLS 1.3 handshake the client can observe its own side as
    // "complete" before the server evaluates and rejects the client's certificate.
    // The server's own verdict, asserted below, is what this test is about.
    let _ = conn.complete_io(&mut tcp);

    let server_result = server.join().unwrap();
    assert!(
        server_result.is_none(),
        "the server must refuse a client certificate signed by a CA it doesn't trust"
    );

    std::fs::remove_dir_all(&pki_a).ok();
    std::fs::remove_dir_all(&pki_b).ok();
    std::fs::remove_dir_all(&bundle_b).ok();
}

/// Same CA on both sides (so the handshake itself succeeds), but the client's
/// certificate was never added to the allowlist -- proves the allowlist check in
/// `check_auth` runs after the handshake, not as a substitute for it.
#[test]
fn client_certificate_not_on_the_allowlist_is_rejected_after_a_successful_handshake() {
    let pki = unique_temp_dir("notallowed-pki");
    let bundle = unique_temp_dir("notallowed-bundle");
    crate::pki::init(&pki).unwrap();
    crate::pki::issue_server(&pki, &[], &["127.0.0.1".parse().unwrap()]).unwrap();
    crate::pki::issue_client(&pki, "stranger", &bundle).unwrap();

    // `issue_client` already added this fingerprint to the allowlist -- overwrite it
    // with an empty one so the certificate is valid but not trusted.
    let allowlist_path = pki.join(crate::pki::ALLOWLIST_FILE);
    std::fs::write(&allowlist_path, "").unwrap();

    let server_config = server_config_from(&pki);
    let client_cfg = client_config_from(&pki, &bundle);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        accept_tls(
            stream,
            &server_config,
            &Auth::Allowlist(allowlist_path),
            None,
        )
    });

    let mut tcp = TcpStream::connect(addr).unwrap();
    let mut conn = rustls::ClientConnection::new(client_cfg, loopback_server_name()).unwrap();
    conn.complete_io(&mut tcp)
        .expect("the TLS handshake itself must succeed -- same CA on both sides");

    let server_result = server.join().unwrap();
    assert!(
        server_result.is_none(),
        "the server must refuse a certificate that isn't on the allowlist, even though the handshake succeeded"
    );

    std::fs::remove_dir_all(&pki).ok();
    std::fs::remove_dir_all(&bundle).ok();
}

/// An expired client certificate is rejected during the handshake, and the error text
/// names expiry specifically, not a generic "handshake failed".
#[test]
fn an_expired_client_certificate_is_rejected_and_the_error_names_expiry() {
    let pki = unique_temp_dir("expired-pki");
    let bundle = unique_temp_dir("expired-bundle");
    crate::pki::init(&pki).unwrap();
    crate::pki::issue_server(&pki, &[], &["127.0.0.1".parse().unwrap()]).unwrap();
    issue_expired_client_bundle(&pki, &bundle);

    let server_config = server_config_from(&pki);
    let client_cfg = client_config_from(&pki, &bundle);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let conn = rustls::ServerConnection::new(server_config).unwrap();
        let mut tls_stream = rustls::StreamOwned::new(conn, stream);
        tls_stream
            .conn
            .complete_io(&mut tls_stream.sock)
            .map_err(|e| e.to_string())
    });

    let mut tcp = TcpStream::connect(addr).unwrap();
    let mut conn = rustls::ClientConnection::new(client_cfg, loopback_server_name()).unwrap();
    let _ = conn.complete_io(&mut tcp); // the client side errors too; not what this test is about

    let err = server
        .join()
        .unwrap()
        .expect_err("the server must reject an expired client certificate");
    assert!(
        err.to_lowercase().contains("expired"),
        "expected the error to name expiry specifically, got: {err}"
    );

    std::fs::remove_dir_all(&pki).ok();
    std::fs::remove_dir_all(&bundle).ok();
}

/// A certificate carrying only a DNS SAN can never validate a connection made by IP
/// address, since `rustls` ignores Common Name entirely. Pinned down so nobody "fixes"
/// the resulting failure by loosening verification instead of adding the right SAN
/// with `cert issue-server --ip`.
#[test]
fn connecting_by_ip_against_a_dns_only_san_certificate_fails() {
    let pki = unique_temp_dir("dnsonly-pki");
    let bundle = unique_temp_dir("dnsonly-bundle");
    crate::pki::init(&pki).unwrap();
    // Deliberately only a DNS SAN -- no --ip.
    crate::pki::issue_server(&pki, &["worker.lan".to_string()], &[]).unwrap();
    crate::pki::issue_client(&pki, "test-client", &bundle).unwrap();

    let server_config = server_config_from(&pki);
    let client_cfg = client_config_from(&pki, &bundle);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let conn = rustls::ServerConnection::new(server_config).unwrap();
        let mut tls_stream = rustls::StreamOwned::new(conn, stream);
        let _ = tls_stream.conn.complete_io(&mut tls_stream.sock);
    });

    let mut tcp = TcpStream::connect(addr).unwrap();
    // Connecting by IP -- the certificate only carries a DNS SAN, so the
    // client itself must refuse to validate it.
    let mut conn = rustls::ClientConnection::new(client_cfg, loopback_server_name()).unwrap();
    let err = conn.complete_io(&mut tcp).expect_err(
        "connecting by IP against a DNS-only-SAN certificate must fail, not silently succeed",
    );
    assert!(
        err.to_string()
            .to_lowercase()
            .contains("not valid for name"),
        "{err}"
    );

    server.join().unwrap();
    std::fs::remove_dir_all(&pki).ok();
    std::fs::remove_dir_all(&bundle).ok();
}

/// `--insecure-no-tls` is for loopback debugging only -- combined with a
/// non-loopback `--bind`, `build_transport` must refuse it outright, even if
/// `--allow-remote` is also set (that flag governs the loopback/non-loopback
/// bind check, a separate, independent gate -- see `run`'s doc comment).
#[test]
fn insecure_no_tls_with_a_non_loopback_bind_is_refused() {
    let args = crate::cli::ServeArgs {
        bind: "0.0.0.0:7878".to_string(),
        threads: 0,
        allow_remote: true,
        ca: None,
        cert: None,
        key: None,
        allowlist: None,
        trust_any_client_cert: false,
        insecure_no_tls: true,
        compute_mode: crate::cli::ComputeMode::default(),
        enroll_bind: None,
        no_enroll: false,
        db: None,
        max_connections: crate::cli::DEFAULT_MAX_CONNECTIONS,
    };
    let bind_addr: SocketAddr = args.bind.parse().unwrap();
    let err = build_transport(&args, bind_addr).unwrap_err();
    assert!(err.contains("--insecure-no-tls"), "{err}");
}

/// The loopback counterpart to the refusal above -- `--insecure-no-tls`
/// combined with a loopback bind must succeed and produce `Transport::Insecure`.
#[test]
fn insecure_no_tls_with_a_loopback_bind_is_accepted() {
    let args = crate::cli::ServeArgs {
        bind: "127.0.0.1:7878".to_string(),
        threads: 0,
        allow_remote: false,
        ca: None,
        cert: None,
        key: None,
        allowlist: None,
        trust_any_client_cert: false,
        insecure_no_tls: true,
        compute_mode: crate::cli::ComputeMode::default(),
        enroll_bind: None,
        no_enroll: false,
        db: None,
        max_connections: crate::cli::DEFAULT_MAX_CONNECTIONS,
    };
    let bind_addr: SocketAddr = args.bind.parse().unwrap();
    assert!(matches!(
        build_transport(&args, bind_addr).unwrap(),
        Transport::Insecure
    ));
}
