//! The imported-mesh base in the plan file: its convex outline's corners, or, for a
//! non-convex rough, its closed triangle mesh, with the inclusions that were added to it.

use super::dto::{MeshDto, RoughDto, SourceFrameDto};
use glam::DVec3;
use indicatrix_cut_core::rough_plan::{
    MAX_MESH_TRIANGLES, RoughBase, import_hull, import_mesh,
    shape::hull::{
        self, HullError, MAX_INCLUSIONS, MeshParts, SourceFrame, import_mesh_with_inclusions,
    },
};

/// The most vertices a saved mesh may list. A closed surface has fewer vertices than
/// triangles, so a mesh within the triangle limit never needs more than this.
const MAX_MESH_VERTICES: usize = 3 * MAX_MESH_TRIANGLES;

/// The hull base the corners of `dto.hull` describe, or the mesh base `dto.mesh` does, with
/// the inclusions of `dto.inclusions` inside it.
///
/// # Errors
///
/// Returns the message for a corner list that is not a usable outline, for a mesh that is
/// too big, not a closed solid, or has an index outside its vertices, and for an inclusion
/// that does not fit in the mesh.
pub(super) fn base_from_hull(dto: &RoughDto) -> Result<RoughBase, String> {
    if let Some(mesh) = &dto.mesh {
        return if dto.inclusions.is_empty() {
            base_from_mesh(mesh)
        } else {
            base_from_mesh_with_inclusions(mesh, &dto.inclusions, dto.source_frame)
        };
    }
    if !dto.inclusions.is_empty() {
        return Err("rough.inclusions needs rough.mesh, the mesh they lie in".to_owned());
    }
    let corners: Vec<DVec3> = dto.hull.iter().map(|&c| DVec3::from_array(c)).collect();
    import_hull(&corners).map_err(|e| format!("rough.hull: {e}"))
}

/// Fails when `mesh`, found at `path`, lists more triangles or vertices than a rough may.
fn check_mesh_size(mesh: &MeshDto, path: &str) -> Result<(), String> {
    if mesh.triangles.len() > MAX_MESH_TRIANGLES {
        return Err(format!(
            "{path} has {} triangles, more than the {MAX_MESH_TRIANGLES} a rough may have",
            mesh.triangles.len()
        ));
    }
    if mesh.vertices.len() > MAX_MESH_VERTICES {
        return Err(format!(
            "{path} has {} vertices, more than the {MAX_MESH_VERTICES} a rough may have",
            mesh.vertices.len()
        ));
    }
    Ok(())
}

/// The points and triangles of `mesh`.
fn parts_of(mesh: &MeshDto) -> MeshParts {
    (
        mesh.vertices
            .iter()
            .map(|&c| DVec3::from_array(c))
            .collect(),
        mesh.triangles.clone(),
    )
}

/// The mesh base of `mesh`, imported the way a file is. A mesh that import would replace
/// by its hull is refused: a plan is only saved with a mesh the planner used.
fn base_from_mesh(mesh: &MeshDto) -> Result<RoughBase, String> {
    check_mesh_size(mesh, "rough.mesh")?;
    let (points, triangles) = parts_of(mesh);
    let (base, note) = import_mesh(&points, &triangles).map_err(|e| format!("rough.mesh: {e}"))?;
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

/// The mesh base of `mesh` with `inclusions` inside it. The rough's own mesh may be convex
/// here: the inclusions make it a mesh rough.
fn base_from_mesh_with_inclusions(
    mesh: &MeshDto,
    inclusions: &[MeshDto],
    source: Option<SourceFrameDto>,
) -> Result<RoughBase, String> {
    if inclusions.len() > MAX_INCLUSIONS {
        return Err(format!(
            "rough.inclusions lists {} inclusions, more than the {MAX_INCLUSIONS} a rough may have",
            inclusions.len()
        ));
    }
    check_mesh_size(mesh, "rough.mesh")?;
    for (i, inclusion) in inclusions.iter().enumerate() {
        check_mesh_size(inclusion, &format!("rough.inclusions[{i}]"))?;
    }
    let source = source
        .map(|frame| {
            let offset = DVec3::from_array(frame.offset_mm);
            if frame.scale.is_finite() && frame.scale > 0.0 && offset.is_finite() {
                Ok(SourceFrame {
                    scale: frame.scale,
                    offset,
                })
            } else {
                Err("rough.source_frame must hold a positive scale and finite numbers".to_owned())
            }
        })
        .transpose()?;
    let (points, triangles) = parts_of(mesh);
    let parts: Vec<MeshParts> = inclusions.iter().map(parts_of).collect();
    import_mesh_with_inclusions(&points, &triangles, &parts, source).map_err(|error| match error {
        HullError::OuterUnusable(_) => format!("rough.mesh: {error}"),
        _ => format!("rough.inclusions: {error}"),
    })
}

/// `p` as written to a plan, with every zero unsigned: a hull corner can carry `-0.0`, which
/// reads back as `0.0`, so writing it as is would make a saved plan differ from its re-save.
fn unsigned(p: DVec3) -> [f64; 3] {
    (p + DVec3::ZERO).to_array()
}

/// Writes the mesh (or, for a convex hull, the corners) of `base` into `dto`, and the
/// inclusions of a mesh; false for any other base. The mesh written is the rough's own,
/// without its inclusions, so a rough without inclusions is written as it always was.
pub(super) fn write_hull(dto: &mut RoughDto, base: &RoughBase) -> bool {
    let RoughBase::Hull { id, .. } = *base else {
        return false;
    };
    "hull".clone_into(&mut dto.base);
    if let Some(mesh) = hull::mesh(id) {
        dto.mesh = Some(MeshDto {
            vertices: mesh.outer_vertices().iter().map(|&p| unsigned(p)).collect(),
            triangles: mesh.outer_triangles().to_vec(),
        });
        dto.inclusions = mesh
            .inclusions()
            .iter()
            .map(|body| MeshDto {
                vertices: body.vertices().iter().map(|&p| unsigned(p)).collect(),
                triangles: body.triangles().to_vec(),
            })
            .collect();
        if !dto.inclusions.is_empty() {
            dto.source_frame = hull::source_frame(id).map(|frame| SourceFrameDto {
                scale: frame.scale,
                offset_mm: frame.offset.to_array(),
            });
        }
        return true;
    }
    let Some(corners) = base.hull_corners() else {
        return false;
    };
    dto.hull = corners;
    true
}
