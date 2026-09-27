use super::*;

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "indicatrix-worker-pki-test-{label}-{}-{}",
        std::process::id(),
        fastrand_seed()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// No rand dependency: a nanosecond timestamp keeps parallel test temp dirs unique.
fn fastrand_seed() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

#[test]
fn init_writes_a_ca_and_refuses_to_overwrite_it() {
    let dir = temp_dir("init");
    init(&dir).unwrap();
    assert!(dir.join(CA_CERT_FILE).exists());
    assert!(dir.join(CA_KEY_FILE).exists());

    let err = init(&dir).unwrap_err().to_string();
    assert!(err.contains("already contains a CA"), "{err}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn issue_server_requires_at_least_one_san() {
    let dir = temp_dir("issue-server-no-san");
    init(&dir).unwrap();

    let err = issue_server(&dir, &[], &[]).unwrap_err().to_string();
    assert!(err.contains("--host or --ip"), "{err}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn issue_server_writes_a_cert_with_the_requested_sans() {
    let dir = temp_dir("issue-server");
    init(&dir).unwrap();
    issue_server(
        &dir,
        &["worker.lan".to_string()],
        &["10.0.0.5".parse().unwrap()],
    )
    .unwrap();

    let cert_pem = std::fs::read_to_string(dir.join(SERVER_CERT_FILE)).unwrap();
    assert!(cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(dir.join(SERVER_KEY_FILE).exists());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn issue_client_writes_a_bundle_and_updates_the_allowlist() {
    let dir = temp_dir("issue-client-dir");
    let out = temp_dir("issue-client-out");
    init(&dir).unwrap();
    issue_client(&dir, "laptop", &out).unwrap();

    assert!(out.join(CA_CERT_FILE).exists());
    assert!(out.join(CLIENT_CERT_FILE).exists());
    assert!(out.join(CLIENT_KEY_FILE).exists());

    let allowlist_text = std::fs::read_to_string(dir.join(VIEWER_ALLOWLIST_FILE)).unwrap();
    assert!(allowlist_text.contains("# laptop"), "{allowlist_text}");

    let client_der = tls::load_certs(&out.join(CLIENT_CERT_FILE)).unwrap();
    let fp = tls::fingerprint(&client_der[0]);
    let allowlist = tls::Allowlist::load(&dir.join(VIEWER_ALLOWLIST_FILE)).unwrap();
    assert!(allowlist.contains(&fp));

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&out).ok();
}

#[test]
fn issue_client_rejects_an_empty_name() {
    let dir = temp_dir("issue-client-empty-name");
    init(&dir).unwrap();
    let err = issue_client(&dir, "  ", &dir.join("bundle"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("--name"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn issue_server_and_issue_client_fail_clearly_without_an_existing_ca() {
    let dir = temp_dir("no-ca");
    let err = issue_server(&dir, &["worker.lan".to_string()], &[])
        .unwrap_err()
        .to_string();
    assert!(err.contains("cert init"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn default_allowlist_paths_sit_beside_the_ca_file() {
    let ca = Path::new("C:/pki-that-does-not-exist/ca.pem");
    assert_eq!(
        default_viewer_allowlist_path(ca),
        PathBuf::from("C:/pki-that-does-not-exist").join(VIEWER_ALLOWLIST_FILE)
    );
    assert_eq!(
        default_worker_allowlist_path(ca),
        PathBuf::from("C:/pki-that-does-not-exist").join(WORKER_ALLOWLIST_FILE)
    );
}

/// `--role worker` prefixes the Common Name, lands in the WORKER allowlist only, and a
/// viewer name that would look like a worker's is refused.
#[test]
fn issue_client_with_the_worker_role_uses_the_prefix_and_the_worker_allowlist() {
    let dir = temp_dir("issue-worker-dir");
    let out = temp_dir("issue-worker-out");
    init(&dir).unwrap();
    issue_client_with_role(&dir, "gpu-box", &out, PeerRole::Worker).unwrap();

    let der = tls::load_certs(&out.join(CLIENT_CERT_FILE)).unwrap();
    assert_eq!(
        role::subject_common_name(&der[0]).unwrap().as_deref(),
        Some("worker:gpu-box")
    );
    assert_eq!(role_of_certificate(&der[0]).unwrap(), PeerRole::Worker);
    let workers = tls::Allowlist::load(&dir.join(WORKER_ALLOWLIST_FILE)).unwrap();
    assert!(workers.contains(&tls::fingerprint(&der[0])));
    assert!(!dir.join(VIEWER_ALLOWLIST_FILE).exists());

    let err = issue_client(&dir, "worker:sneaky", &out)
        .unwrap_err()
        .to_string();
    assert!(err.contains("--role worker"), "{err}");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&out).ok();
}
