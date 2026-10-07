//! Small meshes for the tests, and writers that turn a mesh into STL and PLY bytes.

use glam::DVec3;
use std::fmt::Write as _;

/// A 20 mm cube with a 10 x 10 x 20 mm notch in its `+x` face (6000 mm^3 of material in an
/// 8000 mm^3 hull): the non-convex fixture of the tests.
pub const C_SHAPE_OBJ: &str = "\
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
pub const CUBE_OBJ: &str = "\
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

/// The mesh as ASCII STL text: one facet per triangle, a zero normal (it is ignored).
pub fn ascii_stl_of(points: &[DVec3], triangles: &[[u32; 3]]) -> String {
    let mut text = String::from("solid fixture\n");
    for triangle in triangles {
        text.push_str("  facet normal 0 0 0\n    outer loop\n");
        for &corner in triangle {
            let p = points[corner as usize];
            writeln!(text, "      vertex {} {} {}", p.x, p.y, p.z).expect("writing to a String");
        }
        text.push_str("    endloop\n  endfacet\n");
    }
    text.push_str("endsolid fixture\n");
    text
}

/// The mesh as binary STL: `header` (padded or cut to 80 bytes), the triangle count and one
/// 50-byte record per triangle.
pub fn binary_stl_of(header: &str, points: &[DVec3], triangles: &[[u32; 3]]) -> Vec<u8> {
    let mut bytes = header.as_bytes().to_vec();
    bytes.resize(80, b' ');
    bytes.extend(
        u32::try_from(triangles.len())
            .expect("a small mesh")
            .to_le_bytes(),
    );
    for triangle in triangles {
        for _ in 0..3 {
            bytes.extend(0.0_f32.to_le_bytes());
        }
        for &corner in triangle {
            let p = points[corner as usize];
            for value in [p.x, p.y, p.z] {
                bytes.extend((value as f32).to_le_bytes());
            }
        }
        bytes.extend(0_u16.to_le_bytes());
    }
    bytes
}

/// The mesh as a PLY file in `format` (`ascii`, `binary_little_endian` or
/// `binary_big_endian`). Every vertex carries a normal (`float nx ny nz`) and a colour
/// (`uchar red green blue`) after its position, to be skipped by the reader, and the file
/// has a comment and an `edge` element after the faces, also to be skipped.
pub fn ply_of(format: &str, points: &[DVec3], faces: &[Vec<u32>]) -> Vec<u8> {
    let mut bytes = format!(
        "ply\nformat {format} 1.0\ncomment written by a test\nelement vertex {}\n\
         property float x\nproperty float y\nproperty float z\n\
         property float nx\nproperty float ny\nproperty float nz\n\
         property uchar red\nproperty uchar green\nproperty uchar blue\n\
         element face {}\nproperty list uchar int vertex_indices\nproperty int flags\n\
         element edge 2\nproperty int a\nproperty int b\nend_header\n",
        points.len(),
        faces.len()
    )
    .into_bytes();
    let big = format == "binary_big_endian";
    let binary = format != "ascii";
    // Pushes `value` as text (ascii) or as the bytes of its type in the file's byte order.
    let put_f32 = |bytes: &mut Vec<u8>, value: f32| {
        if !binary {
            bytes.extend(format!("{value} ").bytes());
        } else if big {
            bytes.extend(value.to_be_bytes());
        } else {
            bytes.extend(value.to_le_bytes());
        }
    };
    let put_i32 = |bytes: &mut Vec<u8>, value: i32| {
        if !binary {
            bytes.extend(format!("{value} ").bytes());
        } else if big {
            bytes.extend(value.to_be_bytes());
        } else {
            bytes.extend(value.to_le_bytes());
        }
    };
    let put_u8 = |bytes: &mut Vec<u8>, value: u8| {
        if binary {
            bytes.push(value);
        } else {
            bytes.extend(format!("{value} ").bytes());
        }
    };
    for p in points {
        for value in [p.x, p.y, p.z, 0.0, 0.0, 1.0] {
            put_f32(&mut bytes, value as f32);
        }
        for value in [200, 100, 50] {
            put_u8(&mut bytes, value);
        }
        if !binary {
            bytes.push(b'\n');
        }
    }
    for face in faces {
        put_u8(&mut bytes, u8::try_from(face.len()).expect("a small face"));
        for &corner in face {
            put_i32(&mut bytes, i32::try_from(corner).expect("a small index"));
        }
        put_i32(&mut bytes, 7);
        if !binary {
            bytes.push(b'\n');
        }
    }
    for (a, b) in [(0, 1), (1, 2)] {
        put_i32(&mut bytes, a);
        put_i32(&mut bytes, b);
        if !binary {
            bytes.push(b'\n');
        }
    }
    bytes
}
