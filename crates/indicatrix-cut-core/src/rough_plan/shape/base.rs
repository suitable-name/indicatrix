//! Base starting shapes for rough modelling.
//!
//! Provides the three starting shapes (block, cylinder, pebble), their conversion
//! to bounding halfspaces in the rough coordinate frame, and the vertices of those
//! polytopes, built analytically.

use std::sync::OnceLock;

use glam::DVec3;
use indicatrix::geometry::stone_metrics::measure_solid_with_vertices;

use super::{
    ShapeError, hull,
    pebble_offsets::{PEBBLE_OFFSETS_COARSE, PEBBLE_OFFSETS_FINE},
    sampling,
};
use crate::rough_plan::Axis;

/// Sides of the prism that stands in for a cylinder in the coarse (screening) region.
pub const COARSE_SIDES: usize = 16;
/// Sides of the prism that stands in for a cylinder in the fine region.
pub const FINE_SIDES: usize = 64;
/// Geodesic frequency of the pebble's coarse (screening) region: 42 planes.
pub const COARSE_PEBBLE_FREQUENCY: usize = 2;
/// Geodesic frequency of the pebble's fine region: 162 planes.
pub const FINE_PEBBLE_FREQUENCY: usize = 4;

/// The starting shape of the rough, before any cut. All lengths in mm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RoughBase {
    /// An axis-aligned block `x_mm x y_mm x z_mm`.
    Block {
        /// Extent along x in mm.
        x_mm: f64,
        /// Extent along y in mm.
        y_mm: f64,
        /// Extent along z in mm.
        z_mm: f64,
    },
    /// A cylinder of `diameter_mm` whose axis runs along `axis`, `length_mm` long.
    Cylinder {
        /// Cylinder diameter in mm.
        diameter_mm: f64,
        /// Cylinder length in mm along its axis.
        length_mm: f64,
        /// The axis the cylinder runs along.
        axis: Axis,
    },
    /// A water-worn pebble: the ellipsoid inscribed in the `x_mm x y_mm x z_mm` box.
    Pebble {
        /// Bounding box extent along x in mm.
        x_mm: f64,
        /// Bounding box extent along y in mm.
        y_mm: f64,
        /// Bounding box extent along z in mm.
        z_mm: f64,
    },
    /// The convex hull of an imported mesh, registered under `id` (see
    /// [`import_hull`](super::hull::import_hull)); the box is its bounding box.
    Hull {
        /// The content id the hull is registered under.
        id: u64,
        /// Bounding box extent along x in mm.
        x_mm: f64,
        /// Bounding box extent along y in mm.
        y_mm: f64,
        /// Bounding box extent along z in mm.
        z_mm: f64,
    },
}

impl RoughBase {
    /// Validates the base dimensions.
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError::NonPositiveSize`] if any dimension is non-positive or non-finite,
    /// [`ShapeError::TooLarge`] if any dimension exceeds 2000 mm, or
    /// [`ShapeError::NothingLeft`] for a hull whose id is not registered. The registration
    /// check reads the registry's table only; it never rebuilds a scaled copy that was
    /// dropped from memory.
    pub fn validate(&self) -> Result<(), ShapeError> {
        let (lens, count) = match *self {
            Self::Block { x_mm, y_mm, z_mm }
            | Self::Pebble { x_mm, y_mm, z_mm }
            | Self::Hull {
                x_mm, y_mm, z_mm, ..
            } => ([x_mm, y_mm, z_mm], 3),
            Self::Cylinder {
                diameter_mm,
                length_mm,
                ..
            } => ([diameter_mm, length_mm, 1.0], 2),
        };

        if let Self::Hull { id, .. } = *self
            && !hull::is_registered(id)
        {
            return Err(ShapeError::NothingLeft);
        }
        for &len in &lens[..count] {
            if !len.is_finite() || len <= 0.0 {
                return Err(ShapeError::NonPositiveSize);
            }
            if len > 2000.0 {
                return Err(ShapeError::TooLarge);
            }
        }
        Ok(())
    }

    /// Returns the bounding box extents `[x, y, z]` of the base shape in mm.
    #[must_use]
    pub const fn bounding_box_extents(&self) -> [f64; 3] {
        match *self {
            Self::Block { x_mm, y_mm, z_mm }
            | Self::Pebble { x_mm, y_mm, z_mm }
            | Self::Hull {
                x_mm, y_mm, z_mm, ..
            } => [x_mm, y_mm, z_mm],
            Self::Cylinder {
                diameter_mm,
                length_mm,
                axis,
            } => match axis {
                Axis::X => [length_mm, diameter_mm, diameter_mm],
                Axis::Y => [diameter_mm, length_mm, diameter_mm],
                Axis::Z => [diameter_mm, diameter_mm, length_mm],
            },
        }
    }

    /// Returns the bounding box centre of the base shape in the rough frame.
    #[must_use]
    pub fn bounding_box_centre(&self) -> DVec3 {
        let extents = self.bounding_box_extents();
        DVec3::new(extents[0] * 0.5, extents[1] * 0.5, extents[2] * 0.5)
    }

    /// The base with every length multiplied by `factor`.
    #[must_use]
    pub fn scaled(&self, factor: f64) -> Self {
        match *self {
            Self::Block { x_mm, y_mm, z_mm } => Self::Block {
                x_mm: x_mm * factor,
                y_mm: y_mm * factor,
                z_mm: z_mm * factor,
            },
            Self::Cylinder {
                diameter_mm,
                length_mm,
                axis,
            } => Self::Cylinder {
                diameter_mm: diameter_mm * factor,
                length_mm: length_mm * factor,
                axis,
            },
            Self::Pebble { x_mm, y_mm, z_mm } => Self::Pebble {
                x_mm: x_mm * factor,
                y_mm: y_mm * factor,
                z_mm: z_mm * factor,
            },
            Self::Hull { id, .. } => hull::scaled(*self, id, factor),
        }
    }

    /// Computes the outward halfspaces `n · p <= m` bounding the base shape.
    ///
    /// If `coarse` is true, uses a [`COARSE_SIDES`]-sided polygon for cylinders and the
    /// [`COARSE_PEBBLE_FREQUENCY`] direction set for pebbles; otherwise [`FINE_SIDES`] sides
    /// and the [`FINE_PEBBLE_FREQUENCY`] set.
    ///
    /// Both resolutions are inscribed in the true curved solid (their vertices lie on it),
    /// so the coarse polytope is smaller than the fine one: the coarse polygon's apothem is
    /// `r cos(pi / COARSE_SIDES)` against `r cos(pi / FINE_SIDES)`, a few percent less
    /// area. Neither contains the other exactly, because a coarse vertex sits on the true
    /// circle while the fine edge through that direction lies slightly inside it. A fit
    /// against the coarse region is therefore pessimistic on average, which is acceptable
    /// for ranking designs in the screening stage but is not a bound on the fine fit.
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError`] if dimensions are invalid or measuring the circumscribed polytope fails.
    pub fn to_halfspaces(&self, coarse: bool) -> Result<Vec<(DVec3, f64)>, ShapeError> {
        self.validate()?;
        match *self {
            Self::Block { x_mm, y_mm, z_mm } => Ok(block_halfspaces(x_mm, y_mm, z_mm)),
            Self::Cylinder {
                diameter_mm,
                length_mm,
                axis,
            } => {
                let sides = if coarse { COARSE_SIDES } else { FINE_SIDES };
                Ok(cylinder_halfspaces(diameter_mm, length_mm, axis, sides))
            }
            Self::Pebble { x_mm, y_mm, z_mm } => {
                let frequency = if coarse {
                    COARSE_PEBBLE_FREQUENCY
                } else {
                    FINE_PEBBLE_FREQUENCY
                };
                pebble_halfspaces(x_mm, y_mm, z_mm, frequency)
            }
            Self::Hull { id, .. } => hull::halfspaces(id).ok_or(ShapeError::NothingLeft),
        }
    }

    /// Returns the vertices, in the rough frame, of the polytope that
    /// [`to_halfspaces`](Self::to_halfspaces) bounds at the same `coarse` setting.
    ///
    /// They are built analytically, without enumerating plane triples: a block has its 8
    /// corners, a cylinder the `2 * sides` rim points of its two caps (each the meeting
    /// point of two neighbouring prism planes, at the half-step angle between their
    /// normals), and a pebble the cached vertices of the unit polytope mapped onto the
    /// ellipsoid. They differ from a vertex enumeration of the planes by rounding error
    /// only (a few ULPs), so anything positioned against them (a face cut) may differ from
    /// an enumeration-based result in the last bits.
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError`] if the dimensions are invalid, or
    /// [`ShapeError::NothingLeft`] if a pebble's unit polytope cannot be reconstructed.
    pub fn vertices(&self, coarse: bool) -> Result<Vec<DVec3>, ShapeError> {
        self.validate()?;
        match *self {
            Self::Block { x_mm, y_mm, z_mm } => Ok(block_vertices(x_mm, y_mm, z_mm)),
            Self::Cylinder {
                diameter_mm,
                length_mm,
                axis,
            } => {
                let sides = if coarse { COARSE_SIDES } else { FINE_SIDES };
                Ok(cylinder_vertices(diameter_mm, length_mm, axis, sides))
            }
            Self::Pebble { x_mm, y_mm, z_mm } => {
                let frequency = if coarse {
                    COARSE_PEBBLE_FREQUENCY
                } else {
                    FINE_PEBBLE_FREQUENCY
                };
                pebble_vertices(x_mm, y_mm, z_mm, frequency)
            }
            Self::Hull { id, .. } => hull::vertices(id).ok_or(ShapeError::NothingLeft),
        }
    }

    /// The corners of an imported hull as `[x, y, z]` triples (what a saved plan stores),
    /// `None` for any other base.
    #[must_use]
    pub fn hull_corners(&self) -> Option<Vec<[f64; 3]>> {
        let Self::Hull { id, .. } = *self else {
            return None;
        };
        hull::vertices(id).map(|v| v.iter().map(DVec3::to_array).collect())
    }
}

/// Generates the six bounding halfspaces of a block in the exact order:
/// +X, -X, +Y, -Y, +Z, -Z.
#[must_use]
pub fn block_halfspaces(x: f64, y: f64, z: f64) -> Vec<(DVec3, f64)> {
    vec![
        (DVec3::X, x),
        (-DVec3::X, 0.0),
        (DVec3::Y, y),
        (-DVec3::Y, 0.0),
        (DVec3::Z, z),
        (-DVec3::Z, 0.0),
    ]
}

/// The eight corners of the block `[0, x] x [0, y] x [0, z]`.
#[must_use]
pub fn block_vertices(x: f64, y: f64, z: f64) -> Vec<DVec3> {
    let mut corners = Vec::with_capacity(8);
    for cx in [0.0, x] {
        for cy in [0.0, y] {
            for cz in [0.0, z] {
                corners.push(DVec3::new(cx, cy, cz));
            }
        }
    }
    corners
}

/// The frame of a cylinder running along `axis`: `(axis direction, first cross direction,
/// second cross direction, centre of the low cap)` for radius `r`, in the rough frame where
/// the cylinder's bounding box starts at the origin.
const fn cylinder_frame(axis: Axis, r: f64) -> (DVec3, DVec3, DVec3, DVec3) {
    match axis {
        Axis::X => (DVec3::X, DVec3::Y, DVec3::Z, DVec3::new(0.0, r, r)),
        Axis::Y => (DVec3::Y, DVec3::Z, DVec3::X, DVec3::new(r, 0.0, r)),
        Axis::Z => (DVec3::Z, DVec3::X, DVec3::Y, DVec3::new(r, r, 0.0)),
    }
}

/// Generates the bounding halfspaces of an inscribed cylinder prism.
///
/// Returns two end caps (+axis, then -axis), followed by `sides` prism halfspaces
/// using normals from [`sampling::unit_circle`].
#[must_use]
pub fn cylinder_halfspaces(
    diameter_mm: f64,
    length_mm: f64,
    axis: Axis,
    sides: usize,
) -> Vec<(DVec3, f64)> {
    let mut planes = Vec::with_capacity(2 + sides);
    let r = diameter_mm * 0.5;
    let (e_axis, e1, e2, centre) = cylinder_frame(axis, r);

    // Two end caps: +axis then -axis
    planes.push((e_axis, length_mm));
    planes.push((-e_axis, 0.0));

    let cos_half = sampling::half_step_cos(sides);
    let circle = sampling::unit_circle(sides);

    for [c, s] in circle {
        let normal = c * e1 + s * e2;
        let offset_m = r.mul_add(cos_half, normal.dot(centre));
        planes.push((normal, offset_m));
    }

    planes
}

/// The `2 * sides` vertices of the prism [`cylinder_halfspaces`] bounds with the same
/// arguments: the rim points of both caps.
///
/// Prism plane `k` has its normal at angle `2 pi k / sides` and apothem `r cos(pi / sides)`,
/// so two neighbouring planes meet on the circle of radius `r` at the angle halfway between
/// their normals, `pi (2 k + 1) / sides`. Those are the odd points of
/// [`sampling::unit_circle`] for `2 * sides` points. Returned per rim point: the low-cap
/// vertex, then the high-cap vertex.
#[must_use]
pub fn cylinder_vertices(diameter_mm: f64, length_mm: f64, axis: Axis, sides: usize) -> Vec<DVec3> {
    let r = diameter_mm * 0.5;
    let (e_axis, e1, e2, centre) = cylinder_frame(axis, r);
    let circle = sampling::unit_circle(2 * sides);
    let mut rim = Vec::with_capacity(2 * sides);
    for &[c, s] in circle.iter().skip(1).step_by(2) {
        let low = centre + r * (c * e1 + s * e2);
        rim.push(low);
        rim.push(low + length_mm * e_axis);
    }
    rim
}

/// The unit polytope `{ u : d_j . u <= h_j }` over the geodesic directions `d_j` of one
/// frequency, with the pinned per-face offsets `h_j`.
#[derive(Debug, Clone)]
struct UnitPebble {
    /// The offset `h_j` of the plane with direction `j` of [`sampling::pebble_directions`].
    offsets: Vec<f64>,
    /// The polytope's vertices, all inside the closed unit ball.
    vertices: Vec<DVec3>,
}

/// The [`COARSE_PEBBLE_FREQUENCY`] unit polytope, computed on first use.
static UNIT_COARSE: OnceLock<Option<UnitPebble>> = OnceLock::new();
/// The [`FINE_PEBBLE_FREQUENCY`] unit polytope, computed on first use.
static UNIT_FINE: OnceLock<Option<UnitPebble>> = OnceLock::new();

/// The unit polytope of a planner `frequency`, computed once and cached; any other
/// frequency has no pinned offsets and gives `None`.
fn unit_pebble(frequency: usize) -> Option<&'static UnitPebble> {
    match frequency {
        COARSE_PEBBLE_FREQUENCY => UNIT_COARSE
            .get_or_init(|| compute_unit_pebble(COARSE_PEBBLE_FREQUENCY, &PEBBLE_OFFSETS_COARSE))
            .as_ref(),
        FINE_PEBBLE_FREQUENCY => UNIT_FINE
            .get_or_init(|| compute_unit_pebble(FINE_PEBBLE_FREQUENCY, &PEBBLE_OFFSETS_FINE))
            .as_ref(),
        _ => None,
    }
}

/// Builds the unit polytope with `offsets` over the directions of `frequency`.
///
/// Returns `None` when the offsets do not match the direction count or the polytope cannot
/// be reconstructed.
fn compute_unit_pebble(frequency: usize, offsets: &[f64]) -> Option<UnitPebble> {
    let dirs = sampling::pebble_directions(frequency);
    if dirs.len() != offsets.len() {
        return None;
    }
    let unit_planes: Vec<(DVec3, f64)> = dirs
        .into_iter()
        .zip(offsets)
        .map(|([x, y, z], &h)| (DVec3::new(x, y, z), h))
        .collect();

    let (_, vertices) = measure_solid_with_vertices(&unit_planes)?;
    Some(UnitPebble {
        offsets: offsets.to_vec(),
        vertices,
    })
}

/// Generates the conservative inscribed pebble halfspaces at geodesic `frequency`.
///
/// The polytope is the unit one of the pinned offsets mapped linearly onto the ellipsoid
/// inscribed in the `x_mm x y_mm x z_mm` box, so it lies inside the ellipsoid and its
/// volume is the same fraction of the ellipsoid's as the unit polytope's is of the unit
/// ball's. Only [`COARSE_PEBBLE_FREQUENCY`] and [`FINE_PEBBLE_FREQUENCY`] have pinned
/// offsets.
///
/// # Errors
///
/// Returns [`ShapeError::NothingLeft`] if `frequency` has no pinned offsets or measuring the
/// unit polytope fails.
pub fn pebble_halfspaces(
    x_mm: f64,
    y_mm: f64,
    z_mm: f64,
    frequency: usize,
) -> Result<Vec<(DVec3, f64)>, ShapeError> {
    let semi = DVec3::new(x_mm * 0.5, y_mm * 0.5, z_mm * 0.5);
    let centre = semi;

    let unit = unit_pebble(frequency).ok_or(ShapeError::NothingLeft)?;
    let dirs = sampling::pebble_directions(frequency);

    let mut planes = Vec::with_capacity(dirs.len());
    for (&[dx, dy, dz], &h) in dirs.iter().zip(&unit.offsets) {
        let v_inv = DVec3::new(dx, dy, dz) / semi;
        let len = v_inv.length();
        let normal = v_inv / len;
        let offset_m = normal.dot(centre) + h / len;
        planes.push((normal, offset_m));
    }

    Ok(planes)
}

/// The vertices of the polytope [`pebble_halfspaces`] bounds with the same arguments.
///
/// Plane `d_j` of the pebble is `d_j . u <= h_j` in the unit frame `u = (p - centre) / semi`,
/// so the pebble's vertices are the cached unit-polytope vertices `v` mapped to
/// `centre + semi * v`, with `semi` the half extents and `centre` the box centre.
///
/// # Errors
///
/// Returns [`ShapeError::NothingLeft`] if `frequency` has no pinned offsets or the unit
/// polytope cannot be reconstructed.
pub fn pebble_vertices(
    x_mm: f64,
    y_mm: f64,
    z_mm: f64,
    frequency: usize,
) -> Result<Vec<DVec3>, ShapeError> {
    let semi = DVec3::new(x_mm * 0.5, y_mm * 0.5, z_mm * 0.5);
    let unit = unit_pebble(frequency).ok_or(ShapeError::NothingLeft)?;
    Ok(unit.vertices.iter().map(|&v| semi + semi * v).collect())
}
