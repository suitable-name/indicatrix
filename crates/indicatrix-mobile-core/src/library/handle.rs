//! [`LibraryHandle`]: the open vault database behind a mutex, shared by clones, and
//! [`LibraryOpener`], the `Send` token that opens it on a worker thread.

use std::{
    fmt,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use indicatrix_vault::db::sqlite::Database;

use crate::platform::AppDirs;

/// The database file's name inside `<files>/library/`; the desktop's own default name.
const DATABASE_FILE: &str = "facet_diagrams.sqlite";

/// The library database. Cheap to clone; all clones share one connection, so a call
/// waits while another clone's call runs. Use it from worker threads, never the UI
/// thread.
#[derive(Clone)]
pub struct LibraryHandle {
    pub(super) db: Arc<Mutex<Database>>,
    /// Bumped by every write; shared with `LibraryState::revision`.
    pub(super) revision: Arc<AtomicU64>,
    /// `<cache>`: thumbnails live in `<cache>/thumbs/`.
    pub(super) cache_dir: PathBuf,
}

impl fmt::Debug for LibraryHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibraryHandle")
            .field("revision", &self.revision.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl LibraryHandle {
    /// The database under `<files>/library/`, created with its schema when missing.
    fn open(dirs: &AppDirs, revision: Arc<AtomicU64>) -> Result<Self, String> {
        let folder = dirs.files.join("library");
        std::fs::create_dir_all(&folder)
            .map_err(|error| format!("The library folder could not be created: {error}."))?;
        let path = folder.join(DATABASE_FILE);
        let path_text = path
            .to_str()
            .ok_or_else(|| "The library's path is not valid text.".to_owned())?;
        let database = Database::new(Some(path_text))
            .map_err(|error| format!("The library could not be opened: {error:#}."))?;
        Ok(Self {
            db: Arc::new(Mutex::new(database)),
            revision,
            cache_dir: dirs.cache.clone(),
        })
    }

    /// The database, waiting for any other call to finish. A call that panicked while
    /// holding it does not poison the library: SQLite rolls its transaction back.
    pub(super) fn lock(&self) -> MutexGuard<'_, Database> {
        self.db.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records that the library changed.
    pub(super) fn bump(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    /// The folder the thumbnails are cached in.
    pub(super) fn thumbs_dir(&self) -> PathBuf {
        self.cache_dir.join("thumbs")
    }
}

/// A `Send` token that opens the library database; see `LibraryState::opener`.
#[derive(Debug, Clone)]
pub struct LibraryOpener {
    dirs: AppDirs,
    revision: Arc<AtomicU64>,
}

impl LibraryOpener {
    pub(super) const fn new(dirs: AppDirs, revision: Arc<AtomicU64>) -> Self {
        Self { dirs, revision }
    }

    /// Opens the database (creating the folder and the schema when missing).
    ///
    /// # Errors
    /// A sentence when the database cannot be created or opened.
    pub fn open(&self) -> Result<LibraryHandle, String> {
        LibraryHandle::open(&self.dirs, Arc::clone(&self.revision))
    }
}

/// A closure that turns a database error into a sentence naming what was attempted:
/// `what` followed by the error's chain.
pub(super) fn db_error<E: fmt::Display>(what: &'static str) -> impl Fn(E) -> String {
    move |error| format!("{what}: {error:#}.")
}
