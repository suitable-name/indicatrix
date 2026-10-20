//! Filmed clicks on a facet in the stone's picture: the pointer sweeps over the picture until the
//! hover panel names the facet that is wanted, and that spot is clicked, the way a learner points
//! at the stone and reads what is under the pointer before clicking.

use crate::{director::Film, support::app::pump, targets::Spot};
use indicatrix_cut::{EditorModel, ManipulateModel, SolidPreviewModel};
use indicatrix_editor::guide::CameraFocus;
use slint::ComponentHandle;

/// The tier the drag handle and picking lessons start from.
const CROWN_MAIN: &str = "Crown Main";

impl Film<'_> {
    /// Points at the stone until a facet of Crown Main is under the pointer, clicks it and
    /// requires the three handles to appear.
    pub(crate) fn pick_crown_facet(&mut self) -> Result<(), String> {
        let spot = self.sweep_for_facet(&format!("{CROWN_MAIN} facet"), |hover| {
            hover.contains(CROWN_MAIN)
        })?;
        self.click_and_invoke(&spot)?;
        if !self.ui.global::<ManipulateModel>().get_handles_visible() {
            return Err(format!("picked {CROWN_MAIN} has no handles"));
        }
        Ok(())
    }

    /// Points at the stone until a facet of a tier other than `current` is under the pointer,
    /// clicks it and requires the selection to move to another tier.
    ///
    /// With no facet under the pointer the hover panel keeps reading the facet that was clicked
    /// last, so a text that names `current` is never taken for another tier.
    pub(crate) fn pick_other_tier_facet(&mut self, current: &str) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        let before = editor.get_selected_tier_index();
        let spot = self
            .sweep_for_facet(&format!("facet of a tier other than {current}"), |hover| {
                !hover.is_empty() && !hover.contains(current)
            })?;
        self.click_and_invoke(&spot)?;
        pump(80);
        let after = editor.get_selected_tier_index();
        if after < 0 || after == before {
            return Err(format!(
                "clicking a facet of a tier other than {current} left the selection at row {after} \
                 (it was {before})"
            ));
        }
        Ok(())
    }

    /// Moves the pointer over the stone's picture on a grid, from the camera poses that show the
    /// upper side, until the hover text satisfies `wanted`; the spot to click then. `what` names
    /// the facet for the error.
    fn sweep_for_facet(
        &mut self,
        what: &str,
        wanted: impl Fn(&str) -> bool,
    ) -> Result<Spot, String> {
        let preview = self.ui.global::<SolidPreviewModel>();
        let diagram = preview.get_view_mode() == 3;
        let focuses: &[CameraFocus] = if diagram {
            &[CameraFocus::Keep]
        } else {
            &[CameraFocus::Crown, CameraFocus::Whole, CameraFocus::Top]
        };
        for &focus in focuses {
            if !diagram {
                self.frame(focus, true, true)?;
            }
            let viewport = self.viewport()?;
            // A coarse grid first (49 points), the fine one (361 points) only when it misses.
            for divisions in [8_u16, 20] {
                for row in 1..divisions {
                    for col in 1..divisions {
                        let x = viewport.x + viewport.w * f32::from(col) / f32::from(divisions);
                        let y = viewport.y + viewport.h * f32::from(row) / f32::from(divisions);
                        self.pointer(x, y);
                        let hover = if diagram {
                            preview.get_diagram_hover_text()
                        } else {
                            preview.get_hover_text()
                        };
                        if wanted(hover.as_str()) {
                            return Ok(Spot {
                                x: x - 2.0,
                                y: y - 2.0,
                                w: 4.0,
                                h: 4.0,
                                handle: None,
                            });
                        }
                    }
                }
            }
        }
        Err(format!(
            "no {what} was found after visible viewport hover sweep; final hover: {}",
            if diagram {
                preview.get_diagram_hover_text()
            } else {
                preview.get_hover_text()
            }
        ))
    }
}
