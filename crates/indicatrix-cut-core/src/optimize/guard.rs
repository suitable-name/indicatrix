//! The opt-in shape guard ([`super::OptimizeOptions::min_girdle_fraction`]): a
//! candidate must keep the girdle band and the table facets the starting design had.
//!
//! Three things are measured once on the starting design and checked on every
//! candidate that closes:
//!
//! - the girdle band thickness, from `measure_solid`'s `girdle_thickness`: a band that
//!   vanishes, or is thinner than the requested fraction of the starting band, is
//!   rejected. A starting design with no live girdle has nothing to keep, so this
//!   half then admits everything;
//! - the girdle band's THINNEST point, from [`girdle_band_in`]: the smallest vertical gap
//!   between its crown-side and pavilion-side edges around the whole outline, which the
//!   figure above cannot see (it reads two extreme vertices, and a facet turned about
//!   its girdle edge keeps exactly those). A candidate whose band runs to a knife edge
//!   anywhere, is thinner there than the requested fraction of the starting band's
//!   thinnest point, or has lost a girdle wall, is rejected. A starting band that is a
//!   knife edge already has nothing to keep;
//! - every table tier (an angle of exactly `+0.0`): when the tier's own facet ring was
//!   in the starting solid, it must still be in the candidate's. The ring is looked up
//!   through the tier's own planes (see [`tier_plane_ranges`]), not through the
//!   solid's overall table percentage, which any horizontal face at the top would
//!   satisfy, including the preform's.

use crate::design::{Design, GirdleBand, girdle_band_in, tier_plane_ranges};
use glam::DVec3;
use indicatrix::geometry::{
    meet_solver::SolvedTier,
    stone_metrics::{SolidMesh, measure_solid},
};
use std::ops::Range;

/// Whether `mesh` has a facet ring for any plane in `range`.
pub(super) fn ring_present(mesh: &SolidMesh, range: &Range<usize>) -> bool {
    mesh.rings
        .iter()
        .any(|(plane_index, ring)| range.contains(plane_index) && ring.len() >= 3)
}

/// Whether a candidate's girdle thickness is acceptable against the starting
/// design's: with no starting girdle there is nothing to keep; otherwise the
/// candidate must have a positive band at least `fraction` times the starting one.
pub(super) fn girdle_admits(baseline: Option<f64>, fraction: f64, candidate: Option<f64>) -> bool {
    let Some(baseline) = baseline else {
        return true;
    };
    candidate.is_some_and(|thickness| thickness > 0.0 && thickness >= fraction * baseline)
}

/// Whether a candidate's girdle band keeps the starting design's thinnest point and its
/// walls: with no starting girdle there is nothing to keep; otherwise the candidate must
/// have a girdle, no fewer live walls, and (unless the starting band was a knife edge
/// itself) a thinnest point above a knife edge and at least `fraction` times the starting
/// one.
pub(super) fn band_admits(
    baseline: Option<GirdleBand>,
    fraction: f64,
    candidate: Option<GirdleBand>,
) -> bool {
    let Some(baseline) = baseline else {
        return true;
    };
    let Some(candidate) = candidate else {
        return false;
    };
    if candidate.live_walls < baseline.live_walls {
        return false;
    }
    baseline.is_knife_edge()
        || (!candidate.is_knife_edge()
            && candidate.min_thickness >= fraction * baseline.min_thickness)
}

/// Tiers authored exactly at `+0.0`: the tables (a sign-negative zero is the culet).
fn table_tiers(design: &Design) -> impl Iterator<Item = usize> {
    design
        .tiers
        .iter()
        .enumerate()
        .filter(|(_, tier)| tier.angle_deg == 0.0 && !tier.angle_deg.is_sign_negative())
        .map(|(index, _)| index)
}

/// What the starting design had that a candidate must keep.
#[derive(Debug, Clone)]
pub(super) struct ShapeGuard {
    min_girdle_fraction: f64,
    /// The share of the starting band's thinnest point a candidate must keep. The same as
    /// `min_girdle_fraction` unless [`Self::with_thinnest_fraction`] set it apart.
    min_thinnest_fraction: f64,
    baseline_girdle: Option<f64>,
    /// The starting band's thinnest point and live walls.
    baseline_band: Option<GirdleBand>,
    /// Table tiers whose own facet ring is in the starting solid.
    table_tiers: Vec<usize>,
}

impl ShapeGuard {
    /// Measures the starting design: `solved`, `planes` and `mesh` are its solve, its
    /// plane arrangement and the closed solid those planes bound.
    pub(super) fn measure(
        design: &Design,
        solved: &[SolvedTier],
        planes: &[(DVec3, f64)],
        mesh: &SolidMesh,
        min_girdle_fraction: f64,
    ) -> Self {
        let baseline_girdle = measure_solid(planes)
            .and_then(|metrics| metrics.girdle_thickness)
            .filter(|&thickness| thickness > 0.0);
        let ranges = tier_plane_ranges(design, solved);
        let baseline_band = girdle_band_in(design, &ranges, planes, mesh);
        let table_tiers = table_tiers(design)
            .filter(|&tier| ranges.get(tier).is_some_and(|r| ring_present(mesh, r)))
            .collect();
        let fraction = min_girdle_fraction.max(0.0);
        Self {
            min_girdle_fraction: fraction,
            min_thinnest_fraction: fraction,
            baseline_girdle,
            baseline_band,
            table_tiers,
        }
    }

    /// The same guard with its own floor for the band's thinnest point; `None` keeps the one
    /// it has (the girdle fraction `measure` was given).
    pub(super) const fn with_thinnest_fraction(mut self, fraction: Option<f64>) -> Self {
        if let Some(fraction) = fraction {
            self.min_thinnest_fraction = fraction.max(0.0);
        }
        self
    }

    /// Whether a candidate (its solve, planes and closed mesh) keeps the girdle and
    /// the table facets the starting design had.
    pub(super) fn admits(
        &self,
        design: &Design,
        solved: &[SolvedTier],
        planes: &[(DVec3, f64)],
        mesh: &SolidMesh,
    ) -> bool {
        let thickness = measure_solid(planes).and_then(|metrics| metrics.girdle_thickness);
        if !girdle_admits(self.baseline_girdle, self.min_girdle_fraction, thickness) {
            return false;
        }
        let ranges = tier_plane_ranges(design, solved);
        let band = girdle_band_in(design, &ranges, planes, mesh);
        if !band_admits(self.baseline_band, self.min_thinnest_fraction, band) {
            return false;
        }
        self.table_tiers
            .iter()
            .all(|&tier| ranges.get(tier).is_some_and(|r| ring_present(mesh, r)))
    }
}
