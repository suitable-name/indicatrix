//! Read access that does not queue behind the application's shared writer handle.
//!
//! The GUI keeps one `Mutex<Database>` for every write. Reads that went through the same
//! mutex waited behind whatever long operation a worker held it for (a preview batch
//! saving PNGs, a whole-catalogue refresh), so a row click or a slider tick could stall
//! for that whole hold. WAL lets an independent read-only connection read alongside the
//! writer; this module is the one place that opens such a connection and falls back to
//! the shared handle when it cannot.

use super::{Database, is_memory_db};
use std::sync::{Mutex, PoisonError};
use tracing::warn;

impl Database {
    /// Runs `read` against a fresh read-only connection to `db_path`, or against the
    /// shared `shared` handle when no second connection is possible.
    ///
    /// Opening costs well under a millisecond (no schema work, see
    /// [`Database::open_read_only`]); the gain is that `read` never takes the shared
    /// lock, so it neither waits for a worker holding it nor makes the UI thread wait
    /// for this read. The fallback exists for the `:memory:` database (it only lives
    /// inside the already-open handle) and for a file that cannot be opened read-only;
    /// both keep working through the shared lock, a poisoned one included.
    ///
    /// Reads only: a write issued from `read` would fail on the read-only connection
    /// and silently succeed on the fallback, so keep writes on the shared handle.
    pub fn with_read_connection<T>(
        db_path: &str,
        shared: &Mutex<Self>,
        read: impl FnOnce(&Self) -> T,
    ) -> T {
        if !is_memory_db(db_path) {
            match Self::open_read_only(db_path) {
                Ok(read_db) => return read(&read_db),
                Err(e) => warn!(
                    "could not open a dedicated read-only connection to {db_path:?} ({e}); \
                     reading through the shared database lock instead"
                ),
            }
        }
        let guard = shared.lock().unwrap_or_else(PoisonError::into_inner);
        read(&guard)
    }
}
