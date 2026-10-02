//! Reads/parses whichever file the "Open" picker (or File > Open Recent, or a path
//! given on the command line) names -- a self-contained `.indicatrix` design file, an
//! older sidecar+`.asc` pair, an older self-contained sidecar with no paired `.asc`
//! involved, a directly-picked bare `.asc`, or a `.gem`/`.gcs` design converted to
//! `.asc` cutting instructions -- translating the result into a [`PickedNative`] for
//! [`super::open_native::do_open_native`] to dispatch. A design file is routed by its
//! content ([`indicatrix_formats::native::design::detect_kind`]), never by its name
//! alone.

use super::picker::{PickKind, pick_file};
use crate::{
    MainWindow,
    gui::{
        library::local::{ForeignFormat, convert_foreign_design, converted_asc_file_name},
        show_toast,
    },
};
use indicatrix_cut_core::{
    NativeDesignFile,
    native::{
        DesignLoadError, FileKind, LoadNativeOnlyResult, LoadedDesign, design_from_str,
        load_native_only,
    },
    native_path_for_asc,
};
use indicatrix_formats::native::design::{DesignFileError, detect_kind};
use std::path::{Path, PathBuf};

/// What the "Open" picker actually returned -- see [`setup_open_native_callback`]'s
/// own doc comment for why the same picker also accepts a bare `.asc` (a
/// cutter handed a plain `GemCAD` file by email should not have to import it into the
/// catalogue database first just to look at it).
pub(super) enum PickedNative {
    /// A real older sidecar+`.asc` pair, ready for [`load_paired`]. Boxed: [`PickedPair`]
    /// carries a whole parsed [`NativeDesignFile`] plus both source texts, several
    /// times larger than [`Self::AscOnly`]'s bare path+text
    /// (`clippy::large_enum_variant`).
    Pair(Box<PickedPair>),
    /// A self-contained `.indicatrix` design file, already parsed and turned into a
    /// design. Boxed for the same `clippy::large_enum_variant` reason as
    /// [`Self::Pair`].
    Design {
        native_path: PathBuf,
        loaded: Box<LoadedDesign>,
    },
    /// A directly-picked bare `.asc` with no design file found next to it --
    /// nothing [`load_paired`] has any use for (no per-tier overlay, no fingerprint,
    /// no draft flag); committed straight through
    /// `gui::editor::loading::design_from_asc_text` instead, in [`open_plain_asc`].
    AscOnly { asc_path: PathBuf, asc_text: String },
    /// An older sidecar with NO paired `.asc` findable anywhere (recorded name,
    /// naming-guess, and -- unlike the other two variants -- no prompt for one
    /// either), opened via [`load_native_only`] instead: the autosave-restore case
    /// [`save_native_only`](indicatrix_cut_core::native::save_native_only)'s own
    /// doc comment describes. [`read_native_pair_then`] only ever produces this
    /// when the file actually parses as self-contained -- an ordinary sidecar
    /// with a genuinely missing `.asc` still falls through to
    /// [`resolve_paired_asc_text_then`]'s "Locate the paired .asc" prompt, since
    /// only a [`save_native_only`](indicatrix_cut_core::native::save_native_only)
    /// file carries enough to skip it. Boxed for the same
    /// `clippy::large_enum_variant` reason as [`Self::Pair`].
    SelfContained {
        native_path: PathBuf,
        loaded: Box<LoadNativeOnlyResult>,
        asc_filename: String,
    },
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
    /// a new `.indicatrix` design file rather than the source file.
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

/// [`PickedNative::Pair`]'s payload -- bundled into its own struct (rather than four
/// more parameters on [`open_native_pair`]) purely to keep that function under
/// clippy's argument-count lint, the same reasoning [`LoadedNativeOutcome`] uses.
pub(super) struct PickedPair {
    pub(super) native_path: PathBuf,
    pub(super) parsed_native: Box<NativeDesignFile>,
    pub(super) asc_text: String,
    pub(super) native_text: String,
}

/// [`read_native_pair_then`]'s outcome -- a real pair (the ordinary case), or a
/// self-contained older sidecar with no paired `.asc` involved at all. Kept
/// distinct from [`PickedNative`] itself (rather than reusing it directly)
/// since neither of `read_native_pair_then`'s
/// two callers has a `native_path` to attach until this returns.
pub(super) enum ReadOutcome {
    /// A self-contained `.indicatrix` design file, already parsed.
    Design(Box<LoadedDesign>),
    Pair {
        // Boxed: `clippy::large_enum_variant` against `SelfContained`'s own,
        // much smaller payload.
        parsed_native: Box<NativeDesignFile>,
        asc_text: String,
        native_text: String,
    },
    SelfContained {
        loaded: Box<LoadNativeOnlyResult>,
        asc_filename: String,
    },
}

/// Reads/parses a sidecar already known to live at `native_path`, plus its
/// recorded paired `.asc` -- split out of [`pick_design_then`]/
/// [`read_picked_asc_then`] purely to keep both under clippy's line-count lint.
/// `on_done`'s `None` is any read/parse failure, each already toasted here before
/// calling it.
///
/// [`resolve_paired_asc_text_then`]'s own "Locate the paired .asc" recovery picker
/// runs off the UI thread, so this (and every caller up the chain) is
/// continuation-passing too. The file-read/TOML-parse calls
/// themselves stay synchronous -- reading one small file is cheap enough not to
/// need the same treatment.
///
/// Before ever prompting for the paired `.asc`, tries [`load_native_only`] on
/// `native_text` iff neither the recorded nor the guessed `.asc` path exists on
/// disk -- see [`ReadOutcome::SelfContained`]'s own doc comment.
/// Almost every older sidecar fails that check immediately ([`load_native_only`]
/// itself refuses anything that isn't a
/// [`save_native_only`](indicatrix_cut_core::native::save_native_only) file), so
/// an ordinary design with a genuinely missing `.asc` still reaches
/// [`resolve_paired_asc_text_then`]'s prompt.
pub(super) fn read_native_pair_then(
    ui: &MainWindow,
    native_path: &Path,
    on_done: impl FnOnce(&MainWindow, Option<ReadOutcome>) + 'static,
) {
    let native_text = match std::fs::read_to_string(native_path) {
        Ok(text) => text,
        Err(e) => {
            show_toast(
                ui,
                &format!("Failed to read {}: {e}", native_path.display()),
                "error",
            );
            on_done(ui, None);
            return;
        }
    };
    // Routed by content: a `.indicatrix` file holding an older overlay sidecar still
    // takes the paired path below, and a design file under any name opens as one.
    if detect_kind(native_text.as_bytes()) == FileKind::Design {
        match design_from_str(&native_text) {
            Ok(loaded) => on_done(ui, Some(ReadOutcome::Design(Box::new(loaded)))),
            Err(e) => {
                show_toast(ui, &design_open_error_message(native_path, &e), "error");
                on_done(ui, None);
            }
        }
        return;
    }
    // The sidecar's own recorded `asc_filename` (a bare file name -- see
    // `indicatrix_cut_core::NativeDesignFile::asc_filename`'s own doc comment) is read
    // AFTER parsing the chosen file, since that field is the authoritative pointer to
    // the real paired file once a sidecar has actually been parsed; this never
    // guesses at a sibling `.asc` name the way `indicatrix_cut_core::asc_path_for_native`
    // does for a picker's initial directory (there is no picker here to seed -- the
    // sidecar's own directory plus its own recorded name is exact).
    let parsed_native = match indicatrix_cut_core::native::parse_toml_string(&native_text) {
        Ok(n) => Box::new(n),
        Err(e) => {
            show_toast(
                ui,
                &format!(
                    "'{}' is not a valid .indicatrix.toml sidecar: {e}",
                    native_path.display()
                ),
                "error",
            );
            on_done(ui, None);
            return;
        }
    };
    let asc_path = native_path.with_file_name(&parsed_native.asc_filename);
    // Cloned so `resolve_paired_asc_text_then`'s borrow of it can end before this
    // function's own `move` closure below takes ownership of `parsed_native` (whose
    // own field it would otherwise still be borrowing).
    let asc_filename = parsed_native.asc_filename.clone();

    let asc_findable = asc_path.is_file()
        || indicatrix_cut_core::asc_path_for_native(native_path)
            .is_some_and(|guessed| guessed != asc_path && guessed.is_file());
    // Not self-contained (an ordinary sidecar whose `.asc` genuinely went
    // missing) falls through to the same prompt this always showed.
    if !asc_findable && let Ok(loaded) = load_native_only(&native_text) {
        on_done(
            ui,
            Some(ReadOutcome::SelfContained {
                loaded: Box::new(loaded),
                asc_filename,
            }),
        );
        return;
    }

    resolve_paired_asc_text_then(
        ui,
        native_path,
        &asc_path,
        &asc_filename,
        move |ui, asc_text| {
            on_done(
                ui,
                asc_text.map(|asc_text| ReadOutcome::Pair {
                    parsed_native,
                    asc_text,
                    native_text,
                }),
            );
        },
    );
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

/// Reads the paired `.asc`'s text, recovering from a moved/renamed file (common
/// after `GemCad`'s own Save As) instead of giving up outright. Tries, in
/// order: `recorded_asc_path` (the sidecar's own authoritative
/// `asc_filename`, exact when it still holds); [`indicatrix_cut_core::asc_path_for_native`]'s
/// naming guess (only meaningful when it names a DIFFERENT path -- most sidecars'
/// own recorded name already matches the guess, so this rarely fires on its own); and
/// finally an explicit "Locate the paired .asc" picker (Group 3: [`pick_file`], on a
/// background thread), filtered to `*.asc`, so a cutter who renamed or moved the file
/// can point at it directly rather than hand-editing the sidecar's TOML. `on_done`'s
/// `None` (already toasted) only once all three have failed or the picker was
/// cancelled.
fn resolve_paired_asc_text_then(
    ui: &MainWindow,
    native_path: &Path,
    recorded_asc_path: &Path,
    recorded_asc_filename: &str,
    on_done: impl FnOnce(&MainWindow, Option<String>) + 'static,
) {
    if let Ok(text) = read_asc_text(recorded_asc_path) {
        on_done(ui, Some(text));
        return;
    }
    if let Some(guessed) = indicatrix_cut_core::asc_path_for_native(native_path)
        && guessed != recorded_asc_path
        && let Ok(text) = read_asc_text(&guessed)
    {
        on_done(ui, Some(text));
        return;
    }
    show_toast(
        ui,
        &format!(
            "'{}' names a paired .asc file '{recorded_asc_filename}', but it could not be found \
             next to it. Locate it to continue.",
            native_path.display()
        ),
        "info",
    );
    pick_file(ui, PickKind::LocateAsc, move |ui, located| {
        let Some(located) = located else {
            // The cutter's own explicit cancel -- no toast.
            on_done(ui, None);
            return;
        };
        match read_asc_text(&located) {
            Ok(text) => on_done(ui, Some(text)),
            Err(e) => {
                show_toast(
                    ui,
                    &format!("Failed to read {}: {e}", located.display()),
                    "error",
                );
                on_done(ui, None);
            }
        }
    });
}

/// A directly-picked `.asc` file: checks for a sibling design file or older sidecar first (via
/// `native_path_for_asc`), so a pair still opens as a full pair even when picked by
/// its `.asc` half -- preserving the authored meet constraints/detached facets a bare
/// `.asc` re-import would otherwise drop entirely. Falls back to
/// [`PickedNative::AscOnly`] only when no sidecar file exists at all; a sidecar that
/// exists but fails to read/parse is a real error (already toasted by
/// [`read_native_pair_then`]), not silently skipped.
/// Wraps [`read_native_pair_then`]'s outcome into a [`PickedNative`] for either
/// caller below -- both need this exact translation and differ only in where
/// `native_path` itself came from (a naming guess vs. the cutter's own picker
/// choice).
fn picked_native_from_pair_result(native_path: PathBuf, outcome: ReadOutcome) -> PickedNative {
    match outcome {
        ReadOutcome::Design(loaded) => PickedNative::Design {
            native_path,
            loaded,
        },
        ReadOutcome::Pair {
            parsed_native,
            asc_text,
            native_text,
        } => PickedNative::Pair(Box::new(PickedPair {
            native_path,
            parsed_native,
            asc_text,
            native_text,
        })),
        ReadOutcome::SelfContained {
            loaded,
            asc_filename,
        } => PickedNative::SelfContained {
            native_path,
            loaded,
            asc_filename,
        },
    }
}

fn read_picked_asc_then(
    ui: &MainWindow,
    asc_path: PathBuf,
    on_done: impl FnOnce(&MainWindow, Option<PickedNative>) + 'static,
) {
    let native_path = native_path_for_asc(&asc_path);
    if native_path.is_file() {
        // `native_path` itself is still borrowed for this very call while
        // `on_done` is being constructed below (it moves its own copy in).
        let native_path_for_read = native_path.clone();
        read_native_pair_then(ui, &native_path_for_read, move |ui, result| {
            on_done(
                ui,
                result.map(|outcome| picked_native_from_pair_result(native_path, outcome)),
            );
        });
        return;
    }

    let asc_text = match read_asc_text(&asc_path) {
        Ok(text) => text,
        Err(e) => {
            show_toast(
                ui,
                &format!("Failed to read {}: {e}", asc_path.display()),
                "error",
            );
            on_done(ui, None);
            return;
        }
    };
    on_done(ui, Some(PickedNative::AscOnly { asc_path, asc_text }));
}

/// Shows the "Open" picker (accepting `.toml`, `.asc`, `.gem` or `.gcs`)
/// and reads whichever one the cutter picked. `on_done`'s `None` is a
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

/// Reads the design at `picked_path`, whichever kind it is: a `.asc` (with its older
/// sidecar beside it, if any), a `.gem`/`.gcs`, or a native file (a `.indicatrix`
/// design file or an older sidecar, told apart by content). The shared body of the
/// Open picker, File > Open Recent and the command-line open; `on_done`'s `None` is
/// any read/parse failure, already toasted.
pub(super) fn read_picked_path_then(
    ui: &MainWindow,
    picked_path: PathBuf,
    on_done: impl FnOnce(&MainWindow, Option<PickedNative>) + 'static,
) {
    if picked_path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("asc"))
    {
        read_picked_asc_then(ui, picked_path, on_done);
        return;
    }
    if ForeignFormat::from_path(&picked_path).is_some() {
        match read_foreign_design(&picked_path) {
            Ok(converted) => on_done(ui, Some(PickedNative::Converted(converted))),
            Err(message) => {
                show_toast(ui, &message, "error");
                on_done(ui, None);
            }
        }
        return;
    }
    // `picked_path` itself is still borrowed for this very call while
    // `on_done` is being constructed below (it moves its own copy in).
    let picked_path_for_read = picked_path.clone();
    read_native_pair_then(ui, &picked_path_for_read, move |ui, result| {
        on_done(
            ui,
            result.map(|outcome| picked_native_from_pair_result(picked_path, outcome)),
        );
    });
}
