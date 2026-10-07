//! What the user marked on the photos of the calibration cube: the outer edges as line
//! segments, the corner with the orientation dot, and the edges of the coated diagonal.
//!
//! Pixels are those of the full-resolution photo. An edge segment takes two clicks (its two
//! ends, which need not be the cube's corners: any two points of the line the edge makes). The
//! diagonal's edges are marked by several clicks along them, one diagonal edge at a time, as
//! seen THROUGH the glass.

use indicatrix_cut_core::rough_plan::locate::{DiagonalLine, EdgeLine, ViewObservations};

/// How many edges the coated diagonal has.
pub const DIAGONAL_EDGES: u8 = 4;

/// What a click on a calibration photo does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibMode {
    /// Two clicks make one outer edge of the cube.
    Edge,
    /// A click on the orientation dot at the cube's corner.
    Mark,
    /// A click along edge `0..4` of the coated diagonal.
    Diagonal(u8),
}

impl CalibMode {
    /// The mode of pill `index`: 0 outer edge, 1 corner dot, 2 to 5 the diagonal's edges.
    #[must_use]
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Mark,
            2..=5 => Self::Diagonal(u8::try_from(index - 2).unwrap_or(0)),
            _ => Self::Edge,
        }
    }
}

/// The marks of one view.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CalibViewMarks {
    /// The finished outer edges.
    pub lines: Vec<EdgeLine>,
    /// The first end of an edge whose second end is still to be clicked.
    pub pending: Option<[f64; 2]>,
    /// The corner dot.
    pub mark: Option<[f64; 2]>,
    /// The clicks along each of the diagonal's four edges.
    pub diagonal: [Vec<[f64; 2]>; DIAGONAL_EDGES as usize],
}

/// The marks of every view.
#[derive(Debug, Clone, PartialEq)]
pub struct CalibMarks {
    /// One entry per view of the rig.
    pub views: Vec<CalibViewMarks>,
}

impl CalibMarks {
    /// An empty set for `views` views.
    #[must_use]
    pub fn new(views: usize) -> Self {
        Self {
            views: vec![CalibViewMarks::default(); views],
        }
    }

    /// Makes room for `views` views (keeps the marks of the views that remain).
    pub fn resize(&mut self, views: usize) {
        self.views.resize(views, CalibViewMarks::default());
    }

    /// Applies a click at `pixel` in `view`.
    pub fn click(&mut self, view: usize, mode: CalibMode, pixel: [f64; 2]) {
        let Some(marks) = self.views.get_mut(view) else {
            return;
        };
        match mode {
            CalibMode::Edge => {
                if let Some(first) = marks.pending.take() {
                    marks.lines.push(EdgeLine { a: first, b: pixel });
                    return;
                }
                marks.pending = Some(pixel);
            }
            CalibMode::Mark => marks.mark = Some(pixel),
            CalibMode::Diagonal(edge) => {
                if let Some(clicks) = marks.diagonal.get_mut(usize::from(edge)) {
                    clicks.push(pixel);
                }
            }
        }
    }

    /// Takes back the last click of `mode` in `view`.
    pub fn undo(&mut self, view: usize, mode: CalibMode) {
        let Some(marks) = self.views.get_mut(view) else {
            return;
        };
        match mode {
            CalibMode::Edge => {
                if marks.pending.take().is_none() {
                    marks.lines.pop();
                }
            }
            CalibMode::Mark => marks.mark = None,
            CalibMode::Diagonal(edge) => {
                if let Some(clicks) = marks.diagonal.get_mut(usize::from(edge)) {
                    clicks.pop();
                }
            }
        }
    }

    /// Removes everything marked in `view`.
    pub fn clear(&mut self, view: usize) {
        if let Some(marks) = self.views.get_mut(view) {
            *marks = CalibViewMarks::default();
        }
    }

    /// The pass-1 observations: every view with edge lines or a corner dot.
    #[must_use]
    pub fn observations(&self) -> Vec<ViewObservations> {
        self.views
            .iter()
            .enumerate()
            .filter(|(_, marks)| !marks.lines.is_empty() || marks.mark.is_some())
            .map(|(view, marks)| ViewObservations {
                view,
                lines: marks.lines.clone(),
                mark: marks.mark,
            })
            .collect()
    }

    /// The pass-2 lines: every diagonal edge with at least two clicks in a view.
    #[must_use]
    pub fn diagonal_lines(&self) -> Vec<DiagonalLine> {
        let mut lines = Vec::new();
        for (view, marks) in self.views.iter().enumerate() {
            for (edge, clicks) in marks.diagonal.iter().enumerate() {
                if clicks.len() >= 2 {
                    lines.push(DiagonalLine {
                        edge: u8::try_from(edge).unwrap_or(0),
                        view,
                        pixels: clicks.clone(),
                    });
                }
            }
        }
        lines
    }

    /// A line for the view's slot: what is marked in it.
    #[must_use]
    pub fn summary(&self, view: usize) -> String {
        let Some(marks) = self.views.get(view) else {
            return String::new();
        };
        let diagonal: usize = marks.diagonal.iter().map(Vec::len).sum();
        let mut parts = vec![format!("edges {}", marks.lines.len())];
        if marks.pending.is_some() {
            parts.push("one end waiting".to_owned());
        }
        if marks.mark.is_some() {
            parts.push("dot".to_owned());
        }
        if diagonal > 0 {
            parts.push(format!("diagonal clicks {diagonal}"));
        }
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pills_map_to_the_modes() {
        assert_eq!(CalibMode::from_index(0), CalibMode::Edge);
        assert_eq!(CalibMode::from_index(1), CalibMode::Mark);
        assert_eq!(CalibMode::from_index(2), CalibMode::Diagonal(0));
        assert_eq!(CalibMode::from_index(5), CalibMode::Diagonal(3));
        assert_eq!(CalibMode::from_index(99), CalibMode::Edge);
    }

    #[test]
    fn two_clicks_make_an_edge_and_an_undo_takes_back_the_waiting_end_first() {
        let mut marks = CalibMarks::new(2);
        marks.click(0, CalibMode::Edge, [1.0, 2.0]);
        assert!(
            marks.observations().is_empty(),
            "one end is not an edge yet"
        );
        marks.click(0, CalibMode::Edge, [3.0, 4.0]);
        marks.click(0, CalibMode::Edge, [5.0, 6.0]);
        let observed = marks.observations();
        assert_eq!(observed.len(), 1);
        assert_eq!(
            observed[0].lines,
            vec![EdgeLine {
                a: [1.0, 2.0],
                b: [3.0, 4.0]
            }]
        );
        marks.undo(0, CalibMode::Edge);
        assert_eq!(marks.views[0].lines.len(), 1, "the waiting end went first");
        marks.undo(0, CalibMode::Edge);
        assert_eq!(marks.views[0].lines.len(), 0);
    }

    #[test]
    fn the_corner_dot_replaces_itself_and_counts_as_an_observation() {
        let mut marks = CalibMarks::new(2);
        marks.click(1, CalibMode::Mark, [10.0, 10.0]);
        marks.click(1, CalibMode::Mark, [11.0, 12.0]);
        let observed = marks.observations();
        assert_eq!(observed[0].view, 1);
        assert_eq!(observed[0].mark, Some([11.0, 12.0]));
        marks.undo(1, CalibMode::Mark);
        assert_eq!(marks.observations().len(), 0);
    }

    #[test]
    fn diagonal_clicks_make_a_line_from_two_clicks_on() {
        let mut marks = CalibMarks::new(3);
        marks.click(2, CalibMode::Diagonal(1), [1.0, 1.0]);
        assert_eq!(marks.diagonal_lines().len(), 0);
        marks.click(2, CalibMode::Diagonal(1), [2.0, 2.0]);
        marks.click(2, CalibMode::Diagonal(3), [9.0, 9.0]);
        marks.click(2, CalibMode::Diagonal(7), [9.0, 9.0]);
        let lines = marks.diagonal_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!((lines[0].edge, lines[0].view), (1, 2));
        assert_eq!(lines[0].pixels.len(), 2);
    }

    #[test]
    fn clearing_a_view_leaves_the_others_and_the_summary_says_what_is_marked() {
        let mut marks = CalibMarks::new(2);
        marks.click(0, CalibMode::Edge, [0.0, 0.0]);
        marks.click(0, CalibMode::Edge, [1.0, 1.0]);
        marks.click(0, CalibMode::Mark, [2.0, 2.0]);
        marks.click(0, CalibMode::Diagonal(0), [3.0, 3.0]);
        marks.click(1, CalibMode::Mark, [4.0, 4.0]);
        assert_eq!(marks.summary(0), "edges 1, dot, diagonal clicks 1");
        marks.clear(0);
        assert_eq!(marks.summary(0), "edges 0");
        assert!(marks.views[1].mark.is_some());
        assert_eq!(marks.summary(9), "");
    }
}
