//! Tests of the stone cap in saved plans: the optional file key `settings.max_stones`
//! (written only for a plan with a cap, absent in every older plan, reported when out of
//! range), its round trip through the file and the library, and a really planned capped plan.

use super::{
    convert::{CandidateSource, settings_to_dto},
    fixtures::{expect_error, plain_block, sample_layout, settings, write, write_with_settings},
    format::{parse_and_validate_plan, payload_version_of},
    open::prepare_open,
    store,
    tests_db::{bits, checked_plan_text, library, planned_rough_with},
};
use indicatrix_cut_core::rough_plan::PlanSettings;

/// The fixture settings with the stone cap `max_stones`.
fn capped(max_stones: u8) -> PlanSettings {
    PlanSettings {
        max_stones,
        ..settings()
    }
}

/// A plan of the fixture rough and settings, as written before the stone cap existed: one
/// that never has the key.
fn old_plan_text() -> String {
    write(&plain_block(), &[sample_layout()])
}

/// The line after which the settings section's other keys follow; a key added by hand goes
/// here.
const COUNT_LINE: &str = "count = 6\n";

#[test]
fn a_plan_without_a_stone_cap_writes_no_key_and_reads_as_none() {
    let text = old_plan_text();
    assert!(
        !text.contains("max_stones"),
        "an uncapped plan is written as it always was:\n{text}"
    );
    let loaded = parse_and_validate_plan(&text).expect("the plan loads");
    assert_eq!(loaded.settings, settings());
    assert_eq!(loaded.settings.max_stones, 0);
    assert_eq!(loaded.settings.stone_cap(), None);
    assert_eq!(
        settings_to_dto(&settings(), CandidateSource::Library).max_stones,
        None
    );
}

#[test]
fn a_stone_cap_round_trips_through_the_file_and_the_library() {
    let settings = capped(7);
    assert_eq!(
        settings_to_dto(&settings, CandidateSource::Library).max_stones,
        Some(7)
    );
    let text = write_with_settings(&plain_block(), &settings, &[], &[]);
    assert!(
        text.contains("max_stones = 7\n"),
        "a capped plan writes the key:\n{text}"
    );
    let loaded = parse_and_validate_plan(&text).expect("the plan loads");
    assert_eq!(loaded.settings, settings);
    assert_eq!(loaded.settings.stone_cap(), Some(7));

    // Through the library: the same text comes back, still schema version 1.
    let db = library();
    let id = store::save_plan(&db, "Capped", &text).expect("stored");
    let stored = store::load_saved(&db, id).expect("loaded");
    assert_eq!(stored.payload, text);
    assert_eq!(stored.payload_version, 1, "the cap is no schema change");
    assert_eq!(payload_version_of(&stored.payload), Some(1));
    let opened = prepare_open(&db, id).expect("opens");
    assert_eq!(opened.plan.settings.max_stones, 7);
    assert_eq!(opened.plan.settings, settings);
}

#[test]
fn an_old_row_without_the_key_opens_with_no_stone_cap() {
    let text = old_plan_text();
    assert!(text.contains(COUNT_LINE), "{text}");
    let db = library();
    let id = store::save_plan(&db, "Old", &text).expect("stored");
    let opened = prepare_open(&db, id).expect("an old row opens");
    assert_eq!(opened.plan.settings.max_stones, 0);
    assert_eq!(opened.plan.settings, settings());

    // An explicit 0 means the same, and is not written back.
    let explicit = text.replace(COUNT_LINE, &format!("{COUNT_LINE}max_stones = 0\n"));
    assert_ne!(explicit, text);
    let loaded = parse_and_validate_plan(&explicit).expect("an explicit 0 loads");
    assert_eq!(loaded.settings, settings());
    assert_eq!(
        settings_to_dto(&loaded.settings, loaded.candidate_source).max_stones,
        None,
        "0 is not written back"
    );
    let again = write_with_settings(&plain_block(), &loaded.settings, &[], &[]);
    assert!(!again.contains("max_stones"), "{again}");
}

#[test]
fn the_stone_cap_is_reported_never_clamped() {
    let text = write_with_settings(&plain_block(), &settings(), &[], &[]);
    assert!(text.contains(COUNT_LINE), "{text}");
    for wrong in ["100", "255", "-1", "1000"] {
        let with_cap = text.replace(COUNT_LINE, &format!("{COUNT_LINE}max_stones = {wrong}\n"));
        let error = expect_error(&with_cap);
        assert!(error.contains("settings.max_stones"), "{wrong}: {error}");
        assert!(error.contains(wrong), "{wrong}: {error}");
    }
    for stones in [0_u8, 1, 99] {
        let with_cap = text.replace(COUNT_LINE, &format!("{COUNT_LINE}max_stones = {stones}\n"));
        let loaded = parse_and_validate_plan(&with_cap)
            .unwrap_or_else(|error| panic!("max_stones = {stones} loads: {error}"));
        assert_eq!(loaded.settings.max_stones, stones);
    }
}

#[test]
fn a_planned_capped_plan_survives_save_and_load() {
    let (model, settings, candidates, planned) = planned_rough_with(2);
    assert_eq!(settings.stone_cap(), Some(2));
    assert!(!planned.is_empty(), "a capped plan still finds layouts");
    assert!(
        planned.iter().all(|layout| layout.stones.len() <= 2),
        "every planned layout holds at most two stones"
    );

    let text = checked_plan_text(&model, &settings, &candidates, &planned);
    assert!(text.contains("max_stones = 2\n"), "{text}");
    let db = library();
    let id = store::save_plan(&db, "Capped plan", &text).expect("stored");
    let stored = store::load_saved(&db, id).expect("loaded");
    assert_eq!(stored.payload_version, 1);
    let loaded = parse_and_validate_plan(&stored.payload).expect("parsed");
    assert_eq!(
        loaded.settings, settings,
        "the cap comes back with the plan"
    );
    assert_eq!(loaded.layouts.len(), planned.len());
    for (before, after) in planned.iter().zip(&loaded.layouts) {
        assert_eq!(bits(before), bits(after), "every float survives exactly");
        assert_eq!(before.stones.len(), after.stones.len());
        assert!(after.stones.len() <= 2);
    }
}
