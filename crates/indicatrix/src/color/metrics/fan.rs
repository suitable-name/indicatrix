//! Scale of the analytical ray fan: how far from the view axis the grid's rays start,
//! derived from the stone being measured.
//!
//! The 18x18 grid lives in unit coordinates `(u, v)`; [`FanGeometry`] maps a cell to a
//! ray origin in model units. The mapping is proportional to the stone's measured girdle
//! half-width, so the sampled disc reaches [`OUTLINE_COVERAGE`] of a face-up round
//! outline whatever size the design was solved at, instead of a fixed disc that stops
//! short of the crown rim on a wide stone and overshoots a small one. For an elongated
//! outline the measured width is the short axis, so the disc spans that axis and the
//! ends of the long axis stay outside it.

use std::cell::Cell;

use glam::Vec3;

use crate::{
    geometry::plane::GpuFacetPlane,
    render_setup::{hash_planes, measure_model_width},
};

/// Squared radius, in the grid's unit coordinates, beyond which a cell is skipped.
pub(super) const GRID_DISC_RADIUS_SQ: f32 = 0.70;

/// Fraction of the girdle half-width the outermost sampled cell reaches.
const OUTLINE_COVERAGE: f32 = 0.95;

/// Distance from the stone's centre back to the plane the rays start on, in girdle
/// half-widths. Scaling it with the stone keeps every ray origin outside the solid.
const STANDOFF_HALF_WIDTHS: f32 = 2.5;

/// Half-width assumed when the design does not measure (an unfinished plane set).
const FALLBACK_HALF_WIDTH: f32 = 1.0;

thread_local! {
    /// The last design measured on this thread, as `(hash_planes, half_width)`.
    ///
    /// Measuring enumerates every plane triple, far more expensive than one evaluation,
    /// while a sweep evaluates the same design hundreds of times in a row. The half-width
    /// is a pure function of the planes, so reusing it changes no result.
    static LAST_MEASURED: Cell<Option<(u64, f32)>> = const { Cell::new(None) };
}

/// The girdle half-width of `planes` in model units, [`FALLBACK_HALF_WIDTH`] when the
/// design does not measure.
fn girdle_half_width(planes: &[GpuFacetPlane]) -> f32 {
    let key = hash_planes(planes);
    let cached = LAST_MEASURED
        .with(Cell::get)
        .filter(|&(hash, _)| hash == key);
    if let Some((_, half_width)) = cached {
        return half_width;
    }
    let half_width = measure_model_width(planes)
        .filter(|width| width.is_finite() && *width > 0.0)
        .map_or(FALLBACK_HALF_WIDTH, |width| (width * 0.5) as f32);
    LAST_MEASURED.with(|slot| slot.set(Some((key, half_width))));
    half_width
}

/// Where the fan's rays start, for one stone: the view-plane offset of a unit-grid cell
/// and the stand-off of that plane from the stone's centre.
#[derive(Clone, Copy, Debug)]
pub(super) struct FanGeometry {
    /// Distance of the ray-origin plane from the stone's centre, along the view axis.
    standoff: f32,
    /// Model units per unit of grid coordinate in the view plane.
    lateral: f32,
}

impl FanGeometry {
    /// The fan for a stone with the given facet planes.
    pub(super) fn for_planes(planes: &[GpuFacetPlane]) -> Self {
        let half_width = girdle_half_width(planes);
        Self {
            standoff: STANDOFF_HALF_WIDTHS * half_width,
            lateral: OUTLINE_COVERAGE * half_width / GRID_DISC_RADIUS_SQ.sqrt(),
        }
    }

    /// The measured girdle half-width in model units (the stand-off is a fixed multiple
    /// of it), the length scale the face-up tone's path histogram is laid out in.
    pub(super) fn half_width(self) -> f32 {
        self.standoff / STANDOFF_HALF_WIDTHS
    }

    /// The ray origin for grid cell `(u, v)` (each in `[-1, 1]`) seen along the camera
    /// frame `forward`/`right`/`up`.
    pub(super) fn origin(self, forward: Vec3, right: Vec3, up: Vec3, u: f32, v: f32) -> Vec3 {
        -forward * self.standoff + (u * self.lateral) * right + (v * self.lateral) * up
    }
}
