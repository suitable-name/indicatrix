//! What the user marked on the photos of an inclusion: a point, a line or a polygon in each
//! view, and the stone's outline in each view.
//!
//! All coordinates are image pixels of the full-resolution photo (`u` right, `v` down), the
//! same numbers the core takes. The set is plain data: it is what the located inclusion is
//! stored with, so a mark set can be restored and solved again after the rig was calibrated
//! anew.

use indicatrix_cut_core::rough_plan::locate::{Mark, OutlineView, ViewPolyline};
use serde::{Deserialize, Serialize};

/// The fewest vertices of a marked line.
const MIN_LINE_VERTICES: usize = 2;

/// The fewest vertices of a marked polygon, and of an outline.
const MIN_POLYGON_VERTICES: usize = 3;

/// What kind of inclusion is marked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkKind {
    /// A compact inclusion: one click per view.
    Point,
    /// A feather or a silk bundle seen edge-on: a polyline, the same vertices in every view.
    Line,
    /// A planar inclusion: a closed polygon, the same vertices in every view.
    Polygon,
}

impl MarkKind {
    /// Whether the last vertex joins the first.
    #[must_use]
    pub const fn closed(self) -> bool {
        matches!(self, Self::Polygon)
    }

    /// The fewest vertices a view needs.
    #[must_use]
    pub const fn min_vertices(self) -> usize {
        match self {
            Self::Point => 1,
            Self::Line => MIN_LINE_VERTICES,
            Self::Polygon => MIN_POLYGON_VERTICES,
        }
    }

    /// The word for the kind in messages.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Point => "point",
            Self::Line => "line",
            Self::Polygon => "polygon",
        }
    }
}

/// What a click on the photo does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickMode {
    /// Marks the inclusion, as the given kind.
    Mark(MarkKind),
    /// Adds a vertex to the stone's outline.
    Outline,
}

impl ClickMode {
    /// The mode of pill `index`: 0 point, 1 line, 2 polygon, 3 outline.
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Mark(MarkKind::Line),
            2 => Self::Mark(MarkKind::Polygon),
            3 => Self::Outline,
            _ => Self::Mark(MarkKind::Point),
        }
    }
}

/// The marks of one view.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewMarks {
    /// The clicked point of a point inclusion.
    pub point: Option<[f64; 2]>,
    /// The vertices of a line or polygon, in order.
    pub path: Vec<[f64; 2]>,
    /// The vertices of the stone's outline (a closed polygon), in order.
    pub outline: Vec<[f64; 2]>,
}

/// The marks of every view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarkSet {
    /// What is being marked.
    pub kind: MarkKind,
    /// One entry per view of the rig.
    pub views: Vec<ViewMarks>,
}

impl MarkSet {
    /// An empty set for a rig of `views` views.
    #[must_use]
    pub fn new(views: usize) -> Self {
        Self {
            kind: MarkKind::Point,
            views: vec![ViewMarks::default(); views],
        }
    }

    /// Makes room for `views` views (keeps the marks of the views that remain).
    pub fn resize(&mut self, views: usize) {
        self.views.resize(views, ViewMarks::default());
    }

    /// Applies a click at `pixel` in `view`. A mark click also selects its kind.
    pub fn click(&mut self, view: usize, mode: ClickMode, pixel: [f64; 2]) {
        let Some(marks) = self.views.get_mut(view) else {
            return;
        };
        match mode {
            ClickMode::Mark(kind) => {
                self.kind = kind;
                if kind == MarkKind::Point {
                    marks.point = Some(pixel);
                } else {
                    marks.path.push(pixel);
                }
            }
            ClickMode::Outline => marks.outline.push(pixel),
        }
    }

    /// Takes back the last click of `mode` in `view`.
    pub fn undo(&mut self, view: usize, mode: ClickMode) {
        let Some(marks) = self.views.get_mut(view) else {
            return;
        };
        match mode {
            ClickMode::Mark(MarkKind::Point) => marks.point = None,
            ClickMode::Mark(_) => {
                marks.path.pop();
            }
            ClickMode::Outline => {
                marks.outline.pop();
            }
        }
    }

    /// Removes everything `mode` marked in `view`.
    pub fn clear(&mut self, view: usize, mode: ClickMode) {
        let Some(marks) = self.views.get_mut(view) else {
            return;
        };
        match mode {
            ClickMode::Mark(MarkKind::Point) => marks.point = None,
            ClickMode::Mark(_) => marks.path.clear(),
            ClickMode::Outline => marks.outline.clear(),
        }
    }

    /// The point marks, one per marked view (empty unless the kind is a point).
    #[must_use]
    pub fn point_marks(&self) -> Vec<Mark> {
        if self.kind != MarkKind::Point {
            return Vec::new();
        }
        self.views
            .iter()
            .enumerate()
            .filter_map(|(view, marks)| marks.point.map(|pixel| Mark { view, pixel }))
            .collect()
    }

    /// The lines or polygons, one per view that has enough vertices (empty for a point).
    #[must_use]
    pub fn polylines(&self) -> Vec<ViewPolyline> {
        if self.kind == MarkKind::Point {
            return Vec::new();
        }
        let least = self.kind.min_vertices();
        self.views
            .iter()
            .enumerate()
            .filter(|(_, marks)| marks.path.len() >= least)
            .map(|(view, marks)| ViewPolyline {
                view,
                pixels: marks.path.clone(),
            })
            .collect()
    }

    /// The outlines, one per view that has enough vertices.
    #[must_use]
    pub fn outlines(&self) -> Vec<OutlineView> {
        self.views
            .iter()
            .enumerate()
            .filter(|(_, marks)| marks.outline.len() >= MIN_POLYGON_VERTICES)
            .map(|(view, marks)| OutlineView {
                view,
                outline: marks.outline.clone(),
            })
            .collect()
    }

    /// How many views carry a mark of the current kind.
    #[must_use]
    pub fn marked_views(&self) -> usize {
        match self.kind {
            MarkKind::Point => self.views.iter().filter(|v| v.point.is_some()).count(),
            _ => self.polylines().len(),
        }
    }

    /// What is marked in `view`, for its slot in the window's list ("outline 6, mark").
    #[must_use]
    pub fn summary(&self, view: usize) -> String {
        let Some(marks) = self.views.get(view) else {
            return String::new();
        };
        let mut parts = Vec::new();
        if !marks.outline.is_empty() {
            parts.push(format!("outline {}", marks.outline.len()));
        }
        if marks.point.is_some() {
            parts.push("mark".to_owned());
        }
        if !marks.path.is_empty() {
            parts.push(format!("vertices {}", marks.path.len()));
        }
        parts.join(", ")
    }

    /// A message when the marked lines or polygons do not have the same number of vertices in
    /// every view (the core pairs vertex `k` of every view). `names` are the views' names.
    #[must_use]
    pub fn vertex_count_problem(&self, names: &[String]) -> Option<String> {
        if self.kind == MarkKind::Point {
            return None;
        }
        let lines = self.polylines();
        let first = lines.first()?;
        let count = first.pixels.len();
        let odd = lines.iter().find(|line| line.pixels.len() != count)?;
        let name = |view: usize| names.get(view).map_or("a view", String::as_str);
        Some(format!(
            "Every view needs the same {} vertices in the same order: {} has {}, {} has {}.",
            self.kind.word(),
            name(first.view),
            count,
            name(odd.view),
            odd.pixels.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set() -> MarkSet {
        MarkSet::new(4)
    }

    #[test]
    fn the_pills_map_to_the_modes() {
        assert_eq!(ClickMode::from_index(0), ClickMode::Mark(MarkKind::Point));
        assert_eq!(ClickMode::from_index(1), ClickMode::Mark(MarkKind::Line));
        assert_eq!(ClickMode::from_index(2), ClickMode::Mark(MarkKind::Polygon));
        assert_eq!(ClickMode::from_index(3), ClickMode::Outline);
        assert_eq!(ClickMode::from_index(-1), ClickMode::Mark(MarkKind::Point));
    }

    #[test]
    fn a_point_click_replaces_the_point_of_that_view() {
        let mut marks = set();
        let point = ClickMode::Mark(MarkKind::Point);
        marks.click(1, point, [10.0, 20.0]);
        marks.click(1, point, [11.0, 21.0]);
        marks.click(3, point, [5.0, 6.0]);
        assert_eq!(
            marks.point_marks(),
            vec![
                Mark {
                    view: 1,
                    pixel: [11.0, 21.0]
                },
                Mark {
                    view: 3,
                    pixel: [5.0, 6.0]
                }
            ]
        );
        assert_eq!(marks.marked_views(), 2);
        marks.undo(1, point);
        assert_eq!(marks.marked_views(), 1);
        marks.clear(3, point);
        assert_eq!(marks.marked_views(), 0);
    }

    #[test]
    fn a_line_click_adds_a_vertex_and_a_view_needs_two_to_count() {
        let mut marks = set();
        let line = ClickMode::Mark(MarkKind::Line);
        marks.click(0, line, [1.0, 1.0]);
        assert!(marks.polylines().is_empty(), "one vertex is not a line");
        marks.click(0, line, [2.0, 2.0]);
        marks.click(2, line, [3.0, 3.0]);
        marks.click(2, line, [4.0, 4.0]);
        assert_eq!(marks.polylines().len(), 2);
        assert!(marks.point_marks().is_empty(), "a line is not a point mark");
        marks.undo(2, line);
        assert_eq!(marks.polylines().len(), 1);
        marks.clear(0, line);
        assert_eq!(marks.polylines().len(), 0);
    }

    #[test]
    fn a_polygon_needs_three_vertices() {
        let mut marks = set();
        let polygon = ClickMode::Mark(MarkKind::Polygon);
        for pixel in [[0.0, 0.0], [5.0, 0.0]] {
            marks.click(0, polygon, pixel);
        }
        assert_eq!(marks.polylines().len(), 0);
        marks.click(0, polygon, [5.0, 5.0]);
        assert_eq!(marks.polylines().len(), 1);
        assert!(marks.kind.closed());
    }

    #[test]
    fn the_outline_is_kept_apart_from_the_marks() {
        let mut marks = set();
        for pixel in [[0.0, 0.0], [9.0, 0.0], [9.0, 9.0]] {
            marks.click(2, ClickMode::Outline, pixel);
        }
        assert_eq!(marks.outlines().len(), 1);
        assert_eq!(marks.outlines()[0].view, 2);
        assert_eq!(marks.marked_views(), 0);
        marks.undo(2, ClickMode::Outline);
        assert_eq!(marks.outlines().len(), 0);
        marks.clear(2, ClickMode::Outline);
        assert_eq!(marks.views[2].outline.len(), 0);
    }

    #[test]
    fn a_click_outside_the_views_is_ignored() {
        let mut marks = set();
        marks.click(9, ClickMode::Outline, [1.0, 1.0]);
        marks.undo(9, ClickMode::Outline);
        marks.clear(9, ClickMode::Outline);
        assert_eq!(marks, set());
    }

    #[test]
    fn unequal_vertex_counts_are_reported_by_view_name() {
        let names: Vec<String> = ["+X upper", "-X upper", "+Y upper", "-Y upper"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let mut marks = set();
        let line = ClickMode::Mark(MarkKind::Line);
        for pixel in [[0.0, 0.0], [1.0, 1.0]] {
            marks.click(0, line, pixel);
        }
        for pixel in [[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]] {
            marks.click(1, line, pixel);
        }
        let message = marks.vertex_count_problem(&names).expect("a problem");
        assert!(message.contains("+X upper has 2"), "{message}");
        assert!(message.contains("-X upper has 3"), "{message}");
        marks.undo(1, line);
        assert_eq!(marks.vertex_count_problem(&names), None);
    }

    #[test]
    fn the_summary_says_what_a_view_holds() {
        let mut marks = set();
        assert_eq!(marks.summary(0), "");
        marks.click(0, ClickMode::Outline, [0.0, 0.0]);
        marks.click(0, ClickMode::Mark(MarkKind::Point), [1.0, 1.0]);
        assert_eq!(marks.summary(0), "outline 1, mark");
        marks.click(1, ClickMode::Mark(MarkKind::Line), [1.0, 1.0]);
        assert_eq!(marks.summary(1), "vertices 1");
        assert_eq!(marks.summary(9), "");
    }

    #[test]
    fn resizing_keeps_the_marks_of_the_remaining_views() {
        let mut marks = set();
        marks.click(0, ClickMode::Mark(MarkKind::Point), [1.0, 2.0]);
        marks.resize(8);
        assert_eq!(marks.views.len(), 8);
        marks.resize(1);
        assert_eq!(marks.point_marks().len(), 1);
    }

    #[test]
    fn a_mark_set_round_trips_through_json() {
        let mut marks = set();
        marks.click(0, ClickMode::Mark(MarkKind::Polygon), [1.5, 2.5]);
        marks.click(0, ClickMode::Outline, [3.0, 4.0]);
        let text = serde_json::to_string(&marks).expect("serialize");
        let back: MarkSet = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(back, marks);
    }
}
