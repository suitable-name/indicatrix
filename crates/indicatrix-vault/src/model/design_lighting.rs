//! A design's own lighting choice.
//!
//! Kept in the local library keyed by the design's UUID (see [`super::design_key`]).
//!
//! See `Database::set_design_lighting`, `design_lighting` and `clear_design_lighting` for
//! the storage side.

/// The lighting a design was last shown under.
///
/// The library does not know what the settings mean: `settings_json` is text the
/// application writes and reads back, so the lighting model can change without a
/// database migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesignLighting {
    /// The name of the lighting preset the settings came from.
    pub preset_name: String,
    /// The lighting settings, as text the application understands.
    pub settings_json: String,
    /// When the choice was stored, in Unix seconds.
    pub updated_at: i64,
}
