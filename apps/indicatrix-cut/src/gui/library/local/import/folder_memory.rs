//! Remembers where the cutter last imported/saved to, and quietly checks that
//! folder for new `.asc` files once per session.

use super::confirm::count_pending_collisions;
use crate::{LibraryModel, MainWindow, gui::show_toast, settings::SettingsPersister};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Weak};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

/// The folder the cutter's most recent native
/// save/open used (`AppSettings::recent_native_files`'s first, most-recent entry),
/// non-recursively. `None` when nothing has been saved/opened natively this app knows
/// of yet, or that file's own path has no parent directory (should not happen for a
/// real saved file, but never assumed).
pub(super) fn last_save_folder(settings_store: &Arc<SettingsPersister>) -> Option<PathBuf> {
    settings_store
        .snapshot()
        .settings
        .recent_native_files
        .first()
        .map(PathBuf::from)
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

/// The library never rescans the save folder on its own; once per session, this
/// quietly checks [`last_save_folder`] for `.asc` files not yet in
/// the local catalogue at all (the `total - collisions` half of
/// [`count_pending_collisions`]'s own count) and, if it finds any, toasts a pointer at
/// the existing "Choose folder..." picker rather than importing anything itself.
///
/// Deliberately informational, not automatic: this app's own precedent for "something
/// found at startup" (`setup_startup_restore`, `gui::editor::mod`) is an explicit
/// OFFER, never a silent action, for the same "must not surprise the cutter with an
/// unasked-for write" reasoning -- an automatic
/// import here would also have to decide, unasked, what to do about any filename
/// COLLISION it found, which this module treats as something that must never happen
/// silently (see `super::confirm::confirm_collisions_then`).
///
/// Runs off the UI thread (the folder listing plus one `has_detail_for_entry_url`
/// point lookup per candidate is cheap, but this still follows
/// `gui::batch::preview::scan::spawn_missing_preview_scan`'s own "never block startup
/// on catalogue I/O" convention) and posts its toast back via
/// `Weak::upgrade_in_event_loop`.
pub(super) fn spawn_save_folder_rescan(
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    settings_store: Arc<SettingsPersister>,
) {
    thread::spawn(move || {
        let Some(folder) = last_save_folder(&settings_store) else {
            return;
        };
        let (collisions, total) = count_pending_collisions(&db, &folder, false);
        let new_files = total - collisions;
        if new_files == 0 {
            return;
        }
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            show_toast(
                &ui,
                &format!(
                    "{new_files} new .asc file(s) found in your last save folder -- open \
                     Import > Choose folder to bring {} in.",
                    if new_files == 1 { "it" } else { "them" }
                ),
                "info",
            );
        });
    });
}

/// The folder the last import came from, as a
/// [`crate::gui::pickers::PickerRequest::starting_dir`] -- `None`
/// (leaving the OS's own default in place) when nothing has been imported yet,
/// or the remembered folder has since been moved or deleted (checked here
/// rather than handed to `rfd` unchecked, since it silently ignores a missing
/// directory on some platforms and falls back on others).
pub(super) fn last_import_directory(settings_store: &Arc<SettingsPersister>) -> Option<PathBuf> {
    let remembered = settings_store.snapshot().settings.last_import_directory;
    if remembered.is_empty() {
        return None;
    }
    let path = PathBuf::from(remembered);
    path.is_dir().then_some(path)
}

/// Records where the cutter just imported from, so the next picker opens there.
/// A `None` directory (a path with no parent, which a picked file should never
/// have) leaves the previous value alone rather than clearing it.
pub(super) fn remember_import_directory(
    settings_store: &Arc<SettingsPersister>,
    directory: Option<&Path>,
) {
    let Some(directory) = directory else {
        return;
    };
    let as_string = directory.to_string_lossy().into_owned();
    settings_store.update(|s| s.settings.last_import_directory.clone_from(&as_string));
}

/// Clears the previous import's progress readout before starting a new one -- without
/// this, `import_done`/`import_total`/`import_progress` would keep showing the last
/// import's finished state briefly, before this import's first progress callback fires.
pub(super) fn reset_import_progress(ui: &MainWindow) {
    ui.global::<LibraryModel>().set_import_done(0);
    ui.global::<LibraryModel>().set_import_total(0);
    ui.global::<LibraryModel>().set_import_progress(0.0);
}
