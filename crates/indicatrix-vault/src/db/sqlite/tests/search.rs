//! The library search/filter surface: free-text matching (including the notes
//! `EXISTS` subquery and `LIKE`-wildcard escaping), range/tag/ignored-flag filtering,
//! keyset pagination, attribute-range percentile bounds, and the combined
//! display-plus-count query.

use super::{super::*, fixtures::temp_db_path, performance::flat_tilt_curves};

/// Builds a `Database` (fresh temp file, migrated schema) and inserts one diagram per
/// `(title, shape, ri, lw, volume, facets_count)` tuple via the public save API, for
/// exercising `search_diagrams`/`get_attribute_ranges`.
fn seeded_db(rows: &[(&str, &str, &str, &str, &str, &str)]) -> Database {
    let path = temp_db_path("search");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    for (title, shape, ri, lw, volume, facets_count) in rows {
        let entry = FacetingDiagramEntry {
            title: (*title).to_string(),
            url: format!("https://example.test/{title}"),
            design_id: String::new(),
        };
        let entry_id = db
            .save_diagram_entry(&entry, LEGACY_SOURCE_ID)
            .expect("save entry");
        let detail = FacetingDiagramDetail {
            shape: Some((*shape).to_string()),
            refractive_index: Some((*ri).to_string()),
            lw_ratio: Some((*lw).to_string()),
            volume: Some((*volume).to_string()),
            facets_count: Some((*facets_count).to_string()),
            index_gear: Some("96".to_string()),
            ..Default::default()
        };
        db.save_diagram_detail(&detail, entry_id)
            .expect("save detail");
    }
    db
}

/// The search tooltip/placeholder promises "title,
/// designer or notes"; this asserts the predicate honors the "notes"
/// third of that claim -- a term that appears ONLY in a tier's `angle_settings.notes`
/// (not in the title or `designer_info`) must still surface the design, and a design
/// with no such note must not be a false positive.
#[test]
fn search_diagrams_matches_a_term_found_only_in_tier_notes() {
    let path = temp_db_path("search_notes");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");

    let noted_entry = db
        .save_diagram_entry(
            &FacetingDiagramEntry {
                title: "Noted Design".to_string(),
                url: "local://noted.asc".to_string(),
                design_id: String::new(),
            },
            LEGACY_SOURCE_ID,
        )
        .expect("save noted entry");
    db.save_diagram_detail(
        &FacetingDiagramDetail {
            angle_settings_table: vec![crate::model::angle::AngleSetting {
                order_index: 0,
                facet: "P1".to_string(),
                angle: "41".to_string(),
                index: "0".to_string(),
                notes: "cut to a client's heirloom spec".to_string(),
            }],
            ..Default::default()
        },
        noted_entry,
    )
    .expect("save noted detail");

    let plain_entry = db
        .save_diagram_entry(
            &FacetingDiagramEntry {
                title: "Plain Design".to_string(),
                url: "local://plain.asc".to_string(),
                design_id: String::new(),
            },
            LEGACY_SOURCE_ID,
        )
        .expect("save plain entry");
    db.save_diagram_detail(&FacetingDiagramDetail::default(), plain_entry)
        .expect("save plain detail");

    let results = db
        .search_diagrams("heirloom", "All", "All", &RangeFilter::default())
        .expect("search must succeed");
    assert_eq!(
        results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        vec!["Noted Design"],
        "a term found only in a tier's notes must match, and only that design"
    );

    let _ = std::fs::remove_file(&path);
}

/// An unescaped `_`/`%` in the search box is a SQL `LIKE` wildcard,
/// not the literal character it looks like -- on the real catalogue that made a bare
/// `"_"` query match all 3,299 rows. `Database::search_diagrams` must treat `_` (and
/// `%`) as literal text, matching only a title that actually contains one.
#[test]
fn search_diagrams_treats_underscore_and_percent_as_literal_characters() {
    let db = Database::new(Some(":memory:")).expect("create in-memory db");

    let underscore_entry = db
        .save_diagram_entry(
            &FacetingDiagramEntry {
                title: "Round_Brilliant".to_string(),
                url: "local://has-underscore.asc".to_string(),
                design_id: String::new(),
            },
            LEGACY_SOURCE_ID,
        )
        .expect("save underscore entry");
    db.save_diagram_detail(&FacetingDiagramDetail::default(), underscore_entry)
        .expect("save underscore detail");

    let plain_entry = db
        .save_diagram_entry(
            &FacetingDiagramEntry {
                title: "Round Brilliant".to_string(),
                url: "local://no-underscore.asc".to_string(),
                design_id: String::new(),
            },
            LEGACY_SOURCE_ID,
        )
        .expect("save plain entry");
    db.save_diagram_detail(&FacetingDiagramDetail::default(), plain_entry)
        .expect("save plain detail");

    let underscore_results = db
        .search_diagrams("_", "All", "All", &RangeFilter::default())
        .expect("search must succeed");
    assert_eq!(
        underscore_results
            .iter()
            .map(|r| r.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Round_Brilliant"],
        "an unescaped '_' must match only a literal underscore, not every title"
    );

    let percent_entry = db
        .save_diagram_entry(
            &FacetingDiagramEntry {
                title: "50% Off Design".to_string(),
                url: "local://has-percent.asc".to_string(),
                design_id: String::new(),
            },
            LEGACY_SOURCE_ID,
        )
        .expect("save percent entry");
    db.save_diagram_detail(&FacetingDiagramDetail::default(), percent_entry)
        .expect("save percent detail");

    let percent_results = db
        .search_diagrams("50%", "All", "All", &RangeFilter::default())
        .expect("search must succeed");
    assert_eq!(
        percent_results
            .iter()
            .map(|r| r.title.as_str())
            .collect::<Vec<_>>(),
        vec!["50% Off Design"],
        "a literal '%' in the query must not act as an open wildcard"
    );
}

#[test]
fn range_query_returns_only_rows_within_known_bounds() {
    let db = seeded_db(&[
        ("Low", "Round", "1.50", "1.00", "0.10", "50"),
        ("Mid", "Round", "1.76", "1.10", "0.20", "60+8"),
        ("High", "Round", "2.40", "1.20", "0.30", "70"),
    ]);

    // RI in [1.6, 2.0] matches only "Mid" (1.76).
    let range = RangeFilter {
        ri_min: Some(1.6),
        ri_max: Some(2.0),
        ..Default::default()
    };
    let results = db.search_diagrams("", "All", "All", &range).unwrap();
    assert_eq!(
        results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        vec!["Mid"]
    );

    // facets in [55, 65] matches only "Mid" (facets = 60).
    let range = RangeFilter {
        facets_min: Some(55),
        facets_max: Some(65),
        ..Default::default()
    };
    let results = db.search_diagrams("", "All", "All", &range).unwrap();
    assert_eq!(
        results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        vec!["Mid"]
    );

    // No range filter returns everything.
    let results = db
        .search_diagrams("", "All", "All", &RangeFilter::default())
        .unwrap();
    assert_eq!(results.len(), 3);
}

/// The library's "regenerate previews/tilt curves for the whole filtered set" batch
/// action (`apps/indicatrix-cut`'s `gui::library::diagram_list`) needs the exact same
/// match set `search_diagrams` itself would show, just uncapped and id-only.
#[test]
fn matching_entry_ids_agrees_with_search_diagrams_on_which_rows_match() {
    let db = seeded_db(&[
        ("Low", "Round", "1.50", "1.00", "0.10", "50"),
        ("Mid", "Round", "1.76", "1.10", "0.20", "60"),
        ("High", "Round", "2.40", "1.20", "0.30", "70"),
    ]);
    let range = RangeFilter {
        ri_min: Some(1.6),
        ri_max: Some(2.0),
        ..Default::default()
    };

    let expected: Vec<i64> = db
        .search_diagrams("", "All", "All", &range)
        .unwrap()
        .into_iter()
        .map(|item| item.id)
        .collect();
    let ids = db
        .matching_entry_ids("", "All", "All", &range, DisplayFilters::default())
        .unwrap();

    assert_eq!(ids.len(), 1, "only Mid's RI (1.76) falls in [1.6, 2.0]");
    assert_eq!(ids, expected);
}

#[test]
fn matching_entry_ids_is_empty_for_a_range_no_row_satisfies() {
    let db = seeded_db(&[("Only", "Round", "1.76", "1.10", "0.20", "60")]);
    let range = RangeFilter {
        ri_min: Some(9.0),
        ri_max: Some(9.5),
        ..Default::default()
    };
    let ids = db
        .matching_entry_ids("", "All", "All", &range, DisplayFilters::default())
        .unwrap();
    assert_eq!(ids, [] as [i64; 0]);
}

#[test]
fn inverted_range_bounds_return_nothing_not_everything() {
    let db = seeded_db(&[
        ("Low", "Round", "1.50", "1.00", "0.10", "50"),
        ("Mid", "Round", "1.76", "1.10", "0.20", "60+8"),
        ("High", "Round", "2.40", "1.20", "0.30", "70"),
    ]);

    // min > max must return empty, not silently act as if unfiltered.
    let range = RangeFilter {
        ri_min: Some(2.0),
        ri_max: Some(1.0),
        ..Default::default()
    };
    let results = db.search_diagrams("", "All", "All", &range).unwrap();
    assert!(
        results.is_empty(),
        "inverted RI bounds must return no rows, got {results:?}"
    );

    let range = RangeFilter {
        facets_min: Some(90),
        facets_max: Some(10),
        ..Default::default()
    };
    let results = db.search_diagrams("", "All", "All", &range).unwrap();
    assert!(
        results.is_empty(),
        "inverted facets bounds must return no rows, got {results:?}"
    );
}

#[test]
fn search_diagrams_page_walks_the_whole_result_set_via_keyset_cursor() {
    let db = seeded_db(&[
        ("A", "Round", "1.50", "1.00", "0.10", "50"),
        ("B", "Round", "1.55", "1.00", "0.10", "50"),
        ("C", "Round", "1.60", "1.00", "0.10", "50"),
        ("D", "Round", "1.65", "1.00", "0.10", "50"),
        ("E", "Round", "1.70", "1.00", "0.10", "50"),
    ]);

    let mut collected = Vec::new();
    let mut after_id = None;
    loop {
        let page = db
            .search_diagrams_page("", "All", "All", &RangeFilter::default(), after_id, 2)
            .unwrap();
        if page.is_empty() {
            break;
        }
        let full_page = page.len() == 2;
        after_id = page.last().map(|r| r.id);
        collected.extend(page);
        if !full_page {
            break;
        }
    }

    assert_eq!(
        collected
            .iter()
            .map(|r| r.title.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "B", "C", "D", "E"],
        "a multi-page walk must reach every row, in order, including ones past the \
             first page"
    );
    // Strictly increasing ids: no page boundary skipped or duplicated a row.
    for pair in collected.windows(2) {
        assert!(pair[0].id < pair[1].id);
    }

    // Pagination composes with, not changes, what search_diagrams already returns.
    let unpaged = db
        .search_diagrams("", "All", "All", &RangeFilter::default())
        .unwrap();
    assert_eq!(
        collected.iter().map(|r| r.id).collect::<Vec<_>>(),
        unpaged.iter().map(|r| r.id).collect::<Vec<_>>()
    );
}

#[test]
fn search_diagrams_delegates_to_the_first_unpaginated_page() {
    let db = seeded_db(&[
        ("A", "Round", "1.50", "1.00", "0.10", "50"),
        ("B", "Round", "1.55", "1.00", "0.10", "50"),
    ]);

    let via_search = db
        .search_diagrams("", "All", "All", &RangeFilter::default())
        .unwrap();
    let via_page = db
        .search_diagrams_page("", "All", "All", &RangeFilter::default(), None, 1000)
        .unwrap();
    assert_eq!(
        via_search.iter().map(|r| r.id).collect::<Vec<_>>(),
        via_page.iter().map(|r| r.id).collect::<Vec<_>>(),
        "search_diagrams must return exactly what search_diagrams_page(.., None, 1000) does"
    );
}

#[test]
fn get_attribute_ranges_uses_real_min_but_a_percentile_based_max() {
    let db = seeded_db(&[
        ("Low", "Round", "1.50", "1.00", "0.10", "50"),
        ("Mid", "Round", "1.76", "1.10", "0.20", "60+8"),
        ("High", "Round", "2.40", "1.20", "0.30", "70"),
    ]);

    let ranges = db.get_attribute_ranges().unwrap();
    // Minimums are still the real minimums -- only the upper bound changed.
    assert!((ranges.ri.0 - 1.50).abs() < 1e-9);
    assert!((ranges.lw_ratio.0 - 1.00).abs() < 1e-9);
    // p99 (linear interpolation) of 3 sorted points sits at index 1.98: 98% from b to c.
    assert!((ranges.ri.1 - 0.98_f64.mul_add(2.40 - 1.76, 1.76)).abs() < 1e-6);
    assert!((ranges.lw_ratio.1 - 0.98_f64.mul_add(1.20 - 1.10, 1.10)).abs() < 1e-6);
    // Upper bound must not equal the raw max for a right-skewed sample -- the fix's whole point.
    assert!(
        ranges.ri.1 < 2.40,
        "p99 bound must sit below the raw max, got {}",
        ranges.ri.1
    );
    assert!(
        ranges.lw_ratio.1 < 1.20,
        "p99 bound must sit below the raw max, got {}",
        ranges.lw_ratio.1
    );
}

#[test]
fn get_attribute_ranges_does_not_let_a_single_outlier_set_the_scale() {
    // Mirrors the real catalogue's volume column: a tight cluster of normal values
    // plus one physically-impossible outlier that must not drag the slider's usable
    // bound toward it. 200 normal rows keeps the outlier under 1% of the sample.
    let mut owned: Vec<(String, String, String, String, String, String)> = (0..200)
        .map(|i| {
            let vol = f64::from(i).mul_add(0.20 / 199.0, 0.10);
            (
                format!("Normal{i}"),
                "Round".to_string(),
                "1.50".to_string(),
                "1.00".to_string(),
                format!("{vol:.4}"),
                "50".to_string(),
            )
        })
        .collect();
    owned.push((
        "Outlier".to_string(),
        "Round".to_string(),
        "1.50".to_string(),
        "1.00".to_string(),
        "195.0".to_string(),
        "50".to_string(),
    ));
    let rows: Vec<(&str, &str, &str, &str, &str, &str)> = owned
        .iter()
        .map(|(a, b, c, d, e, f)| {
            (
                a.as_str(),
                b.as_str(),
                c.as_str(),
                d.as_str(),
                e.as_str(),
                f.as_str(),
            )
        })
        .collect();
    let db = seeded_db(&rows);

    let ranges = db.get_attribute_ranges().unwrap();
    assert!(
        (ranges.volume.0 - 0.10).abs() < 1e-6,
        "min should be the real min, got {}",
        ranges.volume.0
    );
    assert!(
        ranges.volume.1 < 1.0,
        "a single outlier of 195 must not set the slider's usable max bound, got {}",
        ranges.volume.1
    );

    // Excluded from the slider's scale must not mean excluded from results.
    let results = db
        .search_diagrams("", "All", "All", &RangeFilter::default())
        .unwrap();
    assert!(
        results.iter().any(|r| r.title == "Outlier"),
        "outlier row must still be reachable via unfiltered search"
    );
}

#[test]
fn percentile_of_sorted_interpolates_linearly() {
    let values = [1.0, 2.0, 3.0, 4.0, 5.0];
    assert!((percentile_of_sorted(&values, 0.0) - 1.0).abs() < 1e-9);
    assert!((percentile_of_sorted(&values, 50.0) - 3.0).abs() < 1e-9);
    assert!((percentile_of_sorted(&values, 100.0) - 5.0).abs() < 1e-9);
    // idx = 3.96 -> 96% from values[3]=4.0 to values[4]=5.0.
    assert!((percentile_of_sorted(&values, 99.0) - 4.96).abs() < 1e-9);
}

#[test]
fn percentile_of_sorted_single_value_returns_that_value() {
    assert!((percentile_of_sorted(&[42.0], 99.0) - 42.0).abs() < 1e-9);
}

#[test]
fn get_unique_gears_still_returns_display_strings_after_retype() {
    let db = seeded_db(&[("A", "Round", "1.50", "1.00", "0.10", "50")]);
    let gears = db.get_unique_gears().unwrap();
    assert_eq!(gears, vec!["96".to_string()]);
}

#[test]
fn get_unique_shapes_unions_the_vocabulary_with_real_catalogue_data() {
    let db = seeded_db(&[
        // Already in DEFAULT_SHAPES.
        ("A", "Round", "1.50", "1.00", "0.10", "50"),
        // Real scraped shape not in the canonical list -- must not be dropped.
        ("B", "Portuguese Round", "1.55", "1.00", "0.10", "50"),
    ]);

    let shapes = db.get_unique_shapes().unwrap();

    for shape in DEFAULT_SHAPES {
        assert!(
            shapes.iter().any(|s| s == shape),
            "seeded vocabulary entry '{shape}' must appear in the union, got {shapes:?}"
        );
    }
    assert!(
        shapes.iter().any(|s| s == "Portuguese Round"),
        "a real shape string outside the canonical list must still appear, got {shapes:?}"
    );
    // "Round" must not be duplicated for being in both sources.
    assert_eq!(shapes.iter().filter(|s| s.as_str() == "Round").count(), 1);
    let mut sorted = shapes.clone();
    sorted.sort();
    assert_eq!(shapes, sorted);
}

#[test]
fn search_excludes_ignored_designs_by_default_and_includes_them_when_opted_in() {
    let path = temp_db_path("search_ignored");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let visible_id = db
        .save_diagram_entry(
            &FacetingDiagramEntry {
                title: "Visible".to_string(),
                url: "local://visible.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    let hidden_id = db
        .save_diagram_entry(
            &FacetingDiagramEntry {
                title: "Hidden".to_string(),
                url: "local://hidden.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    db.set_diagram_ignored(hidden_id, true).unwrap();

    let default_results = db
        .search_diagrams("", "All", "All", &RangeFilter::default())
        .unwrap();
    assert!(default_results.iter().any(|r| r.id == visible_id));
    assert!(
        !default_results.iter().any(|r| r.id == hidden_id),
        "an ignored design must be excluded by default"
    );

    let opted_in = RangeFilter {
        include_ignored: true,
        ..Default::default()
    };
    let with_ignored = db.search_diagrams("", "All", "All", &opted_in).unwrap();
    assert!(
        with_ignored.iter().any(|r| r.id == hidden_id),
        "include_ignored: true must bring the ignored design back"
    );
    assert!(with_ignored.iter().any(|r| r.id == visible_id));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn ri_tolerance_composes_with_ri_min_max_by_intersection() {
    let db = seeded_db(&[
        ("Low", "Round", "1.50", "1.00", "0.10", "50"),
        ("Mid", "Round", "1.76", "1.10", "0.20", "60+8"),
        ("High", "Round", "2.40", "1.20", "0.30", "70"),
    ]);

    // Tolerance band (centre 1.76, tolerance 0.05) matches only "Mid".
    let tolerance_only = RangeFilter {
        ri_tolerance: Some((1.76, 0.05)),
        ..Default::default()
    };
    let results = db
        .search_diagrams("", "All", "All", &tolerance_only)
        .unwrap();
    assert_eq!(
        results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        vec!["Mid"]
    );

    // Tolerance band matching "Mid" and "High" ([1.55, 2.45]), intersected with an
    // ri_max excluding "High" -- only "Mid" survives, proving AND not OR.
    let intersected = RangeFilter {
        ri_tolerance: Some((2.0, 0.45)),
        ri_max: Some(2.0),
        ..Default::default()
    };
    let results = db.search_diagrams("", "All", "All", &intersected).unwrap();
    assert_eq!(
        results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        vec!["Mid"]
    );
}

/// The single-pass fix:
/// [`Database::search_diagrams_display_with_count`] must agree, item-for-item and
/// count-for-count, with calling [`Database::search_diagrams_display`] and
/// [`Database::count_matching_diagrams`] separately for the same arguments -- with an
/// active performance filter, which is exactly the case the combined method exists to
/// walk only once instead of twice.
#[test]
fn search_diagrams_display_with_count_agrees_with_the_two_separate_calls() {
    let db = Database::new(Some(":memory:")).expect("create in-memory db");

    // Five designs: three pass a `Windowing <= 20` filter, two don't; one has no
    // curves at all and can never pass.
    for (i, windowing) in [10.0, 15.0, 18.0, 55.0, 90.0].iter().enumerate() {
        let entry_id = db
            .save_diagram_entry(
                &FacetingDiagramEntry {
                    title: format!("Design {i}"),
                    url: format!("local://design-{i}.asc"),
                    design_id: String::new(),
                },
                "local-import",
            )
            .unwrap();
        db.save_tilt_curves(
            entry_id,
            &flat_tilt_curves(*windowing),
            1,
            "test-fingerprint",
            db.entry_updated_at(entry_id).unwrap(),
        )
        .unwrap();
    }
    db.save_diagram_entry(
        &FacetingDiagramEntry {
            title: "No Curves".to_string(),
            url: "local://no-curves.asc".to_string(),
            design_id: String::new(),
        },
        "local-import",
    )
    .unwrap();

    let filter = RangeFilter {
        performance: vec![crate::model::performance::PerformanceFilter {
            metric: crate::model::performance::PerformanceMetric::Windowing,
            bound: crate::model::performance::PerformanceBound::AtMost(20.0),
            tilt_radius_deg: 45.0,
            aggregate: crate::model::performance::PerformanceAggregate::Worst,
        }],
        ..Default::default()
    };

    let separate_display = db
        .search_diagrams_display("", "All", "All", &filter, DisplayFilters::default())
        .unwrap();
    let separate_count = db
        .count_matching_diagrams("", "All", "All", &filter, DisplayFilters::default())
        .unwrap();

    let (combined_display, combined_count) = db
        .search_diagrams_display_with_count("", "All", "All", &filter, DisplayFilters::default())
        .unwrap();

    assert_eq!(
        separate_count, 3,
        "exactly the three windowing<=20 designs match"
    );
    assert_eq!(combined_count, separate_count);
    assert_eq!(
        combined_display
            .items
            .iter()
            .map(|i| i.id)
            .collect::<Vec<_>>(),
        separate_display
            .items
            .iter()
            .map(|i| i.id)
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        combined_display.excluded_for_missing_curves,
        separate_display.excluded_for_missing_curves
    );
}

/// The library search predicate's supporting indexes must exist on a fresh database,
/// and the query planner must actually pick the `angle_settings` one -- see
/// `migrations::SEARCH_INDEXES_SQL`'s own doc comment for the measurements. Without it
/// the notes-matching `EXISTS` subquery degrades to a full scan of a table holding one
/// row per TIER, which froze the library's search box for tens of seconds per keystroke
/// on a 3,299-design catalogue.
#[test]
fn search_indexes_exist_and_the_planner_uses_them() {
    let path = temp_db_path("search_indexes");
    let db = Database::new(Some(path.to_str().unwrap())).expect("fresh database opens");

    for index in [
        "idx_angle_settings_detail_id",
        "idx_diagram_tag_links_tag_id",
    ] {
        let found: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                [index],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(found, 1, "{index} must exist on a fresh database");
    }

    // The decisive half: an index nothing plans against would not have fixed anything.
    // `SEARCH ... USING INDEX` is the plan line that replaced `SCAN a`.
    let plan: Vec<String> = db
        .conn
        .prepare(
            "EXPLAIN QUERY PLAN
             SELECT 1 FROM angle_settings a WHERE a.detail_id = 1 AND a.notes LIKE '%x%'",
        )
        .unwrap()
        .query_map([], |r| r.get::<_, String>(3))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        plan.iter()
            .any(|line| line.contains("idx_angle_settings_detail_id")),
        "the planner must use the detail_id index, got {plan:?}"
    );

    // Reopening runs the migration a second time: `CREATE INDEX IF NOT EXISTS` must be
    // a no-op, not an error.
    drop(db);
    let db2 = Database::new(Some(path.to_str().unwrap())).expect("second open is idempotent");
    drop(db2);
}
