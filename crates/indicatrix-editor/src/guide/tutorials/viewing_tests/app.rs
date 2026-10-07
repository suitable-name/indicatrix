//! Walks of the lessons in `viewing/app.rs`.

use super::{Sim, act, event, guide_named, read, viewing_events as events, walk};

/// What the command palette reports when it opens.
const PALETTE_OPENED: &str = "palette_opened";

#[test]
fn the_interface_switch_is_used_twice() {
    walk(
        "prefs-interface-mode",
        vec![
            read(),
            event(events::INTERFACE_MODE_CHANGED),
            event(events::INTERFACE_MODE_CHANGED),
            read(),
        ],
    );
}

#[test]
fn each_preference_waits_for_its_own_switch() {
    walk(
        "prefs-high-contrast",
        vec![read(), event(events::HIGH_CONTRAST_CHANGED), read()],
    );
    walk(
        "prefs-interface-scale",
        vec![read(), event(events::UI_SCALE_CHANGED), read()],
    );
    walk(
        "prefs-larger-handles",
        vec![
            read(),
            event(events::LARGE_HANDLES_CHANGED),
            event(events::TIER_SELECTED),
            read(),
        ],
    );
}

#[test]
fn a_preference_is_not_met_by_another_preference() {
    let lessons = [
        ("prefs-high-contrast", events::HIGH_CONTRAST_CHANGED),
        ("prefs-interface-scale", events::UI_SCALE_CHANGED),
        ("prefs-larger-handles", events::LARGE_HANDLES_CHANGED),
    ];
    for (id, own) in lessons {
        let guide = guide_named(id);
        for (_, other) in lessons.iter().filter(|(_, other)| *other != own) {
            let mut sim = Sim::start(&guide.starting_state);
            sim.fire(other);
            assert!(
                !sim.met(&guide.steps[1].goal),
                "{id} must not be finished by {other}"
            );
        }
    }
}

#[test]
fn the_palette_lesson_needs_the_palette_and_the_result() {
    walk(
        "app-command-palette",
        vec![
            read(),
            act(|sim| {
                sim.fire(PALETTE_OPENED);
                sim.show(3);
            }),
            read(),
        ],
    );
}

#[test]
fn the_palette_step_is_not_met_by_either_half_alone() {
    let guide = guide_named("app-command-palette");
    let step = &guide.steps[1];

    // The key 4 reaches the Diagram view without the palette.
    let mut by_key = Sim::start(&guide.starting_state);
    by_key.show(3);
    assert!(!by_key.met(&step.goal), "the key 4 is not the palette");

    // The palette opened and closed again without running the command.
    let mut opened_only = Sim::start(&guide.starting_state);
    opened_only.fire(PALETTE_OPENED);
    assert!(
        !opened_only.met(&step.goal),
        "opening the palette is not running Diagram View"
    );
}

#[test]
fn the_keyboard_lesson_switches_the_view_the_tab_and_opens_the_list() {
    walk(
        "app-keyboard-shortcuts",
        vec![
            read(),
            act(|sim| sim.show(3)),
            act(|sim| sim.show(0)),
            event(events::LIVE_RENDER_OPENED),
            event(events::SHORTCUTS_OPENED),
            read(),
        ],
    );
}

#[test]
fn the_help_lesson_opens_the_manual_and_a_context_page() {
    walk(
        "app-help-viewer",
        vec![
            read(),
            event(events::HELP_OPENED),
            event(events::HELP_OPENED),
            read(),
        ],
    );
}

#[test]
fn the_glossary_lesson_opens_the_glossary() {
    walk(
        "app-glossary",
        vec![read(), event(events::GLOSSARY_OPENED), read()],
    );
}

#[test]
fn the_help_and_glossary_events_are_different() {
    let guide = guide_named("app-glossary");
    let mut sim = Sim::start(&guide.starting_state);
    sim.fire(events::HELP_OPENED);
    assert!(
        !sim.met(&guide.steps[1].goal),
        "the manual is not the glossary"
    );
}
