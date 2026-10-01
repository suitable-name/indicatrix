//! B-Rep reconstruction tests (`geometry::brep::GemPolyhedron::from_planes`):
//! topology checks via Euler's formula and exact counts for a hand-verifiable cube,
//! degenerate-input rejection, and the `reconstruct_validated_brep` fallback
//! behaviour.

use glam::Vec3;
use indicatrix::{
    FacetSpec,
    geometry::{
        brep::{BrepError, GemPolyhedron},
        cuts::StandardGemCuts,
        plane::GpuFacetPlane,
    },
};

use crate::fixtures::assert_euler_formula;

// ---------------------------------------------------------------------------
// B-Rep reconstruction (`geometry::brep::GemPolyhedron::from_planes`).
//
// `from_planes` used to build the dual-space convex hull (GEMSTONE_RENDERING_BLUEPRINT.md
// section 1.2) with the `chull` crate. Since 2026-09-28 it enumerates the primal plane
// arrangement instead (the same deterministic walk `stone_metrics` uses), welds triple
// solutions where more than 3 planes meet (common in symmetric cuts, e.g.
// round-brilliant girdle/kite/star junctions) into one vertex, and orders every facet
// ring canonically, so its output is byte-identical call to call.
//
// These tests check the result via Euler's formula (the single most valuable
// topological check), exact counts for a hand-verifiable cube, the degenerate-input
// handling, and the `reconstruct_validated_brep` fallbacks. The determinism, scale and
// validation unit tests live next to the code in `src/geometry/brep/tests.rs`.
// ---------------------------------------------------------------------------

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
    // points ~1.5e-4 apart -- just outside the weld radius (`VERTEX_WELD_EPS_REL`,
    // 1e-4 of the solid's radius) -- which welded incompletely and reconstructed 60
    // vertices instead of 48 (same 34/34-touched, same ~1.8307 volume, so neither of
    // those alone would have caught it).
    assert_eq!(
        hull.vertices.len(),
        48,
        "emerald_cut() should reconstruct to exactly 48 vertices; a higher count here \
         (e.g. 60) means intended-coincident meet points drifted outside the weld radius \
         (VERTEX_WELD_EPS_REL) again, most likely because a tier offset went back to \
         being a rounded literal instead of being derived from the shared profile"
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
    // The escaping vertices are the four side planes meeting the blank box's +Z
    // face; the smallest of them is +X, index 0.
    assert_eq!(
        err,
        BrepError::UnboundedRegion { plane: Some(0) },
        "expected UnboundedRegion naming plane 0, got: {err:?}"
    );
    let text = err.to_string();
    assert!(
        text.contains("unbounded") || text.contains("finite region"),
        "error should explain the region is unbounded, got: {text}"
    );
}

#[test]
fn brep_rejects_ill_conditioned_near_parallel_triple() {
    // Add a seventh plane tilted 5e-7 rad off the +Y face about the Z axis, through
    // the +Y face's centre line x = 0: `5e-7 x + y <= 1`. The solid stays the bounded
    // unit cube (the new plane shaves at most 5e-7 off the +Y face), and the planes are
    // not coincident (their dual points differ by 5e-7 relative, above the 1e-7
    // coincidence threshold). But the new plane meets +Y and +/-Z at (0, 1, +/-1) with
    // |det| = 5e-7, below the 1e-6 conditioning threshold, and no better-conditioned
    // triple meets there: the vertex position cannot be trusted, so the
    // reconstruction must return a descriptive error rather than a malformed mesh.
    //
    // (The earlier version of this test replaced +X with a plane tilted 0.00005 degrees
    // off +Y. That solid is not "closed comfortably": it is about 2e6 long, and the
    // arrangement enumeration now reports it as an `UnboundedRegion`, which it is at
    // the blank box's 64x-the-largest-offset reach.)
    let mut planes = axis_aligned_cube_planes();
    planes.push(GpuFacetPlane::new(Vec3::new(5e-7, 1.0, 0.0), -1.0));
    let err = GemPolyhedron::from_planes(planes).expect_err(
        "a near-parallel facet triple must be rejected rather than produce a malformed mesh",
    );
    // Vertices are ordered by incident set, so the first ill-conditioned one is the
    // (+Y, +Z, new) vertex; the payload is the same on every call.
    assert!(
        matches!(err, BrepError::IllConditionedTriple { a: 2, b: 4, c: 6, det } if det.abs() < 1e-6),
        "expected IllConditionedTriple {{ a: 2, b: 4, c: 6 }}, got: {err:?}"
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
