//! Reads/parses whichever file the "Open" picker (or File > Open Recent, or a path
//! given on the command line) names -- a self-contained `.isdd` design file (or a
//! legacy `.indicatrix` one, read exactly the same), a directly-picked bare `.asc`, or
//! a `.gem`/`.gcs` design converted to `.asc` cutting instructions -- translating the
//! result into a [`PickedNative`] for [`super::open_native::do_open_native`] to
//! dispatch. A design file is routed by its content
//! ([`indicatrix_formats::native::design::detect_kind`]), never by its name alone.
//!
//! The file reads and parses run on a worker thread ([`read_picked_path_then`]): a design
//! file with embedded attachments is read, decoded and parsed in one go, which was a
//! UI-thread stall that grew with the attachment budget. Everything that needs the window
//! (toasts) runs back on the UI thread.

use super::picker::{PickKind, pick_file};
use crate::{
    MainWindow,
    gui::{
        library::local::{ForeignFormat, convert_foreign_design, converted_asc_file_name},
        show_toast,
    },
};
use indicatrix_cut_core::native::{DesignLoadError, FileKind, LoadedDesign, design_from_str};
use indicatrix_formats::native::design::{DesignFileError, detect_kind};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
};

/// What the "Open" picker actually returned -- see [`setup_open_native_callback`]'s
/// own doc comment for why the same picker also accepts a bare `.asc` (a
/// cutter handed a plain `GemCAD` file by email should not have to import it into the
/// catalogue database first just to look at it).
pub(super) enum PickedNative {
    /// A self-contained `.isdd` (or legacy `.indicatrix`) design file, already parsed
    /// and turned into a design. Boxed: a whole parsed design is several times larger
    /// than [`Self::AscOnly`]'s bare path+text (`clippy::large_enum_variant`).
    Design {
        native_path: PathBuf,
        loaded: Box<LoadedDesign>,
    },
    /// A directly-picked bare `.asc`, committed straight through
    /// `gui::editor::loading::design_from_asc_text` in [`open_plain_asc`].
    AscOnly { asc_path: PathBuf, asc_text: String },
    /// A directly-picked `.gem` (`GemCAD`) or `.gcs` (Gem Cut Studio) file,
    /// already converted to `.asc` cutting instructions by
    /// `gui::library::local::convert_foreign_design` -- the same conversion Import
    /// uses. Committed through the bare-`.asc` path in
    /// `super::open_commit::open_converted_design`.
    Converted(ConvertedPick),
}

/// [`PickedNative::Converted`]'s payload: the picked file and the `.asc` text its
/// design converted to.
pub(super) struct ConvertedPick {
    /// The `.gem`/`.gcs` file the cutter picked; named in the toast and the window
    /// title, and never written to.
    pub(super) source_path: PathBuf,
    /// The `.asc` name the design is recorded under (`<stem>.asc`), so Save offers
    /// a new `.isdd` design file rather than the source file.
    pub(super) asc_file_name: String,
    /// The converted cutting instructions as `.asc` text.
    pub(super) asc_text: String,
    /// The reader's and converter's warnings, shown in the open toast.
    pub(super) warnings: Vec<String>,
}

/// Reads and converts a picked `.gem`/`.gcs` file (`path`'s extension picks the
/// reader).
///
/// # Errors
///
/// A toast-ready message when the file cannot be read, is neither a `.gem` nor a
/// `.gcs`, or does not parse.
pub(super) fn read_foreign_design(path: &Path) -> Result<ConvertedPick, String> {
    let format = ForeignFormat::from_path(path)
        .ok_or_else(|| format!("'{}' is not a .gem or .gcs file.", path.display()))?;
    let bytes =
        std::fs::read(path).map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    let converted =
        crate::gui::library::local::catch_file_panic(std::panic::AssertUnwindSafe(|| {
            convert_foreign_design(format, &bytes)
        }))
        .map_err(|panic_msg| {
            format!(
                "Cannot open {}: internal error: {panic_msg}",
                path.display()
            )
        })?
        .map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    let file_name = path.file_name().map_or_else(
        || format!("design{}", format.dotted_extension()),
        |n| n.to_string_lossy().into_owned(),
    );
    Ok(ConvertedPick {
        source_path: path.to_path_buf(),
        asc_file_name: converted_asc_file_name(&file_name),
        asc_text: converted.asc_text,
        warnings: converted.warnings,
    })
}

/// What the worker thread hands back to the UI thread for one picked path.
enum BackgroundRead {
    Picked(PickedNative),
    /// A toast-ready failure message.
    Failed(String),
}

/// The continuation of one [`read_picked_path_then`] call, parked on the UI thread while
/// its worker runs (an `FnOnce` closure over UI state is not `Send`, so only the key
/// crosses the thread boundary -- the same trick `gui::pickers::pick` uses).
type OpenDone = Box<dyn FnOnce(&MainWindow, Option<PickedNative>)>;

thread_local! {
    static PENDING_OPEN_READS: RefCell<HashMap<u64, OpenDone>> = RefCell::new(HashMap::new());
    static NEXT_OPEN_READ_KEY: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Forgets the open-file continuations a previous main window left parked. See
/// `gui::window_release`.
pub(super) fn release_window_state() {
    let pending = PENDING_OPEN_READS.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
    drop(pending);
}

/// Reads and parses the design file at `design_path` -- no window access, so it runs on
/// a worker thread. A failure is returned as the message the UI thread toasts. The file
/// is routed by content: anything that is not a design file (an older overlay sidecar
/// included, which is no longer supported) is refused with a plain message.
fn read_design_blocking(design_path: &Path) -> BackgroundRead {
    let text = match std::fs::read_to_string(design_path) {
        Ok(text) => text,
        Err(e) => {
            return BackgroundRead::Failed(format!(
                "Failed to read {}: {e}",
                design_path.display()
            ));
        }
    };
    if detect_kind(text.as_bytes()) != FileKind::Design {
        return BackgroundRead::Failed(format!(
            "Cannot open '{}': it is not an Indicatrix design file.",
            design_path.display()
        ));
    }
    match design_from_str(&text) {
        Ok(loaded) => BackgroundRead::Picked(PickedNative::Design {
            native_path: design_path.to_path_buf(),
            loaded: Box::new(loaded),
        }),
        Err(e) => BackgroundRead::Failed(design_open_error_message(design_path, &e)),
    }
}

/// The toast text for a design file that did not open. A file written by a newer
/// version says so plainly, with the versions involved; anything else names the file and
/// what is wrong with it.
pub(super) fn design_open_error_message(path: &Path, error: &DesignLoadError) -> String {
    match error {
        DesignLoadError::File(DesignFileError::UnsupportedVersion { found, supported }) => {
            format!(
                "Cannot open '{}': it was saved by a newer version of Indicatrix (file \
                 version {found}; this version reads up to {supported}). Update Indicatrix \
                 to open it.",
                path.display()
            )
        }
        other => format!("Cannot open '{}': {other}", path.display()),
    }
}

/// Reads an `.asc` file's text through
/// [`indicatrix_formats::asc::decode_asc_bytes`] rather than `read_to_string`:
/// `GemCAD` for Windows writes Windows-1252 (a legacy `°` is byte `0xB0`), which
/// `read_to_string` rejects outright, and a UTF-8 byte-order mark or CR-only line
/// endings are normalised on the way in.
fn read_asc_text(path: &Path) -> std::io::Result<String> {
    std::fs::read(path).map(|bytes| indicatrix_formats::asc::decode_asc_bytes(&bytes).into_owned())
}

/// Reads the design at `picked_path`, whichever kind it is: a `.asc`, a `.gem`/`.gcs`,
/// or a design file (`.isdd`, or a legacy `.indicatrix`). No window access, so it runs
/// on a worker thread.
///
/// A directly-picked `.asc` opens as [`PickedNative::AscOnly`]: authored meet
/// constraints and detached facets live in the design file, which is opened directly.
fn read_path_blocking(picked_path: PathBuf) -> BackgroundRead {
    if picked_path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("asc"))
    {
        return match read_asc_text(&picked_path) {
            Ok(asc_text) => BackgroundRead::Picked(PickedNative::AscOnly {
                asc_path: picked_path,
                asc_text,
            }),
            Err(e) => {
                BackgroundRead::Failed(format!("Failed to read {}: {e}", picked_path.display()))
            }
        };
    }
    if ForeignFormat::from_path(&picked_path).is_some() {
        return match read_foreign_design(&picked_path) {
            Ok(converted) => BackgroundRead::Picked(PickedNative::Converted(converted)),
            Err(message) => BackgroundRead::Failed(message),
        };
    }
    read_design_blocking(&picked_path)
}

/// Shows the "Open" picker (accepting `.isdd`, `.indicatrix`, `.asc`, `.gem` or
/// `.gcs`) and reads whichever one the cutter picked. `on_done`'s `None` is a
/// cancellation or any read/parse failure, each already toasted before calling it.
///
/// The picker itself ([`PickKind::OpenDesign`]) runs on a background
/// thread via [`pick_file`] -- see that function's own `match` for the filter
/// shape it builds.
pub(super) fn pick_design_then(
    ui: &MainWindow,
    on_done: impl FnOnce(&MainWindow, Option<PickedNative>) + 'static,
) {
    pick_file(ui, PickKind::OpenDesign, move |ui, picked_path| {
        let Some(picked_path) = picked_path else {
            // A dismissed file dialog is the cutter's own deliberate cancel --
            // no toast needed.
            on_done(ui, None);
            return;
        };
        read_picked_path_then(ui, picked_path, on_done);
    });
}

/// Reads the design at `picked_path` on a worker thread ([`read_path_blocking`]) and calls
/// `on_done` on the UI thread with the result. The shared body of the Open picker, File >
/// Open Recent and the command-line open; `on_done`'s `None` is any read/parse failure,
/// already toasted. `on_done` silently never runs if the window closed meanwhile, the same
/// convention every worker in this crate follows.
pub(super) fn read_picked_path_then(
    ui: &MainWindow,
    picked_path: PathBuf,
    on_done: impl FnOnce(&MainWindow, Option<PickedNative>) + 'static,
) {
    let key = NEXT_OPEN_READ_KEY.with(|c| {
        let key = c.get();
        c.set(key + 1);
        key
    });
    PENDING_OPEN_READS.with(|cell| {
        cell.borrow_mut().insert(key, Box::new(on_done));
    });
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || {
        let read = read_path_blocking(picked_path);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            let Some(on_done) = PENDING_OPEN_READS.with(|cell| cell.borrow_mut().remove(&key))
            else {
                return;
            };
            match read {
                BackgroundRead::Picked(picked) => on_done(&ui, Some(picked)),
                BackgroundRead::Failed(message) => {
                    show_toast(&ui, &message, "error");
                    on_done(&ui, None);
                }
            }
        });
    });
}
