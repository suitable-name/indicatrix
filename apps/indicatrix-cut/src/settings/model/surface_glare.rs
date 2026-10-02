//! `AppSettings::surface_glare`: the viewport's "Surface glare" scale, its default and
//! the one rule every loaded or edited value passes through.

/// Default surface glare for new settings: `1.0`, the unscaled render.
pub const DEFAULT_SURFACE_GLARE: f32 = 1.0;

/// `value` limited to `0.0..=1.0`; NaN means the default `1.0`.
#[must_use]
pub const fn clamp_surface_glare(value: f32) -> f32 {
    if value.is_nan() || value >= 1.0 {
        DEFAULT_SURFACE_GLARE
    } else if value > 0.0 {
        value
    } else {
        0.0
    }
}

/// The slider's percent (`0..=100`) as a glare fraction, snapped to steps of 5 % and
/// limited to `0.0..=1.0` (NaN means `1.0`).
#[must_use]
pub fn surface_glare_from_percent(percent: f32) -> f32 {
    clamp_surface_glare((percent / 5.0).round() * 5.0 / 100.0)
}

/// A glare fraction as the slider's percent (`0.0..=100.0`).
#[must_use]
pub fn percent_from_surface_glare(glare: f32) -> f32 {
    clamp_surface_glare(glare) * 100.0
}

pub(super) const fn default_surface_glare() -> f32 {
    DEFAULT_SURFACE_GLARE
}

/// Reads `AppSettings::surface_glare` as a float and limits it, so a hand-edited value
/// outside `0.0..=1.0` loads as the nearest valid one instead of failing the file.
pub(super) fn deserialize_surface_glare<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = <f64 as serde::Deserialize>::deserialize(deserializer)?;
    Ok(clamp_surface_glare(raw as f32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::model::{AppSettings, SettingsFile};

    #[test]
    fn the_default_is_unscaled_and_a_file_without_the_key_loads_it() {
        assert_eq!(
            AppSettings::default().surface_glare.to_bits(),
            1.0f32.to_bits()
        );
        let parsed: SettingsFile = toml::from_str("[settings]\nexposure = 1.2\n").expect("parse");
        assert_eq!(parsed.settings.surface_glare.to_bits(), 1.0f32.to_bits());
    }

    #[test]
    fn the_limit_is_zero_through_one_and_nan_is_the_default() {
        assert_eq!(clamp_surface_glare(-2.0).to_bits(), 0.0f32.to_bits());
        assert_eq!(clamp_surface_glare(0.0).to_bits(), 0.0f32.to_bits());
        assert_eq!(clamp_surface_glare(0.35).to_bits(), 0.35f32.to_bits());
        assert_eq!(clamp_surface_glare(1.0).to_bits(), 1.0f32.to_bits());
        assert_eq!(clamp_surface_glare(4.0).to_bits(), 1.0f32.to_bits());
        assert_eq!(clamp_surface_glare(f32::NAN).to_bits(), 1.0f32.to_bits());
    }

    #[test]
    fn the_slider_percent_snaps_to_five_and_maps_to_a_fraction() {
        assert_eq!(
            surface_glare_from_percent(100.0).to_bits(),
            1.0f32.to_bits()
        );
        assert_eq!(surface_glare_from_percent(0.0).to_bits(), 0.0f32.to_bits());
        assert_eq!(surface_glare_from_percent(52.0).to_bits(), 0.5f32.to_bits());
        assert_eq!(surface_glare_from_percent(-9.0).to_bits(), 0.0f32.to_bits());
        assert_eq!(
            surface_glare_from_percent(180.0).to_bits(),
            1.0f32.to_bits()
        );
        assert_eq!(
            surface_glare_from_percent(f32::NAN).to_bits(),
            1.0f32.to_bits()
        );
        assert_eq!(percent_from_surface_glare(0.5).to_bits(), 50.0f32.to_bits());
        assert_eq!(
            percent_from_surface_glare(2.0).to_bits(),
            100.0f32.to_bits()
        );
    }

    #[test]
    fn an_out_of_range_value_in_the_file_is_limited_on_load() {
        for (written, expected) in [("-0.5", 0.0f32), ("0.25", 0.25), ("1", 1.0), ("7.5", 1.0)] {
            let text = format!("[settings]\nsurface_glare = {written}\n");
            let parsed: SettingsFile = toml::from_str(&text).expect("a number always parses");
            assert_eq!(
                parsed.settings.surface_glare.to_bits(),
                expected.to_bits(),
                "wrote {written}"
            );
        }
    }

    #[test]
    fn a_dialled_in_value_round_trips_through_the_settings_file() {
        let mut file = SettingsFile::default();
        file.settings.surface_glare = 0.4;
        let text = toml::to_string(&file).expect("serialize");
        assert!(text.contains("surface_glare = 0.4"), "{text}");
        let back: SettingsFile = toml::from_str(&text).expect("parse");
        assert_eq!(back.settings.surface_glare.to_bits(), 0.4f32.to_bits());
    }
}
