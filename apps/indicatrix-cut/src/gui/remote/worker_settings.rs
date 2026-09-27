//! `WorkerItem` (the Slint-facing struct) <-> `settings::RemoteEndpoint` (the persisted
//! one) conversions, and pushing the configured endpoint to every UI surface that shows
//! or defaults from it ([`refresh_remote_ui`]).
//!
//! Split out of `gui::remote` purely to keep that module (already sizeable) from
//! growing further -- same reasoning as `gui::detail`/`gui::search`/`gui::remote`
//! itself.

use crate::{
    MainWindow, RemoteWorkerModel, SettingsModel, TiltVideoExportModel, WorkerItem,
    settings::{ExportTransfer, PreviewScale, RemoteEndpoint, WorkerSettings},
};
use indicatrix_net::messages::TransferMode;
use slint::ComponentHandle;

const fn transfer_mode_index(mode: TransferMode) -> i32 {
    match mode {
        // `DisplayOnly` is chosen per live request by the "Live Transfer" setting
        // (`LiveTransfer::FinalPicture`), never stored here; a hand-edited settings
        // file carrying it shows as progressive.
        TransferMode::LiveProgressive | TransferMode::DisplayOnly => 0,
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

fn to_worker_item(endpoint: &RemoteEndpoint) -> WorkerItem {
    let w = &endpoint.connection;
    let (preview_scale_index, preview_scale_percent) = preview_scale_parts(w.preview_scale);
    WorkerItem {
        name: w.name.clone().into(),
        address: w.address.clone().into(),
        cert_dir: w.cert_dir.clone().into(),
        transfer_mode_index: transfer_mode_index(w.transfer_mode),
        cadence_ms: w.cadence_ms as i32,
        preview_scale_index,
        preview_scale_percent,
        export_transfer_index: endpoint.export_transfer.index(),
    }
}

/// The form's contents as an endpoint. The live transfer is not on the form (it lives in
/// the settings dialog), so it is carried over from `previous` when there was one.
pub(super) fn from_worker_item(
    item: &WorkerItem,
    previous: Option<&RemoteEndpoint>,
) -> RemoteEndpoint {
    RemoteEndpoint {
        connection: WorkerSettings {
            name: item.name.to_string(),
            address: item.address.to_string(),
            cert_dir: item.cert_dir.to_string(),
            transfer_mode: transfer_mode_from_index(item.transfer_mode_index),
            cadence_ms: item.cadence_ms.max(1) as u32,
            preview_scale: preview_scale_from_parts(
                item.preview_scale_index,
                item.preview_scale_percent,
            ),
        },
        export_transfer: ExportTransfer::from_index(item.export_transfer_index),
        live_transfer: previous.map(|p| p.live_transfer).unwrap_or_default(),
    }
}

/// Pushes the configured remote endpoint (or its absence) to the "Remote coordinator"
/// form (`RemoteWorkerModel.configured`/`endpoint`), the settings dialog's "Live
/// Transfer" pills and the tilt video's "Transfer" row (whose default it is). Called
/// after startup load and after every save/remove.
pub fn refresh_remote_ui(ui: &MainWindow, remote: Option<&RemoteEndpoint>) {
    let model = ui.global::<RemoteWorkerModel>();
    model.set_configured(remote.is_some());
    model.set_endpoint(to_worker_item(&remote.cloned().unwrap_or_default()));
    ui.global::<SettingsModel>()
        .set_live_transfer_index(remote.map_or(0, |r| r.live_transfer.index()));
    let video = ui.global::<TiltVideoExportModel>();
    video.set_remote_configured(remote.is_some());
    video.set_transfer_index(remote.map_or(0, |r| r.export_transfer.index()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::LiveTransfer;

    /// The form round-trips every field it shows, and saving it keeps the live
    /// transfer (set in the settings dialog, not on the form).
    #[test]
    fn a_form_round_trip_keeps_every_field_and_the_live_transfer() {
        let endpoint = RemoteEndpoint {
            connection: WorkerSettings {
                name: "Coordinator".to_string(),
                address: "coord.lan:7878".to_string(),
                cert_dir: "C:/certs/coord".to_string(),
                transfer_mode: TransferMode::FinalOnly,
                cadence_ms: 750,
                preview_scale: PreviewScale::Custom(40),
            },
            export_transfer: ExportTransfer::FinalPicture,
            live_transfer: LiveTransfer::FinalPicture,
        };
        let item = to_worker_item(&endpoint);
        assert_eq!(item.export_transfer_index, 1);
        assert_eq!(from_worker_item(&item, Some(&endpoint)), endpoint);
        // A brand-new endpoint starts with the default live transfer.
        assert_eq!(
            from_worker_item(&item, None).live_transfer,
            LiveTransfer::FullData
        );
    }
}
