//! Types for the tilt-performance search filters: "only X% windowing in a tilt radius
//! of Y%", and the brilliance/extinction equivalents.
//!
//! # Where the predicate actually runs: Rust, not SQL -- and why
//!
//! An earlier version limited [`PerformanceFilter`] to four selectable tilt radii
//! (+/-15/30/45/90 degrees) so the predicate could be answered by 36 precomputed SQL
//! columns (3 metrics x 4 radii x {min, max, mean}). Wrong: the feature's own phrasing --
//! "only x% windowing in a tilt radius of y%" -- makes both numbers inputs, and a fixed
//! ladder can't express an arbitrary radius. [`PerformanceFilter::tilt_radius_deg`] is
//! now a plain `f32`, validated to `0.0..=90.0` (see [`PerformanceFilter::new`]).
//!
//! An arbitrary radius also can't be precomputed per-R, so
//! [`crate::model::tilt_curves::TiltPerformanceCurves::matches_performance_filter`]
//! evaluates it directly in Rust over a decoded curve. Cost is negligible: the whole
//! corpus is 3,187 designs x 8,688 bytes = ~27.7 MB, and one filter against one decoded
//! curve is at most 4*181 = 724 `f32` comparisons.
//!
//! # SQL still does the narrowing -- just less of it
//!
//! `crate::db::sqlite::search::build_search_predicate` still applies the cheap
//! predicates in SQL first (text, shape, gear, RI, L/W, volume, facets, ignored). For a
//! [`PerformanceFilter`], exactly SIX derived columns survive from the old 36: the
//! global min/max of each metric across the full sweep (see
//! [`crate::model::tilt_curves::TiltPerformanceCurves::global_extremes`] and
//! [`global_extreme_column_name`]). A global extreme bounds every narrower window's same
//! extreme, making it a SOUND pruning predicate without decoding a BLOB -- see
//! [`PerformanceFilter::sound_sql_narrowing`] for why this only works for `Worst` (never
//! `Mean`, whose narrower-window value can sit anywhere between the global min and max).
//!
//! This narrowing is an optimization, never the source of truth: it may pass through a
//! candidate that fails the exact check, but must never exclude one that would have
//! passed -- see [`crate::db::sqlite::tests`]'s pruning-soundness test, which compares
//! the pruned result set against a brute-force scan. `crate::db::sqlite::search` always
//! re-checks exactly in Rust, so a narrowing bug can only cost performance, never
//! correctness; the soundness test exists to catch the wrongly-excludes direction.

use crate::model::tilt_curves::AxisTiltCurves;
use std::fmt;

/// The three tilt-performance metrics this crate stores curves and derived global
/// extremes for -- one column pair per variant (see [`global_extreme_column_name`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PerformanceMetric {
    Brilliance,
    Extinction,
    Windowing,
}

impl PerformanceMetric {
    /// Every variant, in a fixed order (schema generation, exhaustive tests) -- a plain
    /// array, not a `HashSet`, so order is never hasher-dependent.
    pub const ALL: [Self; 3] = [Self::Brilliance, Self::Extinction, Self::Windowing];

    /// The lowercase column-name fragment for this metric (e.g. `"brilliance"`), used by
    /// [`global_extreme_column_name`]. Not `Display`: a SQL identifier, not user-facing text.
    #[must_use]
    pub const fn column_key(self) -> &'static str {
        match self {
            Self::Brilliance => "brilliance",
            Self::Extinction => "extinction",
            Self::Windowing => "windowing",
        }
    }

    /// Selects this metric's sample array out of one axis -- the one place mapping
    /// [`AxisTiltCurves`] fields to [`PerformanceMetric`] variants, shared by
    /// `global_extremes` and `matches_performance_filter` so they can't disagree.
    #[must_use]
    pub const fn select(
        self,
        axis: &AxisTiltCurves,
    ) -> &[f32; crate::model::tilt_curves::TILT_CURVE_POINTS_PER_AXIS] {
        match self {
            Self::Brilliance => &axis.brilliance_pct,
            Self::Extinction => &axis.extinction_pct,
            Self::Windowing => &axis.windowing_pct,
        }
    }
}

/// Which extreme of a metric's global range [`global_extreme_column_name`] names --
/// `Min`/`Max` only (there is no "mean" column any more; see this module's doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Extreme {
    Min,
    Max,
}

impl Extreme {
    pub const ALL: [Self; 2] = [Self::Min, Self::Max];

    #[must_use]
    pub const fn column_key(self) -> &'static str {
        match self {
            Self::Min => "min",
            Self::Max => "max",
        }
    }
}

/// The `diagram_tilt_curves` column name storing `metric`'s global `extreme` (full
/// -90..+90 sweep, all 4 axes), e.g. `global_extreme_column_name(Windowing, Max)` ->
/// `"perf_windowing_global_max"`.
///
/// Single source of truth for that naming: both `crate::db::sqlite::migrations` and
/// `search::build_search_predicate` call this rather than hand-transcribing the name.
#[must_use]
pub fn global_extreme_column_name(metric: PerformanceMetric, extreme: Extreme) -> String {
    format!(
        "perf_{}_global_{}",
        metric.column_key(),
        extreme.column_key()
    )
}

/// Every `(metric, extreme)` pair [`global_extreme_column_name`] can name, in a fixed
/// order.
///
/// What `crate::db::sqlite::migrations` iterates to build the 6 derived columns. A
/// plain nested-array walk, not a `HashSet`, to stay order-stable.
pub fn all_global_extreme_columns() -> impl Iterator<Item = (PerformanceMetric, Extreme)> {
    PerformanceMetric::ALL
        .into_iter()
        .flat_map(|metric| Extreme::ALL.map(|extreme| (metric, extreme)))
}

/// One side of a tilt-performance threshold: "stays at most `T`%" or "stays at least
/// `T`%". Matches [`crate::model::tilt_curves::AxisTiltCurves`]'s 0..100-scale
/// percentage fields, not 0..1 fractions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PerformanceBound {
    AtMost(f32),
    AtLeast(f32),
}

/// How every sampled point within the tilt window (across all 4 axes) collapses into
/// the one value [`PerformanceBound`] compares against.
///
/// `Worst` is the value making the bound hardest to satisfy (window MAX for `AtMost`,
/// MIN for `AtLeast`); `Mean` is the flat average.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerformanceAggregate {
    Worst,
    Mean,
}

/// The lowest and highest sampled radius the tilt sweep stores (see
/// [`crate::model::tilt_curves`]'s module doc) -- [`PerformanceFilter::new`]'s valid range.
pub const MIN_TILT_RADIUS_DEG: f32 = 0.0;
pub const MAX_TILT_RADIUS_DEG: f32 = 90.0;

/// One tilt-performance predicate.
///
/// "Does `metric` stay `bound` across every sampled point (aggregated by `aggregate`)
/// within +/- `tilt_radius_deg` of TABLE-UP (face-up -- see `crate::model::tilt_curves`'s
/// "Table-up at the centre" doc), over all 4 axes?" -- e.g. "windowing never exceeds 20%
/// within +/-45 degrees" (`Windowing`, `AtMost(20.0)`, `45.0`, `Worst`).
///
/// Several compose by plain `AND` (see [`crate::model::filter::RangeFilter::performance`]);
/// no OR/NOT since nothing requires one.
///
/// # Constructing one
///
/// Fields are `pub`, but [`Self::new`] is the recommended constructor: it validates
/// `tilt_radius_deg` against [`MIN_TILT_RADIUS_DEG`]/[`MAX_TILT_RADIUS_DEG`] at the
/// boundary where external input (a GUI field, a value off the wire) becomes a
/// `PerformanceFilter`. A struct literal bypassing `new` with an out-of-range radius
/// won't panic -- the evaluator treats a negative radius as matching no points -- but
/// only `new`'s path guarantees the `0.0..=90.0` contract.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerformanceFilter {
    pub metric: PerformanceMetric,
    pub bound: PerformanceBound,
    pub tilt_radius_deg: f32,
    pub aggregate: PerformanceAggregate,
}

impl PerformanceFilter {
    /// Builds a filter, rejecting a `tilt_radius_deg` outside
    /// [`MIN_TILT_RADIUS_DEG`]..=[`MAX_TILT_RADIUS_DEG`] (`0.0..=90.0`) -- including
    /// `NaN`, since `RangeInclusive::contains` already treats it as out of range.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidTiltRadius`] carrying the rejected value.
    pub fn new(
        metric: PerformanceMetric,
        bound: PerformanceBound,
        tilt_radius_deg: f32,
        aggregate: PerformanceAggregate,
    ) -> Result<Self, InvalidTiltRadius> {
        if !(MIN_TILT_RADIUS_DEG..=MAX_TILT_RADIUS_DEG).contains(&tilt_radius_deg) {
            return Err(InvalidTiltRadius {
                degrees: tilt_radius_deg,
            });
        }
        Ok(Self {
            metric,
            bound,
            tilt_radius_deg,
            aggregate,
        })
    }

    /// The SQL-level narrowing predicate this filter's `Worst` aggregate makes sound --
    /// `None` for `Mean` (see this module's doc for why mean can't be pruned this way).
    ///
    /// Returns `(column, comparator, threshold)`: a row failing `column {comparator}
    /// threshold` can be excluded with certainty. A row passing it is not necessarily a
    /// true match -- `crate::db::sqlite::search` always re-checks exactly in Rust.
    ///
    /// The two cases:
    /// - `AtMost(T)`: if even the GLOBAL min already exceeds `T`, every point in every
    ///   window does too, so the row can be excluded outright. Kept: `global_min <= T`.
    /// - `AtLeast(T)`: mirror image. If the GLOBAL max already falls short of `T`, no
    ///   window's min can reach `T` either. Kept: `global_max >= T`.
    #[must_use]
    pub fn sound_sql_narrowing(&self) -> Option<(String, &'static str, f32)> {
        match self.aggregate {
            PerformanceAggregate::Mean => None,
            PerformanceAggregate::Worst => Some(match self.bound {
                PerformanceBound::AtMost(t) => (
                    global_extreme_column_name(self.metric, Extreme::Min),
                    "<=",
                    t,
                ),
                PerformanceBound::AtLeast(t) => (
                    global_extreme_column_name(self.metric, Extreme::Max),
                    ">=",
                    t,
                ),
            }),
        }
    }
}

/// The error [`PerformanceFilter::new`] returns for an out-of-range `tilt_radius_deg`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InvalidTiltRadius {
    pub degrees: f32,
}

impl fmt::Display for InvalidTiltRadius {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}\u{b0} is not a valid tilt-performance radius; must be within \
             {MIN_TILT_RADIUS_DEG}\u{b0}..={MAX_TILT_RADIUS_DEG}\u{b0}",
            self.degrees
        )
    }
}

impl std::error::Error for InvalidTiltRadius {}

/// One metric's minimum and maximum sampled value across a design's entire tilt sweep
/// (-90..+90, all 4 axes) -- what the `perf_*_global_min`/`perf_*_global_max` columns
/// store.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlobalExtremes {
    pub min: f32,
    pub max: f32,
}

/// Every metric's [`GlobalExtremes`] for one design -- what
/// `crate::db::sqlite::Database::save_tilt_curves` writes into the 6 derived columns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerformanceGlobalExtremes {
    pub brilliance: GlobalExtremes,
    pub extinction: GlobalExtremes,
    pub windowing: GlobalExtremes,
}

impl PerformanceGlobalExtremes {
    /// Looks up one metric's [`GlobalExtremes`] -- called once per
    /// [`all_global_extreme_columns`] pair to fill each of the 6 derived columns.
    #[must_use]
    pub const fn get(&self, metric: PerformanceMetric) -> GlobalExtremes {
        match metric {
            PerformanceMetric::Brilliance => self.brilliance,
            PerformanceMetric::Extinction => self.extinction,
            PerformanceMetric::Windowing => self.windowing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn new_accepts_the_full_closed_range() {
        for r in [0.0_f32, 0.001, 22.7, 45.0, 89.999, 90.0] {
            assert!(
                PerformanceFilter::new(
                    PerformanceMetric::Windowing,
                    PerformanceBound::AtMost(20.0),
                    r,
                    PerformanceAggregate::Worst,
                )
                .is_ok(),
                "{r} should be a valid radius"
            );
        }
    }

    #[test]
    fn new_rejects_out_of_range_and_nan() {
        for r in [-0.001_f32, -45.0, 90.001, 180.0, f32::NAN, f32::INFINITY] {
            let err = PerformanceFilter::new(
                PerformanceMetric::Windowing,
                PerformanceBound::AtMost(20.0),
                r,
                PerformanceAggregate::Worst,
            )
            .unwrap_err();
            if r.is_nan() {
                assert!(err.degrees.is_nan());
            } else {
                assert_eq!(err.degrees, r);
            }
        }
    }

    #[test]
    fn all_global_extreme_columns_are_unique_and_number_six() {
        let names: Vec<String> = all_global_extreme_columns()
            .map(|(m, e)| global_extreme_column_name(m, e))
            .collect();
        assert_eq!(names.len(), 6, "3 metrics x 2 extremes");
        let unique: HashSet<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(
            unique.len(),
            6,
            "every generated column name must be unique"
        );
        assert!(names.contains(&"perf_windowing_global_max".to_string()));
    }

    #[test]
    fn sound_sql_narrowing_uses_global_min_for_at_most_and_global_max_for_at_least() {
        let at_most = PerformanceFilter::new(
            PerformanceMetric::Windowing,
            PerformanceBound::AtMost(20.0),
            45.0,
            PerformanceAggregate::Worst,
        )
        .unwrap();
        assert_eq!(
            at_most.sound_sql_narrowing(),
            Some(("perf_windowing_global_min".to_string(), "<=", 20.0))
        );

        let at_least = PerformanceFilter::new(
            PerformanceMetric::Brilliance,
            PerformanceBound::AtLeast(60.0),
            30.0,
            PerformanceAggregate::Worst,
        )
        .unwrap();
        assert_eq!(
            at_least.sound_sql_narrowing(),
            Some(("perf_brilliance_global_max".to_string(), ">=", 60.0))
        );
    }

    #[test]
    fn sound_sql_narrowing_is_none_for_mean_regardless_of_bound_direction() {
        let mean_at_most = PerformanceFilter::new(
            PerformanceMetric::Extinction,
            PerformanceBound::AtMost(10.0),
            90.0,
            PerformanceAggregate::Mean,
        )
        .unwrap();
        assert_eq!(mean_at_most.sound_sql_narrowing(), None);

        let mean_at_least = PerformanceFilter::new(
            PerformanceMetric::Extinction,
            PerformanceBound::AtLeast(10.0),
            90.0,
            PerformanceAggregate::Mean,
        )
        .unwrap();
        assert_eq!(mean_at_least.sound_sql_narrowing(), None);
    }
}
