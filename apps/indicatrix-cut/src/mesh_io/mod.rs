//! Reading a rough's 3D model from a mesh file: Wavefront OBJ, STL (ASCII and binary) and
//! PLY (ASCII and binary, either byte order), plus the unit the file's numbers are in.
//!
//! The parsers are plain functions from bytes to points and triangles, with no window and
//! no file access, so their tests run with the rest of the workspace. They all return the
//! same [`NotedMesh`] and agree on what happens past the planner's triangle limit: the
//! vertices are kept (their convex hull is the rough), the triangles are dropped, and the
//! note says why.
//!
//! Everything is deterministic: triangles come out in file order, and the one place that
//! de-duplicates vertices (STL, a triangle soup) uses an ordered map.

mod face;
mod inclusion;
mod obj;
mod ply;
mod stl;
mod units;

#[cfg(test)]
pub mod fixtures;

#[cfg(test)]
pub use self::obj::{parse_obj_mesh, parse_obj_noted};
pub use self::{
    inclusion::{
        NOT_A_MESH_ROUGH, add_inclusion, including_text, inclusion_in_rough_frame, inclusion_parts,
        inclusion_row_text, parse_margin_mm,
    },
    units::MeshUnit,
};

use glam::DVec3;
use indicatrix_cut_core::rough_plan::MeshError;

/// Points, triangles (0-based indices into the points) and an optional import note.
pub type NotedMesh = (Vec<DVec3>, Vec<[u32; 3]>, Option<String>);

/// The file formats the planner reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshFormat {
    /// Wavefront OBJ (text).
    Obj,
    /// STL, ASCII or binary (told apart by [`stl::parse_stl`]).
    Stl,
    /// PLY, ASCII or binary.
    Ply,
}

/// The note for a mesh with more triangles than the planner takes: the same words the mesh
/// check gives, so a file that is counted out while it is read says what one that is
/// counted out afterwards would.
pub fn too_many_triangles_note(count: usize) -> String {
    format!(
        "The mesh could not be used as it is ({}); its convex outline is used instead.",
        MeshError::TooManyTriangles(count)
    )
}

/// Which format the file `bytes` is, given its extension (without the dot, any case).
///
/// A PLY magic (`ply` and a line break) wins over the extension. Otherwise a known extension
/// decides. A file with another or no extension is sniffed: a binary STL by the size its
/// triangle count implies, an ASCII STL by a leading `solid` and a `facet`, anything else is
/// read as OBJ.
pub fn detect_format(bytes: &[u8], extension: Option<&str>) -> MeshFormat {
    if bytes.starts_with(b"ply\n") || bytes.starts_with(b"ply\r") {
        return MeshFormat::Ply;
    }
    match extension.map(str::to_ascii_lowercase).as_deref() {
        Some("obj") => MeshFormat::Obj,
        Some("stl") => MeshFormat::Stl,
        Some("ply") => MeshFormat::Ply,
        _ if stl::looks_like_stl(bytes) => MeshFormat::Stl,
        _ => MeshFormat::Obj,
    }
}

/// The points and triangles of the mesh file `bytes` (see [`detect_format`] for how the
/// format is chosen), and a note when the triangles are not used.
///
/// # Errors
///
/// Returns the message for a file that is truncated, malformed or has no vertices.
pub fn parse_mesh(bytes: &[u8], extension: Option<&str>) -> Result<NotedMesh, String> {
    match detect_format(bytes, extension) {
        MeshFormat::Obj => obj::parse_obj_noted(&String::from_utf8_lossy(bytes)),
        MeshFormat::Stl => stl::parse_stl(bytes),
        MeshFormat::Ply => ply::parse_ply(bytes),
    }
}

#[cfg(test)]
mod tests;
