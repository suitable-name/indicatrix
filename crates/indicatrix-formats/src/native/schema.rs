//! The tables of the native design file: plain, `serde`-deriving mirrors of an editor's
//! own in-memory types (owned by `indicatrix-cut-core`, not this crate; converting to
//! and from them is that crate's job, since only it can see both sides). The design
//! file ([`super::design`]) assembles them into the whole document. See the parent
//! module's doc comment for the format choice and the "Unknown fields" convention
//! every table here follows.

pub use super::custom_material::CustomMaterialSnapshot;

mod tier;

pub use tier::{ConcaveTierTable, TierTable};

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
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NativePreformShape {
    Block,
    Cylinder {
        sides: usize,
    },
    /// A convex plane set in model units: `planes` holds `[nx, ny, nz, m]` rows of `n . x <= m`
    /// in the design's own frame; `source` says where it came from (the Rough Planner's planned
    /// piece) and may be empty. A build older than 2026-10-12 does not know this kind and refuses
    /// the file.
    Planes {
        planes: Vec<[f64; 4]>,
        #[serde(default)]
        source: String,
    },
}

/// The `[preform]` table: a TOML mirror of the editor's own preform spec.
///
/// See the module doc comment for why `.asc` has no room for this. Reading one back
/// into an actual preform spec is `indicatrix-cut-core`'s job; building one from a
/// fresh design's preform spec is [`PreformTable::new`]'s.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PreformTable {
    /// Shape category of the stone.
    pub shape: NativePreformShape,
    /// Half the preform width. For a [`NativePreformShape::Planes`] shape this and the next
    /// two fields describe the set's bounding box and are informational: the planes decide
    /// the solid.
    pub half_width: f64,
    /// Preform length divided by its width.
    pub length_over_width: f64,
    /// Preform depth.
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

/// The `[material]` table: a TOML mirror of the editor's own material selection.
///
/// Building one from -- or reading one back into -- an actual material selection is
/// `indicatrix-cut-core`'s job, same as [`PreformTable`].
///
/// Reading also accepts the British spelling `body_colour_override`, which builds from
/// 2026-09-28 wrote for [`Self::body_color_override`]; writing always uses `body_color_override`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(from = "MaterialTableWire")]
pub struct MaterialTable {
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Specific gravity overriding the material default.
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
    /// The design's per-design body color, `body_color_override = [r, g, b]`: the
    /// absorption-RGB triple the editor applies on top of `name`'s own material
    /// (`None` = the material's own color). `#[serde(default)]` so a file written
    /// before this field existed loads with `None`, and omitted when unset so a
    /// design with no override never grows the key at all.
    ///
    /// Stored as `f64` (TOML's only float) even though the editor's triple is `f32`:
    /// `indicatrix-cut-core`'s converter writes each component's shortest
    /// round-trip decimal, so the file reads `[0.2, 0.4, 2.8]` rather than
    /// `[0.20000000298023224, ...]`, and loads back to the identical `f32` bits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_color_override: Option<[f64; 3]>,
    /// The N-band form of the per-design body color (path-aware L*C*h editor),
    /// `body_color_bands_override = [[centre_nm, width_nm, amplitude_per_mm], ...]`, written
    /// NEXT TO the nearest [`Self::body_color_override`] triple so a build that does not know
    /// the key (it keeps it in [`Self::unknown`] and writes it back) still shows a close
    /// colour. When present and non-empty the bands win on load. `#[serde(default)]`: a file
    /// written before this field existed loads with `None`; omitted when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_color_bands_override: Option<Vec<[f64; 3]>>,
    /// Millimetres per model unit the bands were solved for
    /// (`absorption_path_scale_override = 24.5`); only meaningful with
    /// [`Self::body_color_bands_override`]. Omitted when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absorption_path_scale_override: Option<f64>,
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
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
            unknown: toml::Table::new(),
        }
    }

    /// Attaches [`Self::body_color_bands_override`] and
    /// [`Self::absorption_path_scale_override`] -- see those fields' doc comments.
    #[must_use]
    pub fn with_body_color_bands_override(
        mut self,
        bands: Option<Vec<[f64; 3]>>,
        path_scale: Option<f64>,
    ) -> Self {
        self.absorption_path_scale_override = bands.as_ref().and(path_scale);
        self.body_color_bands_override = bands;
        self
    }

    /// Attaches [`Self::custom`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_custom(mut self, custom: Option<CustomMaterialSnapshot>) -> Self {
        self.custom = custom;
        self
    }

    /// Attaches [`Self::body_color_override`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_body_color_override(mut self, rgb: Option<[f64; 3]>) -> Self {
        self.body_color_override = rgb;
        self
    }
}

/// The key builds from 2026-09-28 wrote for `MaterialTable::body_color_override`, before
/// the identifier was respelled. Read for old files, never written.
const LEGACY_BODY_COLOUR_KEY: &str = "body_colour_override";

/// What [`MaterialTable`] is parsed through: the same keys, plus the old spelling of the
/// body-color key as a field of its own.
///
/// A plain `serde(alias)` would fail a file holding both spellings ("duplicate field"), and
/// leaving the old key to the catch-all `unknown` table would write it back next to the new
/// one. As a named field here, the old key is always consumed. When both spellings are
/// present, the new one wins.
#[derive(serde::Deserialize)]
struct MaterialTableWire {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    specific_gravity_override: Option<f64>,
    #[serde(default)]
    refractive_index_override: Option<f64>,
    #[serde(default)]
    custom: Option<CustomMaterialSnapshot>,
    #[serde(default)]
    body_color_override: Option<[f64; 3]>,
    #[serde(default)]
    body_color_bands_override: Option<Vec<[f64; 3]>>,
    #[serde(default)]
    absorption_path_scale_override: Option<f64>,
    /// Kept as a loose value so a malformed old key is parked in `unknown` untouched
    /// instead of failing the whole file.
    #[serde(default, rename = "body_colour_override")]
    legacy_body_colour_override: Option<toml::Value>,
    #[serde(flatten, default)]
    unknown: toml::Table,
}

impl From<MaterialTableWire> for MaterialTable {
    fn from(wire: MaterialTableWire) -> Self {
        let MaterialTableWire {
            name,
            specific_gravity_override,
            refractive_index_override,
            custom,
            mut body_color_override,
            body_color_bands_override,
            absorption_path_scale_override,
            legacy_body_colour_override,
            mut unknown,
        } = wire;
        if let Some(legacy) = legacy_body_colour_override {
            match legacy.clone().try_into::<[f64; 3]>() {
                Ok(rgb) => body_color_override = body_color_override.or(Some(rgb)),
                Err(_) => {
                    unknown.insert(LEGACY_BODY_COLOUR_KEY.to_owned(), legacy);
                }
            }
        }
        Self {
            name,
            specific_gravity_override,
            refractive_index_override,
            custom,
            body_color_override,
            body_color_bands_override,
            absorption_path_scale_override,
            unknown,
        }
    }
}

/// The `[source]` table: the printed/measured proportions a catalogue row supplied
/// when this design was loaded, if any.
///
/// Without it, Open would reset `printed_proportions`, disabling Deep
/// Solve for catalogue designs. Every field mirrors one
/// `indicatrix::geometry::stone_metrics::ExternalProportions` figure one-to-one;
/// building/reading the actual type is `indicatrix-cut-core`'s job (this crate has no
/// dependency on `indicatrix` at all -- see the crate doc comment's "only pulls in
/// what it needs" rule), same split as [`PreformTable`]/[`MaterialTable`]. Entirely
/// optional at the top level (`DesignFile::source`): a design that was never
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
    /// (checked before attaching one, see `DesignFile::with_source`).
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
    /// Entry names in order.
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

    /// `true` iff there is nothing worth writing at all (checked before attaching
    /// one, see `DesignFile::with_history`).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
