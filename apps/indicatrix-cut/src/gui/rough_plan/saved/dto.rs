//! The serde documents of a saved rough plan (the TOML payload).
//!
//! These types mirror the file one to one and carry no rules; [`super::convert`]
//! turns them into core types and checks every number, [`super::format`] reads and
//! writes whole documents.

use serde::{Deserialize, Serialize};

/// The expected format identifier in the header of a plan file.
pub const ROUGH_PLAN_FORMAT: &str = "indicatrix-rough-plan";

/// The newest schema version this build reads. A plan with a non-convex mesh rough is
/// written with this version; every other plan is written with [`BASE_SCHEMA_VERSION`], so
/// the plans of convex roughs are byte for byte what they always were.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

/// The schema version that introduced the `rough.mesh` table.
pub const MESH_SCHEMA_VERSION: u32 = 2;

/// The schema version of a plan without a mesh rough (version 1; also the oldest this
/// build reads).
pub const BASE_SCHEMA_VERSION: u32 = 1;

/// The largest plan document that is parsed, in bytes. A plan with 99 stones in ten
/// layouts is well under 1 MiB; anything near this limit is not a plan.
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

/// Most layouts a plan holds: the planner's final list has ten.
pub const MAX_LAYOUTS: usize = 10;

/// Longest plan name, in characters.
pub const MAX_NAME_CHARS: usize = 200;

/// Most stones in a layout, which is the planner's stone-count limit.
pub const MAX_STONES_PER_LAYOUT: usize = 99;

/// Most designs a plan lists: ten layouts of 99 stones, every stone of another design.
pub const MAX_DESIGNS: usize = MAX_LAYOUTS * MAX_STONES_PER_LAYOUT;

/// Most slabs in a layout, bars in a slab and pieces in a bar. Every piece holds a
/// stone, so none of them can pass the stone limit.
pub const MAX_SAW_ITEMS: usize = 99;

/// Longest kerf, allowance or skin the planner form accepts, in mm.
pub const MAX_LOSS_MM: f64 = 50.0;

/// Longest minimum stone width the planner form accepts, in mm.
pub const MAX_MIN_WIDTH_MM: f64 = 1000.0;

/// The first keys of a plan file, read before anything else so a wrong or newer file
/// gets its own message instead of a complaint about a missing field.
#[derive(Debug, Default, Deserialize)]
pub struct HeaderDto {
    /// Format identifier; must equal [`ROUGH_PLAN_FORMAT`].
    #[serde(default)]
    pub format: Option<String>,
    /// Schema version; must be `1..=CURRENT_SCHEMA_VERSION`.
    #[serde(default)]
    pub version: Option<i64>,
}

/// The root TOML document for an exported or database-persisted rough plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedPlanDto {
    /// Format identifier; equals [`ROUGH_PLAN_FORMAT`].
    pub format: String,
    /// Schema version.
    pub version: u32,
    /// User-assigned label or name.
    pub name: String,
    /// Unix timestamp (seconds) when this plan was created.
    pub created_at: i64,
    /// Identity stamp of the library whose entry ids the plan uses (absent in older
    /// files). Equal to the opening library's stamp, the ids are trusted; otherwise each
    /// design is checked by title as well.
    #[serde(default)]
    pub library_id: Option<u32>,
    /// The modelled rough geometry, material, and optional weight.
    pub rough: RoughDto,
    /// Sawing losses and limits used when calculating the plan.
    pub settings: SettingsDto,
    /// Metadata and shape fingerprints for every design used in the layouts.
    #[serde(default)]
    pub designs: Vec<SavedDesignDto>,
    /// One or more saved cutting layouts with full stone poses.
    #[serde(default)]
    pub layouts: Vec<SavedLayoutDto>,
}

/// Serialized rough geometry and material info.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoughDto {
    /// Base shape: "block", "cylinder", "pebble" or "hull".
    pub base: String,
    /// Extent along X in mm (block, pebble).
    #[serde(default)]
    pub x_mm: Option<f64>,
    /// Extent along Y in mm (block, pebble).
    #[serde(default)]
    pub y_mm: Option<f64>,
    /// Extent along Z in mm (block, pebble).
    #[serde(default)]
    pub z_mm: Option<f64>,
    /// Diameter in mm (cylinder).
    #[serde(default)]
    pub diameter_mm: Option<f64>,
    /// Cylinder length along its axis in mm.
    #[serde(default)]
    pub length_mm: Option<f64>,
    /// Cylinder axis: "x", "y", or "z".
    #[serde(default)]
    pub axis: Option<String>,
    /// Corners in mm of an imported mesh's convex outline (hull).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hull: Vec<[f64; 3]>,
    /// The closed triangle mesh of a non-convex imported rough (schema version 2). The
    /// hull is derived from it, so `hull` is empty then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<MeshDto>,
    /// Material name (e.g. "Quartz").
    pub material: String,
    /// Material specific gravity.
    pub specific_gravity: f64,
    /// Optional weighed carat check value.
    #[serde(default)]
    pub weighed_ct: Option<f64>,
    /// Planar cuts modifying the base shape.
    #[serde(default)]
    pub cuts: Vec<CutDto>,
}

/// A rough's triangle mesh: vertices in mm and triangles as 0-based vertex indices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshDto {
    /// Vertex positions in mm.
    pub vertices: Vec<[f64; 3]>,
    /// Triangles as indices into `vertices`.
    pub triangles: Vec<[u32; 3]>,
}

/// Serialized planar cut.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CutDto {
    /// Cut kind: "edge", "corner", or "face".
    pub kind: String,
    /// Faces involved (for edge or corner).
    #[serde(default)]
    pub faces: Option<Vec<String>>,
    /// Setbacks in mm along the faces (for edge or corner).
    #[serde(default)]
    pub setbacks_mm: Option<Vec<f64>>,
    /// Outward normal of a face cut.
    #[serde(default)]
    pub normal: Option<[f64; 3]>,
    /// Depth of a face cut from the base's outermost point in that direction, in mm.
    #[serde(default)]
    pub depth_mm: Option<f64>,
}

/// Planning settings and kerf losses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsDto {
    /// Maximum stone count (`1..=99`); read as a wide integer so a wrong value is
    /// reported instead of failing the parse.
    pub count: i64,
    /// Saw blade kerf width in mm.
    pub kerf_mm: f64,
    /// Preform allowance per side in mm.
    pub allowance_mm: f64,
    /// Rough skin thickness in mm.
    pub skin_mm: f64,
    /// Minimum stone width in mm.
    pub min_width_mm: f64,
    /// Where the candidate designs came from: "filter" (the library filter) or "library"
    /// (every design). Absent in older files, which mean "filter".
    #[serde(default)]
    pub candidate_source: Option<String>,
}

/// Design identifier and geometric fingerprint for staleness detection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedDesignDto {
    /// Design catalogue entry ID.
    pub entry_id: i64,
    /// Authored design title.
    pub title: String,
    /// Ratios `[L/W, H/W, V/W^3]` from the design extents cache; all zero means unknown.
    pub fingerprint: [f64; 3],
    /// The design's width (the smaller caliper extent) in model units, which the ratios
    /// alone cannot tell: a design drawn twice as large has the same ratios. Absent in
    /// older files and when the design was not measured; the size is then not compared.
    #[serde(default)]
    pub width_caliper: Option<f64>,
    /// The measuring rule version the figures come from (`SOLID_EXTENTS_VERSION`);
    /// 0 when unknown. Figures of another rule version cannot be compared.
    #[serde(default)]
    pub extents_version: u32,
}

/// The shape a design had when a plan measured it: what the staleness check compares.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DesignShape {
    /// `[L/W, H/W, V/W^3]`; all zero means unknown.
    pub fingerprint: [f64; 3],
    /// The width in model units, when known.
    pub width_caliper: Option<f64>,
    /// The measuring rule version; 0 when unknown.
    pub extents_version: u32,
}

impl SavedDesignDto {
    /// The shape this record stores.
    #[must_use]
    pub const fn shape(&self) -> DesignShape {
        DesignShape {
            fingerprint: self.fingerprint,
            width_caliper: self.width_caliper,
            extents_version: self.extents_version,
        }
    }
}

/// One saved cutting layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedLayoutDto {
    /// Rank of this layout in the plan it came from (1-based).
    pub rank: i64,
    /// Saw cutting order: "xyz", "yxz", etc.
    pub cut_order: String,
    /// Total finished carat yield.
    pub total_carat: f64,
    /// Total finished stone volume in mm^3.
    pub total_volume_mm3: f64,
    /// Volume yield fraction relative to the modelled rough.
    pub yield_fraction: f64,
    /// Staged saw cut slabs and bars.
    #[serde(default)]
    pub slabs: Vec<SlabDto>,
    /// Placed stone details including 3D poses.
    #[serde(default)]
    pub stones: Vec<StoneDto>,
    /// Whether the layout is an exact single-stone fit (no saw stages). Absent in files
    /// written before the marker existed, which read as a sawn layout.
    #[serde(default)]
    pub exact_fit: bool,
}

/// Saw cut slab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlabDto {
    /// Thickness along the stage-1 axis in mm.
    pub thickness_mm: f64,
    /// Bars within this slab.
    #[serde(default)]
    pub bars: Vec<BarDto>,
}

/// Saw cut bar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BarDto {
    /// Width along the stage-2 axis in mm.
    pub width_mm: f64,
    /// Piece lengths along the stage-3 axis in mm.
    #[serde(default)]
    pub pieces_mm: Vec<f64>,
}

/// Placed finished stone within the rough.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoneDto {
    /// Catalogue design entry ID.
    pub entry_id: i64,
    /// Minimum corner of the sawn piece in mm.
    pub piece_origin_mm: [f64; 3],
    /// Sawn piece bounding box in mm.
    pub piece_size_mm: [f64; 3],
    /// Finished stone bounding box in the rough's axes in mm.
    pub stone_size_mm: [f64; 3],
    /// The rough axis the stone's table faces: "x", "y", or "z".
    pub table_axis: String,
    /// Finished stone carat.
    pub carat: f64,
    /// Finished stone volume in mm^3.
    pub volume_mm3: f64,
    /// Caliper-frame origin in rough coordinates in mm.
    pub center_mm: [f64; 3],
    /// Caliper-frame basis unit vectors in rough coordinates.
    pub axes: [[f64; 3]; 3],
    /// Millimetres per model unit.
    pub mm_per_unit: f64,
}

/// Just enough of a plan to describe it in the saved list.
#[derive(Debug, Deserialize)]
pub struct SummaryDto {
    /// The rough's base and material.
    pub rough: SummaryRoughDto,
    /// The saved layouts; only counted.
    #[serde(default)]
    pub layouts: Vec<serde::de::IgnoredAny>,
}

/// The two rough fields the list summary shows.
#[derive(Debug, Deserialize)]
pub struct SummaryRoughDto {
    /// Base shape name.
    pub base: String,
    /// Material name.
    pub material: String,
}
