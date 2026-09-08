//! Query-time types for the catalogue's numeric range filters.
//!
//! Refractive index, L/W ratio, volume, facet count, the RI-tolerance and
//! tilt-performance filters that compose alongside them, and the actual data bounds
//! they're built against.

use crate::model::{entry::DiagramListItem, performance::PerformanceFilter};

/// Optional min/max bounds to apply to `search_diagrams`.
///
/// One independent pair per numeric attribute, plus the RI-tolerance,
/// ignored-inclusion, and tilt-performance filters that compose alongside them (see
/// each field's own doc comment for exactly how).
///
/// Each numeric bound is applied only when `Some`; a `None` bound leaves that side
/// unconstrained (and if *both* sides are `None`, no SQL predicate is added at all --
/// rows with no value for that attribute are still returned, matching the unfiltered
/// "All" behavior of the shape/gear dropdowns).
///
/// New fields default (via `#[derive(Default)]`) to "no additional filtering", so
/// `RangeFilter::default()` keeps meaning what it always did. A caller constructing an
/// exhaustive struct literal (not `..Default::default()`) needs updating for new
/// fields -- `apps/indicatrix-cut/src/gui/search.rs`'s `read_range_filter` is one such.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RangeFilter {
    pub ri_min: Option<f64>,
    pub ri_max: Option<f64>,
    pub lw_min: Option<f64>,
    pub lw_max: Option<f64>,
    pub volume_min: Option<f64>,
    pub volume_max: Option<f64>,
    pub facets_min: Option<i64>,
    pub facets_max: Option<i64>,
    /// `(center, tolerance)`: an additional refractive-index band, `[center -
    /// tolerance, center + tolerance]`, for the GUI's "match the currently loaded
    /// material's RI" filter -- a separate control from `ri_min`/`ri_max`, not a
    /// replacement. Composes with them by intersection (AND when both active), matching
    /// how every other pair of active filters in this struct behaves.
    pub ri_tolerance: Option<(f64, f64)>,
    /// `false` (default): designs marked `ignored` (`Database::set_diagram_ignored`)
    /// are excluded from every search. `true`: included like any other design.
    pub include_ignored: bool,
    /// Zero or more tilt-performance predicates (see `crate::model::performance`),
    /// combined by `AND`. Empty (default) means no performance filtering. A design with
    /// no stored tilt curves can never satisfy an active filter and is excluded exactly
    /// as if it failed the predicate -- see
    /// `Database::search_diagrams_with_performance_exclusions` for the count of those.
    pub performance: Vec<PerformanceFilter>,
}

impl RangeFilter {
    /// `true` when every bound and filter field is at its default (unfiltered) state.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.ri_min.is_none()
            && self.ri_max.is_none()
            && self.lw_min.is_none()
            && self.lw_max.is_none()
            && self.volume_min.is_none()
            && self.volume_max.is_none()
            && self.facets_min.is_none()
            && self.facets_max.is_none()
            && self.ri_tolerance.is_none()
            && !self.include_ignored
            && self.performance.is_empty()
    }
}

/// The result of `Database::search_diagrams_with_performance_exclusions`.
///
/// The normal search results plus how many otherwise-matching designs were dropped
/// because they have no stored tilt curves to test an active
/// [`RangeFilter::performance`] predicate against. A separate result type, not a change
/// to `search_diagrams`'s return type, so every existing caller keeps compiling and
/// behaving identically.
#[derive(Debug, Clone)]
pub struct PerformanceSearchResult {
    pub items: Vec<DiagramListItem>,
    /// How many designs matched every active filter *except* the performance ones, but
    /// were excluded for having no stored tilt curves -- `0` whenever
    /// `RangeFilter::performance` is empty. Does NOT count designs that have curves but
    /// genuinely failed the performance predicate -- those are ordinary non-matches.
    pub excluded_for_missing_curves: usize,
}

/// The *usable* min/max bounds for each range-filterable attribute -- drives the
/// sliders' scale in the UI.
///
/// As of the last time the catalogue was queried. The minimum is the real minimum; the
/// maximum is **not** the raw maximum but a robust percentile (see
/// `Database::get_attribute_ranges`/`RANGE_BOUND_PERCENTILE`) so a handful of
/// data-entry errors can't compress the rest of the catalogue into a sliver of the
/// slider's travel. Rows beyond this bound are not excluded from search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttributeRanges {
    pub ri: (f64, f64),
    pub lw_ratio: (f64, f64),
    pub volume: (f64, f64),
    pub facets: (i64, i64),
}
