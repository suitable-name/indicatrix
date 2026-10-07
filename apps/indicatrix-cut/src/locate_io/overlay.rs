//! What the photo canvas draws, and the mapping between the photo's pixels and the canvas.
//!
//! The canvas shows a photo scaled to fit, so a click arrives as a fraction of the picture's
//! width and height (0 to 1) and a marker is placed by the same fractions: the canvas never
//! needs to know the photo's size. The photo is kept at a reduced size for display, but every
//! mark is stored in the pixels of the full-resolution photo, because that is what the rig's
//! focal length and principal point refer to. Fractions are the same for both, so the reduced
//! size never enters a mark.
//!
//! Polylines are drawn as SVG path commands in full-resolution pixels; the canvas scales them
//! with the picture.

use std::fmt::Write as _;

use super::{
    calib_marks::CalibMarks,
    marks::{MarkKind, MarkSet},
    pipeline::{Found, Solution},
    report::{ghost_label, reprojection_label},
};

/// The longest side of the picture kept for display, in pixels. A larger photo is shown
/// reduced; its marks are still in its own pixels.
pub const MAX_DISPLAY_SIDE: u32 = 2400;

/// Marker kinds, as the canvas colours them.
pub const KIND_MARK: i32 = 0;
/// A predicted ghost image (drawn hollow).
pub const KIND_GHOST: i32 = 1;
/// A re-projected solution (drawn hollow, with the error beside it).
pub const KIND_REPROJECTION: i32 = 2;
/// A vertex of a marked line or polygon.
pub const KIND_VERTEX: i32 = 3;
/// The cube's orientation dot.
pub const KIND_CORNER: i32 = 4;
/// A vertex of the stone's outline.
pub const KIND_OUTLINE: i32 = 5;

/// Path kinds: the stone's outline.
pub const PATH_OUTLINE: i32 = 0;
/// A marked line or polygon.
pub const PATH_LINE: i32 = 1;
/// An edge of the calibration cube.
pub const PATH_EDGE: i32 = 2;
/// The solution re-projected.
pub const PATH_PREDICTED: i32 = 3;

/// A marker on the canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkerSpec {
    /// Where, as fractions of the picture's width and height.
    pub fraction: [f32; 2],
    /// Which kind (the `KIND_` constants).
    pub kind: i32,
    /// The text beside it.
    pub label: String,
}

/// A path on the canvas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathSpec {
    /// SVG path commands in the photo's own pixels.
    pub commands: String,
    /// Which kind (the `PATH_` constants).
    pub kind: i32,
}

/// What the canvas draws for one view.
pub type Drawing = (Vec<MarkerSpec>, Vec<PathSpec>);

/// The size the photo of `full` pixels is kept at for display: the same shape, its longer side
/// at most [`MAX_DISPLAY_SIDE`].
#[must_use]
pub fn display_size(full: [u32; 2]) -> [u32; 2] {
    let longest = full[0].max(full[1]);
    if longest <= MAX_DISPLAY_SIDE {
        return full;
    }
    let scale = f64::from(MAX_DISPLAY_SIDE) / f64::from(longest);
    full.map(|side| ((f64::from(side) * scale).round() as u32).max(1))
}

/// The pixel of a photo of `size` at a click at `fraction` of the picture, clamped into the
/// photo.
#[must_use]
pub fn to_pixel(fraction: [f32; 2], size: [u32; 2]) -> [f64; 2] {
    let at = |fraction: f32, side: u32| f64::from(fraction).clamp(0.0, 1.0) * f64::from(side);
    [at(fraction[0], size[0]), at(fraction[1], size[1])]
}

/// The fractions of the picture of `pixel` in a photo of `size`, or `None` when the pixel is
/// outside the photo (or the photo has no size).
#[must_use]
pub fn to_fraction(pixel: [f64; 2], size: [u32; 2]) -> Option<[f32; 2]> {
    if size[0] == 0 || size[1] == 0 {
        return None;
    }
    let along = |value: f64, side: u32| {
        let fraction = value / f64::from(side);
        (0.0..=1.0).contains(&fraction).then_some(fraction as f32)
    };
    Some([along(pixel[0], size[0])?, along(pixel[1], size[1])?])
}

/// SVG path commands through `points`, closed when asked; `None` for fewer than two points.
#[must_use]
pub fn path_commands(points: &[[f64; 2]], closed: bool) -> Option<String> {
    if points.len() < 2 {
        return None;
    }
    let mut commands = String::new();
    for (index, point) in points.iter().enumerate() {
        let letter = if index == 0 { 'M' } else { 'L' };
        let _ = write!(commands, "{letter} {:.2} {:.2} ", point[0], point[1]);
    }
    if closed && points.len() >= 3 {
        commands.push('Z');
    }
    Some(commands.trim_end().to_owned())
}

/// A marker at `pixel`, when it lies inside the photo.
fn marker(pixel: [f64; 2], size: [u32; 2], kind: i32, label: String) -> Option<MarkerSpec> {
    to_fraction(pixel, size).map(|fraction| MarkerSpec {
        fraction,
        kind,
        label,
    })
}

/// Markers for every point of `points`, numbered when `numbered`.
fn vertex_markers(
    points: &[[f64; 2]],
    size: [u32; 2],
    kind: i32,
    numbered: bool,
) -> Vec<MarkerSpec> {
    points
        .iter()
        .enumerate()
        .filter_map(|(index, point)| {
            let label = if numbered {
                (index + 1).to_string()
            } else {
                String::new()
            };
            marker(*point, size, kind, label)
        })
        .collect()
}

/// What the canvas draws over the photo of `view` (of `size` pixels) while an inclusion is
/// located: the stone's outline, the marks, and, once solved, the re-projected solution with
/// its error in pixels and the predicted ghost images.
#[must_use]
pub fn locate_drawing(
    view: usize,
    size: [u32; 2],
    marks: &MarkSet,
    solution: Option<&Solution>,
) -> Drawing {
    let mut markers = Vec::new();
    let mut paths = Vec::new();
    if let Some(view_marks) = marks.views.get(view) {
        markers.extend(vertex_markers(
            &view_marks.outline,
            size,
            KIND_OUTLINE,
            false,
        ));
        if let Some(commands) = path_commands(&view_marks.outline, true) {
            paths.push(PathSpec {
                commands,
                kind: PATH_OUTLINE,
            });
        }
        if let Some(point) = view_marks.point
            && marks.kind == MarkKind::Point
        {
            markers.extend(marker(point, size, KIND_MARK, String::new()));
        }
        if marks.kind != MarkKind::Point {
            markers.extend(vertex_markers(&view_marks.path, size, KIND_VERTEX, true));
            if let Some(commands) = path_commands(&view_marks.path, marks.kind.closed()) {
                paths.push(PathSpec {
                    commands,
                    kind: PATH_LINE,
                });
            }
        }
    }
    if let Some(solution) = solution
        && let Some(overlay) = solution.overlays.get(view)
    {
        let line = matches!(solution.found, Found::Line(_));
        let mut predicted = Vec::new();
        for item in &overlay.reprojections {
            let reprojection = &item.reprojection;
            predicted.push(reprojection.pixel);
            let error = reprojection_label(reprojection.error_px);
            let label = if line {
                format!("{} {error}", item.vertex + 1)
            } else {
                error
            };
            markers.extend(marker(reprojection.pixel, size, KIND_REPROJECTION, label));
        }
        if line && let Some(commands) = path_commands(&predicted, marks.kind.closed()) {
            paths.push(PathSpec {
                commands,
                kind: PATH_PREDICTED,
            });
        }
        for ghost in &overlay.ghosts {
            markers.extend(marker(
                ghost.pixel,
                size,
                KIND_GHOST,
                ghost_label(ghost.bounces),
            ));
        }
    }
    (markers, paths)
}

/// What the canvas draws over the photo of `view` (of `size` pixels) while the rig is
/// calibrated: the cube's edge segments, the corner dot and the diagonal's clicks.
#[must_use]
pub fn calibration_drawing(view: usize, size: [u32; 2], marks: &CalibMarks) -> Drawing {
    let mut markers = Vec::new();
    let mut paths = Vec::new();
    let Some(view_marks) = marks.views.get(view) else {
        return (markers, paths);
    };
    let mut edges = String::new();
    for line in &view_marks.lines {
        if let Some(commands) = path_commands(&[line.a, line.b], false) {
            edges.push_str(&commands);
            edges.push(' ');
        }
        markers.extend(marker(line.a, size, KIND_VERTEX, String::new()));
        markers.extend(marker(line.b, size, KIND_VERTEX, String::new()));
    }
    if !edges.is_empty() {
        paths.push(PathSpec {
            commands: edges.trim_end().to_owned(),
            kind: PATH_EDGE,
        });
    }
    if let Some(pending) = view_marks.pending {
        markers.extend(marker(pending, size, KIND_VERTEX, "end?".to_owned()));
    }
    if let Some(dot) = view_marks.mark {
        markers.extend(marker(dot, size, KIND_CORNER, "dot".to_owned()));
    }
    for (edge, clicks) in view_marks.diagonal.iter().enumerate() {
        for click in clicks {
            markers.extend(marker(*click, size, KIND_MARK, format!("d{}", edge + 1)));
        }
        if let Some(commands) = path_commands(clicks, false) {
            paths.push(PathSpec {
                commands,
                kind: PATH_LINE,
            });
        }
    }
    (markers, paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locate_io::{calib_marks::CalibMode, marks::ClickMode};

    const SIZE: [u32; 2] = [4000, 3000];

    #[test]
    fn a_small_photo_is_shown_as_it_is_and_a_big_one_reduced_in_the_same_shape() {
        assert_eq!(display_size([1600, 1200]), [1600, 1200]);
        assert_eq!(display_size([2400, 1000]), [2400, 1000]);
        assert_eq!(display_size([4800, 3600]), [2400, 1800]);
        assert_eq!(display_size([3000, 6000]), [1200, 2400]);
        assert_eq!(display_size([100_000, 1]), [2400, 1]);
    }

    #[test]
    fn clicks_map_to_full_resolution_pixels_and_back() {
        assert_eq!(to_pixel([0.5, 0.5], SIZE), [2000.0, 1500.0]);
        assert_eq!(to_pixel([0.0, 1.0], SIZE), [0.0, 3000.0]);
        // A click slightly outside the picture lands on its edge.
        assert_eq!(to_pixel([-0.2, 1.7], SIZE), [0.0, 3000.0]);
        let fraction = to_fraction([1000.0, 750.0], SIZE).expect("inside");
        assert_eq!(fraction, [0.25, 0.25]);
        assert_eq!(to_pixel(fraction, SIZE), [1000.0, 750.0]);
    }

    #[test]
    fn a_pixel_outside_the_photo_has_no_fraction() {
        assert_eq!(to_fraction([-1.0, 5.0], SIZE), None);
        assert_eq!(to_fraction([4001.0, 5.0], SIZE), None);
        assert_eq!(to_fraction([5.0, 5.0], [0, 100]), None);
        assert!(
            to_fraction([4000.0, 3000.0], SIZE).is_some(),
            "the far corner counts"
        );
    }

    #[test]
    fn path_commands_are_svg_in_pixels() {
        assert_eq!(path_commands(&[[1.0, 2.0]], false), None);
        assert_eq!(
            path_commands(&[[1.0, 2.0], [3.5, 4.25]], false).as_deref(),
            Some("M 1.00 2.00 L 3.50 4.25")
        );
        assert_eq!(
            path_commands(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]], true).as_deref(),
            Some("M 0.00 0.00 L 10.00 0.00 L 10.00 10.00 Z")
        );
        // Two points do not make a polygon.
        assert!(
            !path_commands(&[[0.0, 0.0], [1.0, 1.0]], true)
                .unwrap()
                .ends_with('Z')
        );
    }

    #[test]
    fn the_locate_drawing_shows_the_outline_the_marks_and_numbers_the_vertices() {
        let mut marks = MarkSet::new(2);
        for pixel in [[100.0, 100.0], [900.0, 100.0], [900.0, 900.0]] {
            marks.click(0, ClickMode::Outline, pixel);
        }
        marks.click(0, ClickMode::Mark(MarkKind::Point), [500.0, 500.0]);
        let (markers, paths) = locate_drawing(0, SIZE, &marks, None);
        assert_eq!(markers.iter().filter(|m| m.kind == KIND_OUTLINE).count(), 3);
        assert_eq!(markers.iter().filter(|m| m.kind == KIND_MARK).count(), 1);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].kind, PATH_OUTLINE);
        assert!(paths[0].commands.ends_with('Z'));
        // The other view is empty.
        assert_eq!(
            locate_drawing(1, SIZE, &marks, None),
            (Vec::new(), Vec::new())
        );

        let mut lines = MarkSet::new(2);
        for pixel in [[10.0, 10.0], [20.0, 20.0]] {
            lines.click(1, ClickMode::Mark(MarkKind::Line), pixel);
        }
        let (markers, paths) = locate_drawing(1, SIZE, &lines, None);
        let labels: Vec<&str> = markers.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["1", "2"]);
        assert_eq!(paths[0].kind, PATH_LINE);
    }

    #[test]
    fn a_solution_adds_a_reprojection_with_its_error_and_hollow_ghosts() {
        use crate::locate_io::pipeline::{VertexReprojection, ViewOverlay};
        use indicatrix_cut_core::rough_plan::locate::{Ghost, InsideState, Located, Reprojection};

        let located = Located {
            point: [1.0, 2.0, 3.0],
            rms_mm: 0.05,
            sigma_mm: 0.05,
            used_views: 2,
            inside: InsideState::Inside,
            views: Vec::new(),
        };
        let solution = Solution {
            found: Found::Point(located),
            overlays: vec![ViewOverlay {
                reprojections: vec![VertexReprojection {
                    vertex: 0,
                    reprojection: Reprojection {
                        view: 0,
                        pixel: [100.0, 100.0],
                        miss_mm: 0.0,
                        error_px: Some(2.04),
                    },
                }],
                ghosts: vec![Ghost {
                    view: 0,
                    bounces: 2,
                    pixel: [300.0, 300.0],
                    miss_mm: 0.1,
                }],
            }],
        };
        let marks = MarkSet::new(1);
        let (markers, paths) = locate_drawing(0, SIZE, &marks, Some(&solution));
        assert_eq!(paths.len(), 0);
        let reprojected = markers
            .iter()
            .find(|m| m.kind == KIND_REPROJECTION)
            .unwrap();
        assert_eq!(reprojected.label, "2.0 px");
        let ghost = markers.iter().find(|m| m.kind == KIND_GHOST).unwrap();
        assert_eq!(ghost.label, "ghost, 2 reflections");
        // A view the solution has no overlay for draws nothing extra.
        let (markers, _) = locate_drawing(1, SIZE, &MarkSet::new(2), Some(&solution));
        assert_eq!(markers.len(), 0);
    }

    #[test]
    fn the_calibration_drawing_shows_edges_the_waiting_end_the_dot_and_the_diagonal() {
        let mut marks = CalibMarks::new(1);
        marks.click(0, CalibMode::Edge, [10.0, 10.0]);
        marks.click(0, CalibMode::Edge, [200.0, 10.0]);
        marks.click(0, CalibMode::Edge, [300.0, 50.0]);
        marks.click(0, CalibMode::Mark, [400.0, 400.0]);
        marks.click(0, CalibMode::Diagonal(2), [5.0, 5.0]);
        marks.click(0, CalibMode::Diagonal(2), [50.0, 60.0]);
        let (markers, paths) = calibration_drawing(0, SIZE, &marks);
        assert!(markers.iter().any(|m| m.label == "end?"));
        assert!(markers.iter().any(|m| m.kind == KIND_CORNER));
        assert_eq!(markers.iter().filter(|m| m.label == "d3").count(), 2);
        assert_eq!(paths.iter().filter(|p| p.kind == PATH_EDGE).count(), 1);
        assert_eq!(paths.iter().filter(|p| p.kind == PATH_LINE).count(), 1);
        assert_eq!(
            calibration_drawing(5, SIZE, &marks),
            (Vec::new(), Vec::new())
        );
    }
}
