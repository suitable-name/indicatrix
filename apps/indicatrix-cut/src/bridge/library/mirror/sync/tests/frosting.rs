//! The frosting of a remote design (`DesignRecord::frosting`): a mirror sync copies it into the
//! local library beside the design's rows, a later revision replaces it, and a design that has
//! none leaves the local design unfrosted.

use super::{
    super::pass::run_mirror_sync,
    fixtures::{FakeTransport, design_record_at, design_summary_at, temp_db},
};
use crate::bridge::library::mirror::options::{MirrorOptions, MirrorOutcome};
use indicatrix_net::library::FrostedTierWire;
use indicatrix_vault::model::frosting::StoredTierFrosting;
use std::sync::atomic::AtomicBool;

fn sync(
    db: &std::sync::Arc<std::sync::Mutex<indicatrix_vault::db::sqlite::Database>>,
    revision: u32,
    frosting: Vec<FrostedTierWire>,
) {
    let url = "https://example.test/frosted";
    let mut record = design_record_at(1, "Frosted Brilliant", url, "v1", revision);
    record.frosting = frosting;
    let transport = FakeTransport::new(
        vec![design_summary_at(
            1,
            "Frosted Brilliant",
            url,
            "v1",
            revision,
        )],
        vec![record],
    );
    let outcome = run_mirror_sync(
        db,
        &transport,
        "remote-library:w",
        MirrorOptions::default(),
        &AtomicBool::new(false),
        |_| {},
    );
    assert!(
        matches!(outcome, MirrorOutcome::Completed(ref c) if c.failed == 0),
        "{outcome:?}"
    );
}

fn stored(
    db: &std::sync::Arc<std::sync::Mutex<indicatrix_vault::db::sqlite::Database>>,
) -> Vec<StoredTierFrosting> {
    let guard = db.lock().unwrap();
    let id = guard
        .diagram_entry_id_for_url("https://example.test/frosted")
        .unwrap()
        .expect("the design was mirrored");
    let rows = guard.diagram_frosting(id).unwrap();
    assert_eq!(
        guard.get_diagram_full(id).unwrap().unwrap().frosting,
        rows,
        "the record carries the stored rows"
    );
    rows
}

#[test]
fn a_mirror_sync_copies_the_remote_frosting_and_a_later_revision_replaces_it() {
    let (db, path) = temp_db();
    sync(
        &db,
        0,
        vec![
            FrostedTierWire {
                tier: 1,
                facets: Vec::new(),
            },
            FrostedTierWire {
                tier: 3,
                facets: vec![1, 3],
            },
        ],
    );
    assert_eq!(
        stored(&db),
        vec![
            StoredTierFrosting::whole(1),
            StoredTierFrosting {
                tier: 3,
                facets: vec![1, 3]
            },
        ]
    );

    // The remote design was re-cut with a different frosting: replaced, not merged.
    sync(
        &db,
        1,
        vec![FrostedTierWire {
            tier: 0,
            facets: vec![2],
        }],
    );
    assert_eq!(
        stored(&db),
        vec![StoredTierFrosting {
            tier: 0,
            facets: vec![2]
        }]
    );

    // And one that is polished now leaves the local design polished.
    sync(&db, 2, Vec::new());
    assert_eq!(
        stored(&db),
        [] as [indicatrix_vault::model::frosting::StoredTierFrosting; 0]
    );

    std::fs::remove_file(&path).ok();
}

#[test]
fn a_design_without_remote_frosting_is_mirrored_unfrosted() {
    let (db, path) = temp_db();
    sync(&db, 0, Vec::new());
    assert_eq!(
        stored(&db),
        [] as [indicatrix_vault::model::frosting::StoredTierFrosting; 0]
    );
    std::fs::remove_file(&path).ok();
}
