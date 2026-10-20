//! The filmed route of the Multi-Select lesson's batch Offset step: the cursor moves to the deg
//! box in the bar above the tier table, types the number and clicks Offset. The bar changes every
//! ticked tier in one step, so the tiers are never edited one by one, and no row is clicked first.

use crate::{director::Film, support::app::pump};
use indicatrix_cut::EditorModel;
use slint::{ComponentHandle, Model};
use std::time::Duration;

/// The accessible label of the deg box of the bar (`TierTableToolbar`, `reader_label`).
const OFFSET_BOX: &str = "Offset in Degrees";
/// The accessible label of the Offset button of the bar (`CompactButton.text`).
const OFFSET_BUTTON: &str = "Offset";
/// The accessible label of the Delete button of the bar (`CompactButton.text`).
const DELETE_BUTTON: &str = "Delete";

/// How often the table is looked at again after the click, a tenth of a second apart.
const SETTLE_TRIES: usize = 12;

/// One ticked row: its index and the angle it showed before the offset.
struct Ticked {
    index: i32,
    angle: String,
}

impl Film<'_> {
    /// Films the step "Offset them together": types `degrees` in the deg box, clicks Offset and
    /// requires every ticked tier to show another angle.
    ///
    /// The bar acts on the ticked group by itself (the cursor need not be on one of its rows), so
    /// the route is only the two on-screen actions of the lesson.
    pub(super) fn offset_ticked_tiers(&mut self, degrees: &str) -> Result<(), String> {
        let ticked = self.ticked_rows();
        if ticked.len() < 2 {
            return Err(format!(
                "Offset needs a group of ticked tiers, the table shows {}",
                ticked.len()
            ));
        }
        let field = self
            .input_spot_in(OFFSET_BOX, false)
            .ok_or_else(|| format!("the {OFFSET_BOX:?} box of the bar is not visible"))?;
        self.click_and_type(&field, degrees)?;
        let button = self.scoped_button("EditorTierTable", OFFSET_BUTTON)?;
        self.click_and_invoke(&button)?;
        pump(80);
        for _ in 0..SETTLE_TRIES {
            if self.all_angles_changed(&ticked) {
                return self.rec.hold(Duration::from_millis(150));
            }
            self.rec.hold(Duration::from_millis(100))?;
        }
        Err(format!(
            "clicking {OFFSET_BUTTON:?} with {degrees:?} typed did not change the angle of every ticked tier"
        ))
    }

    /// Films the step "Delete them together": clicks Delete in the bar above the tier table and
    /// requires every ticked tier to be gone.
    ///
    /// Like Offset, the bar acts on the ticked group by itself, so no row is clicked first and
    /// the tiers are never removed one by one.
    pub(super) fn delete_ticked_tiers(&mut self) -> Result<(), String> {
        let ticked = self.ticked_rows().len();
        if ticked < 2 {
            return Err(format!(
                "Delete needs a group of ticked tiers, the table shows {ticked}"
            ));
        }
        let editor = self.ui.global::<EditorModel>();
        let before = editor.get_tiers().row_count();
        let button = self.scoped_button("EditorTierTable", DELETE_BUTTON)?;
        self.click_and_invoke(&button)?;
        pump(80);
        for _ in 0..SETTLE_TRIES {
            if editor.get_tiers().row_count() + ticked == before {
                return self.rec.hold(Duration::from_millis(150));
            }
            self.rec.hold(Duration::from_millis(100))?;
        }
        Err(format!(
            "clicking {DELETE_BUTTON:?} did not remove the {ticked} ticked tiers"
        ))
    }

    /// The rows of the tier table that are ticked now.
    fn ticked_rows(&self) -> Vec<Ticked> {
        self.ui
            .global::<EditorModel>()
            .get_tiers()
            .iter()
            .filter(|row| row.multi_selected)
            .map(|row| Ticked {
                index: row.index,
                angle: row.angle_deg.to_string(),
            })
            .collect()
    }

    /// Whether every row of `before` shows another angle than it did.
    fn all_angles_changed(&self, before: &[Ticked]) -> bool {
        let rows = self.ui.global::<EditorModel>().get_tiers();
        before.iter().all(|row| {
            usize::try_from(row.index)
                .ok()
                .and_then(|at| rows.row_data(at))
                .is_some_and(|now| now.angle_deg.as_str() != row.angle)
        })
    }
}
