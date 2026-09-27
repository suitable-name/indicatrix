//! Tests for `Search`/`SearchPage` and the pure [`sort_order_from_wire`] mapping: the
//! multi-row search/paging side of the library protocol, as opposed to `tests::design`'s
//! single-record lookup tests.

use super::fixtures::populated_temp_db;
use crate::serve::library::{handle_request, handlers::sort_order_from_wire};
use indicatrix_net::library::{
    LibraryRequest, LibraryResponse, PerformanceAggregateWire, PerformanceBoundWire,
    PerformanceFilterWire, PerformanceMetricWire, RangeFilterWire, SortOrderWire,
};
use indicatrix_vault::{
    db::sqlite::{Database, SortOrder},
    model::entry::FacetDiagramEntry,
};

#[test]
fn search_returns_the_seeded_design_with_a_version_hash() {
    let path = populated_temp_db();
    let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

    let response = handle_request(
        &LibraryRequest::Search {
            query: "Round".to_string(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire::default(),
            order: SortOrderWire::default(),
            tag_filter: None,
        },
        &db,
    );
    let LibraryResponse::SearchResults {
        items,
        excluded_for_missing_curves,
    } = response
    else {
        panic!("expected SearchResults, got {response:?}");
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title, "Round Brilliant");
    assert_ne!(items[0].version, [0u8; 32]);
    assert!(!items[0].ignored);
    assert_eq!(
        excluded_for_missing_curves, 0,
        "no performance filter was active, so nothing can have been excluded for lacking curves"
    );

    drop(db);
    std::fs::remove_file(&path).ok();
}

#[test]
fn two_designs_with_different_fields_get_different_version_hashes() {
    let path = populated_temp_db();
    let db = Database::new(Some(path.to_str().unwrap())).unwrap();
    db.save_diagram_entry(
        &FacetDiagramEntry {
            title: "Emerald Cut".to_string(),
            url: "https://example.test/diagram/2".to_string(),
            design_id: "EC-1".to_string(),
        },
        "facetdiagrams.org",
    )
    .unwrap();
    drop(db);

    let ro = Database::open_read_only(path.to_str().unwrap()).unwrap();
    let response = handle_request(
        &LibraryRequest::Search {
            query: String::new(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire::default(),
            order: SortOrderWire::default(),
            tag_filter: None,
        },
        &ro,
    );
    let LibraryResponse::SearchResults { items, .. } = response else {
        panic!("expected SearchResults, got {response:?}");
    };
    assert_eq!(items.len(), 2);
    assert_ne!(items[0].version, items[1].version);

    drop(ro);
    std::fs::remove_file(&path).ok();
}

#[test]
fn search_page_returns_no_next_cursor_when_the_page_is_not_full() {
    let path = populated_temp_db();
    let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

    let response = handle_request(
        &LibraryRequest::SearchPage {
            query: String::new(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire::default(),
            cursor: None,
        },
        &db,
    );
    let LibraryResponse::SearchResultsPage {
        results,
        next_cursor,
        excluded_for_missing_curves,
    } = response
    else {
        panic!("expected SearchResultsPage, got {response:?}");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(
        next_cursor, None,
        "one row is far short of a full page -- there is nothing more to fetch"
    );
    assert_eq!(
        excluded_for_missing_curves, 0,
        "no performance filter was active on this request"
    );

    drop(db);
    std::fs::remove_file(&path).ok();
}

#[test]
fn search_page_cursor_excludes_rows_already_returned_by_an_earlier_page() {
    let path = populated_temp_db();
    let db = Database::new(Some(path.to_str().unwrap())).unwrap();
    let second_entry_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Emerald Cut".to_string(),
                url: "https://example.test/diagram/2".to_string(),
                design_id: "EC-1".to_string(),
            },
            "facetdiagrams.org",
        )
        .unwrap();
    drop(db);

    let ro = Database::open_read_only(path.to_str().unwrap()).unwrap();

    // First page with no cursor sees both designs.
    let first = handle_request(
        &LibraryRequest::SearchPage {
            query: String::new(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire::default(),
            cursor: None,
        },
        &ro,
    );
    let LibraryResponse::SearchResultsPage {
        results: first_results,
        ..
    } = first
    else {
        panic!("expected SearchResultsPage, got {first:?}");
    };
    assert_eq!(first_results.len(), 2);
    let first_id = first_results[0].entry_id;

    // Re-requesting with that row's id as cursor must see only what came after it.
    let second = handle_request(
        &LibraryRequest::SearchPage {
            query: String::new(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire::default(),
            cursor: Some(first_id),
        },
        &ro,
    );
    let LibraryResponse::SearchResultsPage {
        results: second_results,
        next_cursor,
        ..
    } = second
    else {
        panic!("expected SearchResultsPage, got {second:?}");
    };
    assert_eq!(second_results.len(), 1);
    assert_eq!(second_results[0].entry_id, second_entry_id);
    assert_eq!(next_cursor, None);

    drop(ro);
    std::fs::remove_file(&path).ok();
}

#[test]
fn search_with_an_out_of_range_tilt_radius_is_rejected_without_touching_the_database() {
    let path = populated_temp_db();
    let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

    let response = handle_request(
        &LibraryRequest::Search {
            query: String::new(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire {
                performance: vec![PerformanceFilterWire {
                    metric: PerformanceMetricWire::Windowing,
                    bound: PerformanceBoundWire::AtMost(20.0),
                    tilt_radius_deg: 91.0, // out of range
                    aggregate: PerformanceAggregateWire::Worst,
                }],
                ..RangeFilterWire::default()
            },
            order: SortOrderWire::default(),
            tag_filter: None,
        },
        &db,
    );
    let LibraryResponse::Error(err) = response else {
        panic!("expected Error, got {response:?}");
    };
    assert_eq!(
        err.code,
        crate::serve::library::LIBRARY_VALIDATION_ERROR_CODE
    );

    drop(db);
    std::fs::remove_file(&path).ok();
}

#[test]
fn search_with_a_valid_performance_filter_reaches_the_database() {
    let path = populated_temp_db();
    let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

    // A valid-but-unsatisfiable filter must reach the database, not be rejected.
    let response = handle_request(
        &LibraryRequest::Search {
            query: String::new(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire {
                performance: vec![PerformanceFilterWire {
                    metric: PerformanceMetricWire::Brilliance,
                    bound: PerformanceBoundWire::AtLeast(0.0),
                    tilt_radius_deg: 45.0,
                    aggregate: PerformanceAggregateWire::Worst,
                }],
                ..RangeFilterWire::default()
            },
            order: SortOrderWire::default(),
            tag_filter: None,
        },
        &db,
    );
    let LibraryResponse::SearchResults {
        items,
        excluded_for_missing_curves,
    } = response
    else {
        panic!("expected SearchResults, got {response:?}");
    };
    assert!(
        items.is_empty(),
        "the seeded design has no tilt curves, so it can never satisfy an active \
         performance filter"
    );
    // The design matched the otherwise-unfiltered search but was excluded for
    // lacking tilt curves; a remote caller must be told so, not just see "no match".
    assert_eq!(
        excluded_for_missing_curves, 1,
        "the seeded design was excluded specifically for lacking tilt curves"
    );

    drop(db);
    std::fs::remove_file(&path).ok();
}

/// A design marked ignored locally must come back from a `Search` reply with
/// `DesignSummary::ignored == true`.
#[test]
fn search_reports_an_ignored_design_as_ignored_when_include_ignored_is_set() {
    let path = populated_temp_db();
    let db = Database::new(Some(path.to_str().unwrap())).unwrap();
    db.set_diagram_ignored(1, true).unwrap();
    drop(db);

    let ro = Database::open_read_only(path.to_str().unwrap()).unwrap();
    let response = handle_request(
        &LibraryRequest::Search {
            query: String::new(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire {
                include_ignored: true,
                ..RangeFilterWire::default()
            },
            order: SortOrderWire::default(),
            tag_filter: None,
        },
        &ro,
    );
    let LibraryResponse::SearchResults { items, .. } = response else {
        panic!("expected SearchResults, got {response:?}");
    };
    assert_eq!(items.len(), 1);
    assert!(items[0].ignored, "the design was just marked ignored");

    drop(ro);
    std::fs::remove_file(&path).ok();
}

/// A `Search` with `order: Title` must return results sorted by title,
/// not the catalogue's insertion (id) order -- `populated_temp_db` seeds "Round
/// Brilliant" first, so two more titles are added here specifically out of
/// alphabetical order to prove the sort, not just pass by coincidence.
#[test]
fn search_with_title_order_returns_titles_sorted() {
    let path = populated_temp_db();
    let db = Database::new(Some(path.to_str().unwrap())).unwrap();
    for (title, design_id) in [("Zircon Cut", "ZC-1"), ("Asscher Cut", "AC-1")] {
        db.save_diagram_entry(
            &FacetDiagramEntry {
                title: title.to_string(),
                url: format!("https://example.test/diagram/{design_id}"),
                design_id: design_id.to_string(),
            },
            "facetdiagrams.org",
        )
        .unwrap();
    }
    drop(db);

    let ro = Database::open_read_only(path.to_str().unwrap()).unwrap();
    let response = handle_request(
        &LibraryRequest::Search {
            query: String::new(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire::default(),
            order: SortOrderWire::Title,
            tag_filter: None,
        },
        &ro,
    );
    let LibraryResponse::SearchResults { items, .. } = response else {
        panic!("expected SearchResults, got {response:?}");
    };
    let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(
        titles,
        vec!["Asscher Cut", "Round Brilliant", "Zircon Cut"],
        "results must come back alphabetically by title, not insertion order"
    );

    drop(ro);
    std::fs::remove_file(&path).ok();
}

/// A `Search` whose `tag_filter` names a tag one seeded design carries
/// (and the other does not) must restrict the results to that design only.
#[test]
fn search_with_tag_filter_restricts_to_designs_carrying_that_tag() {
    let path = populated_temp_db();
    let db = Database::new(Some(path.to_str().unwrap())).unwrap();
    let tagged_entry_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Emerald Cut".to_string(),
                url: "https://example.test/diagram/2".to_string(),
                design_id: "EC-1".to_string(),
            },
            "facetdiagrams.org",
        )
        .unwrap();
    db.add_tag_to_entry(tagged_entry_id, "Favorites").unwrap();
    drop(db);

    let ro = Database::open_read_only(path.to_str().unwrap()).unwrap();
    let response = handle_request(
        &LibraryRequest::Search {
            query: String::new(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire::default(),
            order: SortOrderWire::default(),
            tag_filter: Some("Favorites".to_string()),
        },
        &ro,
    );
    let LibraryResponse::SearchResults { items, .. } = response else {
        panic!("expected SearchResults, got {response:?}");
    };
    assert_eq!(
        items.len(),
        1,
        "only the tagged design should match, not the seeded untagged one too"
    );
    assert_eq!(items[0].entry_id, tagged_entry_id);
    assert_eq!(items[0].title, "Emerald Cut");

    drop(ro);
    std::fs::remove_file(&path).ok();
}

/// A `tag_filter` naming a tag this worker's catalogue has never seen
/// must resolve to no restriction (every design matches), the same tolerance
/// `apps/indicatrix-cut`'s own tag-chip lookup has for an unknown tag -- not an
/// error, and not "match nothing".
#[test]
fn search_with_an_unknown_tag_name_applies_no_restriction() {
    let path = populated_temp_db();
    let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

    let response = handle_request(
        &LibraryRequest::Search {
            query: String::new(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire::default(),
            order: SortOrderWire::default(),
            tag_filter: Some("Nonexistent Tag".to_string()),
        },
        &db,
    );
    let LibraryResponse::SearchResults { items, .. } = response else {
        panic!("expected SearchResults, got {response:?}");
    };
    assert_eq!(
        items.len(),
        1,
        "an unknown tag name must not exclude the seeded design"
    );

    drop(db);
    std::fs::remove_file(&path).ok();
}

/// [`sort_order_from_wire`] maps all four [`SortOrderWire`] variants to the matching
/// [`SortOrder`]. A pure function, so unlike the sibling `search_with_*`
/// tests above (which additionally prove `Title` drives a real sorted query) this
/// needs no database fixture -- it just pins the mapping itself, including the two
/// variants (`Newest`, `RecentlyEdited`) no integration test above exercises.
#[test]
fn sort_order_from_wire_maps_every_variant() {
    assert_eq!(
        sort_order_from_wire(SortOrderWire::CatalogueOrder),
        SortOrder::CatalogueOrder
    );
    assert_eq!(sort_order_from_wire(SortOrderWire::Title), SortOrder::Title);
    assert_eq!(
        sort_order_from_wire(SortOrderWire::Newest),
        SortOrder::Newest
    );
    assert_eq!(
        sort_order_from_wire(SortOrderWire::RecentlyEdited),
        SortOrder::RecentlyEdited
    );
}
