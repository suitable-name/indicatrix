//! The Stone tab's values and the edits behind its controls: the stone width, the
//! inclusion haze, the frosted girdle, the edge rounding and the crystal-axis override.
//!
//! All of them live in the design and change through undoable edits: the width is the
//! girdle diameter (`Edit::SetGirdleDiameterMm`, the same value as the Preform field of the
//! Checks tab), the four effects are the design's `StoneRenderEffects`
//! (`Edit::SetRenderEffects`). A slider is applied when it is released, one undo step per
//! release.

use indicatrix::{optics::materials::OpticalCharacter, render_setup::inclusion_scale};
use indicatrix_cut_core::{Edit, StoneRenderEffects};

use super::{
    materials::{
        body_color_available, body_color_label, effective_ri_text, material_summary, resolved_gem,
    },
    narrow, parse_optional_positive, trimmed,
};
use crate::app_state::AppState;

/// The edge-rounding slider's range, in model units (the furnace-verified ceiling).
pub const EDGE_ROUNDING_RANGE: (f32, f32) = (0.0, 0.03);
/// The crystal-axis tilt range from the table normal, degrees.
pub const AXIS_TILT_RANGE_DEG: (f32, f32) = (0.0, 90.0);
/// The crystal-axis azimuth range around the table normal, degrees.
pub const AXIS_AZIMUTH_RANGE_DEG: (f32, f32) = (0.0, 360.0);

/// The widest stone a width field accepts, in millimetres.
const MAX_WIDTH_MM: f64 = 1000.0;

/// Everything the Stone tab shows, read from the design.
#[derive(Debug, Clone, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the Stone tab's Slint model; color_enabled, frosted, axis_on and axis_available are independent switches"
)]
pub struct StoneView {
    /// The design's material name, or `Not Set`.
    pub material_title: String,
    /// The material's optics in one line (or what to do when none is set).
    pub material_detail: String,
    /// The refractive-index override as typed text; empty when the material's own applies.
    pub ri_text: String,
    /// `Effective RI 2.4170 · Critical angle 24.41°`, empty without a usable index.
    pub effective_ri_text: String,
    /// The body color's label (`Material Default`, `Blue`, ...).
    pub color_label: String,
    /// Whether the body color can be changed.
    pub color_enabled: bool,
    /// The stone width in millimetres as editable text; empty when not set.
    pub width_text: String,
    /// One sentence under the width field.
    pub width_note: String,
    /// The inclusion-haze slider position (`0.0..=1.0`, finer at the clean end).
    pub haze_position: f32,
    /// Whether the girdle is frosted.
    pub frosted: bool,
    /// The edge-rounding radius ([`EDGE_ROUNDING_RANGE`]).
    pub edge_rounding: f32,
    /// Whether the material has a crystal axis to orient (false for an isotropic one).
    pub axis_available: bool,
    /// Whether the crystal-axis override is on.
    pub axis_on: bool,
    /// The override's tilt in degrees.
    pub axis_tilt_deg: f32,
    /// The override's azimuth in degrees.
    pub axis_azimuth_deg: f32,
}

/// The Stone tab for the design as it stands.
#[must_use]
pub fn stone_view(state: &AppState) -> StoneView {
    let design = &state.session.design;
    let summary = material_summary(state);
    let effects = design.render_effects;
    let width = design.girdle_diameter_mm;
    let axis_available =
        resolved_gem(state).is_none_or(|gem| gem.optical_character != OpticalCharacter::Isotropic);
    StoneView {
        material_title: summary.title,
        material_detail: summary.detail,
        ri_text: design
            .material
            .refractive_index_override
            .map_or_else(String::new, |value| trimmed(value, 4)),
        effective_ri_text: effective_ri_text(state),
        color_label: body_color_label(state),
        color_enabled: body_color_available(state),
        width_text: width.map_or_else(String::new, |mm| trimmed(mm, 3)),
        width_note: width.map_or_else(
            || "Not set: colors are shown for a 7 mm stone.".to_owned(),
            |mm| format!("Saved in the design: {} mm.", trimmed(mm, 3)),
        ),
        haze_position: inclusion_scale::sigma_s_to_position(narrow(effects.inclusion_sigma_s)),
        frosted: effects.girdle_frosted,
        edge_rounding: narrow(effects.edge_rounding_radius),
        axis_available,
        axis_on: effects.c_axis_override_enabled,
        axis_tilt_deg: narrow(effects.c_axis_tilt_deg),
        axis_azimuth_deg: narrow(effects.c_axis_azimuth_deg),
    }
}

/// Sets the stone width (the girdle diameter in millimetres) from `text`; blank clears it.
/// Arithmetic is allowed (`6.5+0.25`).
///
/// # Errors
///
/// A sentence when the text is not a positive number, or the editor refuses.
pub fn set_stone_width(state: &mut AppState, text: &str) -> Result<bool, String> {
    let girdle_diameter_mm = parse_optional_positive("Stone width", text)?;
    if girdle_diameter_mm.is_some_and(|mm| mm > MAX_WIDTH_MM) {
        return Err(format!(
            "Stone width must be at most {MAX_WIDTH_MM} millimetres."
        ));
    }
    if girdle_diameter_mm == state.session.design.girdle_diameter_mm {
        return Ok(false);
    }
    state
        .session
        .apply(Edit::SetGirdleDiameterMm { girdle_diameter_mm })
        .map(|_| true)
        .map_err(|error| error.to_string())
}

/// Rounds to six decimals, so a value read back from a design file compares equal to the
/// one written (the rule `StoneRenderEffects::from_render_values` follows).
fn round6(value: f64) -> f64 {
    (value * 1e6).round() / 1e6
}

/// Applies `change` to a copy of the design's stone effects (sanitized afterwards) as one
/// undoable edit; none when nothing changed.
fn update_effects(
    state: &mut AppState,
    change: impl FnOnce(&mut StoneRenderEffects),
) -> Result<bool, String> {
    let current = state.session.design.render_effects;
    let mut next = current;
    change(&mut next);
    let next = next.sanitised();
    if next == current {
        return Ok(false);
    }
    state
        .session
        .apply(Edit::SetRenderEffects { effects: next })
        .map(|_| true)
        .map_err(|error| error.to_string())
}

/// Sets the inclusion haze from the slider `position` (`0.0..=1.0`; the coefficient is
/// `2 * position^2`, finer at the clean end).
///
/// # Errors
///
/// The editor's refusal as a sentence.
pub fn set_inclusion_haze(state: &mut AppState, position: f32) -> Result<bool, String> {
    let sigma_s = f64::from(inclusion_scale::position_to_sigma_s(position));
    update_effects(state, |effects| effects.inclusion_sigma_s = round6(sigma_s))
}

/// Turns the frosted (ground, matt) girdle on or off.
///
/// # Errors
///
/// The editor's refusal as a sentence.
pub fn set_frosted_girdle(state: &mut AppState, frosted: bool) -> Result<bool, String> {
    update_effects(state, |effects| effects.girdle_frosted = frosted)
}

/// Sets the facet-edge rounding radius (clamped to [`EDGE_ROUNDING_RANGE`]).
///
/// # Errors
///
/// The editor's refusal as a sentence.
pub fn set_edge_rounding(state: &mut AppState, radius: f32) -> Result<bool, String> {
    let radius = if radius.is_finite() {
        radius.clamp(EDGE_ROUNDING_RANGE.0, EDGE_ROUNDING_RANGE.1)
    } else {
        0.0
    };
    update_effects(state, |effects| {
        effects.edge_rounding_radius = round6(f64::from(radius));
    })
}

/// Sets the crystal-axis override: on or off, and the axis's tilt from the table normal
/// and azimuth around it (both clamped to their ranges).
///
/// # Errors
///
/// The editor's refusal as a sentence.
pub fn set_crystal_axis(
    state: &mut AppState,
    on: bool,
    tilt_deg: f32,
    azimuth_deg: f32,
) -> Result<bool, String> {
    let clamp = |value: f32, range: (f32, f32)| {
        if value.is_finite() {
            value.clamp(range.0, range.1)
        } else {
            range.0
        }
    };
    let tilt = round6(f64::from(clamp(tilt_deg, AXIS_TILT_RANGE_DEG)));
    let azimuth = round6(f64::from(clamp(azimuth_deg, AXIS_AZIMUTH_RANGE_DEG)));
    update_effects(state, |effects| {
        effects.c_axis_override_enabled = on;
        effects.c_axis_tilt_deg = tilt;
        effects.c_axis_azimuth_deg = azimuth;
    })
}
