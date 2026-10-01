//! `StandardGemCuts::from_database_angles` crown/pavilion classification and
//! angle-parsing robustness tests -- classification must trust explicit evidence
//! (facet-name prefixes / `index_val` markers actually observed in the scraped
//! facetdiagrams.org data) over the blind positional guess, and the parser must
//! tolerate a trailing UTF-8 degree sign, the "N girdle facets" shorthand, and
//! individually unparseable rows without bailing out on the whole schedule.

use glam::Vec3;
use indicatrix::{FacetSpec, geometry::cuts::StandardGemCuts};

// ---------------------------------------------------------------------------
// Crown/pavilion classification in from_database_angles should trust
// explicit evidence (facet-name prefixes / index_val markers actually observed
// in the scraped facetdiagrams.org data) over the blind positional guess.
// ---------------------------------------------------------------------------
#[test]
fn from_database_angles_classifies_by_facet_name_prefix_not_position() {
    // 4 rows, so the old fallback heuristic (`tier_idx > angles.len() / 2`) would
    // classify indices 0-1 as "pavilion" and indices 2-3 as "crown" purely from
    // position. Place a C-prefixed (crown) facet at index 0 -- where the old
    // heuristic would wrongly say pavilion -- and a P-prefixed (pavilion) facet
    // at index 3 -- where the old heuristic would wrongly say crown -- to prove
    // the classification now comes from the evidenced facet-name marker, not
    // list position. Indices 1-2 are neutral filler with no C/P/G marker, so
    // they still fall back to the old positional guess (untested here).
    let filler = || FacetSpec {
        facet: "5".into(),
        angle: "45.0".into(),
        index: "10".into(),
        notes: String::new(),
    };

    let angles = vec![
        FacetSpec {
            facet: "C1".into(),
            angle: "40.0".into(),
            index: "0".into(),
            notes: String::new(),
        },
        filler(),
        filler(),
        FacetSpec {
            facet: "P1".into(),
            angle: "40.0".into(),
            index: "48".into(),
            notes: String::new(),
        },
    ];

    let planes = StandardGemCuts::from_database_angles(&angles, 96);
    assert_eq!(planes.len(), 4);

    // C1 (index 0, "pavilion" by the old positional guess) must be classified
    // as crown -> positive Y component.
    let n0 = Vec3::from(planes[0].normal);
    assert!(
        n0.y > 0.0,
        "C1 facet should be classified as crown (normal.y > 0) despite its early position, got {n0:?}"
    );

    // P1 (index 3, "crown" by the old positional guess) must be classified as
    // pavilion -> negative Y component.
    let n3 = Vec3::from(planes[3].normal);
    assert!(
        n3.y < 0.0,
        "P1 facet should be classified as pavilion (normal.y < 0) despite its late position, got {n3:?}"
    );
}

#[test]
fn from_database_angles_classifies_table_and_culet_via_index_val() {
    // from_database_angles() falls back to the built-in standard_round_brilliant()
    // table whenever it reconstructs fewer than 4 planes total, so pad this out
    // with a couple of multi-index filler tiers to stay above that floor while
    // keeping the Table/Culet rows as the ones actually under test.
    //
    // The angle is written as "0.00°" (a literal trailing degree sign), matching the
    // real scraped format in facet_diagrams.sqlite, NOT the plain "0.0" that would
    // already have parsed under the old strict `.parse()` call. Using the real format
    // here is what actually exercises the degree-sign parsing fix -- with the old
    // code every one of these angles would have silently become 45 degrees instead
    // of 0, and this assertion would have failed.
    let angles = vec![
        FacetSpec {
            facet: "U".into(),
            angle: "0.00\u{b0}".into(),
            index: "Table".into(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "5".into(),
            angle: "35.00\u{b0}".into(),
            index: "10-20".into(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "6".into(),
            angle: "35.00\u{b0}".into(),
            index: "30-40".into(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "41".into(),
            angle: "0.00\u{b0}".into(),
            index: "Culet".into(),
            notes: String::new(),
        },
    ];

    let planes = StandardGemCuts::from_database_angles(&angles, 96);
    assert_eq!(planes.len(), 6);

    let table_n = Vec3::from(planes[0].normal);
    assert!(
        (table_n - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-5,
        "index_val=Table must map to crown table (0,1,0), got {table_n:?}"
    );

    let culet_n = Vec3::from(planes[5].normal);
    assert!(
        (culet_n - Vec3::new(0.0, -1.0, 0.0)).length() < 1e-5,
        "index_val=Culet must map to pavilion culet (0,-1,0), got {culet_n:?}"
    );
}

// ---------------------------------------------------------------------------
// Every row in the real database's `angle_settings.angle` column carries a trailing
// UTF-8 degree sign (e.g. "44.86°"), which `f32::from_str` rejects outright. A strict
// `item.angle.parse().unwrap_or(45.0)` would silently fall back to a fabricated 45
// degrees for every single row in the real database, flattening every reconstructed
// diagram into the same shape. These tests exercise the lenient parser, the "N girdle
// facets" index_val expansion, and the bail-out-to-SRB path.
// ---------------------------------------------------------------------------
#[test]
fn from_database_angles_parses_real_degree_sign_format() {
    let angles = vec![
        FacetSpec {
            facet: "C1".into(),
            angle: "10.00\u{b0}".into(), // literal "10.00°", exactly as stored in the DB
            index: String::new(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "5".into(),
            angle: "45.00\u{b0}".into(),
            index: "10-20".into(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "6".into(),
            angle: "45.00\u{b0}".into(),
            index: "30-40".into(),
            notes: String::new(),
        },
    ];

    let planes = StandardGemCuts::from_database_angles(&angles, 96);
    // item 0 (empty index_val) -> 1 plane via the single-default-orientation branch;
    // items 1 and 2 each carry 2 indices -> 2 planes each. Total 5.
    assert_eq!(planes.len(), 5);

    let n0 = Vec3::from(planes[0].normal);
    let expected_y = 10.0f32.to_radians().cos();
    assert!(
        (n0.y - expected_y).abs() < 1e-4,
        "\"10.00°\" should parse as 10 degrees (cos ~= {expected_y}), got normal {n0:?}"
    );

    // Must not have silently fallen back to the old hard-coded 45 degree default.
    let old_buggy_y = 45.0f32.to_radians().cos();
    assert!(
        (n0.y - old_buggy_y).abs() > 0.05,
        "angle parse appears to have silently fallen back to the old 45 degree default: {n0:?}"
    );
}

#[test]
fn from_database_angles_expands_n_girdle_facets_form() {
    let angles = vec![FacetSpec {
        facet: "G1".into(),
        angle: "90.00\u{b0}".into(),
        index: "48 girdle facets".into(),
        notes: String::new(),
    }];

    let planes = StandardGemCuts::from_database_angles(&angles, 96);
    assert_eq!(
        planes.len(),
        48,
        "\"48 girdle facets\" must expand into 48 separate, evenly spaced facets"
    );

    // At 90 degrees the crown/pavilion formulas coincide (cos(90) == 0), so every
    // facet normal must be perfectly horizontal regardless of classification.
    for p in &planes {
        let n = Vec3::from(p.normal);
        assert!(
            n.y.abs() < 1e-4,
            "girdle facet normal must be horizontal (y == 0), got {n:?}"
        );
    }

    // Adjacent facets should be evenly spaced at 360/48 = 7.5 degrees apart
    // (indices generated as i * gear_teeth / N for i in 0..N).
    let phi = |p: &indicatrix::geometry::GpuFacetPlane| p.normal[2].atan2(p.normal[0]).to_degrees();
    let mut delta = phi(&planes[1]) - phi(&planes[0]);
    if delta < 0.0 {
        delta += 360.0;
    }
    assert!(
        (delta - 7.5).abs() < 0.5,
        "expected ~7.5 deg spacing between adjacent expanded girdle facets, got {delta}"
    );
}

#[test]
fn from_database_angles_bails_out_to_srb_when_mostly_unparseable() {
    // 3 of 4 angle strings are garbage (75% unparseable, well over the bail-out
    // threshold) -- the function must refuse to build a solid out of fabricated
    // angles and fall back to the built-in standard_round_brilliant() table instead.
    let angles = vec![
        FacetSpec {
            facet: "1".into(),
            angle: "not-a-number".into(),
            index: "10".into(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "2".into(),
            angle: "???".into(),
            index: "20".into(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "3".into(),
            angle: String::new(),
            index: "30".into(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "4".into(),
            angle: "40.00\u{b0}".into(),
            index: "40".into(),
            notes: String::new(),
        },
    ];

    let planes = StandardGemCuts::from_database_angles(&angles, 96);
    let srb = StandardGemCuts::standard_round_brilliant();
    assert_eq!(
        planes.len(),
        srb.len(),
        "should bail out to standard_round_brilliant() when most angle values are unparseable"
    );
}

#[test]
fn from_database_angles_skips_single_bad_row_without_bailing_out() {
    // A single unparseable row among many (below the bail-out threshold) should be
    // skipped individually rather than either fabricating an angle for it or
    // discarding the whole, otherwise-good reconstruction.
    let mut angles: Vec<FacetSpec> = (0..10)
        .map(|i| FacetSpec {
            facet: format!("{}", i + 1),
            angle: "30.00\u{b0}".into(),
            index: String::new(),
            notes: String::new(),
        })
        .collect();
    angles.push(FacetSpec {
        facet: "bad".into(),
        angle: "garbage".into(),
        index: String::new(),
        notes: String::new(),
    });

    let planes = StandardGemCuts::from_database_angles(&angles, 96);
    // 10 good rows -> 10 planes (single-default-orientation branch, one each); the
    // one bad row contributes nothing and must not trigger the SRB bail-out (11
    // planes != standard_round_brilliant()'s facet count).
    assert_eq!(
        planes.len(),
        10,
        "the single unparseable row should be skipped, not fabricated or bailed out on"
    );
}
