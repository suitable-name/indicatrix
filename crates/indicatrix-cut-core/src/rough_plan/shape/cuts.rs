//! Cut definitions and conversion to bounding halfspaces.
//!
//! Provides the three cut types (edge chamfer, corner cut, planar face cut)
//! and their conversion to halfspaces in the rough coordinate frame.

use glam::DVec3;

use super::{RoughBase, ShapeError};

/// A face of the base's bounding box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxFace {
    /// The top face (+Y).
    Top,
    /// The bottom face (-Y).
    Bottom,
    /// The right face (+X).
    Right,
    /// The left face (-X).
    Left,
    /// The front face (+Z).
    Front,
    /// The back face (-Z).
    Back,
}

impl BoxFace {
    /// Returns `(axis_index, is_high_side)`.
    ///
    /// Axis indices: `0 = X`, `1 = Y`, `2 = Z`.
    #[must_use]
    pub const fn axis_and_side(self) -> (usize, bool) {
        match self {
            Self::Right => (0, true),
            Self::Left => (0, false),
            Self::Top => (1, true),
            Self::Bottom => (1, false),
            Self::Front => (2, true),
            Self::Back => (2, false),
        }
    }
}

/// One cut that removes material with a single plane.
#[derive(Debug, Clone, PartialEq)]
pub enum RoughCut {
    /// Chamfer the bounding-box edge shared by `faces` (two adjacent faces). `setbacks_mm[i]`
    /// is measured from the edge along `faces[i]`. Block only.
    Edge {
        /// The two adjacent faces defining the edge.
        faces: [BoxFace; 2],
        /// The setbacks in mm measured along each face.
        setbacks_mm: [f64; 2],
    },
    /// Cut the bounding-box corner shared by `faces` (three faces on three different
    /// axes). `setbacks_mm[i]` is the length cut off the corner along the axis that
    /// `faces[i]` is perpendicular to. Block only.
    Corner {
        /// The three faces meeting at the corner.
        faces: [BoxFace; 3],
        /// The setbacks in mm along each face's perpendicular axis.
        setbacks_mm: [f64; 3],
    },
    /// A flat face (a sawn or broken surface) in any direction: `normal` is the unit
    /// outward normal of the new face, `depth_mm` how far it reaches into the base shape,
    /// measured from the base's outermost point in that direction. Every base.
    Face {
        /// The outward normal vector of the cut face.
        normal: [f64; 3],
        /// How far the cut penetrates into the base shape in mm.
        depth_mm: f64,
    },
}

impl RoughCut {
    /// The cut with every length multiplied by `factor`: the setbacks and the depth
    /// scale, a face normal does not.
    #[must_use]
    pub fn scaled(&self, factor: f64) -> Self {
        match *self {
            Self::Edge { faces, setbacks_mm } => Self::Edge {
                faces,
                setbacks_mm: setbacks_mm.map(|setback| setback * factor),
            },
            Self::Corner { faces, setbacks_mm } => Self::Corner {
                faces,
                setbacks_mm: setbacks_mm.map(|setback| setback * factor),
            },
            Self::Face { normal, depth_mm } => Self::Face {
                normal,
                depth_mm: depth_mm * factor,
            },
        }
    }

    /// Validates the cut against `base` and computes its outward halfspace `n · p <= m`.
    ///
    /// `base_verts` must contain the vertices of the base polytope in the rough frame,
    /// used to evaluate support for [`RoughCut::Face`]; [`RoughBase::vertices`] with
    /// `coarse == false` gives them. They are ignored by edge and corner cuts. A face cut
    /// is positioned against these vertices only, so passing the fine polytope's vertices
    /// gives the same plane whichever resolution of the base it is then combined with.
    ///
    /// # Errors
    ///
    /// Returns [`ShapeError`] if the cut is incompatible with the base shape, if face
    /// selections are invalid, or if setbacks/depths are out of range, and
    /// [`ShapeError::NothingLeft`] if a face cut is given no base vertices.
    pub fn to_halfspace(
        &self,
        index: usize,
        base: &RoughBase,
        base_verts: &[DVec3],
    ) -> Result<(DVec3, f64), ShapeError> {
        let extents = base.bounding_box_extents();
        match self {
            Self::Edge { faces, setbacks_mm } => {
                if !matches!(base, RoughBase::Block { .. }) {
                    return Err(ShapeError::CutNotForBase { index });
                }
                let (k1, side1) = faces[0].axis_and_side();
                let (k2, side2) = faces[1].axis_and_side();
                if k1 == k2 {
                    return Err(ShapeError::BadFaces { index });
                }
                let s1 = setbacks_mm[0];
                let s2 = setbacks_mm[1];
                check_setback(index, s1, extents[k2])?;
                check_setback(index, s2, extents[k1])?;
                let terms = [(k2, side2, s1), (k1, side1, s2)];
                Ok(plane_from_intercepts(&terms, extents))
            }
            Self::Corner { faces, setbacks_mm } => {
                if !matches!(base, RoughBase::Block { .. }) {
                    return Err(ShapeError::CutNotForBase { index });
                }
                let (k0, side0) = faces[0].axis_and_side();
                let (k1, side1) = faces[1].axis_and_side();
                let (k2, side2) = faces[2].axis_and_side();
                if k0 == k1 || k1 == k2 || k0 == k2 {
                    return Err(ShapeError::BadFaces { index });
                }
                let axes_sides = [(k0, side0), (k1, side1), (k2, side2)];
                for (i, &(k, _)) in axes_sides.iter().enumerate() {
                    check_setback(index, setbacks_mm[i], extents[k])?;
                }
                let terms = [
                    (k0, side0, setbacks_mm[0]),
                    (k1, side1, setbacks_mm[1]),
                    (k2, side2, setbacks_mm[2]),
                ];
                Ok(plane_from_intercepts(&terms, extents))
            }
            Self::Face { normal, depth_mm } => {
                let n_raw = DVec3::new(normal[0], normal[1], normal[2]);
                let len = n_raw.length();
                if !len.is_finite() || len <= 1e-9 {
                    return Err(ShapeError::BadNormal { index });
                }
                let n = n_raw / len;
                if !depth_mm.is_finite() || *depth_mm <= 0.0 {
                    return Err(ShapeError::BadDepth {
                        index,
                        depth_mm: *depth_mm,
                        thickness_mm: None,
                    });
                }

                if base_verts.is_empty() {
                    return Err(ShapeError::NothingLeft);
                }
                let mut h_base = f64::NEG_INFINITY;
                let mut min_base = f64::INFINITY;
                for v in base_verts {
                    let dot = n.dot(*v);
                    h_base = h_base.max(dot);
                    min_base = min_base.min(dot);
                }
                let thickness = h_base - min_base;
                if *depth_mm >= thickness {
                    return Err(ShapeError::BadDepth {
                        index,
                        depth_mm: *depth_mm,
                        thickness_mm: Some(thickness),
                    });
                }

                let m = h_base - *depth_mm;
                Ok((n, m))
            }
        }
    }
}

/// Checks that `setback_mm` is a positive, finite length no longer than the face it is
/// measured along (`face_mm`).
fn check_setback(index: usize, setback_mm: f64, face_mm: f64) -> Result<(), ShapeError> {
    if setback_mm.is_finite() && setback_mm > 0.0 && setback_mm <= face_mm {
        Ok(())
    } else {
        Err(ShapeError::BadSetback {
            index,
            setback_mm,
            face_mm,
        })
    }
}

/// Computes the outward halfspace `n · p <= m` from intercept terms on bounding box faces.
///
/// Each term specifies `(axis, is_high_side, setback)`. The kept region satisfies:
/// `sum_t (u_{k_t} / s_t) >= 1`.
pub(crate) fn plane_from_intercepts(
    terms: &[(usize, bool, f64)],
    extents: [f64; 3],
) -> (DVec3, f64) {
    let mut a = DVec3::ZERO;
    let mut c = 0.0;

    for &(axis, is_high, setback) in terms {
        let (sigma, c_k) = if is_high {
            (-1.0, extents[axis])
        } else {
            (1.0, 0.0)
        };
        let coeff = sigma / setback;
        match axis {
            0 => a.x += coeff,
            1 => a.y += coeff,
            2 => a.z += coeff,
            _ => unreachable!(),
        }
        c += c_k / setback;
    }

    let len = a.length();
    let n = -a / len;
    let m = (c - 1.0) / len;
    (n, m)
}
