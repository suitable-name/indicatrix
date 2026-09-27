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
//! - [`entries`]: cross-source dedup, rename, `updated_at` bump semantics, `url`
//!   rewrite, delete cascade, and the `ignored` flag.
//! - [`search`]: the library search/filter surface and attribute-range stats.
//! - [`performance`]: tilt-performance filtering, including the SQL-narrowing
//!   soundness property against a brute-force scan.
//! - [`connection`]: WAL/`checkpoint`/`open_read_only`/`:memory:` connection behaviour.

mod connection;
mod detail;
mod entries;
mod fixtures;
mod materials;
mod migrations;
mod performance;
mod schema_migrations;
mod search;
