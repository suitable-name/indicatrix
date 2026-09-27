//! The core search/paging primitives: [`Database::search_diagrams`] and its
//! keyset-paginated form, the raw single-query executors both delegate to, and the
//! exact per-design tilt-performance recheck every performance-filtered caller (here
//! and in [`super::display`]) shares.

use super::{
    super::Database,
    predicate::build_search_predicate,
    types::{DisplayFilters, DisplayPage, SEARCH_RESULT_CAP},
};
use crate::model::filter::RangeFilter;
use anyhow::{Context, Result};
use tracing::warn;

impl Database {
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
    /// backs [`super::display`]'s display/count queries, never
    /// the exhaustive keyset walk a mirror sync needs.
    ///
    /// `offset`/`limit` rather than `after_id`'s keyset cursor: unlike
    /// `search_diagrams_page_raw` (whose id-ASC order and "no skip/dup on concurrent
    /// insert" guarantee an exhaustive background walk depends on), a caller here is a
    /// single foreground display fetch over an arbitrary
    /// [`super::types::SortOrder`], for which no single column is guaranteed both
    /// unique and monotonic across every order. The catalogue this ships against tops
    /// out in the low thousands of rows, so the `OFFSET` cost this trades away is not
    /// measurable in practice.
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
    pub(super) fn search_diagrams_page_raw_ordered(
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
    pub(super) fn item_satisfies_performance_filters(
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
}
