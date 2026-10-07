use super::*;
use crate::gui::commands::{
    rules::LOCKED_BY_GUIDE,
    state::{CommandState, Fact},
};
use std::collections::HashSet;

#[test]
fn command_ids_are_unique_and_well_formed() {
    let mut seen = HashSet::new();
    for command in COMMANDS {
        assert!(seen.insert(command.id), "duplicate id {}", command.id);
        let (area, name) = command
            .id
            .split_once('.')
            .unwrap_or_else(|| panic!("id {} has no dot", command.id));
        assert!(!area.is_empty() && !name.is_empty(), "{}", command.id);
        assert!(
            command
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_'),
            "id {} should be lower case letters, digits, dots and underscores",
            command.id
        );
    }
}

#[test]
fn command_titles_are_unique_and_not_empty() {
    let mut seen = HashSet::new();
    for command in COMMANDS {
        assert!(!command.title.trim().is_empty(), "{}", command.id);
        assert!(
            seen.insert(command.title),
            "two commands are both called {:?}",
            command.title
        );
    }
}

#[test]
fn every_command_has_search_keywords_and_a_plain_title() {
    for command in COMMANDS {
        assert!(!command.keywords.trim().is_empty(), "{}", command.id);
        // The UI font has no glyphs for symbols beyond plain punctuation.
        assert!(
            command.title.is_ascii(),
            "{} has a non-ASCII character in its title",
            command.id
        );
    }
}

#[test]
fn a_ready_window_can_run_every_command_except_the_interface_it_is_already_in() {
    let ready = CommandState::ready();
    let unavailable: Vec<&str> = COMMANDS
        .iter()
        .filter(|command| command.availability(&ready).is_err())
        .map(|command| command.id)
        .collect();
    // `ready()` is the Advanced interface.
    assert_eq!(unavailable, vec!["edit.advanced_interface"]);
}

#[test]
fn the_other_interface_command_is_available_in_simple_mode() {
    let simple = CommandState::ready().with(Fact::SimpleInterface, true);
    let find = |id: &str| COMMANDS.iter().find(|c| c.id == id).unwrap();
    assert_eq!(
        find("edit.advanced_interface").availability(&simple),
        Ok(())
    );
    assert_eq!(
        find("edit.simple_interface").availability(&simple),
        Err("Already using the Simple interface")
    );
}

#[test]
fn the_schedule_tab_command_says_to_switch_when_the_simple_interface_is_on() {
    let schedule = COMMANDS
        .iter()
        .find(|c| c.id == "view.inspector_schedule")
        .unwrap();
    let simple = CommandState::ready().with(Fact::SimpleInterface, true);
    assert_eq!(
        schedule.availability(&simple),
        Err("Switch to the Advanced interface first")
    );
    assert_eq!(schedule.availability(&CommandState::ready()), Ok(()));
    // The other inspector tabs exist in both interfaces.
    for id in [
        "view.inspector_tier",
        "view.inspector_preform",
        "view.inspector_history",
    ] {
        let tab = COMMANDS.iter().find(|c| c.id == id).unwrap();
        assert_eq!(tab.availability(&simple), Ok(()), "{id}");
    }
}

#[test]
fn a_command_whose_controls_the_guide_locks_is_disabled_with_that_reason() {
    let mut checked = 0;
    for command in COMMANDS {
        let Some(Check::Guide(group)) = command
            .requires
            .iter()
            .copied()
            .find(|check| matches!(check, Check::Guide(_)))
        else {
            continue;
        };
        // The lock is the first requirement, so it is the reason shown.
        assert_eq!(
            command.requires.first(),
            Some(&Check::Guide(group)),
            "{} must list the guide's lock first",
            command.id
        );
        let locked = CommandState::ready().with(Fact::Allows(group), false);
        assert_eq!(
            command.availability(&locked),
            Err(LOCKED_BY_GUIDE),
            "{}",
            command.id
        );
        checked += 1;
    }
    assert!(checked > 30, "only {checked} guide-locked commands");
}

#[test]
fn the_guide_locks_match_the_buttons_they_mirror() {
    use crate::gui::commands::state::GuideGroup as G;
    let group_of = |id: &str| {
        let command = COMMANDS.iter().find(|c| c.id == id).unwrap();
        command.requires.iter().find_map(|check| match check {
            Check::Guide(group) => Some(*group),
            _ => None,
        })
    };
    assert_eq!(group_of("file.new"), Some(G::NewDesign));
    assert_eq!(group_of("file.save"), Some(G::FileOps));
    assert_eq!(group_of("file.export_cutting_sheet"), Some(G::FileOps));
    assert_eq!(group_of("edit.undo"), Some(G::History));
    assert_eq!(group_of("edit.preferences"), Some(G::Advanced));
    assert_eq!(group_of("tiers.add"), Some(G::TierForm));
    assert_eq!(group_of("tiers.add_concave"), Some(G::ConcaveTier));
    assert_eq!(group_of("tiers.delete"), Some(G::TierTable));
    assert_eq!(group_of("solve.run"), Some(G::Solve));
    assert_eq!(group_of("solve.deep"), Some(G::Advanced));
    assert_eq!(group_of("solve.optimize"), Some(G::Advanced));
    assert_eq!(group_of("solve.retarget"), Some(G::Advanced));
    assert_eq!(group_of("view.tab_cutting"), Some(G::ViewTabs));
    assert_eq!(group_of("view.inspector_preform"), Some(G::PreformTab));
    // The Edit pill and the 3D tab never lock, so the guide cannot strand anyone.
    assert_eq!(group_of("view.edit"), None);
    assert_eq!(group_of("view.tab_3d"), None);
}

#[test]
fn commands_that_need_a_design_or_a_selection_say_so() {
    let find = |id: &str| COMMANDS.iter().find(|c| c.id == id).unwrap();
    let no_design = CommandState::ready().with(Fact::Design, false);
    assert_eq!(
        find("file.save").availability(&no_design),
        Err("Open or create a design first")
    );
    assert_eq!(
        find("solve.run").availability(&no_design),
        Err("Open or create a design first")
    );
    let no_selection = CommandState::ready().with(Fact::TierSelected, false);
    assert_eq!(
        find("tiers.delete").availability(&no_selection),
        Err("Select a tier first")
    );
    assert_eq!(find("tiers.add").availability(&no_selection), Ok(()));
}

#[test]
fn solve_commands_wait_for_a_running_solve() {
    let busy = CommandState::ready().with(Fact::Busy, true);
    for id in ["solve.run", "solve.deep", "solve.optimize"] {
        let command = COMMANDS.iter().find(|c| c.id == id).unwrap();
        assert_eq!(
            command.availability(&busy),
            Err("Wait for the running solve to finish"),
            "{id}"
        );
    }
    // Cancelling is exactly what a running solve allows.
    let cancel = COMMANDS.iter().find(|c| c.id == "solve.abandon").unwrap();
    assert_eq!(cancel.availability(&busy), Ok(()));
    let idle = CommandState::ready().with(Fact::SolveRunning, false);
    assert_eq!(cancel.availability(&idle), Err("No solve is running"));
}

#[test]
fn undo_and_redo_follow_the_history() {
    let find = |id: &str| COMMANDS.iter().find(|c| c.id == id).unwrap();
    let fresh = CommandState::ready()
        .with(Fact::CanUndo, false)
        .with(Fact::CanRedo, false);
    assert_eq!(
        find("edit.undo").availability(&fresh),
        Err("Nothing to undo")
    );
    assert_eq!(
        find("edit.redo").availability(&fresh),
        Err("Nothing to redo")
    );
}

#[test]
fn the_viewport_tools_are_unavailable_in_the_diagram_view() {
    let diagram = CommandState::ready().with(Fact::DiagramView, true);
    let slice = COMMANDS.iter().find(|c| c.id == "view.slice").unwrap();
    assert_eq!(
        slice.availability(&diagram),
        Err("Not available in the Diagram view")
    );
}

#[test]
fn building_a_library_lesson_needs_a_selected_library_design() {
    let build = COMMANDS
        .iter()
        .find(|c| c.id == "help.build_library_design")
        .unwrap();
    assert_eq!(build.availability(&CommandState::ready()), Ok(()));
    let nothing_selected = CommandState::ready().with(Fact::LibrarySelected, false);
    assert_eq!(
        build.availability(&nothing_selected),
        Err("Select a design in the library first")
    );
    // Starting a lesson never needs an open design, so a window without one can still
    // build one.
    let no_design = CommandState::ready().with(Fact::Design, false);
    assert_eq!(build.availability(&no_design), Ok(()));
}
