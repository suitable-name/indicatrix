//! Inclusions of a mesh rough, in the registry: adding and removing them, listing them, and
//! reading a saved rough back with them.
//!
//! An inclusion is a closed shell inside the rough (see the mesh module's `inclusion`
//! module). The base that names the rough carries a registry id, and the id hashes which
//! shells are inclusions, so adding one gives a new base and the old one stays valid: an
//! undo step that names it still resolves. Everything here works on the base, so an app adds
//! an inclusion from a file (parsed to points and triangles in millimetres) with
//! [`add_inclusion_points`], or from a mesh it already has with [`add_inclusion_mesh`], with
//! no file path anywhere.
//!
//! A convex rough imported from a mesh file has no mesh of its own (it plans as its hull).
//! Adding an inclusion to it first builds the mesh of its hull, which is exactly the rough,
//! and the inclusion then makes it a mesh rough; removing the last inclusion gives the plain
//! hull back, with its old id.

use std::sync::Arc;

use glam::DVec3;
use indicatrix::geometry::stone_metrics::measure_solid_with_vertices;

use super::{
    Entry, HullError, HullShape, SourceFrame, hull_base, hull_faces, hull_outline, lookup,
    register_planes_from, registry, shape_id,
};
use crate::rough_plan::shape::{
    RoughBase,
    mesh::{MeshError, RoughMesh},
};

/// Points (mm) and triangles (0-based indices into them) of one closed mesh.
pub type MeshParts = (Vec<DVec3>, Vec<[u32; 3]>);

/// The most inclusions a rough may have.
pub const MAX_INCLUSIONS: usize = 64;

/// One inclusion of a mesh rough, as the planner window lists it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InclusionInfo {
    /// Its volume in mm^3, with its margin.
    pub volume_mm3: f64,
    /// The centre of its bounding box in the rough frame, in mm.
    pub centre_mm: DVec3,
    /// The extents of its bounding box, in mm.
    pub extents_mm: DVec3,
}

/// The inclusions of hull `id`, in the order they were added; empty for a rough without a
/// mesh, without inclusions, or an id that is not registered.
#[must_use]
pub fn inclusion_list(id: u64) -> Vec<InclusionInfo> {
    let Some(shape) = lookup(id) else {
        return Vec::new();
    };
    let Some(mesh) = shape.mesh.as_deref() else {
        return Vec::new();
    };
    mesh.inclusions()
        .iter()
        .map(|body| {
            let (lo, hi) = body.bounds();
            InclusionInfo {
                volume_mm3: body.volume(),
                centre_mm: (lo + hi) * 0.5,
                extents_mm: hi - lo,
            }
        })
        .collect()
}

/// The registered shape of `base` and its extents.
fn hull_of(base: &RoughBase) -> Result<(Arc<HullShape>, DVec3), HullError> {
    let RoughBase::Hull {
        id,
        x_mm,
        y_mm,
        z_mm,
    } = *base
    else {
        return Err(HullError::NotHull);
    };
    let shape = lookup(id).ok_or(HullError::NotHull)?;
    Ok((shape, DVec3::new(x_mm, y_mm, z_mm)))
}

/// The mesh of a convex rough: the triangles of the convex hull of its corners.
fn convex_outer_mesh(corners: &[DVec3]) -> Result<RoughMesh, HullError> {
    let lo = corners
        .iter()
        .fold(DVec3::splat(f64::INFINITY), |m, &p| m.min(p));
    let hi = corners
        .iter()
        .fold(DVec3::splat(f64::NEG_INFINITY), |m, &p| m.max(p));
    let eps = 1e-9 * (hi - lo).max_element().max(1e-12);
    let faces = hull_faces(corners, eps).ok_or(HullError::Flat)?;
    let tris: Vec<[u32; 3]> = faces.iter().map(|face| face.v.map(|i| i as u32)).collect();
    RoughMesh::new(corners, &tris).map_err(HullError::OuterUnusable)
}

/// The base of `shape` (its outline) with `mesh` beside it, registered under the id that
/// hashes both; `None` registers the plain hull. `extents` are the bounding box's.
fn register_with(shape: &HullShape, extents: DVec3, mesh: Option<RoughMesh>) -> RoughBase {
    let shape = HullShape {
        planes: shape.planes.clone(),
        vertices: shape.vertices.clone(),
        mesh: mesh.map(Arc::new),
        outline_faces: shape.outline_faces,
        source: shape.source,
    };
    let id = shape_id(&shape, extents);
    registry()
        .entries
        .entry(id)
        .or_insert_with(|| Entry::Root(Arc::new(shape)));
    hull_base(id, extents)
}

/// The rough `base` (an imported mesh, convex or not) with `inclusions` added inside it, each
/// grown by `margin_mm` first (see [`RoughMesh::grown`]), as a new base. `base` itself is
/// unchanged.
///
/// Each inclusion is a closed solid wound outward, in the rough's frame in mm (the frame of
/// [`RoughBase`]'s bounding box at the origin). It must lie wholly in the material and must
/// not cross the surface or another inclusion.
///
/// # Errors
///
/// [`HullError::NotHull`] for a base that is not an imported mesh,
/// [`HullError::Inclusion`] for an inclusion that does not fit (it reaches the surface, lies
/// outside, crosses another), [`HullError::InclusionTooClose`] for one that fits only
/// without its margin, and [`HullError::OuterUnusable`] when the convex rough's own mesh
/// cannot be built.
pub fn add_inclusion_meshes(
    base: &RoughBase,
    inclusions: &[RoughMesh],
    margin_mm: f64,
) -> Result<RoughBase, HullError> {
    let (shape, extents) = hull_of(base)?;
    let outer = match shape.mesh.as_deref() {
        Some(mesh) => mesh.clone(),
        None => convex_outer_mesh(&shape.vertices)?,
    };
    if outer.inclusion_count() + inclusions.len() > MAX_INCLUSIONS {
        return Err(HullError::TooManyInclusions);
    }
    let mut grown = Vec::with_capacity(inclusions.len());
    for (k, body) in inclusions.iter().enumerate() {
        // As given first, so a mesh that reaches the surface is said so, not blamed on the
        // margin.
        outer
            .check_inclusion(body, k)
            .map_err(HullError::Inclusion)?;
        grown.push(body.grown(margin_mm).map_err(HullError::Inclusion)?);
    }
    let combined = RoughMesh::with_inclusions(&outer, &grown).map_err(|error| match error {
        MeshError::InclusionOutside(_)
        | MeshError::InclusionReachesSurface(_)
        | MeshError::InclusionsOverlap
            if margin_mm > 0.0 =>
        {
            HullError::InclusionTooClose
        }
        other => HullError::Inclusion(other),
    })?;
    Ok(register_with(&shape, extents, Some(combined)))
}

/// [`add_inclusion_meshes`] for one inclusion. This is the entry point for anything that
/// finds an inclusion by other means than a file (for example, locating it from photos).
///
/// # Errors
///
/// As [`add_inclusion_meshes`].
pub fn add_inclusion_mesh(
    base: &RoughBase,
    inclusion: &RoughMesh,
    margin_mm: f64,
) -> Result<RoughBase, HullError> {
    add_inclusion_meshes(base, std::slice::from_ref(inclusion), margin_mm)
}

/// [`add_inclusion_mesh`] for an inclusion given as points (mm, in the rough's frame) and
/// triangles indexing them: a closed mesh, in any winding.
///
/// # Errors
///
/// [`HullError::Inclusion`] when the points and triangles are not a usable closed mesh (see
/// [`RoughMesh::new`]), else as [`add_inclusion_meshes`].
pub fn add_inclusion_points(
    base: &RoughBase,
    points: &[DVec3],
    tris: &[[u32; 3]],
    margin_mm: f64,
) -> Result<RoughBase, HullError> {
    let body = RoughMesh::new(points, tris).map_err(HullError::Inclusion)?;
    add_inclusion_mesh(base, &body, margin_mm)
}

/// The rough `base` without its inclusion number `index`, as a new base.
///
/// `index` is 0-based, in the order added. Removing the last inclusion of a rough that was
/// convex before gives back the plain hull, with the id it had.
///
/// # Errors
///
/// [`HullError::NotHull`] for a base that is not an imported mesh, [`HullError::NoInclusion`]
/// when it has no inclusion `index`.
pub fn remove_inclusion(base: &RoughBase, index: usize) -> Result<RoughBase, HullError> {
    let (shape, extents) = hull_of(base)?;
    let mesh = shape.mesh.as_deref().ok_or(HullError::NoInclusion)?;
    let reduced = mesh
        .without_inclusion(index)
        .ok_or(HullError::NoInclusion)?;
    let convex_again = reduced.inclusion_count() == 0
        && shape.outline_faces.is_none()
        && measure_solid_with_vertices(&shape.planes)
            .is_some_and(|(metrics, _)| reduced.volume() >= metrics.volume * (1.0 - 1e-6));
    Ok(register_with(
        &shape,
        extents,
        (!convex_again).then_some(reduced),
    ))
}

/// The rough of the mesh `points`/`tris` with `inclusions` inside it: what a saved plan reads back.
///
/// Each inclusion is points and triangles, already grown by whatever margin it was saved
/// with. The outline and the frame are those of the rough's own mesh, as in
/// [`import_mesh`](super::import_mesh). `source` is the [`SourceFrame`] the plan stored for
/// the rough's file, if it did, so that more inclusions can be added in that file's
/// coordinates.
///
/// # Errors
///
/// [`HullError::OuterUnusable`] when the rough's own mesh cannot be used,
/// [`HullError::Inclusion`] when an inclusion cannot, or does not fit, and the errors of
/// [`import_hull`](super::import_hull) for its outline.
pub fn import_mesh_with_inclusions(
    points: &[DVec3],
    tris: &[[u32; 3]],
    inclusions: &[MeshParts],
    source: Option<SourceFrame>,
) -> Result<RoughBase, HullError> {
    if inclusions.len() > MAX_INCLUSIONS {
        return Err(HullError::TooManyInclusions);
    }
    let outer = RoughMesh::new(points, tris).map_err(HullError::OuterUnusable)?;
    let bodies = inclusions
        .iter()
        .map(|(p, t)| RoughMesh::new(p, t).map_err(HullError::Inclusion))
        .collect::<Result<Vec<_>, _>>()?;
    let combined = RoughMesh::with_inclusions(&outer, &bodies).map_err(HullError::Inclusion)?;
    let outline = hull_outline(outer.vertices())?;
    register_planes_from(
        &outline.planes,
        Some(&combined),
        outline.simplified_from,
        source.unwrap_or(SourceFrame::IDENTITY),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rough_plan::shape::mesh_fixture::{CUBE_OBJ, parse_obj};

    #[test]
    fn the_id_tells_an_inclusion_from_a_cavity_of_the_same_shape() {
        let (corners, tris) = parse_obj(CUBE_OBJ);
        let base = super::super::import_hull(&corners).expect("a cube");
        let (shape, extents) = hull_of(&base).expect("registered");
        let outer = convex_outer_mesh(&shape.vertices).expect("the cube's mesh");
        let (mut inner, _) = parse_obj(CUBE_OBJ);
        for p in &mut inner {
            *p = *p * 0.2 + DVec3::splat(8.0);
        }
        let body = RoughMesh::new(&inner, &tris).expect("a closed cube");
        let with = RoughMesh::with_inclusions(&outer, &[body]).expect("it fits");
        // The same vertices and triangles as a plain mesh: a cube with a hollow in it. Building
        // a mesh numbers its vertices by first use in its triangles, and an inclusion's
        // triangles are wound the other way, so the rebuilt mesh holds the same points in
        // another order: the points are compared as a set, and the ids on exactly the content
        // of `with` below.
        let hollow = RoughMesh::new(with.vertices(), with.triangles()).expect("a hollow cube");
        let points = |mesh: &RoughMesh| {
            let mut bits: Vec<[u64; 3]> = mesh
                .vertices()
                .iter()
                // `+ 0.0` makes a negative zero a zero, which compares equal.
                .map(|p| [p.x, p.y, p.z].map(|c| (c + 0.0).to_bits()))
                .collect();
            bits.sort_unstable();
            bits
        };
        assert_eq!(points(&hollow), points(&with));
        assert_eq!(hollow.triangles().len(), with.triangles().len());
        assert_eq!(hollow.inclusion_count(), 0);
        assert_eq!(with.inclusion_count(), 1);
        let id_of = |mesh: RoughMesh| {
            let shape = HullShape {
                planes: shape.planes.clone(),
                vertices: shape.vertices.clone(),
                mesh: Some(Arc::new(mesh)),
                outline_faces: None,
                source: SourceFrame::IDENTITY,
            };
            shape_id(&shape, extents)
        };
        // What the id of `with` would be if it did not know its inclusion: the very same
        // vertices and triangles, hashed without the marks.
        let without_marks = super::super::content_id(
            shape
                .planes
                .iter()
                .flat_map(|&(n, d)| [n.x, n.y, n.z, d])
                .chain(extents.to_array())
                .chain(with.vertices().iter().flat_map(DVec3::to_array))
                .chain(with.triangles().iter().flatten().map(|&i| f64::from(i))),
        );
        assert_ne!(id_of(with.clone()), without_marks);
        assert_ne!(id_of(with), id_of(hollow));
    }

    #[test]
    fn a_mesh_without_inclusions_hashes_as_it_always_did() {
        // The marker is only hashed for inclusions: the id of a hull with a plain mesh must
        // not change, or every saved registration of one would.
        let (corners, tris) = parse_obj(CUBE_OBJ);
        let base = super::super::import_hull(&corners).expect("a cube");
        let (shape, extents) = hull_of(&base).expect("registered");
        let mesh = RoughMesh::new(&corners, &tris).expect("a closed cube");
        let plain = HullShape {
            planes: shape.planes.clone(),
            vertices: shape.vertices.clone(),
            mesh: Some(Arc::new(mesh.clone())),
            outline_faces: None,
            source: SourceFrame::IDENTITY,
        };
        let expected = super::super::content_id(
            plain
                .planes
                .iter()
                .flat_map(|&(n, d)| [n.x, n.y, n.z, d])
                .chain(extents.to_array())
                .chain(mesh.vertices().iter().flat_map(DVec3::to_array))
                .chain(mesh.triangles().iter().flatten().map(|&i| f64::from(i))),
        );
        assert_eq!(shape_id(&plain, extents), expected);
    }
}
