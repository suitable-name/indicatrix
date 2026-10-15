//! Publishes the Rust-owned constants of the Render & Output tab to Slint: the one bounce
//! ladder and the sample / thumbnail ranges (`SettingsModel.bounce_ladder`,
//! `*_samples_*_exponent`, `preview_*_min/max`) and the Lighting tab's exposure and light
//! elevation ranges. Slint keeps no literal copies once its pickers read these.

use crate::{
    MainWindow, SettingsModel,
    settings::model::{
        BOUNCE_LADDER, EXPORT_SAMPLES_EXPONENT_RANGE, EXPOSURE_RANGE, LIVE_SAMPLES_EXPONENT_RANGE,
        PITCH_RANGE_RAD, PREVIEW_SAMPLES_RANGE, PREVIEW_SIZE_RANGE,
    },
};
use slint::{ComponentHandle, ModelRc, VecModel};

fn as_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// Pushes the constants into `SettingsModel`. Called once, when the window is built.
pub(in crate::gui) fn publish_parameter_constants(ui: &MainWindow) {
    let settings = ui.global::<SettingsModel>();
    let ladder: Vec<i32> = BOUNCE_LADDER.iter().map(|&rung| as_i32(rung)).collect();
    settings.set_bounce_ladder(ModelRc::new(VecModel::from(ladder)));
    settings.set_live_samples_min_exponent(as_i32(LIVE_SAMPLES_EXPONENT_RANGE.0));
    settings.set_live_samples_max_exponent(as_i32(LIVE_SAMPLES_EXPONENT_RANGE.1));
    settings.set_export_samples_min_exponent(as_i32(EXPORT_SAMPLES_EXPONENT_RANGE.0));
    settings.set_export_samples_max_exponent(as_i32(EXPORT_SAMPLES_EXPONENT_RANGE.1));
    settings.set_preview_samples_min(as_i32(PREVIEW_SAMPLES_RANGE.0));
    settings.set_preview_samples_max(as_i32(PREVIEW_SAMPLES_RANGE.1));
    settings.set_preview_size_min(as_i32(PREVIEW_SIZE_RANGE.0));
    settings.set_preview_size_max(as_i32(PREVIEW_SIZE_RANGE.1));
    settings.set_exposure_min(EXPOSURE_RANGE.0);
    settings.set_exposure_max(EXPOSURE_RANGE.1);
    settings.set_light_elevation_min_deg(PITCH_RANGE_RAD.0.to_degrees());
    settings.set_light_elevation_max_deg(PITCH_RANGE_RAD.1.to_degrees());
}

#[cfg(test)]
mod tests {
    use crate::settings::model::BOUNCE_LADDER;

    const SETTINGS_SLINT: &str = include_str!("../../../ui/models/settings.slint");

    /// Slint's placeholder ladder (what the pickers show before `publish_parameter_constants`
    /// runs, and what the live preview of the .slint files uses) is the Rust ladder.
    #[test]
    fn the_slint_placeholder_ladder_is_the_rust_ladder() {
        let line = SETTINGS_SLINT
            .lines()
            .find(|line| line.contains("property <[int]> bounce_ladder"))
            .expect("settings.slint declares the ladder");
        let list = line
            .split_once("bounce_ladder:")
            .and_then(|(_, rest)| rest.split_once('['))
            .and_then(|(_, rest)| rest.split_once(']'))
            .map(|(list, _)| list)
            .expect("the placeholder list is written out");
        let slint: Vec<u32> = list
            .split(',')
            .map(|rung| rung.trim().parse().expect("an integer rung"))
            .collect();
        assert_eq!(slint, BOUNCE_LADDER);
    }

    /// Every bounce picker reads `SettingsModel.bounce_ladder` and none keeps a literal copy.
    #[test]
    fn every_picker_reads_the_published_ladder() {
        let pickers = [
            (
                "bounces_picker",
                include_str!("../../../ui/components/settings_dialog/bounces_picker.slint"),
            ),
            (
                "export_dialog/compute_and_bounces",
                include_str!("../../../ui/components/export_dialog/compute_and_bounces.slint"),
            ),
            (
                "tilt_video_export_section",
                include_str!(
                    "../../../ui/components/performance_graph/tilt_video_export_section.slint"
                ),
            ),
        ];
        for (name, source) in pickers {
            assert!(
                source.contains("SettingsModel.bounce_ladder"),
                "{name} must read the published ladder"
            );
            let literal = source
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .any(|line| line.contains("64, 128") || line.contains("64,128"));
            assert!(!literal, "{name} keeps its own copy of the ladder");
        }
    }
}
