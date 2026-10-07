//! Walks of the lessons in `viewing/library.rs`.

use super::{Sim, event, guide_named, read, viewing_events as events, walk};

#[test]
fn import_writes_a_file_then_imports_it() {
    walk(
        "library-import",
        vec![
            event(events::ASC_EXPORTED),
            event(events::LIBRARY_IMPORTED),
            read(),
        ],
    );
}

#[test]
fn search_types_clears_and_selects() {
    walk(
        "library-search",
        vec![
            read(),
            event(events::LIBRARY_SEARCHED),
            event(events::LIBRARY_SEARCH_CLEARED),
            event(events::LIBRARY_DESIGN_SELECTED),
            read(),
        ],
    );
}

#[test]
fn filters_change_a_drop_down_a_range_the_sort_and_reset() {
    walk(
        "library-filters",
        vec![
            event(events::LIBRARY_FILTERED),
            event(events::LIBRARY_FILTERED),
            event(events::LIBRARY_FILTERED),
            event(events::LIBRARY_FILTERS_RESET),
        ],
    );
}

/// "Clear it" asks for an empty search box: typing one more letter reports a search, not a
/// cleared one. And a cleared box is not "a search typed".
#[test]
fn clearing_the_search_is_not_typing_in_it() {
    let guide = guide_named("library-search");
    let (typed, cleared) = (&guide.steps[1], &guide.steps[2]);
    let mut sim = Sim::start(&guide.starting_state);
    sim.fire(events::LIBRARY_SEARCHED);
    assert!(sim.met(&typed.goal));
    assert!(
        !sim.met(&cleared.goal),
        "one more letter must not clear the search"
    );
    let mut sim = Sim::start(&guide.starting_state);
    sim.fire(events::LIBRARY_SEARCH_CLEARED);
    assert!(sim.met(&cleared.goal));
    assert!(
        !sim.met(&typed.goal),
        "an emptied box is not a search typed"
    );
}

/// "Put everything back" asks for no search or filter left on: changing a filter, the sort
/// order or the shape reports a change, not a reset.
#[test]
fn changing_a_filter_is_not_putting_everything_back() {
    let guide = guide_named("library-filters");
    let reset = guide.steps.last().expect("a last step");
    let mut sim = Sim::start(&guide.starting_state);
    sim.fire(events::LIBRARY_FILTERED);
    assert!(
        !sim.met(&reset.goal),
        "a second filter or a sort must not finish the reset step"
    );
    sim.fire(events::LIBRARY_FILTERS_RESET);
    assert!(sim.met(&reset.goal));
}

#[test]
fn a_design_is_selected_then_loaded() {
    walk(
        "library-load-design",
        vec![
            event(events::LIBRARY_DESIGN_SELECTED),
            event(events::DESIGN_REPLACED),
            read(),
        ],
    );
}

#[test]
fn the_rough_planner_opens_and_plans() {
    walk(
        "library-rough-planner",
        vec![
            event(events::ROUGH_PLANNER_OPENED),
            read(),
            event(events::ROUGH_PLAN_FINISHED),
            read(),
        ],
    );
}

#[test]
fn selecting_a_design_is_not_loading_it() {
    let guide = guide_named("library-load-design");
    let mut sim = Sim::start(&guide.starting_state);
    sim.fire(events::LIBRARY_DESIGN_SELECTED);
    assert!(
        !sim.met(&guide.steps[1].goal),
        "the load step waits for the design to replace the open one"
    );
}

#[test]
fn opening_the_planner_is_not_planning() {
    let guide = guide_named("library-rough-planner");
    let mut sim = Sim::start(&guide.starting_state);
    sim.fire(events::ROUGH_PLANNER_OPENED);
    assert!(
        !sim.met(&guide.steps[2].goal),
        "the plan step waits for a finished plan"
    );
}

#[test]
fn the_library_lessons_start_from_the_open_design_except_import() {
    for id in [
        "library-search",
        "library-filters",
        "library-load-design",
        "library-rough-planner",
    ] {
        let guide = guide_named(id);
        assert!(
            matches!(
                guide.starting_state,
                crate::guide::StartingState::CurrentDesign
            ),
            "{id} works with whatever is open"
        );
    }
}
