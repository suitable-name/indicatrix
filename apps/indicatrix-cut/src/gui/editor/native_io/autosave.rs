//! Group 4: the autosave timer -- writes a self-contained recovery snapshot every
//! two minutes while the design is dirty, independent of whether it currently
//! solves. [`find_leftover_autosave`] is this group's own startup-recovery check.

use super::{
    atomic_write::{temp_sibling, write_synced},
    design_paths::{autosave_file_name, is_autosave_file_name, legacy_autosave_file_name},
    save_helpers::{custom_material_snapshot_for_save, design_file_text},
};
use crate::{
    EditorModel, MainWindow,
    gui::{editor::state::EditorState, show_toast},
    settings::SettingsPersister,
};
use indicatrix_cut_core::native::DesignExtras;
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::warn;

/// How often the autosave timer checks [`EditorState::is_dirty`] and, if `true`,
/// writes a recovery snapshot. Two minutes:
/// frequent enough that a crash loses at most a couple of minutes of edits,
/// infrequent enough that it never shows up as a hitch even on a slow disk or a
/// large design. Gated on `is_dirty` (never writes while nothing has changed) and
/// never touches the cutter's own save path -- see [`autosave_path`]/
/// [`run_autosave_tick`].
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(120);

thread_local! {
    /// Keeps the autosave `slint::Timer` alive for the life of the window -- a
    /// `Timer` stops as soon as its value is dropped, and this module owns no
    /// longer-lived struct to park the handle in (unlike `gui::mod`'s own
    /// `remote_rendering_timer` field). Mirrors `retarget_actions::RETARGET_ASYNC`'s
    /// own reasoning for using a `thread_local!` here: Slint's event loop is
    /// single-threaded, so this is sound without any real synchronization.
    static AUTOSAVE_TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };

    /// The recovery file the most recent autosave tick dispatched a write to, until a
    /// save takes it ([`take_last_autosave_path`]). A thread-local for the same reason
    /// [`AUTOSAVE_TIMER`] is: only UI-thread callbacks read or write it.
    static LAST_AUTOSAVE_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Takes the path of the recovery file the last autosave tick wrote, if any tick has
/// run since the previous take.
///
/// A landed save deletes this file (via the listener [`setup_autosave_timer`]
/// registers), not just [`autosave_path`] of its own file name: the autosave is named
/// after the design's name AT THE TIME of the tick (`untitled.autosave.indicatrix`
/// for a design never saved), which is not the name the save gives it. Deleting only the
/// file named after the new name left the untitled one behind, and the "Recover unsaved
/// work?" prompt came back on every start.
fn take_last_autosave_path() -> Option<PathBuf> {
    LAST_AUTOSAVE_PATH.with(|cell| cell.borrow_mut().take())
}

/// Starts the autosave timer -- bundled into [`setup_save_native_callback`] (called
/// exactly once, like every other `setup_*` entry point here) rather than given its
/// own, the same reasoning [`setup_dirty_tracking`] documents on itself.
///
/// # Recovery
///
/// The autosave lands at [`autosave_path`]: `<name>.autosave.indicatrix` inside
/// this app's OWN settings directory (`settings::store::default_settings_path`'s
/// parent directory), never beside the cutter's real save location and never the
/// cutter's real design file (this design's own path isn't tracked here). After a
/// crash, the startup prompt offers it, or a cutter chooses File > Open and browses
/// to that file directly; it opens exactly like any other `.indicatrix` design file.
///
/// The autosave is a complete design file (see [`design_file_text`]), carrying every
/// tier in full, so restoring it needs nothing else on disk. Autosaves written by
/// older versions (`<name>.indicatrix.autosave.toml`) are still found and opened.
pub(super) fn setup_autosave_timer(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
) {
    let state = Rc::clone(state);
    let db = Arc::clone(db);
    let ui_weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, AUTOSAVE_INTERVAL, move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        crate::gui::editor::stall_guard::stall_guard("autosave_tick", || {
            run_autosave_tick(&ui, &state, &db);
        });
    });
    AUTOSAVE_TIMER.with(|cell| *cell.borrow_mut() = Some(timer));
    // A landed save makes the recovery file it stood in for obsolete, whatever name the
    // design was autosaved under -- see `take_last_autosave_path`.
    super::on_save_completed(|_ui, _state| {
        if let Some(stale) = take_last_autosave_path() {
            let _ = std::fs::remove_file(stale);
        }
    });
}

/// One autosave timer tick: writes a recovery snapshot iff [`EditorState::is_dirty`]
/// -- see [`setup_autosave_timer`]'s own doc comment.
///
/// Writes a self-contained design file ([`design_file_text`]) that carries every
/// tier's real `angle_deg`/`indices` directly, regardless of whether `design`
/// currently solves. Autosave needs no matching cached solve at all -- there is no
/// solve step to skip -- so every dirty tick produces a recoverable snapshot, not
/// just the ones lucky enough to land after a background solve completed. Carries
/// the same custom-material snapshot/history trail as a real Save so a
/// crash recovery does not itself reintroduce the "reopens as Diamond" bug.
///
/// Still skips the tick entirely while a background solve
/// (`EditorModel.solve_running`) is in flight, so this and that solve never stack
/// against the same `RenderContext`/database locks.
///
/// Group 4: the actual write ([`write_autosave`]) runs on a background thread,
/// never the UI thread; building the TOML string itself
/// ([`design_file_text`]) is in-memory serialization with no solving and no I/O, but it
/// encodes the design's attachments, so it runs on that same background thread.
fn run_autosave_tick(ui: &MainWindow, state: &Rc<RefCell<EditorState>>, db: &Arc<Mutex<Database>>) {
    if ui.global::<EditorModel>().get_solve_running() {
        return;
    }
    let (design, asc_filename, printed_proportions, history_entries, file_extras) = {
        let st = state.borrow();
        if !st.is_dirty() {
            return;
        }
        (
            st.design.clone(),
            st.asc_filename.clone(),
            st.printed_proportions,
            st.history.description_log().to_vec(),
            st.file_extras.clone(),
        )
    };
    let custom_material = custom_material_snapshot_for_save(&design, db);
    let path = autosave_path(asc_filename.as_deref());
    // Remembered so the save that eventually lands can delete THIS file even though the
    // design may have a different name by then -- see `take_last_autosave_path`.
    LAST_AUTOSAVE_PATH.with(|cell| *cell.borrow_mut() = Some(path.clone()));
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || {
        // Serialised here, off the UI thread: the design's attachments (PDFs, images)
        // are encoded and hashed, which is the one expensive part of the snapshot.
        let extras = DesignExtras {
            custom_material: custom_material.as_ref(),
            history_entries: &history_entries,
            metadata: Some(&file_extras.metadata),
            attachments: &file_extras.attachments,
        };
        // Serialization is unreachable in practice for a design that was read from a
        // valid file (see `design_file_text`'s own doc comment) and there is nothing
        // actionable to toast for a silent background autosave.
        let Ok(native_toml) =
            design_file_text(&design, printed_proportions.as_ref(), &extras, false)
        else {
            return;
        };
        let result = write_autosave(&path, &native_toml);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            if let Err(e) = result {
                show_toast(
                    &ui,
                    &format!("Autosave to {} failed: {e}", path.display()),
                    "error",
                );
            }
        });
    });
}

/// The settings directory autosaves live in, falling back to the OS temp directory on
/// the rare platform where even that can't be resolved -- either way, never a
/// location the cutter chose or the design's own files live at.
fn autosave_dir() -> PathBuf {
    crate::settings::store::default_settings_path()
        .parent()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf)
}

/// Where [`run_autosave_tick`] writes: `<name>.autosave.indicatrix` inside this app's
/// own settings directory.
pub(super) fn autosave_path(asc_filename: Option<&str>) -> PathBuf {
    autosave_dir().join(autosave_file_name(asc_filename))
}

/// Removes the recovery files for the design recorded as `asc_filename`, in both the
/// current (`<name>.autosave.indicatrix`) and the older
/// (`<name>.indicatrix.autosave.toml`) naming. Errors are ignored: a missing file is the
/// ordinary case.
pub(super) fn delete_autosaves_for(asc_filename: Option<&str>) {
    let dir = autosave_dir();
    let _ = std::fs::remove_file(dir.join(autosave_file_name(asc_filename)));
    let _ = std::fs::remove_file(dir.join(legacy_autosave_file_name(asc_filename)));
}

/// This is the startup check for a leftover autosave file, exposed for
/// `gui::editor::mod`'s startup sequence to call before [`EditorState::fresh`]
/// replaces whatever the previous session had. Without it, a crash still loses the
/// session in practice even though the autosave file itself is written, since
/// nothing checks for one or prompts to restore it on launch.
///
/// Scans [`autosave_path`]'s own directory (never recurses -- there is nothing to
/// recurse into, every autosave lands flat in this one app-settings directory) for
/// any `*.autosave.indicatrix` file (or an older `*.indicatrix.autosave.toml` one) and
/// returns the most recently modified one, if any. Does not open or delete anything itself: a leftover file only means SOME
/// past session was dirty when it stopped ticking (a real crash, or simply the
/// window closing between one 120-second tick and the next), never that the design
/// in it is still wanted -- the caller decides whether to offer it and, either way,
/// to remove it afterward (matching [`setup_save_native_callback`]'s own
/// successful-save cleanup, just above).
#[must_use]
pub(in crate::gui::editor) fn find_leftover_autosave() -> Option<PathBuf> {
    // Lists every leftover, not just the newest, so the newest can be picked out
    // here while the "Delete" action on the startup prompt
    // (`delete_leftover_autosave`) still removes only the ONE file actually
    // offered -- see that function's own doc comment (owner decision 4.5).
    find_all_leftover_autosaves()
        .into_iter()
        .max_by_key(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok())
}

/// Every autosave file (either naming) currently on disk, in
/// [`find_leftover_autosave`]'s own directory -- that function's own list, minus
/// the "keep only the newest" reduction. An older leftover from a DIFFERENT
/// design than the one just offered is neither opened nor removed by this
/// crate today (owner decision 4.5): it simply sits there until its own design
/// is reopened and saved again (which deletes it, see
/// [`finish_save_native_success`]'s own doc comment) or a cutter clears it by
/// hand. Exposed only to [`find_leftover_autosave`] itself; nothing outside
/// this module needs the full list.
fn find_all_leftover_autosaves() -> Vec<PathBuf> {
    std::fs::read_dir(autosave_dir())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_autosave_file_name)
        })
        .collect()
}

/// The startup-restore prompt's "Delete" action (owner decision 4.5): removes
/// exactly the one leftover autosave file that was actually OFFERED (never any
/// other leftover [`find_all_leftover_autosaves`] may also have found -- an
/// older leftover from a different design is left untouched, see that
/// function's own doc comment). Errors are swallowed: there is nothing left to
/// toast into once the startup prompt itself is already gone, and a failed
/// delete only means the same file may be offered again next launch, never
/// that any design data is lost.
pub(in crate::gui::editor) fn delete_leftover_autosave(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Writes `native_toml` to `path` via the same stage-then-rename discipline
/// [`write_file_atomically`] uses, so a crash mid-autosave-write can never leave a
/// half-written, corrupt recovery file behind either. The staged file is flushed to
/// disk before the rename publishes it: without that, a power loss right after the
/// rename can leave the recovery file present but empty, which is exactly the moment
/// the file exists for.
///
/// # Errors
///
/// A ready-to-toast message.
pub(super) fn write_autosave(path: &Path, native_toml: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = temp_sibling(path);
    write_synced(&tmp, native_toml).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

/// "Open Recent" write side: records `native_path_display` as the
/// most-recently-used design file in the settings store
/// ([`crate::settings::model::AppSettings::record_recent_native_file`]) and
/// refreshes `MainWindow.recent_native_files` so File > Open Recent reflects it
/// immediately, with no restart needed. Called after every successful Save
/// and Open (both the picker and File > Open Recent itself, since both funnel
/// through [`commit_loaded_native`]) -- never for [`open_plain_asc`]'s bare-`.asc`
/// path, which has no design file to name.
///
/// The entry goes through the application's [`SettingsPersister`] (the handle
/// installed by `gui::main_window`, see [`SettingsPersister::install_for_this_thread`]),
/// never straight into the settings file: the persister's in-memory snapshot is what
/// every close path flushes over the file, so an entry written around it would be
/// erased on exit. With no persister installed (only possible before the main window
/// is built) nothing is recorded.
pub(super) fn record_recent_native_file(ui: &MainWindow, native_path_display: &str) {
    let Some(persister) = SettingsPersister::installed_for_this_thread() else {
        warn!(
            "No settings persister is installed; not recording {native_path_display} as a recent file"
        );
        return;
    };
    persister.update(|file| {
        file.settings
            .record_recent_native_file(native_path_display.to_string());
    });
    ui.set_recent_native_files(ModelRc::new(VecModel::from(
        persister
            .snapshot()
            .settings
            .recent_native_files
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
}
