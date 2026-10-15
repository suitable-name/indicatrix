//! [`StoneRenderEffects`]: the per-design stone effects of the live and exported render.
//!
//! Inclusion haze (`sigma_s`), crystal-axis orientation, frosted girdle and facet-edge
//! rounding used to be app-level settings, so opening another design rendered it with the
//! previous design's values. They belong to the stone, so they live on the [`Design`]
//! (`Design::render_effects`), travel in the native file's optional `[render_effects]`
//! table, and change only through the undoable `Edit::SetRenderEffects`.
//!
//! # Compatibility
//!
//! A design without the table (an old file, an imported `.asc`) has
//! [`StoneRenderEffects::default`]: every effect off, which is the plain render. `.asc` has
//! no field for these, so a re-imported `.asc` gets the defaults.
//!
//! [`Design`]: super::Design

/// Largest inclusion `sigma_s` the file may hold (the app slider reaches 2.0).
pub const MAX_INCLUSION_SIGMA_S: f64 = 3.0;

/// Largest facet-edge rounding radius, in model units (the furnace-verified ceiling).
pub const MAX_EDGE_ROUNDING_RADIUS: f64 = 0.03;

/// The four per-design stone render effects. `Default` is "everything off".
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StoneRenderEffects {
    /// Inclusion / subsurface scattering `sigma_s`, per model unit. `0.0` is off.
    pub inclusion_sigma_s: f64,
    /// Whether the crystal-axis orientation override is on ("as cut" when off).
    pub c_axis_override_enabled: bool,
    /// Tilt of the override axis from the table normal, `0.0..=90.0` degrees.
    pub c_axis_tilt_deg: f64,
    /// Azimuth of the override axis around the table normal, `0.0..=360.0` degrees.
    pub c_axis_azimuth_deg: f64,
    /// Bruted (frosted) girdle finish instead of polished.
    pub girdle_frosted: bool,
    /// Facet-edge rounding radius in model units, `0.0..=0.03`. `0.0` is off.
    pub edge_rounding_radius: f64,
}

impl StoneRenderEffects {
    /// Whether this is exactly the default (nothing to write to a file).
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// These effects with every value inside its legal range; anything not a number
    /// becomes the default for that field.
    #[must_use]
    pub fn sanitised(self) -> Self {
        let finite = |v: f64| if v.is_finite() { v } else { 0.0 };
        Self {
            inclusion_sigma_s: finite(self.inclusion_sigma_s).clamp(0.0, MAX_INCLUSION_SIGMA_S),
            c_axis_override_enabled: self.c_axis_override_enabled,
            c_axis_tilt_deg: finite(self.c_axis_tilt_deg).clamp(0.0, 90.0),
            c_axis_azimuth_deg: finite(self.c_axis_azimuth_deg).clamp(0.0, 360.0),
            girdle_frosted: self.girdle_frosted,
            edge_rounding_radius: finite(self.edge_rounding_radius)
                .clamp(0.0, MAX_EDGE_ROUNDING_RADIUS),
        }
    }

    /// Builds effects from the `f32` values the render context holds, rounded to six
    /// decimals so a value read back from a TOML file compares equal to the one written.
    #[must_use]
    pub fn from_render_values(
        inclusion_sigma_s: f32,
        c_axis_override_enabled: bool,
        c_axis_tilt_deg: f32,
        c_axis_azimuth_deg: f32,
        girdle_frosted: bool,
        edge_rounding_radius: f32,
    ) -> Self {
        let round6 = |v: f32| (f64::from(v) * 1e6).round() / 1e6;
        Self {
            inclusion_sigma_s: round6(inclusion_sigma_s),
            c_axis_override_enabled,
            c_axis_tilt_deg: round6(c_axis_tilt_deg),
            c_axis_azimuth_deg: round6(c_axis_azimuth_deg),
            girdle_frosted,
            edge_rounding_radius: round6(edge_rounding_radius),
        }
        .sanitised()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_off_and_sanitising_limits_ranges() {
        assert!(StoneRenderEffects::default().is_default());
        let wild = StoneRenderEffects {
            inclusion_sigma_s: 99.0,
            c_axis_tilt_deg: f64::NAN,
            c_axis_azimuth_deg: 720.0,
            edge_rounding_radius: -1.0,
            ..StoneRenderEffects::default()
        }
        .sanitised();
        assert_eq!(wild.inclusion_sigma_s.to_bits(), 3.0f64.to_bits());
        assert_eq!(wild.c_axis_tilt_deg.to_bits(), 0.0f64.to_bits());
        assert_eq!(wild.c_axis_azimuth_deg.to_bits(), 360.0f64.to_bits());
        assert_eq!(wild.edge_rounding_radius.to_bits(), 0.0f64.to_bits());
    }
}
