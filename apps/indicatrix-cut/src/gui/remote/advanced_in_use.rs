//! The "Some advanced settings are in use" note of the remote coordinator dialog.
//!
//! Simple mode leaves the stream tuning out of that dialog: the live-stream mode, the update
//! cadence, the preview scale, the default export transfer and the number of remote lanes a
//! batch keeps busy. Name, address, certificate folder, the connection test and the library
//! switch stay. A hidden control keeps its value and still applies, so while Simple mode is on
//! the dialog says so when any of them is off its default ("Switch to Advanced to see them"),
//! instead of leaving a remote slower or chattier for a reason nobody can see.
//!
//! The decision is [`HiddenRemoteSettings::in_use`], a pure comparison with the defaults a new
//! endpoint starts from. The dialog (`remote_worker_dialog.slint`) asks for it through the pure
//! callback `RemoteWorkerModel.advanced_in_use`, which [`setup_advanced_in_use_callback`]
//! answers; the tests keep the default values equal to the persisted ones
//! (`WorkerSettings::default()`, `AppSettings::default()`), so a changed default cannot leave the
//! note wrong.

use crate::{MainWindow, RemoteWorkerModel};
use slint::ComponentHandle;

/// The "Custom" preview scale, the only choice that carries a percentage.
const CUSTOM_PREVIEW_SCALE_INDEX: i32 = 3;

/// What the controls Simple mode hides hold, as the dialog sends them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HiddenRemoteSettings {
    /// Live stream: 0 progressive (the default), 1 final only.
    pub transfer_mode_index: i32,
    /// Milliseconds between updated pictures.
    pub cadence_ms: i32,
    /// Preview scale: 0 full, 1 half, 2 quarter (the default), 3 custom.
    pub preview_scale_index: i32,
    /// The custom preview scale in percent; only meaningful when the index is 3.
    pub preview_scale_percent: i32,
    /// Default export transfer: 0 full data (the default), 1 final picture only.
    pub export_transfer_index: i32,
    /// How many pictures a catalogue batch keeps in flight on the remote.
    pub batch_lanes: i32,
}

impl HiddenRemoteSettings {
    /// The values a new endpoint starts from.
    pub const DEFAULT: Self = Self {
        transfer_mode_index: 0,
        cadence_ms: 500,
        preview_scale_index: 2,
        preview_scale_percent: 50,
        export_transfer_index: 0,
        batch_lanes: 4,
    };

    /// Whether any hidden setting is off its default. The custom percentage only counts while
    /// the custom scale is the chosen one: a stored 50 or 30 behind "Quarter" does nothing.
    #[must_use]
    pub const fn in_use(&self) -> bool {
        let custom_percent_differs = self.preview_scale_index == CUSTOM_PREVIEW_SCALE_INDEX
            && self.preview_scale_percent != Self::DEFAULT.preview_scale_percent;
        self.transfer_mode_index != Self::DEFAULT.transfer_mode_index
            || self.cadence_ms != Self::DEFAULT.cadence_ms
            || self.preview_scale_index != Self::DEFAULT.preview_scale_index
            || custom_percent_differs
            || self.export_transfer_index != Self::DEFAULT.export_transfer_index
            || self.batch_lanes != Self::DEFAULT.batch_lanes
    }
}

/// Answers `RemoteWorkerModel.advanced_in_use`, the pure callback behind the dialog's note.
pub(super) fn setup_advanced_in_use_callback(ui: &MainWindow) {
    ui.global::<RemoteWorkerModel>().on_advanced_in_use(
        |transfer_mode_index,
         cadence_ms,
         preview_scale_index,
         preview_scale_percent,
         export_transfer_index,
         batch_lanes| {
            HiddenRemoteSettings {
                transfer_mode_index,
                cadence_ms,
                preview_scale_index,
                preview_scale_percent,
                export_transfer_index,
                batch_lanes,
            }
            .in_use()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        gui::remote::worker_settings::to_worker_item,
        settings::{RemoteEndpoint, model::AppSettings},
    };

    const DEFAULT: HiddenRemoteSettings = HiddenRemoteSettings::DEFAULT;

    #[test]
    fn the_default_endpoint_is_not_advanced() {
        assert!(!DEFAULT.in_use());
    }

    #[test]
    fn every_hidden_setting_off_its_default_counts_as_in_use() {
        let changed = [
            HiddenRemoteSettings {
                transfer_mode_index: 1,
                ..DEFAULT
            },
            HiddenRemoteSettings {
                cadence_ms: 250,
                ..DEFAULT
            },
            HiddenRemoteSettings {
                preview_scale_index: 0,
                ..DEFAULT
            },
            HiddenRemoteSettings {
                preview_scale_index: 3,
                preview_scale_percent: 40,
                ..DEFAULT
            },
            HiddenRemoteSettings {
                export_transfer_index: 1,
                ..DEFAULT
            },
            HiddenRemoteSettings {
                batch_lanes: 8,
                ..DEFAULT
            },
        ];
        for settings in changed {
            assert!(settings.in_use(), "{settings:?}");
        }
    }

    #[test]
    fn a_custom_percentage_behind_another_scale_does_nothing() {
        let settings = HiddenRemoteSettings {
            preview_scale_percent: 40,
            ..DEFAULT
        };
        assert!(!settings.in_use());
    }

    /// The numbers above are the form's and the settings file's, copied: they must be what a
    /// new endpoint holds, or the note would show on every open (or never).
    #[test]
    fn the_default_values_are_the_persisted_defaults() {
        let item = to_worker_item(&RemoteEndpoint::default());
        assert_eq!(item.transfer_mode_index, DEFAULT.transfer_mode_index);
        assert_eq!(item.cadence_ms, DEFAULT.cadence_ms);
        assert_eq!(item.preview_scale_index, DEFAULT.preview_scale_index);
        assert_eq!(item.preview_scale_percent, DEFAULT.preview_scale_percent);
        assert_eq!(item.export_transfer_index, DEFAULT.export_transfer_index);
        assert_eq!(
            i64::from(AppSettings::default().remote_batch_lanes),
            i64::from(DEFAULT.batch_lanes)
        );
    }
}
