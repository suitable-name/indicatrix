//! Reads/parses whichever file the "Open Native" picker (or File > Open Recent)
//! names -- a real native+`.asc` pair, a self-contained native file with no paired
//! `.asc` involved, or a directly-picked bare `.asc` -- translating the result into
//! a [`PickedNative`] for [`super::open_native::do_open_native`] to dispatch.

use super::picker::{PickKind, pick_file};
use crate::{MainWindow, gui::show_toast};
use indicatrix_cut_core::{
    NativeDesignFile,
    native::{LoadNativeOnlyResult, load_native_only},
    native_path_for_asc,
};
use std::path::{Path, PathBuf};

/// What the "Open Native" picker actually returned -- see [`setup_open_native_callback`]'s
/// own doc comment for why the same picker also accepts a bare `.asc` (a
/// cutter handed a plain `GemCAD` file by email should not have to import it into the
/// catalogue database first just to look at it).
pub(super) enum PickedNative {
    /// A real native+`.asc` pair, ready for [`load_paired`]. Boxed: [`PickedPair`]
    /// carries a whole parsed [`NativeDesignFile`] plus both source texts, several
    /// times larger than [`Self::AscOnly`]'s bare path+text
    /// (`clippy::large_enum_variant`).
    Pair(Box<PickedPair>),
    /// A directly-picked bare `.asc` with no native sidecar found next to it --
    /// nothing [`load_paired`] has any use for (no per-tier overlay, no fingerprint,
    /// no draft flag); committed straight through
    /// `gui::editor::loading::design_from_asc_text` instead, in [`open_plain_asc`].
    AscOnly { asc_path: PathBuf, asc_text: String },
    /// A native file with NO paired `.asc` findable anywhere (recorded name,
    /// naming-guess, and -- unlike the other two variants -- no prompt for one
    /// either), opened via [`load_native_only`] instead: the autosave-restore case
    /// [`save_native_only`](indicatrix_cut_core::native::save_native_only)'s own
    /// doc comment describes. [`read_native_pair_then`] only ever produces this
    /// when the file actually parses as self-contained -- an ordinary native file
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
/// self-contained native file with no paired `.asc` involved at all. Kept
/// distinct from [`PickedNative`] itself (rather than reusing it directly)
/// since neither of `read_native_pair_then`'s
/// two callers has a `native_path` to attach until this returns.
pub(super) enum NativePairOrSelfContained {
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

/// Reads/parses a native file already known to live at `native_path`, plus its
/// recorded paired `.asc` -- split out of [`pick_native_or_asc_then`]/
/// [`read_picked_asc_then`] purely to keep both under clippy's line-count lint.
/// `on_done`'s `None` is any read/parse failure, each already toasted here before
/// calling it.
///
/// [`resolve_paired_asc_text_then`]'s own "Locate the paired .asc" recovery picker
/// runs off the UI thread, so this (and every caller up the chain) is
/// continuation-passing too. The `std::fs::read_to_string`/TOML-parse calls
/// themselves stay synchronous -- reading one small file is cheap enough not to
/// need the same treatment.
///
/// Before ever prompting for the paired `.asc`, tries [`load_native_only`] on
/// `native_text` iff neither the recorded nor the guessed `.asc` path exists on
/// disk -- see [`NativePairOrSelfContained::SelfContained`]'s own doc comment.
/// Almost every native file fails that check immediately ([`load_native_only`]
/// itself refuses anything that isn't a
/// [`save_native_only`](indicatrix_cut_core::native::save_native_only) file), so
/// an ordinary design with a genuinely missing `.asc` still reaches
/// [`resolve_paired_asc_text_then`]'s prompt.
pub(super) fn read_native_pair_then(
    ui: &MainWindow,
    native_path: &Path,
    on_done: impl FnOnce(&MainWindow, Option<NativePairOrSelfContained>) + 'static,
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
    // The native file's own recorded `asc_filename` (a bare file name -- see
    // `indicatrix_cut_core::NativeDesignFile::asc_filename`'s own doc comment) is read
    // AFTER parsing the chosen file, since that field is the authoritative pointer to
    // the real paired file once a native file has actually been parsed; this never
    // guesses at a sibling `.asc` name the way `indicatrix_cut_core::asc_path_for_native`
    // does for a picker's initial directory (there is no picker here to seed -- the
    // native file's own directory plus its own recorded name is exact).
    let parsed_native = match indicatrix_cut_core::native::parse_toml_string(&native_text) {
        Ok(n) => Box::new(n),
        Err(e) => {
            show_toast(
                ui,
                &format!(
                    "'{}' is not a valid native design file: {e}",
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
    // Not self-contained (an ordinary native file whose `.asc` genuinely went
    // missing) falls through to the same prompt this always showed.
    if !asc_findable && let Ok(loaded) = load_native_only(&native_text) {
        on_done(
            ui,
            Some(NativePairOrSelfContained::SelfContained {
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
                asc_text.map(|asc_text| NativePairOrSelfContained::Pair {
                    parsed_native,
                    asc_text,
                    native_text,
                }),
            );
        },
    );
}

/// Reads the paired `.asc`'s text, recovering from a moved/renamed file (common
/// after `GemCad`'s own Save As) instead of giving up outright. Tries, in
/// order: `recorded_asc_path` (the native file's own authoritative
/// `asc_filename`, exact when it still holds); [`indicatrix_cut_core::asc_path_for_native`]'s
/// naming guess (only meaningful when it names a DIFFERENT path -- most native files'
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
    if let Ok(text) = std::fs::read_to_string(recorded_asc_path) {
        on_done(ui, Some(text));
        return;
    }
    if let Some(guessed) = indicatrix_cut_core::asc_path_for_native(native_path)
        && guessed != recorded_asc_path
        && let Ok(text) = std::fs::read_to_string(&guessed)
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
        match std::fs::read_to_string(&located) {
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

/// A directly-picked `.asc` file: checks for a sibling native sidecar first (via
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
fn picked_native_from_pair_result(
    native_path: PathBuf,
    outcome: NativePairOrSelfContained,
) -> PickedNative {
    match outcome {
        NativePairOrSelfContained::Pair {
            parsed_native,
            asc_text,
            native_text,
        } => PickedNative::Pair(Box::new(PickedPair {
            native_path,
            parsed_native,
            asc_text,
            native_text,
        })),
        NativePairOrSelfContained::SelfContained {
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

    let asc_text = match std::fs::read_to_string(&asc_path) {
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

/// Shows the "Open Native" picker (accepting `.toml` OR `.asc`) and reads
/// whichever one the cutter picked. `on_done`'s `None` is a cancellation or any
/// read/parse failure, each already toasted before calling it.
///
/// The picker itself ([`PickKind::OpenNativeOrAsc`]) runs on a background
/// thread via [`pick_file`] -- see that function's own `match` for the filter
/// shape it builds.
pub(super) fn pick_native_or_asc_then(
    ui: &MainWindow,
    on_done: impl FnOnce(&MainWindow, Option<PickedNative>) + 'static,
) {
    pick_file(ui, PickKind::OpenNativeOrAsc, move |ui, picked_path| {
        let Some(picked_path) = picked_path else {
            // A dismissed file dialog is the cutter's own deliberate cancel --
            // no toast needed.
            on_done(ui, None);
            return;
        };
        if picked_path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("asc"))
        {
            read_picked_asc_then(ui, picked_path, on_done);
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
    });
}
