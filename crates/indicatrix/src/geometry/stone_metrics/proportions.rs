//! [`StoneProportions`]: a design's proportion readouts the way a cutter
//! quotes them off a finished stone, derived from an already-measured solid.

use glam::DVec3;

use super::{
    TABLE_NY,
    types::{SolidMesh, SolidMetrics},
};

/// A design's proportion readouts the way a cutter quotes them off a
/// finished stone.
///
/// Table size as a percentage of width, crown height, pavilion depth, total
/// depth, girdle thickness, and the ratios (L/W, C/W, P/W) every faceting
/// diagram prints alongside them.
///
/// All lengths are in the arrangement's own mast units, the same convention
/// [`SolidMetrics`] uses -- multiply by a real millimetres-per-unit scale
/// (e.g. `indicatrix_cut_core::yield_metrics::mm_per_unit`) for a physical
/// figure, or use [`Self::to_mm`]. `table_percent`, `length_to_width` and the
/// `_to_width_percent` fields are already scale-invariant ratios/percentages
/// and never need that conversion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoneProportions {
    /// The table facet's own horizontal extent, as a percentage of the
    /// stone's [`SolidMetrics::width_axis`] -- the single figure every
    /// faceting diagram prints for a round or near-round cut. `None` when
    /// the arrangement has no single facet with an outward normal near
    /// straight up (e.g. a pointed crown authored with no table at all).
    pub table_percent: Option<f64>,
    /// [`SolidMetrics::crown_height`], unchanged.
    pub crown_height: Option<f64>,
    /// [`SolidMetrics::pavilion_depth`], unchanged.
    pub pavilion_depth: Option<f64>,
    /// [`SolidMetrics::girdle_thickness`], unchanged. `None` under the exact
    /// same condition as `crown_height`/`pavilion_depth`: no vertical girdle
    /// plane with a live facet (e.g. the manual's chapter-7 0-degree-"girdle"
    /// example, which classifies as a second table instead).
    pub girdle_thickness: Option<f64>,
    /// [`SolidMetrics::total_height`], under the name a cutter actually uses
    /// for it ("total depth": table to culet).
    pub total_depth: f64,
    /// [`SolidMetrics::length_axis`] over [`SolidMetrics::width_axis`].
    /// `None` only when `width_axis` is not positive -- a degenerate solid
    /// that should never reach here in practice, since
    /// [`measure_solid`](super::measure_solid) already refuses to return a
    /// non-positive-volume arrangement, but the ratio is still guarded
    /// rather than dividing by zero.
    pub length_to_width: Option<f64>,
    /// `crown_height` as a percentage of `width_axis` -- the printed "C/W"
    /// figure. `None` whenever `crown_height` itself is `None`, or
    /// `width_axis` is not positive.
    pub crown_to_width_percent: Option<f64>,
    /// `pavilion_depth` as a percentage of `width_axis` -- the printed "P/W"
    /// figure. `None` whenever `pavilion_depth` itself is `None`, or
    /// `width_axis` is not positive.
    pub pavilion_to_width_percent: Option<f64>,
    /// `girdle_thickness` as a percentage of `width_axis` -- the girdle
    /// thickness figure every faceter checks alongside table/crown/pavilion.
    /// `None` whenever `girdle_thickness` itself is `None`, or `width_axis`
    /// is not positive.
    pub girdle_to_width_percent: Option<f64>,
}

impl StoneProportions {
    /// Derives every figure from an already-measured solid: `metrics` for
    /// the extents, `mesh` and `planes` (the same slice
    /// [`build_solid_mesh`](super::build_solid_mesh) was called with, in the
    /// same order) for the table facet's own ring, which `metrics` alone
    /// does not carry.
    #[must_use]
    pub fn from_solid(metrics: &SolidMetrics, mesh: &SolidMesh, planes: &[(DVec3, f64)]) -> Self {
        let width = (metrics.width_axis > 1e-9).then_some(metrics.width_axis);
        let to_width_percent =
            |value: Option<f64>| Option::zip(value, width).map(|(v, w)| 100.0 * v / w);
        Self {
            table_percent: table_percent(metrics, mesh, planes),
            crown_height: metrics.crown_height,
            pavilion_depth: metrics.pavilion_depth,
            girdle_thickness: metrics.girdle_thickness,
            total_depth: metrics.total_height,
            length_to_width: width.map(|w| metrics.length_axis / w),
            crown_to_width_percent: to_width_percent(metrics.crown_height),
            pavilion_to_width_percent: to_width_percent(metrics.pavilion_depth),
            girdle_to_width_percent: to_width_percent(metrics.girdle_thickness),
        }
    }

    /// Scales the absolute-length fields (crown height, pavilion depth,
    /// girdle thickness, total depth) by `mm_per_unit`; every ratio/percentage
    /// field (`table_percent`, `length_to_width`, the three `_to_width_percent`
    /// fields) passes through unchanged -- the same linear-factor convention
    /// `indicatrix_cut_core::yield_metrics::PreformFit::to_mm` uses.
    #[must_use]
    pub fn to_mm(&self, mm_per_unit: f64) -> Self {
        Self {
            table_percent: self.table_percent,
            crown_height: self.crown_height.map(|v| v * mm_per_unit),
            pavilion_depth: self.pavilion_depth.map(|v| v * mm_per_unit),
            girdle_thickness: self.girdle_thickness.map(|v| v * mm_per_unit),
            total_depth: self.total_depth * mm_per_unit,
            length_to_width: self.length_to_width,
            crown_to_width_percent: self.crown_to_width_percent,
            pavilion_to_width_percent: self.pavilion_to_width_percent,
            girdle_to_width_percent: self.girdle_to_width_percent,
        }
    }
}

/// The table facet's own horizontal extent, as a percentage of the solid's
/// width -- the geometric half of [`StoneProportions::table_percent`].
///
/// Finds the ring in `mesh.rings` whose originating plane (looked up in
/// `planes`) has an outward normal within [`TABLE_NY`] of straight up
/// (`+Y`); among any that match (there should be exactly one on a normal
/// faceted stone), picks the one whose vertices sit highest, breaking a tie
/// by plane order for determinism. Measures that ring's own horizontal
/// extent with the same axis convention
/// [`measure_solid`](super::measure_solid) uses for the whole stone's
/// `width_axis` (the smaller of the two axis-aligned extents), so
/// `table_percent` and `width_axis` are always directly comparable.
fn table_percent(metrics: &SolidMetrics, mesh: &SolidMesh, planes: &[(DVec3, f64)]) -> Option<f64> {
    if metrics.width_axis <= 1e-9 {
        return None;
    }
    let mut best: Option<(f64, &Vec<DVec3>)> = None;
    for (plane_idx, ring) in &mesh.rings {
        let Some(&(normal, _)) = planes.get(*plane_idx) else {
            continue;
        };
        if normal.y < TABLE_NY || ring.len() < 3 {
            continue;
        }
        let top = ring.iter().map(|v| v.y).fold(f64::NEG_INFINITY, f64::max);
        if best.is_none_or(|(best_top, _)| top > best_top) {
            best = Some((top, ring));
        }
    }
    let (_, ring) = best?;
    let x_max = ring.iter().map(|v| v.x).fold(f64::NEG_INFINITY, f64::max);
    let x_min = ring.iter().map(|v| v.x).fold(f64::INFINITY, f64::min);
    let z_max = ring.iter().map(|v| v.z).fold(f64::NEG_INFINITY, f64::max);
    let z_min = ring.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
    let table_width = (x_max - x_min).min(z_max - z_min);
    Some(100.0 * table_width / metrics.width_axis)
}
