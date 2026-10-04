//! Importing a Wavefront OBJ file as the rough: a closed mesh becomes the (possibly non-convex)
//! rough; anything else falls back to the convex hull of its vertices, with a note.

mod face;

use self::face::triangulate_face;
use super::{
    editing::{input_error, install_imported_base},
    host::{Host, on_idle_host},
};
use crate::gui::pickers::{PickerFilter, PickerKind, PickerRequest, pick};
use glam::DVec3;
use indicatrix_cut_core::rough_plan::{RoughBase, import_mesh};
use std::{path::Path, rc::Rc};

/// The vertices (`v x y z`, in mm) and triangles (0-based indices into the vertices) of
/// the OBJ text `text`.
///
/// A face (`f`) may name its vertices as `v`, `v/vt`, `v//vn` or `v/vt/vn`, with 1-based
/// or negative (relative to the vertices read so far) indices; a convex polygon is fanned
/// from its first vertex and a non-convex one is ear-clipped in its plane. Normals, textures and everything else are ignored. A file with vertices
/// but no faces gives no triangles.
///
/// # Errors
///
/// Returns the message for a vertex line without three numbers, a face with fewer than
/// three vertices, an index that is not a number or points outside the vertices, or a file
/// without vertices.
#[cfg(test)]
pub(super) fn parse_obj_mesh(text: &str) -> Result<(Vec<DVec3>, Vec<[u32; 3]>), String> {
    parse_obj_noted(text).map(|(points, triangles, _)| (points, triangles))
}

/// Points, triangles and an optional import note, as [`parse_obj_noted`] returns them.
type NotedMesh = (Vec<DVec3>, Vec<[u32; 3]>, Option<String>);

/// The points and triangles of an OBJ file, and a note when a non-convex polygon face could not be split into
/// triangles: the triangles are then empty (the file is a point cloud, whose convex hull is
/// the rough) because a mesh with that face left out would be open or wrong.
fn parse_obj_noted(text: &str) -> Result<NotedMesh, String> {
    let mut points = Vec::new();
    // Faces as signed indices, negative ones already made absolute; positive ones are
    // checked against the final vertex count (a face may name a later vertex).
    let mut faces: Vec<(usize, Vec<i64>)> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        let mut words = line.split_whitespace();
        match words.next() {
            Some("v") => {
                let mut coordinate = || {
                    words
                        .next()
                        .and_then(|word| word.parse::<f64>().ok())
                        .ok_or_else(|| format!("line {line_no} is not a vertex with three numbers"))
                };
                points.push(DVec3::new(coordinate()?, coordinate()?, coordinate()?));
            }
            Some("f") => {
                let count = i64::try_from(points.len()).unwrap_or(i64::MAX);
                let mut corners = Vec::new();
                for word in words {
                    let raw = word.split('/').next().unwrap_or_default();
                    let value: i64 = raw
                        .parse()
                        .map_err(|_| format!("line {line_no} has a face index `{word}`"))?;
                    // 1-based; `-1` is the latest vertex.
                    let absolute = match value {
                        0 => return Err(format!("line {line_no} has the face index 0")),
                        v if v < 0 => count + v + 1,
                        v => v,
                    };
                    corners.push(absolute);
                }
                if corners.len() < 3 {
                    return Err(format!(
                        "line {line_no} is a face with fewer than three vertices"
                    ));
                }
                faces.push((line_no, corners));
            }
            _ => {}
        }
    }
    if points.is_empty() {
        return Err("the file has no vertices (`v x y z` lines)".to_string());
    }
    let total = i64::try_from(points.len()).unwrap_or(i64::MAX);
    let mut triangles = Vec::new();
    let mut note = None;
    for (line_no, corners) in faces {
        let mut resolved = Vec::with_capacity(corners.len());
        for corner in corners {
            if !(1..=total).contains(&corner) {
                return Err(format!(
                    "line {line_no} names vertex {corner}, but the file has {total} vertices"
                ));
            }
            // In range of `1..=total`, so it fits a `u32` for any file the mesh limit lets
            // through; a larger vertex count is refused by the cast.
            resolved.push(
                u32::try_from(corner - 1)
                    .map_err(|_| "the file has too many vertices".to_string())?,
            );
        }
        match triangulate_face(&points, &resolved) {
            Some(split) => triangles.extend(split),
            None => {
                note.get_or_insert_with(|| {
                    format!(
                        "Line {line_no} is a polygon face that is not convex and could not be \
                         split into triangles, so the mesh is not used and its convex hull is \
                         the rough."
                    )
                });
            }
        }
    }
    if note.is_some() {
        triangles.clear();
    }
    Ok((points, triangles, note))
}

/// The rough base of the OBJ file at `path`, and the note to show when the closed mesh could
/// not be used and the base is the convex hull of its vertices instead.
fn base_from_file(path: &Path) -> Result<(RoughBase, Option<String>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("could not read the file: {e}"))?;
    let (points, triangles, split_note) = parse_obj_noted(&String::from_utf8_lossy(&bytes))?;
    let (base, note) = import_mesh(&points, &triangles).map_err(|e| e.to_string())?;
    let note = split_note.or(note).or_else(|| {
        triangles
            .is_empty()
            .then(|| "The file has no faces, so its convex hull is the rough.".to_string())
    });
    Ok((base, note))
}

/// Asks for an OBJ file and, on a pick, makes its mesh (or, failing that, its convex hull)
/// the rough.
pub(super) fn start(host: &Rc<Host>) {
    let Some(main) = host.main.upgrade() else {
        return;
    };
    let request = PickerRequest {
        kind: PickerKind::OpenFile,
        title: Some("Import rough from OBJ".to_string()),
        filters: vec![PickerFilter::single("obj")],
        default_file_name: None,
        starting_dir: None,
    };
    pick(&main, request, |_, path| {
        let Some(path) = path else {
            return;
        };
        on_idle_host(|host| match base_from_file(&path) {
            Ok((base, note)) => install_imported_base(host, base, note.as_deref()),
            Err(message) => input_error(host, &format!("OBJ import failed: {message}")),
        });
    });
}

/// A 20 mm cube with a 10 x 10 x 20 mm notch in its `+x` face (6000 mm^3 of material in an
/// 8000 mm^3 hull): the non-convex fixture of the tests.
#[cfg(test)]
pub(super) const C_SHAPE_OBJ: &str = "\
# C-shaped rough
v 0 0 0
v 20 0 0
v 20 5 0
v 10 5 0
v 10 15 0
v 20 15 0
v 20 20 0
v 0 20 0
v 0 0 20
v 20 0 20
v 20 5 20
v 10 5 20
v 10 15 20
v 20 15 20
v 20 20 20
v 0 20 20
# top
f 9 10 11
f 9 11 12
f 9 12 13
f 9 13 16
f 13 14 15
f 13 15 16
# bottom
f 1 3 2
f 1 4 3
f 1 5 4
f 1 8 5
f 5 7 6
f 5 8 7
# sides
f 1 2 10 9
f 2 3 11 10
f 3 4 12 11
f 4 5 13 12
f 5 6 14 13
f 6 7 15 14
f 7 8 16 15
f 8 1 9 16
";

/// A convex 20 mm cube, as quads.
#[cfg(test)]
pub(super) const CUBE_OBJ: &str = "\
# cube
v 0 0 0
v 20 0 0
v 20 20 0
v 0 20 0
v 0 0 20
v 20 0 20
v 20 20 20
v 0 20 20
f 1 4 3 2
f 5 6 7 8
f 1 2 6 5
f 2 3 7 6
f 3 4 8 7
f 4 1 5 8
";

/// The base of the OBJ text `obj`, imported as a mesh; panics on a note or an error.
#[cfg(test)]
pub(super) fn mesh_base_of(obj: &str) -> RoughBase {
    let (points, triangles) = parse_obj_mesh(obj).expect("the fixture parses");
    let (base, note) = import_mesh(&points, &triangles).expect("the fixture imports");
    assert_eq!(note, None);
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::rough_plan::RoughModel;

    #[test]
    fn vertices_are_read_and_everything_else_is_skipped() {
        let text = "# cube
v 0 0 0
v 1.5 0 0 1
vn 0 0 1
v -1e1 2 3 0.5 0.5 0.5
f 1 2 3
";
        let (points, triangles) = parse_obj_mesh(text).expect("parses");
        assert_eq!(points.len(), 3);
        assert_eq!(points[2], DVec3::new(-10.0, 2.0, 3.0));
        assert_eq!(triangles, vec![[0, 1, 2]]);
    }

    #[test]
    fn a_short_vertex_line_or_an_empty_file_is_refused() {
        assert!(parse_obj_mesh("v 1 2\n").unwrap_err().contains("line 1"));
        assert!(parse_obj_mesh("f 1 2 3\n").is_err());
    }

    #[test]
    fn all_four_index_forms_name_the_same_triangle() {
        let head = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvn 0 0 1\n";
        for face in [
            "f 1 2 3",
            "f 1/1 2/1 3/1",
            "f 1//1 2//1 3//1",
            "f 1/1/1 2/1/1 3/1/1",
        ] {
            let (_, triangles) = parse_obj_mesh(&format!("{head}{face}\n")).expect(face);
            assert_eq!(triangles, vec![[0, 1, 2]], "{face}");
        }
    }

    #[test]
    fn negative_indices_count_back_from_the_vertices_read_so_far() {
        let text = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf -3 -2 -1\nv 0 0 1\nf -1 -4 -2\n";
        let (_, triangles) = parse_obj_mesh(text).expect("parses");
        assert_eq!(triangles, vec![[0, 1, 2], [3, 0, 2]]);
    }

    #[test]
    fn polygons_are_fanned_from_their_first_vertex() {
        let text = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0.5 1.5 0\nv 0 1 0\nf 1 2 3 4 5\n";
        let (_, triangles) = parse_obj_mesh(text).expect("parses");
        assert_eq!(triangles, vec![[0, 1, 2], [0, 2, 3], [0, 3, 4]]);
    }

    #[test]
    fn a_convex_quad_face_is_fanned_as_before() {
        let text = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n";
        let (_, triangles) = parse_obj_mesh(text).expect("parses");
        assert_eq!(triangles, vec![[0, 1, 2], [0, 2, 3]]);
    }

    #[test]
    fn a_concave_face_is_ear_clipped_instead_of_fanned() {
        // The dart `(0,0) (4,0) (0,4) (1,1)`: its fan would flip a triangle.
        let text = "v 0 0 0\nv 4 0 0\nv 0 4 0\nv 1 1 0\nf 1 2 3 4\n";
        let (points, triangles) = parse_obj_mesh(text).expect("parses");
        assert_eq!(triangles.len(), 2);
        let area: f64 = triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|k| points[k as usize]);
                (b - a).cross(c - a).z * 0.5
            })
            .sum();
        assert!((area - 6.0).abs() < 1e-12, "{triangles:?}");
        assert!(triangles.iter().all(|t| {
            let [a, b, c] = t.map(|k| points[k as usize]);
            (b - a).cross(c - a).z > 0.0
        }));
    }

    #[test]
    fn a_prism_with_concave_caps_has_its_exact_volume() {
        let text = "v 0 0 0\nv 4 0 0\nv 0 4 0\nv 1 1 0\nv 0 0 2\nv 4 0 2\nv 0 4 2\nv 1 1 2\n\
                    f 4 3 2 1\nf 5 6 7 8\nf 1 2 6 5\nf 2 3 7 6\nf 3 4 8 7\nf 4 1 5 8\n";
        let (points, triangles, note) = parse_obj_noted(text).expect("parses");
        assert_eq!(note, None);
        let (base, note) = import_mesh(&points, &triangles).expect("imports");
        assert_eq!(note, None, "the split mesh is closed");
        let model = RoughModel::new(base, Vec::new());
        let mesh = model.mesh().expect("the dart is not convex");
        assert!((mesh.volume() - 12.0).abs() < 1e-9, "{}", mesh.volume());
    }

    #[test]
    fn a_face_that_cannot_be_split_gives_a_note_and_no_triangles() {
        let text = "v 0 0 0\nv 3 3 0\nv 3 0 0\nv 0 1 0\nf 1 2 3 4\n";
        let (points, triangles, note) = parse_obj_noted(text).expect("parses");
        assert_eq!(points.len(), 4);
        assert!(triangles.is_empty());
        assert!(note.expect("a note").contains("Line 5"));
    }

    #[test]
    fn faces_with_bad_indices_are_refused() {
        let head = "v 0 0 0\nv 1 0 0\nv 0 1 0\n";
        for face in ["f 1 2 4", "f 1 2 0", "f -4 1 2", "f 1 2", "f 1 2 x"] {
            assert!(
                parse_obj_mesh(&format!("{head}{face}\n")).is_err(),
                "{face} must be refused"
            );
        }
    }

    #[test]
    fn a_cube_imports_as_the_same_convex_hull_as_its_vertices() {
        let (points, triangles) = parse_obj_mesh(CUBE_OBJ).expect("parses");
        assert_eq!(triangles.len(), 12);
        let (base, note) = import_mesh(&points, &triangles).expect("imports");
        assert_eq!(note, None);
        assert!(RoughModel::new(base, Vec::new()).mesh().is_none());
        assert_eq!(
            base,
            import_mesh(&points, &[]).expect("hull").0,
            "a convex mesh is registered exactly like its hull"
        );
    }

    #[test]
    fn the_c_shape_keeps_its_mesh_and_its_volume() {
        let (points, triangles) = parse_obj_mesh(C_SHAPE_OBJ).expect("parses");
        let (base, note) = import_mesh(&points, &triangles).expect("imports");
        assert_eq!(note, None);
        let measure = RoughModel::new(base, Vec::new())
            .measure()
            .expect("measures");
        assert!((measure.volume_mm3 - 6000.0).abs() < 1e-6, "{measure:?}");
    }

    #[test]
    fn an_open_mesh_falls_back_to_the_hull_with_a_note() {
        let (points, mut triangles) = parse_obj_mesh(CUBE_OBJ).expect("parses");
        triangles.pop();
        let (_, note) = import_mesh(&points, &triangles).expect("imports");
        assert!(note.is_some());
    }
}
