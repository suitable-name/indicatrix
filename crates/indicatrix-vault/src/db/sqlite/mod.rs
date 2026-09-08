//! The SQLite-backed [`Database`]: schema creation/migration, and the two ways to open
//! it -- [`Database::new`] (read-write) and [`Database::open_read_only`].
//!
//! # Concurrency model
//!
//! Exactly one process opens this file read-write at a time -- `apps/indicatrix-cut`,
//! whose own mirror-sync writer thread and UI thread can both be issuing statements
//! against that single read-write [`Connection`] concurrently (a `Connection` is `Send`
//! but not `Sync`, so within that process access is already serialized by whatever
//! `Mutex`/channel wraps it). Any number of *other* processes -- most notably
//! `apps/indicatrix-worker`'s `serve`, one short-lived [`Database::open_read_only`]
//! connection per accepted connection -- read the same file concurrently from outside
//! that process.
//!
//! [`Database::new`] enables `PRAGMA journal_mode=WAL` (skipped for `:memory:`, where
//! the pragma is meaningless -- see [`is_memory_db`]) plus `PRAGMA synchronous=NORMAL`,
//! which WAL makes safe (durable across an application crash; only an OS crash or power
//! loss between the WAL commit and its later checkpoint can lose the last few commits --
//! an acceptable trade for a local design-library file, not a system of record). WAL is
//! what lets that one writer proceed without blocking every reader, and vice versa,
//! instead of every reader colliding with the writer under the old rollback-journal
//! mode. Both connections additionally set a 5s `busy_timeout` as a second line of
//! defense against the write-lock acquisition itself (`BEGIN IMMEDIATE`/`COMMIT`), which
//! WAL narrows to a much smaller window but doesn't eliminate.
//!
//! A WAL database's un-checkpointed writes live in a separate `-wal` file (with a
//! `-shm` index alongside it) until something checkpoints them back into the main file
//! -- SQLite does this automatically as the `-wal` file grows, but that's a poor fit for
//! any process that copies the `.sqlite` file itself (a backup, an export, a bundle
//! handed to another machine) rather than opening it through SQLite: a raw copy of just
//! the main file silently drops every write still sitting in `-wal`. [`Database::checkpoint`]
//! exists for exactly that case -- call it immediately before any such copy so the main
//! file alone is a complete, consistent snapshot. Nothing in this workspace performs
//! that kind of raw file copy today (`indicatrix-worker` always opens the live file
//! directly via [`Database::open_read_only`], never a copy of it), so no call site wires
//! this in yet; it's here for the first future copy/export/backup path that needs it.
//!
//! [`Database::open_read_only`] tries the plain read-only open first. A WAL database
//! additionally needs its `-shm` file to be creatable/writable (or already present) for
//! *any* connection, reader included, to map the shared index that coordinates against
//! the writer -- so a read-only connection into a WAL database sitting in a directory
//! the reader's process can't write to (permissions, a read-only mount) fails with
//! `SQLITE_CANTOPEN`/`SQLITE_READONLY` even though the connection itself only asked to
//! read. On that failure, [`Database::open_read_only`] retries once with the SQLite URI
//! option `immutable=1`, which tells SQLite the file is guaranteed not to change for the
//! life of the connection -- it then skips the `-shm`/locking machinery entirely and
//! reads the main file directly. The trade-off: an `immutable=1` connection will not see
//! writes committed by another connection after it was opened (no change detection --
//! each new logical request needs a fresh connection to observe fresh data, which is
//! already this crate's usage pattern: `indicatrix-worker` opens one connection per
//! accepted connection rather than holding one open across requests).

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use tracing::{debug, info};

mod entries;
mod materials;
mod migrations;
mod mirror_state;
mod previews;
mod search;
#[cfg(test)]
mod tests;
mod tilt_curves;

pub use materials::CustomMaterialParams;
pub use search::SEARCH_RESULT_CAP;

// The following imports have no production use *in this file* -- they exist solely so
// that `tests`' `use super::*;` (see `tests.rs`, moved unaltered) resolves. Every name
// below is used by production code in one of this module's submodules already; this is
// an extra, test-only binding of the same items into this module's own namespace, gated
// out of non-test builds so it can never trigger an unused-import warning there.
#[cfg(test)]
use crate::model::{
    detail::FacetDiagramDetail, entry::FacetDiagramEntry, filter::RangeFilter,
    metadata_update::MetadataUpdate,
};
#[cfg(test)]
use rusqlite::params;
#[cfg(test)]
use search::percentile_of_sorted;

/// The default database file [`Database::new`] opens when given `None`.
///
/// A path resolved relative to the process's current working directory. `pub` so a
/// caller that wants to report or reuse this default (e.g. `indicatrix-worker`'s
/// `serve --db`, which falls back to this exact path) doesn't have to duplicate the
/// literal.
pub const DEFAULT_DB_FILE: &str = "facet_diagrams.sqlite";

/// The `source_id` every row synced before the `source_id` column existed is
/// backfilled with -- see `Database::migrate_source_id_column`.
///
/// Every design predating the `source_id` column has exactly one possible origin, so
/// backfilling them all to this value is a historical fact about that data, not a
/// guess.
///
/// Deliberately a plain string literal: `db` is the lower layer and must not depend on
/// whatever produces any particular `source_id`. `pub` so that a crate which CAN see
/// both this constant and the identifier it has to match is able to assert the two
/// stay equal -- that assertion cannot live here, because from here only one side of
/// it is visible.
pub const LEGACY_SOURCE_ID: &str = "facetdiagrams.org";

/// The canonical faceting-design shape vocabulary, most-common first and freeform
/// last (deliberate order -- e.g. a GUI shape picker can present it as-is without
/// re-sorting).
///
/// This is what seeds the `shape_vocabulary` table on every [`Database::new`] (see
/// `Database::migrate_shape_vocabulary`), and it's `pub` so another crate building an
/// import/assignment flow (e.g. `apps/indicatrix-cut`) can offer the same list as a
/// picker without a round trip through the database -- there is exactly one
/// definition of this list, here.
///
/// This is a *starting* vocabulary, not an exhaustive one: the real catalogue
/// contains free-text scraped shape strings this list doesn't cover (see
/// `Database::get_unique_shapes`, which unions this list with whatever actually
/// appears in `diagram_details.shape` so neither source drops the other's values).
pub const DEFAULT_SHAPES: &[&str] = &[
    "Round",
    "Oval",
    "Cushion",
    "Square",
    "Rectangle",
    "Emerald",
    "Pear",
    "Marquise",
    "Heart",
    "Triangle",
    "Trillion",
    "Hexagon",
    "Octagon",
    "Pentagon",
    "Kite",
    "Rhombus",
    "Shield",
    "Star",
    "Barion",
    "Briolette",
    "Freeform",
];

/// Whether `path` names SQLite's special in-memory database, for which
/// `journal_mode=WAL` is meaningless (SQLite always reports `journal_mode` back as
/// `memory` there and refuses to change it). Only the exact literal SQLite itself
/// recognizes -- a URI form like `file::memory:` would need its own `SQLITE_OPEN_URI`
/// handling this crate doesn't otherwise use, and no caller in this workspace passes
/// one (see `Database::new`'s callers).
fn is_memory_db(path: &str) -> bool {
    path == ":memory:"
}

/// Enables WAL journal mode and `synchronous=NORMAL` on `conn`, opened at `path`. Never
/// fails `Database::new` on its own -- a WAL attempt can fail for reasons outside this
/// database's control (a network filesystem SQLite's WAL doesn't support, a read-only
/// parent directory that can't hold the new `-wal`/`-shm` files), and falling back to
/// SQLite's default rollback-journal mode is a safe, correct degradation, just a slower
/// one under concurrent access. Logged at debug either way so a deployment that
/// unexpectedly never gets WAL has a way to notice.
fn enable_wal(conn: &Connection, path: &str) {
    match conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0)) {
        Ok(mode) if mode.eq_ignore_ascii_case("wal") => {
            debug!("{path}: journal_mode=WAL enabled");
        }
        Ok(mode) => {
            debug!(
                "{path}: requested journal_mode=WAL but SQLite reports {mode:?} -- continuing with it \
                 (common cause: the database file lives on a filesystem WAL doesn't support, e.g. a network share)"
            );
        }
        Err(e) => {
            debug!(
                "{path}: failed to set journal_mode=WAL ({e}); continuing with the existing journal mode"
            );
        }
    }

    // Safe under WAL specifically: a commit is still durable across an application
    // crash (the WAL frame is on disk before `COMMIT` returns), and only an OS crash or
    // power loss between that commit and its later checkpoint could lose it -- an
    // acceptable trade here, not a system of record. Best-effort: leaving `synchronous`
    // at its previous value if this fails costs nothing this database relies on.
    if let Err(e) = conn.pragma_update(None, "synchronous", "NORMAL") {
        debug!(
            "{path}: failed to set synchronous=NORMAL ({e}); continuing with the existing setting"
        );
    }
}

/// Builds a `file:` URI naming `path` with the `immutable=1` query option, for
/// [`Database::open_read_only`]'s fallback -- see that method and this module's doc
/// comment.
///
/// Percent-encodes the characters SQLite's URI filenames treat specially (`?`, `#`,
/// `%`) and normalizes Windows `\` separators to `/`: SQLite's URI parser accepts an
/// absolute path like `file:C:/path/to/db.sqlite` directly, no authority (`//`)
/// component needed.
fn to_sqlite_immutable_uri(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let mut encoded = String::with_capacity(normalized.len());
    for ch in normalized.chars() {
        match ch {
            '%' => encoded.push_str("%25"),
            '?' => encoded.push_str("%3f"),
            '#' => encoded.push_str("%23"),
            other => encoded.push(other),
        }
    }
    format!("file:{encoded}?immutable=1")
}

pub struct Database {
    conn: Connection,
}

impl Database {
    /// Creates a new Database instance, connecting to the SQLite database file.
    /// Creates the file and tables if they don't exist.
    ///
    /// # Arguments
    /// * `db_path` - Optional path to the database file. Defaults to "`facet_diagrams.sqlite`".
    ///
    /// # Errors
    ///
    /// Returns an error if the SQLite file at `db_path` cannot be opened (e.g. bad
    /// path, permissions, or a file that isn't a valid SQLite database), if enabling
    /// the `foreign_keys` pragma or setting the busy timeout fails, or if creating the
    /// schema (tables/indexes) fails.
    pub fn new(db_path: Option<&str>) -> Result<Self> {
        let path = db_path.unwrap_or(DEFAULT_DB_FILE);
        info!("Connecting to database: {}", path);
        let conn = Connection::open(path).context(format!("Failed to open database at {path}"))?;

        // Enable foreign key constraints. Crucial for data integrity.
        conn.execute("PRAGMA foreign_keys = ON;", [])
            .context("Failed to enable foreign keys")?;

        // WAL lets this connection's writes (this crate's own callers, plus the
        // desktop app's mirror-sync writer thread on the same connection) proceed
        // without blocking a concurrent reader -- including another *process*'s
        // `Database::open_read_only`, e.g. `indicatrix-worker serve` -- instead of the
        // rollback-journal mode's every-writer-blocks-every-reader behavior. See this
        // module's doc comment for the full concurrency model. `:memory:` has no
        // on-disk journal to speak of -- `journal_mode` there is always reported back
        // as `memory` and cannot be changed -- so it's skipped rather than attempted
        // and logged as a no-op every time.
        if is_memory_db(path) {
            debug!("journal_mode=WAL skipped: {path} is an in-memory database");
        } else {
            enable_wal(&conn, path);
        }

        // The desktop app can have a mirror-sync writer thread active alongside UI
        // reads on this same connection's process, so a query can hit SQLITE_BUSY
        // rather than an immediate lock error. Retry internally for up to 5s instead
        // of failing right away. WAL (above) already narrows this window a great deal
        // by letting readers and a writer proceed concurrently; busy_timeout remains
        // as a second line of defense around the writer's own `BEGIN IMMEDIATE`/
        // `COMMIT`, which WAL doesn't make lock-free.
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .context("Failed to set busy timeout")?;

        let db = Self { conn };
        db.create_tables_if_not_exist()?;
        db.migrate_numeric_columns()?;
        db.migrate_source_id_column()?;
        db.migrate_proportions_columns()?;
        db.migrate_designer_and_attachment_columns()?;
        db.migrate_crystal_optics_columns()?;
        db.migrate_per_axis_dispersion_column()?;
        db.migrate_shape_vocabulary()?;
        db.migrate_ignored_column()?;
        db.migrate_diagram_previews_table()?;
        db.migrate_diagram_tilt_curves_table()?;
        db.migrate_prune_tilt_curve_aggregate_columns()?;
        Ok(db)
    }

    /// Opens `db_path` READ-ONLY at the SQLite connection level (`SQLITE_OPEN_READ_ONLY`,
    /// no `SQLITE_OPEN_CREATE`) -- for a caller that must never write to this database,
    /// ever, not even to create it if missing.
    ///
    /// Unlike [`Self::new`], this does NOT run schema creation or any migration: a
    /// read-only connection cannot perform the `CREATE TABLE`/`ALTER TABLE` statements
    /// those need (and, per this method's own contract, must not even try). It assumes
    /// `db_path` already has the schema [`Self::new`] would have brought it to --
    /// appropriate for pointing this at an existing, already-populated catalogue (e.g.
    /// a long-running server reading the user's own library), never for provisioning a
    /// fresh one.
    ///
    /// # Errors
    ///
    /// Returns an error if `db_path` doesn't exist, isn't a valid SQLite database,
    /// can't be opened read-only even with the `immutable=1` fallback (see this
    /// method's doc comment above), or if enabling the `foreign_keys` pragma or setting
    /// the busy timeout fails.
    pub fn open_read_only(db_path: &str) -> Result<Self> {
        info!("Connecting to database (read-only): {}", db_path);
        let read_only_flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;

        let conn = match Connection::open_with_flags(db_path, read_only_flags) {
            Ok(conn) => conn,
            Err(primary_err) => {
                // A WAL database needs its `-shm` file creatable/writable for even a
                // read-only connection (see this module's doc comment) -- if that's
                // what just failed, `immutable=1` sidesteps the requirement entirely.
                // Retried unconditionally rather than trying to distinguish the exact
                // SQLite result code: the fallback is strictly narrower than the
                // primary open (no change detection), so trying it only on failure and
                // still surfacing the original error if it also fails costs nothing.
                debug!(
                    "open_read_only: primary open of {db_path} failed ({primary_err}); retrying with the \
                     immutable=1 URI fallback"
                );
                let uri = to_sqlite_immutable_uri(db_path);
                Connection::open_with_flags(&uri, read_only_flags | OpenFlags::SQLITE_OPEN_URI).map_err(
                    |fallback_err| {
                        anyhow::anyhow!(
                            "Failed to open database read-only at {db_path}: {primary_err} (immutable=1 \
                             fallback also failed: {fallback_err})"
                        )
                    },
                )?
            }
        };

        conn.execute("PRAGMA foreign_keys = ON;", [])
            .context("Failed to enable foreign keys")?;

        // Same rationale as `Self::new`: a concurrent mirror-sync writer can hold the
        // lock briefly, so retry internally for up to 5s instead of failing right
        // away. WAL (enabled by the read-write connection in `Self::new`) already lets
        // this reader proceed alongside that writer in the common case; this remains
        // as a second line of defense.
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .context("Failed to set busy timeout")?;

        Ok(Self { conn })
    }

    /// Runs `PRAGMA wal_checkpoint(TRUNCATE)`: copies every frame currently sitting in
    /// the `-wal` file into the main database file, then truncates `-wal` back to
    /// empty. Call this immediately before copying the `.sqlite` file itself (a backup,
    /// an export, a bundle handed to another machine) -- see this module's doc comment
    /// on why a raw file copy otherwise silently drops un-checkpointed writes. Not
    /// needed around ordinary use of this `Database`; every read/write already goes
    /// through SQLite itself, which always sees the `-wal` file's contents regardless
    /// of whether it's been checkpointed yet.
    ///
    /// A no-op (successful, no error) on a database not in WAL mode, including
    /// `:memory:`.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `PRAGMA wal_checkpoint(TRUNCATE)` fails
    /// outright (e.g. the connection was opened read-only). If it succeeds but could
    /// only partially checkpoint because another connection currently holds a
    /// conflicting lock, that's logged at debug and still returns `Ok(())` -- a
    /// "busy" checkpoint is a normal transient condition under concurrent access, not a
    /// failure of this call.
    pub fn checkpoint(&self) -> Result<()> {
        let (busy, wal_frames, checkpointed_frames): (i64, i64, i64) = self
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE);", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .context("Failed to checkpoint the write-ahead log")?;
        if busy != 0 {
            debug!(
                "wal_checkpoint(TRUNCATE) could not fully checkpoint while busy: {checkpointed_frames}/{wal_frames} \
                 WAL frames checkpointed"
            );
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one long, literal CREATE TABLE-per-table schema definition -- every \
                  existing table this function has always created lives in one place \
                  precisely so the whole schema can be read start to finish in one \
                  scroll; splitting it across several small functions would only make \
                  that harder to audit for no offsetting benefit, the same tradeoff \
                  this file's own migration tests already accept for their \
                  #[expect(clippy::too_many_lines)] setup/assertion blocks"
    )]
    fn create_tables_if_not_exist(&self) -> Result<()> {
        debug!("Ensuring database tables exist...");
        // `diagram_previews`/`diagram_tilt_curves` are spliced in via `format!` rather
        // than hand-copied into this literal a second time -- see
        // `migrations::DIAGRAM_PREVIEWS_TABLE_SQL`/`migrations::
        // diagram_tilt_curves_table_sql`'s own doc comments for why a fresh database's
        // `CREATE TABLE` and an old database's migration must share the exact same SQL
        // text for these two tables (the tilt-curves one in particular: its 6 generated
        // derived-aggregate columns are not something to safely hand-transcribe twice).
        let diagram_previews_sql = migrations::DIAGRAM_PREVIEWS_TABLE_SQL;
        let diagram_tilt_curves_sql = migrations::diagram_tilt_curves_table_sql();
        let sql = format!(
            "BEGIN;

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
                ignored BOOLEAN NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS diagram_details (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entry_id INTEGER NOT NULL UNIQUE, -- Each entry should have only one detail record
                page_url TEXT NOT NULL,
                diagram_image_name TEXT,
                diagram_image_data BLOB,
                competition_diagram TEXT,
                lw_ratio TEXT,
                refractive_index TEXT,
                index_gear TEXT,
                volume TEXT,
                facets_count TEXT,
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
                FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
            );

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

            CREATE TABLE IF NOT EXISTS attached_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                detail_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                url TEXT NOT NULL,
                content BLOB NOT NULL,
                FOREIGN KEY (detail_id) REFERENCES diagram_details (id) ON DELETE CASCADE
            );

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
                per_axis_dispersion_json TEXT
            );

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
                design_version BLOB NOT NULL
            );

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

            -- Cached preview renders and cached tilt-performance curves -- see
            -- migrate_diagram_previews_table/migrate_diagram_tilt_curves_table's own
            -- doc comments for why both are side tables keyed by entry_id (surviving a
            -- diagram_details re-sync) rather than columns on diagram_details itself.
            {diagram_previews_sql}

            {diagram_tilt_curves_sql}

            COMMIT;"
        );
        self.conn
            .execute_batch(&sql)
            .context("Failed to create database tables")?;
        info!("Database tables checked/created successfully.");
        Ok(())
    }
}
