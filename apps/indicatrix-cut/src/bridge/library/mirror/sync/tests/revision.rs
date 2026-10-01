//! The mirror's second staleness tier: the per-design revision token
//! (`DesignSummary::design_version` / `DesignRecord::version`) that catches an edit the
//! summary hash cannot see, the all-zero "no revision" sentinel, and the `decide` rules
//! around tombstones and moved versions.

use super::{
    super::pass::{Decision, decide, remote_design_version, run_mirror_sync},
    fixtures::{FakeTransport, design_record_at, design_summary, design_summary_at, temp_db},
};
use crate::bridge::library::mirror::options::{MirrorOptions, MirrorOutcome};
use indicatrix_net::library::DesignSummary;
use indicatrix_vault::model::mirror::MirrorState;
use std::sync::atomic::{AtomicBool, Ordering};

/// A remote edit that leaves every search-row field alone (an angle-table or notes edit)
/// keeps the summary hash equal but moves the design revision token: the design must be
/// re-fetched and updated, and the pass after that must skip it again.
#[test]
fn an_edit_that_leaves_the_summary_hash_unchanged_is_refetched_via_the_design_version() {
    let (db, path) = temp_db();
    let url = "https://example.test/1";
    let summary_v1 = design_summary_at(1, "Round Brilliant", url, "v1", 0);
    let first = run_mirror_sync(
        &db,
        &FakeTransport::new(
            vec![summary_v1.clone()],
            vec![design_record_at(1, "Round Brilliant", url, "v1", 0)],
        ),
        "remote-library:w",
        MirrorOptions::default(),
        &AtomicBool::new(false),
        |_| {},
    );
    assert!(matches!(first, MirrorOutcome::Completed(c) if c.new_count == 1));

    // The remote angle table was edited: same search row, new revision.
    let summary_v2 = design_summary_at(1, "Round Brilliant", url, "v1", 1);
    assert_eq!(summary_v1.version, summary_v2.version);
    assert_ne!(summary_v1.design_version, summary_v2.design_version);
    let mut edited = design_record_at(1, "Round Brilliant", url, "v1", 1);
    edited.angle_settings[0].angle = "42.5".to_string();
    let transport = FakeTransport::new(vec![summary_v2.clone()], vec![edited]);
    let second = run_mirror_sync(
        &db,
        &transport,
        "remote-library:w",
        MirrorOptions::default(),
        &AtomicBool::new(false),
        |_| {},
    );
    let MirrorOutcome::Completed(counts) = second else {
        panic!("expected Completed, got {second:?}");
    };
    assert_eq!(counts.updated_count, 1);
    assert_eq!(counts.skipped_unchanged, 0);
    assert_eq!(transport.fetch_design_calls.load(Ordering::Relaxed), 1);

    let guard = db.lock().unwrap();
    let id = guard.diagram_entry_id_for_url(url).unwrap().unwrap();
    let full = guard.get_diagram_full(id).unwrap().unwrap();
    assert_eq!(full.angle_settings[0].angle, "42.5");
    let state = guard.get_mirror_state(url).unwrap().unwrap();
    assert_eq!(state.design_version, summary_v2.design_version);
    drop(guard);

    // Nothing moved since: the next pass fetches nothing.
    let third = run_mirror_sync(
        &db,
        &transport,
        "remote-library:w",
        MirrorOptions::default(),
        &AtomicBool::new(false),
        |_| {},
    );
    let MirrorOutcome::Completed(counts) = third else {
        panic!("expected Completed, got {third:?}");
    };
    assert_eq!(counts.skipped_unchanged, 1);
    assert_eq!(transport.fetch_design_calls.load(Ordering::Relaxed), 1);
    std::fs::remove_file(&path).ok();
}

/// Builds a non-tombstoned state whose versions match `summary` for [`decide`]'s tests.
fn state_matching(summary: &DesignSummary) -> MirrorState {
    MirrorState {
        url: summary.url.clone(),
        source_id: "remote-library:w".to_string(),
        summary_version: summary.version,
        design_version: summary.design_version,
        deleted_locally: false,
    }
}

#[test]
fn decide_skips_a_tombstone_whatever_the_versions_say() {
    let summary = design_summary(1, "Round Brilliant", "https://example.test/1", "v1");
    let mut state = state_matching(&summary);
    state.deleted_locally = true;
    assert_eq!(
        decide(Some(&state), &summary, Some([9u8; 32])),
        Decision::SkipDeleted
    );
    state.summary_version = [0u8; 32];
    assert_eq!(decide(Some(&state), &summary, None), Decision::SkipDeleted);
}

#[test]
fn decide_syncs_when_either_version_moved_and_skips_only_when_both_match() {
    let summary = design_summary(1, "Round Brilliant", "https://example.test/1", "v1");
    let state = state_matching(&summary);
    assert_eq!(decide(None, &summary, None), Decision::Sync);
    assert_eq!(
        decide(Some(&state), &summary, Some(summary.design_version)),
        Decision::SkipUnchanged
    );
    // Same summary hash, but the revision token moved: only the second tier sees it.
    assert_eq!(
        decide(Some(&state), &summary, Some([6u8; 32])),
        Decision::Sync
    );
    // A remote that states no revision token cannot be compared, so it counts as moved.
    assert_eq!(decide(Some(&state), &summary, None), Decision::Sync);
    let moved_summary = design_summary(1, "Round Brilliant", "https://example.test/1", "v2");
    assert_eq!(
        decide(Some(&state), &moved_summary, Some(summary.design_version)),
        Decision::Sync
    );
}

#[test]
fn a_summary_with_an_all_zero_design_version_states_no_revision() {
    let mut summary = design_summary(1, "Round Brilliant", "https://example.test/1", "v1");
    assert_eq!(
        remote_design_version(&summary),
        Some(summary.design_version)
    );
    summary.design_version = [0u8; 32];
    assert_eq!(remote_design_version(&summary), None);
}
