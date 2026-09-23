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
    /// inaccessible database). This allows a broken database to be distinguished
    /// from a genuinely empty one.
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
    /// Returns an error if preparing or running the assembled `SELECT` query fails, or
    /// if a row fails to decode into a `DiagramListItem`. A candidate whose tilt-curve
    /// BLOB fails to decode is never an error here -- see
    /// [`Self::item_satisfies_performance_filters`].
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
                if self.item_satisfies_performance_filters(item.id, &range.performance) {
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
        let (mut sql, mut params) = build_search_predicate(
            query,
            shape_filter,
            gear_filter,
            range,
            true,
            DisplayFilters::default(),
        );

        if let Some(after) = after_id {
            sql.push_str(" AND de.id > ? ");
            params.push(Box::new(after));
        }
        sql.push_str(" ORDER BY de.id ASC LIMIT ? ");
        params.push(Box::new(limit));

        self.query_diagram_list_items(&sql, &params)
    }

    /// Ordered, offset-paginated counterpart of [`Self::search_diagrams_page_raw`] --
    /// backs [`Self::search_diagrams_display`]/[`Self::count_matching_diagrams`], never
    /// the exhaustive keyset walk a mirror sync needs.
    ///
    /// `offset`/`limit` rather than `after_id`'s keyset cursor: unlike
    /// `search_diagrams_page_raw` (whose id-ASC order and "no skip/dup on concurrent
    /// insert" guarantee an exhaustive background walk depends on), a caller here is a
    /// single foreground display fetch over an arbitrary [`SortOrder`], for which no
    /// single column is guaranteed both unique and monotonic across every order. The
    /// catalogue this ships against tops out in the low thousands of rows, so the
    /// `OFFSET` cost this trades away is not measurable in practice.
    ///
    /// `local_only` restricts the predicate to `diagram_entries.url` values written by
    /// [`crate::local::LOCAL_SOURCE_ID`] imports (`local://...`, see
    /// [`build_search_predicate`]'s doc comment) -- the cutter's own designs, as
    /// opposed to the wider scraped catalogue.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the assembled `SELECT` query fails, or
    /// if a row fails to decode into a `DiagramListItem`.
    fn search_diagrams_page_raw_ordered(
        &self,
        query: &str,
        shape_filter: &str,
        gear_filter: &str,
        range: &RangeFilter,
        page: DisplayPage<'_>,
    ) -> Result<Vec<crate::model::entry::DiagramListItem>> {
        let (mut sql, mut params) = build_search_predicate(
            query,
            shape_filter,
            gear_filter,
            range,
            true,
            DisplayFilters {
                order: page.order,
                local_only: page.local_only,
                tag_filter: page.tag_filter,
                id_filter: page.id_filter,
            },
        );
        sql.push_str(page.order.order_by_sql());
        sql.push_str(" LIMIT ? OFFSET ? ");
        params.push(Box::new(page.limit));
        params.push(Box::new(page.offset));

        self.query_diagram_list_items(&sql, &params)
    }

    /// Runs `sql` (a full `SELECT` over `diagram_entries`/`diagram_details`/
    /// `diagram_tilt_curves` matching [`build_search_predicate`]'s fixed column list and
    /// order) with `params` bound positionally, decoding every row into a
    /// [`crate::model::entry::DiagramListItem`]. Shared by
    /// [`Self::search_diagrams_page_raw`] and [`Self::search_diagrams_page_raw_ordered`]
    /// so the two can never decode the column list differently.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running `sql` fails, or if a row fails to
    /// decode.
    fn query_diagram_list_items(
        &self,
        sql: &str,
        params: &[Box<dyn rusqlite::ToSql>],
    ) -> Result<Vec<crate::model::entry::DiagramListItem>> {
        let mut stmt = self.conn.prepare(sql)?;
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

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to decode a row while running a diagram-list query")
    }

    /// The exact, per-design half of the two-stage performance-filter design (see
    /// [`build_search_predicate`]): loads `entry_id`'s tilt curves and tests every
    /// filter in `filters` against them, `true` only if all pass.
    ///
    /// A design with no stored curves, OR whose stored curve BLOB fails to decode
    /// (e.g. a length mismatch -- see [`crate::model::tilt_curves::TiltPerformanceCurves::from_bytes`]),
    /// is treated identically: `false`, logged at `warn!` in the decode-failure case so
    /// the condition is visible without aborting the caller's whole search. A single
    /// corrupt row failing every performance-filtered search outright (the previous
    /// behaviour) would be far worse than that one row silently not matching.
    fn item_satisfies_performance_filters(
        &self,
        entry_id: i64,
        filters: &[crate::model::performance::PerformanceFilter],
    ) -> bool {
        let curves = match self.get_tilt_curves(entry_id) {
            Ok(Some(curves)) => curves,
            Ok(None) => return false,
            Err(e) => {
                warn!(
                    "entry_id {entry_id}: failed to load/decode stored tilt curves ({e:#}); \
                     treating as having no curves rather than failing the whole search"
                );
                return false;
            }
        };
        filters.iter().all(|f| curves.matches_performance_filter(f))
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

        let excluded = self.count_missing_curve_exclusions(
            query,
            shape_filter,
            gear_filter,
            range,
            DisplayFilters::default(),
        )?;

        Ok(PerformanceSearchResult {
            items,
            excluded_for_missing_curves: excluded,
        })
    }

    /// [`Self::search_diagrams_with_performance_exclusions`], plus a caller-chosen
    /// [`SortOrder`] and an opt-in restriction to the cutter's own, locally-imported
    /// designs (`local_only`) -- the display query behind the library panel's sort
    /// selector and "My designs" toggle. Capped at
    /// [`SEARCH_RESULT_CAP`] like every other display-facing search in this module.
    ///
    /// Unlike [`Self::search_diagrams_page`], this does not expose a keyset cursor: it
    /// exists for a single foreground fetch of "the current view," not an exhaustive
    /// background walk -- see [`Self::search_diagrams_page_raw_ordered`]'s doc comment
    /// for why offset pagination is fine here but would not be for that other use.
    ///
    /// # Errors
    ///
    /// Returns an error under the same conditions as
    /// [`Self::search_diagrams_with_performance_exclusions`].
    pub fn search_diagrams_display(
        &self,
        query: &str,
        shape_filter: &str,
        gear_filter: &str,
        range: &RangeFilter,
        filters: DisplayFilters<'_>,
    ) -> Result<PerformanceSearchResult> {
        if range.performance.is_empty() {
            let items = self.search_diagrams_page_raw_ordered(
                query,
                shape_filter,
                gear_filter,
                range,
                DisplayPage {
                    order: filters.order,
                    local_only: filters.local_only,
                    tag_filter: filters.tag_filter,
                    id_filter: filters.id_filter,
                    offset: 0,
                    limit: SEARCH_RESULT_CAP,
                },
            )?;
            return Ok(PerformanceSearchResult {
                items,
                excluded_for_missing_curves: 0,
            });
        }

        let limit_usize = usize::try_from(SEARCH_RESULT_CAP).unwrap_or(0);
        // `exhaustive: false` -- a solo display fetch doesn't need the exact total, so
        // the walk stops as soon as a capped page's worth of matches is found. A
        // caller that DOES want the exact total alongside this same page should use
        // [`Self::search_diagrams_display_with_count`] instead, which walks once for
        // both rather than decoding every candidate's tilt curves twice.
        let (matched, _) = self.walk_matching_candidates(
            query,
            shape_filter,
            gear_filter,
            range,
            filters,
            CandidateWalkOptions {
                order: filters.order,
                display_cap: limit_usize,
                exhaustive: false,
            },
        )?;

        let excluded =
            self.count_missing_curve_exclusions(query, shape_filter, gear_filter, range, filters)?;
        Ok(PerformanceSearchResult {
            items: matched,
            excluded_for_missing_curves: excluded,
        })
    }

    /// [`Self::search_diagrams_display`] and [`Self::count_matching_diagrams`] combined
    /// into one candidate walk -- a single-pass fix.
    ///
    /// Calling those two separately (as the library panel's `fetch_diagram_list_with_options`
    /// once did) decodes every active-performance-filter candidate's tilt-curve BLOB
    /// twice: once in each method's own walk over
    /// [`Self::search_diagrams_page_raw_ordered`]. This method walks the SQL-narrowed
    /// candidate set exactly once via [`Self::walk_matching_candidates`], decoding each
    /// candidate's curve at most once, and returns both the capped display page (in
    /// `filters.order`, same shape as [`Self::search_diagrams_display`]'s result) and
    /// the exact, uncapped match count (same value [`Self::count_matching_diagrams`]
    /// would return for the same arguments).
    ///
    /// When `range.performance` is empty neither query decodes any curve at all, so
    /// this just delegates to the two existing cheap SQL queries -- there is nothing to
    /// merge.
    ///
    /// # Errors
    ///
    /// Returns an error under the same conditions as [`Self::search_diagrams_display`]
    /// and [`Self::count_matching_diagrams`].
    pub fn search_diagrams_display_with_count(
        &self,
        query: &str,
        shape_filter: &str,
        gear_filter: &str,
        range: &RangeFilter,
        filters: DisplayFilters<'_>,
    ) -> Result<(PerformanceSearchResult, usize)> {
        if range.performance.is_empty() {
            let display =
                self.search_diagrams_display(query, shape_filter, gear_filter, range, filters)?;
            let total =
                self.count_matching_diagrams(query, shape_filter, gear_filter, range, filters)?;
            return Ok((display, total));
        }

        let limit_usize = usize::try_from(SEARCH_RESULT_CAP).unwrap_or(0);
        // `exhaustive: true` -- unlike the solo display fetch above, the exact total
        // returned here must be the real total, so every raw page is walked to
        // completion regardless of how early the display cap is reached.
        let (matched, total) = self.walk_matching_candidates(
            query,
            shape_filter,
            gear_filter,
            range,
            filters,
            CandidateWalkOptions {
                order: filters.order,
                display_cap: limit_usize,
                exhaustive: true,
            },
        )?;

        let excluded =
            self.count_missing_curve_exclusions(query, shape_filter, gear_filter, range, filters)?;
        Ok((
            PerformanceSearchResult {
                items: matched,
                excluded_for_missing_curves: excluded,
            },
            total,
        ))
    }

    /// The shared candidate walk behind [`Self::search_diagrams_display`],
    /// [`Self::count_matching_diagrams`], and
    /// [`Self::search_diagrams_display_with_count`] -- walks every SQL-narrowed
    /// candidate for `query`/`shape_filter`/`gear_filter`/`range` in `options.order`,
    /// testing each one's exact match via [`Self::item_satisfies_performance_filters`],
    /// so a candidate's tilt curves are decoded at most once per call regardless of
    /// whether the caller wants the display page, the exact count, or both.
    ///
    /// Returns every genuine match up to `options.display_cap` (`0` for a count-only
    /// caller that never wants the page collected -- nothing is ever pushed onto the
    /// returned `Vec` in that case) alongside the exact match count. Further matches
    /// beyond `display_cap` are still tallied into that count even when not collected.
    /// See [`CandidateWalkOptions`]'s own field docs for `exhaustive`.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying `SELECT` query fails, or
    /// if a row fails to decode into a `DiagramListItem`.
    fn walk_matching_candidates(
        &self,
        query: &str,
        shape_filter: &str,
        gear_filter: &str,
        range: &RangeFilter,
        filters: DisplayFilters<'_>,
        options: CandidateWalkOptions,
    ) -> Result<(Vec<crate::model::entry::DiagramListItem>, usize)> {
        let limit_usize = usize::try_from(SEARCH_RESULT_CAP).unwrap_or(0);
        let mut matched = Vec::new();
        let mut total = 0usize;
        let mut offset = 0i64;
        loop {
            let raw_page = self.search_diagrams_page_raw_ordered(
                query,
                shape_filter,
                gear_filter,
                range,
                DisplayPage {
                    order: options.order,
                    local_only: filters.local_only,
                    tag_filter: filters.tag_filter,
                    id_filter: filters.id_filter,
                    offset,
                    limit: SEARCH_RESULT_CAP,
                },
            )?;
            let raw_page_was_full = raw_page.len() == limit_usize;
            offset += raw_page.len() as i64;

            for item in raw_page {
                if self.item_satisfies_performance_filters(item.id, &range.performance) {
                    total += 1;
                    if matched.len() < options.display_cap {
                        matched.push(item);
                    }
                }
            }

            if !raw_page_was_full || (!options.exhaustive && matched.len() >= options.display_cap) {
                break;
            }
        }
        Ok((matched, total))
    }

    /// How many otherwise-matching designs (every `range` filter except performance)
    /// have no stored tilt curves at all, so an active `range.performance` predicate
    /// can never be satisfied by them -- see [`PerformanceSearchResult`]. Shared by
    /// [`Self::search_diagrams_with_performance_exclusions`] and
    /// [`Self::search_diagrams_display`] so the two can never compute this differently.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the assembled `COUNT` query fails.
    fn count_missing_curve_exclusions(
        &self,
        query: &str,
        shape_filter: &str,
        gear_filter: &str,
        range: &RangeFilter,
        filters: DisplayFilters<'_>,
    ) -> Result<usize> {
        // Every filter except performance, narrowed to rows with no tilt curves
        // generated (`tc.generated_at IS NULL`) -- candidates that could never satisfy
        // the filter.
        let (predicate_sql, params) =
            build_search_predicate(query, shape_filter, gear_filter, range, false, filters);
        let count_sql = format!(
            "SELECT COUNT(*) FROM ({predicate_sql} AND tc.generated_at IS NULL) AS missing_curves"
        );
        let mut stmt = self.conn.prepare(&count_sql)?;
        let bound: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(std::convert::AsRef::as_ref).collect();
        let excluded: i64 = stmt.query_row(bound.as_slice(), |r| r.get(0))?;
        // A SQL COUNT(*) is never negative (workspace-wide `cast_sign_loss` = "allow"
        // covers this cast; see Cargo.toml).
        Ok(excluded as usize)
    }

    /// The real, uncapped count of designs matching `query`/`shape_filter`/
    /// `gear_filter`/`range`/`filters` -- gives the actual match count distinct from
    /// the entire catalogue [`Self::get_total_count`] and a display query's capped result.
    ///
    /// When `range.performance` is empty this is one `COUNT(*)` over
    /// [`build_search_predicate`]'s own predicate. Otherwise -- since a performance
    /// filter's exact test only runs once a candidate's curve is decoded in Rust, see
    /// that function's doc comment -- this walks every SQL-narrowed candidate via
    /// [`Self::walk_matching_candidates`] and tallies exact matches, unbounded by
    /// `SEARCH_RESULT_CAP` (unlike the capped page a caller actually displays).
    ///
    /// A caller that also wants the capped display page for these same arguments
    /// should use [`Self::search_diagrams_display_with_count`] instead of calling this
    /// alongside [`Self::search_diagrams_display`]: doing so separately decodes every
    /// candidate's tilt curves twice.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying query fails, or if a
    /// row fails to decode into a `DiagramListItem`.
    pub fn count_matching_diagrams(
        &self,
        query: &str,
        shape_filter: &str,
        gear_filter: &str,
        range: &RangeFilter,
        filters: DisplayFilters<'_>,
    ) -> Result<usize> {
        if range.performance.is_empty() {
            let (predicate_sql, params) =
                build_search_predicate(query, shape_filter, gear_filter, range, true, filters);
            let count_sql = format!("SELECT COUNT(*) FROM ({predicate_sql}) AS matching");
            let mut stmt = self.conn.prepare(&count_sql)?;
            let bound: Vec<&dyn rusqlite::ToSql> =
                params.iter().map(std::convert::AsRef::as_ref).collect();
            let count: i64 = stmt.query_row(bound.as_slice(), |r| r.get(0))?;
            return Ok(count as usize);
        }

        // `display_cap: 0` -- nothing is ever collected into the walk's `Vec`, only
        // the exact total is wanted here. `exhaustive: true` -- every raw page is
        // walked to completion, since a bare count has no "enough for the page"
        // early-out.
        let (_, total) = self.walk_matching_candidates(
            query,
            shape_filter,
            gear_filter,
            range,
            filters,
            CandidateWalkOptions {
                order: SortOrder::CatalogueOrder,
                display_cap: 0,
                exhaustive: true,
            },
        )?;
        Ok(total)
    }

    /// Every entry id matching `query`/`shape_filter`/`gear_filter`/`range`/`filters`,
    /// with NO [`SEARCH_RESULT_CAP`] truncation -- the library's "regenerate previews/
    /// tilt curves for the whole filtered set" batch action needs the REAL match set,
    /// not just the capped page a display query shows (`Self::search_diagrams_display`)
    /// or a bare count (`Self::count_matching_diagrams`).
    ///
    /// Walks the whole match set via the same offset-paginated
    /// [`Self::search_diagrams_page_raw_ordered`]/exact-performance-filter-recheck
    /// shape [`Self::count_matching_diagrams`] already uses for its own uncapped walk,
    /// just collecting ids instead of tallying a count -- so the two can never
    /// disagree on what "matching" means. `SortOrder::CatalogueOrder` throughout: the
    /// caller is about to batch-process this set, not display it in the panel's
    /// currently-chosen order, so `filters.order` is deliberately not threaded in
    /// here.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying query fails, or if a
    /// row fails to decode into a `DiagramListItem`.
    pub fn matching_entry_ids(
        &self,
        query: &str,
        shape_filter: &str,
        gear_filter: &str,
        range: &RangeFilter,
        filters: DisplayFilters<'_>,
    ) -> Result<Vec<i64>> {
        let limit_usize = usize::try_from(SEARCH_RESULT_CAP).unwrap_or(0);
        let mut ids = Vec::new();
        let mut offset = 0i64;
        loop {
            let raw_page = self.search_diagrams_page_raw_ordered(
                query,
                shape_filter,
                gear_filter,
                range,
                DisplayPage {
                    order: SortOrder::CatalogueOrder,
                    local_only: filters.local_only,
                    tag_filter: filters.tag_filter,
                    id_filter: filters.id_filter,
                    offset,
                    limit: SEARCH_RESULT_CAP,
                },
            )?;
            let raw_page_was_full = raw_page.len() == limit_usize;
            offset += raw_page.len() as i64;
            for item in raw_page {
                if range.performance.is_empty()
                    || self.item_satisfies_performance_filters(item.id, &range.performance)
                {
                    ids.push(item.id);
                }
            }
            if !raw_page_was_full {
                break;
            }
        }
        Ok(ids)
    }
}

/// Sort order for [`Database::search_diagrams_display`] -- the library panel's sort
/// selector (the tag/collection half is deliberately out of scope here).
///
/// `#[default]` is [`Self::CatalogueOrder`], the same `de.id ASC` every other search in
/// this module has always used, so a caller that never sets a sort preference sees no
/// change in behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    /// `de.id ASC` -- insertion order, this module's long-standing default.
    #[default]
    CatalogueOrder,
    /// Case-insensitive title, A-Z.
    Title,
    /// Most recently created first (`diagram_entries.created_at`).
    /// A row that predates that column (`NULL`) sorts last -- SQLite already orders
    /// `NULL` after every non-null value in `DESC`, so no extra `CASE` is needed.
    Newest,
    /// Most recently edited first (`diagram_entries.updated_at`).
    /// Same `NULL`-sorts-last behaviour as [`Self::Newest`].
    RecentlyEdited,
}

impl SortOrder {
    /// The `ORDER BY` clause text for this order, appended directly after
    /// [`build_search_predicate`]'s `WHERE` clause -- every variant is a fixed literal,
    /// never built from caller input, so this is safe to interpolate.
    const fn order_by_sql(self) -> &'static str {
        match self {
            Self::CatalogueOrder => " ORDER BY de.id ASC ",
            Self::Title => " ORDER BY de.title COLLATE NOCASE ASC, de.id ASC ",
            Self::Newest => " ORDER BY de.created_at DESC, de.id DESC ",
            Self::RecentlyEdited => " ORDER BY de.updated_at DESC, de.id DESC ",
        }
    }
}

/// [`Database::search_diagrams_display`]/[`Database::count_matching_diagrams`]'s
/// sort/restriction options.
///
/// Bundled into one value purely to keep those two public functions under clippy's
/// `too_many_arguments` lint -- same reasoning as [`DisplayPage`], which this
/// expands into once `offset`/`limit` are known for a given page.
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplayFilters<'a> {
    pub order: SortOrder,
    /// "My designs" restriction.
    pub local_only: bool,
    /// Tag chip restriction, `None` for no
    /// restriction.
    pub tag_filter: Option<i64>,
    /// "Show these N" restriction to an explicit id set (the batch
    /// case), `None`/empty for no restriction.
    pub id_filter: Option<&'a [i64]>,
}

/// Bundles [`Database::search_diagrams_page_raw_ordered`]'s order/restriction/paging
/// parameters into one value, purely to keep that function under clippy's
/// `too_many_arguments` lint -- each field is independent, not a cohesive value in its
/// own right, so this has no behaviour of its own beyond grouping them.
#[derive(Debug, Clone, Copy)]
struct DisplayPage<'a> {
    order: SortOrder,
    local_only: bool,
    /// Restricts the page to designs carrying this tag id, `None` for no tag
    /// restriction. See [`build_search_predicate`]'s own doc
    /// comment for the predicate this adds.
    tag_filter: Option<i64>,
    /// Restricts the page to exactly these entry ids (the "show
    /// these N" batch-import case), `None`/empty for no restriction. Borrowed, not
    /// owned: every caller already holds the id list (`imported_ids`, or a UI
    /// property read once per query) for at least as long as the query runs, so
    /// cloning it into every `DisplayPage` a multi-page performance-filter walk
    /// builds would be pure waste.
    id_filter: Option<&'a [i64]>,
    offset: i64,
    limit: i64,
}

/// Bundles [`Database::walk_matching_candidates`]'s per-call knobs, purely to keep
/// that function under clippy's `too_many_arguments` lint -- same convention as
/// [`DisplayPage`].
#[derive(Debug, Clone, Copy)]
struct CandidateWalkOptions {
    /// Sort order for the underlying raw pages -- irrelevant to which rows match, only
    /// to the order the returned `Vec` (and so the caller's display page) comes back in.
    order: SortOrder,
    /// Collect at most this many genuine matches into the walk's returned `Vec`. `0`
    /// for a count-only caller that never wants the page collected -- nothing is ever
    /// pushed in that case, so no wasted allocation for an unbounded count walk.
    display_cap: usize,
    /// `false` stops the walk as soon as `display_cap` matches have been collected, so
    /// the returned count is then only a lower bound and must not be surfaced as the
    /// real total (matches the pre-finding-17 [`Database::search_diagrams_display`]
    /// behaviour: a solo display fetch doesn't need the exact total). `true` always
    /// walks every raw page to completion, required whenever the exact total returned
    /// alongside it is actually surfaced to a caller
    /// ([`Database::count_matching_diagrams`],
    /// [`Database::search_diagrams_display_with_count`]).
    exhaustive: bool,
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
/// `local_only` restricts the predicate to designs synced under
/// [`crate::local::LOCAL_SOURCE_ID`] -- identified the same way
/// `library/detail.rs::is_local` already does, by `diagram_entries.url` starting with
/// the synthetic `local://` scheme every local import writes (see
/// `crate::local::import_asc`) -- as opposed to a real page URL from the wider scraped
/// catalogue. `false` (the default for every pre-existing caller) applies no such
/// restriction.
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
    filters: DisplayFilters<'_>,
) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let DisplayFilters {
        local_only,
        tag_filter,
        id_filter,
        order: _order,
    } = filters;
    let q_pattern = format!("%{}%", escape_like_pattern(query.trim()));
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
        // The search tooltip/placeholder (header.slint) promises "title, designer
        // or notes", and this predicate matches a stored note accordingly.
        // `angle_settings.notes` is per-TIER (one design has many rows),
        // so matching it needs an `EXISTS` subquery rather than a plain joined
        // column, which would otherwise duplicate a design once per matching tier.
        //
        // `ESCAPE '\'` on every LIKE here, paired with `escape_like_pattern` above:
        // without it, a literal `%`/`_` a user typed (both appear in real titles and
        // designer names) is read as a SQL wildcard instead of the character it looks
        // like -- an unescaped `_` alone matches every row's title.
        sql.push_str(
            " AND (de.title LIKE ?1 ESCAPE '\\' OR dd.designer_info LIKE ?1 ESCAPE '\\'
                   OR de.design_id LIKE ?1 ESCAPE '\\'
                   OR EXISTS (
                       SELECT 1 FROM angle_settings a
                       WHERE a.detail_id = dd.id AND a.notes LIKE ?1 ESCAPE '\\'
                   )) ",
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

    // "My designs" restriction -- no bound parameter: the
    // `local://` prefix is a fixed literal this crate itself writes (see
    // `crate::local::import_asc`), never caller-supplied text.
    if local_only {
        sql.push_str(" AND de.url LIKE 'local://%' ");
    }

    append_tag_and_id_filters(&mut sql, &mut params, tag_filter, id_filter);

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

/// Appends [`build_search_predicate`]'s tag-chip and "show
/// these N" restrictions -- split out purely to keep that
/// function under clippy's `too_many_lines` limit, not because these two are a
/// cohesive concept; see [`build_search_predicate`]'s own doc comment for the
/// predicate as a whole.
fn append_tag_and_id_filters(
    sql: &mut String,
    params: &mut Vec<Box<dyn rusqlite::ToSql>>,
    tag_filter: Option<i64>,
    id_filter: Option<&[i64]>,
) {
    // Tag chip restriction -- bound, not interpolated, even though a tag id is
    // already an integer: consistent with every other numeric bound in
    // `build_search_predicate`.
    if let Some(tag_id) = tag_filter {
        sql.push_str(" AND de.id IN (SELECT entry_id FROM diagram_tag_links WHERE tag_id = ?) ");
        params.push(Box::new(tag_id));
    }

    // "Show these N" restriction to an explicit id set -- one bound `?` per id, not
    // a single interpolated literal list, even though an entry id is already
    // caller-internal (never raw user text): consistent with every other bound
    // value in `build_search_predicate`, and it costs nothing here since the id set
    // is always small (one import batch's worth).
    if let Some(ids) = id_filter
        && !ids.is_empty()
    {
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(sql, " AND de.id IN ({placeholders}) ");
        for &id in ids {
            params.push(Box::new(id));
        }
    }
}

/// Escapes `\`, `%` and `_` in `raw` so it matches only as a literal substring once
/// wrapped in `%...%` -- paired with `ESCAPE '\'` on every `LIKE ?1` clause
/// [`build_search_predicate`] builds.
///
/// Without this, a character a user typed as ordinary text is silently read as a SQL
/// wildcard instead: real titles and designer names in the catalogue contain both `%`
/// and `_`, so an unescaped `_` alone matches every row's title (any single character),
/// and `50%` degrades to "contains `50` followed by anything" rather than a literal
/// percent sign. `\` itself is escaped first (into `\\`) so a literal backslash already
/// present in the query text isn't misread as the start of an escape sequence once
/// `ESCAPE '\'` is in effect.
fn escape_like_pattern(raw: &str) -> String {
    let mut escaped = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
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
