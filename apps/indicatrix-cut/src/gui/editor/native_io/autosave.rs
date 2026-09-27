//! Group 4: the autosave timer -- writes a self-contained recovery snapshot every
//! two minutes while the design is dirty, independent of whether it currently
//! solves. [`find_leftover_autosave`] is this group's own startup-recovery check.

use super::{
    atomic_write::temp_sibling,
    save_helpers::{custom_material_snapshot_for_save, snapshot_custom_materials},
};
use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{editor::state::EditorState, show_toast},
};
use indicatrix_cut_core::native::{SaveExtras, save_native_only_toml};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

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
}

/// Starts the autosave timer -- bundled into [`setup_save_native_callback`] (called
/// exactly once, like every other `setup_*` entry point here) rather than given its
/// own, the same reasoning [`setup_dirty_tracking`] documents on itself.
///
/// # Recovery
///
/// The autosave lands at [`autosave_path`]: `<name>.indicatrix.autosave.toml` inside
/// this app's OWN settings directory (`settings::store::default_settings_path`'s
/// parent directory), never beside the cutter's real save location and never the
/// cutter's real `.asc`/native pair (this design's own path isn't tracked here).
/// After a crash, a cutter recovers by choosing File > Open Native and browsing to
/// that file directly; it opens exactly like any other native design file, picked
/// directly (not through [`read_picked_asc`]'s own bare-`.asc` path).
///
/// The autosave file is written by
/// [`indicatrix_cut_core::native::save_native_only`] (via [`run_autosave_tick`]),
/// which carries `design` in FULL -- every tier's real `angle_deg`/`indices` and
/// the `.asc`-only `meta` fields a native file otherwise never records at all (see
/// that function's own doc comment) -- so [`read_native_pair`]'s restore path can
/// call [`indicatrix_cut_core::native::load_native_only`] and rebuild the design
/// with NO paired `.asc` text needed, or even present on disk. A cutter recovers by
/// choosing File > Open Native and browsing to the autosave file directly; whether
/// `asc_filename`'s own file exists anywhere does not matter.
pub(super) fn setup_autosave_timer(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let db = Arc::clone(db);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, AUTOSAVE_INTERVAL, move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        crate::gui::editor::stall_guard::stall_guard("autosave_tick", || {
            run_autosave_tick(&ui, &state, &db, &render_ctx);
        });
    });
    AUTOSAVE_TIMER.with(|cell| *cell.borrow_mut() = Some(timer));
}

/// One autosave timer tick: writes a recovery snapshot iff [`EditorState::is_dirty`]
/// -- see [`setup_autosave_timer`]'s own doc comment.
///
/// Writes via [`indicatrix_cut_core::native::save_native_only_toml`] rather than
/// [`save_paired_reusing_solve`] -- a self-contained snapshot that carries every
/// tier's real `angle_deg`/`indices` directly, regardless of whether `design`
/// currently solves, so [`read_native_pair`]'s restore side never needs the
/// cutter's real `.asc` file (which may never have existed, or been moved/deleted)
/// to reopen it. Autosave needs no matching cached solve at all -- there is no
/// solve step to skip -- so every dirty tick produces a recoverable snapshot, not
/// just the ones lucky enough to land after a background solve completed. Carries
/// the same custom-material snapshot/history trail as a real Save Native so a
/// crash recovery does not itself reintroduce the "reopens as Diamond" bug.
///
/// Still skips the tick entirely while a background solve
/// (`EditorModel.solve_running`) is in flight, so this and that solve never stack
/// against the same `RenderContext`/database locks.
///
/// Group 4: the actual write ([`write_autosave`]) runs on a background thread,
/// never the UI thread; building the TOML string itself
/// (`save_native_only_toml`) is a plain, cheap in-memory serialization -- no
/// solving, no I/O -- so it stays on the UI thread like every other snapshot-then-
/// hand-to-a-worker call in this module.
fn run_autosave_tick(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    if ui.global::<EditorModel>().get_solve_running() {
        return;
    }
    let (design, asc_filename, printed_proportions, history_entries) = {
        let st = state.borrow();
        if !st.is_dirty() {
            return;
        }
        (
            st.design.clone(),
            st.asc_filename.clone(),
            st.printed_proportions,
            st.history.description_log().to_vec(),
        )
    };
    let custom_material = custom_material_snapshot_for_save(&design, db);
    // Custom-catalogue-aware -- see
    // `snapshot_custom_materials`'s own doc comment.
    let custom_materials = snapshot_custom_materials(render_ctx);
    let Ok(native_toml) = save_native_only_toml(
        &design,
        format!("{}.asc", autosave_base_name(asc_filename.as_deref())),
        printed_proportions.as_ref(),
        &SaveExtras {
            custom_material: custom_material.as_ref(),
            history_entries: &history_entries,
            custom_catalogue: &custom_materials,
        },
    ) else {
        // `SaveError::Toml` is unreachable in practice (see its own doc comment) and
        // there is nothing actionable to toast for a silent background autosave.
        return;
    };
    let path = autosave_path(asc_filename.as_deref());
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || {
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

/// The base name an autosave is filed under: the design's own `asc_filename` (its
/// bare `.asc` name) with the extension stripped, or `"untitled"` for a design never
/// yet paired with one (a brand-new design, or the angle-table placeholder
/// reconstruction).
fn autosave_base_name(asc_filename: Option<&str>) -> String {
    let name = asc_filename.unwrap_or("untitled");
    name.strip_suffix(".asc").unwrap_or(name).to_string()
}

/// Where [`run_autosave_tick`] writes: `<name>.indicatrix.autosave.toml` inside this
/// app's own settings directory, falling back to the OS temp directory on the rare
/// platform where even that can't be resolved -- either way, never a location the
/// cutter chose or the design's own paired files live at.
pub(super) fn autosave_path(asc_filename: Option<&str>) -> PathBuf {
    let dir = crate::settings::store::default_settings_path()
        .parent()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf);
    dir.join(format!(
        "{}.indicatrix.autosave.toml",
        autosave_base_name(asc_filename)
    ))
}

/// This is the startup check for a leftover autosave file, exposed for
/// `gui::editor::mod`'s startup sequence to call before [`EditorState::fresh`]
/// replaces whatever the previous session had. Without it, a crash still loses the
/// session in practice even though the autosave file itself is written, since
/// nothing checks for one or prompts to restore it on launch.
///
/// Scans [`autosave_path`]'s own directory (never recurses -- there is nothing to
/// recurse into, every autosave lands flat in this one app-settings directory) for
/// any `*.indicatrix.autosave.toml` file and returns the most recently modified one,
/// if any. Does not open or delete anything itself: a leftover file only means SOME
/// past session was dirty when it stopped ticking (a real crash, or simply the
/// window closing between one 120-second tick and the next), never that the design
/// in it is still wanted -- the caller decides whether to offer it and, either way,
/// to remove it afterward (matching [`setup_save_native_callback`]'s own
/// successful-save cleanup, just above).
#[must_use]
pub(in crate::gui::editor) fn find_leftover_autosave() -> Option<PathBuf> {
    let dir = crate::settings::store::default_settings_path()
        .parent()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf);
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".indicatrix.autosave.toml"))
        })
        .max_by_key(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok())
}

/// Writes `native_toml` to `path` via the same stage-then-rename discipline
/// [`write_pair_atomically`] uses, so a crash mid-autosave-write can never leave a
/// half-written, corrupt recovery file behind either.
///
/// # Errors
///
/// A ready-to-toast message.
pub(super) fn write_autosave(path: &Path, native_toml: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = temp_sibling(path);
    std::fs::write(&tmp, native_toml).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

/// "Open Recent" write side: records `native_path_display` as the
/// most-recently-used native file in the settings store
/// ([`crate::settings::model::AppSettings::record_recent_native_file`]) and
/// refreshes `MainWindow.recent_native_files` so File > Open Recent reflects it
/// immediately, with no restart needed. Called after every successful Save Native
/// and Open Native (both the picker and File > Open Recent itself, since both funnel
/// through [`commit_loaded_native`]) -- never for [`open_plain_asc`]'s bare-`.asc`
/// path, which has no native file to name.
///
/// Reads/writes the settings file directly via [`crate::settings::store`] rather
/// than through the debounced [`crate::settings::SettingsPersister`]: this module has
/// no handle to it (`gui::editor::setup_editor_callbacks` doesn't thread one in, and
/// adding one would be a `gui::editor::mod`/`gui::mod` change). This is a real, if
/// narrow, race as a result: a settings change still
/// inside the persister's ~600ms debounce window when this runs writes its own
/// (older, recent-files-less) in-memory snapshot over this one the next time it
/// flushes, silently dropping the just-recorded entry. Accepted rather than leaving
/// the whole feature unwired -- the consequence is losing one recent-files entry
/// occasionally, never any design data, and the entry reappears next save/open
/// anyway.
pub(super) fn record_recent_native_file(ui: &MainWindow, native_path_display: &str) {
    let settings_path = crate::settings::store::default_settings_path();
    let mut file = crate::settings::store::load_or_default(&settings_path);
    file.settings
        .record_recent_native_file(native_path_display.to_string());
    let _ = crate::settings::store::save(&settings_path, &file);
    ui.set_recent_native_files(ModelRc::new(VecModel::from(
        file.settings
            .recent_native_files
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
}
