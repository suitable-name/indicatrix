//! `cert init` / `cert issue-server` / `cert issue-client`: an in-process private CA
//! for `indicatrix-worker`'s mutual TLS.
//!
//! Certificate GENERATION only -- one-shot CLI operations that produce PEM files on
//! disk. Private-key writing (including Windows-ACL permissioning) and loading an
//! issued bundle back into `rustls` configs both live in [`indicatrix_net::tls`]
//! instead, since `apps/indicatrix-cut`'s token-redeem UI needs the same
//! restricted-permission write and both apps already depend on that crate.
//!
//! # Why a private CA rather than a public one
//!
//! The worker is addressed by IP on a LAN or private host, which no public CA will
//! issue for, and there is exactly one operator on both ends of the trust
//! relationship -- no third party's vouching is worth anything here.
//!
//! # Layout
//!
//! ```text
//! <pki-dir>/
//!   ca.pem          CA certificate (public -- ships to every worker and every viewer)
//!   ca.key          CA private key (sensitive -- ACL-restricted, never leaves this dir)
//!   server.pem       this worker's certificate (public)
//!   server.key       this worker's private key (sensitive -- ACL-restricted)
//!   allowlist.txt     trusted client-certificate fingerprints -- see `indicatrix_net::tls`
//!
//! <bundle-dir>/        (from `issue-client --out`, copied to the viewer machine)
//!   ca.pem
//!   client.pem
//!   client.key        (sensitive -- ACL-restricted)
//! ```
//!
//! `issue-server` and `issue-client` both re-derive the CA's signing identity from the
//! saved `ca.pem`/`ca.key` in `<pki-dir>` (via [`rcgen::Issuer::from_ca_cert_pem`]),
//! since each subcommand is its own process invocation with nothing else in memory.
//!
//! # Certificate lifetimes and why they're long
//!
//! CA: 10 years. Server/client leaf certs: 5 years. There is no CRL or OCSP --
//! revocation is the fingerprint [`Allowlist`](indicatrix_net::tls::Allowlist), edited
//! by hand -- so expiry is a backstop, not the primary revocation path. Long lifetimes
//! trade a weaker backstop for not re-enrolling every LAN machine every few months.
//!
//! Every `not_before` is backdated by [`NOT_BEFORE_SLACK_DAYS`] from issuance, so a
//! worker or viewer whose clock is a little behind doesn't reject a freshly issued
//! certificate as not-yet-valid.
//!
//! # Errors
//!
//! Every fallible function in this module returns [`PkiError`] rather than a bare
//! `String`: callers that only ever print the failure (`main.rs`) see the exact same
//! text as before via [`PkiError`]'s [`Display`](std::fmt::Display) impl, while a caller
//! that wants to distinguish failure kinds (rather than pattern-matching substrings out
//! of a message) can match on the enum instead -- `crate::enroll::registry::EnrollIssueError`
//! does exactly that, wrapping [`PkiError`] rather than flattening it to a `String`.

use indicatrix_net::tls;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use std::{
    fmt,
    net::IpAddr,
    path::{Path, PathBuf},
};
use time::{Duration, OffsetDateTime};

pub const CA_CERT_FILE: &str = "ca.pem";
pub const CA_KEY_FILE: &str = "ca.key";
pub const SERVER_CERT_FILE: &str = "server.pem";
pub const SERVER_KEY_FILE: &str = "server.key";
pub const CLIENT_CERT_FILE: &str = "client.pem";
pub const CLIENT_KEY_FILE: &str = "client.key";
/// Lives alongside the CA (`<pki-dir>/allowlist.txt`), never in a viewer bundle.
pub const ALLOWLIST_FILE: &str = "allowlist.txt";

const CA_LIFETIME_DAYS: i64 = 365 * 10;
const LEAF_LIFETIME_DAYS: i64 = 365 * 5;
/// How far back to backdate every certificate's `not_before`, to absorb clock skew
/// between the issuing machine and whichever machine checks validity later.
const NOT_BEFORE_SLACK_DAYS: i64 = 1;

/// Everything that can go wrong generating or loading this module's certificates/keys.
///
/// Each variant carries what's needed to reproduce the exact message this module always
/// printed (see the `# Errors` sections on the functions below, and this module's own
/// doc comment on why the message text is what's preserved, not a particular variant
/// shape).
#[derive(Debug)]
pub enum PkiError {
    /// A plain filesystem operation on `path` failed. `verb` names what was attempted
    /// ("create" / "write" / "read"), reproducing this module's long-standing "could
    /// not {verb} {path}: {source}" phrasing.
    Io {
        verb: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    /// [`init`] refuses to overwrite an existing CA at `dir`.
    CaAlreadyExists { dir: PathBuf },
    /// [`load_ca`] couldn't read `<dir>/ca.pem` or `<dir>/ca.key` back from disk --
    /// distinct from [`Self::Io`] because the message also points at `cert init`.
    CaFileUnreadable {
        path: PathBuf,
        dir: PathBuf,
        source: std::io::Error,
    },
    /// [`issue_server`] was given neither `--host` nor `--ip`.
    NoServerSan,
    /// [`sign_client_leaf`] was given an empty (or all-whitespace) `--name`.
    EmptyClientName,
    /// An [`rcgen`] operation failed (key generation, building a SAN list,
    /// self-signing, or leaf-signing). `context` names what was being built, matching
    /// this module's existing "<what> failed: <e>" messages.
    Rcgen {
        context: &'static str,
        source: rcgen::Error,
    },
    /// [`load_ca`] found `ca.key`/`ca.pem` on disk but [`rcgen`] couldn't parse it back
    /// into a signing identity.
    RcgenAtPath { path: PathBuf, source: rcgen::Error },
    /// A message already fully formatted by a helper outside this module -- currently
    /// only [`indicatrix_net::tls::write_private_key_pem`], which returns `String`
    /// because its own failure modes (directory creation, an external `icacls`
    /// process) aren't PKI-specific either. Printed verbatim, no extra wrapping.
    Message(String),
    /// [`indicatrix_net::tls::load_certs`] couldn't parse the certificate at `path`.
    Certificate {
        path: PathBuf,
        source: tls::TlsError,
    },
    /// [`issue_client`] issued the certificate and wrote the bundle to `out_dir`, but
    /// couldn't append its fingerprint to the allowlist at `path` -- the caller is
    /// told the exact line to add by hand.
    AllowlistUpdate {
        out_dir: PathBuf,
        path: PathBuf,
        fingerprint_hex: String,
        name: String,
        source: tls::TlsError,
    },
}

impl fmt::Display for PkiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { verb, path, source } => {
                write!(f, "could not {verb} {}: {source}", path.display())
            }
            Self::CaAlreadyExists { dir } => write!(
                f,
                "{} already contains a CA ({CA_CERT_FILE} and/or {CA_KEY_FILE}) -- refusing to overwrite it, since \
                 that would invalidate every certificate already issued from it. Remove those files yourself first \
                 if you really mean to start over.",
                dir.display()
            ),
            Self::CaFileUnreadable { path, dir, source } => write!(
                f,
                "could not read {}: {source} (run `indicatrix-worker cert init --dir {}` first)",
                path.display(),
                dir.display()
            ),
            Self::NoServerSan => write!(
                f,
                "issue-server requires at least one --host or --ip: rustls ignores Common Name entirely, so a \
                 certificate with no Subject Alternative Name can never be validated by a mutual-TLS client -- see \
                 --help"
            ),
            Self::EmptyClientName => write!(f, "--name must not be empty"),
            Self::Rcgen { context, source } => write!(f, "{context}: {source}"),
            Self::RcgenAtPath { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Message(msg) => f.write_str(msg),
            Self::Certificate { path, source } => write!(f, "{}: {source}", path.display()),
            Self::AllowlistUpdate {
                out_dir,
                path,
                fingerprint_hex,
                name,
                source,
            } => write!(
                f,
                "issued the certificate at {} but could not update {}: {source} -- add this line to it by hand: \
                 {fingerprint_hex}  # {name}",
                out_dir.display(),
                path.display()
            ),
        }
    }
}

impl std::error::Error for PkiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::CaFileUnreadable { source, .. } => Some(source),
            Self::Rcgen { source, .. } | Self::RcgenAtPath { source, .. } => Some(source),
            Self::Certificate { source, .. } | Self::AllowlistUpdate { source, .. } => Some(source),
            Self::CaAlreadyExists { .. }
            | Self::NoServerSan
            | Self::EmptyClientName
            | Self::Message(_) => None,
        }
    }
}

/// [`indicatrix_net::tls::write_private_key_pem`] returns `Result<(), String>`; this
/// lets every `tls::write_private_key_pem(..)?` call site in this module keep working
/// against this module's own [`PkiError`]-returning functions.
impl From<String> for PkiError {
    fn from(msg: String) -> Self {
        Self::Message(msg)
    }
}

fn not_before_with_skew_slack() -> OffsetDateTime {
    OffsetDateTime::now_utc() - Duration::days(NOT_BEFORE_SLACK_DAYS)
}

fn not_after(lifetime_days: i64) -> OffsetDateTime {
    OffsetDateTime::now_utc() + Duration::days(lifetime_days)
}

/// `indicatrix-worker cert init --dir <pki-dir>`: generates a new private CA keypair and
/// self-signed certificate.
///
/// Refuses to run if `<pki-dir>` already contains a CA -- overwriting one would
/// invalidate every certificate already issued from it. No `--force`: removing the old
/// files yourself is the point at which you're supposed to notice the blast radius.
///
/// # Errors
///
/// A human-readable message if `dir` can't be created, already has a CA, key
/// generation/self-signing fails, or the CA files can't be written (including a
/// Windows-ACL failure restricting `ca.key` -- see [`indicatrix_net::tls::write_private_key_pem`]).
pub fn init(dir: &Path) -> Result<(), PkiError> {
    std::fs::create_dir_all(dir).map_err(|source| PkiError::Io {
        verb: "create",
        path: dir.to_path_buf(),
        source,
    })?;

    let ca_cert_path = dir.join(CA_CERT_FILE);
    let ca_key_path = dir.join(CA_KEY_FILE);
    if ca_cert_path.exists() || ca_key_path.exists() {
        return Err(PkiError::CaAlreadyExists {
            dir: dir.to_path_buf(),
        });
    }

    let key_pair = KeyPair::generate().map_err(|source| PkiError::Rcgen {
        context: "CA key generation failed",
        source,
    })?;
    let mut params =
        CertificateParams::new(Vec::<String>::new()).map_err(|source| PkiError::Rcgen {
            context: "unexpected error building an empty SAN list",
            source,
        })?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params
        .distinguished_name
        .push(DnType::CommonName, "indicatrix-worker private CA");
    params.not_before = not_before_with_skew_slack();
    params.not_after = not_after(CA_LIFETIME_DAYS);

    let cert = params
        .self_signed(&key_pair)
        .map_err(|source| PkiError::Rcgen {
            context: "CA self-signing failed",
            source,
        })?;

    std::fs::write(&ca_cert_path, cert.pem()).map_err(|source| PkiError::Io {
        verb: "write",
        path: ca_cert_path.clone(),
        source,
    })?;
    tls::write_private_key_pem(&ca_key_path, &key_pair.serialize_pem())?;

    tracing::info!(
        "indicatrix-worker cert init: wrote {} and {} -- CA expires {} (UTC)",
        ca_cert_path.display(),
        ca_key_path.display(),
        params.not_after.date()
    );
    Ok(())
}

/// Reloads the CA's signing identity from `<dir>/ca.pem` and `<dir>/ca.key`, as saved
/// by [`init`]. Each `issue-*` subcommand is a fresh process, so there's nothing else
/// in memory to sign with.
///
/// # Errors
///
/// A human-readable message (naming `dir` and suggesting `cert init`) if either file is
/// missing or unreadable, or can't be parsed back into a CA signing identity.
pub(crate) fn load_ca(dir: &Path) -> Result<Issuer<'static, KeyPair>, PkiError> {
    let ca_cert_path = dir.join(CA_CERT_FILE);
    let ca_key_path = dir.join(CA_KEY_FILE);

    let ca_pem =
        std::fs::read_to_string(&ca_cert_path).map_err(|source| PkiError::CaFileUnreadable {
            path: ca_cert_path.clone(),
            dir: dir.to_path_buf(),
            source,
        })?;
    let ca_key_pem =
        std::fs::read_to_string(&ca_key_path).map_err(|source| PkiError::CaFileUnreadable {
            path: ca_key_path.clone(),
            dir: dir.to_path_buf(),
            source,
        })?;

    let ca_key = KeyPair::from_pem(&ca_key_pem).map_err(|source| PkiError::RcgenAtPath {
        path: ca_key_path.clone(),
        source,
    })?;
    // rcgen 0.14: `Issuer` pairs the CA's stored subject/extensions with its private
    // key; issuing a leaf only reads the stored CA cert, never mints a second one.
    let issuer =
        Issuer::from_ca_cert_pem(&ca_pem, ca_key).map_err(|source| PkiError::RcgenAtPath {
            path: ca_cert_path.clone(),
            source,
        })?;

    Ok(issuer)
}

/// `indicatrix-worker cert issue-server --dir <pki-dir> --host <name> --ip <addr>`: issues
/// this worker's server certificate, signed by the CA in `<pki-dir>`.
///
/// `hosts` and `ips` become the certificate's DNS and IP Subject Alternative Names --
/// at least one is required. `rustls` ignores Common Name entirely for hostname/IP
/// verification, so a server certificate with no SAN can never be validated.
///
/// # Errors
///
/// A human-readable message if both `hosts` and `ips` are empty, the CA can't be loaded
/// (see [`load_ca`]), signing fails, or the output files can't be written.
pub fn issue_server(dir: &Path, hosts: &[String], ips: &[IpAddr]) -> Result<(), PkiError> {
    if hosts.is_empty() && ips.is_empty() {
        return Err(PkiError::NoServerSan);
    }

    let ca_issuer = load_ca(dir)?;

    let mut sans: Vec<String> = hosts.to_vec();
    sans.extend(ips.iter().map(IpAddr::to_string));
    // `CertificateParams::new` classifies each string as IP or DNS by parsing it as an
    // `IpAddr` first, so passing them combined is equivalent to building SANs by hand.
    let mut params = CertificateParams::new(sans).map_err(|source| PkiError::Rcgen {
        context: "invalid --host/--ip value",
        source,
    })?;

    let common_name = hosts.first().cloned().unwrap_or_else(|| ips[0].to_string());
    params
        .distinguished_name
        .push(DnType::CommonName, common_name);
    params.not_before = not_before_with_skew_slack();
    params.not_after = not_after(LEAF_LIFETIME_DAYS);
    params.use_authority_key_identifier_extension = true;
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

    let key_pair = KeyPair::generate().map_err(|source| PkiError::Rcgen {
        context: "server key generation failed",
        source,
    })?;
    let cert = params
        .signed_by(&key_pair, &ca_issuer)
        .map_err(|source| PkiError::Rcgen {
            context: "failed to sign the server certificate",
            source,
        })?;

    let cert_path = dir.join(SERVER_CERT_FILE);
    let key_path = dir.join(SERVER_KEY_FILE);
    std::fs::write(&cert_path, cert.pem()).map_err(|source| PkiError::Io {
        verb: "write",
        path: cert_path.clone(),
        source,
    })?;
    tls::write_private_key_pem(&key_path, &key_pair.serialize_pem())?;

    tracing::info!(
        "indicatrix-worker cert issue-server: wrote {} and {} (SANs: {}{}{}) -- expires {} (UTC)",
        cert_path.display(),
        key_path.display(),
        hosts.join(", "),
        if hosts.is_empty() || ips.is_empty() {
            ""
        } else {
            ", "
        },
        ips.iter()
            .map(IpAddr::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        params.not_after.date()
    );
    Ok(())
}

/// `indicatrix-worker cert issue-client --dir <pki-dir> --name <name> --out <bundle-dir>`.
///
/// Issues a viewer's client certificate signed by the CA in `<pki-dir>`, writes a
/// self-contained bundle (`ca.pem`, `client.pem`, `client.key`) to `<bundle-dir>`, and
/// appends the new certificate's fingerprint to `<pki-dir>/allowlist.txt` (labeled
/// `name`) so the worker trusts it immediately -- no separate enrollment step.
///
/// `name` becomes the certificate's Subject Common Name, purely a human label; unlike
/// `issue-server`, no SAN is set since a client cert is never validated by hostname.
///
/// # Errors
///
/// A human-readable message if `name` is empty, the CA can't be loaded, signing fails,
/// `bundle-dir` or its files can't be written, or the fingerprint can't be appended to
/// the allowlist (in which case the printed message includes the line to add by hand).
pub fn issue_client(dir: &Path, name: &str, out: &Path) -> Result<(), PkiError> {
    let ca_issuer = load_ca(dir)?;
    let (cert, key_pair, not_after) = sign_client_leaf(&ca_issuer, name)?;

    std::fs::create_dir_all(out).map_err(|source| PkiError::Io {
        verb: "create",
        path: out.to_path_buf(),
        source,
    })?;

    let ca_source_path = dir.join(CA_CERT_FILE);
    let ca_pem = std::fs::read_to_string(&ca_source_path).map_err(|source| PkiError::Io {
        verb: "read",
        path: ca_source_path.clone(),
        source,
    })?;
    let out_ca_path = out.join(CA_CERT_FILE);
    let out_cert_path = out.join(CLIENT_CERT_FILE);
    let out_key_path = out.join(CLIENT_KEY_FILE);
    std::fs::write(&out_ca_path, &ca_pem).map_err(|source| PkiError::Io {
        verb: "write",
        path: out_ca_path.clone(),
        source,
    })?;
    std::fs::write(&out_cert_path, cert.pem()).map_err(|source| PkiError::Io {
        verb: "write",
        path: out_cert_path.clone(),
        source,
    })?;
    tls::write_private_key_pem(&out_key_path, &key_pair.serialize_pem())?;

    let fingerprint = tls::fingerprint(cert.der());
    let fingerprint_hex = tls::fingerprint_to_hex(&fingerprint);
    let allowlist_path = dir.join(ALLOWLIST_FILE);
    tls::append_to_allowlist(&allowlist_path, &fingerprint, name).map_err(|source| {
        PkiError::AllowlistUpdate {
            out_dir: out.to_path_buf(),
            path: allowlist_path.clone(),
            fingerprint_hex: fingerprint_hex.clone(),
            name: name.to_string(),
            source,
        }
    })?;

    tracing::info!(
        "indicatrix-worker cert issue-client: wrote bundle to {} ({CA_CERT_FILE}, {CLIENT_CERT_FILE}, {CLIENT_KEY_FILE}) -- \
         fingerprint {fingerprint_hex} added to {} -- expires {} (UTC)",
        out.display(),
        allowlist_path.display(),
        not_after.date()
    );
    Ok(())
}

/// The signing logic shared by [`issue_client`] (writes the bundle to disk and
/// allowlists it immediately) and [`issue_client_in_memory`] (returns PEM strings,
/// touches neither disk nor the allowlist).
///
/// # Errors
///
/// A human-readable message if `name` is empty, or signing fails.
fn sign_client_leaf(
    ca_issuer: &Issuer<'static, KeyPair>,
    name: &str,
) -> Result<(rcgen::Certificate, KeyPair, OffsetDateTime), PkiError> {
    if name.trim().is_empty() {
        return Err(PkiError::EmptyClientName);
    }

    let mut params =
        CertificateParams::new(Vec::<String>::new()).map_err(|source| PkiError::Rcgen {
            context: "unexpected error building an empty SAN list",
            source,
        })?;
    params.distinguished_name.push(DnType::CommonName, name);
    params.not_before = not_before_with_skew_slack();
    params.not_after = not_after(LEAF_LIFETIME_DAYS);
    params.use_authority_key_identifier_extension = true;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];

    let key_pair = KeyPair::generate().map_err(|source| PkiError::Rcgen {
        context: "client key generation failed",
        source,
    })?;
    let cert = params
        .signed_by(&key_pair, ca_issuer)
        .map_err(|source| PkiError::Rcgen {
            context: "failed to sign the client certificate",
            source,
        })?;

    Ok((cert, key_pair, params.not_after))
}

/// A freshly issued client certificate bundle, held entirely in memory.
///
/// The counterpart to the three files [`issue_client`] writes to `<bundle-dir>`, for
/// callers (`crate::enroll`) that must not write secret key material to disk while an
/// enrollment token is unclaimed.
pub struct InMemoryClientBundle {
    /// PEM text of the CA certificate at `<pki-dir>/ca.pem` -- included so the claiming
    /// side gets the same self-contained three-file bundle `issue_client` would write.
    pub ca_pem: String,
    /// The CA certificate's SHA-256 fingerprint -- what an enrollment token commits to,
    /// so a claiming client can verify this worker's identity before sending its bearer
    /// secret. See `indicatrix_net::token`'s doc comment.
    pub ca_fingerprint: tls::Fingerprint,
    pub client_cert_pem: String,
    pub client_key_pem: String,
    /// The issued client certificate's SHA-256 fingerprint, appended to
    /// `allowlist.txt` only once the enrollment token is actually claimed (never at
    /// issue time -- see `crate::enroll`).
    pub client_fingerprint: tls::Fingerprint,
}

/// Mints a client certificate exactly like [`issue_client`] does, but in memory only.
///
/// Same CA, lifetime, and `ClientAuth` leaf, returned as PEM strings instead of written
/// to `<bundle-dir>`, and never touches `<pki-dir>/allowlist.txt`. This is what makes
/// token-based enrollment possible -- `crate::enroll` holds the bundle in an
/// [`crate::enroll::EnrollRegistry`] entry until claimed or expired.
///
/// # Errors
///
/// Same as [`issue_client`]: a human-readable message if `name` is empty, the CA can't be
/// loaded, or signing fails.
pub fn issue_client_in_memory(dir: &Path, name: &str) -> Result<InMemoryClientBundle, PkiError> {
    let ca_issuer = load_ca(dir)?;
    let (cert, key_pair, _not_after) = sign_client_leaf(&ca_issuer, name)?;

    let ca_cert_path = dir.join(CA_CERT_FILE);
    let ca_pem = std::fs::read_to_string(&ca_cert_path).map_err(|source| PkiError::Io {
        verb: "read",
        path: ca_cert_path.clone(),
        source,
    })?;
    let ca_der = tls::load_certs(&ca_cert_path).map_err(|source| PkiError::Certificate {
        path: ca_cert_path.clone(),
        source,
    })?;
    let ca_fingerprint = tls::fingerprint(&ca_der[0]);
    let client_fingerprint = tls::fingerprint(cert.der());

    Ok(InMemoryClientBundle {
        ca_pem,
        ca_fingerprint,
        client_cert_pem: cert.pem(),
        client_key_pem: key_pair.serialize_pem(),
        client_fingerprint,
    })
}

/// The default path `serve --allowlist` uses when not given explicitly.
///
/// Sits alongside the `--ca` file, named [`ALLOWLIST_FILE`] -- matching where
/// [`issue_client`] writes by default, so the common case needs no separate
/// `--allowlist` flag.
#[must_use]
pub fn default_allowlist_path(ca_path: &Path) -> PathBuf {
    ca_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(ALLOWLIST_FILE)
}

#[cfg(test)]
mod tests {
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

        let allowlist_text = std::fs::read_to_string(dir.join(ALLOWLIST_FILE)).unwrap();
        assert!(allowlist_text.contains("# laptop"), "{allowlist_text}");

        let client_der = tls::load_certs(&out.join(CLIENT_CERT_FILE)).unwrap();
        let fp = tls::fingerprint(&client_der[0]);
        let allowlist = tls::Allowlist::load(&dir.join(ALLOWLIST_FILE)).unwrap();
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
    fn default_allowlist_path_sits_beside_the_ca_file() {
        let ca = Path::new("C:/pki/ca.pem");
        assert_eq!(
            default_allowlist_path(ca),
            PathBuf::from("C:/pki").join(ALLOWLIST_FILE)
        );
    }
}
