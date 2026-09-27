//! Tilt-performance filtering: a design with no stored curves (or a corrupt curve
//! BLOB) is excluded rather than failing the whole search, multiple filters AND
//! together, and the SQL global-min/max narrowing never disagrees with a brute-force
//! per-design scan across randomised curve data.

use super::{super::*, fixtures::temp_db_path};

/// Builds a flat (every sample identical) `TiltPerformanceCurves` for `value`, used by
/// the performance-filter tests below (and by [`super::search`]'s combined
/// display-plus-count test) where only the aggregate value matters.
pub(super) fn flat_tilt_curves(value: f32) -> crate::model::tilt_curves::TiltPerformanceCurves {
    use crate::model::tilt_curves::{
        AxisTiltCurves, TILT_CURVE_AXIS_COUNT, TILT_CURVE_POINTS_PER_AXIS,
    };
    crate::model::tilt_curves::TiltPerformanceCurves {
        axes: [AxisTiltCurves {
            brilliance_pct: [value; TILT_CURVE_POINTS_PER_AXIS],
            extinction_pct: [value; TILT_CURVE_POINTS_PER_AXIS],
            windowing_pct: [value; TILT_CURVE_POINTS_PER_AXIS],
        }; TILT_CURVE_AXIS_COUNT],
    }
}

#[test]
fn performance_filter_matches_designs_with_curves_and_excludes_those_without() {
    let path = temp_db_path("performance_filter_basic");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");

    let good_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Good Windowing".to_string(),
                url: "local://good.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    db.save_tilt_curves(good_id, &flat_tilt_curves(10.0), None, 1)
        .unwrap();

    let bad_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Bad Windowing".to_string(),
                url: "local://bad.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    db.save_tilt_curves(bad_id, &flat_tilt_curves(90.0), None, 1)
        .unwrap();

    let _no_curves_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "No Curves At All".to_string(),
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
    let results = db.search_diagrams("", "All", "All", &filter).unwrap();
    let titles: Vec<&str> = results.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(titles, vec!["Good Windowing"]);
    assert!(!titles.contains(&"Bad Windowing"));
    assert!(
        !titles.contains(&"No Curves At All"),
        "a design with no stored curves can never satisfy an active performance filter"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn performance_filter_reports_how_many_designs_were_excluded_for_missing_curves() {
    let path = temp_db_path("performance_filter_exclusions");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");

    let with_curves_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Has Curves".to_string(),
                url: "local://has-curves.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    db.save_tilt_curves(with_curves_id, &flat_tilt_curves(5.0), None, 1)
        .unwrap();

    for i in 0..3 {
        db.save_diagram_entry(
            &FacetDiagramEntry {
                title: format!("No Curves {i}"),
                url: format!("local://no-curves-{i}.asc"),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    }

    let filter = RangeFilter {
        performance: vec![crate::model::performance::PerformanceFilter {
            metric: crate::model::performance::PerformanceMetric::Windowing,
            bound: crate::model::performance::PerformanceBound::AtMost(20.0),
            tilt_radius_deg: 45.0,
            aggregate: crate::model::performance::PerformanceAggregate::Worst,
        }],
        ..Default::default()
    };
    let result = db
        .search_diagrams_with_performance_exclusions("", "All", "All", &filter)
        .unwrap();
    assert_eq!(
        result
            .items
            .iter()
            .map(|r| r.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Has Curves"]
    );
    assert_eq!(result.excluded_for_missing_curves, 3);

    // No performance filter active means exclusion count must be exactly 0.
    let no_filter_result = db
        .search_diagrams_with_performance_exclusions("", "All", "All", &RangeFilter::default())
        .unwrap();
    assert_eq!(no_filter_result.excluded_for_missing_curves, 0);
    assert_eq!(no_filter_result.items.len(), 4);

    let _ = std::fs::remove_file(&path);
}

/// A stored tilt-curve BLOB that fails to decode (wrong length --
/// e.g. on-disk corruption, or a future format change) must not fail the whole
/// performance-filtered search; it must be treated the same as "no curves stored" for
/// that one design, silently excluded, leaving every other design's result unaffected.
#[test]
fn a_corrupt_tilt_curve_blob_is_excluded_rather_than_failing_the_whole_search() {
    let db = Database::new(Some(":memory:")).expect("create in-memory db");

    let good_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Good Curves".to_string(),
                url: "local://good.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    db.save_tilt_curves(good_id, &flat_tilt_curves(10.0), None, 1)
        .unwrap();

    let corrupt_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Corrupt Curves".to_string(),
                url: "local://corrupt.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    // Written directly, bypassing `save_tilt_curves`, with a `curves` BLOB of the
    // wrong length. The 6 global-extreme columns are set permissively (0..=100) so
    // this row survives SQL-level narrowing and reaches the Rust-side decode this test
    // targets, rather than being pruned out before `get_tilt_curves` is ever called.
    let mut columns: Vec<String> = vec!["entry_id".into(), "curves".into(), "generated_at".into()];
    let mut values: Vec<Box<dyn rusqlite::ToSql>> =
        vec![Box::new(corrupt_id), Box::new(vec![0u8; 4]), Box::new(1i64)];
    for (metric, extreme) in crate::model::performance::all_global_extreme_columns() {
        columns.push(crate::model::performance::global_extreme_column_name(
            metric, extreme,
        ));
        let bound = match extreme {
            crate::model::performance::Extreme::Min => 0.0_f64,
            crate::model::performance::Extreme::Max => 100.0_f64,
        };
        values.push(Box::new(bound));
    }
    let placeholders: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
    let insert_sql = format!(
        "INSERT INTO diagram_tilt_curves ({}) VALUES ({})",
        columns.join(", "),
        placeholders.join(", ")
    );
    let bound: Vec<&dyn rusqlite::ToSql> = values.iter().map(std::convert::AsRef::as_ref).collect();
    db.conn
        .prepare(&insert_sql)
        .unwrap()
        .execute(bound.as_slice())
        .unwrap();

    let filter = RangeFilter {
        performance: vec![crate::model::performance::PerformanceFilter {
            metric: crate::model::performance::PerformanceMetric::Windowing,
            bound: crate::model::performance::PerformanceBound::AtMost(90.0),
            tilt_radius_deg: 45.0,
            aggregate: crate::model::performance::PerformanceAggregate::Worst,
        }],
        ..Default::default()
    };

    let results = db
        .search_diagrams("", "All", "All", &filter)
        .expect("a corrupt curve BLOB must not fail the whole search");
    assert_eq!(
        results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        vec!["Good Curves"],
        "the corrupt row must be silently excluded, not crash the query nor false-match"
    );
}

#[test]
fn performance_filters_combine_by_and() {
    let path = temp_db_path("performance_filter_and");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");

    // Passes the windowing filter but not the brilliance one.
    let only_windowing_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Only Windowing Passes".to_string(),
                url: "local://only-windowing.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    db.save_tilt_curves(
        only_windowing_id,
        &flat_tilt_curves(10.0), // windowing 10<=20 pass, brilliance 10<50 fails AtLeast(50)
        None,
        1,
    )
    .unwrap();

    // Passes both.
    let both_id = db
        .save_diagram_entry(
            &FacetDiagramEntry {
                title: "Passes Both".to_string(),
                url: "local://both.asc".to_string(),
                design_id: String::new(),
            },
            "local-import",
        )
        .unwrap();
    // windowing and brilliance are separate metrics, so windowing=10 + brilliance=60 works.
    let mut curves = flat_tilt_curves(10.0);
    for axis in &mut curves.axes {
        axis.brilliance_pct = [60.0; crate::model::tilt_curves::TILT_CURVE_POINTS_PER_AXIS];
    }
    db.save_tilt_curves(both_id, &curves, None, 1).unwrap();

    let filter = RangeFilter {
        performance: vec![
            crate::model::performance::PerformanceFilter {
                metric: crate::model::performance::PerformanceMetric::Windowing,
                bound: crate::model::performance::PerformanceBound::AtMost(20.0),
                tilt_radius_deg: 45.0,
                aggregate: crate::model::performance::PerformanceAggregate::Worst,
            },
            crate::model::performance::PerformanceFilter {
                metric: crate::model::performance::PerformanceMetric::Brilliance,
                bound: crate::model::performance::PerformanceBound::AtLeast(50.0),
                tilt_radius_deg: 45.0,
                aggregate: crate::model::performance::PerformanceAggregate::Worst,
            },
        ],
        ..Default::default()
    };
    let results = db.search_diagrams("", "All", "All", &filter).unwrap();
    assert_eq!(
        results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        vec!["Passes Both"],
        "combined filters must AND together, not OR"
    );

    let _ = std::fs::remove_file(&path);
}

/// A tiny deterministic PRNG (xorshift64): this crate stays dependency-lean (no `rand`)
/// and tests must be deterministic, so a fixed seed exercises the same curve data every run.
struct XorShift64(u64);

impl XorShift64 {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A pseudo-random `f32` in `[0.0, 100.0)`, matching `AxisTiltCurves`' `*_pct` scale.
    fn next_pct(&mut self) -> f32 {
        let top_24_bits = (self.next_u64() >> 40) as u32;
        (f64::from(top_24_bits) / f64::from(1u32 << 24) * 100.0) as f32
    }
}

/// Builds a fully-randomised `TiltPerformanceCurves` (every sample independently drawn)
/// -- deliberately not flat like `flat_tilt_curves`, to exercise the general case where
/// a window's value depends on which points fall inside it.
fn randomised_tilt_curves(
    rng: &mut XorShift64,
) -> crate::model::tilt_curves::TiltPerformanceCurves {
    use crate::model::tilt_curves::{
        AxisTiltCurves, TILT_CURVE_AXIS_COUNT, TILT_CURVE_POINTS_PER_AXIS,
    };
    let mut axes = [AxisTiltCurves {
        brilliance_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
        extinction_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
        windowing_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
    }; TILT_CURVE_AXIS_COUNT];
    for axis in &mut axes {
        for curve in [
            &mut axis.brilliance_pct,
            &mut axis.extinction_pct,
            &mut axis.windowing_pct,
        ] {
            for sample in curve.iter_mut() {
                *sample = rng.next_pct();
            }
        }
    }
    crate::model::tilt_curves::TiltPerformanceCurves { axes }
}

/// The soundness property the two-stage (SQL-narrows, Rust-decides) design depends on:
/// searching with a `PerformanceFilter` active must return EXACTLY the same designs as
/// a brute-force scan that decodes every design's curves and evaluates the filter
/// directly, bypassing SQL narrowing. Catches wrongly-excluding pruning bugs; a
/// too-permissive pruning bug is harmless and invisible here by design (see
/// `build_search_predicate`'s doc comment -- the exact check is the source of truth).
#[test]
fn performance_filter_sql_narrowing_matches_a_brute_force_scan_exactly() {
    const DESIGN_COUNT: usize = 24;

    let path = temp_db_path("performance_pruning_soundness");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create migrated db");
    let mut rng = XorShift64(0x9E37_79B9_7F4A_7C15);

    let mut all_entry_ids = Vec::with_capacity(DESIGN_COUNT);
    for i in 0..DESIGN_COUNT {
        let entry_id = db
            .save_diagram_entry(
                &FacetDiagramEntry {
                    title: format!("Randomised {i}"),
                    url: format!("local://randomised-{i}.asc"),
                    design_id: String::new(),
                },
                "local-import",
            )
            .unwrap();
        // Every third design gets no curves, covering exclusion consistency too.
        if i % 3 != 0 {
            let curves = randomised_tilt_curves(&mut rng);
            db.save_tilt_curves(entry_id, &curves, None, 1).unwrap();
        }
        all_entry_ids.push(entry_id);
    }

    let cases = [
        (
            crate::model::performance::PerformanceMetric::Brilliance,
            crate::model::performance::PerformanceBound::AtLeast(30.0),
            0.0_f32,
            crate::model::performance::PerformanceAggregate::Worst,
        ),
        (
            crate::model::performance::PerformanceMetric::Windowing,
            crate::model::performance::PerformanceBound::AtMost(70.0),
            12.5,
            crate::model::performance::PerformanceAggregate::Worst,
        ),
        (
            crate::model::performance::PerformanceMetric::Extinction,
            crate::model::performance::PerformanceBound::AtLeast(50.0),
            37.0,
            crate::model::performance::PerformanceAggregate::Worst,
        ),
        (
            crate::model::performance::PerformanceMetric::Brilliance,
            crate::model::performance::PerformanceBound::AtMost(60.0),
            90.0,
            crate::model::performance::PerformanceAggregate::Worst,
        ),
        (
            crate::model::performance::PerformanceMetric::Windowing,
            crate::model::performance::PerformanceBound::AtMost(55.0),
            45.0,
            crate::model::performance::PerformanceAggregate::Mean,
        ),
        (
            crate::model::performance::PerformanceMetric::Brilliance,
            crate::model::performance::PerformanceBound::AtLeast(45.0),
            22.0,
            crate::model::performance::PerformanceAggregate::Mean,
        ),
    ];

    for (metric, bound, radius, aggregate) in cases {
        let filter =
            crate::model::performance::PerformanceFilter::new(metric, bound, radius, aggregate)
                .unwrap();

        // Pruned path: search narrows in SQL via the 6 global min/max columns first.
        let pruned: std::collections::BTreeSet<i64> = db
            .search_diagrams(
                "",
                "All",
                "All",
                &RangeFilter {
                    performance: vec![filter],
                    ..Default::default()
                },
            )
            .unwrap()
            .into_iter()
            .map(|item| item.id)
            .collect();

        // Brute-force path: decode every design's curves, evaluate the exact predicate.
        let brute: std::collections::BTreeSet<i64> = all_entry_ids
            .iter()
            .copied()
            .filter(|&id| {
                db.get_tilt_curves(id)
                    .unwrap()
                    .is_some_and(|curves| curves.matches_performance_filter(&filter))
            })
            .collect();

        assert_eq!(
            pruned, brute,
            "SQL-narrowed search and a brute-force scan disagreed for {metric:?} \
             {bound:?} radius={radius} {aggregate:?} -- the SQL narrowing predicate is \
             unsound (it excluded a design the exact check would have kept, or vice \
             versa)"
        );
    }

    let _ = std::fs::remove_file(&path);
}
