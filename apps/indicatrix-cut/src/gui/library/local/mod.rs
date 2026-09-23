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
//! sharing [`helpers`]'s `refresh_after_library_change` as the common "refresh after
//! any write" step.

mod export;
mod helpers;
mod import;
mod organize;

pub use export::{sanitize_filename, setup_export_asc_callback};
pub use import::setup_import_callback;
pub use organize::{
    build_shape_picker_options, setup_add_tag_callback, setup_delete_callback,
    setup_ignore_toggle_callback, setup_remove_tag_callback, setup_rename_callback,
    setup_set_shape_callback,
};

/// Re-exported for `gui::editor::native_io`'s own catalogue write-back: "Save Native"
/// merges into an existing source row, or measures a brand-new one, using exactly the
/// same rules a `.asc` re-import already applies -- see each function's own doc
/// comment. `helpers`/`import` stay private modules; only these specific helpers
/// cross the `gui::editor`/`gui::library` boundary, rather than opening either module
/// up wholesale.
pub use helpers::refresh_after_library_change;
pub use import::{apply_measured_metadata, merge_reimport_metadata};
