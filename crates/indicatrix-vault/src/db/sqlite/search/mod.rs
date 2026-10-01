//! The library search/filter surface: free-text + shape/gear/range/tag/tilt-performance
//! filtering over `diagram_entries`/`diagram_details`/`diagram_tilt_curves`, plus the
//! catalogue-wide stats a filter UI needs. Split into sibling modules by concern:
//!
//! - [`types`]: [`SortOrder`], [`DisplayFilters`], and the smaller argument-bundling
//!   structs the functions below are parameterized over.
//! - [`stats`]: catalogue-wide scalar facts (`get_total_count`, unique shapes/gears,
//!   attribute range bounds).
//! - [`predicate`]: the shared `WHERE`-clause builder every search/count/display query
//!   filters through.
//! - [`query`]: the core search/paging primitives and the exact per-design
//!   tilt-performance recheck.
//! - [`display`]: the display-facing surface (performance-exclusion counts, the sorted
//!   display page, the exact match count, the uncapped id walk) built on the shared
//!   candidate walk.
//!
//! Every item is re-exported here at its original flat `search::` path.

mod display;
mod predicate;
mod query;
mod stats;
mod types;

pub(in crate::db::sqlite) use predicate::register_fold_function;
#[cfg(test)]
pub(super) use stats::percentile_of_sorted;
pub use types::{DisplayFilters, SEARCH_RESULT_CAP, SortOrder};
