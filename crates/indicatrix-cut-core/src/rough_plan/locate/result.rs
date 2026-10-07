//! From a located point to an inclusion of the rough.
//!
//! A located point becomes a small closed shell of a given radius in mm, in the rough's frame,
//! ready for [`add_inclusion_points`](crate::rough_plan::shape::hull::add_inclusion_points)
//! (stage A of the inclusion work). The margin that goes with it is the larger of the fixed
//! default and twice the measured uncertainty, so a poorly located inclusion keeps stones further
//! away.

use glam::DVec3;

use super::{
    shapes::sphere_shell,
    triangulate::{Located, LocatedPolyline},
};
use crate::rough_plan::shape::{HullError, MeshError, RoughBase, RoughMesh, hull};

/// The inclusion margin used when nothing better is known, in mm.
pub const DEFAULT_MARGIN_MM: f64 = 0.3;

/// The margin for an inclusion located with this RMS uncertainty: `max(0.3, 2 * rms)` in mm.
#[must_use]
pub const fn suggested_margin_mm(rms_mm: f64) -> f64 {
    let doubled = 2.0 * rms_mm;
    if doubled > DEFAULT_MARGIN_MM {
        doubled
    } else {
        DEFAULT_MARGIN_MM
    }
}

/// A closed shell for one inclusion and the margin to keep around it.
#[derive(Debug, Clone, PartialEq)]
pub struct InclusionShell {
    /// The shell's corners, in the rough's frame in mm.
    pub points: Vec<DVec3>,
    /// The shell's triangles, indexing the corners.
    pub triangles: Vec<[u32; 3]>,
    /// The margin to keep around it, in mm.
    pub margin_mm: f64,
}

impl InclusionShell {
    /// The shell as a mesh.
    ///
    /// # Errors
    ///
    /// [`MeshError`] when the shell is not a usable closed mesh.
    pub fn mesh(&self) -> Result<RoughMesh, MeshError> {
        RoughMesh::new(&self.points, &self.triangles)
    }
}

impl Located {
    /// The margin suggested for this point, `max(0.3, 2 * rms)` in mm.
    #[must_use]
    pub const fn suggested_margin_mm(&self) -> f64 {
        suggested_margin_mm(self.rms_mm)
    }

    /// The point as a small closed shell around it (an icosahedron whose faces touch a sphere of
    /// `radius_mm`), with the suggested margin.
    #[must_use]
    pub fn to_shell(&self, radius_mm: f64) -> InclusionShell {
        let (points, triangles) = sphere_shell(self.point_vec(), radius_mm);
        InclusionShell {
            points,
            triangles,
            margin_mm: self.suggested_margin_mm(),
        }
    }
}

impl LocatedPolyline {
    /// The margin suggested for a polyline: the one its least certain vertex asks for, in mm.
    #[must_use]
    pub fn suggested_margin_mm(&self) -> f64 {
        suggested_margin_mm(self.worst_rms_mm())
    }
}

/// Adds the located point to the rough `base` as an inclusion of the given radius, with the
/// suggested margin.
///
/// # Errors
///
/// [`HullError`] when the inclusion does not fit in the rough (it reaches the surface, or lies
/// outside), or the base is not an imported mesh; see
/// [`add_inclusion_points`](hull::add_inclusion_points).
pub fn add_located_point(
    base: &RoughBase,
    located: &Located,
    radius_mm: f64,
) -> Result<RoughBase, HullError> {
    let shell = located.to_shell(radius_mm);
    hull::add_inclusion_points(base, &shell.points, &shell.triangles, shell.margin_mm)
}
