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

mod key_file;

pub use key_file::write_private_key_pem;

/// A SHA-256 client-certificate fingerprint, as stored in an [`Allowlist`].
pub type Fingerprint = [u8; 32];

/// The host part of a `host:port` address, as `rustls::pki_types::ServerName` accepts it.
///
/// Everything before the last `:`, with the square brackets of an IPv6 literal
/// (`[::1]:7879`) removed; `None` if `addr` has no `:`. The one place this rule lives, so every dialer (enrollment, join, desktop viewer)
/// handles bracketed IPv6 the same way. Full validation still happens in
/// `ServerName::try_from`.
#[must_use]
pub fn host_for_server_name(addr: &str) -> Option<&str> {
    let (host, _port) = addr.rsplit_once(':')?;
    Some(
        host.strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(host),
    )
}

/// Everything that can go wrong loading certs/keys from disk or building a `rustls`
/// config from them.
///
/// Carries the offending path (and, for a `rustls` failure, the inner error's own
/// `Display` text) rather than collapsing to a generic "handshake failed", so an expired
/// certificate, a clock-skew rejection, and a CA mismatch stay distinguishable.
#[derive(Debug)]
pub enum TlsError {
    /// `path` couldn't be opened or read.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// `path` was read but contained no PEM-encoded certificate.
    NoCertificates { path: PathBuf },
    /// `path` was read but contained no recognizable private key block.
    NoPrivateKey { path: PathBuf },
    /// `rustls` itself refused a certificate, key, or handshake (expired, clock skew,
    /// wrong CA, no matching SAN, or a malformed config) -- its own `Display` names which.
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

    /// Whether `fp` is in the allowlist.
    #[must_use]
    pub fn contains(&self, fp: &Fingerprint) -> bool {
        self.fingerprints.contains(fp)
    }

    /// Number of fingerprints in the allowlist.
    #[must_use]
    pub fn len(&self) -> usize {
        self.fingerprints.len()
    }

    /// Whether the allowlist holds no fingerprints.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fingerprints.is_empty()
    }
}

/// Replaces `\r`, `\n`, and `#` in `label` with `_`.
///
/// So it can never inject a second allowlist line (`\r`/`\n`) or terminate the comment
/// early / start a bogus one (`#`) when written into [`append_to_allowlist`]'s
/// `"{fingerprint}  # {label}"` line. `label` ultimately comes from an operator-supplied
/// `--name`/enrollment `name` (loopback-only, `cert issue-client`/`cert issue-token`'s
/// CLI or the enrollment listener's `Issue` request) rather than an untrusted remote
/// peer, so this is robustness against a typo or a copy-pasted stray newline, not a
/// hardened attacker boundary -- but [`Allowlist::load`] treats any malformed line as a
/// hard parse error (fail-closed), so a single bad label could otherwise corrupt every
/// OTHER entry's availability, not just its own.
///
/// `pub`, not just used by [`append_to_allowlist`]: `apps/indicatrix-worker`'s
/// `EnrollRegistry::issue` applies this same sanitization to the enrollment `name`
/// before it is ever minted into a certificate's Subject Common Name, rather than
/// re-implementing an equivalent scrub that could drift out of sync with this one.
#[must_use]
pub fn sanitize_allowlist_label(label: &str) -> String {
    label.replace(['\r', '\n', '#'], "_")
}

/// Serializes every [`append_to_allowlist`] call within this process against every
/// other -- see that function's doc comment. Does not protect against a SECOND
/// process appending to the same file at the same time; enrollment is expected to run
/// from one `serve` process per allowlist file.
static ALLOWLIST_APPEND_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Appends one `fingerprint  # label` line to the allowlist file at `path`, creating
/// the file (and its parent directory) if it doesn't exist yet.
///
/// Used by `indicatrix-worker cert issue-client` to enroll a freshly issued certificate
/// automatically; removing trust later is still the manual one-line file edit above.
/// `label` is sanitized via [`sanitize_allowlist_label`] before being written.
///
/// # Corruption this guards against
///
/// Three separate hazards, all closed here:
/// 1. **No trailing newline on the existing file's last line.** A hand-edited allowlist
///    that doesn't end in `\n` would otherwise glue the new fingerprint onto the
///    previous line's comment (that entry silently stops parsing as a bare
///    fingerprint) or directly onto a bare fingerprint (128 hex characters in a row,
///    which [`Allowlist::load`] rejects as [`TlsError::MalformedAllowlistLine`] --
///    failing every client, not just the new one, since `load` treats one bad line as a
///    hard parse error). Detected by reading the file's last byte first and prefixing a
///    `\n` onto the new line when it's missing (or the file is empty/new).
/// 2. **Multiple `write_all` calls from one appender.** The pre-fix code used `writeln!`
///    on a `File`, whose blanket `io::Write` impl of `write_fmt` performs one
///    `write_all` per piece of the format string (the fingerprint, the literal
///    `"  # "`, the label, the newline) rather than one for the whole line --
///    interleave-able by another thread's write between any of those pieces even within
///    ONE process. Fixed by building the complete line in a `String` first and writing
///    it with exactly one `write_all` call.
/// 3. **Two concurrent claims in DIFFERENT threads of this same process.** Even a
///    single `write_all` per call doesn't prevent two such calls from interleaving with
///    each other (two `O_APPEND` writers can still race at the OS level on some
///    platforms, and nothing serialized the "check trailing newline, then write" pair
///    above across threads). [`ALLOWLIST_APPEND_LOCK`] holds all of it -- the trailing-
///    newline read, the write, and the post-write re-validation below -- under one
///    process-wide mutex.
///
/// Re-validates with [`Allowlist::load`] after writing (holding the same lock) so a
/// write that somehow still produced a malformed file is caught immediately, as an
/// error from THIS call, rather than silently failing every future connection's
/// allowlist check.
///
/// # Errors
///
/// [`TlsError::Io`] if the parent directory can't be created, the existing file can't be
/// read, or the file can't be opened for appending/written to.
/// [`TlsError::MalformedAllowlistLine`] if the post-write re-validation finds the file
/// malformed despite this call's own write (see point 3 above for what that would mean).
pub fn append_to_allowlist(path: &Path, fp: &Fingerprint, label: &str) -> Result<(), TlsError> {
    use std::io::Write;

    let _guard = ALLOWLIST_APPEND_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| io_err(path, e))?;
    }

    // Whether the file already ends in a newline (or doesn't exist / is empty, in which
    // case no leading newline is needed either) -- see point 1 above.
    let needs_leading_newline = match std::fs::read(path) {
        Ok(bytes) => !bytes.is_empty() && bytes.last() != Some(&b'\n'),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(io_err(path, e)),
    };

    let mut line = String::new();
    if needs_leading_newline {
        line.push('\n');
    }
    line.push_str(&fingerprint_to_hex(fp));
    line.push_str("  # ");
    line.push_str(&sanitize_allowlist_label(label));
    line.push('\n');

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| io_err(path, e))?;
    file.write_all(line.as_bytes())
        .map_err(|e| io_err(path, e))?;
    drop(file);

    Allowlist::load(path).map(|_| ())
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
    fn host_for_server_name_strips_ipv6_brackets() {
        assert_eq!(host_for_server_name("127.0.0.1:7879"), Some("127.0.0.1"));
        assert_eq!(host_for_server_name("worker.lan:7879"), Some("worker.lan"));
        assert_eq!(host_for_server_name("[::1]:7879"), Some("::1"));
        assert_eq!(host_for_server_name("worker.lan"), None);
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
    fn sanitize_allowlist_label_replaces_line_breaking_and_comment_characters() {
        assert_eq!(sanitize_allowlist_label("laptop"), "laptop");
        assert_eq!(
            sanitize_allowlist_label("evil\r\ndeadbeef  # injected"),
            "evil__deadbeef  _ injected"
        );
        assert_eq!(sanitize_allowlist_label("a\nb"), "a_b");
        assert_eq!(sanitize_allowlist_label("a#b"), "a_b");
    }

    /// A label containing `\r\n` must not be able to inject a second,
    /// attacker-chosen allowlist line.
    #[test]
    fn append_to_allowlist_sanitizes_a_label_that_would_otherwise_inject_a_line() {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-net-allowlist-injection-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("allowlist.txt");
        let fp: Fingerprint = std::array::from_fn(|i| i as u8);
        let evil_fp: Fingerprint = std::array::from_fn(|i| (i as u8).wrapping_add(1));
        let label = format!(
            "laptop\r\n{}  # not really trusted",
            fingerprint_to_hex(&evil_fp)
        );

        append_to_allowlist(&path, &fp, &label).unwrap();

        // Loads clean (no MalformedAllowlistLine) and contains only the ONE fingerprint
        // actually passed to append_to_allowlist -- the embedded fingerprint-shaped text
        // inside the label never becomes its own trusted entry.
        let list = Allowlist::load(&path).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list.contains(&fp));
        assert!(!list.contains(&evil_fp));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Appending to a hand-edited allowlist whose last line has no trailing
    /// newline must not glue the new fingerprint onto the previous line -- both entries
    /// must parse as their own, separate lines.
    #[test]
    fn append_to_allowlist_prefixes_a_newline_when_the_file_lacks_a_trailing_one() {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-net-allowlist-no-trailing-newline-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("allowlist.txt");
        let existing: Fingerprint = std::array::from_fn(|i| i as u8);
        // Deliberately no trailing `\n` -- exactly the hand-edited shape described above.
        std::fs::write(
            &path,
            format!("{}  # existing", fingerprint_to_hex(&existing)),
        )
        .unwrap();

        let new_fp: Fingerprint = std::array::from_fn(|i| (i as u8).wrapping_add(1));
        append_to_allowlist(&path, &new_fp, "new-client").unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            contents.lines().count(),
            2,
            "the new entry must be its own line, not glued onto the previous one: {contents:?}"
        );

        let list = Allowlist::load(&path).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&existing));
        assert!(list.contains(&new_fp));

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
}
