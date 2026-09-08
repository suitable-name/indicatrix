//! Local-library management: import `.asc` files, rename, delete, and export the
//! user's own designs -- "Organize" plus "Import"/"Export" in the
//! UI (see `apps/indicatrix-cut/ui/components/{import_dialog,detail_header}.slint`).
//!
//! Nothing here talks to the network or parses HTML/PDF; it operates only on files
//! the user handed the app directly (`import_path`) or on rows already in the local
//! SQLite catalogue. See `indicatrix_vault::local`'s doc comment for the underlying
//! parse/reconstruct logic this module wires up to the UI.
//!
//! Split into [`import`], [`organize`] (ignore/rename/shape/delete), and [`export`],
//! sharing [`helpers`]'s `refresh_after_library_change` -- each used to be one
//! function group inside a single flat `library.rs`; the split follows that exact
//! seam, with the once-implicit shared "refresh after any write" step now named and
//! pulled out into `helpers` instead of duplicated three times.

mod export;
mod helpers;
mod import;
mod organize;

pub use export::setup_export_asc_callback;
pub use import::setup_import_callback;
pub use organize::{
    build_shape_picker_options, setup_delete_callback, setup_ignore_toggle_callback,
    setup_rename_callback, setup_set_shape_callback,
};
