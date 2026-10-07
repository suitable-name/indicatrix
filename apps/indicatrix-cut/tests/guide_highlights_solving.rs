//! Guards the on-screen hooks of the solving, optimizing and comparing tutorials
//! (`indicatrix_editor::guide::tutorials::solving`):
//!
//! - every outline target those lessons name is drawn by some `.slint` component, which
//!   compares `GuideModel.highlight_target` with it. A target nobody draws would only mean "no
//!   outline", silently;
//! - every UI event those lessons wait for is raised by the desktop: a Rust call (the constant
//!   from `solving_events` handed to `gui::tutorial_events::raise`) or a Slint call of
//!   `GuideModel.event("name")`. An event nobody raises is a step that never completes.
//!
//! It only reads the sources as text, like `guide_highlights.rs` (the same checks for the viewing
//! lessons) and `theme_tokens.rs`.

use indicatrix_editor::guide::{EVENTS, HIGHLIGHT_TARGETS, solving_events};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The outline targets the solving, optimizing and comparing lessons introduced (see
/// `guide/tutorials/solving/`). The older targets (`solve_button`, `tier_table`,
/// `design_settings`, `inspector_tier`, `preform_tab`) belong to the lessons that introduced them.
const SOLVING_TARGETS: &[&str] = &[
    "auto_solve",
    "verdict_badge",
    "deep_solve_button",
    "optimize_tab",
    "retarget_button",
    "history_tab",
    "snapshot_button",
    "compare_button",
];

/// Every file under `dir` with the extension `extension`, appended to `found`.
fn collect(dir: &Path, extension: &str, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, extension, found);
        } else if path.extension().is_some_and(|ext| ext == extension) {
            found.push(path);
        }
    }
}

/// The text of every file with `extension` under `folder` of this crate.
fn sources(folder: &str, extension: &str) -> Vec<String> {
    let mut paths = Vec::new();
    collect(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join(folder),
        extension,
        &mut paths,
    );
    assert!(!paths.is_empty(), "no .{extension} files under {folder}/");
    paths
        .iter()
        .map(|path| {
            fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        })
        .collect()
}

/// Whether some component compares the open step's highlight target with `target`.
fn is_drawn(target: &str, slint: &[String]) -> bool {
    let check = format!("highlight_target == \"{target}\"");
    slint.iter().any(|source| source.contains(&check))
}

/// The constant that `name` is the value of: the event names are `lower_snake_case`, the
/// constants the same words in `UPPER_SNAKE_CASE`.
fn constant_for(name: &str) -> String {
    name.to_ascii_uppercase()
}

/// Whether `name` is raised: Rust code uses its constant, or a Slint file calls
/// `GuideModel.event` with it.
fn is_raised(name: &str, rust: &[String], slint: &[String]) -> bool {
    let constant = constant_for(name);
    let in_rust = rust.iter().any(|source| source.contains(&constant));
    let in_slint = slint
        .iter()
        .any(|source| source.contains(&format!("event(\"{name}\")")));
    in_rust || in_slint
}

#[test]
fn every_outline_target_of_the_solving_lessons_is_registered_and_drawn() {
    let slint = sources("ui", "slint");
    for target in SOLVING_TARGETS {
        assert!(
            HIGHLIGHT_TARGETS.contains(target),
            "{target} is missing from catalog::HIGHLIGHT_TARGETS"
        );
        assert!(
            is_drawn(target, &slint),
            "no component draws an outline for {target}: add `GuideModel.highlight_target == \"{target}\"`"
        );
    }
}

#[test]
fn every_event_of_the_solving_lessons_is_registered_and_raised() {
    let rust = sources("src", "rs");
    let slint = sources("ui", "slint");
    for name in solving_events::ALL {
        assert!(
            EVENTS.contains(name),
            "{name} is missing from catalog::EVENTS"
        );
        assert!(
            is_raised(name, &rust, &slint),
            "nothing raises {name}: call `tutorial_events::raise` with `solving_events::{}`, or `GuideModel.event(\"{name}\")` in Slint",
            constant_for(name)
        );
    }
}

#[test]
fn the_helpers_find_what_they_look_for() {
    let slint = [
        "if GuideModel.highlight_target == \"auto_solve\": Rectangle { }".to_string(),
        "GuideModel.event(\"verdict_opened\");".to_string(),
    ];
    assert!(is_drawn("auto_solve", &slint));
    assert!(!is_drawn("history_tab", &slint));

    let rust = ["raise(&ui, SOLVE_REQUESTED);".to_string()];
    assert!(is_raised("solve_requested", &rust, &slint));
    assert!(is_raised("verdict_opened", &rust, &slint));
    assert!(!is_raised("snapshot_taken", &rust, &slint));
    assert_eq!(constant_for("compare_opened"), "COMPARE_OPENED");
}
