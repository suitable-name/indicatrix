//! The run orchestration: candidates, the cache notes in the summary, gathering cached
//! designs, the id sources and the model check.

use super::{
    ALL_EXCLUDED_MESSAGE, CANCELLING_STAGE, Candidates, FilterSnapshot, IdSource, Reporter,
    cache_note, candidates_from, concave_note, excluded_note, gather, outline_note, summary_text,
    unmeasured_note, validate_model, without_excluded,
};
use indicatrix_cut_core::rough_plan::{BoxFace, CandidateDesign, RoughBase, RoughCut, RoughModel};
use indicatrix_vault::{
    db::sqlite::Database,
    model::{
        entry::FacetingDiagramEntry,
        filter::RangeFilter,
        solid_extents::{SolidExtents, SolidExtentsSource, StoredSolidExtents},
        solid_hull::SolidHull,
    },
};
use slint::Weak;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

fn extents(width: f64, length: f64, height: f64, volume: f64) -> SolidExtents {
    SolidExtents {
        width_caliper: width,
        length_caliper: length,
        width_axis: width,
        length_axis: length,
        height,
        volume,
    }
}

fn row(source: SolidExtentsSource, extents: Option<SolidExtents>) -> StoredSolidExtents {
    StoredSolidExtents { extents, source }
}

#[test]
fn candidates_are_the_usable_design_file_rows_in_entry_id_order_with_width_below_length() {
    let stored = BTreeMap::from([
        (
            9,
            row(
                SolidExtentsSource::DesignFile,
                Some(extents(3.0, 1.0, 0.7, 1.4)),
            ),
        ),
        (
            2,
            row(
                SolidExtentsSource::DesignFile,
                Some(extents(1.0, 2.0, 0.5, 0.8)),
            ),
        ),
        (
            3,
            row(
                SolidExtentsSource::AngleTable,
                Some(extents(1.0, 2.0, 0.5, 0.8)),
            ),
        ),
        (4, row(SolidExtentsSource::DesignFile, None)),
        (5, row(SolidExtentsSource::Unbounded, None)),
    ]);
    assert_eq!(
        candidates_from(&stored),
        vec![
            CandidateDesign {
                entry_id: 2,
                width: 1.0,
                length: 2.0,
                height: 0.5,
                volume: 0.8,
            },
            CandidateDesign {
                entry_id: 9,
                width: 1.0,
                length: 3.0,
                height: 0.7,
                volume: 1.4,
            },
        ]
    );
}

#[test]
fn candidates_with_a_non_finite_or_non_positive_figure_are_left_out() {
    let bad = [
        extents(f64::NAN, 2.0, 0.5, 0.8),
        extents(1.0, f64::INFINITY, 0.5, 0.8),
        extents(1.0, 2.0, 0.0, 0.8),
        extents(1.0, 2.0, 0.5, -0.8),
    ];
    let mut stored: BTreeMap<i64, StoredSolidExtents> = bad
        .into_iter()
        .zip(1..)
        .map(|(e, id)| (id, row(SolidExtentsSource::DesignFile, Some(e))))
        .collect();
    stored.insert(
        10,
        row(
            SolidExtentsSource::DesignFile,
            Some(extents(1.0, 2.0, 0.5, 0.8)),
        ),
    );
    let ids: Vec<i64> = candidates_from(&stored)
        .iter()
        .map(|c| c.entry_id)
        .collect();
    assert_eq!(ids, vec![10]);
}

fn reporter(cancel: &Arc<AtomicBool>) -> Reporter {
    Reporter::new(Weak::default(), Arc::clone(cancel))
}

#[test]
fn once_cancel_is_requested_every_progress_push_reads_cancelling() {
    let cancel = Arc::new(AtomicBool::new(false));
    let reporter = reporter(&cancel);
    assert_eq!(reporter.shown_stage("Refining"), "Refining");
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(reporter.shown_stage("Refining"), CANCELLING_STAGE);
    assert_eq!(
        reporter.shown_stage("Fitting single stones: exact search 1 / 48"),
        "Cancelling..."
    );
}

#[test]
fn an_abort_from_inside_the_run_reads_as_a_cancel_to_every_lane() {
    let cancel = Arc::new(AtomicBool::new(false));
    let reporter = reporter(&cancel);
    assert!(!reporter.cancelled());
    reporter.abort();
    assert!(reporter.cancelled());
    assert!(
        cancel.load(Ordering::Relaxed),
        "the shared flag is the one set"
    );
}

#[test]
fn gathering_fully_cached_designs_reads_them_back_and_scans_nothing() {
    let db = Mutex::new(Database::new(Some(":memory:")).expect("in-memory database opens"));
    let hull = SolidHull {
        vertices: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ],
    };
    let (bounded, unbounded) = {
        let guard = db.lock().expect("lock");
        let add = |label: &str| {
            guard
                .save_diagram_entry(
                    &FacetingDiagramEntry {
                        title: format!("Gather {label}"),
                        url: format!("local://gather-{label}.asc"),
                        design_id: String::new(),
                    },
                    "local-import",
                )
                .expect("entry saved")
        };
        let (bounded, unbounded) = (add("bounded"), add("unbounded"));
        guard
            .save_solid_extents_and_hull(
                bounded,
                Some(extents(1.0, 2.0, 0.5, 0.8)),
                SolidExtentsSource::DesignFile,
                Some(&hull),
                1,
            )
            .expect("bounded saved");
        guard
            .save_solid_extents_and_hull(unbounded, None, SolidExtentsSource::Unbounded, None, 1)
            .expect("unbounded saved");
        drop(guard);
        (bounded, unbounded)
    };

    let cancel = Arc::new(AtomicBool::new(false));
    let Ok(gathered) = gather(&db, &reporter(&cancel), &[bounded, unbounded]) else {
        panic!("nothing to measure cannot stop the gathering");
    };
    assert!(!gathered.scanned, "everything was cached");
    assert_eq!(gathered.save_failures, 0);
    assert_eq!(
        gathered.stored.keys().copied().collect::<Vec<_>>(),
        vec![bounded, unbounded]
    );
    assert_eq!(
        gathered.stored[&bounded].extents,
        Some(extents(1.0, 2.0, 0.5, 0.8))
    );
    assert_eq!(gathered.stored[&unbounded].extents, None);
    assert_eq!(gathered.hulls.len(), 1);
    assert_eq!(gathered.hulls[&bounded].vertices, hull.vertices);
}

#[test]
fn the_library_source_lists_every_entry_sorted_and_says_when_there_is_none() {
    let db = Mutex::new(Database::new(Some(":memory:")).expect("in-memory database opens"));
    let message = IdSource::Library
        .resolve(&db)
        .expect_err("an empty library has nothing to plan");
    assert_eq!(message, "The library has no designs yet.");

    let added: Vec<i64> = {
        let guard = db.lock().expect("lock");
        ["a", "b", "c"]
            .iter()
            .map(|label| {
                guard
                    .save_diagram_entry(
                        &FacetingDiagramEntry {
                            title: format!("Source {label}"),
                            url: format!("local://source-{label}.asc"),
                            design_id: String::new(),
                        },
                        "local-import",
                    )
                    .expect("entry saved")
            })
            .collect()
    };
    let mut expected = added;
    expected.sort_unstable();
    assert_eq!(
        IdSource::Library.resolve(&db),
        Ok(Candidates {
            ids: expected,
            excluded: 0
        })
    );
}

/// An in-memory database holding one design per label, and the ids it gave them.
fn database_with_designs(labels: &[&str]) -> (Mutex<Database>, Vec<i64>) {
    let db = Mutex::new(Database::new(Some(":memory:")).expect("in-memory database opens"));
    let ids = {
        let guard = db.lock().expect("lock");
        labels
            .iter()
            .map(|label| {
                guard
                    .save_diagram_entry(
                        &FacetingDiagramEntry {
                            title: format!("Candidate {label}"),
                            url: format!("local://candidate-{label}.asc"),
                            design_id: String::new(),
                        },
                        "local-import",
                    )
                    .expect("entry saved")
            })
            .collect()
    };
    (db, ids)
}

/// A library filter that matches every design that is not ignored.
fn unfiltered() -> FilterSnapshot {
    FilterSnapshot {
        search: String::new(),
        shape: "All".to_string(),
        gear: "All".to_string(),
        range: RangeFilter::default(),
        local_only: false,
        tag_name: None,
        id_filter: None,
    }
}

fn exclude(db: &Mutex<Database>, ids: &[i64]) {
    let guard = db.lock().expect("lock");
    for &id in ids {
        guard
            .set_planner_excluded(id, true)
            .expect("the design exists");
    }
}

#[test]
fn an_excluded_design_is_dropped_before_anything_is_measured_and_counted() {
    let (db, ids) = database_with_designs(&["a", "b", "c"]);
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]), "ascending");
    exclude(&db, &ids[1..2]);
    let expected = Candidates {
        ids: vec![ids[0], ids[2]],
        excluded: 1,
    };
    assert_eq!(IdSource::Library.resolve(&db), Ok(expected));
    // The filter source subtracts the same designs.
    let filtered = IdSource::Filter(Box::new(unfiltered())).resolve(&db);
    assert_eq!(
        filtered,
        Ok(Candidates {
            ids: vec![ids[0], ids[2]],
            excluded: 1,
        })
    );
    // Restoring the design brings it back.
    db.lock()
        .expect("lock")
        .set_planner_excluded(ids[1], false)
        .expect("restored");
    assert_eq!(
        IdSource::Library.resolve(&db),
        Ok(Candidates { ids, excluded: 0 })
    );
}

#[test]
fn a_source_whose_every_design_is_excluded_says_so_instead_of_naming_an_empty_library() {
    let (db, ids) = database_with_designs(&["a", "b"]);
    exclude(&db, &ids);
    assert_eq!(
        IdSource::Library.resolve(&db),
        Err(ALL_EXCLUDED_MESSAGE.to_string())
    );
    assert_eq!(
        IdSource::Filter(Box::new(unfiltered())).resolve(&db),
        Err(ALL_EXCLUDED_MESSAGE.to_string())
    );
    assert_eq!(
        ALL_EXCLUDED_MESSAGE,
        "Every candidate design is excluded from planning. Restore one under Candidate \
         designs, or widen the filter."
    );
}

#[test]
fn exclusions_that_match_no_design_of_the_source_change_nothing() {
    let (db, ids) = database_with_designs(&["a", "b"]);
    // An id that is excluded but outside the filtered set is not counted as left out.
    let narrowed = IdSource::Filter(Box::new(FilterSnapshot {
        id_filter: Some(vec![ids[0]]),
        ..unfiltered()
    }));
    exclude(&db, &ids[1..]);
    assert_eq!(
        narrowed.resolve(&db),
        Ok(Candidates {
            ids: vec![ids[0]],
            excluded: 0
        })
    );
}

#[test]
fn removing_the_excluded_ids_keeps_the_order_and_counts_what_went() {
    let excluded: BTreeSet<i64> = BTreeSet::from([3, 7, 40]);
    assert_eq!(
        without_excluded(vec![1, 3, 5, 7, 9], &excluded),
        (vec![1, 5, 9], 2)
    );
    assert_eq!(without_excluded(vec![1, 5], &excluded), (vec![1, 5], 0));
    assert_eq!(without_excluded(vec![3, 7], &excluded), (Vec::new(), 2));
    assert_eq!(
        without_excluded(vec![1, 2], &BTreeSet::new()),
        (vec![1, 2], 0)
    );
    assert_eq!(without_excluded(Vec::new(), &excluded), (Vec::new(), 0));
}

#[test]
fn the_excluded_note_is_empty_for_none_and_counts_otherwise() {
    assert_eq!(excluded_note(0), "");
    assert_eq!(excluded_note(1), "; 1 design excluded from planning");
    assert_eq!(excluded_note(2), "; 2 designs excluded from planning");
    assert_eq!(
        excluded_note(1234),
        "; 1,234 designs excluded from planning"
    );
}

#[test]
fn the_unmeasured_note_is_empty_for_none_and_counts_otherwise() {
    assert_eq!(unmeasured_note(0), "");
    assert_eq!(unmeasured_note(1), "; 1 design could not be measured");
    assert_eq!(
        unmeasured_note(1234),
        "; 1,234 designs could not be measured"
    );
}

#[test]
fn the_concave_note_names_the_designs_skipped_for_unresolvable_tiers() {
    assert_eq!(concave_note(0), "");
    assert_eq!(
        concave_note(1),
        "; 1 design skipped: its concave tiers could not be resolved"
    );
    assert_eq!(
        concave_note(1200),
        "; 1,200 designs skipped: their concave tiers could not be resolved"
    );
}

#[test]
fn the_cache_note_names_the_designs_whose_measurement_could_not_be_kept() {
    assert_eq!(cache_note(0), "");
    assert_eq!(cache_note(1), "; cache could not be saved (1 design)");
    assert_eq!(cache_note(2), "; cache could not be saved (2 designs)");
    assert_eq!(
        cache_note(1500),
        "; cache could not be saved (1,500 designs)"
    );
}

#[test]
fn the_outline_note_names_the_designs_planned_as_boxes_only() {
    assert_eq!(outline_note(0), "");
    assert_eq!(
        outline_note(1),
        "; 1 design without an outline was only planned as a box"
    );
    assert_eq!(
        outline_note(12),
        "; 12 designs without an outline were only planned as boxes"
    );
}

#[test]
fn the_summary_names_the_layouts_the_designs_and_the_time() {
    let took = Duration::from_millis(3210);
    assert_eq!(
        summary_text(10, 2431, took, ""),
        "10 layouts from 2,431 designs -- 3.2 s"
    );
    assert_eq!(
        summary_text(1, 5, took, "; 1 design could not be measured"),
        "1 layout from 5 designs -- 3.2 s; 1 design could not be measured"
    );
    assert!(summary_text(0, 5, took, "").starts_with("No layout fits"));
}

#[test]
fn the_excluded_clause_leads_the_notes_of_every_summary() {
    let took = Duration::from_millis(3210);
    let note = format!("{}{}", excluded_note(2), unmeasured_note(1));
    assert_eq!(
        summary_text(3, 40, took, &note),
        "3 layouts from 40 designs -- 3.2 s; 2 designs excluded from planning\
         ; 1 design could not be measured"
    );
    let none_fit = summary_text(0, 40, took, &note);
    assert!(none_fit.starts_with("No layout fits"), "{none_fit}");
    assert!(
        none_fit.ends_with(
            "small enough; 2 designs excluded from planning; 1 design could not be measured."
        ),
        "{none_fit}"
    );
}

fn block_with(cuts: Vec<RoughCut>) -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: 10.0,
            y_mm: 8.0,
            z_mm: 6.0,
        },
        cuts,
    )
}

#[test]
fn a_valid_model_passes_and_an_invalid_one_says_why_instead_of_planning_nothing() {
    assert_eq!(validate_model(&block_with(Vec::new())), Ok(()));

    let too_deep = RoughCut::Edge {
        faces: [BoxFace::Top, BoxFace::Front],
        setbacks_mm: [50.0, 1.0],
    };
    let message = validate_model(&block_with(vec![too_deep])).unwrap_err();
    assert!(message.contains("Cut 1"), "{message}");
    assert!(message.contains("setback"), "{message}");

    let flat = RoughModel::new(
        RoughBase::Block {
            x_mm: 10.0,
            y_mm: 0.0,
            z_mm: 6.0,
        },
        Vec::new(),
    );
    assert!(validate_model(&flat).is_err());
}
