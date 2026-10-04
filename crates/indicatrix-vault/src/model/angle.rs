use serde::{Deserialize, Serialize};

/// One tier's hand-recorded row from a design's angle-settings table.
///
/// Facet name, angle, index-wheel positions, and any notes -- the queryable, per-tier
/// child of [`crate::model::detail::FacetingDiagramDetail::angle_settings_table`], stored
/// in the `angle_settings` table (see `crate::db::sqlite::Database::save_diagram_detail`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AngleSetting {
    /// This row's position within its design's schedule, `0`-based -- what makes the
    /// schedule an ORDERED list rather than a bag of rows once round-tripped through
    /// SQL (`angle_settings.order_idx`).
    pub order_index: u32,
    /// The facet/tier name as recorded on the design sheet (e.g. `"P1"`, `"C3"`).
    pub facet: String,
    /// The cutting angle text as recorded (e.g. `"41.000000"`), kept as a display
    /// string rather than a parsed number -- see
    /// `crate::local::reconstruct_asc_schedule` for the one path that parses it.
    pub angle: String,
    /// The index-wheel position(s) text as recorded (e.g. `"0, 24, 48, 72"` or the
    /// real catalogue's hyphen-separated `"96-08-16-..."` form).
    pub index: String,
    /// Free-text notes for this tier, if any -- what the library search's free-text
    /// query matches against (see `crate::db::sqlite::search`'s "notes" third of its
    /// "title, designer or notes" search scope).
    pub notes: String,
    /// The concave tool's name for this tier (e.g. `"Ball 6mm"`), or `None` for a flat
    /// tier. Stored apart from [`Self::notes`] so the library search's notes match
    /// never sees tool text.
    pub tool: Option<String>,
    /// The formatted second line of this tier's cutting-sheet row (the tool's
    /// placement, ready to display), or `None` for a flat tier. Kept as the display
    /// string, like [`Self::angle`], so the library renders it without resolving tools.
    pub tool_line: Option<String>,
}
