//! The library: importing designs, searching and filtering, loading a design into the editor, and
//! the Rough Planner.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{FILES, VIEWPORT, event};
use crate::guide::{
    Guide, GuideCategory, GuideStep, StartingState,
    tutorials::viewing_events::{
        ASC_EXPORTED, DESIGN_REPLACED, LIBRARY_DESIGN_SELECTED, LIBRARY_FILTERED,
        LIBRARY_FILTERS_RESET, LIBRARY_IMPORTED, LIBRARY_SEARCH_CLEARED, LIBRARY_SEARCHED,
        ROUGH_PLAN_FINISHED, ROUGH_PLANNER_OPENED,
    },
};

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![
        import(),
        search(),
        filters(),
        load_design(),
        rough_planner(),
    ]
}

fn import() -> Guide {
    Guide::new(
        "library-import",
        "Import designs into the library",
        "Bring an .asc, .gem or .gcs file into your local library.",
        GuideCategory::Library,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Make a file to import",
            "Import needs a design file. This one writes one from the design on screen.",
        )
        .actions([
            "Click Export .asc in the command bar.",
            "In the Save dialog choose a folder you will find again, then click Save.",
        ])
        .check("A toast says Exported edited schedule to the path you chose.")
        .why("You can also import a file you already have: .asc, a GemCAD .gem or a Gem Cut Studio .gcs.")
        .goal(event(ASC_EXPORTED), "An .asc file written")
        .highlight("export_asc_button")
        .allow(FILES),
    )
    .step(
        GuideStep::new(
            "Import it",
            "The Import panel opens over the window, so this step carries everything you need.",
        )
        .actions([
            "Click Import at the top right of the window.",
            "Click Choose file... and pick the .asc file you just wrote.",
            "Wait for the import to finish. To import a whole folder instead, tick Include subfolders first, then click Choose folder...",
        ])
        .check("A toast and the status line at the top report what was imported, and the new design is selected in the Catalog list.")
        .why("A .gem or .gcs file is converted to .asc cutting instructions, and the original file stays attached to the design. A file with the same name as one already in the library asks before it replaces it.")
        .goal(event(LIBRARY_IMPORTED), "A design imported")
        .highlight("library_import")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Previews",
            "After an import the program asks whether to draw pictures of the new designs.",
        )
        .actions([
            "Full render traces them, and takes longest.",
            "Quick (solid) draws a flat-shaded picture of each in moments.",
            "Skip draws nothing; you can make previews later from the Library menu or a design's right-click menu.",
            "Tick Remember my choice to stop being asked.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn search() -> Guide {
    Guide::new(
        "library-search",
        "Search the library",
        "Find a design by title or designer, and select it to read its details.",
        GuideCategory::Library,
    )
    .starting(StartingState::CurrentDesign)
    .step(
        GuideStep::new(
            "The catalogue",
            "The Catalog panel on the left lists every design that matches your search and filters.",
        )
        .actions([
            "Its header reads Catalog (N of M): how many designs match, out of how many there are.",
            "If the list is empty, run the Import tutorial first, so there is something to find.",
        ])
        .highlight("library_search")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Type in the search box",
            "The search box matches a design's title and designer.",
        )
        .actions([
            "Click the search box at the top (or press Ctrl+F) and type a few letters of a title or a designer.",
        ])
        .check("The list shortens and the header's N changes.")
        .why("The Shape and Gear drop-downs next to the box narrow the list further.")
        .goal(event(LIBRARY_SEARCHED), "A search typed")
        .highlight("library_search")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Clear it",
            "A small cross appears in the box once there is text in it.",
        )
        .actions(["Click the cross at the right end of the search box."])
        .check("The whole list is back.")
        .why("Deleting the last letter with the Backspace key empties the box too.")
        .goal(event(LIBRARY_SEARCH_CLEARED), "The search box emptied")
        .highlight("library_search")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Select a design",
            "Selecting a design shows its details without changing the design in the editor.",
        )
        .actions(["Click a card in the Catalog list."])
        .check("The detail header shows the title, the designer and a row of chips for shape, gear, facets, proportions and refractive index.")
        .why("Right-click a card for more: Generate Previews, Compute Tilt Curves, Ignore, Exclude from planner and Build this design.")
        .goal(event(LIBRARY_DESIGN_SELECTED), "A design selected")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Done",
            "To work on the selected design, load it into the editor: see the Load a design tutorial.",
        )
        .why("Every control is unlocked again."),
    )
}

fn filters() -> Guide {
    Guide::new(
        "library-filters",
        "Filter and sort the library",
        "Narrow the catalogue by shape, gear and ranges, and put it in the order you like.",
        GuideCategory::Library,
    )
    .starting(StartingState::CurrentDesign)
    .step(
        GuideStep::new(
            "Pick a shape",
            "The Shape drop-down keeps only designs of one shape classification.",
        )
        .actions(["Open the Shape drop-down next to the search box and choose a shape."])
        .check("The list shortens. Choosing All Shapes puts it back.")
        .why("The Gear drop-down next to it does the same for the index gear: the number of teeth the design is cut on.")
        .goal(event(LIBRARY_FILTERED), "A filter changed")
        .highlight("library_filters")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Range filters",
            "The Advanced Filters panel opens over the window, so this step carries everything you need.",
        )
        .actions([
            "Click Filters, to the right of the Sort drop-down.",
            "Drag a handle of the Refractive Index slider. The panel's first line counts how many designs match.",
            "Click outside the panel to close it.",
        ])
        .check("The Filters button turns amber with a dot while a filter is active.")
        .why("Each slider spans the range your catalogue really has. RI Match centres a tolerance band on a refractive index, and Use current material takes the one in the viewport. Tilt Performance filters narrow by windowing, extinction or brilliance.")
        .goal(event(LIBRARY_FILTERED), "A range filter changed")
        .highlight("library_filters")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Sort the list",
            "The Sort drop-down puts the list in an order.",
        )
        .actions(["Open the Sort drop-down and choose Newest."])
        .check("The designs added last are at the top.")
        .why("The other orders are Catalogue order, Title and Recently edited.")
        .goal(event(LIBRARY_FILTERED), "The sort order changed")
        .highlight("library_filters")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Put everything back",
            "An active filter is easy to forget, so the Filters button stays amber until you clear it.",
        )
        .actions([
            "Click Filters, then Reset Filters at the bottom of the panel, then click outside the panel.",
            "Choose All Shapes and All Gears in the two drop-downs if you set them.",
        ])
        .check("The Filters button is no longer amber and the header reads Catalog (M of M).")
        .why("The sort order is not a filter, so it can stay as it is. A search in the box counts as a filter: clear it too.")
        .goal(event(LIBRARY_FILTERS_RESET), "Every filter and the search off")
        .highlight("library_filters")
        .allow(VIEWPORT),
    )
}

fn load_design() -> Guide {
    Guide::new(
        "library-load-design",
        "Load a design into the editor",
        "Pick a design in the library and open it in the editor.",
        GuideCategory::Library,
    )
    .starting(StartingState::CurrentDesign)
    .step(
        GuideStep::new(
            "Select a design",
            "Load Selected opens the design that is highlighted in the Catalog list.",
        )
        .actions([
            "Click a card in the Catalog list. If the list is empty, run the Import tutorial first.",
        ])
        .check("The detail header shows the design's title and chips.")
        .goal(event(LIBRARY_DESIGN_SELECTED), "A design selected")
        .allow(FILES),
    )
    .step(
        GuideStep::new(
            "Load it",
            "Loading replaces the design in the editor with the selected one.",
        )
        .actions([
            "Click the Edit tab above the picture.",
            "Click Load Selected in the command bar. If the design on screen has unsaved changes you are asked first.",
        ])
        .check("The tiers of the design are in the tier table and its stone is in the Solid view.")
        .why("The design is built from the entry's own file (its .asc, else its .gem, else its .gcs). An entry with only an angle table is rebuilt from the table with placeholder depths, so solve it before you trust it.")
        .goal(event(DESIGN_REPLACED), "A design loaded")
        .highlight("load_button")
        .allow(FILES),
    )
    .step(
        GuideStep::new(
            "What loading does not do",
            "Loading never changes the library entry.",
        )
        .actions([
            "Your edits belong to the design in the editor. Save writes them to a .indicatrix file and updates the library entry; Export .asc writes a file and leaves the library alone.",
            "The status strip may offer to set the material when the design's refractive index matches a built-in one.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn rough_planner() -> Guide {
    Guide::new(
        "library-rough-planner",
        "The Rough Planner",
        "Model a piece of rough and find the heaviest ways to cut stones from it.",
        GuideCategory::Library,
    )
    .starting(StartingState::CurrentDesign)
    .step(
        GuideStep::new(
            "Open the Rough Planner",
            "The planner is a window of its own, so you can keep working in the library beside it.",
        )
        .actions(["Choose Library, then Plan Rough... in the menu bar."])
        .check("A window opens with the rough and the plan on the left, a 3D view in the middle and the results on the right.")
        .why("Closing the window only hides it: the model and the results stay until you quit. Its inputs are not remembered between launches, so save a plan you want to keep.")
        .goal(event(ROUGH_PLANNER_OPENED), "The Rough Planner opened")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Model the rough",
            "The ROUGH section describes the piece you hold.",
        )
        .actions([
            "Choose Block, Cylinder, Pebble or Mesh (OBJ), then type its size in millimetres (Rough X, Y and Z for a block).",
            "The live readout under the material shows the model's volume and weight in carats. Type your scale's weight over the Carat field to see a weight check, and Fit to weight scales the model to it.",
            "Take flat cuts off the shape with +Edge, +Corner and +Face, or by clicking an edge, a corner or a face in the 3D view.",
        ])
        .why("Quartz is the default material. Only materials with a known specific gravity are listed, because the specific gravity turns volume into carats.")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Plan",
            "The PLAN section says how many stones to aim for and which designs to use.",
        )
        .actions([
            "Set Stones (up to) to 1 to get the best ten single stones for this rough.",
            "Leave Candidate designs on Current filter, which uses the designs the library shows now. Narrow the library with a short search first so the first run is quick.",
            "Click Plan, or press Ctrl+Enter.",
        ])
        .check("The stage line and a percentage show the progress, then up to ten result cards appear on the right.")
        .why("The first plan measures every design it may use, once, and keeps the measurements, so later plans start straight away. Plan is greyed out while the model is not valid; the red message under ROUGH says why.")
        .goal(event(ROUGH_PLAN_FINISHED), "A plan finished")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Read the results",
            "Each card is one layout, ranked by the weight of finished stone.",
        )
        .actions([
            "Show all in library filters the library to the designs of the results.",
            "Save selected and Save all keep plans; Saved plans in the title bar reopens them, and a plan can be shared as a file.",
            "A design that keeps winning but that you do not want to cut can be excluded with the Exclude pill on its row.",
        ])
        .why("Every control is unlocked again."),
    )
}
