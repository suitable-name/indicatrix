use glam::Vec3;
use indicatrix::{
    FacetSpec,
    geometry::{
        brep::{BrepError, GemPolyhedron},
        cuts::StandardGemCuts,
        plane::GpuFacetPlane,
    },
    optics::polarization::MuellerMatrix,
};
use indicatrix_formats::asc;
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// FIX A: fresnel_transmission must not apply a spurious sqrt() to the (3,3)/(4,4)
// element. At normal incidence there is no polarization effect, so element
// (1,1) [top-left, "m11"] and element (3,3) ["m33"] of the Mueller matrix must
// be equal.
// ---------------------------------------------------------------------------
#[test]
fn fresnel_transmission_normal_incidence_m11_equals_m33() {
    let n1 = 1.0f32;
    let n2 = 2.4178f32;
    let cos_i = 1.0f32;
    let cos_t = 1.0f32;

    // Standard Fresnel amplitude transmission coefficients.
    let t_s = (2.0 * n1 * cos_i) / n2.mul_add(cos_t, n1 * cos_i);
    let t_p = (2.0 * n1 * cos_i) / n1.mul_add(cos_t, n2 * cos_i);

    let m = MuellerMatrix::fresnel_transmission(n1, n2, cos_i, cos_t, t_s, t_p);

    // glam::Mat4 is column-major and MuellerMatrix::fresnel_transmission is built via
    // Mat4::from_cols_array(&[a, b, 0, 0,  b, a, 0, 0,  0, 0, c, 0,  0, 0, 0, c]).
    // Column 0 is [a, b, 0, 0] and column 2 is [0, 0, c, 0], so:
    //   m11 (row0, col0) = m.x_axis.x
    //   m33 (row2, col2) = m.z_axis.z
    let m11 = m.x_axis.x;
    let m33 = m.z_axis.z;

    // Independently cross-check against the value quoted in the bug report.
    assert!(
        (m11 - 0.8279).abs() < 1e-3,
        "expected m11 ~= 0.8279, got {m11}"
    );

    assert!(
        (m11 - m33).abs() < 1e-5,
        "at normal incidence m11 and m33 must agree (no polarization effect): m11={m11}, m33={m33}"
    );

    // The buggy version (with .sqrt() on m33) produced ~0.9099, a 9.9% deviation --
    // make sure we are nowhere near that.
    assert!(
        (m33 - 0.9099).abs() > 0.01,
        "m33 looks like the old, buggy sqrt() value: {m33}"
    );
}

// ---------------------------------------------------------------------------
// FIX C: crown/pavilion classification in from_database_angles should trust
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
// FIX C (follow-up): every row in the real database's `angle_settings.angle` column
// carries a trailing UTF-8 degree sign (e.g. "44.86°"), which `f32::from_str` rejects
// outright. Under the old `item.angle.parse().unwrap_or(45.0)` this meant every single
// row in the real database silently fell back to a fabricated 45 degrees, flattening
// every reconstructed diagram into the same shape. These tests exercise the lenient
// parser, the "N girdle facets" index_val expansion, and the bail-out-to-SRB path.
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

// ---------------------------------------------------------------------------
// B-Rep reconstruction (`geometry::brep::GemPolyhedron::from_planes`).
//
// `from_planes` implements the dual-space convex hull construction described in
// GEMSTONE_RENDERING_BLUEPRINT.md section 1.2. It was fully written but never called
// from anywhere, and contained an unresolved indexing bug: `chull`'s
// `vertices_indices()` compacts and renumbers its returned point list to just the
// points that became hull vertices (dropping the rest), so the returned triangle
// indices index into that *compacted* list, not into the original `planes` array --
// but the code used them to index `planes` directly, silently pairing each
// reconstructed vertex with the wrong facet planes whenever any input plane was
// redundant (a very common case: see the emerald_cut() test below). A second,
// related defect: whenever more than 3 planes meet at exactly the same point (common
// in symmetric cuts, e.g. round-brilliant girdle/kite/star junctions), the dual hull
// has a coplanar N-gon facet that `chull` triangulates into several triangles, each
// independently re-solving to the *same* primal point -- producing duplicate vertex
// entries that break edge/facet adjacency unless welded back together.
//
// These tests exercise both fixes (via Euler's formula, the single most valuable
// topological check, plus exact counts for a hand-verifiable cube) and the new
// degenerate-input handling.
// ---------------------------------------------------------------------------

/// Independently recomputes the polyhedron's edge set from its public
/// `facet_polygons` field (deliberately not trusting anything internal to
/// `from_planes`), and returns `(V, E, F)`. Panics if any edge is not shared by
/// exactly two facets, or if Euler's formula `V - E + F = 2` does not hold.
fn assert_euler_formula(hull: &GemPolyhedron) -> (usize, usize, usize) {
    let faces: Vec<&Vec<u32>> = hull
        .facet_polygons
        .iter()
        .filter(|p| p.len() >= 3)
        .collect();

    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    let mut edge_hits: std::collections::HashMap<(u32, u32), u32> =
        std::collections::HashMap::new();
    for poly in &faces {
        for w in 0..poly.len() {
            let a = poly[w];
            let b = poly[(w + 1) % poly.len()];
            let key = if a < b { (a, b) } else { (b, a) };
            edges.insert(key);
            *edge_hits.entry(key).or_insert(0) += 1;
        }
    }
    for (edge, count) in &edge_hits {
        assert_eq!(
            *count, 2,
            "edge {edge:?} is shared by {count} facets, not exactly 2 (non-manifold mesh)"
        );
    }

    let v = hull.vertices.len();
    let e = edges.len();
    let f = faces.len();
    assert_eq!(
        v as i64 - e as i64 + f as i64,
        2,
        "Euler's formula V - E + F = 2 failed: V={v}, E={e}, F={f}"
    );
    (v, e, f)
}

/// A cube built from six axis-aligned half-space planes, trivially checkable by hand:
/// 8 vertices, 12 edges, 6 faces, volume 8 (side length 2), surface area 24.
fn axis_aligned_cube_planes() -> Vec<GpuFacetPlane> {
    vec![
        GpuFacetPlane::new(Vec3::new(1.0, 0.0, 0.0), -1.0),
        GpuFacetPlane::new(Vec3::new(-1.0, 0.0, 0.0), -1.0),
        GpuFacetPlane::new(Vec3::new(0.0, 1.0, 0.0), -1.0),
        GpuFacetPlane::new(Vec3::new(0.0, -1.0, 0.0), -1.0),
        GpuFacetPlane::new(Vec3::new(0.0, 0.0, 1.0), -1.0),
        GpuFacetPlane::new(Vec3::new(0.0, 0.0, -1.0), -1.0),
    ]
}

#[test]
fn brep_cube_reconstructs_exact_topology() {
    let hull = GemPolyhedron::from_planes(axis_aligned_cube_planes())
        .expect("a cube's 6 planes must reconstruct");

    let (v, e, f) = assert_euler_formula(&hull);
    assert_eq!(v, 8, "cube must have exactly 8 vertices");
    assert_eq!(e, 12, "cube must have exactly 12 edges");
    assert_eq!(f, 6, "cube must have exactly 6 faces");

    assert!(
        hull.untouched_planes().is_empty(),
        "every one of the cube's 6 planes must be touched by a facet, got untouched: {:?}",
        hull.untouched_planes()
    );

    assert!(
        (hull.volume() - 8.0).abs() < 1e-3,
        "side-2 cube must have volume 8, got {}",
        hull.volume()
    );

    let total_area: f32 = hull.facet_areas().iter().sum();
    assert!(
        (total_area - 24.0).abs() < 1e-3,
        "side-2 cube must have total surface area 24 (6 faces x 4), got {total_area}"
    );
    for area in hull.facet_areas() {
        assert!(
            (area - 4.0).abs() < 1e-3,
            "each cube face must have area 4, got {area}"
        );
    }

    let girdle = hull.girdle_outline();
    assert_eq!(
        girdle.len(),
        4,
        "a cube's top-down silhouette is its own 4-cornered square, got {} points",
        girdle.len()
    );
}

#[test]
fn brep_standard_round_brilliant_reconstructs_valid_closed_solid() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_count = planes.len();
    let hull = GemPolyhedron::from_planes(planes)
        .expect("standard_round_brilliant()'s planes must reconstruct into a valid solid");

    let (v, e, f) = assert_euler_formula(&hull);
    assert_eq!(
        f, plane_count,
        "every one of the {plane_count} SRB planes should surface as its own facet"
    );
    assert!(
        v > 0 && e > 0,
        "expected a non-trivial mesh, got V={v} E={e}"
    );

    assert!(
        hull.untouched_planes().is_empty(),
        "every plane in the reference standard_round_brilliant() cut should be touched by a facet; untouched: {:?}",
        hull.untouched_planes()
    );

    assert!(
        hull.volume().is_finite() && hull.volume() > 0.0,
        "reconstructed volume must be finite and positive, got {}",
        hull.volume()
    );

    let girdle = hull.girdle_outline();
    assert!(
        girdle.len() >= 8,
        "SRB's girdle outline should trace a many-sided polygon, got only {} points",
        girdle.len()
    );
}

#[test]
fn brep_emerald_cut_reconstructs_valid_closed_solid_with_all_planes_touched() {
    let planes = StandardGemCuts::emerald_cut();
    let hull = GemPolyhedron::from_planes(planes)
        .expect("emerald_cut()'s planes must reconstruct into a valid solid");

    assert_euler_formula(&hull);
    assert!(
        hull.volume().is_finite() && hull.volume() > 0.0,
        "reconstructed volume must be finite and positive, got {}",
        hull.volume()
    );

    // Every tier offset in emerald_cut() is now derived from one shared profile
    // (girdle band, crown/pavilion crease rings, and the unchanged girdle radii)
    // instead of being hand-picked independently, so each plane's crease line lands
    // inside the region already bounded by its neighbors: all 34 planes contribute a
    // facet to the reconstructed hull and none are dominated/redundant.
    let untouched = hull.untouched_planes();
    assert!(
        untouched.is_empty(),
        "emerald_cut()'s planes should all be touched by the reconstructed hull; \
         untouched: {untouched:?}"
    );

    // A geometrically correct 34-plane emerald/step cut reconstructs to exactly 48
    // vertices at volume ~1.8307. This pins that shape: `emerald_cut()`'s tier offsets
    // are computed from a shared profile so planes meant to meet at one point (e.g. a
    // girdle-adjacent tier and its neighbor on the crease ring, or the facets converging
    // on a girdle corner) do so to full f32 precision. Before that, the same profile
    // pasted in as 4-decimal-rounded literals left several such intended-coincident
    // points ~1.5e-4 apart -- just outside `VERTEX_WELD_EPS` (1e-4) -- which welded
    // incompletely and reconstructed 60 vertices instead of 48 (same 34/34-touched,
    // same ~1.8307 volume, so neither of those alone would have caught it).
    assert_eq!(
        hull.vertices.len(),
        48,
        "emerald_cut() should reconstruct to exactly 48 vertices; a higher count here \
         (e.g. 60) means intended-coincident meet points drifted outside VERTEX_WELD_EPS \
         again, most likely because a tier offset went back to being a rounded literal \
         instead of being derived from the shared profile"
    );
    let volume = hull.volume();
    assert!(
        (volume - 1.8307).abs() < 0.001,
        "emerald_cut() volume drifted from the expected ~1.8307, got {volume}"
    );
}

#[test]
fn brep_rejects_fewer_than_four_planes() {
    let planes = vec![
        GpuFacetPlane::new(Vec3::new(1.0, 0.0, 0.0), -1.0),
        GpuFacetPlane::new(Vec3::new(0.0, 1.0, 0.0), -1.0),
        GpuFacetPlane::new(Vec3::new(0.0, 0.0, 1.0), -1.0),
    ];
    let err =
        GemPolyhedron::from_planes(planes).expect_err("3 planes cannot bound a finite 3D solid");
    assert!(
        matches!(err, BrepError::TooFewPlanes { count: 3 }),
        "expected TooFewPlanes {{ count: 3 }}, got: {err:?}"
    );
}

#[test]
fn brep_rejects_nonnegative_offset() {
    let mut planes = axis_aligned_cube_planes();
    planes[0] = GpuFacetPlane::new(Vec3::new(1.0, 0.0, 0.0), 1.0); // d >= 0
    let err = GemPolyhedron::from_planes(planes)
        .expect_err("a plane with d >= 0 does not contain the origin");
    assert!(
        matches!(err, BrepError::NonNegativeOffset { .. }),
        "expected NonNegativeOffset, got: {err:?}"
    );
    assert!(
        err.to_string().contains("negative"),
        "error should explain the d < 0 requirement, got: {err}"
    );
}

#[test]
fn brep_rejects_coincident_planes() {
    let mut planes = axis_aligned_cube_planes();
    planes[1] = GpuFacetPlane::new(Vec3::new(1.0, 0.0, 0.0), -1.0); // duplicate of planes[0]
    let err =
        GemPolyhedron::from_planes(planes).expect_err("two identical half-spaces must be rejected");
    assert!(
        matches!(err, BrepError::CoincidentPlanes { .. }),
        "expected CoincidentPlanes, got: {err:?}"
    );
    assert!(
        err.to_string().contains("coincident"),
        "error should call out the coincident planes, got: {err}"
    );
}

#[test]
fn brep_rejects_unbounded_region() {
    // A cube missing its +Z face is an infinite prism, not a finite solid -- even
    // though the origin is still a strict interior point of every *individual*
    // remaining half-space.
    let mut planes = axis_aligned_cube_planes();
    planes.remove(4); // the (0,0,1) face
    let err = GemPolyhedron::from_planes(planes)
        .expect_err("5 planes open on one side cannot bound a finite solid");
    assert!(
        matches!(err, BrepError::UnboundedRegion { .. }),
        "expected UnboundedRegion, got: {err:?}"
    );
    let text = err.to_string();
    assert!(
        text.contains("unbounded") || text.contains("finite region"),
        "error should explain the region is unbounded, got: {text}"
    );
}

#[test]
fn brep_rejects_ill_conditioned_near_parallel_triple() {
    // Replace the cube's +X face with a plane tilted only 0.00005 degrees off the +Y
    // face's own normal. The box still closes up comfortably (the origin stays safely
    // interior, well clear of the separate unbounded-region check), but the two
    // nearly-parallel faces now meet the +Z/-Z faces at vertices whose 3x3
    // intersection solve is numerically ill-conditioned -- this tilt was picked
    // empirically as reliably below `MIN_TRIPLE_DETERMINANT`'s threshold (a coarser
    // tilt, e.g. 0.01 degrees, is still well-conditioned enough to reconstruct
    // successfully; a slightly finer one than this still errors, but via the separate
    // non-manifold/Euler check instead, because welding starts landing inconsistently
    // right at the edge of the ill-conditioned regime -- either way, this is exactly
    // the "return a descriptive error rather than a malformed mesh" contract).
    let mut planes = axis_aligned_cube_planes();
    let tilt = 0.00005f32.to_radians();
    planes[0] = GpuFacetPlane::new(Vec3::new(tilt.sin(), tilt.cos(), 0.0), -1.0);
    let err = GemPolyhedron::from_planes(planes).expect_err(
        "a near-parallel facet triple must be rejected rather than produce a malformed mesh",
    );
    assert!(
        matches!(err, BrepError::IllConditionedTriple { .. }),
        "expected IllConditionedTriple, got: {err:?}"
    );
    let text = err.to_string();
    assert!(
        text.contains("near-parallel") || text.contains("ill-conditioned"),
        "expected the ill-conditioning to be called out, got: {text}"
    );
}

#[test]
fn standard_gem_cut_generators_always_satisfy_from_planes_d_negative_precondition() {
    // `from_planes` requires every plane's d < 0 (so the origin lies strictly inside
    // every half-space). Verify this actually holds for both hand-built reference
    // cuts and for from_database_angles()'s full angle range (5..88 degrees, where it
    // uses a taper formula rather than a hardcoded offset).
    for p in StandardGemCuts::standard_round_brilliant() {
        assert!(
            p.d < 0.0,
            "standard_round_brilliant() produced a plane with d = {} >= 0",
            p.d
        );
    }
    for p in StandardGemCuts::emerald_cut() {
        assert!(
            p.d < 0.0,
            "emerald_cut() produced a plane with d = {} >= 0",
            p.d
        );
    }

    // 5.5, 9.2, 12.9, ... stepping by 3.7 degrees, staying inside the (5, 88) taper
    // range that `from_database_angles` uses its proportional formula for.
    let steps = ((88.0f32 - 5.5) / 3.7).ceil() as u32;
    for step in 0..steps {
        let angle_deg = 3.7f32.mul_add(step as f32, 5.5);
        for (facet_prefix, notes) in [("C1", ""), ("P1", "")] {
            let angles = vec![FacetSpec {
                facet: facet_prefix.into(),
                angle: format!("{angle_deg:.2}\u{b0}"),
                index: "0".into(),
                notes: notes.into(),
            }];
            // from_database_angles() bails out to standard_round_brilliant() below 4
            // planes, which would defeat the point of this check -- pad with filler
            // rows classified oppositely so the row under test survives unchanged.
            let mut rows = angles;
            rows.extend((0..3).map(|i| FacetSpec {
                facet: format!("filler{i}"),
                angle: format!("{angle_deg:.2}\u{b0}"),
                index: "1,2,3".into(),
                notes: String::new(),
            }));
            let planes = StandardGemCuts::from_database_angles(&rows, 96);
            assert_ne!(
                planes,
                Vec::new(),
                "from_database_angles() at angle {angle_deg} deg produced no planes"
            );
            for p in &planes {
                assert!(
                    p.d < 0.0,
                    "from_database_angles() at angle {angle_deg} deg produced a plane with d = {} >= 0",
                    p.d
                );
            }
        }
    }
}

#[test]
fn reconstruct_validated_brep_falls_back_to_srb_on_empty_schedule() {
    let srb_hull = GemPolyhedron::from_planes(StandardGemCuts::standard_round_brilliant()).unwrap();
    let hull = StandardGemCuts::reconstruct_validated_brep(&[], 96);
    assert_eq!(
        hull.facet_planes.len(),
        srb_hull.facet_planes.len(),
        "empty schedule should fall back to standard_round_brilliant()"
    );
    assert_euler_formula(&hull);
}

#[test]
fn reconstruct_validated_brep_falls_back_to_srb_on_mostly_unparseable_schedule() {
    let srb_hull = GemPolyhedron::from_planes(StandardGemCuts::standard_round_brilliant()).unwrap();
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
    let hull = StandardGemCuts::reconstruct_validated_brep(&angles, 96);
    assert_eq!(
        hull.facet_planes.len(),
        srb_hull.facet_planes.len(),
        "mostly-garbage schedule should fall back to standard_round_brilliant()"
    );
    assert_euler_formula(&hull);
}

#[test]
fn reconstruct_validated_brep_uses_the_real_reconstruction_when_well_formed() {
    // A small, well-formed schedule -- 8 crown facets and 8 pavilion facets, evenly
    // spaced and symmetric enough to converge to a single apex at each end without
    // needing separate table/culet/girdle rows -- forms a valid, closed octagonal
    // bipyramid on its own. Its plane count (16) is distinct from
    // standard_round_brilliant()'s (74), so a passing result here proves the gate
    // returned the actual reconstruction rather than silently falling back.
    //
    // (An earlier version of this test also added flat Table/Culet rows, modeled
    // after a real round-brilliant schedule; that turned out to leave exactly one of
    // the resulting 18 planes untouched -- a genuinely over-constrained schedule that
    // `reconstruct_validated_brep` correctly falls back on. That is the gate working
    // as intended, not a bug, but it meant that particular schedule was the wrong
    // fixture for a test whose point is to demonstrate the *non*-fallback path.)
    let angles = vec![
        FacetSpec {
            facet: "C1".into(),
            angle: "40.00\u{b0}".into(),
            index: "8 girdle facets".into(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "P1".into(),
            angle: "40.00\u{b0}".into(),
            index: "8 girdle facets".into(),
            notes: String::new(),
        },
    ];
    let srb_len = StandardGemCuts::standard_round_brilliant().len();
    let hull = StandardGemCuts::reconstruct_validated_brep(&angles, 96);
    assert_ne!(
        hull.facet_planes.len(),
        srb_len,
        "a well-formed custom schedule must not silently fall back to standard_round_brilliant()"
    );
    assert_eq!(hull.facet_planes.len(), 16);
    let (v, _e, f) = assert_euler_formula(&hull);
    assert_eq!(
        v, 10,
        "an 8-fold symmetric crown+pavilion bipyramid should have 8 equatorial vertices + 2 apexes"
    );
    assert_eq!(f, 16, "all 16 facets (8 crown + 8 pavilion) should surface");
    assert!(
        hull.untouched_planes().is_empty(),
        "this hand-built schedule should not leave any plane untouched, got: {:?}",
        hull.untouched_planes()
    );
}

// ---------------------------------------------------------------------------
// `StandardGemCuts::from_asc_schedule` / `reconstruct_validated_brep_from_asc`.
//
// These fixtures are real `.asc` files pulled verbatim from `facet_diagrams.sqlite`
// (via `indicatrix_formats::asc::parse_asc`), not hand-authored test data, so that the
// tests exercise the actual empirically-determined sign/offset conventions against
// real designs whose published `lw_ratio` / `facets_count` (from `diagram_details`)
// are known independently. See `apps/diagram-loader/examples/asc_corpus_report.rs` for the
// same cross-check run across the full corpus.
// ---------------------------------------------------------------------------

/// `attached_files` id 4208 ("pc45149.asc") -- "PC 45.149 Round Trichecker-12" by Fred
/// W. Van Sant. Published: `lw_ratio` = 1.000, `facets_count` = "36+12" (48 total).
const ASC_ROUND_TRICHECKER_12: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 6 y\n\
I 1.72\n\
H PC 45.149  Round Trichecker-12\n\
H by Fred W. Van Sant, X 51, Extra Designs 2000\n\
H Released into the public domain in memory of Charles L. Moon\n\
a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
a 10.000000 0.48799664 96 n C 16 32 48 64 80\n\
F \"For small stones\"\n";

/// `attached_files` id 4210 ("pc46019.asc") -- "PC 46.019 For Fun" by Michiko Huyhn.
/// Published: `lw_ratio` = 1.631, `facets_count` = "48+8" (56 total). Exercises an unsigned
/// zero-angle culet-like tier ("U") with no explicit crown/pavilion marker.
const ASC_FOR_FUN: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 1 y\n\
I 1.54\n\
H PC 46.019  For Fun\n\
H by Michiko Huyhn\n\
a -44.864054 0.53791082 84 n P1 12 G Cut to mast depth X.\n\
a -50.185680 0.48593919 71 n P2 25 G Cut to mast depth X.\n\
a -48.722313 0.50066786 67 n P3 29 G Cut to mast depth X.\n\
a -43.200000 0.55323049 34 62 n P4 G Cut to mast depth X.\n\
a -90.000000 0.78956831 36 12 84 n G1 60 n G1 G Set stone size.\n\
a -90.000000 0.58736554 69 n G2 27 G Meet P1, P2, G1\n\
a -69.917066 0.54241199 69 n P5 27 G Level girdle.\n\
a -63.805515 0.54369547 65 n P6 31 G Meet P2, P3, P5\n\
a -90.000000 0.61916050 65 n G3 31 G Level girdle.\n\
a -50.270584 0.59003581 60 n P7 36 G Level girdle.\n\
a 54.575729 0.70935195 12 n C1 84 G Set girdle width.\n\
a 43.584337 0.48735616 27 n C2 69 G Level girdle.\n\
a 43.883301 0.51120026 65 31 n C3 G Level girdle.\n\
a 40.781358 0.60187658 60 36 n C4 G Level girdle.\n\
a 42.551058 0.68288357 10 n C5 86 G Meet G1, C1\n\
a 42.551058 0.62897635 14 n C6 82 G Meet G1, G2, C1, C2\n\
a 36.064955 0.49509639 24 n C7 72 G Meet G1, G2, C1, C2, C6\n\
a 39.972596 0.47212999 28 n C8 68 G Meet G2, G3, C2, C3\n\
a 40.114031 0.47905350 29 n C9 67 G Meet G2, G3, C2, C3, C8\n\
a 40.468111 0.51438869 64 32 n C10 G Meet G1, G3, C3, C4\n\
a 37.224985 0.62845267 12 n C11 84 G Meet C1, C5, C6\n\
a 29.739521 0.46322909 26 n C12 70 G Meet C2, C7, C8; C6, C7, C11\n\
a 36.239767 0.49263893 31 n C13 65 G Meet C3, C9, C10\n\
a 22.049986 0.47954099 65 31 n C14 G Meet C8, C9, C12, C13; C10, C13\n\
a 6.000000 0.46712247 48 n C15 G Meet C10, C13, C14\n\
a 21.597048 0.45849715 26 n C16 70 G Meet C6, C7, C11, C12; C8, C9, C12, C13, C14\n\
a 24.656793 0.57799286 12 n C17 84 G Meet C5, C11; C6, C7, C11, C12, C16\n\
a 0.000000 0.44755829 96 n U\n\
F Also USFG Newsletter Sep 2013, Facets Jan 2014\n";

/// `attached_files` id 4430 ("pc42060.asc") -- "PC 42.060 Large Texas Star" by
/// Charles `McCoy`. Published: `lw_ratio` = 1.051, `facets_count` = "41+10" (51 total). Gear=80 (not
/// the far more common 96), symmetry order 5 -- exercises both away from the
/// dominant convention, plus an explicit table tier at unsigned zero.
const ASC_LARGE_TEXAS_STAR: &str = "GemCad 5.0\n\
g 80 0.0\n\
y 5 y\n\
I 1.61\n\
H PC 42.060  Large Texas Star\n\
H by Charles McCoy\n\
a -40.000000 0.54589773 76 n 1 68 60 52 44 36 28 20 12 4 G TCP\n\
a -90.000000 1.05672946 76 n 2 68 60 52 44 36 28 20 12 4 G Size stone\n\
a -67.800000 0.78700478 76 n 3 68 60 52 44 36 28 20 12 4 G Determine the size of the star\n\
a -37.310000 0.53454720 78 n 4 66 62 50 46 34 30 18 14 2 G MP 1-3\n\
a 40.000000 1.11585176 4 n A 12 20 28 36 44 52 60 68 76 G Establish girdle thickness\n\
a 0.000000 0.72641642 80 n T G Make table large enough to show all of the star\n\
F Leave #4 frosted\n";

/// `attached_files` id 4422 ("pc43001a.asc") -- "PC 43.001A Shah (Replica)". No facet
/// names anywhere, a rare negative-mast tier at an unsigned zero angle, and two
/// tiers (`-90 ... 0 32` and a later `-90 ... 0`) that share an index and mast,
/// producing a literal duplicate half-space plane. Exercises `dedup_planes` and the
/// "no name at all" parsing path, not the L/W or facet-count cross-check (this
/// design's own footnote flags it as disagreeing with the reference it's replicating).
const ASC_SHAH_REPLICA_NO_NAMES: &str = "GemCad 4.51\n\
g 64 64.0\n\
y 1 n\n\
I 1.54\n\
H PC 43.001A Shah (Replica)\n\
a -90.00 1.00000 16\n\
a -90.00 0.44700 0 32\n\
a 0.00 -0.36800 0\n\
a -90.00 0.99518 49 47\n\
a 1.87 0.34210 49 47\n\
a -90.00 0.44700 0\n\
a 69.84 0.87020 16\n\
a 85.00 0.44530 0\n\
a -71.11 0.90450 48\n\
a 20.59 0.39882 30\n\
a 24.47 0.38860 1.7\n\
F Does not agree with Barbour's 43.001. Glass replica has rounded facets on the ends.\n";

/// Same length/width measure the corpus-wide report in
/// `apps/diagram-loader/examples/asc_corpus_report.rs` uses: the longest chord across the reconstructed
/// girdle outline as length, and the outline's extent perpendicular to that chord as
/// width.
fn length_width_ratio(hull: &GemPolyhedron) -> f64 {
    let outline = hull.girdle_outline();
    let mut best = (0usize, 0usize, 0.0f32);
    for i in 0..outline.len() {
        for j in (i + 1)..outline.len() {
            let d = (outline[i] - outline[j]).length();
            if d > best.2 {
                best = (i, j, d);
            }
        }
    }
    let length = best.2;
    let dir = (outline[best.1] - outline[best.0]).normalize();
    let perp = Vec3::new(-dir.z, 0.0, dir.x);
    let (mut min_p, mut max_p) = (f32::MAX, f32::MIN);
    for p in &outline {
        let proj = p.dot(perp);
        min_p = min_p.min(proj);
        max_p = max_p.max(proj);
    }
    f64::from(length / (max_p - min_p))
}

#[test]
fn asc_real_designs_reconstruct_closed_solids_with_no_untouched_planes() {
    for (label, content) in [
        ("Round Trichecker-12", ASC_ROUND_TRICHECKER_12),
        ("For Fun", ASC_FOR_FUN),
        ("Large Texas Star", ASC_LARGE_TEXAS_STAR),
        ("Shah Replica (no names)", ASC_SHAH_REPLICA_NO_NAMES),
    ] {
        let schedule = asc::parse_asc(content)
            .unwrap_or_else(|e| panic!("{label}: real .asc sample must parse: {e}"));
        let hull =
            StandardGemCuts::reconstruct_validated_brep_from_asc(&schedule).unwrap_or_else(|e| {
                panic!("{label}: real .asc sample must reconstruct a valid closed solid: {e}")
            });
        assert_euler_formula(&hull);
        assert!(
            hull.untouched_planes().is_empty(),
            "{label}: every plane in a real, well-formed schedule should be touched"
        );
        assert!(
            hull.volume().is_finite() && hull.volume() > 0.0,
            "{label}: volume must be finite and positive"
        );
    }
}

#[test]
fn asc_round_trichecker_12_matches_published_lw_ratio_and_facet_count() {
    let schedule = asc::parse_asc(ASC_ROUND_TRICHECKER_12).unwrap();
    let hull = StandardGemCuts::reconstruct_validated_brep_from_asc(&schedule).unwrap();
    assert_eq!(
        hull.facet_planes.len(),
        48,
        "published facets_count is \"36+12\" = 48"
    );
    let lw = length_width_ratio(&hull);
    assert!(
        (lw - 1.000).abs() < 0.02,
        "published lw_ratio is 1.000, got {lw:.4}"
    );
}

#[test]
fn asc_for_fun_matches_published_lw_ratio_and_facet_count() {
    // Tier-count/angle/distance assertions on top of indicatrix_formats::asc's own
    // parser-level unit tests: 28 tiers (10 pavilion/girdle + 17 crown + 1 culet), the
    // P1 tier's angle and mast distance, and the geometry this schedule produces.
    let schedule = asc::parse_asc(ASC_FOR_FUN).unwrap();
    assert_eq!(schedule.tiers.len(), 28);
    assert_eq!(schedule.gear_teeth, 96);
    assert!((schedule.refractive_index - 1.54).abs() < 1e-9);
    assert!((schedule.tiers[0].angle_deg - (-44.864_054)).abs() < 1e-6);
    assert!((schedule.tiers[0].mast - 0.537_910_82).abs() < 1e-9);

    let hull = StandardGemCuts::reconstruct_validated_brep_from_asc(&schedule).unwrap();
    assert_eq!(
        hull.facet_planes.len(),
        56,
        "published facets_count is \"48+8\" = 56"
    );
    let lw = length_width_ratio(&hull);
    assert!(
        (lw - 1.631).abs() < 0.02,
        "published lw_ratio is 1.631, got {lw:.4}"
    );
}

#[test]
fn asc_large_texas_star_matches_published_lw_ratio_and_facet_count() {
    // gear=80 (not 96) and an explicit unsigned-zero table tier ("T") -- both away
    // from the more common case the other fixtures exercise.
    let schedule = asc::parse_asc(ASC_LARGE_TEXAS_STAR).unwrap();
    assert_eq!(schedule.gear_teeth, 80);
    assert_eq!(schedule.symmetry_order, 5);

    let hull = StandardGemCuts::reconstruct_validated_brep_from_asc(&schedule).unwrap();
    assert_eq!(
        hull.facet_planes.len(),
        51,
        "published facets_count is \"41+10\" = 51"
    );
    let lw = length_width_ratio(&hull);
    assert!(
        (lw - 1.051).abs() < 0.02,
        "published lw_ratio is 1.051, got {lw:.4}"
    );
}

#[test]
fn asc_schedules_always_satisfy_from_planes_d_negative_precondition() {
    for content in [
        ASC_ROUND_TRICHECKER_12,
        ASC_FOR_FUN,
        ASC_LARGE_TEXAS_STAR,
        ASC_SHAH_REPLICA_NO_NAMES,
    ] {
        let schedule = asc::parse_asc(content).unwrap();
        for p in StandardGemCuts::from_asc_schedule(&schedule) {
            assert!(
                p.d < 0.0,
                "from_asc_schedule produced a plane with d = {} >= 0",
                p.d
            );
        }
    }
}

#[test]
fn asc_schedule_negative_mast_and_duplicate_tiers_do_not_panic_or_break_geometry() {
    // ASC_SHAH_REPLICA_NO_NAMES has a negative-mast tier ("0.00 -0.36800 0") and two
    // tiers that produce a literal duplicate half-space plane (index 0 at angle -90,
    // mast 0.44700, listed on two separate rows). Neither should panic, and the
    // duplicate should be silently absorbed by dedup_planes rather than tripping
    // GemPolyhedron::from_planes's "coincident planes" rejection.
    let schedule = asc::parse_asc(ASC_SHAH_REPLICA_NO_NAMES).unwrap();
    let planes = StandardGemCuts::from_asc_schedule(&schedule);
    for p in &planes {
        assert!(
            p.d < 0.0,
            "negative mast must still produce d < 0 (magnitude, not sign, is used), got d={}",
            p.d
        );
    }
    let hull = GemPolyhedron::from_planes(planes)
        .expect("duplicate half-space planes must be deduped before reaching from_planes");
    assert_euler_formula(&hull);
}

#[test]
fn asc_geometry_falls_back_cleanly_when_no_asc_is_available() {
    // Designs without an attached .asc (about 4.8% of the catalog) must still render
    // via the existing angle_settings-based path -- from_asc_schedule/
    // reconstruct_validated_brep_from_asc are purely additive.
    let angles = vec![FacetSpec {
        facet: "C1".into(),
        angle: "40.00\u{b0}".into(),
        index: "8 girdle facets".into(),
        notes: String::new(),
    }];
    let planes = StandardGemCuts::from_database_angles(&angles, 96);
    assert!(
        !planes.is_empty(),
        "the angle_settings fallback path must remain usable independent of from_asc_schedule"
    );
}
