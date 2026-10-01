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
/// SQLite does not index a `FOREIGN KEY` column on its own, and none of these three is
/// the LEADING column of a primary key, so without this trio every lookup below degrades
/// to a full table scan *per candidate design*:
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
/// - `attached_files (detail_id)` -- serves every "load this design's attachments" fetch
///   ([`crate::db::sqlite::Database::get_diagram_full`]/`get_diagram_full_meta`), the
///   detail pane's attachment list, and `indicatrix-worker`'s per-request attachment
///   fetch, plus the `ON DELETE CASCADE` sweep a `diagram_details` delete/re-sync
///   triggers. Measured on the real catalogue via `EXPLAIN QUERY PLAN`: `SCAN
///   attached_files` without this index (11-34 ms per lookup, ~95 s of scanning across a
///   full mirror re-save while the UI mutex is held); `SEARCH attached_files USING INDEX
///   idx_attached_files_detail_id (detail_id=?)` with it.
pub(in crate::db::sqlite) const SEARCH_INDEXES_SQL: &str = "
    CREATE INDEX IF NOT EXISTS idx_angle_settings_detail_id
        ON angle_settings (detail_id);

    CREATE INDEX IF NOT EXISTS idx_diagram_tag_links_tag_id
        ON diagram_tag_links (tag_id);

    CREATE INDEX IF NOT EXISTS idx_attached_files_detail_id
        ON attached_files (detail_id);
";

/// `diagram_previews`' full `CREATE TABLE IF NOT EXISTS` text, shared verbatim between
/// [`crate::db::sqlite::Database::migrate_diagram_previews_table`] and
/// `create_tables_if_not_exist` so the two can never define this table differently.
///
/// `preview_material`/`preview_generated_at`/`params_fingerprint` are declared BEFORE
/// the two PNG blobs (`preview_front`/`preview_top`) -- see
/// [`crate::db::sqlite::Database::migrate_blob_columns_last`]'s doc comment for why blob
/// columns sit last: a fresh database gets this order directly, so that migration only
/// ever has real work to do against a database created before this file adopted it.
///
/// `params_fingerprint` is an opaque caller-built string naming what the stored images
/// were rendered with (renderer build, image size, sample count, bounce cap, lighting,
/// material); `NULL` on a row written before the column existed.
/// [`crate::db::sqlite::Database::entry_ids_missing_previews`] compares it with the
/// fingerprint a render made now would carry, so a changed renderer or setting marks the
/// old images outdated instead of leaving them indistinguishable from current ones.
pub(in crate::db::sqlite) const DIAGRAM_PREVIEWS_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS diagram_previews (
        entry_id INTEGER PRIMARY KEY,
        preview_material TEXT,
        preview_generated_at INTEGER,
        params_fingerprint TEXT,
        preview_front BLOB,
        preview_top BLOB,
        FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
    );
";

/// [`DIAGRAM_PREVIEWS_TABLE_SQL`], but naming a `__reordered` staging table (no
/// `IF NOT EXISTS`, since the staging table must never already exist) --
/// [`crate::db::sqlite::Database::migrate_blob_columns_last`]'s rebuild target for an
/// existing `diagram_previews` still in its pre-fix, blob-first column order, and
/// [`crate::db::sqlite::Database::migrate_diagram_previews_table`]'s rebuild target for a
/// table that predates `params_fingerprint`.
pub(in crate::db::sqlite) const DIAGRAM_PREVIEWS_REORDERED_TABLE_SQL: &str = "
    CREATE TABLE diagram_previews__reordered (
        entry_id INTEGER PRIMARY KEY,
        preview_material TEXT,
        preview_generated_at INTEGER,
        params_fingerprint TEXT,
        preview_front BLOB,
        preview_top BLOB,
        FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
    );
";

/// `diagram_solid_extents`' full `CREATE TABLE IF NOT EXISTS` text, shared verbatim
/// between [`crate::db::sqlite::Database::migrate_diagram_solid_extents_table`] and
/// `create_tables_if_not_exist`, same convention as [`DIAGRAM_PREVIEWS_TABLE_SQL`].
///
/// No BLOB column, so unlike `diagram_previews`/`diagram_tilt_curves` there is no
/// `__reordered` staging variant and no entry in
/// [`crate::db::sqlite::Database::migrate_blob_columns_last`]. The six extents columns
/// are all NULL for a "measured, unusable" row; `measured_at` is Unix seconds, like
/// `diagram_tilt_curves.generated_at`.
pub(in crate::db::sqlite) const DIAGRAM_SOLID_EXTENTS_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS diagram_solid_extents (
        entry_id INTEGER PRIMARY KEY,
        width_caliper REAL,
        length_caliper REAL,
        width_axis REAL,
        length_axis REAL,
        height REAL,
        volume REAL,
        source TEXT NOT NULL,
        measured_at INTEGER NOT NULL,
        extents_version INTEGER NOT NULL,
        FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
    );
";

/// `diagram_solid_hull`'s full `CREATE TABLE IF NOT EXISTS` text, shared verbatim
/// between [`crate::db::sqlite::Database::migrate_diagram_solid_hull_table`] and
/// `create_tables_if_not_exist`, same convention as [`DIAGRAM_PREVIEWS_TABLE_SQL`].
///
/// `vertices` is a packed BLOB of little-endian `[f32; 3]` triples, declared LAST per
/// the house convention (see [`crate::db::sqlite::Database::migrate_blob_columns_last`]).
/// A new table created with the BLOB last from the start needs no `__reordered` staging
/// variant.
pub(in crate::db::sqlite) const DIAGRAM_SOLID_HULL_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS diagram_solid_hull (
        entry_id INTEGER PRIMARY KEY,
        hull_version INTEGER NOT NULL,
        vertex_count INTEGER NOT NULL,
        measured_at INTEGER NOT NULL,
        vertices BLOB NOT NULL,
        FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
    );
";

/// `saved_rough_plans`' full `CREATE TABLE IF NOT EXISTS` text, shared verbatim
/// between [`crate::db::sqlite::Database::migrate_saved_rough_plans_table`] and
/// `create_tables_if_not_exist`.
///
/// `summary` (the one-line list description, see
/// [`crate::model::saved_rough_plan::SavedRoughPlanMeta::summary`]) sits before the
/// large `payload` text so reading it never walks the payload's overflow pages; a table
/// that predates it gets the column appended by the migration instead.
pub(in crate::db::sqlite) const SAVED_ROUGH_PLANS_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS saved_rough_plans (
        plan_id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        payload_version INTEGER NOT NULL,
        summary TEXT,
        payload TEXT NOT NULL
    );
";

/// The `saved_rough_plans` column a table created before list summaries lacks.
pub(in crate::db::sqlite) const SAVED_ROUGH_PLANS_SUMMARY_COLUMN: &str = "summary";

/// `diagram_planner_exclusions`' full `CREATE TABLE IF NOT EXISTS` text, shared verbatim
/// between [`crate::db::sqlite::Database::migrate_planner_exclusion_table`] and
/// `create_tables_if_not_exist`, same convention as [`DIAGRAM_PREVIEWS_TABLE_SQL`].
///
/// One row per design the Rough Planner must leave out of its candidate set; the mere
/// presence of the row is the flag, so the table carries no other column and no BLOB. It
/// is a side table rather than a column on `diagram_entries` so that setting or clearing
/// the flag never touches the entry row (see
/// [`crate::db::sqlite::Database::set_planner_excluded`] for why that matters).
pub(in crate::db::sqlite) const DIAGRAM_PLANNER_EXCLUSIONS_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS diagram_planner_exclusions (
        entry_id INTEGER PRIMARY KEY,
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
///
/// `generated_at`/`params_fingerprint`/the 6 `perf_*` columns are declared BEFORE the two
/// BLOB columns (`curves`/`curve_image`) -- see
/// [`crate::db::sqlite::Database::migrate_blob_columns_last`]'s doc comment for why
/// blob columns sit last: a fresh database gets this order directly, so that migration
/// only ever has real work to do against a database created before this file adopted
/// it.
///
/// `params_fingerprint` is an opaque caller-built string naming what the stored sweep
/// was computed with (renderer build, lighting pose, material); `NULL` on a row written
/// before the column existed. See [`DIAGRAM_PREVIEWS_TABLE_SQL`] for how it is used.
/// `curve_image` is never written (nothing renders a stored graph); the column stays so
/// that an existing database needs no rebuild to drop it.
pub(in crate::db::sqlite) fn diagram_tilt_curves_table_sql() -> String {
    diagram_tilt_curves_table_sql_named("diagram_tilt_curves", true)
}

/// [`diagram_tilt_curves_table_sql`], but naming a `__reordered` staging table (no
/// `IF NOT EXISTS`, since the staging table must never already exist) --
/// [`crate::db::sqlite::Database::migrate_blob_columns_last`]'s rebuild target for an
/// existing `diagram_tilt_curves` still in its pre-fix, blob-first column order.
/// Shares [`diagram_tilt_curves_table_sql_named`] with [`diagram_tilt_curves_table_sql`]
/// so the two column lists -- in particular the 6 generated `perf_*` names -- can never
/// drift apart.
pub(in crate::db::sqlite) fn diagram_tilt_curves_reordered_table_sql() -> String {
    diagram_tilt_curves_table_sql_named("diagram_tilt_curves__reordered", false)
}

/// Shared by [`diagram_tilt_curves_table_sql`]/[`diagram_tilt_curves_reordered_table_sql`]:
/// builds `CREATE TABLE [IF NOT EXISTS] {table_name} (...)` with `entry_id`,
/// `generated_at`, `params_fingerprint`, and the 6 derived `perf_*` REAL columns first
/// and the `curves`/`curve_image` BLOBs last.
fn diagram_tilt_curves_table_sql_named(table_name: &str, if_not_exists: bool) -> String {
    let if_not_exists = if if_not_exists { "IF NOT EXISTS " } else { "" };
    let mut sql = format!(
        "CREATE TABLE {if_not_exists}{table_name} (
    entry_id INTEGER PRIMARY KEY,
    generated_at INTEGER,
    params_fingerprint TEXT,\n"
    );
    for (metric, extreme) in crate::model::performance::all_global_extreme_columns() {
        let _ = writeln!(
            sql,
            "    {} REAL,",
            global_extreme_column_name(metric, extreme)
        );
    }
    sql.push_str(
        "    curves BLOB,
    curve_image BLOB,
    FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE\n);",
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
