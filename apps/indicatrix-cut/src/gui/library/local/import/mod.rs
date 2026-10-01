//! Importing `.asc`, `.gem` and `.gcs` files into the local database, including
//! optional recursive subfolder import, off the UI thread and panic-isolated per
//! file -- the "Import" side of `gui::library::local` (see this group's own `mod.rs`).
//!
//! Nothing here talks to the network or parses HTML/PDF; it operates only on files
//! the user handed the app directly (`import_path`) or on rows already in the local
//! SQLite catalogue. See `indicatrix_vault::local`'s doc comment for the underlying
//! parse/reconstruct logic this module wires up to the UI.
//!
//! Split into [`scan`] (finding candidate files), [`foreign`] (converting a `.gem`/
//! `.gcs` design to `.asc` cutting instructions), [`measure`] (deriving proportions/
//! shape and merging a re-import's metadata), [`pipeline`] (the per-file parse-
//! measure-save steps and the batch loop), [`confirm`] (the filename-collision
//! pre-scan and confirmation dialog) and [`folder_memory`] (remembering where to
//! import from next), all wired together by [`wiring`].

mod confirm;
mod folder_memory;
mod foreign;
mod measure;
mod pipeline;
mod scan;
#[cfg(test)]
mod tests;
mod wiring;

#[cfg(test)]
pub use foreign::test_gem;
pub use foreign::{ForeignFormat, convert_foreign_design, converted_asc_file_name};
pub use measure::{apply_measured_metadata, merge_reimport_metadata};
pub use pipeline::catch_file_panic;
pub use wiring::setup_import_callback;
