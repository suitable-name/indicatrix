//! Wire counterparts of `indicatrix_vault`'s search/filter types: sort order and the
//! range/performance predicates a [`super::LibraryRequest::Search`] carries.

use serde::{Deserialize, Serialize};

/// Wire counterpart of `indicatrix_vault::db::sqlite::search::SortOrder` (the vault is
/// not a dependency of this crate).
///
/// `#[default]` is [`Self::CatalogueOrder`], matching the vault type's own default, so a
/// client that never sets a sort preference gets the long-standing `de.id ASC` behaviour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortOrderWire {
    /// Insertion order -- the long-standing default.
    #[default]
    CatalogueOrder,
    /// Case-insensitive title, A-Z.
    Title,
    /// Most recently created first.
    Newest,
    /// Most recently edited first.
    RecentlyEdited,
}

/// Wire counterpart of `indicatrix_vault::model::filter::RangeFilter`; a `None` bound is
/// unconstrained.
///
/// Kept separate so this crate's wire shapes stay independent of `indicatrix-vault`'s
/// internal representation -- `indicatrix-worker` maps between them (`from_range_wire`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RangeFilterWire {
    /// Lower bound on refractive index.
    pub ri_min: Option<f64>,
    /// Upper bound on refractive index.
    pub ri_max: Option<f64>,
    /// Lower bound on the length-to-width ratio.
    pub lw_min: Option<f64>,
    /// Upper bound on the length-to-width ratio.
    pub lw_max: Option<f64>,
    /// Lower bound on volume.
    pub volume_min: Option<f64>,
    /// Upper bound on volume.
    pub volume_max: Option<f64>,
    /// Lower bound on facet count.
    pub facets_min: Option<i64>,
    /// Upper bound on facet count.
    pub facets_max: Option<i64>,
    /// Wire counterpart of `indicatrix_vault::model::filter::RangeFilter::ri_tolerance`
    /// -- `(centre, tolerance)`.
    pub ri_tolerance: Option<(f64, f64)>,
    /// Wire counterpart of
    /// `indicatrix_vault::model::filter::RangeFilter::include_ignored`.
    pub include_ignored: bool,
    /// Wire counterpart of `indicatrix_vault::model::filter::RangeFilter::performance`.
    pub performance: Vec<PerformanceFilterWire>,
}

/// Wire counterpart of `indicatrix_vault::model::performance::PerformanceMetric`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PerformanceMetricWire {
    Brilliance,
    Extinction,
    Windowing,
}

/// Wire counterpart of `indicatrix_vault::model::performance::PerformanceBound`. Only
/// `PartialEq`, not `Eq`, since `AtMost`/`AtLeast` carry an [`f32`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PerformanceBoundWire {
    AtMost(f32),
    AtLeast(f32),
}

/// Wire counterpart of `indicatrix_vault::model::performance::PerformanceAggregate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PerformanceAggregateWire {
    Worst,
    Mean,
}

/// Wire counterpart of `indicatrix_vault::model::performance::PerformanceFilter`.
/// `tilt_radius_deg` is a plain unvalidated `f32`; `indicatrix-worker`'s
/// `from_range_wire` validates it via `PerformanceFilter::new`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PerformanceFilterWire {
    /// Which performance metric is ranked.
    pub metric: PerformanceMetricWire,
    /// Which side of the metric is bounded.
    pub bound: PerformanceBoundWire,
    /// Tilt radius in degrees the metric is evaluated over.
    pub tilt_radius_deg: f32,
    /// How the metric is aggregated over the tilt range.
    pub aggregate: PerformanceAggregateWire,
}

/// Wire counterpart of `indicatrix_vault::model::filter::AttributeRanges`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AttributeRangesWire {
    /// Observed (min, max) refractive index.
    pub ri: (f64, f64),
    /// Observed (min, max) length-to-width ratio.
    pub lw_ratio: (f64, f64),
    /// Observed (min, max) volume.
    pub volume: (f64, f64),
    /// Observed (min, max) facet count.
    pub facets: (i64, i64),
}
