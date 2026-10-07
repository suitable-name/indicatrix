//! What is stored for a design's lighting ([`LightingValues`]) and the pure helpers around it.

use crate::{
    bridge::render_thread::RenderContext,
    settings::{
        SettingsFile,
        model::{AppSettings, Backdrop, clamp_surface_glare},
        persist::DiskOverride,
    },
};
use indicatrix::optics::LightingPreset;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The stored format version. Raise it only when a field changes meaning; a new optional
/// field does not need it (an older build ignores keys it does not know).
pub(super) const FORMAT_VERSION: u32 = 1;

/// The light's elevation limits in radians: the ones the live light controls use.
pub(super) const PITCH_RANGE_RAD: (f32, f32) = (0.15, 1.55);

/// The exposure limits: the ones the live exposure slider uses.
pub(super) const EXPOSURE_RANGE: (f32, f32) = (0.2, 5.0);

const fn format_version() -> u32 {
    FORMAT_VERSION
}

const fn full_glare() -> f32 {
    1.0
}

// ---------------------------------------------------------------------------------------
// The stored values.
// ---------------------------------------------------------------------------------------

/// The lighting saved for a design, as stored in the database (JSON).
///
/// Every field a reader might lack has a default, so a row written by a build that knew
/// fewer settings still loads, and keys a newer build added are ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct LightingValues {
    /// The format this row was written in, see [`FORMAT_VERSION`].
    #[serde(default = "format_version")]
    pub(super) version: u32,
    /// The lighting rig's label, as `LightingPreset::label` writes it.
    pub(super) lighting_rig: String,
    /// Light yaw in degrees, `0..360`.
    pub(super) light_yaw_deg: f32,
    /// Light pitch (elevation) in degrees.
    pub(super) light_pitch_deg: f32,
    /// Exposure multiplier.
    pub(super) exposure: f32,
    /// Scale of the white mirror image of the light on the table, `0.0..=1.0`.
    #[serde(default = "full_glare")]
    pub(super) surface_glare: f32,
    /// What the camera sees behind the stone.
    #[serde(default)]
    pub(super) backdrop: Backdrop,
    /// The HDR environment map in use, if any. `None` means the studio rig.
    #[serde(default)]
    pub(super) env_map_path: Option<String>,
}

impl LightingValues {
    /// The app's normal lighting, as the settings file holds it.
    pub(super) fn from_app_settings(app: &AppSettings) -> Self {
        Self {
            version: FORMAT_VERSION,
            lighting_rig: app.lighting_rig.clone(),
            light_yaw_deg: app.light_yaw_deg,
            light_pitch_deg: app.light_pitch_deg,
            exposure: app.exposure,
            surface_glare: app.surface_glare,
            backdrop: app.backdrop,
            env_map_path: (!app.env_map_path.is_empty()).then(|| app.env_map_path.clone()),
        }
        .sanitised()
    }

    /// The lighting the live view shows right now. `env_map_path` is the HDR map in use,
    /// which the render context holds only as decoded pixels (see [`env_path_for_capture`]).
    pub(super) fn from_live(ctx: &RenderContext, env_map_path: Option<String>) -> Self {
        Self {
            version: FORMAT_VERSION,
            lighting_rig: ctx.lighting_preset.label().to_string(),
            light_yaw_deg: round2(ctx.light_yaw.to_degrees()),
            light_pitch_deg: round2(ctx.light_pitch.to_degrees()),
            exposure: round2(ctx.exposure),
            surface_glare: ctx.surface_glare,
            backdrop: ctx.backdrop,
            env_map_path,
        }
        .sanitised()
    }

    /// Reads a stored row, limiting every value the way the live controls do.
    pub(super) fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str::<Self>(text)
            .map(Self::sanitised)
            .map_err(|err| err.to_string())
    }

    /// The row as JSON text.
    pub(super) fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|err| err.to_string())
    }

    /// These values with each one inside the range the live controls allow, and anything
    /// that is not a number replaced by the app's default. A hand-edited or damaged row
    /// then still shows something sensible instead of feeding NaN to the tracer.
    pub(super) fn sanitised(mut self) -> Self {
        let defaults = AppSettings::default();
        self.light_yaw_deg =
            finite_or(self.light_yaw_deg, defaults.light_yaw_deg).rem_euclid(360.0);
        self.light_pitch_deg =
            clamp_pitch_deg(finite_or(self.light_pitch_deg, defaults.light_pitch_deg));
        self.exposure =
            finite_or(self.exposure, defaults.exposure).clamp(EXPOSURE_RANGE.0, EXPOSURE_RANGE.1);
        self.surface_glare = clamp_surface_glare(self.surface_glare);
        self.env_map_path = self.env_map_path.filter(|path| !path.trim().is_empty());
        self
    }

    /// Writes these values into the lighting fields of `app`.
    pub(super) fn write_into(&self, app: &mut AppSettings) {
        app.lighting_rig.clone_from(&self.lighting_rig);
        app.light_yaw_deg = self.light_yaw_deg;
        app.light_pitch_deg = self.light_pitch_deg;
        app.exposure = self.exposure;
        app.surface_glare = self.surface_glare;
        app.backdrop = self.backdrop;
        app.env_map_path = self.env_map_path.clone().unwrap_or_default();
    }

    /// The rewrite that keeps these values in the settings file whatever the light controls
    /// write while a design's own lighting is showing, see
    /// [`SettingsPersister::set_disk_override`].
    pub(super) fn disk_pin(&self) -> DiskOverride {
        let values = self.clone();
        Arc::new(move |file: &mut SettingsFile| values.write_into(&mut file.settings))
    }

    /// The lighting rig this row names, if this build has it.
    pub(super) fn rig(&self) -> Option<LightingPreset> {
        known_rig(&self.lighting_rig)
    }

    /// One plain sentence about the lighting, for the settings dialog.
    pub(super) fn summary(&self) -> String {
        let mut text = format!(
            "{}, light {:.0}° around and {:.0}° up, exposure {:.2}x",
            self.lighting_rig, self.light_yaw_deg, self.light_pitch_deg, self.exposure
        );
        if self.env_map_path.is_some() {
            text.push_str(", with an HDR environment map");
        }
        text.push('.');
        text
    }
}

const fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

fn round2(value: f32) -> f32 {
    (value * 100.0).round() / 100.0
}

/// `degrees` limited to the elevation range, left exactly as it is when it already fits.
const fn clamp_pitch_deg(degrees: f32) -> f32 {
    degrees.clamp(
        PITCH_RANGE_RAD.0.to_degrees(),
        PITCH_RANGE_RAD.1.to_degrees(),
    )
}

/// The lighting rig named `label`, or `None` when this build has no such rig.
///
/// `LightingPreset::from_label` falls back to the default (the light tent) for anything it
/// does not know, so on its own it cannot say a rig is gone. The light tent is accepted only
/// under its own label and the older one the parser migrates (`"Soft dome + ring lights"`).
pub(super) fn known_rig(label: &str) -> Option<LightingPreset> {
    let rig = LightingPreset::from_label(label);
    // A UV lamp is not a rig of a build without the `physical-color` feature.
    if rig.is_uv_lamp() && !crate::gui::optics::physics_state::PHYSICS_COLOR_UI {
        return None;
    }
    let is_tent_label = matches!(
        label,
        "Light tent + black cards" | "Soft dome + ring lights"
    );
    (rig != LightingPreset::LightTent || is_tent_label).then_some(rig)
}

/// The HDR map path to store for the live view: the one in the path field, but only while a
/// map is actually loaded (the field also holds text that was typed and never loaded).
pub(super) fn env_path_for_capture(map_loaded: bool, path_text: &str) -> Option<String> {
    let path = path_text.trim();
    (map_loaded && !path.is_empty()).then(|| path.to_string())
}

/// The HDR map path to store when the live lighting is saved for a design: the map being
/// decoded for that lighting (`pending`) when a decode is on its way, since the settings dialog
/// keeps showing the earlier map until it lands, and otherwise the map in use
/// ([`env_path_for_capture`]).
pub(super) fn env_path_to_store(
    pending: Option<&str>,
    map_loaded: bool,
    path_text: &str,
) -> Option<String> {
    pending
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .or_else(|| env_path_for_capture(map_loaded, path_text))
}

/// Where the HDR map a design's lighting names comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EnvSource {
    /// The lighting names no map: the studio rig.
    Studio,
    /// The lighting names the map that is loaded already: its decoded pixels stay as they are.
    AlreadyLoaded,
    /// The lighting names a map that has to be read from this file and decoded.
    Decode(String),
    /// The cutter loaded or cleared a map by hand after the opening began, while the saved
    /// lighting was still being read: whatever is loaded stays, whatever the lighting names.
    ChosenByHand,
}

/// Decides where the map `wanted` (the saved lighting's path, if any) comes from, given the
/// path of the map in use now (`loaded`, see [`env_path_for_capture`]).
///
/// Decoding a large `.hdr` takes long enough to freeze the window, so a map that is loaded
/// already is never decoded again. Paths are compared as typed in the settings dialog: the
/// same text, ignoring blanks at the ends.
pub(super) fn env_source(wanted: Option<&str>, loaded: Option<&str>) -> EnvSource {
    match (wanted.map(str::trim), loaded.map(str::trim)) {
        (None, _) => EnvSource::Studio,
        (Some(wanted), Some(loaded)) if wanted == loaded => EnvSource::AlreadyLoaded,
        (Some(wanted), _) => EnvSource::Decode(wanted.to_string()),
    }
}

/// [`env_source`], unless the cutter chose a map by hand since the opening began
/// (`chosen_since_open`, see `design_lighting::map_chosen_since_open`): the newest of the
/// cutter's own actions and the saved lighting's map is the one that stays, so the map the
/// lighting names (or its absence) must not replace what the cutter loaded or cleared while
/// the saved lighting was still being read from a busy library.
pub(super) fn env_source_for_opening(
    chosen_since_open: bool,
    wanted: Option<&str>,
    loaded: Option<&str>,
) -> EnvSource {
    if chosen_since_open {
        EnvSource::ChosenByHand
    } else {
        env_source(wanted, loaded)
    }
}

/// Whether the answer to an asked-for lookup or decode (`asked`: the opening it was started
/// for) still counts, given the number of the latest opening or reset (`latest`).
pub(super) const fn answer_is_current(asked: u64, latest: u64) -> bool {
    asked == latest
}
