//! Rows of the rough-colour side tables (`zoning` feature only).
//!
//! The vault never parses these payloads: `zoned_json` / `fit_json` are opaque JSON text and
//! the photo `data` is an opaque blob whose layout `encoding` names. The application
//! (`indicatrix-cut`) owns every format, so a change to one needs no migration here.

/// The fitted rough colour of one saved rough plan (`rough_colour`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoughColourRow {
    /// The saved rough plan this colour belongs to (`saved_rough_plans.plan_id`).
    pub plan_id: i64,
    /// The zoned absorption in the rough frame, as JSON text.
    pub zoned_json: String,
    /// The fit report (model comparison, uncertainty, leave-one-view-out), as JSON text.
    pub fit_json: String,
    /// The application's format version of both texts.
    pub version: u32,
    /// Unix seconds when the colour was stored.
    pub created: i64,
}

/// One cached working-resolution photo image of a plan (`rough_colour_photo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoughColourPhotoRow {
    /// The saved rough plan.
    pub plan_id: i64,
    /// The rig view the image belongs to.
    pub view: u32,
    /// What the image is (the application's name for it, for example `transmittance`).
    pub kind: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The layout of `data` (for example `f16le`, `f32le`, `u8`).
    pub encoding: String,
    /// The pixel bytes.
    pub data: Vec<u8>,
}

/// A photo row without its bytes, for listings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoughColourPhotoMeta {
    /// The saved rough plan.
    pub plan_id: i64,
    /// The rig view.
    pub view: u32,
    /// The application's name for the image.
    pub kind: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The layout of the bytes.
    pub encoding: String,
    /// Size of the stored bytes.
    pub byte_len: u64,
}

/// The pose a planned stone was assigned (`stone_pose_choice`): which of the box-symmetric
/// poses of the stone to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct StonePoseChoiceRow {
    /// The result (layout) of the plan, in the plan's own order.
    pub layout_index: u32,
    /// The stone within that layout.
    pub stone_index: u32,
    /// The pose index (`0` is the canonical pose the planner placed).
    pub pose: u8,
}

/// The zones of one custom material (`material_zoning`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialZoningRow {
    /// The custom material's name (`custom_gem_materials.name`).
    pub material_name: String,
    /// The zoned absorption as JSON text.
    pub zoned_json: String,
    /// Whether the zone geometry scales with the stone's width (a library zoned material)
    /// rather than being fixed in millimetres (a material adopted from a rough).
    pub relative_to_stone: bool,
    /// The application's format version of `zoned_json`.
    pub version: u32,
}
