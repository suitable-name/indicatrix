//! The scan against an in-memory catalogue: what is saved, what a cancel leaves behind,
//! and what happens to a measurement whose design changed or whose save failed. The
//! geometry itself is stood in for by a fixed measurement per design.

use super::{
    BTreeMap, Database, Loaded, Measured, Mutex, Reporter, SolidExtents, SolidExtentsSource,
    SolidHull, StoredSolidExtents, merge_measured, scan_with,
};
use indicatrix_vault::model::entry::FacetingDiagramEntry;
use slint::Weak;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn extents() -> SolidExtents {
    SolidExtents {
        width_caliper: 1.0,
        length_caliper: 1.5,
        width_axis: 1.0,
        length_axis: 1.5,
        height: 0.7,
        volume: 0.9,
    }
}

fn hull() -> SolidHull {
    SolidHull {
        vertices: vec![[0.0, 0.0, 0.0]; 4],
    }
}

fn row(source: SolidExtentsSource, usable: bool) -> StoredSolidExtents {
    StoredSolidExtents {
        extents: usable.then(extents),
        source,
    }
}

/// An in-memory catalogue of `count` entries (no geometry: the tests supply the
/// measurements themselves) and their ids.
fn memory_db(count: usize) -> (Mutex<Database>, Vec<i64>) {
    let db = Database::new(Some(":memory:")).expect("in-memory database opens");
    let ids = (0..count)
        .map(|n| {
            db.save_diagram_entry(
                &FacetingDiagramEntry {
                    title: format!("Scan {n}"),
                    url: format!("local://scan-{n}.asc"),
                    design_id: String::new(),
                },
                "local-import",
            )
            .expect("entry saved")
        })
        .collect();
    (Mutex::new(db), ids)
}

/// A finished measurement read under the database's current epoch, as `measure_entry`
/// produces it.
fn ready(db: &Mutex<Database>) -> Loaded {
    let measured: Measured = (
        Some(extents()),
        SolidExtentsSource::DesignFile,
        Some(hull()),
    );
    Loaded::Ready {
        measured,
        epoch: db.lock().expect("lock").solid_extents_epoch(),
    }
}

/// A reporter that pushes nowhere, and its cancel flag.
fn quiet() -> (Reporter, Arc<AtomicBool>) {
    let cancel = Arc::new(AtomicBool::new(false));
    (Reporter::new(Weak::default(), Arc::clone(&cancel)), cancel)
}

/// How many of `ids` have an extents row.
fn saved_rows(db: &Mutex<Database>, ids: &[i64]) -> usize {
    db.lock()
        .expect("lock")
        .solid_extents_for(ids)
        .expect("rows read")
        .len()
}

#[test]
fn a_scan_saves_every_measurement_and_keeps_nothing_in_memory() {
    let (db, ids) = memory_db(5);
    let (reporter, _cancel) = quiet();
    let outcome = scan_with(&db, &reporter, &ids, false, 3, &|db, _| ready(db));
    assert!(!outcome.cancelled);
    assert!(outcome.measured.is_empty());
    assert_eq!(outcome.save_failures, 0);
    assert_eq!(saved_rows(&db, &ids), 5);
}

#[test]
fn a_cancel_after_the_first_design_leaves_exactly_one_saved_row() {
    let (db, ids) = memory_db(5);
    let (reporter, cancel) = quiet();
    // One lane takes the designs in order; the first measurement raises the cancel.
    let outcome = scan_with(&db, &reporter, &ids, false, 1, &|db, _| {
        cancel.store(true, Ordering::Relaxed);
        ready(db)
    });
    assert!(outcome.cancelled);
    assert_eq!(saved_rows(&db, &ids), 1);
}

#[test]
fn a_design_invalidated_between_measuring_and_saving_writes_nothing() {
    let (db, ids) = memory_db(2);
    let (reporter, _cancel) = quiet();
    let outcome = scan_with(&db, &reporter, &ids, false, 1, &|db, id| {
        let loaded = ready(db);
        // An import replaces the design while its measurement is still in hand.
        db.lock()
            .expect("lock")
            .delete_solid_extents(id)
            .expect("invalidated");
        loaded
    });
    assert_eq!(saved_rows(&db, &ids), 0);
    assert!(
        outcome.measured.is_empty(),
        "the figures are of the old geometry, so they are dropped, not planned with"
    );
    assert_eq!(outcome.save_failures, 0, "a skipped save is not a failure");
}

#[test]
fn a_design_deleted_between_measuring_and_saving_writes_nothing() {
    let (db, ids) = memory_db(1);
    let (reporter, _cancel) = quiet();
    let outcome = scan_with(&db, &reporter, &ids, false, 1, &|db, id| {
        let loaded = ready(db);
        db.lock()
            .expect("lock")
            .delete_diagram_entry(id)
            .expect("entry deleted");
        loaded
    });
    assert_eq!(saved_rows(&db, &ids), 0);
    assert_eq!(outcome.save_failures, 1, "the write was refused");
    assert_eq!(
        outcome.measured.keys().copied().collect::<Vec<_>>(),
        ids,
        "the figures stay in memory for this run"
    );
}

#[test]
fn load_failures_and_panics_are_counted_apart_and_the_rest_is_saved() {
    let (db, ids) = memory_db(3);
    let (reporter, _cancel) = quiet();
    let (panics, unreadable) = (ids[0], ids[1]);
    let outcome = scan_with(&db, &reporter, &ids, false, 2, &|db, id| {
        assert!(id != panics, "measuring went wrong");
        if id == unreadable {
            Loaded::Failed
        } else {
            ready(db)
        }
    });
    assert_eq!(outcome.panics, 1);
    assert_eq!(outcome.load_failures, 1);
    assert_eq!(outcome.save_failures, 0);
    assert_eq!(saved_rows(&db, &ids), 1);
    assert!(!outcome.cancelled);
}

#[test]
fn unsaved_measurements_are_laid_over_the_database_read_back() {
    let mut stored = BTreeMap::from([
        (1, row(SolidExtentsSource::DesignFile, true)),
        (3, row(SolidExtentsSource::DesignFile, true)),
    ]);
    let mut hulls = BTreeMap::from([(1, hull()), (3, hull())]);
    let other_hull = SolidHull {
        vertices: vec![[1.0, 2.0, 3.0]; 4],
    };
    let measured = BTreeMap::from([
        // Re-measured without an outline: the old hull must go with the old figures.
        (1, (None, SolidExtentsSource::Unbounded, None)),
        // Measured for the first time, with an outline.
        (
            2,
            (
                Some(extents()),
                SolidExtentsSource::DesignFile,
                Some(other_hull.clone()),
            ),
        ),
    ]);
    merge_measured(&mut stored, &mut hulls, measured);

    assert_eq!(stored[&1], row(SolidExtentsSource::Unbounded, false));
    assert_eq!(stored[&2], row(SolidExtentsSource::DesignFile, true));
    assert_eq!(
        stored[&3],
        row(SolidExtentsSource::DesignFile, true),
        "an id that was not re-measured is untouched"
    );
    assert_eq!(hulls.keys().copied().collect::<Vec<_>>(), vec![2, 3]);
    assert_eq!(hulls[&2], other_hull);
}

#[test]
fn a_design_with_unresolvable_concave_tiers_is_counted_and_never_saved() {
    let (db, ids) = memory_db(3);
    let (reporter, _cancel) = quiet();
    let skipped = ids[1];
    let outcome = scan_with(&db, &reporter, &ids, false, 2, &|db, id| {
        if id == skipped {
            Loaded::ConcaveUnresolved
        } else {
            ready(db)
        }
    });
    assert_eq!(outcome.concave_unresolved, 1);
    assert_eq!(
        outcome.load_failures + outcome.panics + outcome.save_failures,
        0
    );
    assert_eq!(saved_rows(&db, &ids), 2, "the skipped design has no row");
    assert!(outcome.measured.is_empty());
}
