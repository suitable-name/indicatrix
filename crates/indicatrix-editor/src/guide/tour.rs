//! The welcome tour: a short, static guide of reading steps that points at the main
//! parts of the window. Shown once to a new user and on request afterwards
//! (Preferences > "Show the welcome tour again").

use super::{Guide, GuideCategory, GuideStep, StartingState};

/// The tour's guide id; progress is saved under it.
pub const WELCOME_TOUR_ID: &str = "welcome-tour";

/// The welcome tour. Every step is a reading step (Next), nothing is locked, and no
/// design is needed: it only points.
#[must_use]
pub fn welcome_tour_guide() -> Guide {
    Guide::new(
        WELCOME_TOUR_ID,
        "Welcome tour",
        "A two-minute look at the library, the editor, the viewport and the help.",
        GuideCategory::GettingStarted,
    )
    .starting(StartingState::CurrentDesign)
    .step(
        GuideStep::new(
            "Welcome to Indicatrix Cut",
            "Design a faceted gem, check that it closes, and see how it performs.",
        )
        .actions([
            "Read each step, then click Next.",
            "Close this panel at any time with the cross at its top right.",
        ])
        .why("You can run this tour again from Edit > Preferences..., under Tutorials."),
    )
    .step(
        GuideStep::new(
            "The library",
            "The panel on the left lists the designs you have imported.",
        )
        .actions([
            "Type in the search box at the top of the window to find a design by name.",
            "Click a design in the list to see its picture and numbers.",
            "Click Load Selected on the command bar to open it in the editor.",
        ])
        .why("The Import button in the top bar brings in .asc, .gem and .gcs files."),
    )
    .step(
        GuideStep::new(
            "The command bar",
            "The row of buttons above the viewport holds the actions you use most.",
        )
        .actions([
            "New Design... starts a design. Load Selected opens the library design you picked.",
            "Undo and Redo step through your changes.",
            "Save writes your design to a file. Solve works out the depth of every tier.",
        ])
        .highlight("solve_button"),
    )
    .step(
        GuideStep::new(
            "The tier table",
            "A design is a list of tiers. Each row is one tier of facets.",
        )
        .actions([
            "Click a row to select it.",
            "Read across a row: its name, angle, index positions and what it meets.",
            "Use the buttons above the table to add, copy and delete tiers.",
        ])
        .highlight("tier_table"),
    )
    .step(
        GuideStep::new(
            "The inspector",
            "The panel beside the table edits the tier you selected.",
        )
        .actions([
            "The Tier tab changes one tier's angle, indices and name.",
            "The other tabs hold the rough (Preform), Optimize and the History of your edits.",
            "The Advanced interface adds a Schedule tab with the cutting sheet.",
        ])
        .highlight("inspector_tier"),
    )
    .step(
        GuideStep::new(
            "The viewport",
            "The picture in the middle shows the stone as you build it.",
        )
        .actions([
            "Drag in the viewport to turn the stone.",
            "The buttons along its top switch between Solid, Path-traced, Both and Diagram.",
            "Press 1, 2, 3 or 4 to switch the view from the keyboard.",
        ]),
    )
    .step(
        GuideStep::new(
            "Simple and Advanced",
            "One switch decides how many controls the whole app shows.",
        )
        .actions([
            "Find the Simple | Advanced switch in the top bar, next to the logo.",
            "Simple shows what most designs need. Advanced shows everything.",
        ])
        .why("You can change it at any time. Your designs are not affected."),
    )
    .step(
        GuideStep::new(
            "Help and the command palette",
            "Everything you need to learn more is a click or a keystroke away.",
        )
        .actions([
            "Help > Tutorials... lists short guided lessons, one for each tool.",
            "Help > User Manual opens the full manual inside the app.",
            "Press Ctrl+K to search every command by name and run it.",
        ])
        .why("That is the whole tour. Click Finish, then try a tutorial or start a design."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide::{ALL_GROUPS, Goal, HIGHLIGHT_TARGETS};

    #[test]
    fn the_tour_is_fit_to_run_and_has_only_reading_steps() {
        let tour = welcome_tour_guide();
        assert_eq!(tour.problem(), None);
        assert!(tour.steps.len() >= 6, "a tour covers the main window");
        assert!(tour.steps.iter().all(GuideStep::is_manual));
        assert!(matches!(tour.starting_state, StartingState::CurrentDesign));
        assert!(
            tour.steps
                .iter()
                .all(|step| matches!(step.goal, Goal::Manual)),
            "every step waits for Next"
        );
    }

    #[test]
    fn the_tour_locks_nothing() {
        let tour = welcome_tour_guide();
        for step in &tour.steps {
            for group in ALL_GROUPS {
                assert!(
                    step.allow.contains(group),
                    "{:?} locks {group:?}",
                    step.title
                );
            }
        }
    }

    /// The tour is shown on first run, in the Simple interface, whose inspector has the Tier,
    /// Preform, Optimize and History tabs; the Schedule tab appears only in Advanced.
    #[test]
    fn the_tour_names_the_tabs_the_simple_interface_shows() {
        let tour = welcome_tour_guide();
        let inspector = tour
            .steps
            .iter()
            .find(|step| step.title == "The inspector")
            .expect("the tour has an inspector step");
        let text = inspector.actions.join(" ");
        for tab in ["Tier", "Preform", "Optimize", "History"] {
            assert!(text.contains(tab), "the inspector step omits {tab}: {text}");
        }
        for line in &inspector.actions {
            if line.contains("Schedule") {
                assert!(
                    line.contains("Advanced"),
                    "the Schedule tab is named without saying it is Advanced: {line}"
                );
            }
        }
    }

    #[test]
    fn every_outline_the_tour_asks_for_exists() {
        let tour = welcome_tour_guide();
        for step in &tour.steps {
            assert!(
                HIGHLIGHT_TARGETS.contains(&step.highlight_target.as_str()),
                "{:?}",
                step.title
            );
        }
    }
}
