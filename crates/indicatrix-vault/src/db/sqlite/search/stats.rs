//! Catalogue-wide scalar facts a search UI's filter controls need: the total design
//! count, the distinct shape/gear vocabularies, and the *usable* min/max bounds for
//! each range-filterable attribute.

use super::super::Database;
use crate::model::filter::AttributeRanges;
use anyhow::{Context, Result};
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
}

/// Linear-interpolation percentile (the same "linear" method `numpy.percentile`
/// defaults to): walks to fractional index `p/100 * (n-1)` in the sorted slice and
/// interpolates between the two bracketing values. `values` must be sorted ascending
/// and non-empty (callers only reach this after checking `.first()`).
pub(in crate::db::sqlite) fn percentile_of_sorted(values: &[f64], p: f64) -> f64 {
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
