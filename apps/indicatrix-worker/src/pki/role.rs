//! Certificate roles: which of the two client roles -- a viewer
//! or a joining render worker -- a client certificate was issued for, and the two
//! allowlist files that go with them.
//!
//! # How a role is recorded
//!
//! In the certificate's Subject Common Name, signed by the CA like every other field:
//!
//! - a **worker** certificate's CN is `worker:<label>` ([`WORKER_CN_PREFIX`]);
//! - a **viewer** certificate is every other client certificate -- in particular every
//!   certificate issued before roles existed, whose CN is just the `--name` label.
//!
//! `cert issue-client` refuses a viewer `--name` that already starts with `worker:`, so
//! the prefix can only ever come from an explicit `--role worker`. The CA is private and
//! operated by the same person on both ends, so a CN prefix is as trustworthy as a
//! custom extension would be, and far simpler to inspect by hand
//! (`openssl x509 -noout -subject`).
//!
//! # Two allowlists
//!
//! [`VIEWER_ALLOWLIST_FILE`] (`allowlist-viewers.txt`) and [`WORKER_ALLOWLIST_FILE`]
//! (`allowlist-workers.txt`) live next to the CA. A listener checks the presented
//! certificate's role AND its membership in that listener's own list, so a viewer
//! certificate is never accepted on the worker port (and vice versa) even if someone
//! pastes its fingerprint into the wrong file.
//!
//! **Migration:** a PKI directory created before roles existed has `allowlist.txt`
//! ([`LEGACY_ALLOWLIST_FILE`]). As long as `allowlist-viewers.txt` does not exist, that
//! legacy file keeps serving as the viewers list ([`viewer_allowlist_in`]) -- nothing is
//! renamed or rewritten automatically; rename it by hand whenever convenient.

use indicatrix_net::messages::PeerRole;
use std::path::{Path, PathBuf};

/// The Common Name prefix that marks a worker certificate -- see the module doc comment.
pub const WORKER_CN_PREFIX: &str = "worker:";

/// The viewers' allowlist, next to the CA (see the module doc comment).
pub const VIEWER_ALLOWLIST_FILE: &str = "allowlist-viewers.txt";

/// The joining workers' allowlist, next to the CA.
pub const WORKER_ALLOWLIST_FILE: &str = "allowlist-workers.txt";

/// The pre-role allowlist name, still honoured as the viewers list while
/// [`VIEWER_ALLOWLIST_FILE`] does not exist.
pub const LEGACY_ALLOWLIST_FILE: &str = "allowlist.txt";

/// The Common Name a certificate for `label` in `role` carries.
///
/// `label` itself for a viewer, `worker:<label>` for a worker. `None` when `label` is a
/// viewer label that would be mistaken for a worker's (it already starts with
/// [`WORKER_CN_PREFIX`]). A worker label that already carries the prefix is not
/// prefixed twice.
#[must_use]
pub fn common_name_for(role: PeerRole, label: &str) -> Option<String> {
    match role {
        PeerRole::Viewer => (!label.starts_with(WORKER_CN_PREFIX)).then(|| label.to_string()),
        PeerRole::Worker => Some(if label.starts_with(WORKER_CN_PREFIX) {
            label.to_string()
        } else {
            format!("{WORKER_CN_PREFIX}{label}")
        }),
    }
}

/// The role a Common Name encodes (see the module doc comment); an absent CN is a viewer.
#[must_use]
pub fn role_of_common_name(common_name: Option<&str>) -> PeerRole {
    if common_name.is_some_and(|cn| cn.starts_with(WORKER_CN_PREFIX)) {
        PeerRole::Worker
    } else {
        PeerRole::Viewer
    }
}

/// The role of a DER-encoded client certificate, from its Subject Common Name.
///
/// # Errors
///
/// A human-readable message if the certificate's subject cannot be parsed.
pub fn role_of_certificate(der: &[u8]) -> Result<PeerRole, String> {
    subject_common_name(der)
        .map(|cn| role_of_common_name(cn.as_deref()))
        .map_err(|e| format!("could not read the client certificate's subject: {e}"))
}

/// The worker label a Common Name carries: `<label>` of `worker:<label>`; `None` for a
/// viewer's CN or an empty label.
#[must_use]
pub fn worker_label_of_common_name(common_name: &str) -> Option<&str> {
    common_name
        .strip_prefix(WORKER_CN_PREFIX)
        .filter(|label| !label.is_empty())
}

/// The worker label of a DER-encoded client certificate (see
/// [`worker_label_of_common_name`]).
///
/// The stable identity of a joined worker across reconnects (registry ids are per
/// connection). `None` for a viewer certificate, one without a Common Name, or an
/// unparsable subject.
#[must_use]
pub fn worker_label_of_certificate(der: &[u8]) -> Option<String> {
    let common_name = subject_common_name(der).ok().flatten()?;
    worker_label_of_common_name(&common_name).map(str::to_string)
}

/// A human word for `role`, for log lines and refusal messages.
#[must_use]
pub const fn role_name(role: PeerRole) -> &'static str {
    match role {
        PeerRole::Viewer => "viewer",
        PeerRole::Worker => "worker",
    }
}

/// The viewers' allowlist inside `dir`: [`VIEWER_ALLOWLIST_FILE`], or the legacy
/// [`LEGACY_ALLOWLIST_FILE`] while only that one exists (see the module doc comment).
#[must_use]
pub fn viewer_allowlist_in(dir: &Path) -> PathBuf {
    let current = dir.join(VIEWER_ALLOWLIST_FILE);
    let legacy = dir.join(LEGACY_ALLOWLIST_FILE);
    if !current.exists() && legacy.exists() {
        legacy
    } else {
        current
    }
}

/// The workers' allowlist inside `dir`.
#[must_use]
pub fn worker_allowlist_in(dir: &Path) -> PathBuf {
    dir.join(WORKER_ALLOWLIST_FILE)
}

/// The allowlist for `role` inside `dir` -- [`viewer_allowlist_in`] or
/// [`worker_allowlist_in`].
#[must_use]
pub fn allowlist_in(dir: &Path, role: PeerRole) -> PathBuf {
    match role {
        PeerRole::Viewer => viewer_allowlist_in(dir),
        PeerRole::Worker => worker_allowlist_in(dir),
    }
}

/// The directory holding the CA file at `ca_path` (`.` for a bare file name) -- where
/// both allowlists live by default.
#[must_use]
pub fn pki_dir_of(ca_path: &Path) -> PathBuf {
    ca_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

// ---- A minimal DER walk to the subject Common Name ---------------------------------
//
// Only what an X.509 certificate's subject needs: definite-length TLVs, a SEQUENCE /
// SET / OID / string walk. No new dependency (rcgen's parser does not expose a leaf's
// subject publicly).

/// DER tags this walk needs.
const TAG_SEQUENCE: u8 = 0x30;
const TAG_SET: u8 = 0x31;
const TAG_OID: u8 = 0x06;
const TAG_EXPLICIT_VERSION: u8 = 0xA0;
/// `id-at-commonName`, 2.5.4.3.
const OID_COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];

/// Splits one DER TLV off `input`: `(tag, content, rest)`.
fn take_tlv(input: &[u8]) -> Result<(u8, &[u8], &[u8]), &'static str> {
    let (&tag, after_tag) = input.split_first().ok_or("truncated tag")?;
    let (&first_len, after_len) = after_tag.split_first().ok_or("truncated length")?;
    let (len, body) = if first_len < 0x80 {
        (usize::from(first_len), after_len)
    } else {
        let count = usize::from(first_len & 0x7F);
        if count == 0 || count > 4 || after_len.len() < count {
            return Err("unsupported length encoding");
        }
        let len = after_len[..count]
            .iter()
            .fold(0_usize, |acc, &b| (acc << 8) | usize::from(b));
        (len, &after_len[count..])
    };
    if body.len() < len {
        return Err("length runs past the end of the certificate");
    }
    Ok((tag, &body[..len], &body[len..]))
}

/// [`take_tlv`], additionally requiring `expected` as the tag.
fn take_expected(input: &[u8], expected: u8) -> Result<(&[u8], &[u8]), &'static str> {
    let (tag, content, rest) = take_tlv(input)?;
    if tag == expected {
        Ok((content, rest))
    } else {
        Err("unexpected DER tag")
    }
}

/// Decodes a directory-string value by its DER tag.
fn decode_string(tag: u8, bytes: &[u8]) -> Result<String, &'static str> {
    match tag {
        // UTF8String, PrintableString, IA5String, TeletexString (treated as UTF-8/ASCII).
        0x0C | 0x13 | 0x16 | 0x14 => std::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|_| "Common Name is not valid UTF-8"),
        // BMPString: UTF-16BE.
        0x1E => {
            if bytes.len() & 1 == 1 {
                return Err("odd-length BMPString");
            }
            let units: Vec<u16> = (0..bytes.len() / 2)
                .map(|i| u16::from_be_bytes([bytes[2 * i], bytes[2 * i + 1]]))
                .collect();
            String::from_utf16(&units).map_err(|_| "invalid BMPString")
        }
        _ => Err("unsupported Common Name string type"),
    }
}

/// The first Common Name in a certificate's SUBJECT (not issuer), or `None` if the
/// subject has none.
///
/// # Errors
///
/// A short description of the first structural problem found.
pub fn subject_common_name(der: &[u8]) -> Result<Option<String>, &'static str> {
    let (certificate, _) = take_expected(der, TAG_SEQUENCE)?;
    let (mut tbs, _) = take_expected(certificate, TAG_SEQUENCE)?;
    if tbs.first() == Some(&TAG_EXPLICIT_VERSION) {
        tbs = take_tlv(tbs)?.2;
    }
    // serialNumber, signature, issuer, validity -- then subject.
    for _ in 0..4 {
        tbs = take_tlv(tbs)?.2;
    }
    let (mut subject, _) = take_expected(tbs, TAG_SEQUENCE)?;
    while !subject.is_empty() {
        let (mut set, rest) = take_expected(subject, TAG_SET)?;
        subject = rest;
        while !set.is_empty() {
            let (attribute, rest) = take_expected(set, TAG_SEQUENCE)?;
            set = rest;
            let (oid, value) = take_expected(attribute, TAG_OID)?;
            if oid == OID_COMMON_NAME {
                let (tag, bytes, _) = take_tlv(value)?;
                return decode_string(tag, bytes).map(Some);
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cert_with_cn(cn: Option<&str>) -> Vec<u8> {
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        // rcgen's default subject carries its own placeholder CN; start from nothing.
        params.distinguished_name = rcgen::DistinguishedName::new();
        if let Some(cn) = cn {
            params
                .distinguished_name
                .push(rcgen::DnType::CommonName, cn);
        }
        params
            .distinguished_name
            .push(rcgen::DnType::OrganizationName, "indicatrix");
        let key = rcgen::KeyPair::generate().unwrap();
        params.self_signed(&key).unwrap().der().to_vec()
    }

    #[test]
    fn subject_common_name_reads_the_subject_of_a_real_certificate() {
        let der = cert_with_cn(Some("worker:render-box"));
        assert_eq!(
            subject_common_name(&der).unwrap().as_deref(),
            Some("worker:render-box")
        );
        assert_eq!(role_of_certificate(&der).unwrap(), PeerRole::Worker);
        assert_eq!(
            worker_label_of_certificate(&der).as_deref(),
            Some("render-box")
        );

        let viewer = cert_with_cn(Some("laptop"));
        assert_eq!(role_of_certificate(&viewer).unwrap(), PeerRole::Viewer);
        assert_eq!(worker_label_of_certificate(&viewer), None);
        assert_eq!(worker_label_of_common_name("worker:"), None);

        let no_cn = cert_with_cn(None);
        assert_eq!(subject_common_name(&no_cn).unwrap(), None);
        assert_eq!(role_of_certificate(&no_cn).unwrap(), PeerRole::Viewer);
    }

    #[test]
    fn subject_common_name_rejects_garbage_without_panicking() {
        assert!(subject_common_name(&[]).is_err());
        assert!(subject_common_name(&[0x30, 0x82, 0xFF]).is_err());
        let der = cert_with_cn(Some("laptop"));
        assert!(subject_common_name(&der[..der.len() / 3]).is_err());
    }

    #[test]
    fn common_names_encode_and_decode_the_role() {
        assert_eq!(
            common_name_for(PeerRole::Viewer, "laptop").as_deref(),
            Some("laptop")
        );
        assert_eq!(common_name_for(PeerRole::Viewer, "worker:sneaky"), None);
        assert_eq!(
            common_name_for(PeerRole::Worker, "gpu-box").as_deref(),
            Some("worker:gpu-box")
        );
        assert_eq!(
            common_name_for(PeerRole::Worker, "worker:gpu-box").as_deref(),
            Some("worker:gpu-box")
        );
        assert_eq!(role_of_common_name(Some("worker:x")), PeerRole::Worker);
        assert_eq!(role_of_common_name(Some("Worker:x")), PeerRole::Viewer);
        assert_eq!(role_of_common_name(None), PeerRole::Viewer);
    }

    #[test]
    fn the_legacy_allowlist_serves_viewers_until_the_new_file_exists() {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-worker-role-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(viewer_allowlist_in(&dir), dir.join(VIEWER_ALLOWLIST_FILE));
        std::fs::write(dir.join(LEGACY_ALLOWLIST_FILE), "").unwrap();
        assert_eq!(viewer_allowlist_in(&dir), dir.join(LEGACY_ALLOWLIST_FILE));
        std::fs::write(dir.join(VIEWER_ALLOWLIST_FILE), "").unwrap();
        assert_eq!(viewer_allowlist_in(&dir), dir.join(VIEWER_ALLOWLIST_FILE));
        assert_eq!(
            allowlist_in(&dir, PeerRole::Worker),
            dir.join(WORKER_ALLOWLIST_FILE)
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
