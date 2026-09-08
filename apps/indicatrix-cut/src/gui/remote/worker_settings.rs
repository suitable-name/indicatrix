//! `WorkerItem` (the Slint-facing struct) <-> `settings::WorkerSettings` (the persisted
//! one) conversions, and rebuilding `MainWindow.remote_workers` from the settings
//! store's current worker list.
//!
//! Split out of `gui::remote` purely to keep that module (already sizeable) from
//! growing further -- same reasoning as `gui::detail`/`gui::search`/`gui::remote`
//! itself.

use crate::{
    MainWindow, RemoteWorkerModel, WorkerItem,
    settings::{PreviewScale, WorkerSettings},
};
use indicatrix_net::messages::TransferMode;
use slint::{ComponentHandle, ModelRc, VecModel};

const fn transfer_mode_index(mode: TransferMode) -> i32 {
    match mode {
        TransferMode::LiveProgressive => 0,
        TransferMode::FinalOnly => 1,
    }
}

const fn transfer_mode_from_index(idx: i32) -> TransferMode {
    if idx == 1 {
        TransferMode::FinalOnly
    } else {
        TransferMode::LiveProgressive
    }
}

const fn preview_scale_parts(scale: PreviewScale) -> (i32, i32) {
    match scale {
        PreviewScale::Full => (0, 50),
        PreviewScale::Half => (1, 50),
        PreviewScale::Quarter => (2, 50),
        PreviewScale::Custom(pct) => (3, pct as i32),
    }
}

fn preview_scale_from_parts(idx: i32, pct: i32) -> PreviewScale {
    match idx {
        1 => PreviewScale::Half,
        2 => PreviewScale::Quarter,
        3 => PreviewScale::Custom(pct.clamp(1, 100) as u32),
        _ => PreviewScale::Full,
    }
}

fn to_worker_item(w: &WorkerSettings) -> WorkerItem {
    let (preview_scale_index, preview_scale_percent) = preview_scale_parts(w.preview_scale);
    WorkerItem {
        name: w.name.clone().into(),
        address: w.address.clone().into(),
        cert_dir: w.cert_dir.clone().into(),
        transfer_mode_index: transfer_mode_index(w.transfer_mode),
        cadence_ms: w.cadence_ms as i32,
        preview_scale_index,
        preview_scale_percent,
    }
}

pub(super) fn from_worker_item(item: &WorkerItem) -> WorkerSettings {
    WorkerSettings {
        name: item.name.to_string(),
        address: item.address.to_string(),
        cert_dir: item.cert_dir.to_string(),
        transfer_mode: transfer_mode_from_index(item.transfer_mode_index),
        cadence_ms: item.cadence_ms.max(1) as u32,
        preview_scale: preview_scale_from_parts(
            item.preview_scale_index,
            item.preview_scale_percent,
        ),
    }
}

/// Rebuilds `MainWindow.remote_workers` from the settings store's current worker list.
/// Called after startup load and after every add/edit/remove, matching
/// `refresh_lighting_preset_options`'s pattern in `gui::mod`.
pub fn refresh_worker_options(ui: &MainWindow, workers: &[WorkerSettings]) {
    let items: Vec<WorkerItem> = workers.iter().map(to_worker_item).collect();
    ui.global::<RemoteWorkerModel>()
        .set_workers(ModelRc::new(VecModel::from(items)));
}
