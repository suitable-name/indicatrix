//! Importing a mesh file (Wavefront OBJ, STL or PLY) as the rough: a closed mesh becomes the
//! (possibly non-convex) rough; anything else falls back to the convex hull of its vertices,
//! with a note. The file's numbers are read in the unit chosen in the window and scaled to
//! millimetres.
//!
//! The parsers live in [`crate::mesh_io`] (no window, so their tests run in the workspace);
//! this module reads the file, picks the unit, runs the size check and installs the result.
//!
//! The file is read, parsed and turned into a rough (the convex hull, the mesh checks and
//! the search tree) on a worker thread ([`mesh_task::spawn`]): a scan of 50,000 triangles
//! takes seconds, and the window must stay alive meanwhile. A file larger than
//! [`MAX_MESH_FILE_BYTES`] is refused from its size alone, before any of it is read.

use super::{
    cut_rows::fmt_num,
    editing::install_imported_base,
    host::{Host, on_idle_host},
    mesh_task,
    saved::show_error,
};
#[cfg(test)]
pub(super) use crate::mesh_io::{
    fixtures::{C_SHAPE_OBJ, CUBE_OBJ},
    parse_obj_mesh,
};
use crate::{
    RoughPlanModel,
    gui::pickers::{PickerFilter, PickerKind, PickerRequest, pick},
    mesh_io::{MeshUnit, parse_mesh},
};
use indicatrix_cut_core::rough_plan::{RoughBase, ShapeError, import_mesh};
use slint::ComponentHandle;
use std::{
    io::Read,
    path::{Path, PathBuf},
    rc::Rc,
};

/// The largest mesh file the planner reads, in bytes (64 MB).
///
/// A mesh at the planner's limit of 50,000 triangles is about 5 MB as OBJ text, even with
/// texture and normal indices on every corner (2.5 MB as binary STL), so this leaves more
/// than ten times that. A larger file is a scan with far more triangles than the planner
/// takes (it would fall back to its hull after being read and parsed in full), or a file
/// with much more than geometry in it, and reading it whole would only fill the memory.
pub(super) const MAX_MESH_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// The smallest the largest side of an imported rough may be, in mm. Below this the file is
/// almost certainly in metres, centimetres or inches: no stone is cut from a rough under a
/// millimetre across (the planner's default minimum stone width is 1 mm itself).
const MIN_ROUGH_MM: f64 = 1.0;

/// `bytes` megabytes as the text the messages use ("64 MB", "120.5 MB").
fn megabytes(bytes: u64) -> String {
    fmt_num(bytes as f64 / (1024.0 * 1024.0), 1)
}

/// Fails when a file of `len` bytes is more than `max_bytes`.
///
/// # Errors
///
/// Returns the message for the file being too large, naming both sizes.
fn check_file_size(len: u64, max_bytes: u64) -> Result<(), String> {
    if len > max_bytes {
        Err(format!(
            "the file is {} MB, and the planner reads mesh files up to {} MB. \
             Reduce the number of triangles in a mesh tool first.",
            megabytes(len),
            megabytes(max_bytes)
        ))
    } else {
        Ok(())
    }
}

/// The bytes of the file at `path`, reading at most `max_bytes + 1` of them: a file that
/// grew since its size was looked at cannot fill the memory either.
fn read_bounded(path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The bytes of the mesh file at `path`, read with `read` (given the path and the cap) only
/// after the file's size, from its metadata, was found to be within `max_bytes`: an
/// oversized file is refused without one byte of it being read.
///
/// # Errors
///
/// Returns the message for a file that cannot be read or is too large.
fn read_bytes_with(
    path: &Path,
    max_bytes: u64,
    read: impl FnOnce(&Path, u64) -> std::io::Result<Vec<u8>>,
) -> Result<Vec<u8>, String> {
    let len = std::fs::metadata(path)
        .map_err(|e| format!("could not read the file: {e}"))?
        .len();
    check_file_size(len, max_bytes)?;
    let bytes = read(path, max_bytes).map_err(|e| format!("could not read the file: {e}"))?;
    // The file may have grown after its size was looked at.
    check_file_size(u64::try_from(bytes.len()).unwrap_or(u64::MAX), max_bytes)?;
    Ok(bytes)
}

/// The bytes of the mesh file at `path`, within the planner's file size cap (see
/// [`read_bytes_with`]); for the inclusion files, which are read like a rough's.
///
/// # Errors
///
/// Returns the message for a file that cannot be read or is too large.
pub(super) fn read_mesh_file(path: &Path) -> Result<Vec<u8>, String> {
    read_bytes_with(path, MAX_MESH_FILE_BYTES, read_bounded)
}

/// What to tell the user about the unit when the rough's largest side, `largest_mm`, is not
/// believable with the file read as `unit`: the unit that would fix it, when one does, and
/// how to choose it. Never applied automatically.
fn units_hint(largest_mm: f64, unit: MeshUnit) -> String {
    let raw = largest_mm / unit.factor_to_mm();
    let Some(suggested) = MeshUnit::suggest(raw, unit) else {
        return format!(
            "The planner read the numbers in the file as {}. If the file uses another unit \
             (mm, cm, m, inch or \u{b5}m), choose it in the unit box next to the mesh choice, \
             or scale the file to millimetres in a mesh tool, and import it again.",
            unit.plural()
        );
    };
    format!(
        "The file's largest side is {} units; read as {} that is {} mm. Choose {} in the \
         unit box next to the mesh choice and import it again.",
        fmt_num(raw, 3),
        suggested.plural(),
        fmt_num(raw * suggested.factor_to_mm(), 1),
        suggested.label()
    )
}

/// Why a rough with the base `base` (the file read in `unit`) cannot be the rough the file
/// was meant to be, with a suggested unit; `None` when its size is believable (at least
/// [`MIN_ROUGH_MM`] across at its widest, and a size the planner takes).
fn size_refusal(base: &RoughBase, unit: MeshUnit) -> Option<String> {
    let largest = base
        .bounding_box_extents()
        .into_iter()
        .fold(0.0_f64, f64::max);
    match base.validate() {
        Err(error @ (ShapeError::TooLarge | ShapeError::NonPositiveSize)) => Some(format!(
            "{error} The mesh is {} mm across at its widest. {}",
            fmt_num(largest, 1),
            units_hint(largest, unit)
        )),
        _ if largest < MIN_ROUGH_MM => Some(format!(
            "The mesh is only {} mm across at its widest, too small to cut stones from. {}",
            fmt_num(largest, 3),
            units_hint(largest, unit)
        )),
        _ => None,
    }
}

/// The rough base of the mesh file at `path` (OBJ, STL or PLY, its numbers in `unit`), and
/// the note to show when the closed mesh could not be used and the base is the convex hull
/// of its vertices instead. Everything slow happens in here, so it runs on a worker thread.
///
/// # Errors
///
/// Returns the message for a file that is too large, cannot be read or parsed, has no volume
/// or whose size is not believable in `unit`.
fn base_from_file(path: &Path, unit: MeshUnit) -> Result<(RoughBase, Option<String>), String> {
    let bytes = read_bytes_with(path, MAX_MESH_FILE_BYTES, read_bounded)?;
    let extension = path.extension().and_then(|e| e.to_str());
    let (mut points, triangles, split_note) = parse_mesh(&bytes, extension)?;
    drop(bytes);
    if unit != MeshUnit::Millimetre {
        let factor = unit.factor_to_mm();
        for point in &mut points {
            *point *= factor;
        }
    }
    let (base, note) = import_mesh(&points, &triangles).map_err(|e| e.to_string())?;
    if let Some(refusal) = size_refusal(&base, unit) {
        return Err(refusal);
    }
    // A smooth scan's simplified outline is told here, not by `import_mesh`: a saved plan
    // re-imports the same mesh and must come back without a note.
    // The same holds for what the mesh repair did (closed gaps, turned faces, filled holes):
    // only the mesh built by this import carries it, a saved plan stores the repaired mesh.
    let outline_note = match base {
        RoughBase::Hull { id, .. } => {
            use indicatrix_cut_core::rough_plan::shape::hull::{mesh, outline_note};
            let repaired = mesh(id).and_then(|m| m.repair_note().map(str::to_owned));
            match (repaired, outline_note(id)) {
                (Some(repair), Some(outline)) => Some(format!("{repair} {outline}")),
                (repair, outline) => repair.or(outline),
            }
        }
        _ => None,
    };
    let note = split_note.or(note).or(outline_note).or_else(|| {
        triangles
            .is_empty()
            .then(|| "The file has no faces, so its convex hull is the rough.".to_string())
    });
    Ok((base, note))
}

/// Reads the mesh file at `path` (its numbers in `unit`) on a worker thread and, when it is
/// ready, makes its mesh (or, failing that, its convex hull) the rough.
fn import_file(host: &Rc<Host>, path: PathBuf, unit: MeshUnit) {
    show_error(host, "");
    mesh_task::spawn(
        host,
        "Reading the mesh file...",
        move || base_from_file(&path, unit),
        |host, result| match result {
            Ok((base, note)) => install_imported_base(host, base, note.as_deref()),
            Err(message) => show_error(host, &format!("Mesh import failed: {message}")),
        },
    );
}

/// Asks for an OBJ, STL or PLY file and, on a pick, makes its mesh (or, failing that, its
/// convex hull) the rough, reading its numbers in the unit chosen in the window. The file is
/// read in the background (see [`import_file`]).
pub(super) fn start(host: &Rc<Host>) {
    let Some(main) = host.main.upgrade() else {
        return;
    };
    // Read now, before the dialog: the choice is the one the user made for this import.
    let unit = MeshUnit::from_index(host.window.global::<RoughPlanModel>().get_mesh_unit_index());
    let request = PickerRequest {
        kind: PickerKind::OpenFile,
        title: Some("Import rough from a mesh file".to_string()),
        filters: vec![PickerFilter {
            label: "Mesh (OBJ, STL, PLY)".to_string(),
            extensions: ["obj", "stl", "ply"].map(str::to_string).to_vec(),
        }],
        default_file_name: None,
        starting_dir: None,
    };
    pick(&main, request, move |_, path| {
        let Some(path) = path else {
            return;
        };
        on_idle_host(move |host| import_file(host, path, unit));
    });
}

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
    use crate::mesh_io::parse_obj_noted;
    use glam::DVec3;
    use indicatrix_cut_core::rough_plan::{MAX_MESH_TRIANGLES, RoughModel};

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
        assert_eq!(triangles.len(), 0, "the face gives no triangles");
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

    /// A file with four vertices and `copies` lines of `face`.
    fn many_faces(face: &str, copies: usize) -> String {
        let mut text = String::from("v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n");
        for _ in 0..copies {
            text.push_str(face);
            text.push('\n');
        }
        text
    }

    #[test]
    fn a_file_over_the_triangle_limit_is_counted_out_while_it_is_read() {
        let over = MAX_MESH_TRIANGLES + 1;
        let (points, triangles, note) =
            parse_obj_noted(&many_faces("f 1 2 3", over)).expect("parses");
        assert_eq!(points.len(), 4, "the vertices are kept for the hull");
        assert!(triangles.is_empty(), "no triangle is kept");
        let note = note.expect("a note says why the mesh is not used");
        assert!(note.contains(&format!("{over} triangles")), "{note}");
        assert!(note.contains("convex outline"), "{note}");
        assert!(
            note.contains(&MAX_MESH_TRIANGLES.to_string()),
            "the limit is named: {note}"
        );
        // A polygon counts as its triangles: one quad past half the limit is two over it.
        let quads = MAX_MESH_TRIANGLES / 2 + 1;
        let (_, triangles, note) =
            parse_obj_noted(&many_faces("f 1 2 3 4", quads)).expect("parses");
        assert!(
            triangles.is_empty(),
            "quads past the limit keep no triangle"
        );
        assert!(
            note.expect("a note")
                .contains(&format!("{} triangles", 2 * quads))
        );
    }

    #[test]
    fn a_file_at_the_triangle_limit_is_read_in_full() {
        let (_, triangles, note) =
            parse_obj_noted(&many_faces("f 1 2 3", MAX_MESH_TRIANGLES)).expect("parses");
        assert_eq!(triangles.len(), MAX_MESH_TRIANGLES);
        assert_eq!(note, None);
    }

    #[test]
    fn faces_past_the_triangle_limit_are_not_checked_but_vertices_still_are() {
        // The mesh is not used past the limit, so a bad index in the rest of the file does
        // not stop the import (its hull is used) ...
        let mut text = many_faces("f 1 2 3", MAX_MESH_TRIANGLES + 10);
        text.push_str("f 1 2 99\nf 1\n");
        let (_, triangles, note) = parse_obj_noted(&text).expect("the hull import goes on");
        assert!(triangles.is_empty() && note.is_some());
        // ... but a broken vertex line still does.
        text.push_str("v 1 2\n");
        assert!(
            parse_obj_noted(&text)
                .unwrap_err()
                .contains("is not a vertex")
        );
    }

    /// A fresh empty directory for a test, named after the test and the process.
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-obj-import-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the test directory is created");
        dir
    }

    #[test]
    fn a_file_over_the_cap_is_refused_before_any_of_it_is_read() {
        let dir = scratch_dir("cap");
        let path = dir.join("big.obj");
        std::fs::write(&path, [b'#'; 40]).expect("the test file is written");
        let reader_ran = std::cell::Cell::new(false);
        let message = read_bytes_with(&path, 16, |_, _| {
            reader_ran.set(true);
            Ok(Vec::new())
        })
        .expect_err("40 bytes are over a cap of 16");
        assert!(!reader_ran.get(), "the size alone refused the file");
        assert!(
            message.contains("Reduce the number of triangles"),
            "{message}"
        );
        // A file at the cap is read, whole.
        let text = read_bytes_with(&path, 40, read_bounded).expect("40 bytes are within 40");
        assert_eq!(text.len(), 40);
        std::fs::remove_dir_all(&dir).expect("the test directory is removed");
    }

    #[test]
    fn the_cap_message_names_both_sizes_in_megabytes() {
        assert_eq!(
            check_file_size(MAX_MESH_FILE_BYTES, MAX_MESH_FILE_BYTES),
            Ok(())
        );
        let message = check_file_size(MAX_MESH_FILE_BYTES + 1, MAX_MESH_FILE_BYTES).unwrap_err();
        assert!(message.contains("up to 64 MB"), "{message}");
        let message = check_file_size(126_353_408, MAX_MESH_FILE_BYTES).unwrap_err();
        assert!(message.starts_with("the file is 120.5 MB"), "{message}");
    }

    #[test]
    fn a_file_that_grew_after_its_size_was_looked_at_is_still_bounded() {
        let dir = scratch_dir("bounded");
        let path = dir.join("grown.obj");
        std::fs::write(&path, [b'#'; 10]).expect("the test file is written");
        // The reader stops one byte past the cap, however long the file is.
        assert_eq!(read_bounded(&path, 4).expect("readable").len(), 5);
        // A reader that returns more than the cap (the file grew after its size was looked
        // at: 10 bytes are within the cap of 10) is refused as well.
        let message = read_bytes_with(&path, 10, |_, _| Ok(vec![b'#'; 20]))
            .expect_err("20 bytes are over a cap of 10");
        assert!(message.contains("the file is"), "{message}");
        std::fs::remove_dir_all(&dir).expect("the test directory is removed");
    }

    #[test]
    fn a_missing_or_foreign_file_gives_a_plain_message_or_its_readable_part() {
        let dir = scratch_dir("missing");
        let message = read_bytes_with(&dir.join("none.obj"), MAX_MESH_FILE_BYTES, read_bounded)
            .expect_err("there is no such file");
        assert!(message.starts_with("could not read the file"), "{message}");
        // Bytes that are not UTF-8 are replaced, not refused.
        let path = dir.join("latin.obj");
        std::fs::write(&path, b"# caf\xe9\nv 0 0 0\n").expect("the test file is written");
        let text = read_bytes_with(&path, MAX_MESH_FILE_BYTES, read_bounded).expect("readable");
        assert!(String::from_utf8_lossy(&text).contains("v 0 0 0"));
        std::fs::remove_dir_all(&dir).expect("the test directory is removed");
    }

    #[test]
    fn a_whole_file_goes_through_the_worker_job() {
        let dir = scratch_dir("job");
        let cube = dir.join("cube.obj");
        std::fs::write(&cube, CUBE_OBJ).expect("the test file is written");
        let (base, note) = base_from_file(&cube, MeshUnit::Millimetre).expect("a cube imports");
        assert_eq!(note, None);
        for side in base.bounding_box_extents() {
            assert!((side - 20.0).abs() < 1e-9, "{side}");
        }

        let c_shape = dir.join("c.obj");
        std::fs::write(&c_shape, C_SHAPE_OBJ).expect("the test file is written");
        let (base, note) =
            base_from_file(&c_shape, MeshUnit::Millimetre).expect("the C-shape imports");
        assert_eq!(note, None);
        assert!(RoughModel::new(base, Vec::new()).mesh().is_some());

        let no_faces = dir.join("points.obj");
        std::fs::write(&no_faces, "v 0 0 0\nv 9 0 0\nv 0 9 0\nv 0 0 9\n").expect("written");
        let (_, note) =
            base_from_file(&no_faces, MeshUnit::Millimetre).expect("a point cloud imports");
        assert!(note.expect("a note").contains("no faces"));

        let many = dir.join("many.obj");
        let mut heavy = String::from("v 0 0 0\nv 9 0 0\nv 0 9 0\nv 0 0 9\n");
        heavy.push_str(&"f 1 2 3\n".repeat(MAX_MESH_TRIANGLES + 1));
        std::fs::write(&many, heavy).expect("the test file is written");
        let (_, note) = base_from_file(&many, MeshUnit::Millimetre)
            .expect("too many triangles falls back to the hull");
        assert!(note.expect("a note").contains("convex outline"));
        std::fs::remove_dir_all(&dir).expect("the test directory is removed");
    }

    #[test]
    fn a_file_in_other_units_is_refused_with_a_units_hint() {
        let (points, _) = parse_obj_mesh(CUBE_OBJ).expect("parses");
        let scaled = |factor: f64| -> Vec<DVec3> { points.iter().map(|&p| p * factor).collect() };
        let hull = |factor: f64| import_mesh(&scaled(factor), &[]).expect("a hull").0;

        let mm = MeshUnit::Millimetre;
        assert_eq!(
            size_refusal(&hull(1.0), mm),
            None,
            "20 mm is a believable rough"
        );

        // In metres the cube is 0.02 units: far too small.
        let message = size_refusal(&hull(0.001), mm).expect("0.02 mm is not a rough");
        assert!(message.contains("only 0.02 mm across"), "{message}");
        assert!(
            message.contains("largest side is 0.02 units; read as metres that is 20 mm"),
            "{message}"
        );
        assert!(message.contains("Choose m in the unit box"), "{message}");

        // In micrometres it is 20,000: over the planner's 2000 mm.
        let message = size_refusal(&hull(1000.0), mm).expect("20000 mm is too large");
        assert!(message.contains("must not exceed 2000 mm"), "{message}");
        assert!(message.contains("20000 mm across"), "{message}");
        assert!(
            message.contains("read as micrometres that is 20 mm"),
            "{message}"
        );

        // No unit fixes a file that is far off in every one: the hint only explains.
        let message = size_refusal(&hull(1e7), mm).expect("far too large");
        assert!(
            message.contains("read the numbers in the file as millimetres"),
            "{message}"
        );
        assert!(message.ends_with("import it again."), "{message}");
    }

    #[test]
    fn a_rough_under_a_millimetre_across_is_refused_and_one_millimetre_is_not() {
        let block = |x_mm| RoughBase::Block {
            x_mm,
            y_mm: 0.5,
            z_mm: 0.5,
        };
        assert_eq!(size_refusal(&block(1.0), MeshUnit::Millimetre), None);
        assert!(size_refusal(&block(0.99), MeshUnit::Millimetre).is_some());
        assert_eq!(size_refusal(&block(2000.0), MeshUnit::Millimetre), None);
        assert!(size_refusal(&block(2000.5), MeshUnit::Millimetre).is_some());
    }

    #[test]
    fn an_imported_file_of_the_wrong_size_never_becomes_the_rough() {
        let dir = scratch_dir("units");
        let path = dir.join("metres.obj");
        let metres = CUBE_OBJ
            .lines()
            .map(|line| {
                line.strip_prefix("v ").map_or_else(
                    || line.to_string(),
                    |rest| {
                        let scaled: Vec<String> = rest
                            .split_whitespace()
                            .map(|word| {
                                format!("{}", word.parse::<f64>().expect("a number") / 1000.0)
                            })
                            .collect();
                        format!("v {}", scaled.join(" "))
                    },
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, metres).expect("the test file is written");
        let message = base_from_file(&path, MeshUnit::Millimetre).expect_err("0.02 mm is refused");
        assert!(
            message.contains("read as metres that is 20 mm"),
            "{message}"
        );
        // The suggestion is only shown: choosing the unit is what imports it.
        let (base, note) = base_from_file(&path, MeshUnit::Metre).expect("read as metres");
        assert_eq!(note, None);
        for side in base.bounding_box_extents() {
            assert!((side - 20.0).abs() < 1e-9, "{side}");
        }
        std::fs::remove_dir_all(&dir).expect("the test directory is removed");
    }

    #[test]
    fn every_format_and_unit_goes_through_the_worker_job() {
        use crate::mesh_io::fixtures::{ascii_stl_of, binary_stl_of, ply_of};
        let dir = scratch_dir("formats");
        let (points, triangles) = parse_obj_mesh(C_SHAPE_OBJ).expect("parses");
        let faces: Vec<Vec<u32>> = triangles.iter().map(|t| Vec::from(*t)).collect();
        // The C-shape in centimetres (2 x 2 x 2 cm) as each format.
        let cm: Vec<DVec3> = points.iter().map(|&p| p / 10.0).collect();
        let files: Vec<(&str, Vec<u8>)> = vec![
            ("c.stl", ascii_stl_of(&cm, &triangles).into_bytes()),
            ("cb.STL", binary_stl_of("solid", &cm, &triangles)),
            ("c.ply", ply_of("binary_little_endian", &cm, &faces)),
            ("cbe.ply", ply_of("binary_big_endian", &cm, &faces)),
            ("ca.ply", ply_of("ascii", &cm, &faces)),
        ];
        for (name, bytes) in files {
            let path = dir.join(name);
            std::fs::write(&path, bytes).expect("the test file is written");
            let (base, note) = base_from_file(&path, MeshUnit::Centimetre).expect(name);
            assert_eq!(note, None, "{name}");
            for side in base.bounding_box_extents() {
                assert!((side - 20.0).abs() < 1e-4, "{name}: {side}");
            }
            let model = RoughModel::new(base, Vec::new());
            let mesh = model
                .mesh()
                .unwrap_or_else(|| panic!("{name} keeps its mesh"));
            assert!(
                (mesh.volume() - 6000.0).abs() < 1e-2,
                "{name}: {}",
                mesh.volume()
            );
            // Read as millimetres the same file is 2 mm across: believable, and 1000 times
            // too small a volume. Only the unit choice tells them apart.
            let (base, _) = base_from_file(&path, MeshUnit::Millimetre).expect(name);
            assert!(
                base.bounding_box_extents()
                    .iter()
                    .all(|&s| (s - 2.0).abs() < 1e-4)
            );
        }
        std::fs::remove_dir_all(&dir).expect("the test directory is removed");
    }
}
