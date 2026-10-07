//! Verification: where does a solved point appear in each photo, and where would its ghosts?
//!
//! The reprojection looks for the pixel whose refracted ray passes through the point: a
//! derivative-free pattern search on the distance of the point from the ray inside the stone,
//! started from the best local minima of a coarse grid around the straight-line pixel. The surface is
//! piecewise flat, so the distance is continuous but not smooth, and a pattern search copes with
//! that where a Newton step would not.
//!
//! Ghost images are the same search with the interior ray reflected totally at the surface one or
//! two times before it passes the point: the user may have clicked such an image by mistake.

use glam::{DVec2, DVec3};

use super::trace::{Scene, trace_pixel};

/// The pixel of a solved point in one view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reprojection {
    /// The view.
    pub view: usize,
    /// The pixel whose refracted ray passes closest to the point, `u` then `v`.
    pub pixel: [f64; 2],
    /// How far that ray still misses the point, in mm (0 for an exact image).
    pub miss_mm: f64,
    /// The distance in pixels from the user's mark, when one was given.
    pub error_px: Option<f64>,
}

/// A predicted ghost image of a point: a pixel where the point shows after internal reflections.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ghost {
    /// The view.
    pub view: usize,
    /// How many total internal reflections the light makes (1 or 2).
    pub bounces: u8,
    /// The pixel of the ghost, `u` then `v`.
    pub pixel: [f64; 2],
    /// How far the ray at that pixel still misses the point, in mm.
    pub miss_mm: f64,
}

/// The most reflections a ghost prediction follows.
pub const MAX_GHOST_BOUNCES: u8 = 2;

/// The eight neighbours of a pixel, as unit steps.
const NEIGHBOURS: [DVec2; 8] = [
    DVec2::new(1.0, 0.0),
    DVec2::new(-1.0, 0.0),
    DVec2::new(0.0, 1.0),
    DVec2::new(0.0, -1.0),
    DVec2::new(1.0, 1.0),
    DVec2::new(1.0, -1.0),
    DVec2::new(-1.0, 1.0),
    DVec2::new(-1.0, -1.0),
];

/// The smallest pattern-search step, in pixels.
const MIN_STEP_PX: f64 = 1e-6;

/// Grid cells per side of the coarse search.
const GRID_CELLS: usize = 40;

/// How many of the best local minima of the coarse grid are refined.
const REFINED_MINIMA: usize = 6;

/// The distance of `target` from the last stretch of the ray of `pixel` after `bounces` internal
/// reflections, or infinity when the ray cannot be followed that far.
fn path_miss(scene: &Scene<'_>, view: usize, pixel: DVec2, bounces: u8, target: DVec3) -> f64 {
    trace_pixel(scene, view, pixel, bounces).map_or(f64::INFINITY, |path| {
        path.legs
            .last()
            .map_or(f64::INFINITY, |leg| leg.distance_to(target))
    })
}

/// Minimises `objective` from `start` by a pattern search over the eight neighbours with a step
/// that halves whenever none of them improves. Returns the pixel and its value.
fn pattern_search(objective: &dyn Fn(DVec2) -> f64, start: DVec2, first_step: f64) -> (DVec2, f64) {
    let mut best = start;
    let mut best_value = objective(start);
    let mut step = first_step;
    for _ in 0..400 {
        if step < MIN_STEP_PX || best_value < 1e-12 {
            break;
        }
        let mut candidate = (best, best_value);
        for direction in NEIGHBOURS {
            let pixel = best + direction * step;
            let value = objective(pixel);
            if value < candidate.1 {
                candidate = (pixel, value);
            }
        }
        if candidate.1 < best_value {
            (best, best_value) = candidate;
        } else {
            step *= 0.5;
        }
    }
    (best, best_value)
}

/// The values of `objective` on a `(cells + 1)` by `(cells + 1)` grid centred on `centre`
/// reaching `half_extent` each way, row by row, with the grid's cell size.
fn grid_values(
    objective: &dyn Fn(DVec2) -> f64,
    centre: DVec2,
    half_extent: DVec2,
    cells: usize,
) -> (Vec<(DVec2, f64)>, f64) {
    let side = cells + 1;
    let cell = (half_extent * 2.0) / cells as f64;
    let corner = centre - half_extent;
    let mut samples = Vec::with_capacity(side * side);
    for row in 0..side {
        for col in 0..side {
            let pixel = corner + DVec2::new(cell.x * col as f64, cell.y * row as f64);
            samples.push((pixel, objective(pixel)));
        }
    }
    (samples, cell.x.max(cell.y))
}

/// The pixel at which the refracted ray through the stone passes closest to `target` (a point in
/// the mesh frame), and how closely.
///
/// `marked` is the pixel the user clicked, if any; the result then carries the distance to it,
/// the reprojection error. `None` when the view does not exist, the target is behind the camera
/// or no ray near it enters the stone.
#[must_use]
pub fn reproject(
    scene: &Scene<'_>,
    view: usize,
    target: DVec3,
    marked: Option<[f64; 2]>,
) -> Option<Reprojection> {
    let pose = scene.rig.views.get(view)?;
    let straight = pose.project(scene.alignment.to_rig(target))?;
    let half = (0.25 * f64::from(pose.image_size[0])).max(40.0);
    let objective = |pixel: DVec2| path_miss(scene, view, pixel, 0, target);
    let (samples, cell) = grid_values(&objective, straight, DVec2::splat(half), GRID_CELLS);
    let values: Vec<f64> = samples.iter().map(|sample| sample.1).collect();
    // A ray that enters through another face is another branch of the surface map, so several
    // local minima of the grid are refined and the best one wins.
    let (pixel, miss_mm) = local_minima(&values, GRID_CELLS + 1)
        .iter()
        .take(REFINED_MINIMA)
        .map(|&index| pattern_search(&objective, samples[index].0, cell))
        .min_by(|left, right| left.1.total_cmp(&right.1))?;
    if !miss_mm.is_finite() {
        return None;
    }
    Some(Reprojection {
        view,
        pixel: pixel.to_array(),
        miss_mm,
        error_px: marked.map(|mark| (pixel - DVec2::from_array(mark)).length()),
    })
}

/// Whether grid cell `index` (row-major, `side` cells per row) has a finite value no larger than
/// any neighbour's.
fn is_local_minimum(values: &[f64], side: usize, index: usize) -> bool {
    let here = values[index];
    if !here.is_finite() {
        return false;
    }
    let (row, col, side) = (
        (index / side) as isize,
        (index % side) as isize,
        side as isize,
    );
    (-1..=1).all(|d_row| {
        (-1..=1).all(|d_col| {
            let (near_row, near_col) = (row + d_row, col + d_col);
            let outside = near_row < 0 || near_col < 0 || near_row >= side || near_col >= side;
            outside
                || (d_row == 0 && d_col == 0)
                || here <= values[(near_row * side + near_col) as usize]
        })
    })
}

/// The indices of the grid cells whose value is finite and no larger than any neighbour's, best
/// first (equal values in index order).
fn local_minima(values: &[f64], side: usize) -> Vec<usize> {
    let mut minima: Vec<usize> = (0..values.len())
        .filter(|&index| is_local_minimum(values, side, index))
        .collect();
    minima.sort_by(|&left, &right| {
        values[left]
            .total_cmp(&values[right])
            .then(left.cmp(&right))
    });
    minima
}

/// The pixel box the stone covers in `view`, from the projected corners of the mesh's bounding
/// box, grown by 10 %; `None` when a corner is behind the camera.
fn stone_window(scene: &Scene<'_>, view: usize) -> Option<(DVec2, DVec2)> {
    let pose = scene.rig.views.get(view)?;
    let (lo, hi) = scene.mesh.bounds();
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for corner in 0..8_u32 {
        let pick = |bit: u32, low: f64, high: f64| if bit == 0 { low } else { high };
        let point = DVec3::new(
            pick(corner & 1, lo.x, hi.x),
            pick((corner >> 1) & 1, lo.y, hi.y),
            pick((corner >> 2) & 1, lo.z, hi.z),
        );
        let pixel = pose.project(scene.alignment.to_rig(point))?;
        min = min.min(pixel);
        max = max.max(pixel);
    }
    let margin = (max - min) * 0.1;
    Some((min - margin, max + margin))
}

/// Predicts the ghost images of `target` in view `view`: pixels where the point shows after one
/// or two (up to `max_bounces`, at most [`MAX_GHOST_BOUNCES`]) total internal reflections.
///
/// Only ghosts whose ray passes within `max_miss_mm` of the point are returned, nearest first.
/// The search covers the stone's outline in the photo; each ghost is a local minimum of the
/// distance found on a coarse grid and refined, so very faint or very narrow ghosts can be
/// missed. Hollow markers at these pixels show the user which image a click may have hit.
#[must_use]
pub fn predict_ghosts(
    scene: &Scene<'_>,
    view: usize,
    target: DVec3,
    max_bounces: u8,
    max_miss_mm: f64,
) -> Vec<Ghost> {
    let Some((low, high)) = stone_window(scene, view) else {
        return Vec::new();
    };
    let centre = low.midpoint(high);
    let half = (high - low) * 0.5;
    let cells = GRID_CELLS;
    let mut ghosts: Vec<Ghost> = Vec::new();
    for bounces in 1..=max_bounces.min(MAX_GHOST_BOUNCES) {
        let objective = |pixel: DVec2| path_miss(scene, view, pixel, bounces, target);
        let (samples, cell) = grid_values(&objective, centre, half, cells);
        let values: Vec<f64> = samples.iter().map(|sample| sample.1).collect();
        for &index in local_minima(&values, cells + 1).iter().take(8) {
            let (pixel, miss_mm) = pattern_search(&objective, samples[index].0, cell);
            let known = ghosts.iter().any(|ghost| {
                ghost.bounces == bounces && (DVec2::from_array(ghost.pixel) - pixel).length() < 0.5
            });
            if miss_mm <= max_miss_mm && !known {
                ghosts.push(Ghost {
                    view,
                    bounces,
                    pixel: pixel.to_array(),
                    miss_mm,
                });
            }
        }
    }
    ghosts.sort_by(|left, right| left.miss_mm.total_cmp(&right.miss_mm));
    ghosts
}
