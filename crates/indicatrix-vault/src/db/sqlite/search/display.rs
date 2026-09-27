//! The display-facing search surface: performance-exclusion counts, the sorted/
//! restricted page the library panel shows, the exact uncapped match count, and the
//! uncapped id set a batch action walks -- all built on the shared candidate walk in
//! [`Database::walk_matching_candidates`], which decodes each performance-filtered
//! candidate's tilt curve at most once regardless of which of these a caller wants.

use super::{
    super::Database,
    predicate::build_search_predicate,
    types::{DisplayFilters, DisplayPage, SEARCH_RESULT_CAP, SortOrder},
};
use crate::model::filter::{PerformanceSearchResult, RangeFilter};
use anyhow::Result;

/// Bundles [`Database::walk_matching_candidates`]'s per-call knobs, purely to keep
/// that function under clippy's `too_many_arguments` lint -- same convention as
/// [`super::types::DisplayPage`].
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

impl Database {
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
