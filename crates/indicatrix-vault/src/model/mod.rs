/// One tier's hand-recorded facet/angle/index/notes row -- see
/// [`angle::AngleSetting`].
pub mod angle;
/// A design's full detail row and the child records it carries -- see
/// [`detail::FacetingDiagramDetail`].
pub mod detail;
/// A catalogue entry and its display-facing/full-record shapes -- see
/// [`entry::FacetingDiagramEntry`].
pub mod entry;
/// Parsing for the scraped `facets_count` field -- see [`facets::parse_facets_count`].
pub mod facets;
/// One attachment's bytes plus its display-only metadata counterpart -- see
/// [`file::AttachedFile`].
pub mod file;
/// The library search/filter surface's range and tilt-performance filter bundle -- see
/// [`filter::RangeFilter`].
pub mod filter;
/// A custom (user-authored) gem material's stored parameters.
pub mod material;
/// Refractive-index preset matching for preview-material selection -- see
/// [`material_match::pick_ri_preset`].
pub mod material_match;
/// The hand-correctable subset of a design's metadata -- see
/// [`metadata_update::MetadataUpdate`].
pub mod metadata_update;
/// Pull-mirror sync state for a design synced from a remote library.
///
/// See [`mirror::MirrorState`].
pub mod mirror;
/// Types for the tilt-performance search filters.
///
/// See [`performance::PerformanceFilter`].
pub mod performance;
/// Cached preview-render state -- see [`preview::PreviewImages`].
pub mod preview;
/// Saved rough plans -- see [`saved_rough_plan::SavedRoughPlan`].
pub mod saved_rough_plan;
/// Cached finished-solid extents for the Rough Planner -- see
/// [`solid_extents::SolidExtents`].
pub mod solid_extents;
/// Cached finished-solid convex hull vertices for the Rough Planner -- see
/// [`solid_hull::SolidHull`].
pub mod solid_hull;
/// A flat catalogue tag.
pub mod tag;
/// Packed tilt-performance sweep curves -- see
/// [`tilt_curves::TiltPerformanceCurves`].
pub mod tilt_curves;
