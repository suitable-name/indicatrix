//! `AppSettings::head_shadow_deg`: the "Head shadow" radius of the lit lighting presets,
//! its default and the one rule every loaded or edited value passes through.

/// Default head shadow for new settings: `16.0` degrees, a head at arm's length.
pub const DEFAULT_HEAD_SHADOW_DEG: f32 = 16.0;

/// Upper end of the slider and of a loaded value, in degrees.
pub const MAX_HEAD_SHADOW_DEG: f32 = 30.0;

/// `value` limited to `0.0..=30.0` (`0.0` is off); NaN means the default `16.0`.
#[must_use]
pub const fn clamp_head_shadow_deg(value: f32) -> f32 {
    if value.is_nan() {
        DEFAULT_HEAD_SHADOW_DEG
    } else if value >= MAX_HEAD_SHADOW_DEG {
        MAX_HEAD_SHADOW_DEG
    } else if value > 0.0 {
        value
    } else {
        0.0
    }
}

/// The slider's value as a head-shadow radius: whole degrees, limited to `0.0..=30.0`.
#[must_use]
pub const fn head_shadow_deg_from_slider(value: f32) -> f32 {
    clamp_head_shadow_deg(value.round())
}

pub(super) const fn default_head_shadow_deg() -> f32 {
    DEFAULT_HEAD_SHADOW_DEG
}

/// Reads `AppSettings::head_shadow_deg` as a float and limits it, so a hand-edited value
/// outside `0.0..=30.0` loads as the nearest valid one instead of failing the file.
pub(super) fn deserialize_head_shadow_deg<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = <f64 as serde::Deserialize>::deserialize(deserializer)?;
    Ok(clamp_head_shadow_deg(raw as f32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::model::{AppSettings, SettingsFile};

    #[test]
    fn the_default_is_sixteen_degrees_and_a_file_without_the_key_loads_it() {
        assert_eq!(
            AppSettings::default().head_shadow_deg.to_bits(),
            16.0f32.to_bits()
        );
        let parsed: SettingsFile = toml::from_str("[settings]\nexposure = 1.2\n").expect("parse");
        assert_eq!(parsed.settings.head_shadow_deg.to_bits(), 16.0f32.to_bits());
    }

    #[test]
    fn clamping_limits_to_the_slider_range_and_maps_nan_to_the_default() {
        assert_eq!(clamp_head_shadow_deg(-3.0).to_bits(), 0.0f32.to_bits());
        assert_eq!(clamp_head_shadow_deg(0.0).to_bits(), 0.0f32.to_bits());
        assert_eq!(clamp_head_shadow_deg(12.5).to_bits(), 12.5f32.to_bits());
        assert_eq!(clamp_head_shadow_deg(30.0).to_bits(), 30.0f32.to_bits());
        assert_eq!(clamp_head_shadow_deg(80.0).to_bits(), 30.0f32.to_bits());
        assert_eq!(clamp_head_shadow_deg(f32::NAN).to_bits(), 16.0f32.to_bits());
    }

    #[test]
    fn the_slider_snaps_to_whole_degrees() {
        assert_eq!(
            head_shadow_deg_from_slider(15.6).to_bits(),
            16.0f32.to_bits()
        );
        assert_eq!(head_shadow_deg_from_slider(0.4).to_bits(), 0.0f32.to_bits());
        assert_eq!(
            head_shadow_deg_from_slider(44.0).to_bits(),
            30.0f32.to_bits()
        );
    }

    #[test]
    fn a_hand_edited_value_loads_clamped_and_round_trips() {
        for (written, expected) in [
            ("-5.0", 0.0f32),
            ("0.0", 0.0),
            ("22.0", 22.0),
            ("99.0", 30.0),
        ] {
            let text = format!("[settings]\nhead_shadow_deg = {written}\n");
            let parsed: SettingsFile = toml::from_str(&text).expect("parse");
            assert_eq!(
                parsed.settings.head_shadow_deg.to_bits(),
                expected.to_bits(),
                "{written}"
            );
        }
        let mut file = SettingsFile::default();
        file.settings.head_shadow_deg = 20.0;
        let text = toml::to_string(&file).expect("serialise");
        assert!(text.contains("head_shadow_deg = 20.0"), "{text}");
        let back: SettingsFile = toml::from_str(&text).expect("parse back");
        assert_eq!(back.settings.head_shadow_deg.to_bits(), 20.0f32.to_bits());
    }
}
