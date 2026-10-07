//! The Solid viewport: picking facets, the drag handles, Slice, the Diagram view and the Cut
//! slider.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{
    VIEWPORT, VIEWPORT_TABLE, angle_moved, check, depth_moved, event, index_moved, rich_untouched,
};
use crate::guide::{
    Goal, Guide, GuideCategory, GuideStep, StartingState,
    tutorials::viewing_events::{
        CUT_SLIDER_FINISHED, CUT_SLIDER_ROUGH, CUT_SLIDER_STEP, DIAGRAM_PANEL_ENLARGED,
        FACET_PICKED, SLICE_CUTS_STONE, SLICE_DRAWN, SLICE_FLIPPED, SLICE_STARTED,
        SLICE_SYMMETRIC_TOGGLED, SNAP_TOGGLED, TIER_SELECTED,
    },
};

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![
        solid_picking(),
        drag_handles(),
        slice(),
        diagram(),
        cut_slider(),
    ]
}

/// The number of tiers the Rich Teaching Design starts with; Keep in the Slice lesson makes one
/// more.
const START_TIERS: usize = 4;

fn solid_picking() -> Guide {
    Guide::new(
        "viewing-solid-picking",
        "Picking facets in the Solid view",
        "Point at a facet to read its tier, angle and index, then click it to select its whole tier.",
        GuideCategory::Viewing,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "The Solid view",
            "The Edit tab shows your design as a flat-shaded stone you can point at.",
        )
        .actions([
            "Click the Edit tab above the picture, if it is not open.",
            "The buttons above the stone choose the view: Solid, Path-traced, Both and Diagram.",
            "In Solid, Path-traced and Both, drag to turn the stone and scroll to zoom.",
        ])
        .why("The Solid view is quick and independent of the light tracer, so it can follow almost every edit.")
        .highlight("view_modes")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Hover over a facet",
            "Pointing at a facet, without clicking, tells you what it is.",
        )
        .actions([
            "Move the pointer over the stone.",
            "A small panel in the lower-left corner names the tier, its angle, its index, its block (Crown, Pavilion or Girdle) and its margin over the critical angle.",
        ])
        .why("A positive margin means the facet stays clear of the critical angle for the design's refractive index.")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Click a facet",
            "Clicking a facet selects the tier it belongs to.",
        )
        .actions(["Click one of the facets in the ring around the table."])
        .check("The tier's row is highlighted in the tier list and every facet of that tier is tinted on the stone.")
        .why("You picked one facet, but a tier is a whole ring of them, so the whole ring is selected.")
        .goal(event(FACET_PICKED), "A click on a facet")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Pick in another view",
            "Picking works in every view that shows the stone, not only in Solid.",
        )
        .actions(["Click Both above the stone."])
        .check("Both is lit, and the solid's facet edges are drawn over the traced picture.")
        .why("Both puts the Solid view's facet edges on top of the path-traced picture, so you can see which edge belongs to which highlight.")
        .goal(Goal::ViewMode(2), "The Both view")
        .highlight("view_modes")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Click a different tier",
            "Selecting another tier moves the tint to its facets.",
        )
        .actions(["Click a facet that belongs to a different tier."])
        .check("A different row is highlighted in the tier list.")
        .goal(event(FACET_PICKED), "A click on a facet")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Pick from the list instead",
            "Picking also works the other way round: choose the tier first and see where it is.",
        )
        .actions(["Click a row in the tier list."])
        .check("The facets of that tier are tinted on the stone.")
        .why("This is the quickest way to find out which facets a row stands for.")
        .goal(event(TIER_SELECTED), "A tier picked in the list")
        .highlight("tier_table")
        .allow(VIEWPORT_TABLE),
    )
    .step(
        GuideStep::new(
            "Done",
            "You can read any facet by hovering and select its tier by clicking, in the picture or in the list.",
        )
        .actions(["The drag handles tutorial shows how to change the selected tier by dragging."])
        .why("Every control is unlocked again."),
    )
}

fn drag_handles() -> Guide {
    Guide::new(
        "viewing-drag-handles",
        "Drag handles",
        "Change a tier by dragging its angle (A), depth (D) and index (I) handles on the stone.",
        GuideCategory::Viewing,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Select a tier",
            "Three handles grow out of the selected tier's facet, so start by selecting one.",
        )
        .actions([
            "Click the Edit tab above the picture, if it is not open.",
            "Click a facet of the ring around the table (Crown Main).",
        ])
        .check("Three handles appear on the facet: A (a cyan circle), D (a blue square) and I (an amber diamond).")
        .why("The handles need a solved design. A handle drags the whole tier, not only the facet you grabbed.")
        .goal(event(TIER_SELECTED), "A selected tier")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Drag the angle handle",
            "A tilts the tier: steeper one way, shallower the other.",
        )
        .actions([
            "Press on A, the cyan circle, and drag it along its line.",
            "Watch the line under the toolbar: it names the tier and its new angle while you drag.",
        ])
        .check("The tier list shows a new angle for the tier.")
        .why("The angle stays on its own side of zero, so a crown tier stops at 0 degrees instead of turning into a pavilion tier.")
        .goal(check("a tier is at another angle", angle_moved), "A tier at a new angle")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "One drag, one undo",
            "However long you dragged, the whole gesture is a single step in the undo history.",
        )
        .actions(["Press Ctrl+Z, or click Undo."])
        .check("The tier is back at the angle it had before you pressed.")
        .goal(check("the stone is as it started", rich_untouched), "The starting angles back")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Drag the depth handle",
            "D moves the tier in or out along its own normal, which changes its depth.",
        )
        .actions([
            "Press on D, the blue square, and drag it along its line.",
            "Let go, and read the toast under the picture.",
        ])
        .check("The tier list shows a new depth (mast) for the tier.")
        .why("Dragging D pins the tier to the mast you drop it on, replacing whatever it met before. Undo brings that back.")
        .goal(check("a tier is at another depth", depth_moved), "A tier at a new depth")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Turn the tier with the index handle",
            "I turns the tier round the index wheel, a whole number of teeth at a time.",
        )
        .actions([
            "Press on I, the amber diamond, and drag it a few teeth round.",
            "The line under the toolbar counts the teeth.",
        ])
        .check("The tier's facets sit at new index positions.")
        .why("A tier with no index positions, such as the Table, has no I handle.")
        .goal(check("a tier is at other indices", index_moved), "A tier at new indices")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Switch snapping off",
            "By default an angle snaps to 0.1 degrees and a depth to 0.01. The Snap pill turns that off.",
        )
        .actions([
            "Click the Snap pill in the toolbar.",
            "Hold Shift while you drag A or D to move in fine steps instead (0.01 degrees and 0.001).",
        ])
        .check("The Snap pill is no longer lit.")
        .why("The index handle always moves in whole teeth, whatever the pill says.")
        .goal(event(SNAP_TOGGLED), "The Snap pill clicked")
        .highlight("snap_pill")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Switch snapping back on",
            "Free dragging is for fine work; coarse snapping is easier for everything else.",
        )
        .actions(["Click the Snap pill again."])
        .check("The Snap pill is lit.")
        .goal(event(SNAP_TOGGLED), "The Snap pill clicked")
        .highlight("snap_pill")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Cancel a drag",
            "A drag you do not like never has to be undone.",
        )
        .actions([
            "Press Escape while you are still holding the mouse button: the design goes back exactly as it was when you pressed.",
            "Escape with nothing being dragged clears the selection.",
            "Press Ctrl+Z to put back anything you changed in this lesson.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn slice() -> Guide {
    Guide::new(
        "viewing-slice",
        "Slice: cut a facet with the mouse",
        "Draw a line across the stone, cut it in with the depth handle and keep it as a new tier.",
        GuideCategory::Viewing,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Switch Slice on",
            "Slice starts a new tier by drawing on the stone instead of typing an angle and an index.",
        )
        .actions([
            "Click the Edit tab above the picture, then Solid, if they are not already chosen.",
            "Click the Slice pill in the toolbar (or press S with the stone in focus).",
        ])
        .check("The Slice pill is lit and the line under the toolbar says what to do.")
        .why("While Slice is on, a left drag draws a line instead of turning the stone. Slice needs the 3D stone, so it is not offered in the Diagram view.")
        .goal(event(SLICE_STARTED), "Slice switched on")
        .highlight("slice_pill")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Draw a line",
            "The line says where the new facet goes and which side is cut away.",
        )
        .actions([
            "Press on one side of the stone, drag across its upper part and release.",
            "The part to the right of your drag direction is cut away; three faint ticks mark that side.",
        ])
        .check("The new tier appears with a green outline and the line under the toolbar gives its angle and index.")
        .why("Nothing is cut yet and nothing is in the undo history. The plane is snapped to the index wheel and to 0.1 degrees, and placed so it only just touches the stone.")
        .goal(event(SLICE_DRAWN), "A line drawn on the stone")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Flip the side",
            "If the cut would take the wrong side, you do not have to draw again.",
        )
        .actions([
            "Click Flip in the strip under the toolbar (or press F).",
            "Click it again to come back to the first side.",
        ])
        .check("The green outline moves to the other side of the stone.")
        .goal(event(SLICE_FLIPPED), "Flip pressed")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Cut it in",
            "A new facet starts exactly touching the stone, so it cuts nothing yet.",
        )
        .actions([
            "Drag the depth handle D, the blue square, inward.",
            "Stop when the line under the toolbar no longer says to drag the depth handle inward first.",
        ])
        .check("The green outline now cuts into the stone.")
        .why("Keep refuses until the facet really touches the stone. The angle and index handles work on the new tier too; none of these drags is an undo step yet.")
        .goal(event(SLICE_CUTS_STONE), "The new facet cutting the stone")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Symmetric or single",
            "The Symmetric pill decides how many facets the new tier has.",
        )
        .actions([
            "Click Symmetric in the strip under the toolbar to turn it off: the cut becomes a single facet.",
            "Click it again to turn it back on.",
        ])
        .check("The Symmetric pill changes between lit and dark.")
        .why("On, the new tier is cut at every index position the design's symmetry gives it, like any other orbit. Off, it is cut at one position only. The tier keeps its angle and depth when you switch.")
        .goal(event(SLICE_SYMMETRIC_TOGGLED), "Symmetric clicked")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Keep the facet",
            "Keeping turns the provisional facet into a real tier.",
        )
        .actions(["Click Keep in the strip under the toolbar (or press Enter)."])
        .check("A new row appears in the tier list and is selected, and Slice switches off.")
        .why("Keep adds the tier as one undo step. Discard (or Escape) throws it away instead and leaves the design as it was.")
        .goal(Goal::TierCountAtLeast(START_TIERS + 1), "The new tier kept")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Undo the new tier",
            "A kept slice is one step in the history.",
        )
        .actions(["Press Ctrl+Z, or click Undo."])
        .check("The new row is gone from the tier list.")
        .goal(
            check("the slice is undone", |ctx| ctx.design.tiers.len() <= START_TIERS),
            "The slice undone",
        )
        .highlight("tier_table")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Done",
            "Slice is quick for a first sketch of a facet; the tier form is better for exact numbers.",
        )
        .why("Every control is unlocked again."),
    )
}

fn diagram() -> Guide {
    Guide::new(
        "viewing-diagram",
        "The Diagram view",
        "Read the crown, pavilion and profile panels, pick a facet and drag its handle on the flat diagram.",
        GuideCategory::Viewing,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Switch to the Diagram",
            "The Diagram draws your design the way a faceting reference diagram does.",
        )
        .actions([
            "Click the Edit tab above the picture, if it is not open.",
            "Click Diagram above the stone.",
        ])
        .check("Three flat panels appear: Crown, Pavilion and Profile.")
        .why("The Diagram does not turn with the camera, and it always shows the finished design: switching to it puts the Cut slider back to Finished.")
        .goal(Goal::ViewMode(3), "The Diagram view")
        .highlight("view_modes")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Read the three panels",
            "Each panel looks at the stone from a different side.",
        )
        .actions([
            "Crown looks straight down at the top of the stone.",
            "Pavilion looks straight up at the bottom, so it is mirrored left to right compared with the crown.",
            "Profile is a side view: the crown angle, the girdle and the pavilion angle stacked up.",
            "The ring of ticks round the crown and pavilion is the index wheel. Index 0 is always at the top.",
        ])
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Pick a facet",
            "Clicking works as in the Solid view, on the diagram's own layout.",
        )
        .actions(["Click a facet in the Crown panel."])
        .check("The tier's row is highlighted in the tier list, and its handles A, D and I appear on a panel.")
        .why("The exact facet you click is remembered, so the same facet stays picked when you switch back to the Solid view.")
        .goal(event(FACET_PICKED), "A click on a facet")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Drag a handle on the diagram",
            "The handles work exactly as they do on the stone.",
        )
        .actions([
            "Press on A and drag it away from the centre of the wheel to make the facet steeper, or towards the centre to make it shallower.",
            "Hold Shift for fine steps.",
        ])
        .check("The tier list shows a new angle.")
        .why("D moves the tier in or out and I turns it round the wheel, on the crown and pavilion panels. The Profile panel offers handles only for facets seen exactly side on.")
        .goal(check("a tier is at another angle", angle_moved), "A tier at a new angle")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Enlarge a panel",
            "One panel at a time can fill the whole view.",
        )
        .actions([
            "Click the Pavilion pill in the toolbar (or double-click a panel).",
        ])
        .check("The panel fills the view and its pill is lit.")
        .why("Scroll to zoom and drag to pan the diagram; Reset View puts both back.")
        .goal(event(DIAGRAM_PANEL_ENLARGED), "A panel enlarged")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Back to all three",
            "Going back is the same gesture.",
        )
        .actions([
            "Click the lit pill again, double-click the panel, or press Escape.",
            "Press Ctrl+Z to put back the angle you changed.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn cut_slider() -> Guide {
    Guide::new(
        "viewing-cut-slider",
        "The Cut slider",
        "Watch the stone being cut: the rough, the stone after each step and the finished design.",
        GuideCategory::Viewing,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Find the Cut slider",
            "The Cut slider shows the stone as it looks after each tier of the cutting steps.",
        )
        .actions([
            "Click the Edit tab above the picture, then Solid, Path-traced or Both.",
            "The slider is at the left end of the toolbar, with a label beside it that reads Finished.",
        ])
        .why("It appears as soon as the design has a tier. The Diagram does not show it, because the Diagram always draws the finished design.")
        .highlight("cut_slider")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Go back to the rough",
            "All the way to the left is the block before any facet is cut.",
        )
        .actions(["Drag the slider all the way to the left."])
        .check("The label reads Rough and the stone is the uncut block.")
        .why("Every position is a whole stone: a step only removes material, so even the rough is a closed solid you can turn and pick.")
        .goal(event(CUT_SLIDER_ROUGH), "The slider at the rough")
        .highlight("cut_slider")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Step through the cuts",
            "Each step to the right adds one tier.",
        )
        .actions(["Drag the slider one step to the right, then another."])
        .check("The label names the tier you have just cut and counts the steps, for example After Crown Main (2 of 4).")
        .why("The steps follow the order of the cutting instructions. With a concave tier they follow the order the stone is really cut in.")
        .goal(event(CUT_SLIDER_STEP), "The slider between rough and finished")
        .highlight("cut_slider")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Back to the finished stone",
            "All the way to the right is the whole design.",
        )
        .actions(["Drag the slider all the way to the right."])
        .check("The label reads Finished and the border is no longer amber.")
        .why("The slider's border turns amber whenever it is anywhere but Finished, so you can tell at a glance that you are looking at part of the design.")
        .goal(event(CUT_SLIDER_FINISHED), "The slider at Finished")
        .highlight("cut_slider")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "What the slider leaves alone",
            "The slider changes what you see, never the design.",
        )
        .actions([
            "While it is short of Finished the drag handles are hidden.",
            "Live Render follows the cut, with a small amber badge and a Show finished button.",
            "Exports, the tilt video and the tilt curves always use the finished stone.",
        ])
        .why("Every control is unlocked again."),
    )
}
