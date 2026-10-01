//! The `CREATE TABLE` statements of the tables `Database::create_tables_if_not_exist`
//! defines itself, one constant per table. The side tables shared with the migrations
//! (`diagram_previews`, `diagram_tilt_curves`, the solid caches, the tags and the search
//! indexes) keep their single definition in `migrations`.

/// The `diagram_entries` table: one row per design, keyed by its unique `url`.
pub(super) const DIAGRAM_ENTRIES_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS diagram_entries (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        title TEXT NOT NULL,
        url TEXT NOT NULL UNIQUE,
        design_id TEXT,
        source_id TEXT NOT NULL DEFAULT 'facetdiagrams.org',
        -- 'ignored' library flag (Database::set_diagram_ignored); see
        -- migrate_ignored_column's doc comment for why NOT NULL DEFAULT 0 is
        -- safe here even though every other purely-additive migration in this
        -- crate adds a nullable column.
        ignored BOOLEAN NOT NULL DEFAULT 0,
        -- Provenance: the entry this row was derived from (e.g. an
        -- export-then-reimport of an existing catalogue design), NULL when
        -- unknown/not applicable -- see migrate_diagram_entries_provenance's
        -- doc comment. No FOREIGN KEY: the source row can be deleted
        -- independently without this column blocking or cascading that delete.
        derived_from_entry_id INTEGER,
        -- When this row was first created / last had its title or detail
        -- metadata changed, in Unix seconds. Both NULL for a design that
        -- predates this column -- never backfilled with a fabricated time, see
        -- migrate_diagram_entries_timestamps's doc comment.
        created_at INTEGER,
        updated_at INTEGER
    );
";

/// The `diagram_details` table: the one detail row per entry (proportions, shape,
/// designer, source files and the diagram image BLOB, declared last).
pub(super) const DIAGRAM_DETAILS_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS diagram_details (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        entry_id INTEGER NOT NULL UNIQUE, -- Each entry should have only one detail record
        page_url TEXT NOT NULL,
        diagram_image_name TEXT,
        competition_diagram TEXT,
        -- lw_ratio/refractive_index/volume/index_gear are typed numeric here
        -- (not TEXT) so a fresh database never needs migrate_numeric_columns'
        -- DROP-COLUMN/RENAME-COLUMN retype cycle -- see that migration's doc
        -- comment for why it's gated on the actual PRAGMA table_info type
        -- rather than column presence, precisely so it can detect and skip
        -- past a table already shaped like this one.
        lw_ratio REAL,
        refractive_index REAL,
        index_gear INTEGER,
        volume REAL,
        facets_count TEXT,
        -- facets/girdle_facets: queryable split of facets_count (e.g. 55+6),
        -- see parse_facets_count and Self::save_diagram_detail/
        -- Self::update_diagram_metadata, which derive and write both at save
        -- time. Declared here (not just added by migrate_numeric_columns) for
        -- the same reason as the four columns above.
        facets INTEGER,
        girdle_facets INTEGER,
        shape TEXT,
        designer_info TEXT,
        hw_ratio REAL,
        tw_ratio REAL,
        uw_ratio REAL,
        pw_ratio REAL,
        cw_ratio REAL,
        symmetry_order INTEGER,
        mirror_symmetry BOOLEAN,
        designer TEXT,
        source_citation TEXT,
        pdf_file TEXT,
        gem_file TEXT,
        shape_category INTEGER,
        -- Declared LAST, after every searched/text column -- see
        -- Database::migrate_blob_columns_last's doc comment: SQLite reads a
        -- row's columns in physical order, so a query that never touches this
        -- BLOB still pays to skip its bytes if it sits earlier. Measured on the
        -- real catalogue: a text search over this table costs 40.9/39.2 ms with
        -- this column at position 5 (its original spot), 16.5 ms with it here.
        diagram_image_data BLOB,
        FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
    );
";

/// The `angle_settings` table: one row per cutting-instruction line of a detail.
pub(super) const ANGLE_SETTINGS_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS angle_settings (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        order_idx INTEGER NOT NULL, 
        detail_id INTEGER NOT NULL,
        facet TEXT NOT NULL,
        angle TEXT NOT NULL,
        index_val TEXT NOT NULL, -- 'index' is a reserved keyword in SQL
        notes TEXT NOT NULL,
        FOREIGN KEY (detail_id) REFERENCES diagram_details (id) ON DELETE CASCADE
    );
";

/// The `attached_files` table: the files stored with a detail.
pub(super) const ATTACHED_FILES_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS attached_files (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        detail_id INTEGER NOT NULL,
        name TEXT NOT NULL,
        url TEXT NOT NULL,
        content BLOB NOT NULL,
        FOREIGN KEY (detail_id) REFERENCES diagram_details (id) ON DELETE CASCADE
    );
";

/// The `custom_gem_materials` table: the user-defined gemstone materials.
pub(super) const CUSTOM_GEM_MATERIALS_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS custom_gem_materials (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL UNIQUE,
        refractive_index REAL NOT NULL,
        dispersion REAL NOT NULL,
        birefringence REAL NOT NULL,
        absorption_r REAL NOT NULL,
        absorption_g REAL NOT NULL,
        absorption_b REAL NOT NULL,
        crystal_system TEXT,
        optical_character TEXT,
        biaxial_delta_beta_alpha REAL,
        -- Nullable per-axis dispersion coefficients (JSON); see
        -- migrations::Database::migrate_per_axis_dispersion_column's doc
        -- comment for why this is one JSON column rather than several new
        -- REAL ones.
        per_axis_dispersion_json TEXT,
        -- Nullable specific gravity (density relative to water); see
        -- migrations::Database::migrate_custom_material_specific_gravity's
        -- doc comment.
        specific_gravity REAL
    );
";

/// The `library_mirror_state` table: pull-mirror sync bookkeeping per mirrored design.
pub(super) const LIBRARY_MIRROR_STATE_TABLE_SQL: &str = "
    -- Pull-mirror sync (see `crate::model::mirror`): the last remote
    -- content hashes seen for a design mirrored from a remote library server,
    -- keyed by the same `url` that already governs `diagram_entries`' own
    -- cross-sync identity. Purely additive bookkeeping -- an install with no
    -- remote configured never gains a row here, and this table's absence of
    -- data never changes any query against `diagram_entries`/`diagram_details`.
    CREATE TABLE IF NOT EXISTS library_mirror_state (
        url TEXT PRIMARY KEY,
        source_id TEXT NOT NULL,
        summary_version BLOB NOT NULL,
        design_version BLOB NOT NULL,
        -- Tombstone set by delete_diagram_entry: the user deleted this mirrored
        -- design locally and a sync must not bring it back; see
        -- migrate_mirror_state_tombstone_column's doc comment.
        deleted_locally INTEGER NOT NULL DEFAULT 0
    );
";

/// The `shape_vocabulary` table: the canonical shape list.
pub(super) const SHAPE_VOCABULARY_TABLE_SQL: &str = "
    -- Canonical shape vocabulary (see `DEFAULT_SHAPES`), seeded by
    -- `migrate_shape_vocabulary` -- created here too (`IF NOT EXISTS`) so a
    -- fresh database already has the table before that migration runs, the
    -- same convention every other table on this list follows. A plain lookup
    -- list, not a FK target for `diagram_details.shape`: the real catalogue
    -- holds free-text scraped shape strings no fixed vocabulary covers, and a
    -- FK constraint would either reject them or force a lossy migration of
    -- real data (see `Database::get_unique_shapes`).
    CREATE TABLE IF NOT EXISTS shape_vocabulary (
        name TEXT PRIMARY KEY,
        sort_order INTEGER NOT NULL
    );
";

/// Every table defined here, in creation order.
pub(super) const BASE_TABLES_SQL: [&str; 7] = [
    DIAGRAM_ENTRIES_TABLE_SQL,
    DIAGRAM_DETAILS_TABLE_SQL,
    ANGLE_SETTINGS_TABLE_SQL,
    ATTACHED_FILES_TABLE_SQL,
    CUSTOM_GEM_MATERIALS_TABLE_SQL,
    LIBRARY_MIRROR_STATE_TABLE_SQL,
    SHAPE_VOCABULARY_TABLE_SQL,
];
