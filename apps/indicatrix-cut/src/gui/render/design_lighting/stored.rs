//! What is stored for a design's lighting and the pure helpers around it.
//!
//! The stored value is the shared [`LightingValues`] (`settings::model::lighting_values`),
//! the one struct every lighting surface uses. The parts of it that need the render
//! context or the GUI (reading the live view, the rig lookup, the settings-file pin, the
//! summary sentence) are inherent `impl` blocks here.

use crate::{
    bridge::render_thread::RenderContext,
    settings::{SettingsFile, persist::DiskOverride},
};
use indicatrix::optics::LightingPreset;
use std::sync::Arc;

#[cfg(test)]
pub(super) use crate::settings::model::EXPOSURE_RANGE;
pub(super) use crate::settings::model::{FORMAT_VERSION, LightingValues, PITCH_RANGE_RAD};

// ---------------------------------------------------------------------------------------
// The stored values.
// ---------------------------------------------------------------------------------------

impl LightingValues {
    /// The lighting the live view shows right now. `env_map_path` is the HDR map in use,
    /// which the render context holds only as decoded pixels (see [`env_path_for_capture`]).
    /// The camera pose is not part of a design's lighting.
    pub(super) fn from_live(ctx: &RenderContext, env_map_path: Option<String>) -> Self {
        Self {
            version: FORMAT_VERSION,
            lighting_rig: ctx.lighting_preset.label().to_string(),
            light_yaw_deg: round2(ctx.light_yaw.to_degrees()),
            light_pitch_deg: round2(ctx.light_pitch.to_degrees()),
            exposure: round2(ctx.exposure),
            surface_glare: ctx.surface_glare,
            backdrop: ctx.backdrop,
            head_shadow_deg: ctx.head_shadow_deg,
            env_map_path,
            camera_yaw: None,
            camera_pitch: None,
            camera_distance: None,
        }
        .sanitised()
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

    /// One plain sentence about the lighting, for the Parameters panel.
    pub(super) fn summary(&self) -> String {
        let mut text = format!(
            "{}, light {:.0}° around and {:.0}° up, exposure {:.2}x",
            self.lighting_rig, self.light_yaw_deg, self.light_pitch_deg, self.exposure
        );
        if self.env_map_path.is_some() {
            text.push_str(", with an HDR environment map");
        } else {
            // An HDR map ignores both; the built-in rigs use them.
            text = format!(
                "{text}, glare {:.0} %, head shadow {:.0}°",
                self.surface_glare * 100.0,
                self.head_shadow_deg
            );
        }
        text.push('.');
        text
    }
}

fn round2(value: f32) -> f32 {
    (value * 100.0).round() / 100.0
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
/// decoded for that lighting (`pending`) when a decode is on its way, since the Parameters panel
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
/// already is never decoded again. Paths are compared as typed in the Parameters panel: the
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
