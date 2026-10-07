//! Guards the on-screen hooks of the viewing, output, library and app tutorials
//! (`indicatrix_editor::guide::tutorials::viewing`):
//!
//! - every outline target those lessons name is drawn by some `.slint` component, which
//!   compares `GuideModel.highlight_target` with it. A target nobody draws would only mean "no
//!   outline", silently;
//! - every UI event those lessons wait for is raised by the desktop: a Rust call (the constant
//!   from `viewing_events` handed to `gui::tutorial_events::raise`) or a Slint call of
//!   `GuideModel.event("name")`. An event nobody raises is a step that never completes.
//!
//! It only reads the sources as text, like `theme_tokens.rs`.

use indicatrix_editor::guide::{EVENTS, HIGHLIGHT_TARGETS, viewing_events};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The outline targets the viewing, output, library and app lessons use (see
/// `guide/tutorials/viewing/`). The older targets (`tier_table`, `design_settings`,
/// `inspector_tier`, ...) belong to the lessons that introduced them.
const VIEWING_TARGETS: &[&str] = &[
    "solid_viewport",
    "view_modes",
    "cut_slider",
    "snap_pill",
    "slice_pill",
    "live_render_tab",
    "lighting_combo",
    "cutting_mode_button",
    "export_asc_button",
    "export_menu",
    "save_button",
    "open_button",
    "load_button",
    "library_search",
    "library_filters",
    "library_import",
    "mode_switch",
];

/// The three events one Rust function reports for the Cut slider's position: the desktop never
/// names their constants, it calls `cut_slider_event` with the slider's value.
const CUT_SLIDER_EVENTS: &[&str] = &[
    viewing_events::CUT_SLIDER_ROUGH,
    viewing_events::CUT_SLIDER_STEP,
    viewing_events::CUT_SLIDER_FINISHED,
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
/// `GuideModel.event` with it, or it is one of the Cut slider events and Rust calls
/// `cut_slider_event`.
fn is_raised(name: &str, rust: &[String], slint: &[String]) -> bool {
    let constant = constant_for(name);
    let in_rust = rust.iter().any(|source| source.contains(&constant));
    let in_slint = slint
        .iter()
        .any(|source| source.contains(&format!("event(\"{name}\")")));
    let by_cut_slider = CUT_SLIDER_EVENTS.contains(&name)
        && rust
            .iter()
            .any(|source| source.contains("cut_slider_event("));
    in_rust || in_slint || by_cut_slider
}

#[test]
fn every_outline_target_of_the_viewing_lessons_is_registered_and_drawn() {
    let slint = sources("ui", "slint");
    for target in VIEWING_TARGETS {
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
fn every_event_of_the_viewing_lessons_is_registered_and_raised() {
    let rust = sources("src", "rs");
    let slint = sources("ui", "slint");
    for name in viewing_events::ALL {
        assert!(
            EVENTS.contains(name),
            "{name} is missing from catalog::EVENTS"
        );
        assert!(
            is_raised(name, &rust, &slint),
            "nothing raises {name}: call `tutorial_events::raise` with `viewing_events::{}`",
            constant_for(name)
        );
    }
}

#[test]
fn the_helpers_find_what_they_look_for() {
    let slint = [
        "if GuideModel.highlight_target == \"save_button\": Rectangle { }".to_string(),
        "GuideModel.event(\"shortcuts_opened\");".to_string(),
    ];
    assert!(is_drawn("save_button", &slint));
    assert!(!is_drawn("open_button", &slint));

    let rust = ["raise(&ui, events::SNAP_TOGGLED);".to_string()];
    assert!(is_raised("snap_toggled", &rust, &slint));
    assert!(is_raised("shortcuts_opened", &rust, &slint));
    assert!(!is_raised("slice_started", &rust, &slint));
    assert!(!is_raised("cut_slider_rough", &rust, &slint));

    let cut_slider = ["raise(&ui, cut_slider_event(count));".to_string()];
    assert!(is_raised("cut_slider_rough", &cut_slider, &slint));
    assert!(is_raised("cut_slider_finished", &cut_slider, &slint));
    assert!(!is_raised("slice_started", &cut_slider, &slint));
    assert_eq!(constant_for("tier_selected"), "TIER_SELECTED");
}
