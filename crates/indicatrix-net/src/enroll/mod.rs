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
    net::TcpStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use zeroize::{Zeroize, Zeroizing};

/// One request on the enrollment wire protocol.
///
/// Deliberately a *different* message schema than [`crate::messages::ClientMessage`]
/// (the render protocol), not a variant of it, so bytes can never accidentally decode as
/// the wrong protocol if a wire got crossed.
#[derive(Debug, Serialize, Deserialize)]
pub enum EnrollRequest {
    /// Mint a new token for a viewer labeled `name` once claimed. Only honored from a
    /// loopback peer.
    Issue { name: String },
    /// Attempt to claim a pending enrollment with this secret.
    Claim { secret: [u8; token::SECRET_LEN] },
}

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

/// Splits `addr` (`host:port`) into its host part, for building a TLS [`ServerName`].
/// Splits on the last `:` so a bracketed IPv6 literal's own colons don't confuse it (full
/// validation still happens in `ServerName::try_from`). `None` if `addr` has no `:`.
fn host_of(addr: &str) -> Option<&str> {
    addr.rsplit_once(':').map(|(host, _)| host)
}

/// A freshly claimed client-certificate bundle, still in memory.
///
/// The same three PEM blocks `indicatrix-worker cert issue-client`/`cert claim` write to
/// `ca.pem`/`client.pem`/`client.key`. Writing to disk (with the private key's
/// permissions restricted -- see [`crate::tls::write_private_key_pem`]) is the caller's
/// job, since where the bundle belongs is caller-specific.
#[derive(Debug)]
pub struct ClaimedBundle {
    pub ca_pem: String,
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

    let host = host_of(addr)
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

    let tcp = TcpStream::connect(addr).map_err(|e| ClaimError::Connect {
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

    let response: EnrollResponse = crate::messages::read_message(&mut stream)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_of_splits_on_the_last_colon() {
        assert_eq!(host_of("127.0.0.1:7879"), Some("127.0.0.1"));
        assert_eq!(host_of("worker.lan:7879"), Some("worker.lan"));
    }

    #[test]
    fn host_of_returns_none_for_a_missing_port() {
        assert_eq!(host_of("worker.lan"), None);
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
