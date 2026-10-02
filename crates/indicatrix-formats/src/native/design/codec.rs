//! Reading, writing and recognising self-contained design files.
//!
//! The reader is header-first: `format` and `version` are read and judged before the
//! rest of the file is parsed, so a file of another kind or from a newer major
//! version gets its own typed error even when the rest of it would not parse.

use super::{
    attachment::{AttachmentsError, validate_attachments},
    schema::{DESIGN_FORMAT, DESIGN_VERSION, DesignFile, MAX_DESIGN_TIERS},
};
use crate::asc::AscParseError;
use std::{collections::BTreeSet, fmt};

/// Why a design file could not be written or read.
#[derive(Debug)]
pub enum DesignFileError {
    /// The document could not be serialised (in practice unreachable for the named
    /// fields; surfaced because the `unknown` tables can hold arbitrary values).
    Serialize(toml::ser::Error),
    /// The text is not valid TOML, or does not match the schema.
    Parse(toml::de::Error),
    /// The file is not a design file: its `format` key is missing or names another
    /// format. `found` is the format it named, if any.
    NotADesignFile {
        /// The `format` value the file carries, when it has one.
        found: Option<String>,
    },
    /// The header has no `version` key.
    MissingVersion,
    /// The `version` key is below 1 or does not fit a version number.
    InvalidVersion(i64),
    /// The file was written by a newer major version than this build reads.
    UnsupportedVersion {
        /// The version the file declares.
        found: i64,
        /// The newest version this build reads.
        supported: u32,
    },
    /// A tier has no `angle_deg` or no `indices`.
    TierMissingGeometry {
        /// Zero-based position of the tier in the file.
        index: usize,
    },
    /// Two tiers carry the same `tier_id`.
    DuplicateTierId(u64),
    /// A field holds a value outside what a design can be.
    InvalidField {
        /// The key (or table) at fault.
        field: &'static str,
        /// What is wrong with it.
        reason: String,
    },
    /// The `[[attachments]]` array is refused: a bad or duplicate name, a size over the
    /// limit, data that is not base64, or bytes that do not match the recorded size or
    /// SHA-256 (the file is damaged).
    Attachments(AttachmentsError),
}

impl fmt::Display for DesignFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(e) => write!(f, "cannot serialize the design file: {e}"),
            Self::Parse(e) => write!(f, "the design file is not valid: {e}"),
            Self::NotADesignFile { found: Some(other) } => write!(
                f,
                "this is not an Indicatrix design file: its format is '{other}', expected '{DESIGN_FORMAT}'"
            ),
            Self::NotADesignFile { found: None } => write!(
                f,
                "this is not an Indicatrix design file: the 'format' key is missing"
            ),
            Self::MissingVersion => write!(f, "the design file's 'version' key is missing"),
            Self::InvalidVersion(v) => write!(f, "the design file's 'version' {v} is invalid"),
            Self::UnsupportedVersion { found, supported } => write!(
                f,
                "this design was made by a newer Indicatrix (file version {found}; this one reads up to {supported}); update Indicatrix to open it"
            ),
            Self::TierMissingGeometry { index } => {
                write!(f, "tier {} has no angle_deg or indices recorded", index + 1)
            }
            Self::DuplicateTierId(id) => write!(f, "tier_id {id} is used by more than one tier"),
            Self::InvalidField { field, reason } => write!(f, "'{field}' is invalid: {reason}"),
            Self::Attachments(e) => write!(f, "the design file's attachments are invalid: {e}"),
        }
    }
}

impl std::error::Error for DesignFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(e) => Some(e),
            Self::Parse(e) => Some(e),
            Self::Attachments(e) => Some(e),
            _ => None,
        }
    }
}

/// The kind of document a file holds, as far as its header or name shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// A self-contained design file (`format = "indicatrix-design"`).
    Design,
    /// An overlay sidecar of the older paired format (`.indicatrix.toml` or
    /// `.gemcut.toml`): it needs its `.asc` next to it.
    OverlaySidecar,
    /// Neither.
    Unknown,
}

/// The two header keys, plus the keys that identify an older overlay sidecar. Every
/// other key is ignored here.
#[derive(Debug, Default, serde::Deserialize)]
struct Header {
    format: Option<String>,
    version: Option<i64>,
    format_version: Option<i64>,
    asc_filename: Option<String>,
}

/// The byte offset of the first table header line: everything before it holds the
/// top-level keys.
fn header_end(text: &str) -> usize {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with('[') {
            return offset;
        }
        offset += line.len();
    }
    text.len()
}

/// Reads the header keys: the part before the first table is tried first, so the
/// header stays readable when the rest of the file is truncated or broken.
fn read_header(text: &str) -> Result<Header, toml::de::Error> {
    if let Some(prefix) = text.get(..header_end(text))
        && let Ok(header) = toml::from_str::<Header>(prefix)
    {
        return Ok(header);
    }
    toml::from_str::<Header>(text)
}

fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Judges the header of `text` and returns its version.
///
/// # Errors
///
/// [`DesignFileError::NotADesignFile`] for another or no format,
/// [`DesignFileError::MissingVersion`], [`DesignFileError::InvalidVersion`] and
/// [`DesignFileError::UnsupportedVersion`] (a newer major version) for the version, or
/// [`DesignFileError::Parse`] when not even the header is TOML.
pub fn check_header(text: &str) -> Result<u32, DesignFileError> {
    let header = read_header(strip_bom(text)).map_err(DesignFileError::Parse)?;
    if header.format.as_deref() != Some(DESIGN_FORMAT) {
        return Err(DesignFileError::NotADesignFile {
            found: header.format,
        });
    }
    match header.version {
        None => Err(DesignFileError::MissingVersion),
        Some(v) if v > i64::from(DESIGN_VERSION) => Err(DesignFileError::UnsupportedVersion {
            found: v,
            supported: DESIGN_VERSION,
        }),
        Some(v) if v < 1 => Err(DesignFileError::InvalidVersion(v)),
        Some(v) => u32::try_from(v).map_err(|_| DesignFileError::InvalidVersion(v)),
    }
}

/// Checks `[meta]` and `[[attachments]]`: sizes, formats, unique names and, for every
/// attachment, that the bytes match the recorded size and SHA-256.
///
/// # Errors
///
/// [`DesignFileError::InvalidField`] for `[meta]`, [`DesignFileError::Attachments`] for
/// the attachments.
pub fn validate_extras(file: &DesignFile) -> Result<(), DesignFileError> {
    file.meta
        .validate()
        .map_err(|(field, reason)| DesignFileError::InvalidField { field, reason })?;
    validate_attachments(&file.attachments).map_err(DesignFileError::Attachments)
}

/// Checks the parts of a parsed document no field type can express.
///
/// # Errors
///
/// [`DesignFileError::InvalidField`], [`DesignFileError::TierMissingGeometry`],
/// [`DesignFileError::DuplicateTierId`] or [`DesignFileError::Attachments`].
pub fn validate(file: &DesignFile) -> Result<(), DesignFileError> {
    let teeth = file.schedule.gear_teeth.unsigned_abs();
    if teeth == 0 || teeth > AscParseError::MAX_GEAR_TEETH {
        return Err(DesignFileError::InvalidField {
            field: "schedule.gear_teeth",
            reason: format!(
                "{} is not between 1 and {}",
                file.schedule.gear_teeth,
                AscParseError::MAX_GEAR_TEETH
            ),
        });
    }
    if file.tiers.len() > MAX_DESIGN_TIERS {
        return Err(DesignFileError::InvalidField {
            field: "tiers",
            reason: format!(
                "{} tiers; a design holds at most {MAX_DESIGN_TIERS}",
                file.tiers.len()
            ),
        });
    }
    let mut seen = BTreeSet::new();
    for (index, tier) in file.tiers.iter().enumerate() {
        if tier.angle_deg.is_none() || tier.indices.is_none() {
            return Err(DesignFileError::TierMissingGeometry { index });
        }
        if let Some(id) = tier.tier_id
            && !seen.insert(id)
        {
            return Err(DesignFileError::DuplicateTierId(id));
        }
    }
    validate_extras(file)
}

/// Parses and validates the text of a design file.
///
/// # Errors
///
/// See [`DesignFileError`]; the header is judged first.
pub fn parse(text: &str) -> Result<DesignFile, DesignFileError> {
    let text = strip_bom(text);
    check_header(text)?;
    let file: DesignFile = toml::from_str(text).map_err(DesignFileError::Parse)?;
    validate(&file)?;
    Ok(file)
}

/// Serialises a design file to deterministic TOML text (LF line endings, header first).
///
/// `[meta]` and `[[attachments]]` are checked first (see [`validate_extras`]), so a
/// file this function writes always passes those checks when read back.
///
/// # Errors
///
/// [`DesignFileError::InvalidField`] or [`DesignFileError::Attachments`] for a `[meta]`
/// or attachment the reader would refuse; [`DesignFileError::Serialize`], in practice
/// unreachable for the named fields.
pub fn to_string(file: &DesignFile) -> Result<String, DesignFileError> {
    validate_extras(file)?;
    toml::to_string_pretty(file).map_err(DesignFileError::Serialize)
}

/// Tells a design file from an older overlay sidecar by its content.
///
/// Reads only the header: `format = "indicatrix-design"` is a [`FileKind::Design`];
/// the older sidecar's `format_version`/`asc_filename` keys make it a
/// [`FileKind::OverlaySidecar`]; anything else (including bytes that are not UTF-8
/// TOML) is [`FileKind::Unknown`].
#[must_use]
pub fn detect_kind(bytes: &[u8]) -> FileKind {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return FileKind::Unknown;
    };
    let Ok(header) = read_header(strip_bom(text)) else {
        return FileKind::Unknown;
    };
    if header.format.as_deref() == Some(DESIGN_FORMAT) {
        FileKind::Design
    } else if header.format.is_none()
        && (header.format_version.is_some() || header.asc_filename.is_some())
    {
        FileKind::OverlaySidecar
    } else {
        FileKind::Unknown
    }
}
