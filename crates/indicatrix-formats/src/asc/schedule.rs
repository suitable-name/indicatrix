//! [`AscSchedule`]: a fully parsed `GemCAD` `.asc` cutting schedule.

use super::tier::AscTier;
use std::fmt;

/// A fully parsed `GemCAD` `.asc` cutting schedule.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AscSchedule {
    /// The `GemCad` version string from the file's first line (e.g. "5.0", "4.51").
    pub gemcad_version: String,
    /// Index-wheel tooth count, as encoded in the file's `g` line. `GemCAD` sometimes
    /// encodes this as a negative number (an internal handedness/direction
    /// convention); use [`Self::gear_teeth_abs`] for the actual tooth count.
    pub gear_teeth: i32,
    /// The `g` line's second field: the index wheel's reference angle.
    pub gear_reference_angle: f64,
    /// Rotational symmetry order (the `y` line's first field).
    pub symmetry_order: u32,
    /// Mirror-symmetry flag (the `y` line's second field, `y`/`n`).
    pub mirror: bool,
    /// Refractive index (the `I` line).
    pub refractive_index: f64,
    /// Every `H` (header/title) line, in file order, with the leading `H` stripped.
    pub headers: Vec<String>,
    /// Every `F` (footnote) line, in file order, with the leading `F` stripped.
    pub footnotes: Vec<String>,
    /// Every facet tier, in file order.
    pub tiers: Vec<AscTier>,
}

impl AscSchedule {
    /// The index wheel's tooth count as an unsigned magnitude, for azimuth
    /// computations (`phi = 2*pi*index/gear_teeth_abs()`).
    #[must_use]
    pub const fn gear_teeth_abs(&self) -> u32 {
        self.gear_teeth.unsigned_abs()
    }

    /// Total number of individual facet planes this schedule describes (the sum of
    /// each tier's index count, or 1 for a tier with no explicit index).
    #[must_use]
    pub fn facet_plane_count(&self) -> usize {
        self.tiers.iter().map(|t| t.indices.len().max(1)).sum()
    }
}

impl fmt::Display for AscSchedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "AscSchedule(gear={}, order={}, mirror={}, RI={}, tiers={})",
            self.gear_teeth,
            self.symmetry_order,
            self.mirror,
            self.refractive_index,
            self.tiers.len()
        )
    }
}
