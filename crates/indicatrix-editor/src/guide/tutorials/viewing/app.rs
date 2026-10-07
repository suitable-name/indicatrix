//! The program itself: the Simple | Advanced switch, high contrast, the interface scale, larger
//! handles, the command palette, the keyboard, the manual and the glossary.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{ADVANCED, VIEWPORT, VIEWPORT_TABS, event};
use crate::guide::{
    Goal, Guide, GuideCategory, GuideStep, StartingState,
    tutorials::viewing_events::{
        GLOSSARY_OPENED, HELP_OPENED, HIGH_CONTRAST_CHANGED, INTERFACE_MODE_CHANGED,
        LARGE_HANDLES_CHANGED, LIVE_RENDER_OPENED, SHORTCUTS_OPENED, TIER_SELECTED,
        UI_SCALE_CHANGED,
    },
};

/// The event the command palette reports when it opens. It is older than the viewing events and
/// is named by its own string in `catalog::EVENTS`.
const PALETTE_OPENED: &str = "palette_opened";

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![
        interface_mode(),
        high_contrast(),
        interface_scale(),
        larger_handles(),
        command_palette(),
        keyboard(),
        help_viewer(),
        glossary(),
    ]
}

fn interface_mode() -> Guide {
    Guide::new(
        "prefs-interface-mode",
        "Simple or Advanced",
        "Choose how many controls the program shows: the ones most designs need, or all of them.",
        GuideCategory::Preferences,
    )
    .starting(StartingState::CurrentDesign)
    .step(
        GuideStep::new(
            "Two interfaces",
            "The program can show every control, or only the ones most designs need.",
        )
        .actions([
            "Simple shows the controls most designs need. Advanced shows every control.",
            "It is one switch with two places to change it: the Simple | Advanced pill at the top left of the window, and the two choices at the top of the Preferences dialog.",
            "Switching hides or shows controls and nothing else. Your designs, your settings and your files are never affected.",
        ])
        .why("A new installation starts in Simple. A program that was used before the switch existed stays in Advanced.")
        .highlight("mode_switch")
        .allow(ADVANCED),
    )
    .step(
        GuideStep::new(
            "Switch the interface",
            "The pill at the top of the window changes it at once.",
        )
        .actions(["Click the half of the Simple | Advanced pill that is not lit."])
        .check("The other half is lit.")
        .why("While this step is open the program keeps the advanced controls on screen, so you can switch back. Close the tutorial to see the smaller window of Simple.")
        .goal(event(INTERFACE_MODE_CHANGED), "The interface switched")
        .highlight("mode_switch")
        .allow(ADVANCED),
    )
    .step(
        GuideStep::new(
            "Switch back",
            "Change it again whenever you like.",
        )
        .actions(["Click the other half of the pill."])
        .check("The half you started with is lit again.")
        .goal(event(INTERFACE_MODE_CHANGED), "The interface switched back")
        .highlight("mode_switch")
        .allow(ADVANCED),
    )
    .step(
        GuideStep::new(
            "What Simple hides",
            "A hidden setting keeps its value and still applies.",
        )
        .actions([
            "Simple hides the Tilt Curve and Save View Preset buttons, and the advanced rows in Settings: preview image size and samples, motion preview resolution, live compute and transfer, local compute, max ray bounces and the Tilt Performance Curve panel.",
            "When one of those settings is not at its default, Settings says Some advanced settings are in use.",
            "The command palette has Use the Simple Interface and Use the Advanced Interface as well.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn high_contrast() -> Guide {
    Guide::new(
        "prefs-high-contrast",
        "High contrast",
        "Switch to near-black backgrounds, white text and bright accents for easier reading.",
        GuideCategory::Preferences,
    )
    .starting(StartingState::CurrentDesign)
    .step(
        GuideStep::new(
            "What High contrast changes",
            "It is for bright rooms and for low vision.",
        )
        .actions([
            "Backgrounds turn near-black, text white, borders clear and accents bright.",
            "It takes effect at once, in the main window and in the Compare and Rough Planner windows, including ones you open later.",
            "A few colours keep their value because they mean something: the tilt graph's curve colours, colour swatches and the rendered pictures themselves.",
        ])
        .allow(ADVANCED),
    )
    .step(
        GuideStep::new(
            "Switch it on",
            "The switch is in the Preferences dialog, which covers this panel while it is open.",
        )
        .actions([
            "Choose Edit, then Preferences... (or press Ctrl+,).",
            "In Appearance, switch High contrast on.",
            "Switch it off again if you only wanted to look, then click Done.",
        ])
        .check("The colours of the window change as you flip the switch.")
        .why("The choice is saved at once and comes back the next time you start the program.")
        .goal(event(HIGH_CONTRAST_CHANGED), "High contrast switched")
        .allow(ADVANCED),
    )
    .step(
        GuideStep::new(
            "Done",
            "You can change it again in the same place at any time.",
        )
        .why("Every control is unlocked again."),
    )
}

fn interface_scale() -> Guide {
    Guide::new(
        "prefs-interface-scale",
        "Interface scale",
        "Make everything in the windows larger or smaller.",
        GuideCategory::Preferences,
    )
    .starting(StartingState::CurrentDesign)
    .step(
        GuideStep::new(
            "How the scale works",
            "Interface scale makes everything in the windows larger or smaller.",
        )
        .actions([
            "The choices are Automatic, which follows your system's display scaling, and 75, 90, 100, 110, 125, 150, 175 and 200 percent.",
            "The scale is applied when the program starts, so a change takes effect after a restart.",
            "Picking and dragging in the viewport use the same scale, so a scaled window still hits what it shows.",
        ])
        .why("If you have set the SLINT_SCALE_FACTOR environment variable yourself, your value is used and this choice is ignored.")
        .allow(ADVANCED),
    )
    .step(
        GuideStep::new(
            "Choose a scale",
            "The setting is in the Preferences dialog, which covers this panel while it is open.",
        )
        .actions([
            "Choose Edit, then Preferences... (or press Ctrl+,).",
            "In Appearance, open Interface scale and choose a value.",
            "Choose Automatic again if you do not want the new size at the next start, then click Done.",
        ])
        .check("A note under the drop-down says it takes effect after a restart.")
        .goal(event(UI_SCALE_CHANGED), "A scale chosen")
        .allow(ADVANCED),
    )
    .step(
        GuideStep::new(
            "Done",
            "Close and reopen the program to see a new scale.",
        )
        .why("Every control is unlocked again."),
    )
}

fn larger_handles() -> Guide {
    Guide::new(
        "prefs-larger-handles",
        "Larger handles",
        "Make the drag handles on the stone bigger, for touch screens and pens.",
        GuideCategory::Preferences,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Handles for fingers",
            "A fingertip is far less precise than a mouse.",
        )
        .actions([
            "Larger handles makes the angle, depth and index handles about 60 percent bigger, and the area you can grab around each one just as much bigger.",
            "A touch screen has no hover, so with larger handles on, the line under the toolbar explains the three handles as soon as you select a facet.",
        ])
        .allow(ADVANCED),
    )
    .step(
        GuideStep::new(
            "Switch it on",
            "The switch is in the Preferences dialog, which covers this panel while it is open.",
        )
        .actions([
            "Choose Edit, then Preferences... (or press Ctrl+,).",
            "In Direct manipulation, switch Larger handles on, then click Done.",
        ])
        .check("The switch is on.")
        .goal(event(LARGE_HANDLES_CHANGED), "Larger handles switched")
        .allow(ADVANCED),
    )
    .step(
        GuideStep::new(
            "Select a facet",
            "The bigger handles show on the selected facet.",
        )
        .actions([
            "Click the Edit tab, then a facet of the stone in the Solid view.",
        ])
        .check("The handles A, D and I are bigger than before, and the line under the toolbar names what each one does.")
        .why("Switch Larger handles off in the same place if you prefer the small ones. Snap to gear steps when dragging is in the same section; it is the Snap pill of the toolbar.")
        .goal(event(TIER_SELECTED), "A facet selected")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Done",
            "The choice is saved and comes back the next time you start the program.",
        )
        .why("Every control is unlocked again."),
    )
}

fn command_palette() -> Guide {
    Guide::new(
        "app-command-palette",
        "The command palette",
        "Find and run any action by typing a few letters of its name.",
        GuideCategory::GettingStarted,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "A search box for every action",
            "The palette lists every action of the program in one small window.",
        )
        .actions([
            "Press Ctrl+K (or Ctrl+Shift+P, or choose Edit, then Command Palette...) to open it. The cursor is already in its search box.",
            "Type a few letters of the action's name. The letters only have to appear in order, so sv finds Save and dsolve finds Deep Solve.",
            "Up and Down move the highlight, Enter runs the command and Esc closes the palette.",
        ])
        .why("Before you type anything, the palette shows the commands you ran lately first.")
        .allow(VIEWPORT_TABS),
    )
    .step(
        GuideStep::new(
            "Run a command",
            "The palette covers this panel while it is open, so this step carries everything you need.",
        )
        .actions([
            "Click the Edit tab above the picture, if it is not open.",
            "Press Ctrl+K and type diag.",
            "Check that the highlighted row reads Diagram View, then press Enter.",
        ])
        .check("The Diagram view shows three flat panels, and the palette has closed.")
        .why("The palette follows the same rules as the buttons and menus: it never lets you do what the matching button would refuse.")
        .goal(
            Goal::All(vec![event(PALETTE_OPENED), Goal::ViewMode(3)]),
            "The Diagram view chosen from the palette",
        )
        .allow(VIEWPORT_TABS),
    )
    .step(
        GuideStep::new(
            "Dim rows explain themselves",
            "A command that cannot run right now is dimmed, with the reason under its name.",
        )
        .actions([
            "Open the palette and type slice. While the Diagram view shows, Toggle Slice Mode is dim and says Not available in the Diagram view.",
            "Choosing a dim row does not close the palette. It shows a short message instead, so you learn why.",
            "Close the palette with Esc, and press 1 to go back to the Solid view.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn keyboard() -> Guide {
    Guide::new(
        "app-keyboard-shortcuts",
        "Keyboard shortcuts",
        "Switch views, tabs and tools with keys, and find every shortcut in one list.",
        GuideCategory::GettingStarted,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Keys you will use",
            "You never need to learn these, because the command palette finds everything. These are the ones people use all day.",
        )
        .actions([
            "File: Ctrl+N new, Ctrl+O open, Ctrl+S save, Ctrl+Shift+S save as.",
            "History: Ctrl+Z undo, Ctrl+Y redo. Solving: F5.",
            "Tabs: Ctrl+1, Ctrl+2 and Ctrl+3 switch the three top tabs, and Ctrl+E toggles Live Render and Edit.",
            "Solid viewport: 1, 2, 3 and 4 switch the view mode, and S toggles Slice.",
            "Everywhere: Ctrl+K opens the palette, Ctrl+, Preferences, F1 help for the screen, Esc backs out, and ? shows every shortcut.",
        ])
        .why("A key that types a character does nothing while you are typing in a text box, so a comma in a name stays a comma.")
        .allow(VIEWPORT_TABS),
    )
    .step(
        GuideStep::new(
            "Switch the view with a key",
            "The number keys choose the Solid viewport's view mode.",
        )
        .actions([
            "Click the Edit tab above the picture, then click an empty part of the window so that no text box has the keyboard.",
            "Press 4 for the Diagram view.",
        ])
        .check("The Diagram view shows three flat panels.")
        .goal(Goal::ViewMode(3), "The Diagram view")
        .highlight("view_modes")
        .allow(VIEWPORT_TABS),
    )
    .step(
        GuideStep::new(
            "And back",
            "The keys 1 to 4 are Solid, Path-traced, Both and Diagram.",
        )
        .actions(["Press 1 for the Solid view."])
        .check("The Solid view shows the stone.")
        .goal(Goal::ViewMode(0), "The Solid view")
        .highlight("view_modes")
        .allow(VIEWPORT_TABS),
    )
    .step(
        GuideStep::new(
            "Swap tabs",
            "Ctrl+E flips between Live Render and Edit.",
        )
        .actions([
            "Press Ctrl+E.",
            "Press Ctrl+E again to come back to the Edit tab.",
        ])
        .check("Live Render shows the traced picture, and the second press brings back the Edit tab.")
        .goal(event(LIVE_RENDER_OPENED), "The Live Render view")
        .highlight("live_render_tab")
        .allow(VIEWPORT_TABS),
    )
    .step(
        GuideStep::new(
            "Every key in one list",
            "The shortcuts window lists every key, grouped by where it works.",
        )
        .actions([
            "Click an empty part of the window, then press ? (or choose Help, then Keyboard Shortcuts).",
            "Press Esc to close the list.",
        ])
        .check("A list of shortcuts opens over the window.")
        .goal(event(SHORTCUTS_OPENED), "The shortcuts list opened")
        .allow(VIEWPORT_TABS),
    )
    .step(
        GuideStep::new(
            "Done",
            "The palette's rows show the key of each command, so it teaches you the shortcuts as you use it.",
        )
        .why("Every control is unlocked again."),
    )
}

fn help_viewer() -> Guide {
    Guide::new(
        "app-help-viewer",
        "The manual and context help",
        "Read the manual inside the program, and open the page for what is on screen with F1.",
        GuideCategory::GettingStarted,
    )
    .starting(StartingState::CurrentDesign)
    .step(
        GuideStep::new(
            "The manual is in the program",
            "Every chapter of the manual is built into the program, so Help works without any file beside it.",
        )
        .actions([
            "The help window has the chapter list on the left, with the sections of the open chapter under it, and a search box at the top.",
            "Back and Forward walk your trail, like a browser.",
            "A link to another chapter opens in the same window.",
        ])
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Open the manual",
            "The manual opens in a window of its own, so this panel stays where it is.",
        )
        .actions(["Choose Help, then User Manual in the menu bar."])
        .check("A window titled Help opens on the contents page.")
        .goal(event(HELP_OPENED), "The manual opened")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Help for this screen",
            "F1 opens the page for what you are looking at.",
        )
        .actions([
            "Click the main window, then press F1 (or choose Help, then Help for This Screen).",
            "Click the Cutting Instructions tab and press F1 again to see the page change.",
        ])
        .check("The manual jumps to the page for the tab or panel on screen.")
        .why("The round question mark on a panel opens the page for that panel in the same way. Anything the program does not know opens the contents.")
        .goal(event(HELP_OPENED), "A manual page opened")
        .allow(VIEWPORT_TABS),
    )
    .step(
        GuideStep::new(
            "Search the manual",
            "The search box finds sections, not only chapters.",
        )
        .actions([
            "In the help window, click Search the manual (or press Ctrl+F in it) and type a word, for example mast.",
            "Click a result to open that section.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn glossary() -> Guide {
    Guide::new(
        "app-glossary",
        "The glossary",
        "Look up the words of cutting and optics that the program and the manual use.",
        GuideCategory::GettingStarted,
    )
    .starting(StartingState::CurrentDesign)
    .step(
        GuideStep::new(
            "Words of the craft",
            "Mast, meet, index, cheater, critical angle: the program uses words a new cutter may not know.",
        )
        .actions([
            "The glossary explains the words the manual and the program use for cutting and for optics.",
            "Many hover notes end with See the glossary.",
        ])
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Open the glossary",
            "The glossary opens over the window, so this step carries everything you need.",
        )
        .actions([
            "Choose Help, then Glossary in the menu bar.",
            "Click Search the terms and type a word, for example mast.",
            "Press Esc, or click the close button, to come back to this panel.",
        ])
        .check("The list narrows to the terms that match what you typed.")
        .goal(event(GLOSSARY_OPENED), "The glossary opened")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Done",
            "The glossary is in the manual too, as its first appendix.",
        )
        .why("Every control is unlocked again."),
    )
}
