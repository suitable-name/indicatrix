//! Tests for the mirror sync pipeline, built against [`fixtures::FakeTransport`], a
//! scripted, in-memory [`LibraryTransport`](crate::bridge::library::mirror::options::LibraryTransport).

mod fixtures;

use super::pass::run_mirror_sync;
use crate::{
    bridge::library::mirror::options::{MirrorOptions, MirrorOutcome, mirror_source_id},
    settings::WorkerSettings,
};
use fixtures::{FakeTransport, design_record, design_summary, temp_db};
use indicatrix_net::library::{AttachedFileMeta, DesignRecord, DesignSummary};
use indicatrix_vault::model::{
    detail::FacetDiagramDetail, entry::FacetDiagramEntry, file::AttachedFile,
};
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn a_local_only_design_survives_a_mirror_sync() {
    let (db, path) = temp_db();
    // A design that exists ONLY locally (a user's own `.asc` import) -- a synthetic
    // `local://` URL, exactly `indicatrix_vault::local::import_asc`'s convention.
    let local_entry_id = {
        let guard = db.lock().unwrap();
        guard
            .save_diagram_entry(
                &FacetDiagramEntry {
                    title: "My Own Trichecker".to_string(),
                    url: "local://my_trichecker.asc".to_string(),
                    design_id: String::new(),
                },
                indicatrix_vault::local::LOCAL_SOURCE_ID,
            )
            .unwrap()
    };
    {
        let guard = db.lock().unwrap();
        guard
            .save_diagram_detail(
                &FacetDiagramDetail {
                    shape: Some("Trichecker".to_string()),
                    attached_files: vec![AttachedFile {
                        name: "my_trichecker.asc".to_string(),
                        url: String::new(),
                        content: b"a real user file".to_vec(),
                    }],
                    ..FacetDiagramDetail::default()
                },
                local_entry_id,
            )
            .unwrap();
    }

    let remote_summary = design_summary(1, "Round Brilliant", "https://example.test/1", "v1");
    let remote_design = design_record(1, "Round Brilliant", "https://example.test/1", "v1");
    let transport = FakeTransport::new(vec![remote_summary], vec![remote_design]);

    let outcome = run_mirror_sync(
        &db,
        &transport,
        "remote-library:example.test:9443",
        MirrorOptions::default(),
        &AtomicBool::new(false),
        |_| {},
    );
    assert!(matches!(outcome, MirrorOutcome::Completed(c) if c.new_count == 1));

    // The local-only design is completely untouched: still there, same title, same
    // attachment content -- sync never deleted or altered it.
    let guard = db.lock().unwrap();
    let local_full = guard.get_diagram_full(local_entry_id).unwrap().unwrap();
    assert_eq!(local_full.title, "My Own Trichecker");
    assert_eq!(local_full.url, "local://my_trichecker.asc");
    assert_eq!(local_full.attached_files.len(), 1);
    assert_eq!(local_full.attached_files[0].content, b"a real user file");
    // And the remote design was actually added alongside it -- total count is both.
    assert_eq!(guard.get_total_count().unwrap(), 2);
    drop(guard);

    std::fs::remove_file(&path).ok();
}

#[test]
fn a_new_design_is_saved_with_its_attachment() {
    let (db, path) = temp_db();
    let mut record = design_record(1, "Round Brilliant", "https://example.test/1", "v1");
    record.attachments = vec![AttachedFileMeta {
        id: 100,
        name: "schedule.pdf".to_string(),
        url: "https://example.test/schedule.pdf".to_string(),
        size: 5,
    }];
    let summary = design_summary(1, "Round Brilliant", "https://example.test/1", "v1");
    let transport = FakeTransport::new(vec![summary], vec![record]).with_attachment(
        100,
        "schedule.pdf",
        vec![1, 2, 3, 4, 5],
    );

    let outcome = run_mirror_sync(
        &db,
        &transport,
        "remote-library:w",
        MirrorOptions::default(),
        &AtomicBool::new(false),
        |_| {},
    );
    let MirrorOutcome::Completed(counts) = outcome else {
        panic!("expected Completed, got {outcome:?}");
    };
    assert_eq!(counts.new_count, 1);
    assert_eq!(counts.attachments_fetched, 1);
    assert_eq!(counts.attachment_bytes_fetched, 5);

    let guard = db.lock().unwrap();
    let full = guard
        .search_diagrams(
            "",
            "All",
            "All",
            &indicatrix_vault::model::filter::RangeFilter::default(),
        )
        .unwrap();
    assert_eq!(full.len(), 1);
    let entry_id = full[0].id;
    let record = guard.get_diagram_full(entry_id).unwrap().unwrap();
    assert_eq!(record.attached_files.len(), 1);
    assert_eq!(record.attached_files[0].content, vec![1, 2, 3, 4, 5]);
    drop(guard);
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_second_sync_of_an_unchanged_catalogue_skips_every_design_and_makes_no_fetch_design_calls() {
    let (db, path) = temp_db();
    let summary = design_summary(1, "Round Brilliant", "https://example.test/1", "v1");
    let record = design_record(1, "Round Brilliant", "https://example.test/1", "v1");
    let transport = FakeTransport::new(vec![summary], vec![record]);

    let first = run_mirror_sync(
        &db,
        &transport,
        "remote-library:w",
        MirrorOptions::default(),
        &AtomicBool::new(false),
        |_| {},
    );
    assert!(matches!(first, MirrorOutcome::Completed(c) if c.new_count == 1));
    assert_eq!(transport.fetch_design_calls.load(Ordering::Relaxed), 1);

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
    assert_eq!(counts.skipped_unchanged, 1);
    assert_eq!(counts.new_count, 0);
    assert_eq!(counts.updated_count, 0);
    // The whole point: a no-op resync makes exactly one more Search and ZERO
    // FetchDesign/FetchAttachment calls beyond the first sync's.
    assert_eq!(transport.search_calls.load(Ordering::Relaxed), 2);
    assert_eq!(transport.fetch_design_calls.load(Ordering::Relaxed), 1);
    assert_eq!(transport.fetch_attachment_calls.load(Ordering::Relaxed), 0);

    std::fs::remove_file(&path).ok();
}

#[test]
fn a_changed_design_is_refetched_and_updated_in_place() {
    let (db, path) = temp_db();
    let summary_v1 = design_summary(1, "Round Brilliant", "https://example.test/1", "v1");
    let record_v1 = design_record(1, "Round Brilliant", "https://example.test/1", "v1");
    let transport_v1 = FakeTransport::new(vec![summary_v1], vec![record_v1]);
    let _ = run_mirror_sync(
        &db,
        &transport_v1,
        "remote-library:w",
        MirrorOptions::default(),
        &AtomicBool::new(false),
        |_| {},
    );

    // The remote design's L/W ratio changed -- both its summary and design hashes
    // move (see `design_summary`/`design_record`'s shared `content_hash` input).
    let summary_v2 = design_summary(1, "Round Brilliant", "https://example.test/1", "v2-updated");
    let record_v2 = design_record(1, "Round Brilliant", "https://example.test/1", "v2-updated");
    let transport_v2 = FakeTransport::new(vec![summary_v2], vec![record_v2]);

    let outcome = run_mirror_sync(
        &db,
        &transport_v2,
        "remote-library:w",
        MirrorOptions::default(),
        &AtomicBool::new(false),
        |_| {},
    );
    let MirrorOutcome::Completed(counts) = outcome else {
        panic!("expected Completed, got {outcome:?}");
    };
    assert_eq!(counts.updated_count, 1);
    assert_eq!(counts.new_count, 0);

    let guard = db.lock().unwrap();
    let items = guard
        .search_diagrams(
            "",
            "All",
            "All",
            &indicatrix_vault::model::filter::RangeFilter::default(),
        )
        .unwrap();
    assert_eq!(
        items.len(),
        1,
        "must update the existing row, not add a second one"
    );
    assert_eq!(items[0].lw_ratio.as_deref(), Some("v2-updated"));
    drop(guard);
    std::fs::remove_file(&path).ok();
}

#[test]
fn cancelling_mid_sync_leaves_earlier_designs_committed_and_the_rest_untouched() {
    let (db, path) = temp_db();
    let summaries = vec![
        design_summary(1, "First", "https://example.test/1", "v1"),
        design_summary(2, "Second", "https://example.test/2", "v1"),
        design_summary(3, "Third", "https://example.test/3", "v1"),
    ];
    let designs = vec![
        design_record(1, "First", "https://example.test/1", "v1"),
        design_record(2, "Second", "https://example.test/2", "v1"),
        design_record(3, "Third", "https://example.test/3", "v1"),
    ];
    let transport = FakeTransport::new(summaries, designs);
    let cancel = AtomicBool::new(false);

    let mut processed_count = 0;
    let outcome = run_mirror_sync(
        &db,
        &transport,
        "remote-library:w",
        MirrorOptions::default(),
        &cancel,
        |progress| {
            processed_count = progress.processed;
            // Cancel right after the first design commits, before the second is
            // ever looked at.
            if progress.processed == 1 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    );
    assert_eq!(processed_count, 1);
    let MirrorOutcome::Cancelled(counts) = outcome else {
        panic!("expected Cancelled, got {outcome:?}");
    };
    assert_eq!(counts.new_count, 1);

    let guard = db.lock().unwrap();
    // Exactly one design landed -- the second and third were never even fetched
    // (FetchDesign was called exactly once), so nothing about them exists locally,
    // not even a half-written row.
    assert_eq!(guard.get_total_count().unwrap(), 1);
    assert_eq!(transport.fetch_design_calls.load(Ordering::Relaxed), 1);
    assert_eq!(*transport.fetched_entry_ids.borrow(), vec![1]);
    drop(guard);
    std::fs::remove_file(&path).ok();
}

#[test]
fn an_oversized_attachment_is_skipped_but_the_design_is_still_saved() {
    let (db, path) = temp_db();
    let mut record = design_record(1, "Round Brilliant", "https://example.test/1", "v1");
    record.attachments = vec![
        AttachedFileMeta {
            id: 100,
            name: "huge.pdf".to_string(),
            url: String::new(),
            size: 1000,
        },
        AttachedFileMeta {
            id: 101,
            name: "small.pdf".to_string(),
            url: String::new(),
            size: 5,
        },
    ];
    let summary = design_summary(1, "Round Brilliant", "https://example.test/1", "v1");
    let transport = FakeTransport::new(vec![summary], vec![record])
        .with_attachment(100, "huge.pdf", vec![0u8; 1000])
        .with_attachment(101, "small.pdf", vec![9, 9, 9, 9, 9]);

    let options = MirrorOptions {
        max_attachment_bytes: 500,
    };
    let outcome = run_mirror_sync(
        &db,
        &transport,
        "remote-library:w",
        options,
        &AtomicBool::new(false),
        |_| {},
    );
    let MirrorOutcome::Completed(counts) = outcome else {
        panic!("expected Completed, got {outcome:?}");
    };
    assert_eq!(counts.new_count, 1, "the design itself must still be saved");
    assert_eq!(counts.attachments_fetched, 1);
    assert_eq!(counts.attachments_skipped_too_large, 1);
    assert_eq!(
        transport.fetch_attachment_calls.load(Ordering::Relaxed),
        1,
        "the oversized attachment's bytes must never even be requested"
    );

    let guard = db.lock().unwrap();
    let items = guard
        .search_diagrams(
            "",
            "All",
            "All",
            &indicatrix_vault::model::filter::RangeFilter::default(),
        )
        .unwrap();
    let full = guard.get_diagram_full(items[0].id).unwrap().unwrap();
    assert_eq!(full.attached_files.len(), 1);
    assert_eq!(full.attached_files[0].name, "small.pdf");
    drop(guard);
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_failed_fetch_is_counted_and_never_marked_synced() {
    let (db, path) = temp_db();
    // The summary claims entry_id 1, but the transport's `designs` map has nothing
    // for it -- simulating a design that vanished between Search and FetchDesign.
    let summary = design_summary(1, "Ghost", "https://example.test/1", "v1");
    let transport = FakeTransport::new(vec![summary], Vec::new());

    let outcome = run_mirror_sync(
        &db,
        &transport,
        "remote-library:w",
        MirrorOptions::default(),
        &AtomicBool::new(false),
        |_| {},
    );
    let MirrorOutcome::Completed(counts) = outcome else {
        panic!("expected Completed, got {outcome:?}");
    };
    assert_eq!(counts.failed, 1);
    assert_eq!(counts.new_count, 0);

    let guard = db.lock().unwrap();
    assert_eq!(guard.get_total_count().unwrap(), 0);
    assert_eq!(
        guard.get_mirror_state("https://example.test/1").unwrap(),
        None
    );
    drop(guard);
    std::fs::remove_file(&path).ok();
}

/// The scenario the held-connection task exists to handle: a mid-sync connection
/// drop. In production, `LibrarySession` (see `bridge::library::client`) hides a
/// RECOVERABLE drop entirely -- it reconnects and retries the same request, so
/// `run_mirror_sync` never even sees a failure (see
/// `library_client::tests::request_with_reconnect_reconnects_once_after_a_dead_connection_and_succeeds`
/// for that half, proven with no live socket). This test covers the other half: what
/// `run_mirror_sync` itself does when a design's connection drop could NOT be
/// recovered (the reconnect attempt also failed) -- exactly what
/// `FakeTransport::with_fetch_design_failure` scripts. The sync must not abort: it
/// counts that one design as failed, leaves it exactly as it was locally (so the next
/// sync retries it, same as `a_failed_fetch_is_counted_and_never_marked_synced`), and
/// keeps going to fetch and save every OTHER design in the catalogue.
#[test]
fn a_design_whose_connection_could_not_be_recovered_is_skipped_but_the_rest_of_the_sync_completes()
{
    let (db, path) = temp_db();
    let summaries = vec![
        design_summary(1, "First", "https://example.test/1", "v1"),
        design_summary(2, "Second", "https://example.test/2", "v1"),
        design_summary(3, "Third", "https://example.test/3", "v1"),
    ];
    let designs = vec![
        design_record(1, "First", "https://example.test/1", "v1"),
        design_record(2, "Second", "https://example.test/2", "v1"),
        design_record(3, "Third", "https://example.test/3", "v1"),
    ];
    let transport = FakeTransport::new(summaries, designs).with_fetch_design_failure(2);

    let mut processed_count = 0;
    let outcome = run_mirror_sync(
        &db,
        &transport,
        "remote-library:w",
        MirrorOptions::default(),
        &AtomicBool::new(false),
        |progress| processed_count = progress.processed,
    );
    let MirrorOutcome::Completed(counts) = outcome else {
        panic!(
            "an unrecoverable connection drop for ONE design must not abort the whole \
             sync, got {outcome:?}"
        );
    };
    assert_eq!(
        processed_count, 3,
        "every design was still examined, including after the drop"
    );
    assert_eq!(counts.total_found, 3);
    assert_eq!(
        counts.new_count, 2,
        "designs 1 and 3 still land despite design 2's failure"
    );
    assert_eq!(counts.failed, 1);

    let guard = db.lock().unwrap();
    assert_eq!(guard.get_total_count().unwrap(), 2);
    let items = guard
        .search_diagrams(
            "",
            "All",
            "All",
            &indicatrix_vault::model::filter::RangeFilter::default(),
        )
        .unwrap();
    let titles: std::collections::HashSet<&str> = items.iter().map(|i| i.title.as_str()).collect();
    assert!(titles.contains("First"));
    assert!(titles.contains("Third"));
    assert!(
        !titles.contains("Second"),
        "the design whose connection could not be recovered must never be half-saved, \
         titles were: {titles:?}"
    );
    assert_eq!(
        guard.get_mirror_state("https://example.test/2").unwrap(),
        None,
        "never marked synced, so the next sync retries it -- same as any other failed fetch"
    );
    drop(guard);
    std::fs::remove_file(&path).ok();
}

#[test]
fn mirror_source_id_is_distinct_per_worker_address() {
    let a = WorkerSettings {
        address: "10.0.0.5:9443".to_string(),
        ..WorkerSettings::default()
    };
    let b = WorkerSettings {
        address: "10.0.0.6:9443".to_string(),
        ..WorkerSettings::default()
    };
    assert_ne!(mirror_source_id(&a), mirror_source_id(&b));
}

#[test]
fn a_multi_page_mirror_reaches_designs_beyond_the_first_page() {
    let (db, path) = temp_db();
    let entries: Vec<(i64, &str)> = vec![
        (1, "First"),
        (2, "Second"),
        (3, "Third"),
        (4, "Fourth"),
        (5, "Fifth"),
    ];
    let summaries: Vec<DesignSummary> = entries
        .iter()
        .map(|(id, title)| design_summary(*id, title, &format!("https://example.test/{id}"), "v1"))
        .collect();
    let designs: Vec<DesignRecord> = entries
        .iter()
        .map(|(id, title)| design_record(*id, title, &format!("https://example.test/{id}"), "v1"))
        .collect();
    // 5 designs at 2 rows per SearchPage reply: pages of [1,2], [3,4], [5] -- the
    // last page is short, so exactly 3 round trips, not 5 and not 1.
    let transport = FakeTransport::new(summaries, designs).with_page_size(2);

    let outcome = run_mirror_sync(
        &db,
        &transport,
        "remote-library:w",
        MirrorOptions::default(),
        &AtomicBool::new(false),
        |_| {},
    );
    let MirrorOutcome::Completed(counts) = outcome else {
        panic!("expected Completed, got {outcome:?}");
    };
    assert_eq!(
        counts.total_found, 5,
        "every design across every page must be counted, not just the first page's"
    );
    assert_eq!(counts.new_count, 5);
    assert_eq!(transport.search_calls.load(Ordering::Relaxed), 3);
    assert_eq!(transport.fetch_design_calls.load(Ordering::Relaxed), 5);

    let guard = db.lock().unwrap();
    assert_eq!(guard.get_total_count().unwrap(), 5);
    let items = guard
        .search_diagrams(
            "",
            "All",
            "All",
            &indicatrix_vault::model::filter::RangeFilter::default(),
        )
        .unwrap();
    let titles: std::collections::HashSet<&str> = items.iter().map(|i| i.title.as_str()).collect();
    // The whole point: "Fourth" and "Fifth" sat past the first page and must have
    // actually landed locally, not just been counted.
    assert!(titles.contains("Fourth"), "titles were: {titles:?}");
    assert!(titles.contains("Fifth"), "titles were: {titles:?}");
    drop(guard);
    std::fs::remove_file(&path).ok();
}
