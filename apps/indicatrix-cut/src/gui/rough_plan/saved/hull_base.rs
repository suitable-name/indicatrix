//! The imported-mesh base in the plan file: its convex outline's corners, or, for a
//! non-convex rough, its closed triangle mesh.

use super::dto::{MeshDto, RoughDto};
use glam::DVec3;
use indicatrix_cut_core::rough_plan::{
    MAX_MESH_TRIANGLES, RoughBase, import_hull, import_mesh, shape::hull,
};

/// The most vertices a saved mesh may list. A closed surface has fewer vertices than
/// triangles, so a mesh within the triangle limit never needs more than this.
const MAX_MESH_VERTICES: usize = 3 * MAX_MESH_TRIANGLES;

/// The hull base the corners of `dto.hull` describe, or the mesh base `dto.mesh` does.
///
/// # Errors
///
/// Returns the message for a corner list that is not a usable outline, or for a mesh
/// that is too big, not a closed solid, or has an index outside its vertices.
pub(super) fn base_from_hull(dto: &RoughDto) -> Result<RoughBase, String> {
    if let Some(mesh) = &dto.mesh {
        return base_from_mesh(mesh);
    }
    let corners: Vec<DVec3> = dto.hull.iter().map(|&c| DVec3::from_array(c)).collect();
    import_hull(&corners).map_err(|e| format!("rough.hull: {e}"))
}

/// The mesh base of `mesh`, imported the way a file is. A mesh that import would replace
/// by its hull is refused: a plan is only saved with a mesh the planner used.
fn base_from_mesh(mesh: &MeshDto) -> Result<RoughBase, String> {
    if mesh.triangles.len() > MAX_MESH_TRIANGLES {
        return Err(format!(
            "rough.mesh has {} triangles, more than the {MAX_MESH_TRIANGLES} a rough may have",
            mesh.triangles.len()
        ));
    }
    if mesh.vertices.len() > MAX_MESH_VERTICES {
        return Err(format!(
            "rough.mesh has {} vertices, more than the {MAX_MESH_VERTICES} a rough may have",
            mesh.vertices.len()
        ));
    }
    let points: Vec<DVec3> = mesh
        .vertices
        .iter()
        .map(|&c| DVec3::from_array(c))
        .collect();
    let (base, note) =
        import_mesh(&points, &mesh.triangles).map_err(|e| format!("rough.mesh: {e}"))?;
    if let Some(note) = note {
        return Err(format!("rough.mesh: {note}"));
    }
    match base {
        RoughBase::Hull { id, .. } if hull::mesh(id).is_some() => Ok(base),
        _ => Err(
            "rough.mesh: the mesh is convex (or has no faces), so it is not a \
                  non-convex rough; a plan is only saved with a mesh the planner kept"
                .to_owned(),
        ),
    }
}

/// Writes the mesh (or, for a convex hull, the corners) of `base` into `dto`; false for any
/// other base.
pub(super) fn write_hull(dto: &mut RoughDto, base: &RoughBase) -> bool {
    let RoughBase::Hull { id, .. } = *base else {
        return false;
    };
    "hull".clone_into(&mut dto.base);
    if let Some(mesh) = hull::mesh(id) {
        dto.mesh = Some(MeshDto {
            vertices: mesh.vertices().iter().map(DVec3::to_array).collect(),
            triangles: mesh.triangles().to_vec(),
        });
        return true;
    }
    let Some(corners) = base.hull_corners() else {
        return false;
    };
    dto.hull = corners;
    true
}
