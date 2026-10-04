//! The model-level geometry every shaped stage shares.
//!
//! Deriving the halfspaces, the non-box planes, the solid's volume and its
//! bounding box costs a vertex enumeration of the whole rough (about a tenth
//! of a second for a pebble). [`ShapedCtx`] does it once per plan and is passed
//! by reference to the grid, the DP, the layout builder, the uniform pass and
//! the refinement. For a rough with a non-convex mesh it also carries the mesh, which each
//! of them consults before accepting a stone.

use std::sync::Arc;

use glam::DVec3;

use super::clip::filter_non_box;
use crate::rough_plan::{FitMesh, PlanSettings, RoughMesh, RoughModel, ShapeError};

/// Geometry of one modelled rough, computed once per plan.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapedCtx {
    /// All halfspaces `(n, m)` of the model in canonical order (see
    /// [`RoughModel::canonical_halfspaces`]), inset by `skin + allowance`: the
    /// region a finished stone must lie in.
    pub usable: Vec<(DVec3, f64)>,
    /// The subset of `usable` that is not one of the six bounding-box faces of
    /// the base (cut, prism and pebble planes), in the same order.
    pub non_box: Vec<(DVec3, f64)>,
    /// Volume of the modelled solid in mm^3.
    pub model_volume: f64,
    /// Minimum corner of the solid's bounding box in the rough frame, in mm.
    pub bbox_min: [f64; 3],
    /// Extents of the solid's bounding box, in mm.
    pub bbox_extents: [f64; 3],
    /// Corner of the first sawn piece: `bbox_min` plus the skin.
    pub origin_mm: [f64; 3],
    /// The rough's non-convex mesh, `None` for a convex rough. The planes above are its
    /// convex hull (and the cuts); every placement is also checked against the mesh.
    pub mesh: Option<Arc<RoughMesh>>,
    /// `skin + allowance` in mm: the clearance a stone keeps from the mesh surface.
    pub inset_mm: f64,
}

impl ShapedCtx {
    /// Measures `model` once, through [`RoughModel::solid`].
    ///
    /// The volume, bounding box and plane order are those of [`RoughModel::measure`]
    /// and do not depend on the order the model's cuts were listed in.
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError`] if the model is invalid or leaves no material, or
    /// [`ShapeError::BadInset`] if `skin + allowance` is negative or not finite.
    pub fn new(model: &RoughModel, settings: &PlanSettings) -> Result<Self, ShapeError> {
        let inset = settings.skin_mm + settings.allowance_mm;
        let solid = model.solid()?;
        let usable = solid.usable_planes(inset)?;
        let non_box = filter_non_box(&usable, model.base.bounding_box_extents(), inset);
        Ok(Self {
            usable,
            non_box,
            model_volume: solid.volume_mm3,
            bbox_min: solid.bbox_min.to_array(),
            bbox_extents: solid.bbox_extents.to_array(),
            origin_mm: (solid.bbox_min + DVec3::splat(settings.skin_mm)).to_array(),
            mesh: model.mesh(),
            inset_mm: inset,
        })
    }

    /// The mesh with its clearance for the fit and the piece checks, `None` for a convex
    /// rough.
    #[must_use]
    pub fn fit_mesh(&self) -> Option<FitMesh<'_>> {
        self.mesh.as_deref().map(|mesh| FitMesh {
            mesh,
            inset_mm: self.inset_mm,
        })
    }
}
