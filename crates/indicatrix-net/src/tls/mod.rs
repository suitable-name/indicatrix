//! Mutual-TLS configuration for `indicatrix-net`'s transport.
//!
//! Loading certs/keys from PEM, building `rustls` configs against a private CA, and the
//! client-certificate fingerprint allowlist that stands in for revocation. Pinned to
//! TLS 1.3 -- `rustls`'s `tls12` feature is left disabled entirely, not merely
//! unconfigured at runtime.
//!
//! **Socket-free**, consistent with the rest of this crate: every function here takes
//! bytes/paths in and returns a `rustls::ServerConfig`/`rustls::ClientConfig`/fingerprint
//! out, none of it touching a `TcpStream`. Wrapping an actual socket in
//! `rustls::StreamOwned` happens at the call site. Certificate *generation* (the CA and
//! leaf certs) is deliberately NOT here either -- that's `apps/indicatrix-worker`'s `pki`
//! module; this module is what loads an already-issued bundle, shared by worker and
//! viewer alike.
//!
//! # Trust model
//!
//! [`server_config`] and [`client_config`] both verify the peer's certificate chain
//! against a private CA using `rustls`'s own built-in `webpki`-based verifiers, never a
//! hand-written `ServerCertVerifier`/`ClientCertVerifier` impl (that's how validation
//! quietly gets disabled in production -- skip verification for local testing via
//! `indicatrix-worker serve --insecure-no-tls` instead, which is loopback-only and warns
//! on every accepted connection).
//!
//! [`server_config`] also REQUIRES a client certificate -- no anonymous-client path --
//! since that mutual check is what replaces a password in this design.
//!
//! CA-chain validity is necessary but not sufficient: anyone with a CA-signed
//! certificate can complete the handshake. The actual authorization decision is the
//! [`Allowlist`] of client-certificate SHA-256 fingerprints, checked by the caller after
//! the handshake against `rustls::ServerConnection::peer_certificates()`. Revoking a
//! client means deleting its line from the allowlist file; there is no CRL or OCSP.

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fmt,
    io::BufReader,
    path::{Path, PathBuf},
    sync::Arc,
};

/// A SHA-256 client-certificate fingerprint, as stored in an [`Allowlist`].
pub type Fingerprint = [u8; 32];

/// Everything that can go wrong loading certs/keys from disk or building a `rustls`
/// config from them.
///
/// Carries the offending path (and, for a `rustls` failure, the inner error's own
/// `Display` text) rather than collapsing to a generic "handshake failed", so an expired
/// certificate, a clock-skew rejection, and a CA mismatch stay distinguishable.
#[derive(Debug)]
pub enum TlsError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    NoCertificates {
        path: PathBuf,
    },
    NoPrivateKey {
        path: PathBuf,
    },
    Rustls(rustls::Error),
    /// A malformed line in an [`Allowlist`] file: not blank, not a `#` comment, and not
    /// 64 hex characters.
    MalformedAllowlistLine {
        path: PathBuf,
        line_number: usize,
        line: String,
    },
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::NoCertificates { path } => {
                write!(f, "{}: no PEM-encoded certificate found", path.display())
            }
            Self::NoPrivateKey { path } => {
                write!(f, "{}: no PEM-encoded private key found", path.display())
            }
            // e.g. "invalid peer certificate: Expired" / "NotValidYet" / "UnknownIssuer".
            Self::Rustls(e) => write!(f, "TLS error: {e}"),
            Self::MalformedAllowlistLine {
                path,
                line_number,
                line,
            } => {
                write!(
                    f,
                    "{}:{line_number}: not a 64-character hex fingerprint: {line:?}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for TlsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Rustls(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rustls::Error> for TlsError {
    fn from(e: rustls::Error) -> Self {
        Self::Rustls(e)
    }
}

fn io_err(path: &Path, source: std::io::Error) -> TlsError {
    TlsError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Loads every PEM-encoded certificate in `path`, in file order (a leaf certificate
/// followed by any intermediates, for a full chain).
///
/// # Errors
///
/// [`TlsError::Io`] if `path` can't be opened or read, [`TlsError::NoCertificates`] if
/// it contains no PEM `CERTIFICATE` block at all.
pub fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    let file = std::fs::File::open(path).map_err(|e| io_err(path, e))?;
    let mut reader = BufReader::new(file);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<_, _>>()
        .map_err(|e| io_err(path, e))?;
    if certs.is_empty() {
        return Err(TlsError::NoCertificates {
            path: path.to_path_buf(),
        });
    }
    Ok(certs)
}

/// Loads the first PEM-encoded private key found in `path` (PKCS#8, PKCS#1, or SEC1).
///
/// # Errors
///
/// [`TlsError::Io`] if `path` can't be opened or read, [`TlsError::NoPrivateKey`] if it
/// contains no recognizable private key block.
pub fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>, TlsError> {
    let file = std::fs::File::open(path).map_err(|e| io_err(path, e))?;
    let mut reader = BufReader::new(file);
    rustls_pemfile::private_key(&mut reader)
        .map_err(|e| io_err(path, e))?
        .ok_or_else(|| TlsError::NoPrivateKey {
            path: path.to_path_buf(),
        })
}

/// Loads a CA bundle from `path` into a `rustls::RootCertStore`.
///
/// Typically one self-signed certificate -- the trust anchor both [`server_config`]
/// (for verifying client certs) and [`client_config`] (for verifying the server cert)
/// are built with.
///
/// # Errors
///
/// Whatever [`load_certs`] returns, plus [`TlsError::Rustls`] if a loaded certificate
/// isn't a well-formed DER certificate `rustls` can add as a trust anchor.
pub fn load_ca(path: &Path) -> Result<rustls::RootCertStore, TlsError> {
    let certs = load_certs(path)?;
    let mut store = rustls::RootCertStore::empty();
    for cert in certs {
        store.add(cert)?;
    }
    Ok(store)
}

/// Builds a server-side mutual-TLS config, TLS 1.3 only.
///
/// Presents `cert_chain`/`key` to connecting peers, and REQUIRES each peer to present a
/// certificate signed by `ca` -- there is no anonymous-client path.
///
/// This is CA-chain validation only ("was this client certificate signed by a CA I
/// trust"), not "which specific signed client should be trusted" -- pair it with an
/// [`Allowlist`] check against the peer's certificate fingerprint after the handshake.
///
/// # Errors
///
/// [`TlsError::Rustls`] if `ca` is empty (nothing would ever verify) or `cert_chain`/
/// `key` don't form a valid, matching certificate and private key.
pub fn server_config(
    ca: rustls::RootCertStore,
    cert_chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<rustls::ServerConfig>, TlsError> {
    let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(ca))
        .build()
        .map_err(|e| TlsError::Rustls(rustls::Error::General(e.to_string())))?;

    let config = rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_client_cert_verifier(verifier)
        .with_single_cert(cert_chain, key)?;

    Ok(Arc::new(config))
}

/// Builds a client-side mutual-TLS config, TLS 1.3 only.
///
/// Presents `cert_chain`/`key` as its own client certificate, and verifies the server's
/// certificate against `ca` using `rustls`'s standard `webpki` server verifier (installed
/// by `with_root_certificates`, not a custom `ServerCertVerifier`).
///
/// # Errors
///
/// [`TlsError::Rustls`] if `cert_chain`/`key` don't form a valid, matching certificate
/// and private key.
pub fn client_config(
    ca: rustls::RootCertStore,
    cert_chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<rustls::ClientConfig>, TlsError> {
    let config = rustls::ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_root_certificates(ca)
        .with_client_auth_cert(cert_chain, key)?;

    Ok(Arc::new(config))
}

/// The SHA-256 fingerprint of a DER-encoded certificate -- the identity an
/// [`Allowlist`] entry names.
///
/// Computed over the whole DER certificate (not just the public key), so reissuing a
/// certificate for the same key produces a different fingerprint and needs a fresh
/// allowlist entry.
#[must_use]
pub fn fingerprint(cert: &CertificateDer<'_>) -> Fingerprint {
    let mut hasher = Sha256::new();
    hasher.update(cert.as_ref());
    hasher.finalize().into()
}

/// Formats a fingerprint as 64 lowercase hex characters -- the [`Allowlist`] file's
/// on-disk format.
#[must_use]
pub fn fingerprint_to_hex(fp: &Fingerprint) -> String {
    use std::fmt::Write as _;
    fp.iter()
        .fold(String::with_capacity(fp.len() * 2), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Parses a 64-character hex string (whitespace-trimmed) back into a [`Fingerprint`].
/// Returns `None` for anything else -- wrong length, non-hex characters.
#[must_use]
pub fn fingerprint_from_hex(s: &str) -> Option<Fingerprint> {
    let s = s.trim();
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte_slot) in out.iter_mut().enumerate() {
        *byte_slot = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// An allowlist of trusted client-certificate SHA-256 fingerprints -- the whole
/// revocation story for this design (no CRL, no OCSP).
///
/// # File format
///
/// One entry per line: a 64-character hex fingerprint, optionally followed by
/// whitespace and a `#`-prefixed comment (conventionally the `--name` it was issued
/// under, e.g. `... # laptop`). Blank lines and `#`-comment lines are ignored. Revoking a
/// client is deleting its line -- meant to be hand-editable, not just machine-written.
///
/// A line that isn't blank, a comment, or 64 hex characters is a load error rather than
/// silently skipped: a typo silently dropping an entry would fail open (nobody notices a
/// client is no longer allowed), the wrong direction for an allowlist to fail in.
#[derive(Debug, Default, Clone)]
pub struct Allowlist {
    fingerprints: HashSet<Fingerprint>,
}

impl Allowlist {
    /// Loads an allowlist from `path`. See the struct docs for the file format.
    ///
    /// # Errors
    ///
    /// [`TlsError::Io`] if `path` can't be opened or read, including if it doesn't
    /// exist -- fail-closed is deliberate; a caller wanting "no allowlist yet" to mean
    /// "trust nobody" should check [`Path::exists`] first.
    /// [`TlsError::MalformedAllowlistLine`] for the first line that isn't blank, a
    /// comment, or a valid fingerprint.
    pub fn load(path: &Path) -> Result<Self, TlsError> {
        let contents = std::fs::read_to_string(path).map_err(|e| io_err(path, e))?;
        let mut fingerprints = HashSet::new();
        for (i, raw_line) in contents.lines().enumerate() {
            let line = raw_line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let fp =
                fingerprint_from_hex(line).ok_or_else(|| TlsError::MalformedAllowlistLine {
                    path: path.to_path_buf(),
                    line_number: i + 1,
                    line: raw_line.to_string(),
                })?;
            fingerprints.insert(fp);
        }
        Ok(Self { fingerprints })
    }

    #[must_use]
    pub fn contains(&self, fp: &Fingerprint) -> bool {
        self.fingerprints.contains(fp)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.fingerprints.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fingerprints.is_empty()
    }
}

/// Appends one `fingerprint  # label` line to the allowlist file at `path`, creating
/// the file (and its parent directory) if it doesn't exist yet.
///
/// Used by `indicatrix-worker cert issue-client` to enroll a freshly issued certificate
/// automatically; removing trust later is still the manual one-line file edit above.
///
/// # Errors
///
/// [`TlsError::Io`] if the parent directory can't be created or the file can't be
/// opened for appending.
pub fn append_to_allowlist(path: &Path, fp: &Fingerprint, label: &str) -> Result<(), TlsError> {
    use std::io::Write;

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| io_err(path, e))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| io_err(path, e))?;
    writeln!(file, "{}  # {label}", fingerprint_to_hex(fp)).map_err(|e| io_err(path, e))?;
    Ok(())
}

/// Writes a PEM-encoded private key to `path` and immediately restricts its permissions.
///
/// `restrict_key_file` runs before this function returns successfully, not as a
/// separate pass after, so there's no window where the key file is observable
/// unprotected. If restricting permissions fails, the just-written file is deleted
/// rather than left behind unprotected.
///
/// Shared by `apps/indicatrix-worker` and `apps/indicatrix-cut` (via
/// [`crate::enroll::claim`]'s `client.key`). Returns a plain `String` rather than
/// [`TlsError`] since the failure modes here (directory creation, an external
/// `icacls`/`whoami` process) aren't TLS or certificate-parsing errors.
///
/// # Errors
///
/// A human-readable message if the parent directory can't be created, the file can't be
/// written, or its permissions can't be restricted (see `restrict_key_file`).
pub fn write_private_key_pem(path: &Path, pem: &str) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, pem).map_err(|e| format!("could not write {}: {e}", path.display()))?;

    if let Err(e) = restrict_key_file(path) {
        let _ = std::fs::remove_file(path);
        return Err(format!(
            "wrote {} but could not restrict its permissions ({e}) -- the file has been removed rather than left \
             behind world-readable; fix the underlying issue and retry",
            path.display()
        ));
    }
    Ok(())
}

/// Restricts `path` to the current user (plus `SYSTEM` and `Administrators`) via
/// `icacls` -- Windows has no `chmod`, so this needs an ACL instead.
#[cfg(windows)]
fn restrict_key_file(path: &Path) -> Result<(), String> {
    let user = current_user_account()?;
    let path_str = path.to_str().ok_or_else(|| {
        format!(
            "{}: path is not valid Unicode, icacls requires a printable path",
            path.display()
        )
    })?;

    let status = std::process::Command::new("icacls")
        .arg(path_str)
        // Drop inherited ACEs (parent dirs typically grant `Users` read access) before
        // granting anything back, so the end state is exactly the three grants below.
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{user}:F"))
        .arg("/grant:r")
        .arg("SYSTEM:F")
        // Administrators by well-known SID: the localized group name varies by display language.
        .arg("/grant:r")
        .arg("*S-1-5-32-544:F")
        // icacls's own success chatter isn't useful; the exit status is what's checked.
        .stdout(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("failed to run icacls: {e}"))?;

    if !status.success() {
        return Err(format!("icacls exited with {status}"));
    }
    Ok(())
}

#[cfg(windows)]
fn current_user_account() -> Result<String, String> {
    let output = std::process::Command::new("whoami")
        .output()
        .map_err(|e| format!("failed to run whoami: {e}"))?;
    if !output.status.success() {
        return Err(format!("whoami exited with {}", output.status));
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        return Err("whoami printed no output".to_string());
    }
    Ok(name)
}

/// Non-Windows fallback: `chmod 600`.
#[cfg(not(windows))]
fn restrict_key_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod 600 failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_hex_round_trips() {
        let fp: Fingerprint = std::array::from_fn(|i| i as u8);
        let hex = fingerprint_to_hex(&fp);
        assert_eq!(hex.len(), 64);
        assert_eq!(fingerprint_from_hex(&hex), Some(fp));
    }

    #[test]
    fn fingerprint_from_hex_rejects_wrong_length_and_non_hex() {
        assert_eq!(fingerprint_from_hex("abcd"), None);
        assert_eq!(fingerprint_from_hex(&"zz".repeat(32)), None);
    }

    #[test]
    fn allowlist_parses_comments_and_blank_lines() {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-net-allowlist-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("allowlist.txt");
        let fp: Fingerprint = std::array::from_fn(|i| i as u8);
        std::fs::write(
            &path,
            format!(
                "\n# a comment line\n{}  # laptop\n\n",
                fingerprint_to_hex(&fp)
            ),
        )
        .unwrap();

        let list = Allowlist::load(&path).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list.contains(&fp));

        let other: Fingerprint = std::array::from_fn(|i| (i as u8).wrapping_add(1));
        assert!(!list.contains(&other));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn allowlist_rejects_a_malformed_line() {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-net-allowlist-bad-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("allowlist.txt");
        std::fs::write(&path, "not-a-fingerprint\n").unwrap();

        let err = Allowlist::load(&path).unwrap_err();
        assert!(
            matches!(err, TlsError::MalformedAllowlistLine { .. }),
            "{err}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn append_to_allowlist_creates_file_and_parent_dir() {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-net-allowlist-append-test-{}",
            std::process::id()
        ));
        let path = dir.join("nested").join("allowlist.txt");
        let fp: Fingerprint = std::array::from_fn(|i| i as u8);

        append_to_allowlist(&path, &fp, "laptop").unwrap();
        let list = Allowlist::load(&path).unwrap();
        assert!(list.contains(&fp));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_private_key_pem_creates_parent_dirs_and_writes_the_exact_content() {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-net-write-key-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("nested").join("client.key");
        let pem = "-----BEGIN PRIVATE KEY-----\nfake\n-----END PRIVATE KEY-----\n";

        write_private_key_pem(&path, pem).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), pem);

        std::fs::remove_dir_all(&dir).ok();
    }
}
