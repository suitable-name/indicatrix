//! A located inclusion as the session remembers it: its marks, the rig it was located with and
//! the alignment, so that it can be solved again, for example after the rig was calibrated
//! anew.
//!
//! The saved-plan format stores an inclusion's mesh only (G6's version 3), so a record is kept
//! for the session, in memory, and is plain serde data for the day a plan can carry it.

use indicatrix_cut_core::rough_plan::locate::Rigid;
use serde::{Deserialize, Serialize};

use super::marks::MarkSet;

/// One inclusion that was located from photos and added to the rough.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocatedRecord {
    /// The rig it was located with, by name.
    pub rig_name: String,
    /// The marks, in the pixels of the photos.
    pub marks: MarkSet,
    /// The mesh-to-rig alignment used.
    pub alignment: Rigid,
    /// The located position in the rough's frame, in mm.
    pub position_mm: [f64; 3],
    /// The radius of the shell that was added, in mm.
    pub radius_mm: f64,
    /// The margin that was added, in mm.
    pub margin_mm: f64,
    /// The uncertainty (RMS) when it was located, in mm.
    pub rms_mm: f64,
}

impl LocatedRecord {
    /// The line the session list shows. `number` is the 1-based position in the list.
    #[must_use]
    pub fn row_text(&self, number: usize) -> String {
        let [x, y, z] = self.position_mm;
        format!(
            "{number}. ({x:.2}, {y:.2}, {z:.2}) mm, radius {:.2} mm, margin {:.2} mm, {} views, rig {}",
            self.radius_mm,
            self.margin_mm,
            self.marks.marked_views(),
            self.rig_name
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locate_io::marks::{ClickMode, MarkKind};

    fn record() -> LocatedRecord {
        let mut marks = MarkSet::new(3);
        for view in [0, 2] {
            marks.click(view, ClickMode::Mark(MarkKind::Point), [10.0, 20.0]);
        }
        LocatedRecord {
            rig_name: "Bench".to_owned(),
            marks,
            alignment: Rigid::IDENTITY,
            position_mm: [1.0, 2.5, 3.0],
            radius_mm: 0.5,
            margin_mm: 0.3,
            rms_mm: 0.05,
        }
    }

    #[test]
    fn the_row_names_the_place_the_size_the_views_and_the_rig() {
        assert_eq!(
            record().row_text(2),
            "2. (1.00, 2.50, 3.00) mm, radius 0.50 mm, margin 0.30 mm, 2 views, rig Bench"
        );
    }

    #[test]
    fn a_record_round_trips_through_json_so_it_can_be_solved_again() {
        let text = serde_json::to_string(&record()).expect("serialize");
        let back: LocatedRecord = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(back, record());
    }
}
