//! The raw `CREATE TABLE`/`CREATE INDEX` SQL text shared verbatim between a migration
//! (for an existing database) and `create_tables_if_not_exist` (for a fresh one), plus
//! the frozen list of columns
//! [`crate::db::sqlite::Database::migrate_prune_tilt_curve_aggregate_columns`] prunes.

use crate::model::performance::global_extreme_column_name;
use std::fmt::Write as _;

/// The indexes the library search predicate needs, shared verbatim between
/// [`crate::db::sqlite::Database::migrate_search_indexes`] and `create_tables_if_not_exist` -- same
/// convention as [`DIAGRAM_PREVIEWS_TABLE_SQL`].
///
/// SQLite does not index a `FOREIGN KEY` column on its own, and neither of these is the
/// LEADING column of a primary key, so without this pair both lookups below degrade to a
/// full table scan *per candidate design*:
///
/// - `angle_settings (detail_id)` -- serves the `EXISTS (... WHERE a.detail_id
///   = dd.id AND a.notes LIKE ?)` notes match. This table holds one row per TIER (50,817
///   rows against 3,299 designs on the owner's catalogue), so the scan is ~168 million
///   row visits, and `gui::library::search::refresh_diagram_list` issues three such
///   queries per keystroke on the UI thread. Measured on that catalogue: 16.1 s for
///   `%portuguese%`, 23.0 s for `%zzz%`; with this index, 0.01 s.
/// - `diagram_tag_links (tag_id)` -- the tag-chip filter's `SELECT entry_id ... WHERE
///   tag_id = ?`. `tag_id` is the second column of that table's composite primary key, so
///   its automatic index cannot serve a lookup that does not also constrain `entry_id`.
pub(in crate::db::sqlite) const SEARCH_INDEXES_SQL: &str = "
    CREATE INDEX IF NOT EXISTS idx_angle_settings_detail_id
        ON angle_settings (detail_id);

    CREATE INDEX IF NOT EXISTS idx_diagram_tag_links_tag_id
        ON diagram_tag_links (tag_id);
";

/// `diagram_previews`' full `CREATE TABLE IF NOT EXISTS` text, shared verbatim between
/// [`crate::db::sqlite::Database::migrate_diagram_previews_table`] and
/// `create_tables_if_not_exist` so the two can never define this table differently.
pub(in crate::db::sqlite) const DIAGRAM_PREVIEWS_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS diagram_previews (
        entry_id INTEGER PRIMARY KEY,
        preview_front BLOB,
        preview_top BLOB,
        preview_material TEXT,
        preview_generated_at INTEGER,
        FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
    );
";

/// `tags`/`diagram_tag_links`' full `CREATE TABLE IF NOT EXISTS` text, shared verbatim
/// between [`crate::db::sqlite::Database::migrate_tag_tables`] and
/// `create_tables_if_not_exist`, same convention as [`DIAGRAM_PREVIEWS_TABLE_SQL`].
pub(in crate::db::sqlite) const TAG_TABLES_SQL: &str = "
    CREATE TABLE IF NOT EXISTS tags (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL UNIQUE COLLATE NOCASE
    );

    CREATE TABLE IF NOT EXISTS diagram_tag_links (
        entry_id INTEGER NOT NULL,
        tag_id INTEGER NOT NULL,
        PRIMARY KEY (entry_id, tag_id),
        FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE,
        FOREIGN KEY (tag_id) REFERENCES tags (id) ON DELETE CASCADE
    );
";

/// Builds `diagram_tilt_curves`' full `CREATE TABLE IF NOT EXISTS` text, including its
/// 6 derived global-extreme columns generated from
/// [`crate::model::performance::all_global_extreme_columns`] (not hand-listed), shared
/// verbatim between the migration path and `create_tables_if_not_exist`. A function,
/// not a `const` like [`DIAGRAM_PREVIEWS_TABLE_SQL`], since the derived-column tail
/// needs iterating `all_global_extreme_columns` at runtime.
pub(in crate::db::sqlite) fn diagram_tilt_curves_table_sql() -> String {
    let mut sql = String::from(
        "CREATE TABLE IF NOT EXISTS diagram_tilt_curves (
    entry_id INTEGER PRIMARY KEY,
    curves BLOB,
    curve_image BLOB,
    generated_at INTEGER,\n",
    );
    for (metric, extreme) in crate::model::performance::all_global_extreme_columns() {
        let _ = writeln!(
            sql,
            "    {} REAL,",
            global_extreme_column_name(metric, extreme)
        );
    }
    sql.push_str(
        "    FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE\n);",
    );
    sql
}

/// The 30 `diagram_tilt_curves` columns this crate's first-draft 36-column schema
/// created that do not survive into the current 6-column shape -- see
/// [`crate::db::sqlite::Database::migrate_prune_tilt_curve_aggregate_columns`]. A frozen
/// literal list: the enum/functions that would generate these names no longer exist in
/// this crate.
pub(super) const OBSOLETE_TILT_CURVE_AGGREGATE_COLUMNS: &[&str] = &[
    "perf_brilliance_15_min",
    "perf_brilliance_15_max",
    "perf_brilliance_15_mean",
    "perf_extinction_15_min",
    "perf_extinction_15_max",
    "perf_extinction_15_mean",
    "perf_windowing_15_min",
    "perf_windowing_15_max",
    "perf_windowing_15_mean",
    "perf_brilliance_30_min",
    "perf_brilliance_30_max",
    "perf_brilliance_30_mean",
    "perf_extinction_30_min",
    "perf_extinction_30_max",
    "perf_extinction_30_mean",
    "perf_windowing_30_min",
    "perf_windowing_30_max",
    "perf_windowing_30_mean",
    "perf_brilliance_45_min",
    "perf_brilliance_45_max",
    "perf_brilliance_45_mean",
    "perf_extinction_45_min",
    "perf_extinction_45_max",
    "perf_extinction_45_mean",
    "perf_windowing_45_min",
    "perf_windowing_45_max",
    "perf_windowing_45_mean",
    "perf_brilliance_90_mean",
    "perf_extinction_90_mean",
    "perf_windowing_90_mean",
];
