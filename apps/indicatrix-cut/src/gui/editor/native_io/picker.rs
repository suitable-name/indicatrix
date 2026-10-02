//! Group 3: file pickers off the UI thread. [`PickKind`]/[`pick_file`] are a thin
//! translation over `gui::pickers::{PickerKind, PickerRequest, pick}` -- every
//! save/open dialog this module's own write/export/open paths need builds a
//! [`PickKind`] value and threads a continuation through [`pick_file`].

use super::design_paths::design_file_name_for;
use crate::{MainWindow, gui::editor::state::EditorState};
use indicatrix_formats::native::design::{
    DESIGN_EXTENSION, DESIGN_EXTENSION_DOTTED, DESIGN_FILE_DESCRIPTION,
};
use std::path::{Path, PathBuf};

// --- Group 3: file pickers off the UI thread --------------------------------
//
// The picker machinery itself (a `PickKind`-flavoured native file-dialog
// builder, the UI-thread rendezvous, and the `#[cfg(test)]` answer-injection
// hook) lives in `gui::pickers` -- the one place the `rfd` crate's file dialog
// is ever constructed anywhere in this app, so every OTHER module's own
// picker uses the SAME worker instead of each spawning its own thread and
// rendezvous. `PickKind`/`pick_file` here are a thin translation over
// `gui::pickers::{PickerKind, PickerRequest, pick}` -- every call site below
// builds a `PickKind` value and threads a continuation through it. See
// `gui::pickers`'s own module doc comment for why a request enum/struct, not a
// caller-supplied native-file-dialog builder closure.

/// Which native picker [`pick_file`] should show -- one entry per save/open
/// dialog this module's own write/export/open paths need, translated to a
/// [`crate::gui::pickers::PickerRequest`] by [`pick_file`] itself.
pub(super) enum PickKind {
    /// "Save" / "Save As...": the `.indicatrix` design-file save-as picker.
    SaveDesign { default_name: String },
    /// "Export .asc": the `.asc` save-as picker.
    SaveAsc { default_name: String },
    /// "Export as Gem Cut Studio (.gcs)...": the `.gcs` save-as picker.
    SaveGcs { default_name: String },
    /// "Export Cutting Sheet": the `.html` save-as picker.
    SaveCuttingSheet { default_name: String },
    /// "Export Diagram": the `.png` save-as picker.
    SaveDiagram { default_name: String },
    /// "Open": accepts a `.indicatrix` design file, an older `.indicatrix.toml` sidecar, a
    /// bare `.asc`, or a `.gem`/`.gcs` design (converted to `.asc` on open).
    OpenDesign,
    /// [`resolve_paired_asc_text_then`]'s "Locate the paired .asc" recovery picker.
    LocateAsc,
}

/// The `.asc design` filter every save/open dialog below that touches a
/// `.asc` file shares -- a `fn`, not a `const`, since [`PickerFilter`] now
/// owns its data (`String`/`Vec<String>`, see that type's own doc comment for
/// why), which cannot be built in a `const` context.
fn asc_filter() -> crate::gui::pickers::PickerFilter {
    crate::gui::pickers::PickerFilter {
        label: ".asc design".to_string(),
        extensions: vec!["asc".to_string()],
    }
}

/// Translates `kind` into a [`crate::gui::pickers::PickerRequest`] and shows
/// it via [`crate::gui::pickers::pick`] -- the one entry point every native
/// file picker in this module uses. `state` must never be borrowed across a
/// call to this function -- every call site below picks first, then borrows,
/// never the other way around.
pub(super) fn pick_file(
    ui: &MainWindow,
    kind: PickKind,
    on_done: impl FnOnce(&MainWindow, Option<PathBuf>) + 'static,
) {
    use crate::gui::pickers::{PickerFilter, PickerKind, PickerRequest, pick};

    let request = match kind {
        PickKind::SaveDesign { default_name } => PickerRequest {
            kind: PickerKind::SaveFile,
            title: None,
            filters: vec![PickerFilter {
                label: format!("{DESIGN_FILE_DESCRIPTION} (*{DESIGN_EXTENSION_DOTTED})"),
                extensions: vec![DESIGN_EXTENSION.to_string()],
            }],
            default_file_name: Some(default_name),
            // Where the design was last opened from (an older paired file is saved
            // anew beside it), else the `./exports` convention.
            starting_dir: super::SUGGESTED_SAVE_DIR
                .with(|cell| cell.borrow().clone())
                .or_else(default_export_dir),
        },
        PickKind::SaveAsc { default_name } => PickerRequest {
            kind: PickerKind::SaveFile,
            title: None,
            filters: vec![asc_filter()],
            default_file_name: Some(default_name),
            // Seeds `./exports` as the starting directory when it exists, the same
            // convention `gui::library::local::export::setup_export_asc_callback`/
            // `gui::library::detail::export_diagram_file_via_source` already
            // use -- a no-op (leaves the OS's own last-used-directory memory
            // in place) when that folder doesn't exist yet.
            starting_dir: default_export_dir(),
        },
        PickKind::SaveGcs { default_name } => PickerRequest {
            kind: PickerKind::SaveFile,
            title: None,
            filters: vec![PickerFilter {
                label: "Gem Cut Studio design (.gcs)".to_string(),
                extensions: vec!["gcs".to_string()],
            }],
            default_file_name: Some(default_name),
            starting_dir: default_export_dir(),
        },
        PickKind::SaveCuttingSheet { default_name } => PickerRequest {
            kind: PickerKind::SaveFile,
            title: None,
            filters: vec![PickerFilter {
                label: "Cutting sheet (HTML)".to_string(),
                extensions: vec!["html".to_string()],
            }],
            default_file_name: Some(default_name),
            starting_dir: None,
        },
        PickKind::SaveDiagram { default_name } => PickerRequest {
            kind: PickerKind::SaveFile,
            title: None,
            filters: vec![PickerFilter {
                label: "Diagram (PNG)".to_string(),
                extensions: vec!["png".to_string()],
            }],
            default_file_name: Some(default_name),
            starting_dir: None,
        },
        // The design file filter leads; the older sidecar filter must use the FULL
        // compound suffix (not a bare `"toml"`) or `rfd` would offer to select
        // `Cargo.toml`.
        PickKind::OpenDesign => PickerRequest {
            kind: PickerKind::OpenFile,
            title: Some("Open Design".to_string()),
            filters: vec![
                PickerFilter {
                    label: format!("{DESIGN_FILE_DESCRIPTION} (*{DESIGN_EXTENSION_DOTTED})"),
                    extensions: vec![DESIGN_EXTENSION.to_string()],
                },
                PickerFilter {
                    label: "Older Indicatrix sidecar (*.indicatrix.toml, *.gemcut.toml)"
                        .to_string(),
                    extensions: vec![
                        indicatrix_cut_core::native::NATIVE_EXTENSION_SUFFIX.to_string(),
                        indicatrix_cut_core::native::LEGACY_NATIVE_EXTENSION_SUFFIX.to_string(),
                    ],
                },
                PickerFilter {
                    label: "All TOML files".to_string(),
                    extensions: vec!["toml".to_string()],
                },
                asc_filter(),
                PickerFilter {
                    label: "GemCAD / Gem Cut Studio design (.gem, .gcs)".to_string(),
                    extensions: vec!["gem".to_string(), "gcs".to_string()],
                },
            ],
            default_file_name: None,
            starting_dir: None,
        },
        PickKind::LocateAsc => PickerRequest {
            kind: PickerKind::OpenFile,
            title: Some("Locate the paired .asc".to_string()),
            filters: vec![asc_filter()],
            default_file_name: None,
            starting_dir: None,
        },
    };
    pick(ui, request, on_done);
}

/// The file name an Export/Save dialog should default to. Prefers
/// `asc_filename` (this design's own recorded/last-saved name -- see
/// [`crate::gui::editor::state::EditorState::asc_filename`]'s own doc comment) when set, else a
/// sanitized version of the schedule's own first free-text header line (the `GemCad`
/// convention for a design's title/description), else `"edited_design.asc"` for a
/// design with neither (a brand-new "New Design" with no header typed yet). Shared by
/// [`setup_export_asc_callback`] and [`setup_save_native_callback`] so the two
/// dialogs stay in sync instead of each proposing its own default name.
pub(super) fn suggested_file_name(st: &EditorState) -> String {
    if let Some(name) = &st.asc_filename {
        return name.clone();
    }
    match st.design.meta.headers.first() {
        Some(header) if !header.trim().is_empty() => {
            format!(
                "{}.asc",
                crate::gui::library::local::sanitize_filename(header)
            )
        }
        _ => "edited_design.asc".to_string(),
    }
}

/// The file name a Save/Save As dialog should default to: the design's own name with
/// the `.indicatrix` extension (`foo.asc` -> `foo.indicatrix`). Derived from
/// [`suggested_file_name`], so the two dialogs stay in step apart from the extension.
pub(super) fn suggested_design_file_name(st: &EditorState) -> String {
    design_file_name_for(&suggested_file_name(st))
}

/// `./exports` as a [`PickKind::SaveAsc`] picker's starting directory, when
/// that folder exists -- the same convention
/// `gui::library::local::export::setup_export_asc_callback`/
/// `gui::library::detail::export_diagram_file_via_source` use (and the one
/// `docs/manual/11-saving-and-file-formats.md` promises). `None` when the
/// folder doesn't exist, so the OS's own last-used-directory memory still
/// applies for a cutter who has never created one -- see
/// [`crate::gui::pickers::PickerRequest::starting_dir`]'s own doc comment
/// for what a `None` there means.
fn default_export_dir() -> Option<PathBuf> {
    let default_dir = Path::new("exports");
    default_dir.is_dir().then(|| default_dir.to_path_buf())
}
