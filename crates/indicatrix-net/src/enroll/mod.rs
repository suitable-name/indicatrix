//! The enrollment wire protocol, and the claiming half of token-based enrollment.
//!
//! Lives here rather than in `apps/indicatrix-worker` because both it (`cert claim`) and
//! `apps/indicatrix-cut` need to encode/decode these messages and run the same
//! CA-fingerprint verification ([`PinnedCaVerifier`]) before ever sending the secret -- a
//! viewer must not depend on the server binary, and a security policy must not exist in
//! two copies that can drift apart. The server-side registry (minting, hashing,
//! constant-time compare, TTL sweep) has no viewer-side counterpart and stays in
//! `apps/indicatrix-worker/src/enroll.rs`, which imports [`EnrollRequest`]/[`EnrollResponse`]
//! from here. [`crate::token`] moved here for the same sharing reason. See
//! `apps/indicatrix-worker/src/enroll.rs`'s module doc comment for the full enrollment
//! design: the token's security properties, its lifecycle, and why issuing is
//! loopback-only.

use crate::token;
use rustls::{
    DigitallySignedStruct, Error as RustlsError, RootCertStore, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use zeroize::{Zeroize, Zeroizing};

/// Connect timeout and per-read/per-write socket timeout of [`claim`]: an enrollment
/// exchange is a few small messages, so a listener that stalls longer is treated as dead.
const CLAIM_IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Cap on the reply [`claim`] will read. A `Claimed` reply is a few PEM blocks (a few
/// KiB); nothing legitimate approaches this.
const MAX_CLAIM_REPLY_LEN: u32 = 64 * 1024;

/// Length in bytes of the operator secret that authorises [`EnrollRequest::IssueAuthorized`].
pub const OPERATOR_SECRET_LEN: usize = 32;

/// File name of the operator secret inside the PKI directory.
pub const OPERATOR_SECRET_FILE: &str = "issue.secret";

/// Path of the operator secret file inside `pki_dir`.
#[must_use]
pub fn operator_secret_path(pki_dir: &Path) -> PathBuf {
    pki_dir.join(OPERATOR_SECRET_FILE)
}

/// Reads the operator secret (hex, one line) from `<pki_dir>/issue.secret`.
///
/// `serve` creates the file on first start with owner-only permissions; `cert
/// issue-token` reads it and presents it in [`EnrollRequest::IssueAuthorized`]. The returned secret
/// is zeroized on drop.
///
/// # Errors
///
/// A human-readable message if the file can't be read or isn't 64 hex characters.
pub fn read_operator_secret(
    pki_dir: &Path,
) -> Result<Zeroizing<[u8; OPERATOR_SECRET_LEN]>, String> {
    let path = operator_secret_path(pki_dir);
    let text = Zeroizing::new(
        std::fs::read_to_string(&path)
            .map_err(|e| format!("could not read the operator secret {}: {e}", path.display()))?,
    );
    crate::tls::fingerprint_from_hex(text.trim())
        .map(Zeroizing::new)
        .ok_or_else(|| {
            format!(
                "{} is not a valid operator secret (expected {} hex characters)",
                path.display(),
                OPERATOR_SECRET_LEN * 2
            )
        })
}

/// Writes `secret` as hex to `<pki_dir>/issue.secret`, readable by the current user only
/// (the same protection as `ca.key`, see [`crate::tls::write_private_key_pem`]).
///
/// # Errors
///
/// A human-readable message if the file can't be created or its permissions restricted.
pub fn write_operator_secret(
    pki_dir: &Path,
    secret: &[u8; OPERATOR_SECRET_LEN],
) -> Result<(), String> {
    let mut text = Zeroizing::new(crate::tls::fingerprint_to_hex(secret));
    text.push('\n');
    crate::tls::write_private_key_pem(&operator_secret_path(pki_dir), &text)
}

/// One request on the enrollment wire protocol.
///
/// Deliberately a *different* message schema than [`crate::messages::ClientMessage`]
/// (the render protocol), not a variant of it, so bytes can never accidentally decode as
/// the wrong protocol if a wire got crossed.
#[derive(Debug, Serialize, Deserialize)]
pub enum EnrollRequest {
    /// The original unauthenticated issue request. A listener refuses it with a reply
    /// telling the operator to update `cert issue-token`, because issuing is no longer
    /// authorised by "the peer is loopback" alone (see [`Self::IssueAuthorized`]); the
    /// variant stays so an older client gets that message instead of a decode error.
    Issue { name: String },
    /// Attempt to claim a pending enrollment with this secret.
    Claim { secret: [u8; token::SECRET_LEN] },
    /// Mint a new token for a viewer (or, on the worker listener, a worker) labeled `name`
    /// once claimed. Honored only from a loopback peer that also presents the operator
    /// secret (`<pki_dir>/issue.secret`, see [`read_operator_secret`]), so a local user or
    /// a port forward without read access to the PKI directory cannot mint certificates.
    ///
    /// Protocol note: appended after `Claim`, so the existing variants keep their wire
    /// indices. The enrollment protocol has no version field; an `IssueAuthorized` sent to
    /// a listener that predates it fails to decode there.
    IssueAuthorized {
        name: String,
        operator_secret: [u8; OPERATOR_SECRET_LEN],
    },
}

/// The reply to one [`EnrollRequest`] on the enrollment wire protocol.
#[derive(Debug, Serialize, Deserialize)]
pub enum EnrollResponse {
    Issued {
        /// The full `GW1-...` token string -- see [`crate::token`].
        token: String,
        expires_in_secs: u64,
    },
    /// Carries a reason -- loopback-only / registry-full, not attacker-facing. Contrast
    /// with [`EnrollResponse::ClaimFailed`], whose reason is always the same generic
    /// string.
    IssueRefused { reason: String },
    Claimed {
        ca_pem: String,
        client_cert_pem: String,
        client_key_pem: String,
    },
    /// Deliberately the same message for "wrong secret", "expired", "already claimed",
    /// and "internal error" -- distinguishing them would hand a remote prober an
    /// enumeration oracle. The real reason is still logged server-side
    /// (`RUST_LOG=debug`). [`ClaimError::Refused`] preserves this same ambiguity.
    ClaimFailed,
}

/// Verifies the enrollment listener's presented certificate chain against a CA whose
/// SHA-256 fingerprint matches [`Self::expected_ca_fingerprint`] -- never against a CA
/// file on disk, since a claiming client has none yet.
///
/// This is certificate PINNING, a legitimate pattern `rustls` exposes
/// `ClientConfig::dangerous()` specifically to support -- distinct from disabling
/// verification. All cryptographic work (chain construction, signature verification) is
/// still delegated to `rustls`/`rustls-webpki`/`ring`; this type only selects which
/// presented certificate counts as "the CA", using the token's fingerprint.
#[derive(Debug)]
struct PinnedCaVerifier {
    expected_ca_fingerprint: crate::tls::Fingerprint,
    provider: Arc<rustls::crypto::CryptoProvider>,
    /// Set when [`Self::verify_server_cert`] fails because no presented certificate
    /// matched the pinned fingerprint -- the attacker-in-the-middle-shaped failure,
    /// which [`claim`] reports distinctly from an ordinary handshake or connection
    /// failure. A flag, not a richer `rustls::Error` variant, because
    /// `verify_server_cert` must return a real `rustls::Error` with no generic
    /// `Other(Box<dyn Error>)` constructor; `Arc<AtomicBool>` because
    /// `ServerCertVerifier` requires `Send + Sync`.
    ca_mismatch: Arc<AtomicBool>,
}

impl PinnedCaVerifier {
    fn new(expected_ca_fingerprint: crate::tls::Fingerprint, ca_mismatch: Arc<AtomicBool>) -> Self {
        Self {
            expected_ca_fingerprint,
            provider: Arc::new(rustls::crypto::ring::default_provider()),
            ca_mismatch,
        }
    }
}

impl ServerCertVerifier for PinnedCaVerifier {
    /// Finds, among the certificates the server presented (`intermediates`), the one
    /// whose SHA-256 fingerprint matches the token's. If none matches, verification
    /// fails outright -- no fallback to "trust it anyway" -- and [`Self::ca_mismatch`] is
    /// set so [`claim`] can report this distinctly. Once found, a one-off
    /// `RootCertStore` containing just that certificate is built and the rest of
    /// chain/signature verification is delegated to `rustls`'s own `WebPkiServerVerifier`
    /// against it -- see this type's doc comment for why that keeps this "pinning", not
    /// "skipping verification".
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        let Some(pinned_ca) = intermediates
            .iter()
            .find(|c| crate::tls::fingerprint(c) == self.expected_ca_fingerprint)
        else {
            self.ca_mismatch.store(true, Ordering::SeqCst);
            return Err(RustlsError::General(
                "the server did not present a certificate chain containing the CA this \
                 enrollment token commits to -- refusing to trust it (this is either the \
                 wrong worker, or an attacker in the middle)"
                    .to_string(),
            ));
        };

        let mut roots = RootCertStore::empty();
        roots.add(pinned_ca.clone()).map_err(|e| {
            RustlsError::General(format!(
                "the pinned CA certificate is not usable as a trust anchor: {e}"
            ))
        })?;
        let verifier = rustls::client::WebPkiServerVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|e| {
                RustlsError::General(format!("failed to build a verifier for the pinned CA: {e}"))
            })?;

        verifier.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Err(RustlsError::General(
            "TLS 1.2 is not supported by the enrollment listener".to_string(),
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// A freshly claimed client-certificate bundle, still in memory.
///
/// The same three PEM blocks `indicatrix-worker cert issue-client`/`cert claim` write to
/// `ca.pem`/`client.pem`/`client.key`. Writing to disk (with the private key's
/// permissions restricted -- see [`crate::tls::write_private_key_pem`]) is the caller's
/// job, since where the bundle belongs is caller-specific.
#[derive(Debug)]
pub struct ClaimedBundle {
    /// PEM-encoded certificate authority that signs the server.
    pub ca_pem: String,
    /// PEM-encoded client certificate issued to the enrolling peer.
    pub client_cert_pem: String,
    /// Wrapped in [`Zeroizing`] since this is the client's private key, plaintext,
    /// having just crossed the wire. Overwritten as soon as the caller is done with it
    /// rather than lingering in a freed heap allocation.
    pub client_key_pem: Zeroizing<String>,
}

/// Everything that can go wrong redeeming an enrollment token.
///
/// One type so a caller (a CLI's error formatting, or a GUI's toast) has a single match
/// to render instead of parsing a generic string. Every variant maps to genuinely
/// different advice for the user -- see [`fmt::Display`]'s impl below.
#[derive(Debug)]
pub enum ClaimError {
    /// `token` isn't a well-formed `GW1-...` string -- see [`token::TokenError`].
    InvalidToken(token::TokenError),
    /// `addr` isn't `host:port`, or its host half isn't valid for TLS server-name
    /// verification.
    InvalidAddr(String),
    /// The TCP connection itself could not be established -- most likely a wrong or
    /// unreachable address, not a security concern. Kept distinct from
    /// [`Self::CaFingerprintMismatch`] so a wrong address never gets alarming security
    /// wording, and a genuine impersonation never reads like "check your network".
    Connect {
        addr: String,
        source: std::io::Error,
    },
    /// The TLS handshake failed for a reason OTHER than the pinned CA fingerprint not
    /// matching (e.g. the pinned certificate itself failed ordinary `webpki`
    /// validation). Not conflated with [`Self::CaFingerprintMismatch`]: this is not
    /// evidence of impersonation.
    Handshake {
        addr: String,
        source: std::io::Error,
    },
    /// The security-relevant handshake failure: the server's certificate chain does not
    /// contain one matching the CA fingerprint this token commits to -- either the wrong
    /// worker or an attacker in the middle (see [`PinnedCaVerifier`]). Its own variant so
    /// callers must word this differently from an ordinary connection problem.
    CaFingerprintMismatch { addr: String },
    /// A transport-level failure sending the claim request or reading the response,
    /// after a successful, correctly-pinned handshake.
    Protocol(String),
    /// The server reported [`EnrollResponse::ClaimFailed`]: wrong, already-claimed, or
    /// expired (tokens live 180s) -- deliberately not distinguished (see that variant's
    /// doc comment), so this crate cannot honestly report more than the server revealed.
    Refused,
    /// The server responded with something other than `Claimed`/`ClaimFailed` to a
    /// `Claim` request -- a protocol/version mismatch rather than any of the above.
    UnexpectedResponse,
}

impl fmt::Display for ClaimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidToken(e) => write!(f, "not a valid enrollment token: {e}"),
            // Already complete, self-describing messages -- pass through verbatim.
            Self::InvalidAddr(msg) | Self::Protocol(msg) => write!(f, "{msg}"),
            Self::Connect { addr, source } => write!(f, "could not connect to {addr}: {source}"),
            Self::Handshake { addr, source } => {
                write!(f, "TLS handshake with {addr} failed: {source}")
            }
            Self::CaFingerprintMismatch { addr } => write!(
                f,
                "TLS handshake with {addr} failed: the server did not present a certificate chain \
                 containing the CA this enrollment token commits to -- refusing to trust it (this is \
                 either the wrong worker, or an attacker in the middle)"
            ),
            Self::Refused => write!(
                f,
                "claim failed: the enrollment token was invalid, expired, or already used"
            ),
            Self::UnexpectedResponse => {
                write!(f, "unexpected response from the enrollment listener")
            }
        }
    }
}

impl std::error::Error for ClaimError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidToken(e) => Some(e),
            Self::Connect { source, .. } | Self::Handshake { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Redeems `token` against the enrollment listener at `addr` (`host:port`).
///
/// Decodes `token` (see [`crate::token`]), connects to `addr`, and verifies the listener
/// against the CA fingerprint the token carries -- via [`PinnedCaVerifier`] -- *before*
/// ever sending the secret. Zeroizes its own copy of the secret immediately after
/// sending it, and again when the decoded token is dropped. On success, returns the
/// bundle in memory; writing it to disk is the caller's job.
///
/// Never logs `token`, the decoded secret, or the claimed private key, on success or
/// failure -- no `tracing`/`println!` call anywhere in this function or in
/// [`PinnedCaVerifier`].
///
/// # Errors
///
/// See [`ClaimError`]'s variants -- one per distinguishable failure, so a caller can say
/// something different for "you typo'd the address" than for "something is
/// impersonating the worker".
pub fn claim(token: &str, addr: &str) -> Result<ClaimedBundle, ClaimError> {
    let decoded = token::decode(token).map_err(ClaimError::InvalidToken)?;

    let host = crate::tls::host_for_server_name(addr)
        .ok_or_else(|| ClaimError::InvalidAddr(format!("{addr:?} must be host:port")))?;
    let server_name = ServerName::try_from(host.to_string()).map_err(|e| {
        ClaimError::InvalidAddr(format!("{addr:?}: invalid host for TLS verification: {e}"))
    })?;

    let ca_mismatch = Arc::new(AtomicBool::new(false));
    let verifier = Arc::new(PinnedCaVerifier::new(
        decoded.ca_fingerprint,
        Arc::clone(&ca_mismatch),
    ));
    let client_config =
        rustls::ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();

    let tcp = connect_with_timeout(addr).map_err(|e| ClaimError::Connect {
        addr: addr.to_string(),
        source: e,
    })?;
    let conn =
        rustls::ClientConnection::new(Arc::new(client_config), server_name).map_err(|e| {
            ClaimError::Handshake {
                addr: addr.to_string(),
                source: std::io::Error::other(e),
            }
        })?;
    let mut stream = rustls::StreamOwned::new(conn, tcp);
    if let Err(e) = stream.conn.complete_io(&mut stream.sock) {
        return Err(if ca_mismatch.load(Ordering::SeqCst) {
            ClaimError::CaFingerprintMismatch {
                addr: addr.to_string(),
            }
        } else {
            ClaimError::Handshake {
                addr: addr.to_string(),
                source: e,
            }
        });
    }

    // Send the secret, then immediately zeroize both the request's own copy and the
    // token's -- it shouldn't linger in memory longer than necessary on either side.
    let mut request = EnrollRequest::Claim {
        secret: *decoded.secret,
    };
    let write_result = crate::messages::write_message(&mut stream, &request);
    if let EnrollRequest::Claim { secret } = &mut request {
        secret.zeroize();
    }
    drop(decoded); // zeroizes its own `Zeroizing<[u8; 32]>` secret on drop
    write_result.map_err(|e| ClaimError::Protocol(e.to_string()))?;

    let response: EnrollResponse =
        crate::messages::read_message_bounded(&mut stream, MAX_CLAIM_REPLY_LEN)
            .map_err(|e| ClaimError::Protocol(e.to_string()))?;

    match response {
        EnrollResponse::Claimed {
            ca_pem,
            client_cert_pem,
            client_key_pem,
        } => Ok(ClaimedBundle {
            ca_pem,
            client_cert_pem,
            client_key_pem: Zeroizing::new(client_key_pem),
        }),
        EnrollResponse::ClaimFailed => Err(ClaimError::Refused),
        EnrollResponse::Issued { .. } | EnrollResponse::IssueRefused { .. } => {
            Err(ClaimError::UnexpectedResponse)
        }
    }
}

/// Connects to `addr`, trying each resolved address with [`CLAIM_IO_TIMEOUT`], and arms
/// the same timeout as the socket's read and write deadline.
fn connect_with_timeout(addr: &str) -> std::io::Result<TcpStream> {
    let mut last_error = None;
    for candidate in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&candidate, CLAIM_IO_TIMEOUT) {
            Ok(stream) => {
                stream.set_read_timeout(Some(CLAIM_IO_TIMEOUT))?;
                stream.set_write_timeout(Some(CLAIM_IO_TIMEOUT))?;
                return Ok(stream);
            }
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "the address resolved to nothing",
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_secret_round_trips_through_its_file() {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-net-operator-secret-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let secret: [u8; OPERATOR_SECRET_LEN] = std::array::from_fn(|i| u8::try_from(i).unwrap());
        write_operator_secret(&dir, &secret).unwrap();
        assert_eq!(*read_operator_secret(&dir).unwrap(), secret);

        std::fs::write(operator_secret_path(&dir), "not hex\n").unwrap();
        assert!(read_operator_secret(&dir).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn claim_rejects_an_invalid_token_before_ever_touching_the_network() {
        // No listener at this address: a connection-error result would mean `claim`
        // tried to connect before validating the token.
        let err = claim("not-a-token", "127.0.0.1:1").unwrap_err();
        assert!(matches!(err, ClaimError::InvalidToken(_)), "{err}");
    }

    #[test]
    fn claim_rejects_an_address_with_no_port() {
        let secret = [0u8; token::SECRET_LEN];
        let fp: crate::tls::Fingerprint = std::array::from_fn(|i| i as u8);
        let token = token::encode(&secret, &fp);
        let err = claim(&token, "worker-with-no-port").unwrap_err();
        assert!(matches!(err, ClaimError::InvalidAddr(_)), "{err}");
    }

    #[test]
    fn claim_error_display_distinguishes_ca_mismatch_from_an_ordinary_handshake_failure() {
        let mismatch = ClaimError::CaFingerprintMismatch {
            addr: "worker.lan:7879".to_string(),
        }
        .to_string();
        let handshake = ClaimError::Handshake {
            addr: "worker.lan:7879".to_string(),
            source: std::io::Error::other("boom"),
        }
        .to_string();
        assert_ne!(mismatch, handshake);
        assert!(mismatch.contains("attacker in the middle"), "{mismatch}");
        assert!(!handshake.contains("attacker in the middle"), "{handshake}");
    }
}
