//! [`LightingPreset`]: a named, user-saveable snapshot of a full viewport "view" --
//! lighting rig plus camera pose -- plus the built-in presets shipped with the app.

use serde::{Deserialize, Serialize};

/// A named, user-saveable snapshot of the viewport's "view": the lighting rig, the HDR
/// environment map (if any), and optionally the camera pose.
///
/// # Camera pose is part of the view, deliberately
///
/// A preset captures the full view -- lighting rig AND camera pose -- so it can be
/// recalled as a complete, reproducible shot, not just a lighting mood.
/// [`camera_yaw`]/[`camera_pitch`] below hold that pose, recorded here so a future
/// reader doesn't "fix" it back to lighting-only on the assumption that switching
/// moods shouldn't reposition the camera.
///
/// # Why the camera fields are `Option`, not plain `f32`
///
/// Data compatibility: a preset saved before these fields existed has no camera data,
/// and defaulting to `0.0` would fling the stone to yaw=0/pitch=0 on next apply. `None`
/// means "no pose recorded" (predates the fields, or is a built-in); `Some` means a
/// real pose captured by the "Save as preset" viewport button.
/// `gui::lighting_presets::setup_apply_lighting_preset_callback` restores the camera
/// only when both are `Some`, otherwise leaves the current pose untouched. Radians,
/// matching `RenderContext::yaw`/`pitch`.
///
/// # `env_map_path`: same `None`-means-"leave it alone" treatment
///
/// `Some(path)` means a map was loaded when this preset was saved; applying it loads
/// that map. `None` covers both "predates this field" and "no map was active" -- either
/// way, applying the preset leaves whatever environment is currently loaded untouched
/// rather than resetting to the studio rig. A preset can't explicitly say "clear the
/// HDR map" -- clobbering a user's deliberately-loaded map would be the worse surprise.
///
/// # `export_usable`
///
/// Marks this preset as offered in the export dialog's preset fan-out list. Off by
/// default so an existing preset doesn't silently start appearing in every export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightingPreset {
    pub name: String,
    /// Built-in presets ship with the app and cannot be renamed or deleted -- see
    /// `SettingsFile::rename_preset` / `delete_preset`, which both refuse to act on one.
    #[serde(default)]
    pub built_in: bool,
    pub light_yaw_deg: f32,
    pub light_pitch_deg: f32,
    pub exposure: f32,
    pub lighting_rig: String,
    pub camera_distance: f32,
    /// Camera yaw, in radians -- see this type's doc comment for why `Option`.
    #[serde(default)]
    pub camera_yaw: Option<f32>,
    /// Camera pitch, in radians -- always written/read alongside `camera_yaw` as a
    /// pair (both `Some` or both `None`).
    #[serde(default)]
    pub camera_pitch: Option<f32>,
    /// Loaded HDR environment map path, if one was active when saved -- see this
    /// type's doc comment for the `None`-means-"leave it alone" semantics.
    #[serde(default)]
    pub env_map_path: Option<String>,
    /// Whether this preset is offered in the export dialog's preset fan-out list.
    #[serde(default)]
    pub export_usable: bool,
}

/// The 2-3 built-in presets shipped with the app, kept to a small set that cannot be
/// deleted. Names double as their stable identity -- `SettingsFile::ensure_built_in_presets`
/// matches on `name` to decide whether a loaded file already has one.
///
/// `camera_yaw`/`camera_pitch`/`env_map_path` are all `None`, and `export_usable` is
/// `false`, for every built-in: these are lighting moods, not shots of any particular
/// design, so applying one must only ever change the lighting, never reposition the
/// camera or swap in an environment map. They also default out of the export fan-out
/// list.
#[must_use]
pub fn built_in_presets() -> Vec<LightingPreset> {
    vec![
        LightingPreset {
            name: "Studio Softbox".to_string(),
            built_in: true,
            light_yaw_deg: 48.0,
            light_pitch_deg: 54.0,
            exposure: 1.0,
            lighting_rig: "Gem Studio Ring Lights".to_string(),
            camera_distance: 2.4,
            camera_yaw: None,
            camera_pitch: None,
            env_map_path: None,
            export_usable: false,
        },
        LightingPreset {
            name: "Daylight Bright".to_string(),
            built_in: true,
            light_yaw_deg: 30.0,
            light_pitch_deg: 65.0,
            exposure: 1.3,
            // D65 is 6500K, not 5500K -- `LightingPreset::from_label` parses both labels
            // identically, for compatibility with presets saved under the mislabelled string.
            lighting_rig: "D65 Daylight (6500K)".to_string(),
            camera_distance: 2.2,
            camera_yaw: None,
            camera_pitch: None,
            env_map_path: None,
            export_usable: false,
        },
        LightingPreset {
            name: "Dramatic Spotlight".to_string(),
            built_in: true,
            light_yaw_deg: 300.0,
            light_pitch_deg: 25.0,
            exposure: 0.7,
            lighting_rig: "Dramatic Dark Spotlight".to_string(),
            camera_distance: 2.6,
            camera_yaw: None,
            camera_pitch: None,
            env_map_path: None,
            export_usable: false,
        },
        LightingPreset {
            name: "Light Tent".to_string(),
            built_in: true,
            light_yaw_deg: 48.0,
            light_pitch_deg: 54.0,
            exposure: 1.0,
            lighting_rig: "Light tent + black cards".to_string(),
            camera_distance: 2.4,
            camera_yaw: None,
            camera_pitch: None,
            env_map_path: None,
            export_usable: false,
        },
        LightingPreset {
            name: "Daylight Sun".to_string(),
            built_in: true,
            light_yaw_deg: 30.0,
            light_pitch_deg: 55.0,
            exposure: 1.0,
            lighting_rig: "Daylight sky + sun".to_string(),
            camera_distance: 2.4,
            camera_yaw: None,
            camera_pitch: None,
            env_map_path: None,
            export_usable: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A settings file serialized before `camera_yaw`/`camera_pitch`/`env_map_path`/
    /// `export_usable` existed must still deserialize, with every new field defaulted.
    #[test]
    fn a_preset_predating_the_view_fields_still_loads_with_them_defaulted() {
        let old_toml = r#"
            name = "Old Preset"
            built_in = false
            light_yaw_deg = 10.0
            light_pitch_deg = 20.0
            exposure = 1.0
            lighting_rig = "Gem Studio Ring Lights"
            camera_distance = 2.4
        "#;
        let preset: LightingPreset =
            toml::from_str(old_toml).expect("a pre-view-fields preset must still parse");
        assert_eq!(preset.camera_yaw, None);
        assert_eq!(preset.camera_pitch, None);
        assert_eq!(preset.env_map_path, None);
        assert!(!preset.export_usable);
    }

    /// A preset saved by the new "Save as preset" viewport button round-trips its full
    /// captured view -- camera pose and HDR map included -- through serialization.
    #[test]
    fn a_full_view_preset_round_trips_through_toml() {
        let preset = LightingPreset {
            name: "My Shot".to_string(),
            built_in: false,
            light_yaw_deg: 48.0,
            light_pitch_deg: 54.0,
            exposure: 1.0,
            lighting_rig: "Gem Studio Ring Lights".to_string(),
            camera_distance: 2.4,
            camera_yaw: Some(0.6),
            camera_pitch: Some(0.45),
            env_map_path: Some("C:/env/studio.hdr".to_string()),
            export_usable: true,
        };
        let toml_str = toml::to_string_pretty(&preset).expect("must serialize");
        let round_tripped: LightingPreset = toml::from_str(&toml_str).expect("must deserialize");
        assert_eq!(preset, round_tripped);
    }

    /// Every built-in ships with no camera/env data and is not export-usable by default.
    #[test]
    fn built_in_presets_carry_no_camera_or_env_data_and_are_not_export_usable() {
        for preset in built_in_presets() {
            assert_eq!(
                preset.camera_yaw, None,
                "{}: built-ins must not carry a camera pose",
                preset.name
            );
            assert_eq!(
                preset.camera_pitch, None,
                "{}: built-ins must not carry a camera pose",
                preset.name
            );
            assert_eq!(
                preset.env_map_path, None,
                "{}: built-ins must not carry an HDR map",
                preset.name
            );
            assert!(
                !preset.export_usable,
                "{}: built-ins must default out of the export fan-out list",
                preset.name
            );
        }
    }
}
