//! Geometric rough model: base shapes, cuts, and solid measurements.
//!
//! Provides the [`RoughModel`] representation of unworked or preformed gem rough,
//! supporting blocks, cylinders, and water-worn pebbles with flat chamfers, corner cuts,
//! and arbitrary face cuts.

pub mod base;
pub mod cuts;
pub mod hull;
pub mod mesh;
#[cfg(test)]
pub(crate) mod mesh_fixture;
mod pebble_offsets;
pub mod sampling;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_geometry;
#[cfg(test)]
mod tests_hull;
#[cfg(test)]
mod tests_pebble;
#[cfg(test)]
mod tests_validation;

use std::{cmp::Ordering, fmt, sync::Arc};

use glam::DVec3;
use indicatrix::geometry::stone_metrics::measure_solid_with_vertices;

pub use base::RoughBase;
pub use cuts::{BoxFace, RoughCut};
pub use hull::{HullError, MAX_HULL_PLANES, import_hull, import_mesh};
pub use mesh::{BoxState, ClippedSurface, MAX_MESH_TRIANGLES, MeshError, RoughMesh, SurfaceCap};
pub use sampling::{half_step_cos, pebble_directions, sphere_directions, unit_circle};

/// The modelled rough: a starting base shape plus an ordered sequence of planar cuts.
#[derive(Debug, Clone, PartialEq)]
pub struct RoughModel {
    /// The starting unworked base shape.
    pub base: RoughBase,
    /// The planar cuts applied to the base shape.
    pub cuts: Vec<RoughCut>,
}

/// Physical measurements of the modelled rough solid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoughMeasure {
    /// Total volume of the cut polytope in cubic millimetres.
    pub volume_mm3: f64,
    /// Real bounding box extents `[x, y, z]` of the cut polytope in mm.
    pub extents_mm: [f64; 3],
    /// Total number of halfspace planes bounding the solid.
    pub plane_count: usize,
    /// Number of feasible 3D vertices forming the solid's convex polyhedron.
    pub vertex_count: usize,
}

/// Reasons why a rough model is invalid or physically unusable.
#[derive(Debug, Clone, PartialEq)]
pub enum ShapeError {
    /// A base length is less than or equal to 0 or not finite.
    NonPositiveSize,
    /// A base length exceeds 2000 mm.
    TooLarge,
    /// Edge or corner cut attempted on a non-block base.
    CutNotForBase {
        /// 0-based index of the invalid cut.
        index: usize,
    },
    /// Faces specified for an edge or corner cut are invalid or repeated.
    BadFaces {
        /// 0-based index of the invalid cut.
        index: usize,
    },
    /// Setback is non-positive, not finite, or exceeds the face extent.
    BadSetback {
        /// 0-based index of the invalid cut.
        index: usize,
        /// The offending setback in mm as given.
        setback_mm: f64,
        /// The length of the face the setback is measured along, in mm.
        face_mm: f64,
    },
    /// Cut face normal is zero or non-finite.
    BadNormal {
        /// 0-based index of the invalid cut.
        index: usize,
    },
    /// Depth is non-positive, not finite, or cuts through the entire rough.
    BadDepth {
        /// 0-based index of the invalid cut.
        index: usize,
        /// The offending depth in mm as given.
        depth_mm: f64,
        /// The rough's thickness along the cut normal in mm, when it was measured
        /// (`None` when the depth was rejected before the base was examined).
        thickness_mm: Option<f64>,
    },
    /// The cuts leave no remaining material, or only a sliver (less than a
    /// billionth of the base bounding box).
    NothingLeft,
    /// The inset asked of the usable region is negative or not finite.
    BadInset {
        /// The offending inset in mm as given.
        inset_mm: f64,
    },
}

impl fmt::Display for ShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NonPositiveSize => write!(f, "All dimensions must be positive and finite."),
            Self::TooLarge => write!(f, "Dimensions must not exceed 2000 mm."),
            Self::CutNotForBase { index } => {
                write!(
                    f,
                    "Cut {}: edge and corner cuts are only supported on block rough.",
                    index + 1
                )
            }
            Self::BadFaces { index } => {
                write!(f, "Cut {}: faces must be on different axes.", index + 1)
            }
            Self::BadSetback {
                index,
                setback_mm,
                face_mm,
            } => {
                if setback_mm.is_finite() && setback_mm > 0.0 {
                    write!(
                        f,
                        "Cut {}: the setback {setback_mm:.1} mm is longer than the face ({face_mm:.1} mm).",
                        index + 1
                    )
                } else {
                    write!(
                        f,
                        "Cut {}: the setback ({setback_mm}) must be a positive, finite length.",
                        index + 1
                    )
                }
            }
            Self::BadNormal { index } => {
                write!(
                    f,
                    "Cut {}: normal vector must be non-zero and finite.",
                    index + 1
                )
            }
            Self::BadDepth {
                index,
                depth_mm,
                thickness_mm,
            } => match thickness_mm {
                Some(thickness) if depth_mm.is_finite() && depth_mm > 0.0 => write!(
                    f,
                    "Cut {}: the depth {depth_mm:.1} mm reaches through the rough ({thickness:.1} mm thick).",
                    index + 1
                ),
                _ => write!(
                    f,
                    "Cut {}: the depth ({depth_mm}) must be a positive, finite length.",
                    index + 1
                ),
            },
            Self::NothingLeft => write!(f, "The cuts remove all material; nothing is left."),
            Self::BadInset { inset_mm } => write!(
                f,
                "The inset ({inset_mm}) must be a non-negative, finite length."
            ),
        }
    }
}

impl std::error::Error for ShapeError {}

/// The smallest fraction of the base bounding box's volume a modelled solid may fill.
///
/// Anything thinner is a sliver left by cuts that all but meet, not a stone-sized piece of
/// rough; the planes that bound it are also too close for the vertex enumeration to
/// resolve reliably.
const SLIVER_FRACTION: f64 = 1e-9;

/// Whether a solid of `volume` mm^3 is a sliver of a base with bounding box `extents`.
const fn is_sliver(volume: f64, extents: [f64; 3]) -> bool {
    volume < SLIVER_FRACTION * (extents[0] * extents[1] * extents[2])
}

/// The total order on planes `(n, m)` by `(nx, ny, nz, m)` that makes plane lists
/// independent of the order the cuts were given in.
fn plane_order(a: &(DVec3, f64), b: &(DVec3, f64)) -> Ordering {
    a.0.x
        .total_cmp(&b.0.x)
        .then(a.0.y.total_cmp(&b.0.y))
        .then(a.0.z.total_cmp(&b.0.z))
        .then(a.1.total_cmp(&b.1))
}

/// Translates `planes` so `centre` becomes the origin and sorts them by [`plane_order`].
///
/// The polytope measurement sums per-plane terms and walks vertex rings whose start depends
/// on the plane order, so the last bits of its result depend on that order. Sorting first
/// makes equal plane sets (for example the same cuts listed in a different order) give
/// bit-identical measurements.
fn centred_sorted(planes: &[(DVec3, f64)], centre: DVec3) -> Vec<(DVec3, f64)> {
    let mut out: Vec<(DVec3, f64)> = planes
        .iter()
        .map(|&(n, m)| (n, m - n.dot(centre)))
        .collect();
    out.sort_by(plane_order);
    out
}

/// Moves every plane of `planes` inward by `inset_mm` (`m - inset_mm`).
///
/// # Errors
///
/// Returns [`ShapeError::BadInset`] if `inset_mm` is negative or not finite: a negative
/// inset would grow the region past the rough.
fn inset_planes(
    mut planes: Vec<(DVec3, f64)>,
    inset_mm: f64,
) -> Result<Vec<(DVec3, f64)>, ShapeError> {
    if !(inset_mm.is_finite() && inset_mm >= 0.0) {
        return Err(ShapeError::BadInset { inset_mm });
    }
    for (_, m) in &mut planes {
        *m -= inset_mm;
    }
    Ok(planes)
}

/// The cut rough polytope, measured once.
///
/// Every number comes from one vertex enumeration of the planes in canonical order (see
/// [`RoughModel::canonical_halfspaces`]), so equal cut sets give bit-identical values
/// whatever order the cuts were listed in.
#[derive(Debug, Clone, PartialEq)]
pub struct RoughSolid {
    /// The bounding halfspaces `(n, m)` (`n · p <= m`) in canonical order, in the rough frame.
    pub planes: Vec<(DVec3, f64)>,
    /// Volume of the polytope in cubic millimetres.
    pub volume_mm3: f64,
    /// Minimum corner of the polytope's bounding box in the rough frame, in mm.
    pub bbox_min: DVec3,
    /// Extents of the polytope's bounding box in mm.
    pub bbox_extents: DVec3,
    /// Number of feasible 3D vertices of the polytope.
    pub vertex_count: usize,
}

impl RoughSolid {
    /// The polytope's planes in canonical order, moved inward by `inset_mm` (`m - inset_mm`).
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError::BadInset`] if `inset_mm` is negative or not finite.
    pub fn usable_planes(&self, inset_mm: f64) -> Result<Vec<(DVec3, f64)>, ShapeError> {
        inset_planes(self.planes.clone(), inset_mm)
    }
}

impl RoughModel {
    /// The non-convex mesh the rough is made of, `None` for every convex rough (box,
    /// cylinder, pebble, hull, and an imported mesh that is its own hull).
    ///
    /// The planner works from the convex polytope of [`halfspaces`](Self::halfspaces) and
    /// asks the mesh only to reject or shrink what reaches into air.
    #[must_use]
    pub fn mesh(&self) -> Option<Arc<RoughMesh>> {
        match self.base {
            RoughBase::Hull { id, .. } => hull::mesh(id),
            _ => None,
        }
    }

    /// Constructs a new model from a base shape and a list of cuts.
    #[must_use]
    pub const fn new(base: RoughBase, cuts: Vec<RoughCut>) -> Self {
        Self { base, cuts }
    }

    /// The model with every length multiplied by `factor`: the base sizes, the edge and
    /// corner setbacks and the face depths; face normals stay. Fitting a rough to its
    /// weighed carat scales by the cube root of the carat ratio, a factor close to 1.
    #[must_use]
    pub fn scaled(&self, factor: f64) -> Self {
        Self {
            base: self.base.scaled(factor),
            cuts: self.cuts.iter().map(|cut| cut.scaled(factor)).collect(),
        }
    }

    /// Computes all bounding halfspaces `(n, m)` (`n · p <= m`) of the rough model in cut order.
    ///
    /// Returns the base halfspaces followed by one plane per cut. The order is the model's
    /// own (mesh facet ids index it); use [`canonical_halfspaces`](Self::canonical_halfspaces)
    /// where the result must not depend on the order the cuts were listed in.
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError`] if dimensions or cuts are invalid.
    pub fn halfspaces(&self) -> Result<Vec<(DVec3, f64)>, ShapeError> {
        self.halfspaces_internal(false)
    }

    /// Computes coarse bounding halfspaces suitable for fast screening.
    ///
    /// The cut planes are the same as in [`halfspaces`](Self::halfspaces): they are
    /// positioned against the fine base, then appended to the coarse base planes.
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError`] if dimensions or cuts are invalid.
    pub fn coarse_halfspaces(&self) -> Result<Vec<(DVec3, f64)>, ShapeError> {
        self.halfspaces_internal(true)
    }

    /// [`halfspaces`](Self::halfspaces) sorted into the canonical order: ascending by
    /// `(nx, ny, nz, m)` under the IEEE total order.
    ///
    /// Two models with the same planes in a different cut order give bit-identical lists, so
    /// anything that sums over the planes or depends on their order (the LP, the fit, the
    /// measurement) does too.
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError`] if dimensions or cuts are invalid.
    pub fn canonical_halfspaces(&self) -> Result<Vec<(DVec3, f64)>, ShapeError> {
        let mut planes = self.halfspaces()?;
        planes.sort_by(plane_order);
        Ok(planes)
    }

    /// [`coarse_halfspaces`](Self::coarse_halfspaces) in the canonical order of
    /// [`canonical_halfspaces`](Self::canonical_halfspaces).
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError`] if dimensions or cuts are invalid.
    pub fn canonical_coarse_halfspaces(&self) -> Result<Vec<(DVec3, f64)>, ShapeError> {
        let mut planes = self.coarse_halfspaces()?;
        planes.sort_by(plane_order);
        Ok(planes)
    }

    /// Computes the usable halfspaces inset inward by `inset_mm` (`m - inset_mm`), in cut order.
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError::BadInset`] if `inset_mm` is negative or not finite, or another
    /// [`ShapeError`] if halfspace generation fails.
    pub fn usable_halfspaces(&self, inset_mm: f64) -> Result<Vec<(DVec3, f64)>, ShapeError> {
        inset_planes(self.halfspaces()?, inset_mm)
    }

    /// Computes coarse usable halfspaces inset inward by `inset_mm`, in cut order.
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError::BadInset`] if `inset_mm` is negative or not finite, or another
    /// [`ShapeError`] if coarse halfspace generation fails.
    pub fn coarse_usable_halfspaces(&self, inset_mm: f64) -> Result<Vec<(DVec3, f64)>, ShapeError> {
        inset_planes(self.coarse_halfspaces()?, inset_mm)
    }

    /// [`usable_halfspaces`](Self::usable_halfspaces) in the canonical order of
    /// [`canonical_halfspaces`](Self::canonical_halfspaces).
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError::BadInset`] if `inset_mm` is negative or not finite, or another
    /// [`ShapeError`] if halfspace generation fails.
    pub fn canonical_usable_halfspaces(
        &self,
        inset_mm: f64,
    ) -> Result<Vec<(DVec3, f64)>, ShapeError> {
        inset_planes(self.canonical_halfspaces()?, inset_mm)
    }

    /// [`coarse_usable_halfspaces`](Self::coarse_usable_halfspaces) in the canonical order of
    /// [`canonical_halfspaces`](Self::canonical_halfspaces).
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError::BadInset`] if `inset_mm` is negative or not finite, or another
    /// [`ShapeError`] if coarse halfspace generation fails.
    pub fn canonical_coarse_usable_halfspaces(
        &self,
        inset_mm: f64,
    ) -> Result<Vec<(DVec3, f64)>, ShapeError> {
        inset_planes(self.canonical_coarse_halfspaces()?, inset_mm)
    }

    /// Measures the cut rough polytope once: volume, bounding box, vertex count and the
    /// canonical planes.
    ///
    /// Translates the canonical planes to the base centre before invoking vertex
    /// measurement. [`measure`](Self::measure) and the shaped planner's context both read
    /// their numbers from here, so they agree to the bit.
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError::NothingLeft`] if the solid is empty or a sliver (its volume
    /// under a billionth of the base bounding box's), or another [`ShapeError`] if model
    /// halfspaces cannot be generated.
    pub fn solid(&self) -> Result<RoughSolid, ShapeError> {
        self.solid_of_canonical(self.canonical_halfspaces()?)
    }

    /// The solid bounded by `planes`, which must already be in the canonical order of
    /// [`canonical_halfspaces`](Self::canonical_halfspaces). The one body behind
    /// [`solid`](Self::solid), [`measure`](Self::measure) and
    /// [`measure_with_halfspaces`](Self::measure_with_halfspaces).
    fn solid_of_canonical(&self, planes: Vec<(DVec3, f64)>) -> Result<RoughSolid, ShapeError> {
        let c = self.base.bounding_box_centre();
        let trans_planes = centred_sorted(&planes, c);

        let (metrics, trans_verts) =
            measure_solid_with_vertices(&trans_planes).ok_or(ShapeError::NothingLeft)?;
        if trans_verts.is_empty()
            || metrics.volume <= 0.0
            || !metrics.volume.is_finite()
            || is_sliver(metrics.volume, self.base.bounding_box_extents())
        {
            return Err(ShapeError::NothingLeft);
        }

        let mut min_p = DVec3::splat(f64::INFINITY);
        let mut max_p = DVec3::splat(f64::NEG_INFINITY);
        for &v in &trans_verts {
            let p = v + c;
            min_p = min_p.min(p);
            max_p = max_p.max(p);
        }

        // A mesh rough's volume is the mesh's, within the cuts; the polytope is the hull.
        let volume_mm3 = match self.mesh() {
            Some(mesh) => {
                let volume = mesh.volume_within(&self.cut_planes()?);
                if !(volume.is_finite() && volume > 0.0) {
                    return Err(ShapeError::NothingLeft);
                }
                volume
            }
            None => metrics.volume,
        };

        Ok(RoughSolid {
            planes,
            volume_mm3,
            bbox_min: min_p,
            bbox_extents: max_p - min_p,
            vertex_count: trans_verts.len(),
        })
    }

    /// Measures the physical properties of the cut rough polytope.
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError::NothingLeft`] if the solid is empty or a sliver, or another
    /// [`ShapeError`] if model halfspaces cannot be generated.
    pub fn measure(&self) -> Result<RoughMeasure, ShapeError> {
        self.measure_with_halfspaces(&self.halfspaces()?)
    }

    /// Measures the polytope of an already computed [`halfspaces`](Self::halfspaces)
    /// list, so a caller that needs the planes for something else (a mesh) computes them
    /// once.
    ///
    /// `planes` must be this model's own `halfspaces()` output (cut order or canonical
    /// order; it is put into canonical order here). The result is bit-identical to
    /// [`measure`](Self::measure), which is defined as this call.
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError::NothingLeft`] if the solid is empty or a sliver.
    pub fn measure_with_halfspaces(
        &self,
        planes: &[(DVec3, f64)],
    ) -> Result<RoughMeasure, ShapeError> {
        let mut canonical = planes.to_vec();
        canonical.sort_by(plane_order);
        let solid = self.solid_of_canonical(canonical)?;
        Ok(RoughMeasure {
            volume_mm3: solid.volume_mm3,
            extents_mm: solid.bbox_extents.to_array(),
            plane_count: solid.planes.len(),
            vertex_count: solid.vertex_count,
        })
    }

    /// The base planes at the requested resolution followed by the cut planes.
    ///
    /// The cut planes do not depend on `coarse`: they are positioned once against the fine
    /// base (see [`cut_planes`](Self::cut_planes)), so a cut valid for the fine model is
    /// valid for the coarse region too.
    fn halfspaces_internal(&self, coarse: bool) -> Result<Vec<(DVec3, f64)>, ShapeError> {
        let mut planes = self.base.to_halfspaces(coarse)?;
        planes.extend(self.cut_planes()?);
        Ok(planes)
    }

    /// One plane per cut, in cut order, positioned against the fine base's vertices.
    fn cut_planes(&self) -> Result<Vec<(DVec3, f64)>, ShapeError> {
        let has_face_cut = self.cuts.iter().any(|c| matches!(c, RoughCut::Face { .. }));
        let base_verts = if has_face_cut {
            self.base.vertices(false)?
        } else {
            Vec::new()
        };
        self.cuts
            .iter()
            .enumerate()
            .map(|(index, cut)| cut.to_halfspace(index, &self.base, &base_verts))
            .collect()
    }
}
