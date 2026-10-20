//! Unit tests of [`crate::pure_tables`]. They live in their own file, included only by
//! `tests/tutorial_videos_targets.rs`: the video harness (`main.rs`) has `harness = false`, so a
//! `#[test]` there never runs, and a test module inside the tables file would only be compiled
//! (with its imports and `SCRIPT_COMMANDS` unused) into the harness.

use crate::pure_tables::{
    PALETTE_COMMANDS, ROUTED_EVENTS, SCRIPTED, check_guide, check_guide_with, command_is_routed,
    output_path, recipe_gap, scripted_keys,
};
use indicatrix_editor::guide::{
    CMD_ADOPT_ALL, CMD_SOLVE, CMD_UNDO, EVENTS, Goal, GuideCatalog, PERFORM_COMMANDS,
};
use std::path::Component;

/// The command ids the scripts (`scripts.rs`, `scripts_actions.rs`, `optical_routes.rs`,
/// `event_extras.rs`, `routes.rs`) pass to `film_command`.
const SCRIPT_COMMANDS: &[&str] = &[
    CMD_SOLVE,
    CMD_UNDO,
    CMD_ADOPT_ALL,
    "solve.snapshot",
    "view.live_render",
    "view.snap",
    "view.slice",
    "help.tutorials",
    "help.manual",
    "help.shortcuts",
    "tools.plan_rough",
    "tools.export_render",
    "tools.angle_sweep",
];

#[test]
fn every_command_a_recipe_or_script_runs_has_a_route() {
    for id in PERFORM_COMMANDS.iter().chain(SCRIPT_COMMANDS) {
        assert!(
            command_is_routed(id),
            "film_command has no route for {id:?}"
        );
    }
    assert!(command_is_routed("tools.export_render"));
    assert!(command_is_routed("tools.angle_sweep"));
    assert!(!command_is_routed("tools.nothing"));
}

#[test]
fn every_palette_title_is_a_palette_command_with_a_unique_id() {
    for (index, (id, title)) in PALETTE_COMMANDS.iter().enumerate() {
        assert!(!title.is_empty(), "{id}");
        assert!(
            PALETTE_COMMANDS
                .iter()
                .skip(index + 1)
                .all(|(other, _)| other != id),
            "{id} listed twice"
        );
    }
}

#[test]
fn every_guide_passes_the_table_checks() {
    for guide in GuideCatalog::new().all() {
        if let Err(reason) = check_guide(guide) {
            panic!("{reason}");
        }
    }
}

#[test]
fn every_scripted_guide_exists() {
    let catalog = GuideCatalog::new();
    for (id, _, _) in SCRIPTED {
        assert!(catalog.get(id).is_some(), "no guide {id:?}");
    }
}

#[test]
fn every_routed_event_is_a_catalogue_event_and_appears_in_the_route_sources() {
    let routes = include_str!("routes.rs");
    let extras = include_str!("event_extras.rs");
    for name in ROUTED_EVENTS {
        assert!(EVENTS.contains(name), "{name:?} is not in guide::EVENTS");
        let quoted = format!("\"{name}\"");
        assert!(
            routes.contains(&quoted) || extras.contains(&quoted),
            "{name:?} has no arm in routes.rs or event_extras.rs"
        );
    }
}

#[test]
fn a_renamed_step_or_a_missing_arm_is_reported() {
    let catalog = GuideCatalog::new();
    let guide = catalog.get("viewing-slice").expect("the slice lesson");
    assert!(check_guide_with(guide, include_str!("scripts.rs")).is_ok());
    let without_arm =
        include_str!("scripts.rs").replace("(\"viewing-slice\", 3)", "(\"viewing-slice\", 33)");
    assert!(check_guide_with(guide, &without_arm).is_err());
    let mut renamed = guide.clone();
    renamed.steps[3].title = "Cut it deeper".into();
    assert!(check_guide(&renamed).unwrap_err().contains("Cut it deeper"));
}

#[test]
fn the_script_key_scan_reads_match_arms_only() {
    let source = "    // (\"a\", 1) => no\n            (\"guide-a\", 2) => self.x()?,\n            (\"b\", 3)) =>\n            (\"From (degrees)\", \"38\"),\n";
    assert_eq!(scripted_keys(source), vec![("guide-a".to_owned(), 2)]);
}

#[test]
fn output_folders_are_per_guide_and_relative() {
    let first = output_path("guide-a", "edited.asc");
    let second = output_path("guide-b", "edited.asc");
    assert_ne!(first, second);
    for path in [first, second] {
        assert!(path.is_relative());
        assert!(
            path.components()
                .all(|part| matches!(part, Component::Normal(_))),
            "{path:?}"
        );
    }
}

#[test]
fn goals_without_a_recipe_are_judged_by_their_route() {
    assert!(recipe_gap(&Goal::Manual).is_none());
    assert!(recipe_gap(&Goal::SolvedClosed).is_some());
    assert!(recipe_gap(&Goal::YieldApplied).is_some());
    assert!(recipe_gap(&Goal::Event("palette_opened".into())).is_some());
    assert!(recipe_gap(&Goal::Event("snap_toggled".into())).is_none());
    let all = Goal::All(vec![Goal::Event("snap_toggled".into()), Goal::ViewMode(3)]);
    assert!(recipe_gap(&all).is_none());
    let with_yield = Goal::All(vec![Goal::ViewMode(3), Goal::YieldApplied]);
    assert!(recipe_gap(&with_yield).is_some());
}
