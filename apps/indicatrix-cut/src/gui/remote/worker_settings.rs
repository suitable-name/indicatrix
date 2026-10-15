//! `WorkerItem` (the Slint-facing struct) <-> `settings::RemoteEndpoint` (the persisted
//! one) conversions, and pushing the configured endpoint to every UI surface that shows
//! or defaults from it ([`refresh_remote_ui`]).
//!
//! Split out of `gui::remote` purely to keep that module (already sizeable) from
//! growing further -- same reasoning as `gui::detail`/`gui::search`/`gui::remote`
//! itself.

use crate::{
    MainWindow, RemoteAvailabilityItem, RemoteWorkerModel, SettingsModel, WorkerItem,
    settings::{
        ExportTransfer, LiveTransfer, PreviewScale, RemoteEndpoint, WorkerSettings,
        model::RemoteAvailability,
    },
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

/// The merged live transfer of an endpoint, as the form and the Render & Output tab show it:
/// `0` progressive full data, `1` full data when finished, `2` finished pictures. The old
/// "Live stream" (`transfer_mode`) and "Live transfer" (`live_transfer`) settings are this one
/// number; the final picture wins because the live request then ignores `transfer_mode`.
const fn merged_live_transfer_index(endpoint: &RemoteEndpoint) -> i32 {
    match endpoint.live_transfer {
        LiveTransfer::FinalPicture => 2,
        LiveTransfer::FullData => transfer_mode_index(endpoint.connection.transfer_mode),
    }
}

const fn preview_scale_parts(scale: PreviewScale) -> (i32, i32) {
    match scale {
        PreviewScale::Full => (0, 50),
        PreviewScale::Half => (1, 50),
        PreviewScale::Quarter => (2, 50),
        PreviewScale::Custom(pct) => (3, pct.cast_signed()),
    }
}

fn preview_scale_from_parts(idx: i32, pct: i32) -> PreviewScale {
    match idx {
        1 => PreviewScale::Half,
        2 => PreviewScale::Quarter,
        3 => PreviewScale::Custom(pct.clamp(1, 100).cast_unsigned()),
        _ => PreviewScale::Full,
    }
}

pub(super) fn to_worker_item(endpoint: &RemoteEndpoint) -> WorkerItem {
    let w = &endpoint.connection;
    let (preview_scale_index, preview_scale_percent) = preview_scale_parts(w.preview_scale);
    WorkerItem {
        name: w.name.clone().into(),
        address: w.address.clone().into(),
        cert_dir: w.cert_dir.clone().into(),
        transfer_mode_index: merged_live_transfer_index(endpoint),
        cadence_ms: w.cadence_ms.cast_signed(),
        preview_scale_index,
        preview_scale_percent,
        export_transfer_index: endpoint.export_transfer.index(),
    }
}

/// The form's contents as an endpoint. The form's one live-transfer choice
/// (`transfer_mode_index`, see [`merged_live_transfer_index`]) sets both stored settings:
/// `2` is the final picture and keeps the endpoint's previous stream mode (it is ignored
/// while the final picture is chosen), `0` and `1` are full data in that stream mode.
pub(super) fn from_worker_item(
    item: &WorkerItem,
    previous: Option<&RemoteEndpoint>,
) -> RemoteEndpoint {
    let (transfer_mode, live_transfer) = if item.transfer_mode_index == 2 {
        (
            previous.map_or(TransferMode::LiveProgressive, |p| {
                p.connection.transfer_mode
            }),
            LiveTransfer::FinalPicture,
        )
    } else {
        (
            transfer_mode_from_index(item.transfer_mode_index),
            LiveTransfer::FullData,
        )
    };
    RemoteEndpoint {
        connection: WorkerSettings {
            name: item.name.to_string(),
            address: item.address.to_string(),
            cert_dir: item.cert_dir.to_string(),
            transfer_mode,
            cadence_ms: item.cadence_ms.max(1).cast_unsigned(),
            preview_scale: preview_scale_from_parts(
                item.preview_scale_index,
                item.preview_scale_percent,
            ),
        },
        export_transfer: ExportTransfer::from_index(item.export_transfer_index),
        live_transfer,
    }
}

/// Publishes the one remote-availability value (`RemoteWorkerModel.availability`) and the
/// reason text for surfaces that do not probe.
pub(in crate::gui) fn publish_availability(ui: &MainWindow, availability: RemoteAvailability) {
    let model = ui.global::<RemoteWorkerModel>();
    model.set_availability(RemoteAvailabilityItem {
        configured: availability.configured,
        reachable_render_capable: availability.reachable_render_capable,
    });
    model.set_availability_reason(
        availability
            .disabled_reason(false)
            .unwrap_or_default()
            .into(),
    );
}

/// The availability currently published.
#[must_use]
pub(in crate::gui) fn current_availability(ui: &MainWindow) -> RemoteAvailability {
    let item = ui.global::<RemoteWorkerModel>().get_availability();
    RemoteAvailability {
        configured: item.configured,
        reachable_render_capable: item.reachable_render_capable,
    }
}

/// Pushes the configured remote endpoint (or its absence) to the "Remote coordinator"
/// form (`RemoteWorkerModel.configured`/`endpoint`), the one remote-availability value and
/// the read-only live transfer shown in the Render & Output tab. Called after startup load
/// and after every save/remove. Reachability is unknown again afterwards (the still export
/// dialog probes every time it opens); the tilt video's "Transfer" default is re-followed by
/// the caller that has the settings (`output_defaults::follow_remote_default_for_tilt`).
pub fn refresh_remote_ui(ui: &MainWindow, remote: Option<&RemoteEndpoint>) {
    let model = ui.global::<RemoteWorkerModel>();
    model.set_configured(remote.is_some());
    model.set_endpoint(to_worker_item(&remote.cloned().unwrap_or_default()));
    publish_availability(
        ui,
        if remote.is_some() {
            RemoteAvailability::configured_only()
        } else {
            RemoteAvailability::NONE
        },
    );
    ui.global::<SettingsModel>()
        .set_live_transfer_index(remote.map_or(0, merged_live_transfer_index));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::LiveTransfer;

    /// The form round-trips every field it shows, and saving it keeps the live
    /// transfer (set in the Parameters panel, not on the form).
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
        // The final picture is the merged choice `2`.
        assert_eq!(item.transfer_mode_index, 2);
        assert_eq!(from_worker_item(&item, Some(&endpoint)), endpoint);
        // A brand-new endpoint keeps the final picture and starts the stream mode at its
        // default.
        let fresh = from_worker_item(&item, None);
        assert_eq!(fresh.live_transfer, LiveTransfer::FinalPicture);
        assert_eq!(
            fresh.connection.transfer_mode,
            TransferMode::LiveProgressive
        );
    }

    /// The two full-data choices keep the stream mode and the default live transfer.
    #[test]
    fn the_full_data_choices_set_the_stream_mode() {
        let mut endpoint = RemoteEndpoint::default();
        let mut item = to_worker_item(&endpoint);
        assert_eq!(item.transfer_mode_index, 0);
        item.transfer_mode_index = 1;
        endpoint = from_worker_item(&item, Some(&endpoint));
        assert_eq!(endpoint.connection.transfer_mode, TransferMode::FinalOnly);
        assert_eq!(endpoint.live_transfer, LiveTransfer::FullData);
        assert_eq!(to_worker_item(&endpoint).transfer_mode_index, 1);
    }
}
