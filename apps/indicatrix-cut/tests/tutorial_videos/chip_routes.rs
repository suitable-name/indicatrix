//! The filmed routes of the Facet Chips lesson: the cursor moves to the control under Facets in
//! the Tier tab (the position box and + Add, the x and detach icons on a chip, the teeth box and
//! Rotate, Mirror) and clicks it or types into it. A Tier form edit that lands on the same
//! indices is a different gesture, so nothing is set through a model callback or a form field,
//! and every route requires the visible result (the chips) before it returns.

use crate::{director::Film, support::app::pump, targets::Spot};
use indicatrix_cut::EditorModel;
use slint::{ComponentHandle, Model};
use std::time::Duration;

/// The accessible label of the strip of chips (`TierFacetsSection`).
const CHIP_STRIP: &str = "Facets of This Tier";

/// The accessible labels of the boxes and buttons of `TierFacetsSection`. A `LineEdit` is labelled
/// by its `accessible-label`; a `TipButton` by its `reader_label` (its `hint` is only the
/// description), or its `text` when it has no reader label.
const ADD_BOX: &str = "Position of the Facet to Add";
const ADD_BUTTON: &str = "Add Facet";
const ROTATE_BOX: &str = "Number of Teeth to Rotate By";
const ROTATE_UP_BUTTON: &str = "Rotate Up";
const MIRROR_BUTTON: &str = "Mirror";

/// How often the inspector is scrolled up the page, or the strip along its chips, before the
/// icon is given up on.
const REVEAL_TRIES: usize = 14;

/// Whether the centre of `inner` lies inside `outer`.
fn centre_inside(outer: &Spot, inner: &Spot) -> bool {
    let (x, y) = inner.center();
    x >= outer.x && x <= outer.x + outer.w && y >= outer.y && y <= outer.y + outer.h
}

/// Whether `inner` lies wholly inside `outer`, give or take a pixel of rounding.
const fn wholly_inside(outer: &Spot, inner: &Spot) -> bool {
    inner.x >= outer.x - 1.0
        && inner.y >= outer.y - 1.0
        && inner.x + inner.w <= outer.x + outer.w + 1.0
        && inner.y + inner.h <= outer.y + outer.h + 1.0
}

/// Where the detach icon of one chip, the strip of chips and the inspector's scrolling area are
/// on screen right now.
struct ChipPlace {
    icon: Spot,
    strip: Spot,
    viewport: Spot,
}

impl Film<'_> {
    /// Films clicking the detach icon on the chip of the facet at index position `position` of the
    /// tier called `tier`, then requires the chip to read as detached.
    pub(super) fn detach_chip(&mut self, tier: &str, position: f32) -> Result<(), String> {
        self.open_tier_chips(tier, false)?;
        let (label, detached) = self
            .chip_at(position)
            .ok_or_else(|| format!("{tier}: the Tier tab has no chip at position {position}"))?;
        if detached {
            return Err(format!("{tier}: the chip at {label} is already detached"));
        }
        let icon = self.reveal_chip_icon(&format!("Detach the facet at position {label} from"))?;
        self.click_and_invoke(&icon)?;
        self.settle()?;
        if !self.chip_at(position).is_some_and(|(_, now)| now) {
            return Err(format!(
                "{tier}: clicking the detach icon did not detach the chip at {label}"
            ));
        }
        Ok(())
    }

    /// Films the lesson's first step: clicks the row of `tier`, types `text` in the position box
    /// under Facets and clicks + Add, then requires a chip at index position `position`.
    pub(super) fn add_chip_facet(
        &mut self,
        tier: &str,
        position: f32,
        text: &str,
    ) -> Result<(), String> {
        self.open_tier_chips(tier, true)?;
        let before = self.chip_positions();
        if before.iter().any(|at| (at - position).abs() < 1e-3) {
            return Err(format!("{tier}: there is already a chip at {position}"));
        }
        let field = self
            .input_spot(ADD_BOX)?
            .ok_or_else(|| format!("{tier}: the {ADD_BOX:?} box is not visible"))?;
        self.click_and_type(&field, text)?;
        let button = self.reveal_inspector_button(ADD_BUTTON)?;
        self.click_and_invoke(&button)?;
        self.settle()?;
        let after = self.chip_positions();
        if after.len() <= before.len() || !after.iter().any(|at| (at - position).abs() < 1e-3) {
            return Err(format!(
                "{tier}: clicking {ADD_BUTTON:?} did not put a chip at {position} ({} chips before, {} after)",
                before.len(),
                after.len()
            ));
        }
        Ok(())
    }

    /// Films clicking the x on the chip at index position `position` of `tier`, then requires
    /// the chip to be gone.
    pub(super) fn remove_chip(&mut self, tier: &str, position: f32) -> Result<(), String> {
        self.open_tier_chips(tier, false)?;
        let (label, _) = self
            .chip_at(position)
            .ok_or_else(|| format!("{tier}: the Tier tab has no chip at position {position}"))?;
        let icon = self.reveal_chip_icon(&format!("Remove the facet at position {label} from"))?;
        self.click_and_invoke(&icon)?;
        self.settle()?;
        if self.chip_at(position).is_some() {
            return Err(format!(
                "{tier}: clicking the x did not remove the chip at {label}"
            ));
        }
        Ok(())
    }

    /// Films typing `teeth` in the teeth box under Facets and clicking the first Rotate button
    /// (up to higher index numbers), then requires the chips to have moved.
    pub(super) fn rotate_chips_up(&mut self, tier: &str, teeth: &str) -> Result<(), String> {
        self.open_tier_chips(tier, false)?;
        let before = self.chip_positions();
        let field = self
            .input_spot(ROTATE_BOX)?
            .ok_or_else(|| format!("{tier}: the {ROTATE_BOX:?} box is not visible"))?;
        self.click_and_type(&field, teeth)?;
        let button = self.reveal_inspector_button(ROTATE_UP_BUTTON)?;
        self.click_and_invoke(&button)?;
        self.settle()?;
        self.require_moved_chips(tier, &before, ROTATE_UP_BUTTON)
    }

    /// Films clicking Mirror under Facets, then requires the chips to have moved.
    pub(super) fn mirror_chips(&mut self, tier: &str) -> Result<(), String> {
        self.open_tier_chips(tier, false)?;
        let before = self.chip_positions();
        let button = self.reveal_inspector_button(MIRROR_BUTTON)?;
        self.click_and_invoke(&button)?;
        self.settle()?;
        self.require_moved_chips(tier, &before, MIRROR_BUTTON)
    }

    /// Fails unless the chips of the selected tier now sit at other positions than `before`.
    fn require_moved_chips(&self, tier: &str, before: &[f32], clicked: &str) -> Result<(), String> {
        if self.chip_positions() == before {
            return Err(format!(
                "{tier}: clicking {clicked:?} left the chips where they were"
            ));
        }
        Ok(())
    }

    /// Makes the Tier tab the open inspector tab and `tier` the selected row. The row is clicked
    /// when `click_row` is set (the lesson's own "click the row" action) or when it is not the
    /// selected one yet.
    fn open_tier_chips(&mut self, tier: &str, click_row: bool) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        if editor.get_inspector_tab() != 0 {
            self.film_inspector_tab(0)?;
        }
        let selected = usize::try_from(editor.get_selected_tier_index())
            .ok()
            .and_then(|at| editor.get_tiers().row_data(at));
        if click_row || selected.is_none_or(|row| !row.name.trim().eq_ignore_ascii_case(tier)) {
            self.film_select_tier(tier)?;
        }
        Ok(())
    }

    /// Lets the click take effect and holds a frame so the result is on film.
    fn settle(&mut self) -> Result<(), String> {
        pump(80);
        self.rec.hold(Duration::from_millis(150))
    }

    /// The index positions of the chips of the selected tier, in chip order.
    fn chip_positions(&self) -> Vec<f32> {
        self.ui
            .global::<EditorModel>()
            .get_selected_tier_chips()
            .iter()
            .map(|chip| chip.position)
            .collect()
    }

    /// The scrolling area of the inspector.
    fn inspector_viewport(&self) -> Option<Spot> {
        let view = self.view();
        let inspector = view
            .root
            .query_descendants()
            .match_inherits("EditorInspector")
            .find_first()?;
        let scroll = inspector
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()?;
        Some(Spot::of(&scroll))
    }

    /// The inspector button labelled `label`, when it lies inside the scrolling area.
    fn inspector_button_in_view(&self, label: &str) -> Option<Spot> {
        let viewport = self.inspector_viewport()?;
        self.scoped_button("EditorInspector", label)
            .ok()
            .filter(|spot| centre_inside(&viewport, spot))
    }

    /// Scrolls the inspector with wheel turns until the button labelled `label` can be clicked.
    /// It is looked for where the inspector is now first (the box beside it was just typed in).
    fn reveal_inspector_button(&mut self, label: &str) -> Result<Spot, String> {
        if let Some(spot) = self.inspector_button_in_view(label) {
            return Ok(spot);
        }
        self.scroll_inspector(3000.0)?;
        for _ in 0..REVEAL_TRIES {
            if let Some(spot) = self.inspector_button_in_view(label) {
                return Ok(spot);
            }
            self.scroll_inspector(-250.0)?;
        }
        Err(format!(
            "the button {label:?} could not be scrolled into view in {REVEAL_TRIES} turns"
        ))
    }

    /// The label and the detached state of the chip of the selected tier at index position
    /// `position`.
    fn chip_at(&self, position: f32) -> Option<(String, bool)> {
        self.ui
            .global::<EditorModel>()
            .get_selected_tier_chips()
            .iter()
            .find(|chip| (chip.position - position).abs() < 1e-3)
            .map(|chip| (chip.label.to_string(), chip.detached))
    }

    /// Where the icon whose label starts with `prefix` is, with its strip and the inspector's
    /// scrolling area; `None` while the inspector does not show them.
    fn chip_place(&self, prefix: &str) -> Option<ChipPlace> {
        let view = self.view();
        let inspector = view
            .root
            .query_descendants()
            .match_inherits("EditorInspector")
            .find_first()?;
        let viewport = Spot::of(
            &inspector
                .query_descendants()
                .match_inherits("ScrollView")
                .find_first()?,
        );
        let everything = inspector.query_descendants().find_all();
        let strip = everything
            .iter()
            .find(|element| {
                element
                    .accessible_label()
                    .is_some_and(|label| label == CHIP_STRIP)
            })
            .map(Spot::of)?;
        let icon = everything
            .iter()
            .filter(|element| {
                element
                    .accessible_label()
                    .is_some_and(|label| label.starts_with(prefix))
            })
            .map(Spot::of)
            .find(|spot| spot.usable(view.size))?;
        Some(ChipPlace {
            icon,
            strip,
            viewport,
        })
    }

    /// Scrolls until the icon whose label starts with `prefix` can be clicked: the inspector up
    /// to the Facets section, then the strip of chips along to the chip. Both are wheel turns
    /// over the control, as a learner would make them.
    fn reveal_chip_icon(&mut self, prefix: &str) -> Result<Spot, String> {
        self.scroll_inspector(3000.0)?;
        for _ in 0..REVEAL_TRIES {
            let Some(place) = self.chip_place(prefix) else {
                self.scroll_inspector(-250.0)?;
                continue;
            };
            if !centre_inside(&place.viewport, &place.strip) {
                self.scroll_inspector(-250.0)?;
            } else if wholly_inside(&place.strip, &place.icon) {
                return Ok(place.icon);
            } else {
                self.wheel_across(&place.strip, -120.0)?;
            }
        }
        Err(format!(
            "the chip icon {prefix:?} could not be scrolled into view in {REVEAL_TRIES} turns"
        ))
    }
}
