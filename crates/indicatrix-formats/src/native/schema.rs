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

/// The on-disk mirror of a tier's authoring-level target.
///
/// Mirrors `indicatrix_cut_core::design::TierTarget` one-to-one, same
/// lossless-mirror convention as [`NativeMeetConstraint`] -- "cut to 3.20 mm",
/// "girdle 2.5% of width", "table 4.10 mm wide": girdle thickness and table
/// size as authoring targets, not just readouts.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NativeTierTarget {
    /// Mirrors `TierTarget::DepthMm`.
    DepthMm { mm: f64 },
    /// Mirrors `TierTarget::GirdleThicknessMm`.
    GirdleThicknessMm { mm: f64 },
    /// Mirrors `TierTarget::TableWidthMm`.
    TableWidthMm { mm: f64 },
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
    /// How far the preform's own vertical span is shifted from centred, mirroring
    /// `indicatrix_cut_core::design::Design::preform_y_offset` one-to-one.
    /// `#[serde(default)]` so a file saved before this field existed
    /// loads as `0.0` -- exactly the always-centred span every such file's preform
    /// actually had.
    #[serde(default)]
    pub y_offset: f64,
    /// Keys a future build wrote that this build's four named fields above don't
    /// claim -- see the module doc comment's "Unknown fields" section. Always empty
    /// when freshly built via [`PreformTable::new`]; only non-empty after parsing a
    /// newer file.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

impl PreformTable {
    /// Builds a fresh table with no unknown/future fields carried over, and
    /// `y_offset` at `0.0` (see [`Self::with_y_offset`]) -- the shape every
    /// freshly-authored (never parsed) table has. `shape`/`half_width`/
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
            y_offset: 0.0,
            unknown: toml::Table::new(),
        }
    }

    /// Attaches [`Self::y_offset`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_y_offset(mut self, y_offset: f64) -> Self {
        self.y_offset = y_offset;
        self
    }
}

/// The `[material.custom]` snapshot for a design's custom catalogue material.
///
/// Everything needed to reconstruct that material if its name does not resolve on
/// the machine that opens the file -- without it, a native file referencing a
/// custom material by name only falls back to Diamond elsewhere.
///
/// Sharing a `.indicatrix.toml` + `.asc` pair naming a custom material (or simply
/// reinstalling and losing the local database row) would otherwise silently
/// resolve that name to Diamond, with no warning. Every field here is a plain
/// primitive, not
/// `indicatrix`'s own `GemMaterial` type: this crate has no dependency on
/// `indicatrix` at all (see the module doc comment's "only pulls in what it
/// needs" rule), so building this snapshot from a real `GemMaterial` -- and
/// registering it back as a session-local custom material when the name fails to
/// resolve -- is `indicatrix-cut-core`'s job, same split as every other table
/// here. `crystal_system`/`optical_character` are carried as their `Debug`-style
/// names (e.g. `"Trigonal"`, `"UniaxialNegative"`) purely for a human reading the
/// raw TOML; `GemMaterial::new_custom` re-derives both from the sign of
/// `birefringence_delta` on load; and `specific_gravity` is `None` when the
/// material carried no SG at save time.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CustomMaterialSnapshot {
    pub mean_ri: f64,
    pub dispersion_delta: f64,
    pub birefringence_delta: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specific_gravity: Option<f64>,
    pub crystal_system: String,
    pub optical_character: String,
    /// See [`PreformTable::unknown`]'s doc comment.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

impl CustomMaterialSnapshot {
    /// Builds a fresh snapshot with no unknown/future fields carried over -- see
    /// [`PreformTable::new`]'s own doc comment for why this lives here rather than
    /// in `indicatrix-cut-core`.
    #[must_use]
    pub fn new(
        mean_ri: f64,
        dispersion_delta: f64,
        birefringence_delta: f64,
        specific_gravity: Option<f64>,
        crystal_system: impl Into<String>,
        optical_character: impl Into<String>,
    ) -> Self {
        Self {
            mean_ri,
            dispersion_delta,
            birefringence_delta,
            specific_gravity,
            crystal_system: crystal_system.into(),
            optical_character: optical_character.into(),
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
    /// See [`CustomMaterialSnapshot`]'s own doc comment.
    /// `#[serde(default)]` so a file written before this field existed loads with
    /// `None` rather than an error -- and so a design on a built-in material never
    /// grows one at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<CustomMaterialSnapshot>,
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
            custom: None,
            unknown: toml::Table::new(),
        }
    }

    /// Attaches [`Self::custom`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_custom(mut self, custom: Option<CustomMaterialSnapshot>) -> Self {
        self.custom = custom;
        self
    }
}

/// The `[source]` table: the printed/measured proportions a catalogue row supplied
/// when this design was loaded, if any.
///
/// Without it, Open Native would reset `printed_proportions`, disabling Deep
/// Solve for catalogue designs. Every field mirrors one
/// `indicatrix::geometry::stone_metrics::ExternalProportions` figure one-to-one;
/// building/reading the actual type is `indicatrix-cut-core`'s job (this crate has no
/// dependency on `indicatrix` at all -- see the crate doc comment's "only pulls in
/// what it needs" rule), same split as [`PreformTable`]/[`MaterialTable`]. Entirely
/// optional at the top level ([`NativeDesignFile::source`]): a design that was never
/// loaded from a catalogue row (a brand-new "New Design", or a directly opened
/// `.asc`) has nothing to record here.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SourceTable {
    /// Printed `Vol/W^3`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vol_w3: Option<f64>,
    /// Printed `L/W`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lw: Option<f64>,
    /// Printed `C/W`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cw: Option<f64>,
    /// Printed `P/W`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pw: Option<f64>,
    /// Printed `H/W`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hw: Option<f64>,
}

impl SourceTable {
    /// `true` iff every field is `None` -- a table with nothing worth writing at all
    /// (see [`NativeDesignFile::with_source`]'s own doc comment for why this is
    /// checked before attaching one).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.vol_w3.is_none()
            && self.lw.is_none()
            && self.cw.is_none()
            && self.pw.is_none()
            && self.hw.is_none()
    }
}

/// The `[history]` table: a bounded, human-readable trail of edits made to this
/// design since it was first saved natively.
///
/// Each entry is one already-formatted description (oldest first) -- building and
/// bounding the list from the editor's own `History` stack of `Edit`s is
/// `indicatrix-cut-core`'s job, same split as every other table in this file; this
/// schema only needs to round-trip strings. There is deliberately no per-entry
/// timestamp or structured `Edit` payload: the list exists for a cutter (or
/// support conversation) to read, not to be replayed.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HistoryTable {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<String>,
    /// See [`PreformTable::unknown`]'s doc comment.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

impl HistoryTable {
    /// Builds a fresh table from `entries`, with no unknown/future fields carried
    /// over -- see [`PreformTable::new`]'s own doc comment for why this lives here
    /// rather than in `indicatrix-cut-core`.
    #[must_use]
    pub fn new(entries: Vec<String>) -> Self {
        Self {
            entries,
            unknown: toml::Table::new(),
        }
    }

    /// `true` iff there is nothing worth writing at all -- see
    /// [`NativeDesignFile::with_history`]'s own doc comment for why this is checked
    /// before attaching one.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One `[[tiers]]` entry: this native file's per-tier overlay -- or, for a
/// [`NativeDesignFile::draft`] save, the sole record of that tier at all.
///
/// Ordinarily NOT a full mirror of the editor's own tier type -- `angle_deg`/
/// `indices` stay canonical in the paired `.asc`; this otherwise only carries what
/// `.asc` cannot express (the authored [`NativeMeetConstraint`], which orbit members
/// are detached, and -- since the "gap this closes" module docs -- the meet
/// instruction and raw notes text a real `.asc` file's `G` field stated at import
/// time). `name` is purely a human-readable label for raw-TOML readers (e.g. `git
/// diff`); loading a paired (non-draft) file never reads it back into a design.
/// Tiers correlate to the paired `.asc`'s tier list by ARRAY POSITION alone -- see
/// the parent module's "The fingerprint" section for why a fingerprint mismatch
/// disables re-applying this overlay, and [`NativeDesignFile::draft`]'s own doc
/// comment for the one case where `angle_deg`/`indices` here ARE read back (a design
/// that does not currently solve has no reliable paired `.asc` to read them from at
/// all).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TierTable {
    pub name: String,
    pub constraint: NativeMeetConstraint,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detached: Vec<f64>,
    /// Mirrors `indicatrix_cut_core::design::ConstraintTier::angle_deg`. `#[serde(default)]`
    /// so an ordinary (non-draft) file saved before this field existed, or one whose
    /// tiers are only ever an overlay on a solvable paired `.asc`, still loads --
    /// `None` there is never read back into a design; see this type's own doc
    /// comment for the one caller (a draft reload) that does read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle_deg: Option<f64>,
    /// Mirrors `indicatrix_cut_core::design::ConstraintTier::indices`. See
    /// [`Self::angle_deg`]'s own doc comment -- same rule, same one reader.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indices: Option<Vec<f64>>,
    /// Mirrors `indicatrix_cut_core::design::ConstraintTier::imported_meet`: the meet
    /// instruction a real `.asc` file's `G` field actually stated for this tier at
    /// import time, preserved so a native reload can restore the editor's one-click
    /// "Adopt" action without needing to re-derive it from `.asc` text that (for a
    /// draft save) may not even be trustworthy. `#[serde(default)]` so a file saved
    /// before this field existed still loads, as `None` (nothing to adopt on
    /// reload from such a file, same as a tier `indicatrix_cut_core` never imported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_meet: Option<NativeMeetConstraint>,
    /// Mirrors `indicatrix_cut_core::design::ConstraintTier::original_notes`: the raw
    /// `.asc` `G`-field text this tier's file actually carried at import time,
    /// verbatim. `#[serde(default)]` for the same before-this-field-existed reason as
    /// [`Self::imported_meet`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_notes: Option<String>,
    /// A cutter-authored free-text note for this tier --
    /// unlike [`Self::original_notes`] (read-only imported `.asc` `G`-field text),
    /// this is a new authoring surface with no `.asc` counterpart to round-trip
    /// through, so it lives only here. `#[serde(default)]` so a file saved before
    /// this field existed still loads, as `None` (no note yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A per-tier "cheater"/azimuth-offset annotation, in degrees -- mirrors
    /// `indicatrix_cut_core::design::Design::cheater_offsets_deg`'s own map, keyed
    /// by this tier's array position. Like [`Self::note`],
    /// this is authored, undoable data with no `.asc` counterpart to round-trip
    /// through, so it lives only here. `#[serde(default)]` so a file saved before
    /// this field existed still loads, as `None` (no cheater offset recorded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cheater_offset_deg: Option<f64>,
    /// A stable per-tier identity, mirroring
    /// `indicatrix_cut_core::design::TierId::value` --
    /// `cheater_offset_deg`/`note` above are keyed by ARRAY POSITION
    /// (this whole table already documents that), which renumbers on add/remove/
    /// move; a `TierId` does not. `#[serde(default)]` so a file saved before this
    /// field existed still loads, as `None` -- the loader is expected to assign a
    /// fresh id to every such tier on load (old files have no stable identity to
    /// recover, only a fresh one to start from).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_id: Option<u64>,
    /// This tier's authoring-level target, if any -- see [`NativeTierTarget`].
    /// `#[serde(default)]` so a file saved before this field existed still loads,
    /// as `None` (no target authored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<NativeTierTarget>,
    /// See [`PreformTable::unknown`]'s doc comment -- the same rule, per tier.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

impl TierTable {
    /// Builds a fresh entry with no unknown/future fields, and no `angle_deg`/
    /// `indices`/`imported_meet`/`original_notes`/`cheater_offset_deg` (all `None`)
    /// carried over -- see [`PreformTable::new`]'s own doc comment for why this
    /// lives here rather than in `indicatrix-cut-core`. A caller with one of those
    /// five to attach uses the matching `with_*` method afterward.
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
            angle_deg: None,
            indices: None,
            imported_meet: None,
            original_notes: None,
            note: None,
            cheater_offset_deg: None,
            tier_id: None,
            target: None,
            unknown: toml::Table::new(),
        }
    }

    /// Attaches [`Self::tier_id`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_tier_id(mut self, tier_id: Option<u64>) -> Self {
        self.tier_id = tier_id;
        self
    }

    /// Attaches [`Self::target`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_target(mut self, target: Option<NativeTierTarget>) -> Self {
        self.target = target;
        self
    }

    /// Attaches [`Self::angle_deg`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_angle_deg(mut self, angle_deg: Option<f64>) -> Self {
        self.angle_deg = angle_deg;
        self
    }

    /// Attaches [`Self::indices`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_indices(mut self, indices: Option<Vec<f64>>) -> Self {
        self.indices = indices;
        self
    }

    /// Attaches [`Self::imported_meet`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_imported_meet(mut self, imported_meet: Option<NativeMeetConstraint>) -> Self {
        self.imported_meet = imported_meet;
        self
    }

    /// Attaches [`Self::original_notes`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_original_notes(mut self, original_notes: Option<String>) -> Self {
        self.original_notes = original_notes;
        self
    }

    /// Attaches [`Self::note`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_note(mut self, note: Option<String>) -> Self {
        self.note = note;
        self
    }

    /// Attaches [`Self::cheater_offset_deg`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_cheater_offset_deg(mut self, cheater_offset_deg: Option<f64>) -> Self {
        self.cheater_offset_deg = cheater_offset_deg;
        self
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
    /// `true` iff `design` did not currently solve when this file was saved (see
    /// `indicatrix_cut_core::native::save_paired`'s own doc comment): `tiers` here then
    /// carries the FULL tier list (`angle_deg`/`indices` included, not left `None`)
    /// rather than an overlay on the paired `.asc`, and the paired `.asc` itself is a
    /// placeholder -- every mast in it is a made-up number, never a real cut
    /// instruction, and must never be shown to a cutter as one. A loader is expected
    /// to rebuild its design entirely from `tiers` when this is `true`, ignoring the
    /// paired `.asc`'s own tier list (its `angle_deg`/`gear`/`symmetry`/`refractive_index`
    /// header fields are still real, only its per-tier masts are not).
    /// `#[serde(default)]` so a file saved before this field existed loads as `false`
    /// (never a draft) -- the only meaning an absent flag could have, since a draft
    /// save did not exist yet either.
    #[serde(default)]
    pub draft: bool,
    /// See [`SourceTable`]'s own doc comment. `#[serde(default)]` so a file saved
    /// before this field existed still loads, as `None` -- exactly what it should
    /// mean for a design this build never associated with a catalogue row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceTable>,
    /// See [`HistoryTable`]'s own doc comment. `#[serde(default)]` so a file saved
    /// before this field existed still loads, as `None` -- exactly what it should
    /// mean for a design this build never recorded a history trail for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryTable>,
    /// The catalogue row this design was loaded from (or has since saved back to),
    /// if any -- `[source]` carries a
    /// catalogue row's PRINTED proportions, but not which row they came from, so a
    /// reopen can only ever replay the numbers as they were at save time, never
    /// notice that the row's own figures have since changed. A plain top-level
    /// field, not nested inside [`SourceTable`]: that struct is built from a raw
    /// field literal in `indicatrix-cut-core::native::convert` (see this crate's own
    /// "only pulls in what it needs" boundary note), so adding a field there would
    /// break that unrelated call site; a new top-level field, defaulted inside
    /// [`Self::new`] and attached via [`Self::with_catalogue_entry_id`], costs that
    /// file nothing until it opts in. `#[serde(default)]` so a file saved before
    /// this field existed loads as `None` -- exactly what it should mean for a
    /// design never associated with a catalogue row, or saved by a build before this
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalogue_entry_id: Option<i64>,
    /// The AUTHORED (legacy schedule) refractive index, as last set directly or
    /// imported from a `.asc` `I` line the design's material selection did not
    /// override -- distinct from the EFFECTIVE refractive index (material/
    /// override resolved) that `.asc` export always writes instead. `.asc`
    /// has exactly one RI slot and export always
    /// writes the effective value there, so re-importing that same file
    /// overwrites the authored figure with whatever the effective one happened to
    /// be -- this field is the native sidecar's own place to keep the authored
    /// value distinct from that lossy round trip. `#[serde(default)]` so a file
    /// saved before this field existed still loads, as `None` -- the loader then
    /// falls back to whatever the paired `.asc`'s own `I` line says, exactly
    /// today's (lossy) behavior for such a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authored_refractive_index: Option<f64>,
    /// Top-level keys a future build wrote that none of the named fields above claim
    /// -- see [`PreformTable::unknown`]'s doc comment.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

impl NativeDesignFile {
    /// Builds a fresh document -- `format_version` pinned to [`FORMAT_VERSION`],
    /// [`Self::draft`] `false`, no unknown/future fields at this level or any nested
    /// table's, the shape every freshly-authored (never parsed), ordinarily-solvable
    /// document has. `indicatrix-cut-core`'s `to_native_file` is the higher-level
    /// function that actually builds `preform`/`material`/`tiers` from a real editor
    /// design and calls this with the result -- see [`PreformTable::new`]'s own doc
    /// comment for why building the mirror types themselves is split out that way. A
    /// caller saving a design that does not currently solve calls [`Self::with_draft`]
    /// afterward.
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
            draft: false,
            source: None,
            history: None,
            catalogue_entry_id: None,
            authored_refractive_index: None,
            unknown: toml::Table::new(),
        }
    }

    /// Attaches [`Self::authored_refractive_index`] -- see that field's own doc
    /// comment.
    #[must_use]
    pub const fn with_authored_refractive_index(
        mut self,
        authored_refractive_index: Option<f64>,
    ) -> Self {
        self.authored_refractive_index = authored_refractive_index;
        self
    }

    /// Attaches [`Self::draft`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_draft(mut self, draft: bool) -> Self {
        self.draft = draft;
        self
    }

    /// Attaches [`Self::source`] -- `None` when `source.is_empty()` (nothing worth
    /// writing at all), so an ordinary design with no catalogue provenance never
    /// grows an all-`None` `[source]` table it would just have to skip reading back.
    #[must_use]
    pub fn with_source(mut self, source: SourceTable) -> Self {
        self.source = (!source.is_empty()).then_some(source);
        self
    }

    /// Attaches [`Self::history`] -- `None` when `history.is_empty()` (nothing
    /// worth writing at all), so a design with no recorded trail never grows an
    /// empty `[history]` table it would just have to skip reading back. The
    /// caller (`indicatrix-cut-core`) is responsible for bounding `entries`'
    /// length before calling this -- this schema does not enforce a bound itself.
    #[must_use]
    pub fn with_history(mut self, history: HistoryTable) -> Self {
        self.history = (!history.is_empty()).then_some(history);
        self
    }

    /// Attaches [`Self::catalogue_entry_id`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_catalogue_entry_id(mut self, catalogue_entry_id: Option<i64>) -> Self {
        self.catalogue_entry_id = catalogue_entry_id;
        self
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
