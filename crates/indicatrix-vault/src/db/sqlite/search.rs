use super::Database;
use crate::model::filter::{AttributeRanges, PerformanceSearchResult, RangeFilter};
use anyhow::{Context, Result};
use std::fmt::Write as _;
use tracing::warn;

/// The percentile used as the range-filter sliders' *usable* upper bound (see
/// `Database::get_attribute_ranges`). All four range-filterable attributes share the
/// same long-right-tail shape in the real catalogue, so one percentile applies
/// uniformly.
const RANGE_BOUND_PERCENTILE: f64 = 99.0;

/// A raw maximum more than this many times the derived bound is treated as a probable
/// data-quality outlier worth logging (see `warn_if_outlier`). Measured p99-to-max
/// ratios for genuine long-tail attributes stay under ~3x; the one confirmed data error
/// was ~200x, so 5x sits between "wide but real" and "obviously wrong".
const OUTLIER_WARNING_RATIO: f64 = 5.0;

impl Database {
    /// Returns the total number of diagram entries stored.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `COUNT` query fails (e.g. a broken or
    /// inaccessible database) -- previously swallowed into a silent `0`, which made a
    /// broken database indistinguishable from a genuinely empty one.
    pub fn get_total_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM diagram_entries", [], |r| r.get(0))
            .context("Failed to count diagram_entries")?;
        Ok(count as usize)
    }

    /// Returns the union of the seeded `shape_vocabulary` (see `DEFAULT_SHAPES`) and
    /// every distinct non-empty `shape` value actually present in `diagram_details`,
    /// deduplicated, alphabetically sorted across the union (not canonical-list-first)
    /// so scraped shapes don't leave an arbitrary subset of the dropdown out of order.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying `SELECT` query fails. A
    /// row whose value fails to decode as a `String` is silently skipped.
    pub fn get_unique_shapes(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT name FROM (
                 SELECT name FROM shape_vocabulary
                 UNION
                 SELECT shape AS name FROM diagram_details WHERE shape IS NOT NULL AND shape != ''
             ) ORDER BY name ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        let mut shapes = Vec::new();
        for s in rows.flatten() {
            shapes.push(s);
        }
        Ok(shapes)
    }

    /// Returns every distinct non-empty `index_gear` value across all diagram details,
    /// numerically sorted. `index_gear` is stored as INTEGER (see
    /// `migrate_numeric_columns`) and cast back to TEXT, since every caller treats gear
    /// as a display/dropdown string, not a number.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying `SELECT DISTINCT` query
    /// fails. A row whose value fails to decode as a `String` is silently skipped.
    pub fn get_unique_gears(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT CAST(index_gear AS TEXT) FROM diagram_details WHERE index_gear IS NOT NULL ORDER BY index_gear ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        let mut gears = Vec::new();
        for g in rows.flatten() {
            gears.push(g);
        }
        Ok(gears)
    }

    /// Returns *usable* min/max bounds across the catalogue for each range-filterable
    /// attribute (refractive index, L/W ratio, volume, facet count) -- the scale the
    /// range-filter sliders are sized against.
    ///
    /// The lower bound is the real minimum. The upper bound is
    /// [`RANGE_BOUND_PERCENTILE`], not the raw maximum: real scraped data has
    /// occasional single-row data-entry errors (e.g. a `volume` of `195` when `vol/w³`
    /// cannot physically exceed ~1.8) that would otherwise compress 99%+ of the
    /// catalogue into a sliver of the slider's travel. A raw maximum far beyond the
    /// percentile bound is only logged, never altered or dropped.
    ///
    /// Returns `(0.0, 0.0)` / `(0, 0)` for an attribute with no non-null values.
    ///
    /// # Errors
    ///
    /// Returns an error if any of the underlying per-attribute queries fail.
    pub fn get_attribute_ranges(&self) -> Result<AttributeRanges> {
        Ok(AttributeRanges {
            ri: self.attribute_bounds_f64("refractive_index")?,
            lw_ratio: self.attribute_bounds_f64("lw_ratio")?,
            volume: self.attribute_bounds_f64("volume")?,
            facets: self.attribute_bounds_i64("facets")?,
        })
    }

    /// Every non-null value of `diagram_details.{column}`, sorted ascending. `column`
    /// is always one of this module's own hardcoded field names (never user input), so
    /// building the query via `format!` is safe here.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying `SELECT` fails. A row
    /// that fails to decode is silently skipped.
    fn sorted_non_null_reals(&self, column: &str) -> Result<Vec<f64>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {column} FROM diagram_details WHERE {column} IS NOT NULL ORDER BY {column} ASC"
        ))?;
        let rows = stmt.query_map([], |r| r.get::<_, f64>(0))?;
        Ok(rows.flatten().collect())
    }

    /// Integer counterpart of [`Self::sorted_non_null_reals`] -- `facets` is stored as
    /// `INTEGER`, and rusqlite's `f64` decoder doesn't auto-widen an `INTEGER` column,
    /// so it needs its own query.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying `SELECT` fails. A row
    /// that fails to decode is silently skipped.
    fn sorted_non_null_ints(&self, column: &str) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {column} FROM diagram_details WHERE {column} IS NOT NULL ORDER BY {column} ASC"
        ))?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        Ok(rows.flatten().collect())
    }

    /// `(min, usable_max)` for a `REAL` attribute column -- see
    /// [`Self::get_attribute_ranges`]'s doc comment for what "usable" means. Logs a
    /// warning (does not error, and touches no data) when the raw maximum sits far
    /// beyond the derived bound, since that gap is the signature of a data-entry error
    /// rather than a legitimately wide distribution.
    ///
    /// # Errors
    ///
    /// Returns an error if [`Self::sorted_non_null_reals`] fails.
    fn attribute_bounds_f64(&self, column: &str) -> Result<(f64, f64)> {
        let values = self.sorted_non_null_reals(column)?;
        let (Some(&min), Some(&raw_max)) = (values.first(), values.last()) else {
            return Ok((0.0, 0.0));
        };
        let bound_max = percentile_of_sorted(&values, RANGE_BOUND_PERCENTILE).max(min);

        warn_if_outlier(column, raw_max, bound_max);
        Ok((min, bound_max))
    }

    /// Integer counterpart of [`Self::attribute_bounds_f64`], for `facets` (`INTEGER`).
    /// The percentile is computed in `f64` (the same interpolated method as the REAL
    /// columns) and rounded up, so the bound never sits inside a facet count no design
    /// actually has.
    ///
    /// # Errors
    ///
    /// Returns an error if [`Self::sorted_non_null_ints`] fails.
    fn attribute_bounds_i64(&self, column: &str) -> Result<(i64, i64)> {
        let values = self.sorted_non_null_ints(column)?;
        let (Some(&min), Some(&raw_max)) = (values.first(), values.last()) else {
            return Ok((0, 0));
        };
        let floats: Vec<f64> = values.iter().map(|&v| v as f64).collect();
        let bound_max =
            (percentile_of_sorted(&floats, RANGE_BOUND_PERCENTILE).ceil() as i64).max(min);

        warn_if_outlier(column, raw_max as f64, bound_max as f64);
        Ok((min, bound_max))
    }

    /// Searches diagram entries by free-text `query` (matched against title, designer,
    /// and design ID) with optional exact `shape_filter`/`gear_filter` and optional
    /// min/max bounds on refractive index, L/W ratio, volume, and facet count (`range`;
    /// any bound left `None` is unconstrained -- see [`RangeFilter`]). Capped at
    /// [`SEARCH_RESULT_CAP`] results ordered by entry ID.
    ///
    /// `range` also carries the RI-tolerance band, ignored-inclusion opt-in, and
    /// tilt-performance predicates -- see that struct's field docs for how each
    /// composes. A design with `ignored = 1` is excluded unless
    /// `range.include_ignored`; every filter in `range.performance` must be satisfied,
    /// and a design with no stored tilt curves can never satisfy one (see
    /// [`Self::search_diagrams_with_performance_exclusions`] for the count of those).
    ///
    /// Exactly `self.search_diagrams_page(query, shape_filter, gear_filter, range,
    /// None, SEARCH_RESULT_CAP)`, kept as its own method so existing call sites keep
    /// working unchanged. See [`Self::search_diagrams_page`] for the keyset-paginated
    /// form a mirror walking the whole catalogue needs.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the assembled `SELECT` query fails,
    /// or if a row fails to decode into a `DiagramListItem`.
    pub fn search_diagrams(
        &self,
        query: &str,
        shape_filter: &str,
        gear_filter: &str,
        range: &RangeFilter,
    ) -> Result<Vec<crate::model::entry::DiagramListItem>> {
        self.search_diagrams_page(
            query,
            shape_filter,
            gear_filter,
            range,
            None,
            SEARCH_RESULT_CAP,
        )
    }

    /// Keyset-paginated counterpart of [`Self::search_diagrams`]: the same filters,
    /// plus `after_id` and `limit`.
    ///
    /// `after_id` is `None` for the first page, or `Some(id)` of the last row a
    /// previous page returned, to continue strictly after it. Ordered `de.id ASC` over
    /// `INTEGER PRIMARY KEY AUTOINCREMENT`, unique and strictly increasing, so paging
    /// by `id > after_id` is safe -- unlike `OFFSET`, it never skips or duplicates a
    /// row when rows are inserted mid-walk, and stays O(page size) per page.
    ///
    /// A page shorter than `limit` (including empty) means every matching row has now
    /// been seen; exactly `limit` long means there may be more -- the caller
    /// re-requests with `after_id` set to the last row's `id`.
    ///
    /// # When `range.performance` is non-empty: this method internally re-pages
    ///
    /// A [`crate::model::performance::PerformanceFilter`]'s exact test only runs once a
    /// candidate's curve is decoded in Rust (see [`build_search_predicate`]), so a raw
    /// SQL page of `limit` candidates can come back with fewer than `limit` genuine
    /// matches even though more exist further on. Returning that shrunk page would
    /// violate the "short page means no more" contract and make an exhaustive caller
    /// (a mirror sync) stop early and silently miss rows. So this loops instead --
    /// pulling successive raw pages via [`Self::search_diagrams_page_raw`] and
    /// filtering each with [`Self::item_satisfies_performance_filters`] -- until it
    /// accumulates `limit` genuine matches or a raw page comes back short.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the assembled `SELECT` query fails, if a
    /// row fails to decode into a `DiagramListItem`, or (when `range.performance` is
    /// non-empty) if loading or decoding a candidate's tilt curves fails.
    pub fn search_diagrams_page(
        &self,
        query: &str,
        shape_filter: &str,
        gear_filter: &str,
        range: &RangeFilter,
        after_id: Option<i64>,
        limit: i64,
    ) -> Result<Vec<crate::model::entry::DiagramListItem>> {
        if range.performance.is_empty() {
            return self.search_diagrams_page_raw(
                query,
                shape_filter,
                gear_filter,
                range,
                after_id,
                limit,
            );
        }

        let limit_usize = usize::try_from(limit).unwrap_or(0);
        let mut matched: Vec<crate::model::entry::DiagramListItem> = Vec::new();
        let mut cursor = after_id;
        loop {
            let raw_page = self.search_diagrams_page_raw(
                query,
                shape_filter,
                gear_filter,
                range,
                cursor,
                limit,
            )?;
            let raw_page_was_full = raw_page.len() == limit_usize;
            cursor = raw_page.last().map(|item| item.id);

            for item in raw_page {
                if self.item_satisfies_performance_filters(item.id, &range.performance)? {
                    matched.push(item);
                }
            }

            if matched.len() >= limit_usize || !raw_page_was_full {
                break;
            }
        }
        matched.truncate(limit_usize);
        Ok(matched)
    }

    /// The single-query implementation [`Self::search_diagrams_page`] delegates to
    /// directly when `range.performance` is empty, and repeatedly otherwise. Shares its
    /// WHERE-clause construction with [`Self::search_diagrams`] via
    /// [`build_search_predicate`], so the two can never drift apart.
    ///
    /// Unlike [`Self::search_diagrams_page`], does not apply an exact
    /// [`crate::model::performance::PerformanceFilter`] test -- a returned row has only
    /// passed sound-but-incomplete SQL-level narrowing, so `range.performance`-active
    /// callers must not use this directly.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the assembled `SELECT` query fails,
    /// or if a row fails to decode into a `DiagramListItem`.
    fn search_diagrams_page_raw(
        &self,
        query: &str,
        shape_filter: &str,
        gear_filter: &str,
        range: &RangeFilter,
        after_id: Option<i64>,
        limit: i64,
    ) -> Result<Vec<crate::model::entry::DiagramListItem>> {
        let (mut sql, mut params) =
            build_search_predicate(query, shape_filter, gear_filter, range, true);

        if let Some(after) = after_id {
            sql.push_str(" AND de.id > ? ");
            params.push(Box::new(after));
        }
        sql.push_str(" ORDER BY de.id ASC LIMIT ? ");
        params.push(Box::new(limit));

        let mut stmt = self.conn.prepare(&sql)?;
        let bound: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(std::convert::AsRef::as_ref).collect();

        let rows = stmt.query_map(bound.as_slice(), |row| {
            Ok(crate::model::entry::DiagramListItem {
                id: row.get(0)?,
                title: row.get(1)?,
                url: row.get(2)?,
                design_id: row.get(3)?,
                shape: row.get(4)?,
                index_gear: row.get(5)?,
                facets_count: row.get(6)?,
                designer_info: row.get(7)?,
                lw_ratio: row.get(8)?,
                refractive_index: row.get(9)?,
                volume: row.get(10)?,
                competition_diagram: row.get(11)?,
                ignored: row.get(12)?,
            })
        })?;

        let mut list = Vec::new();
        for item in rows.flatten() {
            list.push(item);
        }
        Ok(list)
    }

    /// The exact, per-design half of the two-stage performance-filter design (see
    /// [`build_search_predicate`]): loads `entry_id`'s tilt curves and tests every
    /// filter in `filters` against them, `true` only if all pass. A design with no
    /// stored curves returns `false` without error, never treated as a failure.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `Database::get_tilt_curves` call fails (a
    /// genuine I/O or decode error, never "no curves").
    fn item_satisfies_performance_filters(
        &self,
        entry_id: i64,
        filters: &[crate::model::performance::PerformanceFilter],
    ) -> Result<bool> {
        let Some(curves) = self.get_tilt_curves(entry_id)? else {
            return Ok(false);
        };
        Ok(filters.iter().all(|f| curves.matches_performance_filter(f)))
    }

    /// [`Self::search_diagrams`], plus a count of how many otherwise-matching designs
    /// were excluded because they have no stored tilt curves to test an active
    /// `range.performance` predicate against -- see [`PerformanceSearchResult`] for
    /// exactly what that count includes. When `range.performance` is empty this is
    /// exactly [`Self::search_diagrams`] with the count fixed at `0`, and the second
    /// query below is skipped entirely.
    ///
    /// # Errors
    ///
    /// Returns an error under the same conditions as [`Self::search_diagrams`], or if
    /// the exclusion-count query fails.
    pub fn search_diagrams_with_performance_exclusions(
        &self,
        query: &str,
        shape_filter: &str,
        gear_filter: &str,
        range: &RangeFilter,
    ) -> Result<PerformanceSearchResult> {
        let items = self.search_diagrams(query, shape_filter, gear_filter, range)?;

        if range.performance.is_empty() {
            return Ok(PerformanceSearchResult {
                items,
                excluded_for_missing_curves: 0,
            });
        }

        // Every filter except performance, narrowed to rows with no tilt curves
        // generated (`tc.generated_at IS NULL`) -- candidates that could never satisfy
        // the filter.
        let (predicate_sql, params) =
            build_search_predicate(query, shape_filter, gear_filter, range, false);
        let count_sql = format!(
            "SELECT COUNT(*) FROM ({predicate_sql} AND tc.generated_at IS NULL) AS missing_curves"
        );
        let mut stmt = self.conn.prepare(&count_sql)?;
        let bound: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(std::convert::AsRef::as_ref).collect();
        let excluded: i64 = stmt.query_row(bound.as_slice(), |r| r.get(0))?;

        Ok(PerformanceSearchResult {
            items,
            // A SQL COUNT(*) is never negative (workspace-wide `cast_sign_loss` =
            // "allow" covers this cast; see Cargo.toml).
            excluded_for_missing_curves: excluded as usize,
        })
    }
}

/// Maximum rows [`Database::search_diagrams`] and friends will return.
///
/// Also the default page size a mirror walking [`Database::search_diagrams_page`] is
/// sized around (`apps/indicatrix-worker`'s `SearchPage` handler uses the same value).
pub const SEARCH_RESULT_CAP: i64 = 1000;

/// Builds the `SELECT ... WHERE ...` predicate (everything through the last filter
/// clause, not `ORDER BY`/`LIMIT`) shared by [`Database::search_diagrams_page_raw`]
/// and [`Database::search_diagrams_with_performance_exclusions`], plus the bound
/// parameters its `?` placeholders need, in SQL-text order.
///
/// `index_gear`/`lw_ratio`/`refractive_index`/`volume` are stored as REAL/INTEGER (see
/// `migrate_numeric_columns`) and cast back to TEXT so `DiagramListItem`'s
/// display-string fields stay unchanged.
///
/// # Tilt-performance filters: SQL narrows, Rust decides
///
/// A [`crate::model::performance::PerformanceFilter`]'s tilt radius is an arbitrary
/// `0.0..=90.0` value, ruling out a precomputed SQL column. This function adds only a
/// sound, incomplete narrowing predicate per active filter
/// ([`crate::model::performance::PerformanceFilter::sound_sql_narrowing`], built from
/// `diagram_tilt_curves`'s 6 precomputed global-min/max columns): it can exclude a row
/// with certainty, but a row it lets through isn't yet confirmed. Every caller
/// re-checks every surviving row exactly, in Rust, against the decoded curve (see
/// [`Database::item_satisfies_performance_filters`]) -- this predicate only shrinks the
/// candidate set, it is never the source of truth.
///
/// `include_performance` selects whether `range.performance`'s narrowing predicates
/// are appended at all: `true` for every ordinary search, `false` only for
/// `search_diagrams_with_performance_exclusions`'s second query, which counts designs
/// excluded *for lack of curves* and so must apply every other filter but not these.
///
/// The returned `SELECT` always `LEFT JOIN`s `diagram_tilt_curves AS tc` regardless of
/// `include_performance`: both are 1:1 joins on `diagram_entries.id`, so the
/// unconditional join costs nothing measurable.
fn build_search_predicate(
    query: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilter,
    include_performance: bool,
) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let q_pattern = format!("%{}%", query.trim());
    let mut sql = String::from(
        "SELECT de.id, de.title, de.url, de.design_id,
                dd.shape, CAST(dd.index_gear AS TEXT), dd.facets_count, dd.designer_info,
                CAST(dd.lw_ratio AS TEXT), CAST(dd.refractive_index AS TEXT),
                CAST(dd.volume AS TEXT), dd.competition_diagram,
                de.ignored
         FROM diagram_entries de
         LEFT JOIN diagram_details dd ON de.id = dd.entry_id
         LEFT JOIN diagram_tilt_curves tc ON de.id = tc.entry_id
         WHERE 1=1 ",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if query.trim().is_empty() {
        sql.push_str(" AND (1=1 OR ?1 IS NULL) ");
    } else {
        sql.push_str(
            " AND (de.title LIKE ?1 OR dd.designer_info LIKE ?1 OR de.design_id LIKE ?1) ",
        );
    }
    params.push(Box::new(q_pattern));

    // Bound parameters, not string-interpolated literals -- `?1` above is reserved for
    // the LIKE pattern, so every parameter from here on is a plain positional `?`;
    // rusqlite/SQLite number those sequentially starting just after the highest
    // explicit index used (`?1`), so the first plain `?` here binds to position 2 and
    // each subsequent one to the next position, matching `params`' push order below.
    if !shape_filter.is_empty() && shape_filter != "All" {
        sql.push_str(" AND dd.shape = ? ");
        params.push(Box::new(shape_filter.to_owned()));
    }

    if !gear_filter.is_empty() && gear_filter != "All" {
        sql.push_str(" AND dd.index_gear = ? ");
        params.push(Box::new(gear_filter.to_owned()));
    }

    // Bound parameters, same as shape/gear above: these are slider numbers.
    if let Some(min) = range.ri_min {
        sql.push_str(" AND dd.refractive_index >= ? ");
        params.push(Box::new(min));
    }
    if let Some(max) = range.ri_max {
        sql.push_str(" AND dd.refractive_index <= ? ");
        params.push(Box::new(max));
    }
    if let Some(min) = range.lw_min {
        sql.push_str(" AND dd.lw_ratio >= ? ");
        params.push(Box::new(min));
    }
    if let Some(max) = range.lw_max {
        sql.push_str(" AND dd.lw_ratio <= ? ");
        params.push(Box::new(max));
    }
    if let Some(min) = range.volume_min {
        sql.push_str(" AND dd.volume >= ? ");
        params.push(Box::new(min));
    }
    if let Some(max) = range.volume_max {
        sql.push_str(" AND dd.volume <= ? ");
        params.push(Box::new(max));
    }
    if let Some(min) = range.facets_min {
        sql.push_str(" AND dd.facets >= ? ");
        params.push(Box::new(min));
    }
    if let Some(max) = range.facets_max {
        sql.push_str(" AND dd.facets <= ? ");
        params.push(Box::new(max));
    }

    // RI-tolerance band ANDs alongside ri_min/ri_max above, not replacing them.
    if let Some((center, tolerance)) = range.ri_tolerance {
        sql.push_str(" AND dd.refractive_index >= ? AND dd.refractive_index <= ? ");
        params.push(Box::new(center - tolerance));
        params.push(Box::new(center + tolerance));
    }

    // Ignored designs excluded unless opted back in. No bound parameter: 0/1 is a
    // fixed literal, not caller-supplied.
    if !range.include_ignored {
        sql.push_str(" AND de.ignored = 0 ");
    }

    // Tilt-performance narrowing (see "SQL narrows, Rust decides" above).
    // `sound_sql_narrowing`'s column name only ever comes from
    // `global_extreme_column_name`, never user-supplied text.
    if include_performance && !range.performance.is_empty() {
        // No stored curves can never satisfy any active filter, including a `Mean`
        // filter whose `sound_sql_narrowing()` is `None`.
        sql.push_str(" AND tc.generated_at IS NOT NULL ");
        for filter in &range.performance {
            if let Some((column, comparator, threshold)) = filter.sound_sql_narrowing() {
                let _ = write!(sql, " AND tc.{column} {comparator} ? ");
                params.push(Box::new(f64::from(threshold)));
            }
        }
    }

    (sql, params)
}

/// Linear-interpolation percentile (the same "linear" method `numpy.percentile`
/// defaults to): walks to fractional index `p/100 * (n-1)` in the sorted slice and
/// interpolates between the two bracketing values. `values` must be sorted ascending
/// and non-empty (callers only reach this after checking `.first()`).
pub(super) fn percentile_of_sorted(values: &[f64], p: f64) -> f64 {
    let n = values.len();
    if n == 1 {
        return values[0];
    }
    let idx = (p / 100.0) * (n - 1) as f64;
    let lo = idx.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let frac = idx - lo as f64;
    frac.mul_add(values[hi] - values[lo], values[lo])
}

/// Logs a warning when `raw_max` sits more than [`OUTLIER_WARNING_RATIO`] times beyond
/// `bound` -- the signature of a data-entry error, not a legitimately wide
/// distribution. Purely observational: never errors, never touches any row.
fn warn_if_outlier(column: &str, raw_max: f64, bound: f64) {
    if bound > 0.0 && raw_max > bound * OUTLIER_WARNING_RATIO {
        warn!(
            "diagram_details.{column}: raw max {raw_max} is {:.0}x its p{RANGE_BOUND_PERCENTILE} \
             range-filter bound of {bound} -- likely a data-entry error in the source catalogue, \
             not a real outlier. The row is left untouched and still matches searches while that \
             slider side is unfiltered; it just no longer sets the slider's scale.",
            raw_max / bound
        );
    }
}
