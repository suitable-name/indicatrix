//! The Rough Planner exclusion mark (`diagram_planner_exclusions`): toggling it both
//! ways, reading it back as sorted sets, and its independence from `updated_at`, from
//! design re-saves and deletes, and from the library's `ignored` flag.

use super::{super::*, fixtures::temp_db_path};
use std::collections::BTreeSet;

fn in_memory_db() -> Database {
    Database::new(Some(":memory:")).expect("create in-memory db")
}

/// Saves one local entry titled `title` (its url derives from the title) and returns its id.
fn save_entry(db: &Database, title: &str) -> i64 {
    db.save_diagram_entry(
        &FacetingDiagramEntry {
            title: title.to_string(),
            url: format!("local://{}.asc", title.to_lowercase().replace(' ', "-")),
            design_id: String::new(),
        },
        "local-import",
    )
    .expect("save entry")
}

/// The number of rows in `diagram_planner_exclusions`, read directly.
fn exclusion_row_count(db: &Database) -> i64 {
    db.conn
        .query_row("SELECT COUNT(*) FROM diagram_planner_exclusions", [], |r| {
            r.get(0)
        })
        .unwrap()
}

#[test]
fn excluding_and_including_toggle_the_mark_both_ways() {
    let db = in_memory_db();
    let a = save_entry(&db, "Alpha");
    let b = save_entry(&db, "Beta");
    assert!(db.planner_excluded_ids().unwrap().is_empty());

    db.set_planner_excluded(a, true).unwrap();
    assert_eq!(db.planner_excluded_ids().unwrap(), BTreeSet::from([a]));
    db.set_planner_excluded(b, true).unwrap();
    assert_eq!(db.planner_excluded_ids().unwrap(), BTreeSet::from([a, b]));

    db.set_planner_excluded(a, false).unwrap();
    assert_eq!(db.planner_excluded_ids().unwrap(), BTreeSet::from([b]));
    db.set_planner_excluded(b, false).unwrap();
    assert!(db.planner_excluded_ids().unwrap().is_empty());
    assert_eq!(exclusion_row_count(&db), 0);
}

#[test]
fn excluding_an_already_excluded_design_is_a_no_op() {
    let db = in_memory_db();
    let a = save_entry(&db, "Alpha");

    db.set_planner_excluded(a, true).unwrap();
    db.set_planner_excluded(a, true).unwrap();

    assert_eq!(db.planner_excluded_ids().unwrap(), BTreeSet::from([a]));
    assert_eq!(exclusion_row_count(&db), 1, "one row per excluded design");
}

#[test]
fn excluding_an_unknown_entry_id_fails_through_the_foreign_key_and_writes_nothing() {
    let db = in_memory_db();
    let a = save_entry(&db, "Alpha");

    let err = db
        .set_planner_excluded(a + 999, true)
        .expect_err("excluding a design that does not exist must fail");
    assert!(
        format!("{err:#}").contains("FOREIGN KEY"),
        "the foreign key must be what refuses it, got: {err:#}"
    );
    assert_eq!(exclusion_row_count(&db), 0);
}

#[test]
fn including_a_design_that_is_not_excluded_or_does_not_exist_is_ok() {
    let db = in_memory_db();
    let a = save_entry(&db, "Alpha");

    db.set_planner_excluded(a, false)
        .expect("a design that was never excluded has nothing to remove");
    db.set_planner_excluded(a + 999, false)
        .expect("an unknown id has nothing to remove either");
    assert_eq!(exclusion_row_count(&db), 0);
}

#[test]
fn excluded_ids_and_excluded_among_return_sorted_sets() {
    let db = in_memory_db();
    let ids: Vec<i64> = ["One", "Two", "Three", "Four", "Five"]
        .iter()
        .map(|title| save_entry(&db, title))
        .collect();
    let [one, two, three, four, five] = ids[..] else {
        panic!("five entries were saved");
    };
    // Written out of order: the sets must come back ascending regardless.
    for id in [five, two, four] {
        db.set_planner_excluded(id, true).unwrap();
    }

    let all: Vec<i64> = db.planner_excluded_ids().unwrap().into_iter().collect();
    assert_eq!(all, vec![two, four, five]);

    let among = db
        .planner_excluded_among(&[five, one, two, three, five + 999, -7])
        .unwrap();
    assert_eq!(
        among.into_iter().collect::<Vec<_>>(),
        vec![two, five],
        "ids that are not excluded, or name no design, must be ignored"
    );

    assert_eq!(
        db.planner_excluded_among(&[two, two, four]).unwrap(),
        BTreeSet::from([two, four]),
        "a repeated id must not repeat in the result"
    );
    assert!(db.planner_excluded_among(&[]).unwrap().is_empty());
    assert!(db.planner_excluded_among(&[one, three]).unwrap().is_empty());
}

/// The ids travel as one JSON array bound to one statement, so a list far beyond any
/// bound-parameter limit still works in a single call.
#[test]
fn excluded_among_takes_a_huge_id_list_in_one_query() {
    let db = in_memory_db();
    let a = save_entry(&db, "Alpha");
    let b = save_entry(&db, "Beta");
    db.set_planner_excluded(b, true).unwrap();

    let mut ids: Vec<i64> = (1_000_000..1_040_000).collect();
    ids.extend([a, b]);

    assert_eq!(
        db.planner_excluded_among(&ids).unwrap(),
        BTreeSet::from([b])
    );
}

/// The mark deliberately leaves `diagram_entries.updated_at` alone -- see
/// `set_planner_excluded`'s own doc comment: it is the revision stamp every cache and
/// compare-and-swap write keys on.
#[test]
fn toggling_the_mark_does_not_bump_updated_at() {
    let db = in_memory_db();
    let id = save_entry(&db, "Exclude Me");
    // A known-stale stamp, so a bump would be visible within one fast test.
    db.conn
        .execute(
            "UPDATE diagram_entries SET updated_at = 0 WHERE id = ?1",
            params![id],
        )
        .unwrap();

    db.set_planner_excluded(id, true).unwrap();
    assert_eq!(
        db.entry_updated_at(id).unwrap(),
        Some(0),
        "set_planner_excluded(true) must leave updated_at untouched"
    );

    db.set_planner_excluded(id, false).unwrap();
    assert_eq!(
        db.entry_updated_at(id).unwrap(),
        Some(0),
        "set_planner_excluded(false) must leave updated_at untouched"
    );
}

#[test]
fn deleting_a_design_cascades_its_exclusion_row() {
    let db = in_memory_db();
    let doomed = save_entry(&db, "Doomed");
    let kept = save_entry(&db, "Kept");
    db.set_planner_excluded(doomed, true).unwrap();
    db.set_planner_excluded(kept, true).unwrap();

    db.delete_diagram_entry(doomed).unwrap();

    assert_eq!(db.planner_excluded_ids().unwrap(), BTreeSet::from([kept]));
    assert_eq!(exclusion_row_count(&db), 1, "no orphaned row may remain");
    assert!(db.planner_excluded_among(&[doomed]).unwrap().is_empty());
}

/// A re-save of the same url updates the entry row in place (and a detail re-sync
/// deletes and reinserts the unrelated `diagram_details` row), so the mark -- keyed by
/// `entry_id` -- must survive both.
#[test]
fn the_mark_survives_save_design_over_the_same_url_and_a_detail_re_sync() {
    let db = in_memory_db();
    let url = "local://resave.asc".to_string();
    let id = db
        .save_design(
            &FacetingDiagramEntry {
                title: "First Title".to_string(),
                url: url.clone(),
                design_id: String::new(),
            },
            &FacetingDiagramDetail {
                shape: Some("Round".to_string()),
                ..Default::default()
            },
            "local-import",
        )
        .unwrap();
    db.set_planner_excluded(id, true).unwrap();

    let again = db
        .save_design(
            &FacetingDiagramEntry {
                title: "Second Title".to_string(),
                url,
                design_id: String::new(),
            },
            &FacetingDiagramDetail {
                shape: Some("Pear".to_string()),
                ..Default::default()
            },
            "local-import",
        )
        .unwrap();
    assert_eq!(again, id, "the same url must reuse the entry row");
    assert_eq!(db.planner_excluded_ids().unwrap(), BTreeSet::from([id]));

    db.save_diagram_detail(&FacetingDiagramDetail::default(), id)
        .unwrap();
    assert_eq!(db.planner_excluded_ids().unwrap(), BTreeSet::from([id]));
}

/// The mark is not the library's `ignored` flag: an excluded design is still listed by
/// `all_entry_ids` and by the search, and the two flags move independently.
#[test]
fn an_excluded_design_is_still_listed_and_searchable_and_the_flag_is_orthogonal_to_ignored() {
    let db = in_memory_db();
    let cube = save_entry(&db, "True Cube");
    let round = save_entry(&db, "Round");
    db.set_planner_excluded(cube, true).unwrap();

    assert_eq!(db.all_entry_ids().unwrap(), vec![cube, round]);
    let listed: Vec<i64> = db
        .search_diagrams("", "All", "All", &RangeFilter::default())
        .unwrap()
        .iter()
        .map(|item| item.id)
        .collect();
    assert_eq!(listed, vec![cube, round]);
    let by_title: Vec<i64> = db
        .search_diagrams("cube", "All", "All", &RangeFilter::default())
        .unwrap()
        .iter()
        .map(|item| item.id)
        .collect();
    assert_eq!(
        by_title,
        vec![cube],
        "the search must still find it by name"
    );

    // Ignoring hides a design from the catalogue lists without touching the mark ...
    db.set_diagram_ignored(round, true).unwrap();
    assert_eq!(db.all_entry_ids().unwrap(), vec![cube]);
    assert_eq!(db.planner_excluded_ids().unwrap(), BTreeSet::from([cube]));
    // ... and a design can be ignored and excluded at once, then restored independently.
    db.set_diagram_ignored(cube, true).unwrap();
    assert_eq!(db.all_entry_ids().unwrap(), Vec::<i64>::new());
    assert_eq!(db.planner_excluded_ids().unwrap(), BTreeSet::from([cube]));
    db.set_diagram_ignored(cube, false).unwrap();
    db.set_planner_excluded(cube, false).unwrap();
    assert_eq!(db.all_entry_ids().unwrap(), vec![cube]);
    assert!(db.planner_excluded_ids().unwrap().is_empty());
}

/// The library list reads the mark on a read-only connection (and degrades to "none
/// excluded" if that fails), so both readers must work there while the writer refuses.
#[test]
fn the_readers_work_on_a_read_only_connection_and_the_writer_refuses() {
    let path = temp_db_path("planner_exclusions_read_only");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create fresh db");
    let a = save_entry(&db, "Alpha");
    let b = save_entry(&db, "Beta");
    db.set_planner_excluded(a, true).unwrap();

    let ro = Database::open_read_only(path.to_str().unwrap()).expect("open read-only");
    assert_eq!(ro.planner_excluded_ids().unwrap(), BTreeSet::from([a]));
    assert_eq!(
        ro.planner_excluded_among(&[a, b]).unwrap(),
        BTreeSet::from([a])
    );
    assert!(ro.set_planner_excluded(b, true).is_err());
    assert!(ro.set_planner_excluded(a, false).is_err());
    assert_eq!(
        db.planner_excluded_ids().unwrap(),
        BTreeSet::from([a]),
        "the refused writes must not have changed anything"
    );

    drop(ro);
    drop(db);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}
