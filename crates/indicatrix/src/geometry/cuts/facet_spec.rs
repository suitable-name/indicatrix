//! [`FacetSpec`]: a UI-toolkit-agnostic row of a GemCAD-style cutting
//! schedule.

/// A single row of a GemCAD-style cutting schedule: one facet's angle, index
/// position(s), and any notes.
///
/// Exactly as scraped/stored (plain strings, no numeric parsing done yet -- see
/// `parse_angle_deg` / `parse_girdle_facet_count` below for the lenient parsing of
/// these fields).
///
/// This is a plain, UI-toolkit-agnostic type so that callers (e.g. a Slint-based
/// viewer) can convert their own generated row type into this one at the call
/// site, keeping this crate free of any UI dependency.
#[derive(Debug, Clone, Default)]
pub struct FacetSpec {
    /// The facet's name/label as scraped (e.g. `"C1"`, `"T"`, `"culet"`).
    pub facet: String,
    /// The facet's angle, as scraped (unparsed; see `parse_angle_deg`).
    pub angle: String,
    /// The facet's index position(s), as scraped (unparsed).
    pub index: String,
    /// Any free-text notes attached to this row.
    pub notes: String,
}
