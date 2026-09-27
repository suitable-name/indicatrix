//! `cert init` / `cert issue-server` / `cert issue-client`: an in-process private CA
//! for `indicatrix-worker`'s mutual TLS, with two client roles (viewer and joining
//! worker -- see [`role`]).
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
//!   server.pem       the coordinator's (`serve`'s) certificate (public)
//!   server.key       the coordinator's private key (sensitive -- ACL-restricted)
//!   allowlist-viewers.txt  trusted VIEWER certificate fingerprints -- see `indicatrix_net::tls`
//!                          (a pre-role `allowlist.txt` keeps serving until this exists)
//!   allowlist-workers.txt  trusted WORKER certificate fingerprints (coordinator worker port)
//!
//! <bundle-dir>/        (from `issue-client --out`, copied to the viewer/worker machine)
//!   ca.pem
//!   client.pem
//!   client.key        (sensitive -- ACL-restricted)
//! ```
//!
//! A worker certificate's Common Name is `worker:<name>`; see [`role`] for how roles
//! are recorded and checked.
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

use indicatrix_net::{messages::PeerRole, tls};
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

pub mod role;
#[cfg(test)]
mod tests;

pub use role::{
    LEGACY_ALLOWLIST_FILE, VIEWER_ALLOWLIST_FILE, WORKER_ALLOWLIST_FILE, WORKER_CN_PREFIX,
    allowlist_in, role_of_certificate, viewer_allowlist_in, worker_allowlist_in,
    worker_label_of_certificate,
};

pub const CA_CERT_FILE: &str = "ca.pem";
pub const CA_KEY_FILE: &str = "ca.key";
pub const SERVER_CERT_FILE: &str = "server.pem";
pub const SERVER_KEY_FILE: &str = "server.key";
pub const CLIENT_CERT_FILE: &str = "client.pem";
pub const CLIENT_KEY_FILE: &str = "client.key";

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
    /// A VIEWER certificate was requested for a `--name` starting with
    /// [`WORKER_CN_PREFIX`], which would make it look like a worker certificate.
    ReservedRolePrefix { name: String },
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
            Self::ReservedRolePrefix { name } => write!(
                f,
                "--name {name:?} starts with {WORKER_CN_PREFIX:?}, which marks WORKER certificates -- pick \
                 another name for a viewer, or pass --role worker for a worker certificate"
            ),
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
            | Self::ReservedRolePrefix { .. }
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

/// `indicatrix-worker cert issue-client --dir <pki-dir> --name <name> --out <bundle-dir>`
/// for a VIEWER -- [`issue_client_with_role`] with [`PeerRole::Viewer`].
///
/// # Errors
///
/// See [`issue_client_with_role`].
pub fn issue_client(dir: &Path, name: &str, out: &Path) -> Result<(), PkiError> {
    issue_client_with_role(dir, name, out, PeerRole::Viewer)
}

/// `indicatrix-worker cert issue-client --dir <pki-dir> --name <name> --out <bundle-dir>
/// [--role viewer|worker]`.
///
/// Issues a client certificate for `role` signed by the CA in `<pki-dir>`, writes a
/// self-contained bundle (`ca.pem`, `client.pem`, `client.key`) to `<bundle-dir>`, and
/// appends the new certificate's fingerprint to that role's allowlist in `<pki-dir>`
/// ([`allowlist_in`]: `allowlist-viewers.txt` or `allowlist-workers.txt`), so the
/// matching listener trusts it immediately -- no separate enrollment step.
///
/// The Common Name is `name` for a viewer and `worker:<name>` for a worker (see
/// [`role`]); unlike `issue-server`, no SAN is set since a client cert is never
/// validated by hostname.
///
/// # Errors
///
/// A human-readable message if `name` is empty (or a viewer name carrying the worker
/// prefix), the CA can't be loaded, signing fails, `bundle-dir` or its files can't be
/// written, or the fingerprint can't be appended to the allowlist (in which case the
/// printed message includes the line to add by hand).
pub fn issue_client_with_role(
    dir: &Path,
    name: &str,
    out: &Path,
    role: PeerRole,
) -> Result<(), PkiError> {
    let ca_issuer = load_ca(dir)?;
    let (cert, key_pair, not_after, common_name) = sign_client_leaf(&ca_issuer, name, role)?;

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
    let allowlist_path = allowlist_in(dir, role);
    tls::append_to_allowlist(&allowlist_path, &fingerprint, &common_name).map_err(|source| {
        PkiError::AllowlistUpdate {
            out_dir: out.to_path_buf(),
            path: allowlist_path.clone(),
            fingerprint_hex: fingerprint_hex.clone(),
            name: common_name.clone(),
            source,
        }
    })?;

    tracing::info!(
        "indicatrix-worker cert issue-client: wrote {} bundle {common_name:?} to {} ({CA_CERT_FILE}, {CLIENT_CERT_FILE}, \
         {CLIENT_KEY_FILE}) -- fingerprint {fingerprint_hex} added to {} -- expires {} (UTC)",
        role::role_name(role),
        out.display(),
        allowlist_path.display(),
        not_after.date()
    );
    Ok(())
}

/// The signing logic shared by [`issue_client_with_role`] (writes the bundle to disk
/// and allowlists it immediately) and [`issue_client_in_memory`] (returns PEM strings,
/// touches neither disk nor the allowlist). Returns the certificate, its key, its expiry
/// and the Common Name it was issued under ([`role::common_name_for`]).
///
/// # Errors
///
/// A human-readable message if `name` is empty, a viewer `name` carries the worker
/// prefix, or signing fails.
fn sign_client_leaf(
    ca_issuer: &Issuer<'static, KeyPair>,
    name: &str,
    role: PeerRole,
) -> Result<(rcgen::Certificate, KeyPair, OffsetDateTime, String), PkiError> {
    let label = name.strip_prefix(WORKER_CN_PREFIX).unwrap_or(name);
    if label.trim().is_empty() {
        return Err(PkiError::EmptyClientName);
    }
    let common_name =
        role::common_name_for(role, name).ok_or_else(|| PkiError::ReservedRolePrefix {
            name: name.to_string(),
        })?;

    let mut params =
        CertificateParams::new(Vec::<String>::new()).map_err(|source| PkiError::Rcgen {
            context: "unexpected error building an empty SAN list",
            source,
        })?;
    params
        .distinguished_name
        .push(DnType::CommonName, common_name.as_str());
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

    Ok((cert, key_pair, params.not_after, common_name))
}

/// A freshly issued client certificate bundle, held entirely in memory.
///
/// The counterpart to the three files [`issue_client_with_role`] writes to
/// `<bundle-dir>`, for callers (`crate::enroll`) that must not write secret key material
/// to disk while an enrollment token is unclaimed.
pub struct InMemoryClientBundle {
    /// PEM text of the CA certificate at `<pki-dir>/ca.pem` -- included so the claiming
    /// side gets the same self-contained three-file bundle `issue_client` would write.
    pub ca_pem: String,
    /// The CA certificate's SHA-256 fingerprint -- what an enrollment token commits to,
    /// so a claiming client can verify this worker's identity before sending its bearer
    /// secret. See `indicatrix_net::token`'s doc comment.
    pub ca_fingerprint: tls::Fingerprint,
    /// PEM text of the issued client certificate.
    pub client_cert_pem: String,
    /// PEM text of the issued client certificate's private key.
    pub client_key_pem: String,
    /// The issued client certificate's SHA-256 fingerprint, appended to the role's
    /// allowlist only once the enrollment token is actually claimed (never at issue
    /// time -- see `crate::enroll`).
    pub client_fingerprint: tls::Fingerprint,
    /// The Common Name the certificate was issued under (`worker:<name>` for a worker).
    pub common_name: String,
}

/// Mints a client certificate for `role` exactly like [`issue_client_with_role`] does,
/// but in memory only.
///
/// Same CA, lifetime, and `ClientAuth` leaf, returned as PEM strings instead of written
/// to `<bundle-dir>`, and never touches an allowlist. This is what makes token-based
/// enrollment possible -- `crate::enroll` holds the bundle in an
/// [`crate::enroll::EnrollRegistry`] entry until claimed or expired.
///
/// # Errors
///
/// Same as [`issue_client_with_role`]: a human-readable message if `name` is empty (or
/// a viewer name carrying the worker prefix), the CA can't be loaded, or signing fails.
pub fn issue_client_in_memory(
    dir: &Path,
    name: &str,
    role: PeerRole,
) -> Result<InMemoryClientBundle, PkiError> {
    let ca_issuer = load_ca(dir)?;
    let (cert, key_pair, _not_after, common_name) = sign_client_leaf(&ca_issuer, name, role)?;

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
        common_name,
    })
}

/// The default path of `serve --allowlist` (the VIEWER allowlist).
///
/// [`viewer_allowlist_in`] the `--ca` file's directory: `allowlist-viewers.txt`, or a
/// pre-role `allowlist.txt` while only that exists. Matches where [`issue_client`]
/// writes by default.
#[must_use]
pub fn default_viewer_allowlist_path(ca_path: &Path) -> PathBuf {
    viewer_allowlist_in(&role::pki_dir_of(ca_path))
}

/// The default path `serve --worker-allowlist` uses when not given explicitly:
/// `allowlist-workers.txt` next to the `--ca` file.
#[must_use]
pub fn default_worker_allowlist_path(ca_path: &Path) -> PathBuf {
    worker_allowlist_in(&role::pki_dir_of(ca_path))
}
