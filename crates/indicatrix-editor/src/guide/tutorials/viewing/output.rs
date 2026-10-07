//! Output: cutting mode, the four exports (`.asc`, `.gcs`, the HTML cutting sheet, the diagram
//! PNG), Save and Open.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{ADVANCED, FILES, event};
use crate::guide::{
    Goal, Group, Guide, GuideCategory, GuideStep, StartingState,
    tutorials::viewing_events::{
        ASC_EXPORTED, CUTTING_STEP_MARKED, DESIGN_REPLACED, DESIGN_SAVED, DIAGRAM_EXPORTED,
        GCS_EXPORTED, SHEET_EXPORTED,
    },
};

/// The tier form, the tier table and Undo: Save's lesson first changes a tier.
const EDIT: &[Group] = &[Group::TierForm, Group::TierTable, Group::History];

/// File operations, the Advanced controls and Undo: the Gem Cut Studio export is an advanced
/// export, which the Simple interface leaves out of the Export... menu unless a step unlocks it.
const GEM_CUT_STUDIO: &[Group] = &[Group::FileOps, Group::Advanced, Group::History];

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![
        cutting_mode(),
        export_asc(),
        export_gcs(),
        cutting_sheet(),
        diagram_png(),
        save(),
        open(),
    ]
}

fn cutting_mode() -> Guide {
    Guide::new(
        "output-cutting-mode",
        "Cutting mode",
        "Walk through the cutting steps one page at a time at the machine, and tick off what you have cut.",
        GuideCategory::Output,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "One step per page",
            "Cutting mode is the cutting instructions made for the bench: one step on each page, large enough to read from your lap.",
        )
        .actions([
            "Each page shows the tier, its angle in large figures, the indices to cut as chips, and a picture of the stone after the step beside the index wheel.",
            "Right arrow or Page Down goes to the next step, left arrow or Page Up to the one before. Looking never marks anything.",
            "Space or D marks the step done and goes on. Esc closes cutting mode.",
        ])
        .why("The cutting sheet and the Schedule tab hold the same numbers in one long list. Cutting mode is for the moment you are cutting, with the next step always in front of you.")
        .highlight("cutting_mode_button")
        .allow(ADVANCED),
    )
    .step(
        GuideStep::new(
            "Open it and mark a step",
            "Cutting mode covers the whole window, so this step carries everything you need to do in it.",
        )
        .actions([
            "Click the Edit tab, then Cut Mode in the command bar (or choose Edit, Cutting Mode..., or press Ctrl+K and type cutting).",
            "Read the first page.",
            "Press Space or D, or click Mark step done, to mark the step done.",
            "Press Esc to close cutting mode and come back to this panel.",
        ])
        .check("The progress line at the bottom of the page reads 1 of 4 steps done.")
        .why("Your marks are kept in your library on this computer, filed under the design. There is nothing to save, and closing the program loses nothing.")
        .goal(event(CUTTING_STEP_MARKED), "A step marked done")
        .highlight("cutting_mode_button")
        .allow(ADVANCED),
    )
    .step(
        GuideStep::new(
            "Index ticks and changed steps",
            "Two kinds of mark follow each other, and both are kept for the design.",
        )
        .actions([
            "Click an index chip to tick that one index; click it again to take the tick back. Ticks help when a step has many indices that you cut over several sittings.",
            "Ticking the last index of a step marks the step done. Taking the done mark back from a step clears its ticks.",
            "If you change a step's angle, indices, depth or cheater offset after marking it, the page shows Changed since you marked it and the step no longer counts as done. Names, notes and Meets wording do not change the cut, so they do not count.",
            "Reset progress, at the bottom of the screen, removes every mark of this design after asking.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn export_asc() -> Guide {
    Guide::new(
        "output-export-asc",
        "Export the cutting instructions (.asc)",
        "Write the design as a plain .asc schedule to hand to another cutter or program.",
        GuideCategory::Output,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "The .asc file",
            "An .asc file is the long-standing plain-text schedule: every facet's angle, index and meet instruction.",
        )
        .actions([
            "Export .asc in the command bar writes the design as you edited it in this tab, wherever you choose.",
            "It does not add anything to your library. Save writes the whole .indicatrix design file instead.",
        ])
        .why("Use .asc to hand a schedule to another cutter or program. Use Save to keep working on the design later.")
        .highlight("export_asc_button")
        .allow(FILES),
    )
    .step(
        GuideStep::new(
            "Export it",
            "Exporting re-solves every tier's depth from the design as it is now.",
        )
        .actions([
            "Click Export .asc in the command bar (or choose File, Export Edited .asc).",
            "In the Save dialog choose a folder and a name, then click Save.",
        ])
        .check("A toast says Exported edited schedule to the path you chose.")
        .why("If the design does not solve to a closed solid you are asked first; Export Anyway stamps a warning into the file's header, and Cancel writes nothing.")
        .goal(event(ASC_EXPORTED), "An .asc file written")
        .highlight("export_asc_button")
        .allow(FILES),
    )
    .step(
        GuideStep::new(
            "What is in the file",
            "An .asc file has no room for some of what the editor knows.",
        )
        .actions([
            "Tier names are written with every run of spaces turned into an underscore, so Crown Main reads Crown_Main.",
            "A cheater offset has no field of its own, so the tier's indices are shifted by a fraction of a tooth instead.",
            "A tier whose angle follows another tier's through a relation is written as a plain angle; the relation stays in the .indicatrix file.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn export_gcs() -> Guide {
    Guide::new(
        "output-export-gcs",
        "Export for Gem Cut Studio (.gcs)",
        "Write the cutting instructions as a Gem Cut Studio .gcs file (experimental).",
        GuideCategory::Output,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Gem Cut Studio files",
            "The .gcs export writes the same cutting instructions as Export .asc, in the file format Gem Cut Studio reads.",
        )
        .actions([
            "The app computes every facet's polygon from the facet planes and rescales the stone the way Gem Cut Studio does.",
            "It is marked experimental: it follows the published Gem Cut Studio 1.1 file description and reads back correctly here, but it has not yet been checked in Gem Cut Studio itself.",
        ])
        .why("Chiral designs, with the crown and pavilion twisted against each other, are untested.")
        .highlight("export_menu")
        .allow(GEM_CUT_STUDIO),
    )
    .step(
        GuideStep::new(
            "Export it",
            "The export sits on the command bar's Export... menu and in the File menu.",
        )
        .actions([
            "Click Export... in the command bar, then Export as Gem Cut Studio (.gcs)...",
            "In the Save dialog choose a folder and a name, then click Save.",
        ])
        .check("A toast names the file and reminds you to check it in Gem Cut Studio before cutting from it.")
        .goal(event(GCS_EXPORTED), "A .gcs file written")
        .highlight("export_menu")
        .allow(GEM_CUT_STUDIO),
    )
    .step(
        GuideStep::new(
            "Check it before cutting",
            "An experimental file deserves a look before it goes near a machine.",
        )
        .actions([
            "Open the file in Gem Cut Studio and compare it with the faceting diagram.",
            "This app also opens .gcs and GemCAD .gem files with Open: they are converted to .asc cutting instructions.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn cutting_sheet() -> Guide {
    Guide::new(
        "output-cutting-sheet",
        "The cutting sheet",
        "Write a printable HTML cutting sheet: facet counts, size ratios, four views and every tier in cutting order.",
        GuideCategory::Output,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "A sheet to print",
            "The cutting sheet is one self-contained HTML file meant to be opened in a browser and printed.",
        )
        .actions([
            "A header with the title, designer and date, then Facet Data, Size Data and Design Data, then four small drawings of the stone: from above, the side, the end and from below.",
            "Two tables of every tier in cutting order, Pavilion first and Crown second with the table last: number, code (P1, G1, C1, T), angle, indices, the instruction, which starts with the tier's name and says what it meets, the solved mast, and any cheater offset.",
        ])
        .why("The sheet always solves the design afresh, because the masts it prints are what you set a mast gauge to.")
        .highlight("export_menu")
        .allow(FILES),
    )
    .step(
        GuideStep::new(
            "Export it",
            "The sheet is on the Export... menu of the command bar.",
        )
        .actions([
            "Click Export... in the command bar, then Export Cutting Sheet (HTML)...",
            "In the Save dialog choose a folder and a name, then click Save.",
        ])
        .check("A toast says Wrote cutting sheet to the path you chose.")
        .why("A design that does not solve has nothing to print, so you get Cannot build a cutting sheet and the reason instead.")
        .goal(event(SHEET_EXPORTED), "A cutting sheet written")
        .highlight("export_menu")
        .allow(FILES),
    )
    .step(
        GuideStep::new(
            "Open and print",
            "The file is plain HTML.",
        )
        .actions(["Open it in any browser and print from there, or keep it as a record of the cut."])
        .why("Every control is unlocked again."),
    )
}

fn diagram_png() -> Guide {
    Guide::new(
        "output-diagram-png",
        "Export the diagram (PNG)",
        "Write the crown, pavilion and profile drawing as an image.",
        GuideCategory::Output,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "The diagram as a picture",
            "The same drawing the Diagram view shows, as an image of its own.",
        )
        .actions([
            "It is about 1800 by 720 pixels, roughly 150 DPI across an A4 landscape page, so it is meant to be viewed or printed at full size.",
            "The cutting sheet carries four smaller views of the stone instead.",
        ])
        .highlight("export_menu")
        .allow(FILES),
    )
    .step(
        GuideStep::new(
            "Export it",
            "The diagram is on the Export... menu of the command bar.",
        )
        .actions([
            "Click Export... in the command bar, then Export Diagram (PNG)...",
            "In the Save dialog choose a folder and a name, then click Save.",
        ])
        .check("A toast says Wrote diagram to the path you chose.")
        .why("A design that does not solve cannot be drawn, so you get Cannot draw this design and the reason instead.")
        .goal(event(DIAGRAM_EXPORTED), "A diagram image written")
        .highlight("export_menu")
        .allow(FILES),
    )
    .step(
        GuideStep::new(
            "Done",
            "You can put the picture on a bench sheet or send it along with the .asc file.",
        )
        .why("Every control is unlocked again."),
    )
}

fn save() -> Guide {
    Guide::new(
        "output-save",
        "Save a design",
        "Save the whole design to its own .indicatrix file, and know what happens if the program closes first.",
        GuideCategory::Output,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Make a change",
            "A saved design has a change worth saving, and the program shows you when it has not been.",
        )
        .actions([
            "Click the Crown Main row in the tier table.",
            "Change Angle to 35 in the Tier tab.",
            "Click Save Tier.",
        ])
        .check("The row reads 35.0, the window title starts with an asterisk and the status strip shows an amber UNSAVED badge.")
        .goal(Goal::tier("Crown Main", 35.0), "Crown Main at 35 degrees")
        .highlight("inspector_tier")
        .allow(EDIT),
    )
    .step(
        GuideStep::new(
            "Save the design",
            "Save writes the whole design into one .indicatrix file.",
        )
        .actions([
            "Click Save in the command bar (or press Ctrl+S).",
            "The first time, a Save As dialog offers a name; choose a folder and click Save.",
        ])
        .check("A toast names the file, and the UNSAVED badge and the asterisk go away.")
        .why("The file holds every tier in full, the rough, the material, the girdle size and a short history of your edits, so nothing else has to sit beside it. Save also adds or updates the design's entry in your library.")
        .goal(event(DESIGN_SAVED), "The design saved")
        .highlight("save_button")
        .allow(FILES),
    )
    .step(
        GuideStep::new(
            "Save again, and Save As",
            "After the first Save, the file belongs to the design.",
        )
        .actions([
            "Save now writes straight to the same file, and keeps the previous version beside it as a .bak, one generation back.",
            "Save As (File menu, or Ctrl+Shift+S) always asks for a file, and the design belongs to the new file from then on.",
            "A design that does not solve can still be saved: it is written as a draft, and opens again with every tier as you left it.",
        ])
        .why("A file saved by a newer version of the program is refused with a message, not opened wrongly."),
    )
    .step(
        GuideStep::new(
            "If the program closes first",
            "You do not have to save every minute.",
        )
        .actions([
            "While there are unsaved changes the program writes an autosave every two minutes, to its own folder, never over your design.",
            "After an unclean exit, the next start asks Recover unsaved work? with Recover, Not now and Delete.",
            "A recovered design is never written back by Save: the first Save asks for a file name.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn open() -> Guide {
    Guide::new(
        "output-open",
        "Open a design",
        "Open a saved .indicatrix file, and see what else Open accepts.",
        GuideCategory::Output,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Save the design first",
            "There has to be a file to open.",
        )
        .actions([
            "Click Save in the command bar.",
            "Choose a folder, keep the name offered and click Save.",
        ])
        .check("A toast names the file you saved.")
        .goal(event(DESIGN_SAVED), "The design saved")
        .highlight("save_button")
        .allow(FILES),
    )
    .step(
        GuideStep::new(
            "Open it",
            "Open replaces the design in the editor with the one in the file.",
        )
        .actions([
            "Click Open in the command bar (or press Ctrl+O).",
            "Choose the .indicatrix file you just saved.",
        ])
        .check("The window title names the file, and the design is as you saved it.")
        .why("Open looks at what the file holds, not at its name. If the design on screen has unsaved changes you are asked first.")
        .goal(event(DESIGN_REPLACED), "A design opened")
        .highlight("open_button")
        .allow(FILES),
    )
    .step(
        GuideStep::new(
            "More ways to open",
            "Open is not only for .indicatrix files.",
        )
        .actions([
            "File, Open Recent lists the designs you opened or saved lately.",
            "Open also takes a plain .asc file, a GemCAD .gem file or a Gem Cut Studio .gcs file; the design is converted to .asc cutting instructions, and Save writes a new .indicatrix beside it, never over your file.",
            "Older .indicatrix.toml files still open.",
        ])
        .why("Every control is unlocked again."),
    )
}
