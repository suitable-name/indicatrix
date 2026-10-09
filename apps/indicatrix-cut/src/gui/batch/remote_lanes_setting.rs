//! The "Remote lanes for batches" spin box in the remote coordinator panel
//! (`remote_worker_dialog.slint`): how many pictures a preview or tilt batch keeps in
//! flight on the remote at once, persisted as `AppSettings::remote_batch_lanes`.
//!
//! The sibling "Pictures per preview request" spin box persists
//! `AppSettings::remote_preview_batch_size` the same way.
//!
//! The batches read the setting fresh from the settings snapshot each time one starts
//! (`gui::batch::preview::wiring`, `gui::batch::tilt::wiring`), so a change takes effect
//! on the next batch and never touches one already running.

use crate::{MainWindow, RemoteWorkerModel, settings::SettingsPersister};
use slint::ComponentHandle;
use std::sync::Arc;

/// Shows the saved lane count in the spin box and persists every change the user makes.
///
/// The count is limited to `1..=32` by `AppSettings::set_remote_batch_lanes`; the stored
/// value is written back to the spin box so it can never show a count the batches would
/// not use. Called once, at startup.
pub fn setup_remote_batch_lanes(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    ui.global::<RemoteWorkerModel>()
        .set_remote_batch_lanes(settings_store.snapshot().settings.remote_batch_lanes as i32);

    ui.global::<RemoteWorkerModel>()
        .set_remote_preview_batch_size(
            settings_store.snapshot().settings.remote_preview_batch_size as i32,
        );
    let size_store = Arc::clone(settings_store);
    let size_ui_weak = ui.as_weak();
    ui.global::<RemoteWorkerModel>()
        .on_remote_preview_batch_size_changed(move |size: i32| {
            size_store.update(|s| s.settings.set_remote_preview_batch_size(size.max(1) as u32));
            let stored = size_store.snapshot().settings.remote_preview_batch_size as i32;
            if let Some(ui) = size_ui_weak.upgrade()
                && ui
                    .global::<RemoteWorkerModel>()
                    .get_remote_preview_batch_size()
                    != stored
            {
                ui.global::<RemoteWorkerModel>()
                    .set_remote_preview_batch_size(stored);
            }
        });

    let settings_store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    ui.global::<RemoteWorkerModel>()
        .on_remote_batch_lanes_changed(move |lanes: i32| {
            // A spin box never goes below its own minimum; `max(1)` keeps a stray
            // negative from wrapping into a huge unsigned count before the limit runs.
            settings_store.update(|s| s.settings.set_remote_batch_lanes(lanes.max(1) as u32));
            let stored = settings_store.snapshot().settings.remote_batch_lanes as i32;
            if let Some(ui) = ui_weak.upgrade()
                && ui.global::<RemoteWorkerModel>().get_remote_batch_lanes() != stored
            {
                ui.global::<RemoteWorkerModel>()
                    .set_remote_batch_lanes(stored);
            }
        });
}
