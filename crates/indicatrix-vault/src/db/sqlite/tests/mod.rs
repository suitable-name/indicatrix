//! Integration tests for [`super::Database`], split by topic:
//!
//! - [`fixtures`]: the temp-DB path helper and pre-migration schema seeder shared by
//!   more than one topic below.
//! - [`migrations`]: column-retype/add migrations against `diagram_entries`/
//!   `diagram_details` (numeric retype, `source_id`, designer/attachment columns).
//! - [`schema_migrations`]: table/index/vocabulary-level migrations (shape vocabulary,
//!   `ignored` plus its side tables, tilt-curve aggregate pruning, `sql_identifier`).
//! - [`materials`]: `custom_gem_materials` column migrations and the
//!   `save_custom_material` round trip.
//! - [`detail`][]: `save_diagram_detail`/`update_diagram_metadata`/
//!   `get_derived_from_title`.
//! - [`entries`]: rename, `updated_at` bump semantics, `url`
//!   rewrite, delete cascade, and the `ignored` flag.
//! - [`search`]: the library search/filter surface and attribute-range stats.
//! - [`performance`]: tilt-performance filtering, including the SQL-narrowing
//!   soundness property against a brute-force scan.
//! - [`connection`]: WAL/`open_read_only`/`:memory:` connection behaviour.
//! - [`planner_exclusions`]: the Rough Planner exclusion mark -- toggling, the sorted
//!   readers, and its independence from `updated_at`, re-saves, deletes and `ignored`.
//! - [`design_side_data`]: saved variants, cutting progress and the lighting choice,
//!   keyed by the design's UUID rather than by a catalogue entry.
//! - [`design_side_migrations`]: those three tables on a fresh database, on one that
//!   lacks them, and across repeated opens.

mod connection;
mod design_side_data;
mod design_side_migrations;
mod detail;
mod entries;
mod fixtures;
mod materials;
mod migrations;
mod migrations_blob_order;
mod performance;
mod planner_exclusions;
mod schema_migrations;
mod search;
