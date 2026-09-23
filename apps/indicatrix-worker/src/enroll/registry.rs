//! The pending-entry registry: mint, hash, constant-time compare, TTL sweep, cap -- see
//! `crate::enroll`'s module doc comment for the full security rationale.

use crate::pki;
use indicatrix_net::{tls::Fingerprint, token};
use std::{
    fmt,
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use super::{MAX_PENDING, TOKEN_TTL_SECS};

/// Everything that can go wrong minting and registering a new pending enrollment (see
/// [`EnrollRegistry::issue`]).
///
/// A typed enum rather than a bare `String` -- the one caller that needs the text
/// ([`super::connection::handle_enroll_connection`]'s `Issue` arm, which sends it over
/// the wire as [`indicatrix_net::enroll::EnrollResponse::IssueRefused`]'s `reason`) gets
/// it via [`Display`](fmt::Display), same text as before; a caller that wants to
/// distinguish failure kinds instead of pattern-matching substrings out of a message can
/// match on the variant.
#[derive(Debug)]
pub enum EnrollIssueError {
    /// Minting the client-certificate bundle failed -- see
    /// [`pki::issue_client_in_memory`] and [`pki::PkiError`].
    Pki(pki::PkiError),
    /// The system CSPRNG ([`random_bytes`]) failed to produce randomness -- extremely
    /// rare.
    Csprng,
    /// [`MAX_PENDING`] enrollments are already pending.
    TooManyPending,
}

impl fmt::Display for EnrollIssueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pki(e) => write!(f, "{e}"),
            Self::Csprng => write!(f, "system CSPRNG failed to produce randomness"),
            Self::TooManyPending => write!(
                f,
                "{MAX_PENDING} enrollments are already pending -- claim or wait for one to expire before issuing \
                 another"
            ),
        }
    }
}

impl std::error::Error for EnrollIssueError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pki(e) => Some(e),
            Self::Csprng | Self::TooManyPending => None,
        }
    }
}

impl From<pki::PkiError> for EnrollIssueError {
    fn from(e: pki::PkiError) -> Self {
        Self::Pki(e)
    }
}

// The enrollment wire protocol lives in `indicatrix_net::enroll`, shared with
// `apps/indicatrix-cut`'s claiming client; this module imports it unchanged.

/// A freshly minted client-certificate bundle held in memory for one pending enrollment.
/// See the module doc comment on why `client_key_pem` is wrapped in [`Zeroizing`].
pub(super) struct EnrollBundle {
    pub(super) ca_pem: String,
    pub(super) client_cert_pem: String,
    pub(super) client_key_pem: Zeroizing<String>,
    pub(super) client_fingerprint: Fingerprint,
}

impl From<pki::InMemoryClientBundle> for EnrollBundle {
    fn from(b: pki::InMemoryClientBundle) -> Self {
        Self {
            ca_pem: b.ca_pem,
            client_cert_pem: b.client_cert_pem,
            client_key_pem: Zeroizing::new(b.client_key_pem),
            client_fingerprint: b.client_fingerprint,
        }
    }
}

pub(super) struct PendingEnrollment {
    /// `SHA-256(secret)` -- never the secret itself. See the module doc comment's
    /// point 2.
    secret_hash: Zeroizing<[u8; 32]>,
    expires_at: Instant,
    name: String,
    bundle: EnrollBundle,
}

/// The result of a successful [`EnrollRegistry::claim`]: everything
/// [`handle_enroll_connection`] needs to both reply to the claiming client and append to
/// `allowlist.txt`.
pub(super) struct ClaimedEnrollment {
    pub(super) name: String,
    pub(super) bundle: EnrollBundle,
}

/// All enrollments issued by this `serve` process that haven't yet been claimed or
/// expired.
///
/// Lives for exactly the lifetime of one `serve` invocation -- see the module doc
/// comment on why a restart dropping everything here is intended, not a bug.
#[derive(Default)]
pub struct EnrollRegistry {
    pub(super) pending: Mutex<Vec<PendingEnrollment>>,
}

impl EnrollRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mints a new client-certificate bundle for `name` and registers it as pending,
    /// with a fresh 256-bit CSPRNG secret (the same CSPRNG this workspace's TLS/PKI
    /// stack already uses internally). Returns the encoded token and its TTL.
    ///
    /// # Errors
    ///
    /// See [`EnrollIssueError`]'s variants: too many enrollments already pending,
    /// certificate minting failed, or the CSPRNG failed (extremely rare).
    pub fn issue(&self, pki_dir: &Path, name: &str) -> Result<(String, u64), EnrollIssueError> {
        self.issue_with_ttl(pki_dir, name, Duration::from_secs(TOKEN_TTL_SECS))
    }

    /// The TTL-parameterized implementation behind [`Self::issue`] -- lets expiry tests
    /// use a real, short TTL instead of waiting out the full [`TOKEN_TTL_SECS`] (180s).
    /// Not part of the public API: every real caller is fixed at [`TOKEN_TTL_SECS`].
    ///
    /// `name` is sanitized via [`indicatrix_net::tls::sanitize_allowlist_label`]
    /// before it is used for anything -- both the certificate's Subject
    /// Common Name below and, later, the allowlist line
    /// [`super::connection::handle_enroll_connection`]'s `Claim` arm appends via
    /// [`indicatrix_net::tls::append_to_allowlist`] -- so a `\r`/`\n`/`#` in an
    /// operator-supplied `--name` can never inject a stray allowlist line or corrupt a
    /// later entry's own comment. Sanitizing once, here, keeps the certificate and the
    /// eventual allowlist entry naming the SAME (sanitized) string.
    pub(super) fn issue_with_ttl(
        &self,
        pki_dir: &Path,
        name: &str,
        ttl: Duration,
    ) -> Result<(String, u64), EnrollIssueError> {
        let name = indicatrix_net::tls::sanitize_allowlist_label(name);
        let bundle = pki::issue_client_in_memory(pki_dir, &name)?;

        let mut secret = [0u8; token::SECRET_LEN];
        random_bytes(&mut secret)?;
        let secret_hash = sha256(&secret);
        let encoded = token::encode(&secret, &bundle.ca_fingerprint);
        secret.zeroize_local();

        let mut pending = self.pending.lock().unwrap();
        if pending.len() >= MAX_PENDING {
            return Err(EnrollIssueError::TooManyPending);
        }
        pending.push(PendingEnrollment {
            secret_hash: Zeroizing::new(secret_hash),
            expires_at: Instant::now() + ttl,
            name,
            bundle: EnrollBundle::from(bundle),
        });
        drop(pending);

        Ok((encoded, ttl.as_secs()))
    }

    /// Attempts to claim the pending enrollment whose secret hashes to `secret`'s hash.
    ///
    /// First sweeps and drops every entry already past `expires_at`. Then checks the
    /// candidate's SHA-256 against every *remaining* entry using
    /// [`subtle::ConstantTimeEq::ct_eq`], never short-circuiting on the first match, so
    /// total comparison work doesn't leak which entry (if any) matched via timing. On a
    /// match, that entry is removed before being returned -- single use.
    ///
    /// Returns `None` for "expired", "wrong secret", and "no such token" alike -- the
    /// caller collapses all of these to the same [`EnrollResponse::ClaimFailed`].
    pub(super) fn claim(&self, secret: &[u8; token::SECRET_LEN]) -> Option<ClaimedEnrollment> {
        let candidate_hash = sha256(secret);
        let now = Instant::now();

        let mut pending = self.pending.lock().unwrap();
        pending.retain(|p| p.expires_at > now); // expired entries dropped (and zeroized) here

        let mut matched_index = None;
        for (i, p) in pending.iter().enumerate() {
            let is_match: bool = candidate_hash.ct_eq(&*p.secret_hash).into();
            if is_match {
                matched_index = Some(i);
                // No `break`: every remaining entry is still compared (timing safety).
            }
        }

        let entry = pending.remove(matched_index?);
        drop(pending);
        Some(ClaimedEnrollment {
            name: entry.name,
            bundle: entry.bundle,
        })
    }
}

/// Fills `dest` with cryptographically secure random bytes via
/// [`ring::rand::SystemRandom`] -- `ring` is already this workspace's `rustls`/`rcgen`
/// crypto provider, reusing the CSPRNG already trusted for every key pair and TLS nonce.
///
/// # Errors
///
/// [`EnrollIssueError::Csprng`] in the (extremely rare, effectively "the OS RNG is
/// unavailable") case `ring` itself reports failure.
fn random_bytes(dest: &mut [u8]) -> Result<(), EnrollIssueError> {
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new()
        .fill(dest)
        .map_err(|_| EnrollIssueError::Csprng)
}

fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).into()
}

/// Small local extension so a stack-allocated `[u8; N]` secret can be explicitly
/// zeroized in place without promoting it to a heap-allocated `Zeroizing<Vec<u8>>` just
/// for that one call.
pub(super) trait ZeroizeLocal {
    fn zeroize_local(&mut self);
}

impl<const N: usize> ZeroizeLocal for [u8; N] {
    fn zeroize_local(&mut self) {
        use zeroize::Zeroize;
        self.zeroize();
    }
}
