//! Tests of the library round trip: origin, save to library, open from library, recents.

use std::sync::Arc;

use super::*;
use crate::{
    library::LibraryDesign,
    platform::Platform,
    platform_mock::MockPlatform,
    settings::{MAX_RECENT, RecentKind},
};

fn brilliant() -> (AppState, Arc<MockPlatform>) {
    let mock = Arc::new(MockPlatform::default());
    let platform: Arc<dyn Platform> = mock.clone();
    let mut state = AppState::new(platform);
    new_from_template(&mut state, 1).expect("template 1 exists");
    (state, mock)
}

#[test]
fn a_library_origin_round_trips_through_autosave_and_restore() {
    let (mut state, mock) = brilliant();
    state.document.origin = DocumentOrigin::Library { entry_id: 42 };
    autosave(&mut state).expect("writes");
    let platform: Arc<dyn Platform> = mock;
    let mut restarted = AppState::new(platform);
    restore(&mut restarted).expect("restores");
    assert_eq!(
        restarted.document.origin,
        DocumentOrigin::Library { entry_id: 42 }
    );
    assert_eq!(restarted.document.library_entry(), Some(42));
}

#[test]
fn save_to_library_inserts_then_overwrites_with_the_same_id() {
    let (mut state, _mock) = brilliant();
    let handle = library_handle(&mut state).expect("the library opens");
    let first = save_to_library(&mut state, &handle).expect("inserts");
    assert_eq!(
        state.document.origin,
        DocumentOrigin::Library { entry_id: first }
    );
    assert!(!state.session.is_dirty());
    state
        .session
        .apply(indicatrix_cut_core::Edit::SetGirdleDiameterMm {
            girdle_diameter_mm: Some(6.0),
        })
        .expect("the edit applies");
    assert!(state.session.is_dirty());
    let second = save_to_library(&mut state, &handle).expect("overwrites");
    assert_eq!(first, second);
    assert!(!state.session.is_dirty());
    let stored = handle.load(second).expect("loads");
    let mut reopened = brilliant().0;
    open_library(&mut reopened, stored, second).expect("opens");
    assert_eq!(
        reopened.session.design.tiers.len(),
        state.session.design.tiers.len()
    );
    assert_eq!(reopened.document.library_entry(), Some(second));
}

#[test]
fn an_edit_during_the_write_keeps_the_design_dirty() {
    let (mut state, _mock) = brilliant();
    let handle = library_handle(&mut state).expect("the library opens");
    let plan = plan_library_save(&mut state).expect("plans");
    state
        .session
        .apply(indicatrix_cut_core::Edit::SetGirdleDiameterMm {
            girdle_diameter_mm: Some(6.5),
        })
        .expect("the edit applies");
    let id = write_library_save(&handle, &plan).expect("writes");
    finish_library_save(&mut state, &plan, id);
    assert!(state.session.is_dirty());
    assert_eq!(state.document.library_entry(), Some(id));
}

#[test]
fn opening_a_library_entry_records_a_library_recent_and_no_document_recent() {
    let (mut state, _mock) = brilliant();
    let handle = library_handle(&mut state).expect("the library opens");
    let id = save_to_library(&mut state, &handle).expect("inserts");
    let stored = handle.load(id).expect("loads");
    open_library(&mut state, stored, id).expect("opens");
    let recent = &state.settings.recent;
    assert_eq!(recent.len(), 1, "{recent:?}");
    assert_eq!(recent[0].kind, RecentKind::Library { entry_id: id });
    assert_eq!(recent[0].title, "Standard Round Brilliant");
}

#[test]
fn a_damaged_library_file_leaves_the_design_alone() {
    let (mut state, _mock) = brilliant();
    let tiers = state.session.design.tiers.len();
    let bad = LibraryDesign {
        file_name: "broken.indicatrix".to_owned(),
        bytes: b"not a design".to_vec(),
    };
    assert!(open_library(&mut state, bad, 9).is_err());
    assert_eq!(state.session.design.tiers.len(), tiers);
    assert_eq!(state.settings.recent.len(), 0);
}

#[test]
fn recents_dedupe_limit_and_order() {
    let (mut state, _mock) = brilliant();
    for id in 0..i64::try_from(MAX_RECENT).expect("small") + 3 {
        record_library_recent(&mut state, id);
    }
    assert_eq!(state.settings.recent.len(), MAX_RECENT);
    record_library_recent(&mut state, 5);
    let recent = &state.settings.recent;
    assert_eq!(recent[0].kind, RecentKind::Library { entry_id: 5 });
    assert_eq!(
        recent
            .iter()
            .filter(|item| item.kind == RecentKind::Library { entry_id: 5 })
            .count(),
        1
    );
    assert_eq!(recent.len(), MAX_RECENT);
}
