//! The native document schema itself: [`NativeDesignFile`] and its nested tables --
//! plain, `serde`-deriving mirrors of an editor's own in-memory types (owned by
//! `indicatrix-cut-core`, not this crate; converting to and from them is that crate's
//! job, since only it can see both sides) -- and the plain serialize/parse functions
//! over them. See the parent module's doc comment for the format choice and the
//! "Unknown fields" convention every table here follows.

use std::fmt;

/// This module's own schema version.
///
/// Bumped only for a change `serde(default)` on a newly added field can't handle (see
/// the module doc comment's "Unknown fields" section); expected to stay `1` for a
/// long time.
pub const FORMAT_VERSION: u32 = 1;

const fn default_format_version() -> u32 {
    FORMAT_VERSION
}

/// Errors from [`to_toml_string`]/[`from_toml_str`], the serialize/parse pair over
/// [`NativeDesignFile`].
///
/// Mirrors this crate's [`crate::asc::AscParseError`] in spirit -- a typed error
/// rather than a bare `toml::de::Error`/`toml::ser::Error`, so a caller matching on
/// which direction failed doesn't need this module's TOML dependency in scope at all.
#[derive(Debug)]
pub enum NativeFormatError {
    /// [`to_toml_string`] failed to serialize a [`NativeDesignFile`]. In practice
    /// unreachable for this module's own named fields, but surfaced rather than
    /// `.expect()`ed because [`NativeDesignFile::unknown`] (and every nested
    /// `unknown` table) can hold arbitrary `toml::Value`s carried over from a newer
    /// file this build doesn't understand.
    Serialize(toml::ser::Error),
    /// [`from_toml_str`] failed to parse text into a [`NativeDesignFile`]: not valid
    /// TOML, or valid TOML that doesn't match this schema.
    Parse(toml::de::Error),
}

impl fmt::Display for NativeFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(e) => write!(f, "cannot serialize native design file: {e}"),
            Self::Parse(e) => write!(f, "native design file is not valid: {e}"),
        }
    }
}

impl std::error::Error for NativeFormatError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(e) => Some(e),
            Self::Parse(e) => Some(e),
        }
    }
}

/// The on-disk mirror of a tier's authored meet constraint.
///
/// A total, lossless TOML encoding of the editor's own meet-constraint type (owned by
/// `indicatrix`'s geometry crate, not this one) -- converting between the two is
/// `indicatrix-cut-core`'s job, not this module's, since only that crate can see both
/// types.
///
/// Internally tagged (`kind`) rather than TOML's untagged-by-shape default so the
/// on-disk form stays self-describing: `{ kind = "scale_reference", mast = 0.65 }`
/// names itself; a bare `{ mast = 0.65 }` would not.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NativeMeetConstraint {
    /// Mirrors the editor's "meet whatever facet already occupies this position"
    /// constraint.
    MeetExisting,
    /// Mirrors the editor's "meet a facet by name" constraint.
    MeetNamed { names: Vec<String> },
    /// Mirrors the editor's "pin this tier's mast to an exact scale reference"
    /// constraint -- the variant this format exists to make expressible as authored
    /// intent at all (see the module doc comment's "The gap this closes" section).
    ScaleReference { mast: f64 },
}

/// The on-disk mirror of a preform's shape.
///
/// Same lossless round trip as [`NativeMeetConstraint`], for the editor's own
/// preform-shape type -- kept as a dedicated mirror so this crate's schema never
/// needs that type (or the crate that owns it) in scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NativePreformShape {
    Block,
    Cylinder { sides: usize },
}

/// The `[preform]` table: a TOML mirror of the editor's own preform spec.
///
/// See the module doc comment for why `.asc` has no room for this. Reading one back
/// into an actual preform spec is `indicatrix-cut-core`'s job; building one from a
/// fresh design's preform spec is [`PreformTable::new`]'s.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PreformTable {
    pub shape: NativePreformShape,
    pub half_width: f64,
    pub length_over_width: f64,
    pub depth: f64,
    /// Keys a future build wrote that this build's three named fields above don't
    /// claim -- see the module doc comment's "Unknown fields" section. Always empty
    /// when freshly built via [`PreformTable::new`]; only non-empty after parsing a
    /// newer file.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

impl PreformTable {
    /// Builds a fresh table with no unknown/future fields carried over -- the shape
    /// every freshly-authored (never parsed) table has. `shape`/`half_width`/
    /// `length_over_width`/`depth` mirror an editor preform spec's own fields
    /// one-to-one; building the mirror from an actual spec is
    /// `indicatrix-cut-core`'s job (that crate has no reason to name
    /// [`toml::Table`] itself just to fill in `unknown`).
    #[must_use]
    pub fn new(
        shape: NativePreformShape,
        half_width: f64,
        length_over_width: f64,
        depth: f64,
    ) -> Self {
        Self {
            shape,
            half_width,
            length_over_width,
            depth,
            unknown: toml::Table::new(),
        }
    }
}

/// The `[material]` table: a TOML mirror of the editor's own material selection.
///
/// Building one from -- or reading one back into -- an actual material selection is
/// `indicatrix-cut-core`'s job, same as [`PreformTable`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MaterialTable {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specific_gravity_override: Option<f64>,
    /// `#[serde(default)]` so a file written before this field existed loads with
    /// `None` rather than an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refractive_index_override: Option<f64>,
    /// See [`PreformTable::unknown`]'s doc comment.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

impl MaterialTable {
    /// Builds a fresh table with no unknown/future fields carried over -- see
    /// [`PreformTable::new`]'s own doc comment for why this lives here rather than
    /// in `indicatrix-cut-core`.
    #[must_use]
    pub fn new(
        name: Option<String>,
        specific_gravity_override: Option<f64>,
        refractive_index_override: Option<f64>,
    ) -> Self {
        Self {
            name,
            specific_gravity_override,
            refractive_index_override,
            unknown: toml::Table::new(),
        }
    }
}

/// One `[[tiers]]` entry: this native file's per-tier overlay.
///
/// Deliberately NOT a full mirror of the editor's own tier type -- `angle_deg`/
/// `indices` stay canonical in the paired `.asc`; this only carries what `.asc`
/// cannot express (the authored [`NativeMeetConstraint`] and which orbit members are
/// detached). `name` is purely a human-readable label for raw-TOML readers (e.g.
/// `git diff`); loading a paired file never reads it back into a design. Tiers
/// correlate to the paired `.asc`'s tier list by ARRAY POSITION alone -- see the
/// parent module's "The fingerprint" section for why a fingerprint mismatch disables
/// re-applying this overlay.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TierTable {
    pub name: String,
    pub constraint: NativeMeetConstraint,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detached: Vec<f64>,
    /// See [`PreformTable::unknown`]'s doc comment -- the same rule, per tier.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

impl TierTable {
    /// Builds a fresh entry with no unknown/future fields carried over -- see
    /// [`PreformTable::new`]'s own doc comment for why this lives here rather than
    /// in `indicatrix-cut-core`.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        constraint: NativeMeetConstraint,
        detached: Vec<f64>,
    ) -> Self {
        Self {
            name: name.into(),
            constraint,
            detached,
            unknown: toml::Table::new(),
        }
    }
}

/// The whole native document -- see the module doc comment for the format, the
/// fingerprint, and why each field is (or is not) here.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NativeDesignFile {
    #[serde(default = "default_format_version")]
    pub format_version: u32,
    /// The paired `.asc`'s own bare file name (e.g. `"design.asc"`), never a full
    /// path: the two files travel together in the same directory (see the module doc
    /// comment's "Extension and layout" section), and a baked-in path would go stale
    /// the moment that directory is renamed or moved.
    pub asc_filename: String,
    /// SHA-256 (lowercase hex, via [`crate::native::sha256_hex`]) of the paired
    /// `.asc`'s raw bytes as of this file's last save. See "The fingerprint" section
    /// of the module doc comment.
    pub asc_sha256: String,
    pub preform: PreformTable,
    /// Mirrors the editor's own girdle-diameter-in-mm field directly (already an
    /// `Option<f64>` there, so no separate table needed the way `preform`/`material`
    /// get one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub girdle_diameter_mm: Option<f64>,
    pub material: MaterialTable,
    pub tiers: Vec<TierTable>,
    /// Top-level keys a future build wrote that none of the named fields above claim
    /// -- see [`PreformTable::unknown`]'s doc comment.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

impl NativeDesignFile {
    /// Builds a fresh document -- `format_version` pinned to [`FORMAT_VERSION`], no
    /// unknown/future fields at this level or any nested table's, the shape every
    /// freshly-authored (never parsed) document has. `indicatrix-cut-core`'s
    /// `to_native_file` is the higher-level function that actually builds `preform`/
    /// `material`/`tiers` from a real editor design and calls this with the result --
    /// see [`PreformTable::new`]'s own doc comment for why building the mirror types
    /// themselves is split out that way.
    #[must_use]
    pub fn new(
        asc_filename: impl Into<String>,
        asc_sha256: impl Into<String>,
        preform: PreformTable,
        girdle_diameter_mm: Option<f64>,
        material: MaterialTable,
        tiers: Vec<TierTable>,
    ) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            asc_filename: asc_filename.into(),
            asc_sha256: asc_sha256.into(),
            preform,
            girdle_diameter_mm,
            material,
            tiers,
            unknown: toml::Table::new(),
        }
    }
}

/// Serializes `file` to TOML text.
///
/// Pretty-printed (blank lines between tables, `[[tiers]]` array-of-tables rather than
/// inline) for human readability -- see the module doc comment's "Format: TOML"
/// section.
///
/// # Errors
///
/// [`NativeFormatError::Serialize`] -- in practice unreachable for this module's own
/// types, but surfaced rather than `.expect()`ed because [`NativeDesignFile::unknown`]
/// can hold arbitrary `toml::Value`s carried over from a newer file this build
/// doesn't understand.
pub fn to_toml_string(file: &NativeDesignFile) -> Result<String, NativeFormatError> {
    toml::to_string_pretty(file).map_err(NativeFormatError::Serialize)
}

/// Parses a native document's text into a [`NativeDesignFile`].
///
/// # Errors
///
/// [`NativeFormatError::Parse`] for anything that isn't valid TOML matching this
/// schema. Every field besides the handful with a real default (`format_version`,
/// `girdle_diameter_mm`, `detached`, every `unknown` map) must be present, but key
/// order and extra keys are unconstrained -- see the module doc comment's "Format:
/// TOML" section.
pub fn from_toml_str(text: &str) -> Result<NativeDesignFile, NativeFormatError> {
    toml::from_str(text).map_err(NativeFormatError::Parse)
}
