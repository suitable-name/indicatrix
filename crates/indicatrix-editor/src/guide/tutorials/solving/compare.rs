//! The History panel and jumping, saved variants, Snapshot and Compare, and Edit as Text with its
//! comparison.

use super::{
    HISTORY, HISTORY_EDIT, RICH, RICH_PAVILION, TOOLS, TOOLS_EDIT, check, event, events,
    rich_at_start, tier_at,
};
use crate::guide::{Goal, GoalContext, Guide, GuideCategory, GuideStep, StartingState};

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![history(), variants(), snapshot_compare(), raw_text()]
}

/// The crown at 36 degrees and the pavilion where the template put it: the Rich Teaching Design
/// after the history lesson's first change.
fn after_first_change(ctx: &GoalContext<'_>) -> bool {
    tier_at(ctx.design, "Crown Main", 36.0) && tier_at(ctx.design, "Pavilion Main", RICH_PAVILION)
}

/// The crown at 36 degrees and the pavilion at -41: the stone after both changes of the history
/// lesson.
fn after_both_changes(ctx: &GoalContext<'_>) -> bool {
    tier_at(ctx.design, "Crown Main", 36.0) && tier_at(ctx.design, "Pavilion Main", -41.0)
}

fn history() -> Guide {
    Guide::new(
        "solving-history",
        "History panel and jumping",
        "Read the list of every change, go back to any step in one click, and come forward again.",
        GuideCategory::Optimizing,
    )
    .starting(StartingState::Template(RICH))
    .step(
        GuideStep::new(
            "Open the History tab",
            "The History tab lists every change you make to the open design, newest first, with a small picture of the design at that step.",
        )
        .actions([
            "Click History, the last tab at the top of the inspector below the tier table.",
            "At the top of the tab, check that Steps is picked. Variants is the other view.",
        ])
        .check("the tab lists one row, Start.")
        .why("Start is the design as it was when you created it. You can also press Ctrl+K and type history.")
        .goal(Goal::InspectorTab(4), "the History tab open")
        .highlight("history_tab")
        .allow(HISTORY),
    )
    .step(
        GuideStep::new(
            "Change the crown",
            "Make a first change and watch it appear in the list.",
        )
        .actions([
            "Double-click the ANGLE cell of the Crown Main row in the tier table, or pick the row and press F2.",
            "Type 36 and press Enter.",
        ])
        .check("a new row at the top of the History tab, above Start, with a picture and a line saying what the step did.")
        .why("The row marked Now is the step the design stands at. Several quick nudges of the same angle count as one step, as they do for Undo.")
        .goal(Goal::tier("Crown Main", 36.0), "Crown Main at 36.0")
        .highlight("tier_table")
        .allow(HISTORY_EDIT),
    )
    .step(
        GuideStep::new(
            "Change the pavilion",
            "Make a second change.",
        )
        .actions([
            "Double-click the ANGLE cell of the Pavilion Main row, type 41 and press Enter.",
        ])
        .check("two rows above Start. The newest one has the Now mark.")
        .goal(
            check("the crown at 36 and the pavilion at 41", after_both_changes),
            "Pavilion Main at 41.0 with Crown Main at 36.0",
        )
        .highlight("tier_table")
        .allow(HISTORY_EDIT),
    )
    .step(
        GuideStep::new(
            "Go back one step",
            "Click a row to go to that step.",
        )
        .actions([
            "In the History tab, click the row just under the top one: the step after the crown change.",
            "The design changes to what it was at that step. The tier table and the viewport follow at once. The top row is now dimmed and reads undone.",
        ])
        .check("Pavilion Main reads 40.00 again, and Crown Main stays at 36.00.")
        .why("A jump is not an edit: it adds nothing to the history and loses nothing. A jump does not press Solve for you either. As after Undo, the solved values in the tier table are out of date until you click Solve.")
        .goal(
            check("the crown at 36 and the pavilion at 40", after_first_change),
            "the design back at the step after the crown change",
        )
        .highlight("history_tab")
        .allow(HISTORY),
    )
    .step(
        GuideStep::new(
            "Go back to Start",
            "Any step can be reached from any other, not only the one next to it.",
        )
        .actions(["Click the Start row at the bottom of the list."])
        .check("Crown Main reads 34.50 again.")
        .why("The whole list is one Tab stop. Up and Down move an outline between rows, Enter or Space goes to the outlined row, Home moves to the newest step and End to Start. Moving the outline only looks; nothing changes until you press Enter.")
        .goal(
            check("the template's own crown and pavilion", rich_at_start),
            "the design back at Start",
        )
        .highlight("history_tab")
        .allow(HISTORY),
    )
    .step(
        GuideStep::new(
            "Go forward again",
            "Steps you went past stay in the list until you make a new change.",
        )
        .actions(["Click the top row, the newest step. It is dimmed because it is ahead of where you stand."])
        .check("Crown Main reads 36.00 and Pavilion Main 41.00.")
        .goal(
            check("the crown at 36 and the pavilion at 41", after_both_changes),
            "the design forward at the newest step",
        )
        .highlight("history_tab")
        .allow(HISTORY),
    )
    .step(
        GuideStep::new(
            "Undo from where you stand",
            "Undo and Redo keep working after a jump.",
        )
        .actions(["Press Ctrl+Z, or click Undo on the command bar."])
        .check("Pavilion Main is back at 40.00, and the Now mark moved down one row.")
        .why("Undo moves one step older than where you stand and Redo one step newer, exactly as if you had pressed them that many times.")
        .goal(
            check("the crown at 36 and the pavilion at 40", after_first_change),
            "one step undone",
        )
        .highlight("history_tab")
        .allow(HISTORY),
    )
    .step(
        GuideStep::new(
            "A new change drops the undone steps",
            "One rule to know before you rely on the list.",
        )
        .actions([
            "The design stands at the first change now, and the pavilion change is dimmed as undone.",
            "If you changed the design now, for example another crown angle, the undone step would be dropped from the list for good. This is the same rule as after Undo and then editing.",
            "Creating a new design, opening a file or loading a library design starts a new history: the list begins again with Start. The history is not saved with the design file.",
        ])
        .why("To keep a design for good before you try something risky, save it as a variant. The next lesson shows how.")
        .allow(HISTORY),
    )
}

fn variants() -> Guide {
    Guide::new(
        "solving-variants",
        "Variants",
        "Keep named copies of a design, open one to go back to it, and compare two versions.",
        GuideCategory::Optimizing,
    )
    .starting(StartingState::Template(RICH))
    .step(
        GuideStep::new(
            "Open the Variants view",
            "A variant is a named copy of the design that you keep, to come back to it or to compare it with another.",
        )
        .actions([
            "Click History, the last tab at the top of the inspector below the tier table.",
            "Click Variants at the top of the tab. If the tab already shows Variants, click Steps and then Variants again.",
        ])
        .check("the tab shows a Save as variant... button.")
        .why("Variants are kept in your library on this computer, under the design's id, and not in the design file. A design file you send to someone does not carry them.")
        .goal(
            Goal::All(vec![
                Goal::InspectorTab(4),
                event(events::VARIANTS_OPENED),
            ]),
            "the Variants view opened",
        )
        .highlight("history_tab")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Save the first variant",
            "Keep the design as it is now.",
        )
        .actions([
            "Click Save as variant...",
            "Leave the name as Variant 1, or type one you will recognise. A note is optional.",
            "Click Save, or press Enter.",
        ])
        .check("a row with a picture, the name and the buttons Open, Rename, Note and Delete.")
        .why("A variant keeps the whole design: the cutting instructions, the rough, the girdle size, the material and the concave tiers. It does not keep the lighting, the undo history or attached files. The design itself is not changed.")
        .goal(event(events::VARIANT_SAVED), "a variant saved")
        .highlight("history_tab")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Change the crown",
            "Try something else.",
        )
        .actions([
            "Double-click the ANGLE cell of the Crown Main row in the tier table, or pick the row and press F2.",
            "Type 38 and press Enter.",
        ])
        .check("the Crown Main row shows 38.00.")
        .goal(Goal::tier("Crown Main", 38.0), "Crown Main at 38.0")
        .highlight("tier_table")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Save a second variant",
            "Keep this version too.",
        )
        .actions([
            "Click Save as variant... again, name it Steeper crown, and click Save.",
        ])
        .check("two rows. The newest one is at the top, and under its name it says it was made from the first.")
        .why("When you open or save a variant, the program remembers it as the one the design came from. The next variant you save is marked from: that one, so a run of saves reads as a chain, and saving after opening an older one starts a second branch.")
        .goal(event(events::VARIANT_SAVED), "a second variant saved")
        .highlight("history_tab")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Go back to the first variant",
            "Opening a variant makes it the open design.",
        )
        .actions([
            "On the older row, Variant 1, click Open.",
        ])
        .check("Crown Main reads 34.50 again, and the row you opened has the Working from mark.")
        .why("Opening a variant is one undo step, and it changes the open design, not the file. Save the design with Save when you want the result in the file. If the open design has a tier whose angle follows a relation and the variant gives it another angle, Open refuses and names the tier. Remove that relation, then open the variant again.")
        .goal(
            check("the template's own crown and pavilion", rich_at_start),
            "the first variant opened",
        )
        .highlight("history_tab")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Undo the open",
            "One Undo takes it back.",
        )
        .actions(["Press Ctrl+Z, or click Undo on the command bar."])
        .check("Crown Main reads 38.00 again.")
        .why("Redo brings the opened variant back. The Steps view shows the open as one step like any other change.")
        .goal(Goal::tier("Crown Main", 38.0), "the open undone")
        .highlight("history_tab")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Compare two designs",
            "Look at two versions next to each other.",
        )
        .actions([
            "Under Compare two designs, pick Variant 1 in the first box and Current design in the second.",
            "Click Compare pictures. The compare window opens with the first design on the left and the second on the right.",
            "In the Advanced interface, Compare text shows the cutting instructions of the two designs line by line instead. Lines only in the second design are marked added, lines only in the first removed, and long runs of unchanged lines are folded into one row. Back to variants, or Esc, returns to the list.",
        ])
        .check("the compare window, or the line-by-line list.")
        .why("This comparison only looks: it has no Keep or Discard. To go to a variant, use Open on its row. The text holds the cutting instructions only, so a different rough, girdle size or material shows in the pictures, not in the text.")
        .goal(event(events::VARIANTS_COMPARED), "two designs compared")
        .highlight("history_tab")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Tidy up",
            "These two variants now live in your library.",
        )
        .actions([
            "Rename and Note on a row change its name or note. Delete asks first, and a deleted variant cannot be brought back. None of them changes the open design.",
            "Delete the two variants of this lesson if you do not want to keep them.",
            "A new design that you never save gets a new id each time it opens, so its variants cannot be found again after you close it. Save the design once to give it a lasting id.",
        ])
        .why("A variant cannot be edited in place: open it, change the design and save a new one. To keep a variant as a file, open it and use Save As.")
        .allow(TOOLS_EDIT),
    )
}

fn snapshot_compare() -> Guide {
    Guide::new(
        "solving-snapshot-compare",
        "Snapshot and Compare",
        "Keep a copy of the design, change it, and see what changed in a table and in two stones side by side.",
        GuideCategory::Optimizing,
    )
    .starting(StartingState::Template(RICH))
    .step(
        GuideStep::new(
            "Take a snapshot",
            "A snapshot is a copy of the design and its solved depths, to compare against later.",
        )
        .actions(["Click Snapshot on the command bar's second row."])
        .check("a message says the snapshot was taken, and the Compare button is no longer dim.")
        .why("A snapshot is not saved anywhere. It lives only in memory for this session: closing the design or the program, or clicking Snapshot again, replaces or loses it. It is not written to the design file.")
        .goal(event(events::SNAPSHOT_TAKEN), "a snapshot taken")
        .highlight("snapshot_button")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Change the crown",
            "Give the design something to differ in.",
        )
        .actions([
            "Double-click the ANGLE cell of the Crown Main row in the tier table, or pick the row and press F2.",
            "Type 36 and press Enter.",
        ])
        .check("the Crown Main row shows 36.00.")
        .goal(Goal::tier("Crown Main", 36.0), "Crown Main at 36.0")
        .highlight("tier_table")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Compare to the snapshot",
            "The table says which angles moved.",
        )
        .actions([
            "Click Compare, next to Snapshot. The Compare to Snapshot window opens over the editor, so everything for this step is here.",
            "Read the line under the title. Its second line says in words how the stone differs optically, or No clear optical difference.",
            "Read the table: one row per tier with its Before and After angles, a Mast column with the signed change of the solved depth, and a Status of Same, Changed, Added or Removed. Crown Main reads Changed, the others Same.",
            "Click Close.",
        ])
        .check("a table of the tiers with Before and After columns.")
        .why("Added is a tier that did not exist at the snapshot, Removed one the snapshot had that is gone. Without a snapshot, Compare is dim and its hint says to press Snapshot first.")
        .goal(event(events::COMPARE_OPENED), "the comparison table opened")
        .highlight("compare_button")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "See both stones",
            "The compare window shows what the stone looks like before and after.",
        )
        .actions([
            "Click Compare again, then Compare visually... at the bottom of the table.",
            "The compare window opens: a separate window you can move and resize.",
        ])
        .check("a window with the snapshot on the left and the design now on the right.")
        .goal(event(events::COMPARE_WINDOW_OPENED), "the compare window opened")
        .highlight("compare_button")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Read the compare window",
            "Close the compare window and the table when you have looked.",
        )
        .actions([
            "Side by side shows the two stones next to each other. Split slider shows one stone with a draggable divider, the before design to its left and the after design to its right. Dragging turns both stones together, the mouse wheel zooms both and a double-click resets them.",
            "Solid is the flat grey view. Traced runs the path tracer at a modest fixed quality in each side's own material and starts once you stop turning the stone.",
            "Under the stones, a small table gives brilliance, windowing, extinction, fire and scintillation for both, with the change and the word better, worse or same. The figures are measured table up, in each side's own material, under the lighting chosen in the viewport when you opened Compare.",
            "A snapshot comparison only offers Close. Keep after and Discard belong to the Retarget and Optimize comparisons.",
        ])
        .why("A change smaller than a threshold, 2 points for brilliance, windowing and extinction, reads same: two stones are never measured to the last digit.")
        .allow(TOOLS_EDIT),
    )
}

fn raw_text() -> Guide {
    Guide::new(
        "solving-raw-text",
        "Edit as Text",
        "Edit the cutting instructions as text, compare the text with a snapshot and apply it in one step.",
        GuideCategory::Optimizing,
    )
    .starting(StartingState::Template(RICH))
    .step(
        GuideStep::new(
            "Take a snapshot",
            "The text comparison needs something to compare with. A design that has never been saved has no file, so use a snapshot.",
        )
        .actions(["Click Snapshot on the command bar's second row."])
        .check("a message says the snapshot was taken.")
        .goal(event(events::SNAPSHOT_TAKEN), "a snapshot taken")
        .highlight("snapshot_button")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Edit the instructions as text",
            "The dialog covers this panel, so everything for this step is here.",
        )
        .actions([
            "Choose Edit > Edit as Text... (or press Ctrl+K and type text).",
            "The box holds the design's cutting instructions as .asc text, the same text Export Edited .asc writes. Each tier is one line that starts with a, then its angle and its depth, with its name after n. A space in a name is written as an underscore.",
            "Find the line that contains Crown_Main. Change its angle, the first number after the a, from 34.5 to 36.",
            "Wait a moment. One sentence under the box says whether the text can be used, and lists what Apply would change.",
            "Click Compare with..., then Snapshot. The box is laid against the snapshot line by line. Your changed line is marked added, the old one removed, and long runs of unchanged lines are folded into one row.",
            "Click Back to the text, then click Apply. The dialog closes.",
        ])
        .check("Crown Main reads 36.00 in the tier table.")
        .why("Apply is one undo step. Tiers are matched by name, so a tier you keep keeps its note, offset, target and relation. The text never holds the preform, the material or the girdle size, and Apply never changes them. Revert writes the text again from the design, and closing with edits that were not applied asks first.")
        .goal(
            Goal::All(vec![
                event(events::RAW_TEXT_DIFF_SHOWN),
                Goal::tier("Crown Main", 36.0),
            ]),
            "the text compared with the snapshot and applied",
        )
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Undo the text edit",
            "The whole text edit is one step.",
        )
        .actions(["Press Ctrl+Z, or click Undo on the command bar."])
        .check("Crown Main reads 34.50 again.")
        .why("The History tab shows the edit as one entry, Edit instructions as text.")
        .goal(
            check("the template's own crown and pavilion", rich_at_start),
            "the text edit undone",
        )
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "The saved file and what the text leaves out",
            "Two more things the dialog does.",
        )
        .actions([
            "Compare with... > Saved file on disk lays the text against the .indicatrix file the design was last opened from or saved to. A design that was never saved says it has nothing to compare with, and a design from a plain .asc compares with that file's text.",
            "A line under the intro, starting Not in this text, names what an .asc file cannot hold for this design: tier notes, cheater offsets, depth and width targets, tier relations, concave tiers, and always the preform, the material and the girdle size.",
            "If the design does not solve, the dialog says so instead of showing depths that are not real. Fix the design first.",
        ])
        .allow(TOOLS),
    )
}
