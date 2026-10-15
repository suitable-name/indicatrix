//! [`LightingValues`]: the one struct that carries every scene/lighting field.
//!
//! The live state (`RenderContext`), a design's own lighting (the library's
//! `design_lighting` JSON), the named presets ([`super::LightingPreset`]) and the render-job
//! scene capture all convert to and from it, so no surface can silently lack a field the
//! others carry (before this struct, a design's lighting lacked the head shadow and a preset
//! lacked glare, backdrop and head shadow).
//!
//! No dependency on Slint, the render thread or the GUI: pure data plus pure logic. The
//! pieces that need the render context or the GUI (`from_live`, the rig lookup, the disk
//! pin) are further inherent `impl` blocks in
//! `gui::render::design_lighting::stored`.

use super::{
    app_settings::{AppSettings, Backdrop},
    head_shadow::{DEFAULT_HEAD_SHADOW_DEG, clamp_head_shadow_deg},
    surface_glare::{DEFAULT_SURFACE_GLARE, clamp_surface_glare},
};
use serde::{Deserialize, Serialize};

/// The stored format version. Raise it only when a field changes meaning; a new optional
/// field does not need it (an older build ignores keys it does not know).
pub const FORMAT_VERSION: u32 = 1;

/// The light's elevation limits in radians: the ones the live light controls use.
pub const PITCH_RANGE_RAD: (f32, f32) = (0.15, 1.55);

/// The exposure limits: the ones the live exposure slider uses.
pub const EXPOSURE_RANGE: (f32, f32) = (0.2, 5.0);

const fn format_version() -> u32 {
    FORMAT_VERSION
}

const fn full_glare() -> f32 {
    DEFAULT_SURFACE_GLARE
}

const fn default_head_shadow() -> f32 {
    DEFAULT_HEAD_SHADOW_DEG
}

/// Every scene/lighting field, as one value.
///
/// Every field a reader might lack has a serde default, so a row written by a build that
/// knew fewer settings (a design's stored lighting without a head shadow, say) still loads
/// with the current default for the missing field, and keys a newer build added are ignored.
/// The camera pose is optional and left out of the serialised form when absent, so the
/// design-lighting JSON is byte-compatible with the earlier format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightingValues {
    /// The format this row was written in, see [`FORMAT_VERSION`].
    #[serde(default = "format_version")]
    pub version: u32,
    /// The lighting rig's label, as `LightingPreset::label` writes it.
    pub lighting_rig: String,
    /// Light yaw in degrees, `0..360`.
    pub light_yaw_deg: f32,
    /// Light pitch (elevation) in degrees.
    pub light_pitch_deg: f32,
    /// Exposure multiplier.
    pub exposure: f32,
    /// Scale of the white mirror image of the light on the table, `0.0..=1.0`.
    #[serde(default = "full_glare")]
    pub surface_glare: f32,
    /// What the camera sees behind the stone.
    #[serde(default)]
    pub backdrop: Backdrop,
    /// Head-shadow radius in degrees, `0.0..=30.0` (`0.0` is off).
    #[serde(default = "default_head_shadow")]
    pub head_shadow_deg: f32,
    /// The HDR environment map in use, if any. `None` means the studio rig.
    #[serde(default)]
    pub env_map_path: Option<String>,
    /// Camera yaw in radians, if a pose is carried (always together with
    /// [`Self::camera_pitch`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_yaw: Option<f32>,
    /// Camera pitch in radians, if a pose is carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_pitch: Option<f32>,
    /// Camera distance from the stone, if carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_distance: Option<f32>,
}

impl Default for LightingValues {
    /// The app's built-in defaults (what [`AppSettings::default`] holds).
    fn default() -> Self {
        Self::from_app_settings(&AppSettings::default())
    }
}

impl LightingValues {
    /// The app's normal lighting, as the settings file holds it. No camera pose.
    #[must_use]
    pub fn from_app_settings(app: &AppSettings) -> Self {
        Self {
            version: FORMAT_VERSION,
            lighting_rig: app.lighting_rig.clone(),
            light_yaw_deg: app.light_yaw_deg,
            light_pitch_deg: app.light_pitch_deg,
            exposure: app.exposure,
            surface_glare: app.surface_glare,
            backdrop: app.backdrop,
            head_shadow_deg: app.head_shadow_deg,
            env_map_path: (!app.env_map_path.is_empty()).then(|| app.env_map_path.clone()),
            camera_yaw: None,
            camera_pitch: None,
            camera_distance: None,
        }
        .sanitised()
    }

    /// Reads a stored row, limiting every value the way the live controls do.
    ///
    /// # Errors
    ///
    /// The parser's message when `text` is not the stored format (the rig is required).
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str::<Self>(text)
            .map(Self::sanitised)
            .map_err(|err| err.to_string())
    }

    /// The row as JSON text.
    ///
    /// # Errors
    ///
    /// The serialiser's message (practically unreachable for this plain struct).
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|err| err.to_string())
    }

    /// These values with each one inside the range the live controls allow, and anything
    /// that is not a number replaced by the app's default. A hand-edited or damaged row
    /// then still shows something sensible instead of feeding NaN to the tracer.
    #[must_use]
    pub fn sanitised(mut self) -> Self {
        let defaults = AppSettings::default();
        self.light_yaw_deg =
            finite_or(self.light_yaw_deg, defaults.light_yaw_deg).rem_euclid(360.0);
        self.light_pitch_deg =
            clamp_pitch_deg(finite_or(self.light_pitch_deg, defaults.light_pitch_deg));
        self.exposure =
            finite_or(self.exposure, defaults.exposure).clamp(EXPOSURE_RANGE.0, EXPOSURE_RANGE.1);
        self.surface_glare = clamp_surface_glare(self.surface_glare);
        self.head_shadow_deg = clamp_head_shadow_deg(self.head_shadow_deg);
        self.env_map_path = self.env_map_path.filter(|path| !path.trim().is_empty());
        self.camera_yaw = self.camera_yaw.filter(|v| v.is_finite());
        self.camera_pitch = self.camera_pitch.filter(|v| v.is_finite());
        self.camera_distance = self.camera_distance.filter(|v| v.is_finite() && *v > 0.0);
        self
    }

    /// Writes these values into the lighting fields of `app`. The camera is not written:
    /// the persisted camera pose is the window's own state, not part of the lighting.
    pub fn write_into(&self, app: &mut AppSettings) {
        app.lighting_rig.clone_from(&self.lighting_rig);
        app.light_yaw_deg = self.light_yaw_deg;
        app.light_pitch_deg = self.light_pitch_deg;
        app.exposure = self.exposure;
        app.surface_glare = self.surface_glare;
        app.backdrop = self.backdrop;
        app.head_shadow_deg = self.head_shadow_deg;
        app.env_map_path = self.env_map_path.clone().unwrap_or_default();
    }
}

const fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

/// `degrees` limited to the elevation range, left exactly as it is when it already fits.
const fn clamp_pitch_deg(degrees: f32) -> f32 {
    degrees.clamp(
        PITCH_RANGE_RAD.0.to_degrees(),
        PITCH_RANGE_RAD.1.to_degrees(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row stored before the head shadow existed reads the current default for it.
    #[test]
    fn a_row_without_a_head_shadow_reads_the_default() {
        let json = r#"{"lighting_rig":"ISO hemisphere","light_yaw_deg":10.0,
                       "light_pitch_deg":30.0,"exposure":1.0}"#;
        let values = LightingValues::from_json(json).expect("loads");
        assert_eq!(
            values.head_shadow_deg.to_bits(),
            DEFAULT_HEAD_SHADOW_DEG.to_bits()
        );
        assert_eq!(values.camera_yaw, None);
    }

    /// The camera pose never reaches the stored JSON unless it is carried.
    #[test]
    fn an_absent_camera_pose_is_not_serialised() {
        let json = LightingValues::default().to_json().expect("serialises");
        assert!(!json.contains("camera"), "{json}");
        assert!(json.contains("head_shadow_deg"), "{json}");
    }

    /// The oldest stored rows carried only the rig, the light and the exposure (and perhaps a
    /// map): glare, backdrop and head shadow all read the current defaults.
    #[test]
    fn the_oldest_row_reads_every_newer_field_at_its_default() {
        let json = r#"{"lighting_rig":"Gem Studio Ring Lights","light_yaw_deg":48.0,
                       "light_pitch_deg":54.0,"exposure":1.0,
                       "env_map_path":"C:/env/studio.hdr"}"#;
        let values = LightingValues::from_json(json).expect("loads");
        assert_eq!(values.version, FORMAT_VERSION);
        assert_eq!(
            values.surface_glare.to_bits(),
            DEFAULT_SURFACE_GLARE.to_bits()
        );
        assert_eq!(values.backdrop, Backdrop::default());
        assert_eq!(
            values.head_shadow_deg.to_bits(),
            DEFAULT_HEAD_SHADOW_DEG.to_bits()
        );
        assert_eq!(values.env_map_path.as_deref(), Some("C:/env/studio.hdr"));
        assert_eq!((values.camera_yaw, values.camera_pitch), (None, None));
    }

    /// The row that is written today reads back as itself, and the keys a newer build might
    /// add are ignored rather than refused.
    #[test]
    fn a_row_round_trips_and_ignores_keys_it_does_not_know() {
        let values = LightingValues {
            surface_glare: 0.5,
            backdrop: Backdrop::White,
            head_shadow_deg: 8.0,
            camera_yaw: Some(0.25),
            camera_pitch: Some(0.75),
            camera_distance: Some(2.5),
            ..LightingValues::default()
        };
        let json = values.to_json().expect("serialises");
        assert_eq!(LightingValues::from_json(&json), Ok(values.clone()));
        let newer = json.replacen('{', r#"{"a_future_field":[1,2,3],"#, 1);
        assert_eq!(LightingValues::from_json(&newer), Ok(values));
    }

    #[test]
    fn the_head_shadow_is_limited_like_the_slider() {
        let wild = LightingValues {
            head_shadow_deg: 80.0,
            ..LightingValues::default()
        }
        .sanitised();
        assert_eq!(wild.head_shadow_deg.to_bits(), 30.0f32.to_bits());
    }
}
