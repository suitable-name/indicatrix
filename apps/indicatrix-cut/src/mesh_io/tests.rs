//! Tests of the STL and PLY readers, the format detection and the units.

use super::{
    MeshFormat, MeshUnit, detect_format,
    fixtures::{C_SHAPE_OBJ, CUBE_OBJ, ascii_stl_of, binary_stl_of, ply_of},
    parse_mesh, parse_obj_mesh, ply, stl,
};
use glam::DVec3;
use indicatrix_cut_core::rough_plan::{MAX_MESH_TRIANGLES, RoughMesh};

/// The OBJ fixture's points and triangles.
fn obj(text: &str) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    parse_obj_mesh(text).expect("the fixture parses")
}

/// The faces of the OBJ fixture as the corner lists a PLY file holds (triangles).
fn as_faces(triangles: &[[u32; 3]]) -> Vec<Vec<u32>> {
    triangles.iter().map(|t| Vec::from(*t)).collect()
}

/// The (welded) triangle count and volume of a mesh.
fn welded(points: &[DVec3], triangles: &[[u32; 3]]) -> (usize, f64) {
    let mesh = RoughMesh::new(points, triangles).expect("a closed mesh");
    (mesh.triangles().len(), mesh.volume())
}

#[test]
fn ascii_stl_of_the_c_shape_welds_to_the_obj_mesh() {
    let (points, triangles) = obj(C_SHAPE_OBJ);
    let reference = welded(&points, &triangles);
    let text = ascii_stl_of(&points, &triangles);
    let (stl_points, stl_triangles, note) = stl::parse_stl(text.as_bytes()).expect("parses");
    assert_eq!(note, None);
    assert_eq!(stl_triangles.len(), triangles.len());
    let from_stl = welded(&stl_points, &stl_triangles);
    assert_eq!(from_stl.0, reference.0);
    assert!((from_stl.1 - 6000.0).abs() < 1e-6, "{from_stl:?}");
    assert!((from_stl.1 - reference.1).abs() < 1e-9);
}

#[test]
fn binary_stl_of_the_c_shape_welds_to_the_obj_mesh() {
    let (points, triangles) = obj(C_SHAPE_OBJ);
    let reference = welded(&points, &triangles);
    let bytes = binary_stl_of("binary c shape", &points, &triangles);
    let (stl_points, stl_triangles, note) = stl::parse_stl(&bytes).expect("parses");
    assert_eq!(note, None);
    // The soup is shared exactly: the 16 corners of the C-shape, not 3 per triangle.
    assert_eq!(stl_points.len(), points.len());
    let from_stl = welded(&stl_points, &stl_triangles);
    assert_eq!(from_stl.0, reference.0);
    assert!((from_stl.1 - reference.1).abs() < 1e-9);
}

#[test]
fn triangles_come_out_in_file_order() {
    let (points, triangles) = obj(CUBE_OBJ);
    let bytes = binary_stl_of("order", &points, &triangles);
    let (stl_points, stl_triangles, _) = stl::parse_stl(&bytes).expect("parses");
    for (read, original) in stl_triangles.iter().zip(&triangles) {
        assert_eq!(
            read.map(|k| stl_points[k as usize]),
            original.map(|k| points[k as usize])
        );
    }
}

#[test]
fn a_binary_stl_whose_header_says_solid_is_still_binary() {
    let (points, triangles) = obj(CUBE_OBJ);
    let bytes = binary_stl_of("solid exported by a CAD tool", &points, &triangles);
    assert!(bytes.starts_with(b"solid"));
    assert_eq!(detect_format(&bytes, None), MeshFormat::Stl);
    let (_, read, _) = parse_mesh(&bytes, Some("stl")).expect("read as binary");
    assert_eq!(read.len(), triangles.len());
}

#[test]
fn a_binary_stl_header_mentioning_facet_is_still_binary() {
    let (points, triangles) = obj(CUBE_OBJ);
    let bytes = binary_stl_of("solid facet facet", &points, &triangles);
    let (_, read, _) = stl::parse_stl(&bytes).expect("read as binary");
    assert_eq!(read.len(), triangles.len());
}

#[test]
fn an_ascii_stl_with_a_four_vertex_loop_is_refused() {
    let text = "solid x\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\n\
                vertex 1 1 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid x\n";
    let message = stl::parse_stl(text.as_bytes()).unwrap_err();
    assert!(message.contains("4 vertices"), "{message}");
    assert!(message.contains("triangles"), "{message}");
}

#[test]
fn truncated_stl_files_say_what_is_missing() {
    let (points, triangles) = obj(CUBE_OBJ);
    let bytes = binary_stl_of("cut short", &points, &triangles);
    let message = stl::parse_stl(&bytes[..bytes.len() - 20]).unwrap_err();
    assert!(message.contains("promises 12 triangles"), "{message}");
    assert!(message.contains("truncated"), "{message}");
    let message = stl::parse_stl(&bytes[..40]).unwrap_err();
    assert!(message.contains("too short"), "{message}");

    let text = ascii_stl_of(&points, &triangles);
    let cut = &text[..text.find("endloop").expect("a loop") - 1];
    let message = stl::parse_stl(cut.as_bytes()).unwrap_err();
    assert!(message.contains("ends inside a facet"), "{message}");
    let message = stl::parse_stl(b"solid empty\nfacet normal 0 0 0\nendsolid\n").unwrap_err();
    assert!(message.contains("no facets"), "{message}");
}

#[test]
fn a_binary_stl_with_a_nan_vertex_is_refused() {
    let (points, triangles) = obj(CUBE_OBJ);
    let mut bytes = binary_stl_of("nan", &points, &triangles);
    bytes[84 + 12..84 + 16].copy_from_slice(&f32::NAN.to_le_bytes());
    let message = stl::parse_stl(&bytes).unwrap_err();
    assert!(message.contains("triangle 1"), "{message}");
}

#[test]
fn an_stl_over_the_triangle_limit_keeps_its_vertices_and_drops_the_triangles() {
    // Triangles of one fixed corner and two others, so the vertices stay few.
    let points = [
        DVec3::new(0.0, 0.0, 0.0),
        DVec3::new(1.0, 0.0, 0.0),
        DVec3::new(0.0, 1.0, 0.0),
        DVec3::new(0.0, 0.0, 1.0),
    ];
    let triangles = vec![[0, 1, 2]; MAX_MESH_TRIANGLES + 1];
    let bytes = binary_stl_of("many", &points, &triangles);
    let (read_points, read, note) = stl::parse_stl(&bytes).expect("parses");
    assert_eq!(read_points.len(), 3, "equal vertices are shared");
    assert_eq!(read.len(), 0);
    let note = note.expect("a note says why");
    assert!(
        note.contains(&format!("{} triangles", MAX_MESH_TRIANGLES + 1)),
        "{note}"
    );
    assert!(note.contains("convex outline"), "{note}");
}

#[test]
fn ascii_and_binary_ply_of_the_cube_skip_normals_colours_and_other_elements() {
    let (points, triangles) = obj(CUBE_OBJ);
    let reference = welded(&points, &triangles);
    for format in ["ascii", "binary_little_endian", "binary_big_endian"] {
        let bytes = ply_of(format, &points, &as_faces(&triangles));
        let (read_points, read, note) = ply::parse_ply(&bytes).expect(format);
        assert_eq!(note, None, "{format}");
        assert_eq!(read_points, points, "{format}");
        assert_eq!(read, triangles, "{format}");
        assert_eq!(welded(&read_points, &read), reference, "{format}");
    }
}

#[test]
fn a_ply_quad_face_is_split_like_an_obj_face() {
    let (points, _) = obj(CUBE_OBJ);
    let quads = vec![
        vec![0, 3, 2, 1],
        vec![4, 5, 6, 7],
        vec![0, 1, 5, 4],
        vec![1, 2, 6, 5],
        vec![2, 3, 7, 6],
        vec![3, 0, 4, 7],
    ];
    let bytes = ply_of("binary_little_endian", &points, &quads);
    let (_, triangles, _) = ply::parse_ply(&bytes).expect("parses");
    assert_eq!(triangles.len(), 12);
    assert_eq!(triangles[0], [0, 3, 2]);
    assert_eq!(triangles[1], [0, 2, 1]);
}

#[test]
fn the_c_shape_as_a_ply_keeps_its_volume() {
    let (points, triangles) = obj(C_SHAPE_OBJ);
    let bytes = ply_of("binary_big_endian", &points, &as_faces(&triangles));
    let (read_points, read, _) = parse_mesh(&bytes, Some("PLY")).expect("parses");
    let (count, volume) = welded(&read_points, &read);
    assert_eq!(count, triangles.len());
    assert!((volume - 6000.0).abs() < 1e-6, "{volume}");
}

#[test]
fn truncated_and_malformed_ply_files_name_what_is_missing() {
    let (points, triangles) = obj(CUBE_OBJ);
    let faces = as_faces(&triangles);
    for format in ["ascii", "binary_little_endian"] {
        let bytes = ply_of(format, &points, &faces);
        let message = ply::parse_ply(&bytes[..bytes.len() - 60]).unwrap_err();
        assert!(message.contains("the file ends"), "{format}: {message}");
        assert!(message.contains("`face`"), "{format}: {message}");
    }
    let bytes = ply_of("binary_little_endian", &points, &faces);
    let header_end = bytes
        .windows(10)
        .position(|w| w == b"end_header")
        .expect("a header");
    let message = ply::parse_ply(&bytes[..header_end + 20]).unwrap_err();
    assert!(message.contains("`vertex`"), "{message}");

    let message = ply::parse_ply(b"ply\nformat ascii 1.0\nelement vertex 1\n").unwrap_err();
    assert!(message.contains("end_header"), "{message}");
    let message = ply::parse_ply(
        b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nend_header\n1\n",
    )
    .unwrap_err();
    assert!(message.contains("no x, y and z"), "{message}");
    let message = ply::parse_ply(
        b"ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
          property float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n\
          0 0 0\n1 0 0\n0 1 0\n3 0 1 9\n",
    )
    .unwrap_err();
    assert!(message.contains("face 1 names vertex 9"), "{message}");
}

#[test]
fn a_ply_over_the_triangle_limit_keeps_its_vertices_and_drops_the_triangles() {
    let over = MAX_MESH_TRIANGLES + 1;
    let mut text = format!(
        "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
         property float z\nelement face {over}\nproperty list uchar int vertex_index\nend_header\n\
         0 0 0\n1 0 0\n0 1 0\n",
    );
    text.push_str(&"3 0 1 2\n".repeat(over));
    let (points, triangles, note) = ply::parse_ply(text.as_bytes()).expect("parses");
    assert_eq!(points.len(), 3);
    assert_eq!(triangles.len(), 0);
    assert!(note.expect("a note").contains(&format!("{over} triangles")));
}

#[test]
fn the_format_comes_from_the_magic_then_the_extension_then_the_content() {
    let (points, triangles) = obj(CUBE_OBJ);
    let ply = ply_of("ascii", &points, &as_faces(&triangles));
    assert_eq!(detect_format(&ply, Some("obj")), MeshFormat::Ply);
    assert_eq!(
        detect_format(CUBE_OBJ.as_bytes(), Some("OBJ")),
        MeshFormat::Obj
    );
    assert_eq!(
        detect_format(CUBE_OBJ.as_bytes(), Some("Stl")),
        MeshFormat::Stl
    );
    assert_eq!(detect_format(CUBE_OBJ.as_bytes(), None), MeshFormat::Obj);
    let ascii = ascii_stl_of(&points, &triangles);
    assert_eq!(detect_format(ascii.as_bytes(), None), MeshFormat::Stl);
    assert_eq!(
        detect_format(ascii.as_bytes(), Some("dat")),
        MeshFormat::Stl
    );
    let binary = binary_stl_of("x", &points, &triangles);
    assert_eq!(detect_format(&binary, Some("mesh")), MeshFormat::Stl);
}

#[test]
fn an_obj_goes_through_the_dispatcher_unchanged() {
    let direct = obj(CUBE_OBJ);
    let (points, triangles, note) = parse_mesh(CUBE_OBJ.as_bytes(), Some("obj")).expect("parses");
    assert_eq!((points, triangles), direct);
    assert_eq!(note, None);
}

#[test]
fn unit_factors_and_labels() {
    let factors: Vec<f64> = MeshUnit::ALL
        .into_iter()
        .map(MeshUnit::factor_to_mm)
        .collect();
    assert_eq!(factors, vec![1.0, 10.0, 1000.0, 25.4, 0.001]);
    let labels: Vec<&str> = MeshUnit::ALL.into_iter().map(MeshUnit::label).collect();
    assert_eq!(labels, vec!["mm", "cm", "m", "inch", "\u{b5}m"]);
    assert_eq!(MeshUnit::default(), MeshUnit::Millimetre);
    for (index, unit) in MeshUnit::ALL.into_iter().enumerate() {
        assert_eq!(
            MeshUnit::from_index(i32::try_from(index).expect("small")),
            unit
        );
    }
    assert_eq!(MeshUnit::from_index(-1), MeshUnit::Millimetre);
    assert_eq!(MeshUnit::from_index(99), MeshUnit::Millimetre);
}

#[test]
fn the_suggestion_for_a_file_in_another_unit() {
    // A 34 mm stone in metres is 0.034 units wide.
    assert_eq!(
        MeshUnit::suggest(0.034, MeshUnit::Millimetre),
        Some(MeshUnit::Metre)
    );
    // In micrometres it is 34,000.
    assert_eq!(
        MeshUnit::suggest(34_000.0, MeshUnit::Millimetre),
        Some(MeshUnit::Micrometre)
    );
    // In centimetres it is 3.4: read as mm that is believable, so nothing is refused, but a
    // file that was refused as metres gets the unit that fits.
    assert_eq!(
        MeshUnit::suggest(3.4, MeshUnit::Metre),
        Some(MeshUnit::Millimetre)
    );
    // The current unit is never suggested, and a size no unit fixes gives nothing.
    assert_eq!(MeshUnit::suggest(1e12, MeshUnit::Millimetre), None);
    assert_eq!(MeshUnit::suggest(0.0, MeshUnit::Millimetre), None);
    assert_eq!(MeshUnit::suggest(f64::NAN, MeshUnit::Millimetre), None);
}
