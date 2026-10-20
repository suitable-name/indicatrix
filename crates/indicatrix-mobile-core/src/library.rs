//! The local library: the vault database in the app's private files, imports,
//! thumbnails, search, filters, tags and the design bytes behind each entry.
//!
//! The database is the desktop's own (`indicatrix-vault`, `facet_diagrams.sqlite`, here
//! under `<files>/library/`) with the desktop's storage conventions, so a later mirror
//! sync sees the rows it expects:
//!
//! - every imported or saved design is `diagram_entries.url = local://<file name>` from
//!   the source `local-import`; that prefix is what the "Mine" filter selects;
//! - the design file itself is the first attachment (an `.isdd` file, a `.asc`, or
//!   the `.asc` a `.gem`/`.gcs` converts to with the original beside it);
//! - tags, the ignored mark, the Rough Planner exclusion and the frosting a `.gcs` states
//!   (which `.asc` cannot) live in their side tables.
//!
//! # Threads
//!
//! [`LibraryHandle`] is `Send + Sync + Clone`: the database sits behind a mutex and every
//! call blocks for its duration, so the app calls it from a worker thread and never the
//! UI thread. [`LibraryState`] (on `AppState`) holds the handle and the revision counter
//! a front end compares to know when to re-query.
//!
//! # Layout
//!
//! - `handle`: the handle, opening the database;
//! - `search`: queries, cards and filter options;
//! - `import`: importing picked files;
//! - `store`: reading and writing one entry, tags, marks and delete;
//! - `thumbs`: thumbnails and their cache on disk;
//! - `design_source` and `measure`: reading a stored design and measuring it.

mod design_source;
mod handle;
mod import;
mod measure;
mod search;
mod store;
mod thumbs;
mod variants;

#[cfg(test)]
mod frosting_tests;
#[cfg(test)]
mod tests;

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use crate::{platform::AppDirs, settings::LibrarySort};

pub use handle::{LibraryHandle, LibraryOpener};
pub use indicatrix_vault::model::design_variant::{NewVariant, VariantSummary};
pub use search::QUERY_ROW_CAP;

/// The id of a library entry (`diagram_entries.id`).
pub type EntryId = i64;

/// One entry as the library list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryCard {
    /// The entry's id.
    pub id: EntryId,
    /// The design's title.
    pub title: String,
    /// The designer (the first part of the free-text designer line); empty when unknown.
    pub designer: String,
    /// The shape name ("Round"); empty when unknown.
    pub shape: String,
    /// The index gear's tooth count; `0` when unknown.
    pub gear: u32,
    /// The facet count (main plus girdle facets); `0` when unknown.
    pub facets: u32,
    /// The refractive index the design was drawn for.
    pub ri: Option<f64>,
    /// The entry's tags, alphabetical.
    pub tags: Vec<String>,
    /// Whether the design is the user's own (imported or saved here) rather than synced
    /// from a remote catalogue.
    pub mine: bool,
    /// Whether the entry is marked ignored (hidden unless the query shows ignored).
    pub ignored: bool,
    /// When the entry last changed, in Unix seconds; `0` when unknown.
    pub updated_unix: i64,
}

/// What the library list asks for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LibraryQuery {
    /// Free text, matched case-insensitively against title, designer, design id and notes.
    pub text: String,
    /// Only this shape (exact name).
    pub shape: Option<String>,
    /// Only this index gear.
    pub gear: Option<u32>,
    /// Lowest refractive index.
    pub ri_min: Option<f64>,
    /// Highest refractive index.
    pub ri_max: Option<f64>,
    /// Only the user's own designs.
    pub mine_only: bool,
    /// Only designs with this tag (case-insensitive).
    pub tag: Option<String>,
    /// Also list the entries marked ignored.
    pub show_ignored: bool,
    /// The order of the list.
    pub sort: LibrarySort,
}

/// The values the filter sheet offers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterOptions {
    /// Shapes the library has designs of, alphabetical.
    pub shapes: Vec<String>,
    /// Index gears the library has designs of, ascending.
    pub gears: Vec<u32>,
    /// Every tag, alphabetical.
    pub tags: Vec<String>,
}

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Designs stored (new entries, and entries a changed file replaced).
    pub added: usize,
    /// Files skipped because the library already holds the same file.
    pub duplicates: usize,
    /// Files that were not imported, each as `(file name, sentence saying why)`.
    pub failed: Vec<(String, String)>,
}

/// The file a library entry opens from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryDesign {
    /// The stored file's name (`Barion.isdd`, `Barion.asc`).
    pub file_name: String,
    /// The stored file's bytes; what `document::open_picked` classifies and loads.
    pub bytes: Vec<u8>,
}

/// A straight-alpha RGBA8 picture of a design.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, row-major.
    pub rgba: Vec<u8>,
}

/// The open library and the counter that says when it changed.
#[derive(Debug, Default)]
pub struct LibraryState {
    handle: Option<LibraryHandle>,
    /// Bumped by every write through any handle.
    revision: Arc<AtomicU64>,
}

impl LibraryState {
    /// The library's handle, opening the database under `<files>/library/` on first use
    /// (creating the folder and the schema). Opening takes tens of milliseconds on a
    /// fresh database; a front end that must not block the UI thread uses
    /// [`Self::opener`] on a worker and [`Self::adopt`] on the UI thread instead.
    ///
    /// # Errors
    /// A sentence when the database cannot be created or opened.
    pub fn handle(&mut self, dirs: &AppDirs) -> Result<LibraryHandle, String> {
        if let Some(handle) = &self.handle {
            return Ok(handle.clone());
        }
        let handle = self.opener(dirs).open()?;
        self.handle = Some(handle.clone());
        Ok(handle)
    }

    /// A `Send` token that opens the database on another thread; hand its result to
    /// [`Self::adopt`].
    #[must_use]
    pub fn opener(&self, dirs: &AppDirs) -> LibraryOpener {
        LibraryOpener::new(dirs.clone(), Arc::clone(&self.revision))
    }

    /// Keeps `handle` (made by [`Self::opener`]) as the library's handle. A handle that
    /// is already open stays.
    pub fn adopt(&mut self, handle: LibraryHandle) {
        if self.handle.is_none() {
            self.handle = Some(handle);
        }
    }

    /// The handle if the database was opened already.
    #[must_use]
    pub fn current(&self) -> Option<LibraryHandle> {
        self.handle.clone()
    }

    /// A counter that grows with every write through any handle: a cheap "did the list
    /// change" check for a front end's `push`.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// Lets go of the database; it closes once the last clone of its handle is dropped.
    /// Called by `AppState::shutdown`.
    pub fn close(&mut self) {
        self.handle = None;
    }
}
