//! The Light tab: the lighting rigs the phone offers, the light values, and the scene
//! settings the renderer reads.
//!
//! The rig, exposure, light direction, backdrop and head shadow live in
//! `MobileSettings::lighting`, so they persist across restarts, as does the surface glare.
//! The two UV lamps are not offered: they are CPU-only and belong to the `physical-color`
//! build, which the phone does not ship.

use indicatrix::optics::raytracer::{LightingModel, LightingPreset};
use indicatrix_web_core::settings::{EXPOSURE_RANGE, HEAD_SHADOW_RANGE, RenderSettings};

use super::{DEFAULT_SURFACE_GLARE_PCT, narrow};
use crate::{
    app_state::AppState,
    hdr::HdrView,
    settings::{Backdrop, LightingSettings},
};

/// The light-elevation slider's range in degrees (inside the renderer's own clamp, which
/// is 8.6 to 88.8 degrees).
pub const ELEVATION_RANGE_DEG: (f32, f32) = (9.0, 88.0);

/// The surface glare moves in steps of this many percent.
const GLARE_STEP_PCT: f32 = 5.0;

/// One lighting rig of the Light tab's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightingRow {
    /// The stable name the settings store (the renderer's own label of the rig).
    pub name: String,
    /// The Title Case label the list shows.
    pub title: String,
    /// One sentence about the rig.
    pub description: String,
}

/// The Title Case label of `preset`.
const fn title_of(preset: LightingPreset) -> &'static str {
    match preset {
        LightingPreset::LightTent => "Light Tent + Black Cards",
        LightingPreset::IsoHemisphere => "Grading Tray (D65 Hemisphere)",
        LightingPreset::WhiteTray => "White Tray (Lit from Below)",
        LightingPreset::ShopLights => "Jewelry Shop (Diffuse + Spots)",
        LightingPreset::WindowDaylight => "Window Daylight",
        LightingPreset::DaylightDome => "Daylight Sky (No Sun)",
        LightingPreset::DaylightSun => "Daylight Sky + Direct Sun",
        LightingPreset::Daylight => "D65 Daylight (6500K)",
        LightingPreset::Incandescent => "Incandescent (3200K)",
        LightingPreset::IlluminantA => "Incandescent A (2856K)",
        LightingPreset::RingLights => "Gem Studio Ring Lights",
        LightingPreset::DarkSpotlight => "Dramatic Dark Spotlight",
        LightingPreset::Aset => "Contrast View (ASET-Style)",
        LightingPreset::UvLamp365 => "UV Lamp 365 nm",
        LightingPreset::UvLamp395 => "UV Lamp 395 nm",
    }
}

/// One sentence about `preset`, from the desktop's lighting help.
const fn description_of(preset: LightingPreset) -> &'static str {
    match preset {
        LightingPreset::LightTent => {
            "A white tent with three black cards: the grader's look, and the default."
        }
        LightingPreset::IsoHemisphere => {
            "A uniform dome with the viewer's head shadow, as in a grading tray."
        }
        LightingPreset::WhiteTray => {
            "Bright uniform light from below, so a window shows white as on paper."
        }
        LightingPreset::ShopLights => {
            "Bright diffuse walls and pinpoint spots, like a jewelry shop window."
        }
        LightingPreset::WindowDaylight => {
            "One broad soft source in a dim room. Set the elevation to 20 to 40 degrees."
        }
        LightingPreset::DaylightDome => "A clear sky dome without the sun.",
        LightingPreset::DaylightSun => "A clear sky with a small, physically bright sun.",
        LightingPreset::Daylight => "A single D65 studio light, as in product photography.",
        LightingPreset::Incandescent => "A warm 3200 K studio light.",
        LightingPreset::IlluminantA => {
            "The 2856 K standard illuminant used for judging color change."
        }
        LightingPreset::RingLights => "Soft ring lights around the camera.",
        LightingPreset::DarkSpotlight => "One hard spotlight in a dark room.",
        LightingPreset::Aset => {
            "Colors the light by the angle it leaves the stone: red 45 to 75 degrees, green 0 to \
             45 degrees, blue 75 to 90 degrees."
        }
        LightingPreset::UvLamp365 => "A 365 nm UV lamp (lights fluorescent materials).",
        LightingPreset::UvLamp395 => "A 395 nm UV lamp (lights fluorescent materials).",
    }
}

/// The lighting rigs the phone offers, in the renderer's list order with the default
/// (the light tent) first; the UV lamps are left out.
#[must_use]
pub fn lighting_rows() -> Vec<LightingRow> {
    LightingPreset::ALL
        .iter()
        .filter(|preset| !preset.is_uv_lamp())
        .map(|&preset| LightingRow {
            name: preset.label().to_owned(),
            title: title_of(preset).to_owned(),
            description: description_of(preset).to_owned(),
        })
        .collect()
}

/// The rig the settings name; a stored UV lamp or an unknown label reads as the default.
fn preset_of(lighting: &LightingSettings) -> LightingPreset {
    let preset = LightingPreset::from_label(&lighting.preset_name);
    if preset.is_uv_lamp() {
        LightingPreset::default()
    } else {
        preset
    }
}

/// Chooses the lighting rig called `name` (one of [`lighting_rows`]'s names).
///
/// # Errors
///
/// A sentence when `name` is not an offered rig.
pub fn set_lighting(state: &mut AppState, name: &str) -> Result<(), String> {
    let preset = LightingPreset::ALL
        .iter()
        .copied()
        .find(|preset| preset.label() == name && !preset.is_uv_lamp())
        .ok_or_else(|| format!("'{name}' is not a lighting rig."))?;
    state.update_settings(|settings| {
        preset
            .label()
            .clone_into(&mut settings.lighting.preset_name);
    });
    // Choosing a rig leaves "HDR Map" as the lighting; the imported map stays for later.
    if state.hdr.is_selected()
        && let Err(message) = crate::hdr::set_selected(state, false)
    {
        tracing::warn!(%message, "the HDR map could not be switched off");
    }
    Ok(())
}

/// The light sliders' values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightValues {
    /// Exposure multiplier ([`EXPOSURE_RANGE`]).
    pub exposure: f32,
    /// Light direction around the stone in degrees (`0.0..=360.0`).
    pub azimuth_deg: f32,
    /// Light height above the stone in degrees ([`ELEVATION_RANGE_DEG`]).
    pub elevation_deg: f32,
    /// Head-shadow radius in degrees (`0.0..=30.0`, `0` is off).
    pub head_shadow_deg: f32,
    /// Surface glare in percent (`0.0..=100.0`, steps of 5).
    pub surface_glare_pct: f32,
}

/// `value` within `range`, `fallback` when it is not a number.
const fn clamped(value: f32, range: (f32, f32), fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(range.0, range.1)
    } else {
        fallback
    }
}

/// Stores the light values in the settings (clamped to their ranges; the azimuth wraps; the
/// glare snaps to 5 percent).
pub fn set_light_values(state: &mut AppState, values: LightValues) {
    let defaults = LightingSettings::default();
    let exposure = clamped(values.exposure, EXPOSURE_RANGE, defaults.exposure);
    let azimuth = if values.azimuth_deg.is_finite() {
        values.azimuth_deg.rem_euclid(360.0)
    } else {
        defaults.azimuth_deg
    };
    let elevation = clamped(
        values.elevation_deg,
        ELEVATION_RANGE_DEG,
        defaults.elevation_deg,
    );
    let head_shadow = clamped(
        values.head_shadow_deg,
        HEAD_SHADOW_RANGE,
        defaults.head_shadow_deg,
    );
    let glare = clamped(
        values.surface_glare_pct,
        (0.0, 100.0),
        DEFAULT_SURFACE_GLARE_PCT,
    );
    let glare = (glare / GLARE_STEP_PCT).round() * GLARE_STEP_PCT;
    state.update_settings(|settings| {
        settings.lighting.exposure = exposure;
        settings.lighting.azimuth_deg = azimuth;
        settings.lighting.elevation_deg = elevation;
        settings.lighting.head_shadow_deg = head_shadow;
        settings.lighting.surface_glare_pct = glare;
    });
}

/// Chooses what the camera sees behind the stone.
pub fn set_backdrop(state: &mut AppState, backdrop: Backdrop) {
    state.update_settings(|settings| settings.lighting.backdrop = backdrop);
}

/// Everything the Light tab shows, read from the settings.
#[derive(Debug, Clone, PartialEq)]
pub struct LightingView {
    /// The chosen rig's stable name (one of [`lighting_rows`]'s).
    pub preset_name: String,
    /// The light values.
    pub values: LightValues,
    /// The backdrop as the Slint segmented control numbers it: 0 as lit, 1 grey, 2 white.
    pub backdrop_index: i32,
    /// Whether the rig has a key light whose direction the sliders move (every rig but
    /// the grading tray).
    pub uses_light_direction: bool,
    /// Whether the rig is a lit model, the only kind the head shadow applies to.
    pub uses_head_shadow: bool,
    /// The imported HDR map and whether it is the chosen lighting (then no rig is).
    pub hdr: HdrView,
}

/// The index of `backdrop` in the Light tab's segmented control.
#[must_use]
pub const fn backdrop_index(backdrop: Backdrop) -> i32 {
    match backdrop {
        Backdrop::AsLit => 0,
        Backdrop::Grey => 1,
        Backdrop::White => 2,
    }
}

/// The backdrop at `index` of the Light tab's segmented control; anything unknown reads as
/// the default grey card.
#[must_use]
pub const fn backdrop_from_index(index: i32) -> Backdrop {
    match index {
        0 => Backdrop::AsLit,
        2 => Backdrop::White,
        _ => Backdrop::Grey,
    }
}

/// The Light tab for the settings as they stand.
#[must_use]
pub fn lighting_view(state: &AppState) -> LightingView {
    let lighting = &state.settings.lighting;
    let preset = preset_of(lighting);
    LightingView {
        preset_name: preset.label().to_owned(),
        values: LightValues {
            exposure: lighting.exposure,
            azimuth_deg: lighting.azimuth_deg,
            elevation_deg: lighting.elevation_deg,
            head_shadow_deg: lighting.head_shadow_deg,
            surface_glare_pct: lighting.surface_glare_pct,
        },
        backdrop_index: backdrop_index(lighting.backdrop),
        uses_light_direction: preset != LightingPreset::IsoHemisphere,
        uses_head_shadow: preset.model() != LightingModel::Studio,
        hdr: state.hdr.view(),
    }
}

/// The renderer's scene settings for the app as it stands.
///
/// The lighting comes from the settings, the stone effects and width from the design, the bounce count and sample target from
/// the render preferences (`window_is_compact` picks the phone or the large-window
/// sample count).
#[must_use]
pub fn render_settings(state: &AppState, window_is_compact: bool) -> RenderSettings {
    let lighting = &state.settings.lighting;
    let render = &state.settings.render;
    let effects = state.session.design.render_effects;
    RenderSettings {
        material: state.settings.material_name.clone(),
        lighting: preset_of(lighting).label().to_owned(),
        exposure: lighting.exposure,
        light_yaw_deg: lighting.azimuth_deg,
        light_pitch_deg: lighting.elevation_deg,
        backdrop_index: backdrop_index(lighting.backdrop),
        head_shadow_deg: lighting.head_shadow_deg,
        max_bounces: render.bounces,
        target_spp: if window_is_compact {
            render.live_spp_compact
        } else {
            render.live_spp_large
        },
        link_material: state.props.link_material,
        denoise: render.denoise,
        frosted_girdle: effects.girdle_frosted,
        c_axis_override: effects.c_axis_override_enabled,
        c_axis_tilt_deg: narrow(effects.c_axis_tilt_deg),
        c_axis_azimuth_deg: narrow(effects.c_axis_azimuth_deg),
        inclusion_sigma_s: narrow(effects.inclusion_sigma_s),
        edge_rounding_radius: narrow(effects.edge_rounding_radius),
        stone_width_mm: narrow(state.session.design.girdle_diameter_mm.unwrap_or(0.0)),
        ..RenderSettings::default()
    }
    .sanitized()
}
