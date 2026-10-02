//! The `[[attachments]]` array of a design file: byte-exact files that cannot be
//! recomputed from the design (a design's PDF, its original `.gem`/`.asc`, its diagram
//! image, anything else the owner attached).
//!
//! In memory an attachment is an [`AttachmentBlob`] (plain bytes); in the file it is an
//! [`AttachmentTable`] carrying the same bytes as one-line standard base64 plus the
//! declared size and SHA-256, both checked on read. The cap on the total payload is
//! [`MAX_ATTACHMENT_BYTES`], enforced identically when writing and reading.

use super::base64;
use crate::native::sha256_hex;
use std::{collections::BTreeSet, fmt};

/// Upper bound on the summed decoded size of all attachments in one file: 64 MiB.
pub const MAX_ATTACHMENT_BYTES: u64 = 64 * 1024 * 1024;

/// Upper bound on the number of attachments in one file.
pub const MAX_ATTACHMENTS: usize = 1_000;

/// Upper bound on an attachment's name, in bytes of UTF-8.
pub const MAX_ATTACHMENT_NAME_BYTES: usize = 255;

/// Upper bound on an attachment's media type, in bytes.
pub const MAX_ATTACHMENT_MIME_BYTES: usize = 127;

/// What an attachment is for, so a reader can find the design's PDF or diagram image
/// without guessing from the file name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentRole {
    /// The design's diagram picture (see `[meta]` for nothing else about it).
    DiagramImage,
    /// A PDF describing the design (instructions, competition booklet).
    Pdf,
    /// The original `GemCad` `.gem` file the design was made from.
    Gem,
    /// The original `.asc` text the design was imported from, byte for byte.
    Asc,
    /// Any other file; also what a role this build does not know reads back as.
    #[default]
    #[serde(other)]
    Other,
}

/// One attachment with its bytes, the plain-data form the vault and apps handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentBlob {
    /// File name, unique within a design (e.g. `"design.pdf"`).
    pub name: String,
    /// What the file is for.
    pub role: AttachmentRole,
    /// Media type, e.g. `"application/pdf"`; see [`mime_type_for_name`].
    pub mime_type: String,
    /// The page the file came from, or empty for a locally made file.
    pub source_url: String,
    /// The file's exact bytes.
    pub data: Vec<u8>,
}

impl AttachmentBlob {
    /// A blob with the media type guessed from the name by [`mime_type_for_name`] and
    /// no source URL.
    #[must_use]
    pub fn new(name: impl Into<String>, role: AttachmentRole, data: Vec<u8>) -> Self {
        let name = name.into();
        let mime_type = mime_type_for_name(&name).to_string();
        Self {
            name,
            role,
            mime_type,
            source_url: String::new(),
            data,
        }
    }
}

/// The media type for a file name's extension; `application/octet-stream` when the
/// extension is not one a design normally carries.
#[must_use]
pub fn mime_type_for_name(name: &str) -> &'static str {
    let extension = name.rsplit_once('.').map_or("", |(_, e)| e);
    match extension.to_ascii_lowercase().as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "txt" | "asc" => "text/plain",
        "toml" => "application/toml",
        _ => "application/octet-stream",
    }
}

/// Why one attachment is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentProblem {
    /// The name is empty.
    EmptyName,
    /// The name is longer than [`MAX_ATTACHMENT_NAME_BYTES`].
    NameTooLong {
        /// The name's length in bytes.
        len: usize,
    },
    /// The name holds a path separator or a control character.
    NameInvalid,
    /// Another attachment has the same name.
    DuplicateName,
    /// The media type is too long or not printable ASCII (an empty one is allowed and
    /// means "unknown").
    MimeTypeInvalid,
    /// The declared or actual size alone exceeds [`MAX_ATTACHMENT_BYTES`].
    TooLarge {
        /// The offending size in bytes.
        size: u64,
    },
    /// `data` is not strict padded standard base64.
    BadEncoding,
    /// The declared `size` is not the decoded length.
    SizeMismatch {
        /// The size the file declares.
        declared: u64,
        /// The decoded length.
        actual: u64,
    },
    /// `sha256` is not 64 hexadecimal digits.
    BadHash,
    /// The SHA-256 of the decoded bytes is not the declared `sha256`.
    HashMismatch,
}

impl fmt::Display for AttachmentProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => write!(f, "the name is empty"),
            Self::NameTooLong { len } => write!(
                f,
                "the name is {len} bytes; at most {MAX_ATTACHMENT_NAME_BYTES} are allowed"
            ),
            Self::NameInvalid => write!(f, "the name holds a path separator or control character"),
            Self::DuplicateName => write!(f, "another attachment has the same name"),
            Self::MimeTypeInvalid => write!(f, "the media type is too long or not ASCII"),
            Self::TooLarge { size } => write!(
                f,
                "{size} bytes exceed the {MAX_ATTACHMENT_BYTES}-byte attachment limit"
            ),
            Self::BadEncoding => write!(f, "the data is not valid base64"),
            Self::SizeMismatch { declared, actual } => write!(
                f,
                "the declared size {declared} differs from the decoded size {actual}"
            ),
            Self::BadHash => write!(f, "the sha256 is not 64 hexadecimal digits"),
            Self::HashMismatch => write!(f, "the data does not match its sha256 (file damaged)"),
        }
    }
}

/// One `[[attachments]]` entry as stored: metadata, size, hash and base64 data.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AttachmentTable {
    /// File name, unique within the file.
    pub name: String,
    /// What the file is for.
    #[serde(default)]
    pub role: AttachmentRole,
    /// Media type.
    #[serde(default)]
    pub mime_type: String,
    /// Source page URL; omitted when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// Decoded size in bytes.
    pub size: u64,
    /// Lowercase hexadecimal SHA-256 of the decoded bytes.
    pub sha256: String,
    /// The bytes, standard base64 with padding, on one line.
    pub data: String,
    /// Keys a newer build wrote that this build does not claim; written back as read.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

impl AttachmentTable {
    /// Encodes `blob` (infallible; limits are checked by [`validate_attachments`] when
    /// the file is written or read).
    #[must_use]
    pub fn from_blob(blob: &AttachmentBlob) -> Self {
        Self {
            name: blob.name.clone(),
            role: blob.role,
            mime_type: blob.mime_type.clone(),
            url: blob.source_url.clone(),
            size: blob.data.len() as u64,
            sha256: sha256_hex(&blob.data),
            data: base64::encode(&blob.data),
            unknown: toml::Table::new(),
        }
    }

    /// Decodes the entry, checking the declared size and SHA-256 against the bytes.
    ///
    /// # Errors
    ///
    /// [`AttachmentProblem::TooLarge`], [`AttachmentProblem::BadEncoding`],
    /// [`AttachmentProblem::SizeMismatch`], [`AttachmentProblem::BadHash`] or
    /// [`AttachmentProblem::HashMismatch`].
    pub fn to_blob(&self) -> Result<AttachmentBlob, AttachmentProblem> {
        if self.size > MAX_ATTACHMENT_BYTES {
            return Err(AttachmentProblem::TooLarge { size: self.size });
        }
        let data = base64::decode(&self.data).ok_or(AttachmentProblem::BadEncoding)?;
        let actual = data.len() as u64;
        if actual != self.size {
            return Err(AttachmentProblem::SizeMismatch {
                declared: self.size,
                actual,
            });
        }
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(AttachmentProblem::BadHash);
        }
        if !self.sha256.eq_ignore_ascii_case(&sha256_hex(&data)) {
            return Err(AttachmentProblem::HashMismatch);
        }
        Ok(AttachmentBlob {
            name: self.name.clone(),
            role: self.role,
            mime_type: self.mime_type.clone(),
            source_url: self.url.clone(),
            data,
        })
    }
}

/// Why a set of attachments is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentsError {
    /// More than [`MAX_ATTACHMENTS`] entries.
    TooMany(usize),
    /// The declared sizes sum past [`MAX_ATTACHMENT_BYTES`].
    TotalTooLarge {
        /// The summed size in bytes.
        total: u64,
    },
    /// One entry is refused.
    Entry {
        /// The entry's name (possibly empty or invalid; shown as written).
        name: String,
        /// What is wrong.
        problem: AttachmentProblem,
    },
}

impl fmt::Display for AttachmentsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooMany(n) => write!(f, "{n} attachments; at most {MAX_ATTACHMENTS} are allowed"),
            Self::TotalTooLarge { total } => write!(
                f,
                "the attachments total {total} bytes; at most {MAX_ATTACHMENT_BYTES} are allowed"
            ),
            Self::Entry { name, problem } => write!(f, "attachment '{name}': {problem}"),
        }
    }
}

impl std::error::Error for AttachmentsError {}

fn check_name(name: &str) -> Result<(), AttachmentProblem> {
    if name.is_empty() {
        Err(AttachmentProblem::EmptyName)
    } else if name.len() > MAX_ATTACHMENT_NAME_BYTES {
        Err(AttachmentProblem::NameTooLong { len: name.len() })
    } else if name
        .chars()
        .any(|c| c == '/' || c == '\\' || c.is_control())
    {
        Err(AttachmentProblem::NameInvalid)
    } else {
        Ok(())
    }
}

fn check_mime(mime: &str) -> Result<(), AttachmentProblem> {
    let printable = mime.bytes().all(|b| b.is_ascii_graphic() || b == b' ');
    if mime.len() > MAX_ATTACHMENT_MIME_BYTES || !printable {
        Err(AttachmentProblem::MimeTypeInvalid)
    } else {
        Ok(())
    }
}

/// The cheap checks over the whole array: count, names (valid, unique), media types and
/// the total of the declared sizes. Nothing is decoded.
fn check_entries(tables: &[AttachmentTable]) -> Result<(), AttachmentsError> {
    if tables.len() > MAX_ATTACHMENTS {
        return Err(AttachmentsError::TooMany(tables.len()));
    }
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for table in tables {
        let fail = |problem| AttachmentsError::Entry {
            name: table.name.clone(),
            problem,
        };
        check_name(&table.name).map_err(fail)?;
        if !seen.insert(table.name.as_str()) {
            return Err(fail(AttachmentProblem::DuplicateName));
        }
        check_mime(&table.mime_type).map_err(fail)?;
        total = total.saturating_add(table.size);
        if total > MAX_ATTACHMENT_BYTES {
            return Err(AttachmentsError::TotalTooLarge { total });
        }
    }
    Ok(())
}

fn decode_entry(table: &AttachmentTable) -> Result<AttachmentBlob, AttachmentsError> {
    table.to_blob().map_err(|problem| AttachmentsError::Entry {
        name: table.name.clone(),
        problem,
    })
}

/// Checks every entry of the `[[attachments]]` array.
///
/// Count, names (valid, unique), media types and the total of the declared sizes
/// against [`MAX_ATTACHMENT_BYTES`] come first, then each entry's encoding, size and
/// SHA-256 by decoding it.
///
/// # Errors
///
/// [`AttachmentsError`]; the first problem found, in file order.
pub fn validate_attachments(tables: &[AttachmentTable]) -> Result<(), AttachmentsError> {
    check_entries(tables)?;
    tables
        .iter()
        .try_for_each(|table| decode_entry(table).map(drop))
}

/// Encodes `blobs` as `[[attachments]]` entries, in the order given.
#[must_use]
pub fn attachment_tables(blobs: &[AttachmentBlob]) -> Vec<AttachmentTable> {
    blobs.iter().map(AttachmentTable::from_blob).collect()
}

/// Decodes and verifies every entry (see [`validate_attachments`]) into blobs, in file
/// order.
///
/// # Errors
///
/// The same as [`validate_attachments`].
pub fn attachment_blobs(
    tables: &[AttachmentTable],
) -> Result<Vec<AttachmentBlob>, AttachmentsError> {
    check_entries(tables)?;
    tables.iter().map(decode_entry).collect()
}
