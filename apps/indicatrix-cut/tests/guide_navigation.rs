//! Guards the navigation of the guide panel (`ui/models/guide.slint`, `ui/components/guide_panel.slint`):
//!
//! - every way into a step (start, Next and the automatic advance, Back) fires `step_entered`,
//!   which is where Rust forgets the events of the step before, holds the original of a "Build
//!   this design" lesson for its compare step, and re-checks the step's goal. Back used to skip
//!   it, so a step the learner went back to showed "Waiting for" although its goal held;
//! - after Back a step whose goal still holds stays on screen (`revisiting`) instead of moving
//!   on again by itself after the short "Done" moment.
//!
//! The Slint tree cannot be run in a test, so this reads the sources as text, like
//! `guide_highlights.rs` and `theme_tokens.rs`.

use std::{fs, path::Path};

/// The text of `relative` (a path under this crate).
fn read(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// The text between the braces that follow the first `header` in `source`.
fn body_of<'a>(source: &'a str, header: &str) -> &'a str {
    let start = source
        .find(header)
        .unwrap_or_else(|| panic!("no `{header}` in the source"));
    let open = start
        + source[start..]
            .find('{')
            .unwrap_or_else(|| panic!("`{header}` has no body"));
    let mut depth = 0_usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("`{header}` is never closed");
}

#[test]
fn every_way_into_a_step_fires_step_entered() {
    let guide = read("ui/models/guide.slint");
    for header in ["begin => {", "next => {", "back => {"] {
        assert!(
            body_of(&guide, header).contains("root.step_entered();"),
            "`{header}` does not call root.step_entered(): the step it lands on would not \
             clear its events, hold the original of a Build lesson or re-check its goal"
        );
    }
}

#[test]
fn back_marks_the_step_as_revisited_and_going_forward_clears_it() {
    let guide = read("ui/models/guide.slint");
    assert!(body_of(&guide, "back => {").contains("root.revisiting = true;"));
    for header in ["begin => {", "next => {", "close => {"] {
        assert!(
            body_of(&guide, header).contains("root.revisiting = false;"),
            "`{header}` leaves the guide marked as revisiting"
        );
    }
}

#[test]
fn a_revisited_step_does_not_advance_by_itself_and_offers_next() {
    let panel = read("ui/components/guide_panel.slint");
    assert!(
        panel.contains("running: GuideModel.step_done && !GuideModel.revisiting;"),
        "the advance timer must stop while the learner re-reads a step"
    );
    assert!(
        panel.contains("enabled: (!GuideModel.step_done || GuideModel.revisiting)"),
        "the Next button must stay usable on a revisited step that is done"
    );
    assert!(panel.contains("text: GuideModel.offers_next ?"));
}

/// Next on an unfinished step does the step where the app can (`perform`), and the button waits
/// while the recipe runs. Only a step the app cannot do keeps the plain "Skip step".
#[test]
fn next_on_an_unfinished_step_performs_it_and_waits_for_the_result() {
    let guide = read("ui/models/guide.slint");
    let press = body_of(&guide, "go_on => {");
    assert!(press.contains("root.perform(root.step_index);"));
    assert!(press.contains("root.perform_busy = true;"));
    assert!(
        press.contains("root.next();"),
        "a step without a recipe still skips"
    );
    for header in ["begin => {", "next => {", "back => {", "close => {"] {
        assert!(
            body_of(&guide, header).contains("root.reset_perform();"),
            "`{header}` leaves a recipe's progress behind for the next step"
        );
    }
    let panel = read("ui/components/guide_panel.slint");
    assert!(panel.contains("clicked => { GuideModel.go_on(); }"));
    assert!(panel.contains("&& !GuideModel.perform_busy;"));
    assert!(
        panel.contains("running: GuideModel.perform_busy;"),
        "the overlay layer ticks while a recipe waits for its goal"
    );
}

#[test]
fn the_body_finder_reads_nested_braces() {
    let source =
        "callback a();\n    a => {\n        if x { y(); }\n        z();\n    }\n    b => { w(); }";
    assert_eq!(
        body_of(source, "a => {").trim(),
        "if x { y(); }\n        z();"
    );
    assert_eq!(body_of(source, "b => {").trim(), "w();");
}
