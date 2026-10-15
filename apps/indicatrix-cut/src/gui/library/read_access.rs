//! The one place the library UI opens its own read-only database connection.
//!
//! Every read the library does off the shared `Mutex<Database>` goes through
//! [`with_read_db`]: the shared handle is held by writers and by long workers (a preview
//! batch saving PNGs, a whole-catalogue refresh), and a read that queued behind it froze
//! the UI thread for that whole hold (perf review DB-1). WAL lets the read-only
//! connection proceed alongside them.

use indicatrix_vault::db::sqlite::Database;
use std::sync::{Arc, Mutex, PoisonError};

/// Runs `read` against the catalogue through a fresh read-only connection to
/// [`crate::gui::DB_PATH`], or through the shared handle when no second connection is
/// possible (the `:memory:` fallback library; see [`Database::with_read_connection`]).
///
/// Unit tests always take the shared handle: their databases are temp files, and
/// `DB_PATH` is the owner's real catalogue relative to the working directory.
pub(in crate::gui::library) fn with_read_db<T>(
    db: &Arc<Mutex<Database>>,
    read: impl FnOnce(&Database) -> T,
) -> T {
    if cfg!(test) {
        let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
        return read(&guard);
    }
    Database::with_read_connection(crate::gui::DB_PATH, db, read)
}
