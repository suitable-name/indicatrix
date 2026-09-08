//! The two CLI-facing halves of token-based enrollment that run OUTSIDE the `serve`
//! process.
//!
//! `cert issue-token` (run by the operator, against a running `serve`) and `cert claim`
//! (run on the machine being enrolled). See `crate::enroll`'s module doc comment for the
//! full design and lifecycle these two talk to.
//!
//! The security-critical pinned-CA TLS handshake and wire exchange live in
//! [`indicatrix_net::enroll::claim`] instead, shared with `apps/indicatrix-cut`.
//! [`claim`] below is a thin wrapper: calls that, turns its
//! [`indicatrix_net::enroll::ClaimError`] into this CLI's `--token`/`--addr`-flavored
//! message, and writes the returned bundle to `--out`.

use crate::{
    cli::{CertClaimArgs, CertIssueTokenArgs},
    pki,
};
use indicatrix_net::enroll::{ClaimError, EnrollRequest, EnrollResponse};
use rustls::{ClientConfig, ClientConnection, StreamOwned, pki_types::ServerName};
use std::{net::TcpStream, path::Path, sync::Arc};

/// Splits `addr` (`host:port`) into its host part, for building a TLS [`ServerName`].
/// Splits on the last `:` so a bracketed IPv6 literal's internal colons are tolerated
/// (further validated by `ServerName::try_from`).
///
/// # Errors
///
/// A human-readable message if `addr` has no `:` at all.
fn host_of<'a>(addr: &'a str, flag: &str) -> Result<&'a str, String> {
    addr.rsplit_once(':')
        .map(|(host, _)| host)
        .ok_or_else(|| format!("{flag} {addr:?} must be host:port"))
}

/// Turns a [`ClaimError`] into this CLI's own `--token`/`--addr`-flavored message.
/// `indicatrix_net::enroll::claim` knows nothing of CLI flag names (it's shared with
/// `apps/indicatrix-cut`), so that framing is added here.
fn claim_error_to_string(err: ClaimError, addr: &str) -> String {
    match err {
        ClaimError::InvalidToken(e) => format!("--token: {e}"),
        ClaimError::InvalidAddr(msg) => format!("--addr {msg}"),
        ClaimError::Connect { source, .. } => format!("could not connect to {addr}: {source}"),
        ClaimError::Handshake { source, .. } => format!(
            "TLS handshake with {addr} failed: {source} -- if this names the pinned CA, double-check the \
             token was transcribed correctly (see `indicatrix-worker cert claim --help`)",
        ),
        ClaimError::CaFingerprintMismatch { .. } => format!(
            "TLS handshake with {addr} failed: the server did not present a certificate chain containing the \
             CA this enrollment token commits to -- refusing to trust it (this is either the wrong worker, or \
             an attacker in the middle) -- if this names the pinned CA, double-check the token was transcribed \
             correctly (see `indicatrix-worker cert claim --help`)",
        ),
        ClaimError::Protocol(msg) => msg,
        ClaimError::Refused => {
            "claim failed: the enrollment token was invalid, expired, or already used".to_string()
        }
        ClaimError::UnexpectedResponse => {
            format!("unexpected response from the enrollment listener at {addr}")
        }
    }
}

/// `indicatrix-worker cert claim --token <token> --addr <host:port> --out <bundle-dir>`.
///
/// Redeems `args.token` against `args.addr` via [`indicatrix_net::enroll::claim`], which
/// verifies the enrollment listener against the CA fingerprint the token carries before
/// ever sending the token's secret, then writes the same three-file bundle (`ca.pem`,
/// `client.pem`, `client.key`) `cert issue-client` would have, ACL-restricting the key
/// file the same way ([`indicatrix_net::tls::write_private_key_pem`]).
///
/// # Errors
///
/// A human-readable message if the token doesn't decode, `args.addr` isn't `host:port`,
/// the connection or TLS handshake fails (including the pinned-CA verification failing),
/// the wire exchange fails, the server reports the claim failed (wrong/expired/
/// already-used token), or the bundle can't be written to `args.out`.
pub fn claim(args: &CertClaimArgs) -> Result<(), String> {
    let bundle = indicatrix_net::enroll::claim(&args.token, &args.addr)
        .map_err(|e| claim_error_to_string(e, &args.addr))?;

    std::fs::create_dir_all(&args.out)
        .map_err(|e| format!("could not create {}: {e}", args.out.display()))?;
    let out_ca_path = args.out.join(pki::CA_CERT_FILE);
    let out_cert_path = args.out.join(pki::CLIENT_CERT_FILE);
    let out_key_path = args.out.join(pki::CLIENT_KEY_FILE);
    std::fs::write(&out_ca_path, &bundle.ca_pem)
        .map_err(|e| format!("could not write {}: {e}", out_ca_path.display()))?;
    std::fs::write(&out_cert_path, &bundle.client_cert_pem)
        .map_err(|e| format!("could not write {}: {e}", out_cert_path.display()))?;
    indicatrix_net::tls::write_private_key_pem(&out_key_path, &bundle.client_key_pem)?;
    // `bundle.client_key_pem` is `Zeroizing<String>`, overwritten in place on drop.

    tracing::info!(
        "indicatrix-worker cert claim: wrote bundle to {} ({}, {}, {})",
        args.out.display(),
        pki::CA_CERT_FILE,
        pki::CLIENT_CERT_FILE,
        pki::CLIENT_KEY_FILE,
    );
    Ok(())
}

/// `indicatrix-worker cert issue-token --ca <ca.pem> --admin-addr <host:port> --name <label>`.
///
/// Connects to a running `serve` process's enrollment listener at `args.admin_addr`,
/// verifying it with ordinary (non-pinned) TLS against `args.ca` -- the operator already
/// has that file, so there's no bootstrap problem here the way there is for [`claim`].
/// Asks it to mint a token for `args.name` and prints the result.
///
/// The token is printed via `println!`, deliberately not `tracing::info!`: it is a
/// bearer secret, and a tracing-formatted line is exactly what ends up captured in a
/// log file or forwarded to an aggregator.
///
/// # Errors
///
/// A human-readable message if `args.ca` can't be loaded, `args.admin_addr` isn't
/// `host:port`, the connection or TLS handshake fails, the wire exchange fails, or the
/// server refuses to issue a token (not a loopback connection, or too many enrollments
/// already pending -- see `crate::enroll::EnrollRegistry::issue`).
pub fn run_issue_token(args: &CertIssueTokenArgs) -> Result<(), String> {
    let response = issue_token_over_tls(&args.ca, &args.admin_addr, &args.name)?;

    match_issue_response(response, &args.name)
}

/// The actual TLS connect-and-exchange behind [`run_issue_token`], factored out so this
/// crate's tests can drive a real loopback round trip and inspect the raw
/// [`EnrollResponse`] (`run_issue_token` only ever prints it).
///
/// # Errors
///
/// A human-readable message if `ca_path` can't be loaded, `admin_addr` isn't
/// `host:port`, or the connection, TLS handshake, or wire exchange fails.
fn issue_token_over_tls(
    ca_path: &Path,
    admin_addr: &str,
    name: &str,
) -> Result<EnrollResponse, String> {
    let ca = indicatrix_net::tls::load_ca(ca_path)
        .map_err(|e| format!("--ca {}: {e}", ca_path.display()))?;
    let client_config = ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_root_certificates(ca)
        .with_no_client_auth();

    let host = host_of(admin_addr, "--admin-addr")?;
    let server_name = ServerName::try_from(host.to_string()).map_err(|e| {
        format!("--admin-addr {admin_addr:?}: invalid host for TLS verification: {e}")
    })?;

    let tcp = TcpStream::connect(admin_addr)
        .map_err(|e| format!("could not connect to {admin_addr}: {e}"))?;
    let conn = ClientConnection::new(Arc::new(client_config), server_name)
        .map_err(|e| format!("failed to start a TLS session: {e}"))?;
    let mut stream = StreamOwned::new(conn, tcp);
    stream
        .conn
        .complete_io(&mut stream.sock)
        .map_err(|e| format!("TLS handshake with {admin_addr} failed: {e}"))?;

    let request = EnrollRequest::Issue {
        name: name.to_string(),
    };
    indicatrix_net::messages::write_message(&mut stream, &request).map_err(|e| e.to_string())?;
    indicatrix_net::messages::read_message(&mut stream).map_err(|e| e.to_string())
}

fn match_issue_response(response: EnrollResponse, name: &str) -> Result<(), String> {
    match response {
        EnrollResponse::Issued {
            token,
            expires_in_secs,
        } => {
            tracing::info!(
                "indicatrix-worker cert issue-token: issued a token for {name:?}, valid {expires_in_secs}s"
            );
            println!(
                "Enrollment token for {name:?} (valid {expires_in_secs}s), for one use only:\n"
            );
            println!("  {token}\n");
            println!(
                "Read or send this to whoever is enrolling -- it carries a one-time secret AND this worker's CA \
                 fingerprint, so `cert claim` can verify it's really talking to this worker before it ever sends \
                 anything back. It works once and expires in {expires_in_secs} seconds."
            );
            Ok(())
        }
        EnrollResponse::IssueRefused { reason } => Err(format!("token issue refused: {reason}")),
        other => Err(format!(
            "unexpected response from the enrollment listener: {other:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_of_splits_on_the_last_colon() {
        assert_eq!(host_of("127.0.0.1:7879", "--addr").unwrap(), "127.0.0.1");
        assert_eq!(host_of("worker.lan:7879", "--addr").unwrap(), "worker.lan");
    }

    #[test]
    fn host_of_rejects_a_missing_port() {
        let err = host_of("worker.lan", "--addr").unwrap_err();
        assert!(err.contains("--addr"), "{err}");
    }

    // ---- Real end-to-end round trips over a real loopback TLS connection ----------
    //
    // Unlike `crate::enroll`'s tests (in-memory duplex, never exercises TLS), these spin
    // up a real `crate::enroll::spawn_enroll_listener` on an ephemeral port and drive
    // `issue_token_over_tls`/`claim` against it, to prove the `PinnedCaVerifier`
    // handshake actually works, not just that it type-checks.

    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-worker-enroll-client-test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Sets up a real CA + server certificate and a real (ephemeral-port) enrollment
    /// listener. Returns the pki dir, the listener's bound address, and the allowlist
    /// path it was configured with.
    fn start_real_enroll_listener() -> (std::path::PathBuf, std::net::SocketAddr, std::path::PathBuf)
    {
        let dir = unique_temp_dir("listener");
        crate::pki::init(&dir).unwrap();
        crate::pki::issue_server(&dir, &[], &["127.0.0.1".parse().unwrap()]).unwrap();

        let ca_path = dir.join(crate::pki::CA_CERT_FILE);
        let cert_path = dir.join(crate::pki::SERVER_CERT_FILE);
        let key_path = dir.join(crate::pki::SERVER_KEY_FILE);
        let allowlist_path = dir.join(crate::pki::ALLOWLIST_FILE);

        let config = crate::enroll::EnrollConfig::build(
            "127.0.0.1:1".parse().unwrap(), // unused: overridden below
            Some("127.0.0.1:0"),            // ephemeral port
            false,
            &ca_path,
            &cert_path,
            &key_path,
            Some(allowlist_path.clone()),
        )
        .unwrap();
        let addr = crate::enroll::spawn_enroll_listener(config).unwrap();

        (dir, addr, allowlist_path)
    }

    #[test]
    fn issue_then_claim_end_to_end_over_a_real_tls_connection() {
        let (dir, addr, allowlist_path) = start_real_enroll_listener();
        let ca_path = dir.join(crate::pki::CA_CERT_FILE);

        assert!(
            !allowlist_path.exists(),
            "issuing must not touch the allowlist before any claim"
        );

        let response = issue_token_over_tls(&ca_path, &addr.to_string(), "real-viewer").unwrap();
        let EnrollResponse::Issued {
            token: token_str, ..
        } = response
        else {
            panic!("expected Issued, got {response:?}");
        };

        let out = unique_temp_dir("bundle");
        let claim_args = CertClaimArgs {
            token: token_str,
            addr: addr.to_string(),
            out: out.clone(),
        };
        claim(&claim_args).unwrap();

        assert!(out.join(pki::CA_CERT_FILE).exists());
        assert!(out.join(pki::CLIENT_CERT_FILE).exists());
        assert!(out.join(pki::CLIENT_KEY_FILE).exists());

        let allowlist = indicatrix_net::tls::Allowlist::load(&allowlist_path).unwrap();
        assert_eq!(
            allowlist.len(),
            1,
            "the claimed certificate's fingerprint, and only it"
        );

        // Single-use, over the real connection too.
        let err = claim(&claim_args).unwrap_err();
        assert!(err.contains("invalid"), "{err}");

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    #[test]
    fn claim_refuses_a_token_whose_ca_fingerprint_does_not_match_the_real_server() {
        let (dir, addr, allowlist_path) = start_real_enroll_listener();
        let ca_path = dir.join(crate::pki::CA_CERT_FILE);

        let response = issue_token_over_tls(&ca_path, &addr.to_string(), "real-viewer").unwrap();
        let EnrollResponse::Issued {
            token: token_str, ..
        } = response
        else {
            panic!("expected Issued, got {response:?}");
        };
        let real = indicatrix_net::token::decode(&token_str).unwrap();

        // Same secret, wrong CA fingerprint -- simulates an attacker or a transcription
        // error. `claim` must refuse to send the secret at all.
        let wrong_fingerprint: indicatrix_net::tls::Fingerprint =
            std::array::from_fn(|i| (i as u8).wrapping_add(1));
        assert_ne!(wrong_fingerprint, real.ca_fingerprint);
        let bad_token = indicatrix_net::token::encode(&real.secret, &wrong_fingerprint);

        let out = unique_temp_dir("bundle-wrong-ca");
        let claim_args = CertClaimArgs {
            token: bad_token,
            addr: addr.to_string(),
            out: out.clone(),
        };
        let err = claim(&claim_args).unwrap_err();
        assert!(
            err.to_lowercase().contains("handshake") || err.to_lowercase().contains("pinned"),
            "{err}"
        );

        // The real token still works afterward: the failed attempt never touched the registry.
        let claim_args_real = CertClaimArgs {
            token: token_str,
            addr: addr.to_string(),
            out: out.clone(),
        };
        claim(&claim_args_real).unwrap();
        let allowlist = indicatrix_net::tls::Allowlist::load(&allowlist_path).unwrap();
        assert_eq!(allowlist.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }
}
