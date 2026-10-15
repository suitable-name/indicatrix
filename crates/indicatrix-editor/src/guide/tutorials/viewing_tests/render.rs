//! Walks of the lessons in `viewing/render.rs`.

use super::{Sim, event, guide_named, read, viewing_events as events, walk};

#[test]
fn live_render_opens_changes_the_light_and_returns_to_edit() {
    walk(
        "viewing-live-render",
        vec![
            event(events::LIVE_RENDER_OPENED),
            read(),
            event(events::LIGHTING_PRESET_CHOSEN),
            // The Parameters dock, the stone's material, back to the Edit tab.
            read(),
            read(),
            read(),
        ],
    );
}

#[test]
fn the_dock_lessons_outline_and_open_the_tab_they_talk_about() {
    // A step that names a tab of the Parameters dock outlines it (and `app.slint` opens it),
    // and the lessons that send the learner to the dock use only those targets for it.
    let live = guide_named("viewing-live-render");
    let targets: Vec<&str> = live
        .steps
        .iter()
        .map(|step| step.highlight_target.as_str())
        .collect();
    assert!(targets.contains(&"lighting_tab"), "{targets:?}");
    assert!(targets.contains(&"stone_tab"), "{targets:?}");
    let lighting = guide_named("viewing-design-lighting");
    for step in &lighting.steps[2..4] {
        assert_eq!(step.highlight_target, "lighting_tab", "{:?}", step.title);
    }
    for id in ["viewing-custom-material", "viewing-material-coefficients"] {
        let guide = guide_named(id);
        for step in guide
            .steps
            .iter()
            .filter(|step| !step.highlight_target.is_empty())
        {
            assert_eq!(step.highlight_target, "stone_tab", "{id}, {:?}", step.title);
        }
    }
}

#[test]
fn a_design_can_remember_its_lighting_and_forget_it_again() {
    walk(
        "viewing-design-lighting",
        vec![
            event(events::LIVE_RENDER_OPENED),
            event(events::LIGHTING_PRESET_CHOSEN),
            event(events::DESIGN_LIGHTING_SAVED),
            event(events::DESIGN_LIGHTING_FORGOTTEN),
            read(),
        ],
    );
}

#[test]
fn saving_the_lighting_is_not_done_by_choosing_a_rig() {
    // Choosing a rig only changes what is on screen; the step after it asks for the save.
    let guide = guide_named("viewing-design-lighting");
    let mut sim = Sim::start(&guide.starting_state);
    sim.fire(events::LIGHTING_PRESET_CHOSEN);
    assert!(
        !sim.met(&guide.steps[2].goal),
        "the save step waits for the save, not for a rig"
    );
    assert!(
        !sim.met(&guide.steps[3].goal),
        "the forget step waits for the forget"
    );
}

#[test]
fn a_custom_material_is_made_with_the_editor_and_leaves_the_design_alone() {
    let sim = walk(
        "viewing-custom-material",
        vec![read(), event(events::CUSTOM_MATERIAL_SAVED), read()],
    );
    let fresh = Sim::start(&guide_named("viewing-custom-material").starting_state);
    assert_eq!(
        sim.session.design.tiers.len(),
        fresh.session.design.tiers.len(),
        "saving a material only adds it to the lists"
    );
}

#[test]
fn a_coefficient_material_needs_its_own_event() {
    // A material saved with the two sliders is not one made from coefficients.
    let guide = guide_named("viewing-material-coefficients");
    let mut sim = Sim::start(&guide.starting_state);
    sim.fire(events::CUSTOM_MATERIAL_SAVED);
    assert!(
        !sim.met(&guide.steps[1].goal),
        "a slider material does not finish the coefficients lesson"
    );
    sim.fire(events::COEFFICIENT_MATERIAL_SAVED);
    assert!(sim.met(&guide.steps[1].goal));
    walk(
        "viewing-material-coefficients",
        vec![read(), event(events::COEFFICIENT_MATERIAL_SAVED), read()],
    );
}
