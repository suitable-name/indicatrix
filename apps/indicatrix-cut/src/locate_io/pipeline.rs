//! The two jobs of locating an inclusion that run on a worker thread: aligning the mesh to the
//! rig from the stone's outlines, and solving the marks for the inclusion with the overlay
//! that verifies it. Both are thin wrappers over the core's `rough_plan::locate`, with the
//! messages put in words a cutter reads; neither touches a window.

use std::fmt::Write as _;

use indicatrix_cut_core::rough_plan::{
    locate::{
        AlignOptions, AlignResult, Ghost, LocateError, LocateOptions, Located, LocatedPolyline,
        MAX_GHOST_BOUNCES, OutlineView, Reprojection, RigProfile, Rigid, Scene, ViewStatus,
        align_mesh_to_rig, locate_point, locate_polyline, predict_ghosts, reproject,
    },
    shape::RoughMesh,
};

use super::{
    marks::{MarkKind, MarkSet},
    report::status_words,
};

/// A ghost-image prediction keeps the images whose ray passes within this many mm of the point,
/// at least; it grows with the point's uncertainty (three times its RMS).
const GHOST_MISS_FLOOR_MM: f64 = 0.25;

/// What was located.
#[derive(Debug, Clone, PartialEq)]
pub enum Found {
    /// A compact inclusion.
    Point(Located),
    /// A line or polygon, vertex by vertex.
    Line(LocatedPolyline),
}

impl Found {
    /// The margin the uncertainty asks for, in mm (`max(0.3, 2 * rms)`).
    #[must_use]
    pub fn margin_mm(&self) -> f64 {
        match self {
            Self::Point(located) => located.suggested_margin_mm(),
            Self::Line(line) => line.suggested_margin_mm(),
        }
    }
}

/// Where one located vertex shows in one photo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexReprojection {
    /// The vertex (0 for a point).
    pub vertex: usize,
    /// The reprojection, with the error against the user's click when there was one.
    pub reprojection: Reprojection,
}

/// What the verification overlay of one photo shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ViewOverlay {
    /// The located vertices re-projected into the photo.
    pub reprojections: Vec<VertexReprojection>,
    /// The predicted ghost images of a located point (none for a line).
    pub ghosts: Vec<Ghost>,
}

/// A located inclusion with the overlay of every view.
#[derive(Debug, Clone, PartialEq)]
pub struct Solution {
    /// The point or the line.
    pub found: Found,
    /// One overlay per view of the rig.
    pub overlays: Vec<ViewOverlay>,
}

/// Aligns `mesh` to the rig from the outlines, starting from `start`.
pub fn align(
    mesh: &RoughMesh,
    rig: &RigProfile,
    outlines: &[OutlineView],
    start: Rigid,
) -> Result<AlignResult, String> {
    align_mesh_to_rig(mesh, rig, outlines, start, &AlignOptions::default())
        .map_err(|error| error.to_string())
}

/// Solves the marks for the inclusion, and re-projects the result into every photo.
///
/// A point needs marks in at least two views; a line or polygon needs the same number of
/// vertices in every view that has one. The message of a refusal names the views and what
/// happened to their marks.
pub fn solve(
    mesh: &RoughMesh,
    rig: &RigProfile,
    alignment: Rigid,
    marks: &MarkSet,
) -> Result<Solution, String> {
    rig.validate().map_err(|error| error.to_string())?;
    let names: Vec<String> = rig.views.iter().map(|view| view.name.clone()).collect();
    let scene = Scene::new(mesh, rig, alignment);
    let options = LocateOptions::default();
    let views = rig.views.len();
    if marks.kind == MarkKind::Point {
        let point_marks = marks.point_marks();
        if point_marks.len() < 2 {
            return Err("Mark the inclusion in at least two views.".to_owned());
        }
        let located = locate_point(&scene, &point_marks, &options)
            .map_err(|error| locate_error_text(&error, &names))?;
        let target = located.point_vec();
        let miss_mm = (3.0 * located.rms_mm).max(GHOST_MISS_FLOOR_MM);
        let overlays = (0..views)
            .map(|view| {
                let marked = point_marks
                    .iter()
                    .find(|mark| mark.view == view)
                    .map(|mark| mark.pixel);
                ViewOverlay {
                    reprojections: reproject(&scene, view, target, marked)
                        .map(|reprojection| VertexReprojection {
                            vertex: 0,
                            reprojection,
                        })
                        .into_iter()
                        .collect(),
                    ghosts: predict_ghosts(&scene, view, target, MAX_GHOST_BOUNCES, miss_mm),
                }
            })
            .collect();
        return Ok(Solution {
            found: Found::Point(located),
            overlays,
        });
    }
    if let Some(problem) = marks.vertex_count_problem(&names) {
        return Err(problem);
    }
    let lines = marks.polylines();
    if lines.len() < 2 {
        return Err(format!(
            "Mark the {} in at least two views.",
            marks.kind.word()
        ));
    }
    let located = locate_polyline(&scene, &lines, marks.kind.closed(), &options)
        .map_err(|error| locate_error_text(&error, &names))?;
    let overlays = (0..views)
        .map(|view| {
            let drawn = lines.iter().find(|line| line.view == view);
            let reprojections = located
                .vertices
                .iter()
                .enumerate()
                .filter_map(|(vertex, found)| {
                    let marked = drawn.and_then(|line| line.pixels.get(vertex)).copied();
                    reproject(&scene, view, found.point_vec(), marked).map(|reprojection| {
                        VertexReprojection {
                            vertex,
                            reprojection,
                        }
                    })
                })
                .collect();
            ViewOverlay {
                reprojections,
                ghosts: Vec::new(),
            }
        })
        .collect();
    Ok(Solution {
        found: Found::Line(located),
        overlays,
    })
}

/// A refusal of the core in words: the views whose marks could not be used, and why.
fn locate_error_text(error: &LocateError, names: &[String]) -> String {
    let name = |view: usize| names.get(view).map_or("a view", String::as_str);
    match error {
        LocateError::TooFewViews { usable, views } => {
            let mut text = format!(
                "Only {usable} of the marks reach the stone's interior; at least 2 are needed."
            );
            for report in views {
                if report.status != ViewStatus::Used {
                    let _ = write!(
                        text,
                        " {}: {}.",
                        name(report.view),
                        status_words(report.status)
                    );
                }
            }
            text
        }
        LocateError::AtVertex { index, reason } => {
            format!("Vertex {}: {}", index + 1, locate_error_text(reason, names))
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locate_io::marks::ClickMode;
    use glam::DVec3;
    use indicatrix_cut_core::rough_plan::locate::{Projection, box_mesh};

    /// A cube of 10 mm around the origin, a rig of eight cameras, and the stone's index equal
    /// to the surrounding index so the rays do not bend: the located point is then the
    /// straight-line one and can be checked exactly.
    fn fixture() -> (RoughMesh, RigProfile) {
        let mesh = box_mesh(DVec3::splat(5.0)).expect("a box");
        let views = RigProfile::side_layout(
            150.0,
            30.0,
            Projection::Pinhole { focal_px: 4000.0 },
            [4000, 3000],
        );
        let mut rig = RigProfile::new("test", views, 1.5);
        rig.surround_n = 1.5;
        (mesh, rig)
    }

    fn mark_point(rig: &RigProfile, point: DVec3, views: &[usize]) -> MarkSet {
        let mut marks = MarkSet::new(rig.views.len());
        for &view in views {
            let pixel = rig.views[view].project(point).expect("in front");
            marks.click(view, ClickMode::Mark(MarkKind::Point), pixel.to_array());
        }
        marks
    }

    #[test]
    fn a_point_marked_in_every_view_is_found_where_it_is() {
        let (mesh, rig) = fixture();
        let point = DVec3::new(0.8, -0.5, 0.3);
        let marks = mark_point(&rig, point, &[0, 1, 2, 3, 4, 5, 6, 7]);
        let solution = solve(&mesh, &rig, Rigid::IDENTITY, &marks).expect("solved");
        let Found::Point(located) = &solution.found else {
            panic!("a point");
        };
        assert!((located.point_vec() - point).length() < 1e-3, "{located:?}");
        assert_eq!(located.used_views, 8);
        assert_eq!(solution.overlays.len(), 8);
        assert!(
            solution
                .overlays
                .iter()
                .all(|overlay| overlay.reprojections.len() == 1),
            "every photo shows the point"
        );
        assert!(solution.found.margin_mm() >= 0.3);
    }

    #[test]
    fn one_mark_is_not_enough() {
        let (mesh, rig) = fixture();
        let marks = mark_point(&rig, DVec3::ZERO, &[0]);
        let message = solve(&mesh, &rig, Rigid::IDENTITY, &marks).unwrap_err();
        assert!(message.contains("at least two views"), "{message}");
    }

    #[test]
    fn a_line_needs_the_same_vertices_in_every_view() {
        let (mesh, rig) = fixture();
        let mut marks = MarkSet::new(8);
        let line = ClickMode::Mark(MarkKind::Line);
        for point in [DVec3::new(-1.0, 0.0, 0.0), DVec3::new(1.0, 0.5, 0.0)] {
            for view in [0, 2] {
                let pixel = rig.views[view].project(point).expect("in front");
                marks.click(view, line, pixel.to_array());
            }
        }
        let extra = rig.views[2]
            .project(DVec3::new(2.0, 0.5, 0.0))
            .expect("in front");
        marks.click(2, line, extra.to_array());
        let message = solve(&mesh, &rig, Rigid::IDENTITY, &marks).unwrap_err();
        assert!(message.contains("same line vertices"), "{message}");
    }

    #[test]
    fn a_line_is_located_vertex_by_vertex() {
        let (mesh, rig) = fixture();
        let mut marks = MarkSet::new(8);
        let line = ClickMode::Mark(MarkKind::Line);
        let ends = [DVec3::new(-1.0, 0.0, 0.0), DVec3::new(1.0, 0.5, 0.2)];
        for point in ends {
            for view in [0, 2, 4, 6] {
                let pixel = rig.views[view].project(point).expect("in front");
                marks.click(view, line, pixel.to_array());
            }
        }
        let solution = solve(&mesh, &rig, Rigid::IDENTITY, &marks).expect("solved");
        let Found::Line(found) = &solution.found else {
            panic!("a line");
        };
        assert_eq!(found.vertices.len(), 2);
        for (vertex, end) in found.vertices.iter().zip(ends) {
            assert!((vertex.point_vec() - end).length() < 1e-3);
        }
        assert_eq!(solution.overlays[0].ghosts.len(), 0);
    }

    #[test]
    fn a_mark_that_misses_the_stone_is_explained_by_view_name() {
        let (mesh, rig) = fixture();
        let mut marks = MarkSet::new(8);
        let point = ClickMode::Mark(MarkKind::Point);
        // Far outside the stone's image in two views.
        marks.click(0, point, [10.0, 10.0]);
        marks.click(1, point, [10.0, 10.0]);
        let message = solve(&mesh, &rig, Rigid::IDENTITY, &marks).unwrap_err();
        assert!(message.contains("Only 0 of the marks"), "{message}");
        assert!(message.contains("+X upper"), "{message}");
        assert!(message.contains("misses the stone"), "{message}");
    }

    #[test]
    fn alignment_without_outlines_is_refused_in_words() {
        let (mesh, rig) = fixture();
        let message = align(&mesh, &rig, &[], Rigid::IDENTITY).unwrap_err();
        assert!(message.contains("outline"), "{message}");
    }
}
