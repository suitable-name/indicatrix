//! Persisted deterministic-view quality. Selecting an engine remains a view choice.
use indicatrix::renderer::engine::RayOptions;
use serde::{Deserialize, Serialize};

/// Anti-aliasing and dispersion settings for the opt-in Ray-Traced live view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RaySettings {
    /// Fixed centre-first taps: 1, 4, 8 or 16.
    pub live_taps: u8,
    /// Separate RGB chains using the material dispersion.
    pub fire: bool,
}
impl Default for RaySettings {
    fn default() -> Self {
        Self {
            live_taps: 8,
            fire: true,
        }
    }
}
impl RaySettings {
    /// The shared validated renderer contract, including hand-edited settings.
    #[must_use]
    pub const fn options(self) -> RayOptions {
        RayOptions {
            aa_taps: self.live_taps,
            fire: self.fire,
        }
        .sanitized()
    }
}

#[cfg(test)]
mod tests {
    use crate::settings::model::SettingsFile;
    #[test]
    fn old_settings_default_ray_quality_and_custom_values_round_trip() {
        let mut file: SettingsFile =
            toml::from_str("[settings]\nexposure = 1.2\n").expect("old settings");
        assert_eq!(file.settings.ray_trace, super::RaySettings::default());
        file.settings.ray_trace = super::RaySettings {
            live_taps: 16,
            fire: false,
        };
        let text = toml::to_string(&file).expect("write settings");
        let parsed: SettingsFile = toml::from_str(&text).expect("read settings");
        assert_eq!(parsed.settings.ray_trace, file.settings.ray_trace);
    }
}
