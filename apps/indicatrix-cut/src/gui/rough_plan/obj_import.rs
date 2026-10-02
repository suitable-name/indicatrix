//! Importing a Wavefront OBJ file as the rough: its vertices' convex hull becomes the base.

use super::{
    editing::{input_error, install_base},
    host::{Host, on_idle_host},
};
use crate::gui::pickers::{PickerFilter, PickerKind, PickerRequest, pick};
use glam::DVec3;
use indicatrix_cut_core::rough_plan::{RoughBase, import_hull};
use std::{path::Path, rc::Rc};

/// The vertices (`v x y z`, in mm) of the OBJ text `text`; faces, normals, textures and
/// everything else are ignored, since only the convex outline is used.
///
/// # Errors
///
/// Returns the message for a vertex line without three numbers, or for a file without
/// vertices.
pub(super) fn parse_obj_vertices(text: &str) -> Result<Vec<DVec3>, String> {
    let mut points = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let mut words = line.split_whitespace();
        if words.next() != Some("v") {
            continue;
        }
        let mut coordinate = || {
            words
                .next()
                .and_then(|word| word.parse::<f64>().ok())
                .ok_or_else(|| format!("line {} is not a vertex with three numbers", index + 1))
        };
        points.push(DVec3::new(coordinate()?, coordinate()?, coordinate()?));
    }
    if points.is_empty() {
        return Err("the file has no vertices (`v x y z` lines)".to_string());
    }
    Ok(points)
}

/// The rough base of the OBJ file at `path`.
fn base_from_file(path: &Path) -> Result<RoughBase, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("could not read the file: {e}"))?;
    let points = parse_obj_vertices(&String::from_utf8_lossy(&bytes))?;
    import_hull(&points).map_err(|e| e.to_string())
}

/// Asks for an OBJ file and, on a pick, makes its convex outline the rough.
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
            Ok(base) => install_base(host, base),
            Err(message) => input_error(host, &format!("OBJ import failed: {message}")),
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertices_are_read_and_everything_else_is_skipped() {
        let text = "# cube
v 0 0 0
v 1.5 0 0 1
vn 0 0 1
v -1e1 2 3 0.5 0.5 0.5
f 1 2 3
";
        let points = parse_obj_vertices(text).expect("parses");
        assert_eq!(points.len(), 3);
        assert_eq!(points[2], DVec3::new(-10.0, 2.0, 3.0));
    }

    #[test]
    fn a_short_vertex_line_or_an_empty_file_is_refused() {
        assert!(
            parse_obj_vertices(
                "v 1 2
"
            )
            .unwrap_err()
            .contains("line 1")
        );
        assert!(
            parse_obj_vertices(
                "f 1 2 3
"
            )
            .is_err()
        );
    }
}
