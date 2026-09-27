//! Token-based enrollment.
//!
//! Replaces "copy the certificate bundle to the client machine by hand" with a one-time,
//! time-limited token the operator reads out (or pastes) to the person enrolling a new
//! viewer -- or, on the coordinator's WORKER enrollment listener, a new joining worker.
//!
//! # Two listeners, one per role
//!
//! `serve` runs one enrollment listener per client role: the VIEWER one next to the
//! viewer port (default 7879) and the WORKER one next to the worker port (default
//! 7881). Each has its own [`EnrollRegistry`] (so a token only ever claims on the
//! listener that issued it), mints certificates of its own role only (a worker's
//! Common Name is `worker:<name>`, see `crate::pki::role`) and appends a claimed
//! fingerprint to its own role's allowlist. No wire change: `cert issue-token --role
//! worker` sends its `Issue` name as `worker:<name>`, and each listener refuses an
//! `Issue` for the other role (a viewer listener refuses a `worker:` name, a worker
//! listener refuses a name without it) rather than silently minting the wrong kind.
//!
//! # Why this needs its own listener, not the render port
//!
//! The render listener (`crate::serve`) requires mutual TLS: every connection must
//! already present a certificate signed by this worker's CA. An enrolling client has no
//! such certificate yet, so it structurally cannot use that listener. Enrollment gets
//! its own listener and TLS config that never requires a client certificate, whose
//! connection handler ([`handle_enroll_connection`]) never calls into
//! [`crate::stream_emit`] or [`crate::serve::handle_connection_with_gpu`] -- a claim
//! connection cannot reach `RenderRequest` handling because the code path doesn't exist
//! on this listener, not merely because it's rejected at runtime.
//!
//! # The token
//!
//! See `indicatrix_net::token`'s doc comment for the wire encoding. Three security
//! properties, all enforced here:
//!
//! 1. **The secret is a fresh 256-bit CSPRNG value, never the certificate's
//!    fingerprint** -- the fingerprint is public (it crosses the wire on every TLS
//!    handshake and is what `allowlist.txt` stores), so a bearer secret must never
//!    double as it.
//! 2. **The server stores only [`sha2::Sha256`] of the secret**
//!    ([`PendingEnrollment::secret_hash`]), compared in [`EnrollRegistry::claim`] via
//!    [`subtle::ConstantTimeEq`], not `==`.
//! 3. **The token commits to the server's identity**: it carries the worker's CA
//!    fingerprint alongside the secret, and `indicatrix_net::enroll::claim` verifies the
//!    enrollment listener's certificate chains to that exact CA *before* sending the
//!    secret (`PinnedCaVerifier`) -- otherwise a MITM could impersonate the worker and
//!    walk away with a validly issued client certificate.
//!
//! # Lifecycle
//!
//! - Single use: [`EnrollRegistry::claim`] removes the entry before returning it.
//! - [`TOKEN_TTL_SECS`] (180s), checked server-side against [`Instant::now`] on every
//!   claim -- never a client-side courtesy.
//! - The issued bundle lives only in [`EnrollRegistry`]'s in-memory `Vec`
//!   ([`crate::pki::issue_client_in_memory`]), never written to disk by this process; it
//!   reaches disk only on the claiming machine via `crate::enroll_client::claim`.
//! - The claimed fingerprint is appended to `allowlist.txt` only inside
//!   [`handle_enroll_connection`]'s `Claim` arm, strictly after
//!   [`EnrollRegistry::claim`] returns a match, never at issue time.
//! - On claim, expiry, or registry drop, pending key material is explicitly zeroized:
//!   [`PendingEnrollment`]/[`EnrollBundle`] hold sensitive fields as
//!   [`zeroize::Zeroizing`], so `Drop` overwrites the bytes before the allocator frees
//!   them. Does not protect against the process being killed abruptly (`Drop` never
//!   runs).
//! - A `serve` restart starts a brand new, empty [`EnrollRegistry`] -- fail-closed and
//!   intentional; an operator re-issues a token.
//!
//! # Where issuing happens
//!
//! Issuing and claiming must happen inside the *same* running `serve` process, since the
//! bundle lives only in memory from the moment it's minted. `cert issue-token`
//! (`crate::enroll_client::run_issue_token`) is a small TLS client that connects to this
//! listener using the operator's own `ca.pem` for normal server verification.
//!
//! Issuing is further restricted to loopback peers only, checked against the actual
//! accepted `TcpStream::peer_addr()` (the same listener also accepts non-loopback claim
//! connections under `--allow-remote`). This mirrors `serve`'s own loopback-bind trust:
//! local access already lets an operator read `ca.key` and mint certificates directly,
//! so no further in-band credential is layered on top of a loopback source the OS
//! itself guarantees.

use crate::pki;
use indicatrix_net::messages::PeerRole;
use std::{
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};

mod connection;
mod registry;
#[cfg(test)]
mod tests;

pub use registry::{EnrollIssueError, EnrollRegistry};

/// How long an issued token remains claimable. Fixed, not operator-configurable -- the
/// TTL is enforced server-side, never a courtesy either side can override.
pub const TOKEN_TTL_SECS: u64 = 180;

/// Caps how many enrollments can be pending at once, purely to bound memory against a
/// runaway or misbehaving admin caller -- the `Issue` path is already loopback-only, so
/// this is a sanity bound, not a defense against a remote attacker.
const MAX_PENDING: usize = 64;

/// Builds the enrollment listener's TLS server config: TLS 1.3 only, presenting
/// `[server_cert, ca_cert]` as the chain -- deliberately including the CA certificate
/// so a claiming client (which has no CA file of its own) receives the bytes it needs
/// to hash and compare against its token's fingerprint. See
/// `crate::enroll_client::PinnedCaVerifier`.
///
/// Requires no client certificate: an enrolling client has none yet, and `cert
/// issue-token` authenticates by connecting from loopback instead -- a weaker TLS
/// posture than the render listener's, hence a separate config on a separate port.
///
/// # Errors
///
/// A human-readable message if the server certificate/key/CA can't be loaded from
/// `cert_path`/`key_path`/`ca_path`, or don't form a valid TLS server configuration.
fn build_enroll_server_config(
    ca_path: &Path,
    cert_path: &Path,
    key_path: &Path,
) -> Result<Arc<rustls::ServerConfig>, String> {
    let mut chain = indicatrix_net::tls::load_certs(cert_path)
        .map_err(|e| format!("--cert {}: {e}", cert_path.display()))?;
    let ca_certs = indicatrix_net::tls::load_certs(ca_path)
        .map_err(|e| format!("--ca {}: {e}", ca_path.display()))?;
    chain.extend(ca_certs);
    let key = indicatrix_net::tls::load_private_key(key_path)
        .map_err(|e| format!("--key {}: {e}", key_path.display()))?;

    let config = rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|e| format!("failed to build the enrollment TLS config: {e}"))?;
    Ok(Arc::new(config))
}

/// Configuration [`spawn_enroll_listener`] needs, gathered once at `serve` startup from
/// the same `--ca`/`--cert`/`--key` args the listeners validated.
pub struct EnrollConfig {
    /// Where to listen (already derived and loopback-checked by `crate::serve`).
    pub bind_addr: SocketAddr,
    /// The CA's directory (`ca.pem`, `ca.key`).
    pub pki_dir: PathBuf,
    /// `None` when `--trust-any-client-cert` is set: no allowlist to append to (any
    /// CA-signed client of the right role is already trusted), so a successful claim
    /// skips that step.
    pub allowlist_path: Option<PathBuf>,
    /// TLS with the server certificate and CA chain, no client auth.
    pub tls_config: Arc<rustls::ServerConfig>,
    /// The matching listener's own connection cap, shared here so this listener's
    /// unauthenticated connections are bounded too -- see
    /// [`crate::serve::ConnectionLimiter`]'s doc comment.
    pub limiter: crate::serve::ConnectionLimiter,
    /// Which client role this listener enrolls (see the module doc comment).
    pub role: PeerRole,
}

impl EnrollConfig {
    /// Builds an [`EnrollConfig`] for `role` from the same `serve --ca/--cert/--key`
    /// paths, the role's resolved allowlist path and the matching listener's
    /// [`crate::serve::ConnectionLimiter`] to share.
    ///
    /// # Errors
    ///
    /// A human-readable message if the TLS config can't be built (see
    /// [`build_enroll_server_config`]).
    pub fn build(
        bind_addr: SocketAddr,
        ca_path: &Path,
        cert_path: &Path,
        key_path: &Path,
        allowlist_path: Option<PathBuf>,
        limiter: crate::serve::ConnectionLimiter,
        role: PeerRole,
    ) -> Result<Self, String> {
        let tls_config = build_enroll_server_config(ca_path, cert_path, key_path)?;
        Ok(Self {
            bind_addr,
            pki_dir: pki::role::pki_dir_of(ca_path),
            allowlist_path,
            tls_config,
            limiter,
            role,
        })
    }
}

/// Binds `config.bind_addr` and spawns a background thread accepting enrollment
/// connections forever.
///
/// Mirrors `crate::serve::run`'s accept-loop shape (one `thread::spawn` per connection,
/// `catch_unwind`-wrapped). Returns as soon as the bind succeeds, so a bad
/// `--enroll-bind` is reported synchronously at `serve` startup.
///
/// Returns the listener's actual bound address (useful when `config.bind_addr`'s port
/// was `0`; this crate's tests use that for an ephemeral port).
///
/// # Errors
///
/// A human-readable message if the bind itself fails (e.g. the port is already in use).
pub fn spawn_enroll_listener(config: EnrollConfig) -> Result<SocketAddr, String> {
    let listener = TcpListener::bind(config.bind_addr).map_err(|e| {
        format!(
            "failed to bind enrollment listener {}: {e}",
            config.bind_addr
        )
    })?;
    let bound_addr = listener
        .local_addr()
        .map_err(|e| format!("enrollment listener bound but local_addr() failed: {e}"))?;
    let role_flag = match config.role {
        PeerRole::Viewer => "",
        PeerRole::Worker => " --role worker",
    };
    tracing::info!(
        "indicatrix-worker serve: {} enrollment listener on {bound_addr} (token TTL {TOKEN_TTL_SECS}s; \
         `indicatrix-worker cert issue-token{role_flag} --admin-addr {bound_addr}` to mint one)",
        pki::role::role_name(config.role)
    );

    let registry = Arc::new(EnrollRegistry::for_role(config.role));
    let tls_config = config.tls_config;
    let pki_dir = config.pki_dir;
    let allowlist_path = config.allowlist_path;
    let limiter = config.limiter;

    thread::spawn(move || {
        for incoming in listener.incoming() {
            match incoming {
                Ok(stream) => {
                    let peer = stream.peer_addr().ok();
                    let tls_config = Arc::clone(&tls_config);
                    let registry = Arc::clone(&registry);
                    let pki_dir = pki_dir.clone();
                    let allowlist_path = allowlist_path.clone();
                    let limiter = limiter.clone();
                    thread::spawn(move || {
                        let Some(tls_stream) =
                            connection::accept_enroll_tls(stream, &tls_config, peer)
                        else {
                            return;
                        };
                        // The slot is acquired only AFTER the TLS handshake succeeds
                        // (mirroring the render listener's own connection-acquisition
                        // order), so a bare connect that never completes TLS never holds
                        // a slot; this listener's own `HANDSHAKE_TIMEOUT` (applied inside
                        // `accept_enroll_tls`) already bounds how long that can take.
                        let Ok(_slot) = limiter.try_acquire() else {
                            tracing::warn!(
                                "enrollment connection {peer:?}: refusing -- already at --max-connections capacity \
                                 (shared with the matching viewer/worker listener)"
                            );
                            return;
                        };
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            connection::handle_enroll_connection(
                                tls_stream,
                                &registry,
                                &pki_dir,
                                allowlist_path.as_deref(),
                                peer,
                            )
                        }));
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => {
                                tracing::warn!(
                                    "enrollment connection {peer:?} ended with an error: {e}"
                                );
                            }
                            Err(_) => {
                                tracing::warn!(
                                    "enrollment connection {peer:?} panicked and was dropped"
                                );
                            }
                        }
                    });
                }
                Err(e) => tracing::warn!("enrollment accept error: {e}"),
            }
        }
    });

    Ok(bound_addr)
}
