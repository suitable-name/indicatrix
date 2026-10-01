//! Unit tests for [`GemPolyhedron::from_planes`]: exact cube topology and
//! measures, scale invariance, the determinism guarantee (with a pinned golden),
//! input validation, and the error payloads.

use super::*;
use crate::geometry::cuts::StandardGemCuts;

/// Six axis-aligned half-space planes bounding a cube of the given half-extent
/// centered on the origin. `GpuFacetPlane::new` requires `d < 0`, so `d =
/// -half_extent`.
fn cube_planes(half_extent: f32) -> Vec<GpuFacetPlane> {
    vec![
        GpuFacetPlane::new(Vec3::X, -half_extent),
        GpuFacetPlane::new(-Vec3::X, -half_extent),
        GpuFacetPlane::new(Vec3::Y, -half_extent),
        GpuFacetPlane::new(-Vec3::Y, -half_extent),
        GpuFacetPlane::new(Vec3::Z, -half_extent),
        GpuFacetPlane::new(-Vec3::Z, -half_extent),
    ]
}

/// `n` tangent planes of the unit sphere at Fibonacci-lattice points: a dense,
/// fully non-redundant input with only degree-3 vertices.
fn fibonacci_sphere(n: usize) -> Vec<GpuFacetPlane> {
    let golden_angle = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    (0..n)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f64 + 0.5) / n as f64;
            let r = y.mul_add(-y, 1.0).sqrt();
            let theta = golden_angle * i as f64;
            let normal = DVec3::new(r * theta.cos(), y, r * theta.sin()).as_vec3();
            GpuFacetPlane::new(normal, -1.0)
        })
        .collect()
}

/// FNV-1a over 32-bit words; a fixed, platform-independent hash for goldens.
struct Fnv(u64);

impl Fnv {
    const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn word(&mut self, v: u32) {
        for byte in v.to_le_bytes() {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// Hash of every output bit: vertices, polygons, triangles, radius, volume, areas
/// and the girdle outline.
fn output_hash(hull: &GemPolyhedron) -> u64 {
    let mut h = Fnv::new();
    let mut words: Vec<u32> = Vec::new();
    words.extend(
        hull.vertices
            .iter()
            .flat_map(|v| v.to_array().map(f32::to_bits)),
    );
    for poly in &hull.facet_polygons {
        words.push(poly.len() as u32);
        words.extend_from_slice(poly);
    }
    words.extend_from_slice(&hull.triangle_indices);
    words.push(hull.bounding_radius.to_bits());
    words.push(hull.volume().to_bits());
    words.extend(hull.facet_areas().iter().map(|a| a.to_bits()));
    words.extend(
        hull.girdle_outline()
            .iter()
            .flat_map(|v| v.to_array().map(f32::to_bits)),
    );
    for w in words {
        h.word(w);
    }
    h.0
}

/// Recomputes `V - E + F` and manifoldness from the public fields.
fn assert_closed_manifold(hull: &GemPolyhedron) {
    check_euler(hull.vertices.len(), &hull.facet_polygons)
        .expect("reconstruction must be a closed 2-manifold satisfying Euler's formula");
}

#[test]
fn from_planes_cube_has_expected_topology() {
    let hull = GemPolyhedron::from_planes(cube_planes(1.0)).expect("unit cube must reconstruct");
    assert_eq!(hull.vertices.len(), 8);
    assert_eq!(hull.facet_polygons.len(), 6);
    assert!(hull.facet_polygons.iter().all(|p| p.len() == 4));
    assert_eq!(hull.triangle_indices.len(), 36); // 6 faces * 2 triangles * 3 indices
}

#[test]
fn from_planes_is_scale_invariant_for_cube_topology() {
    // A cube scaled 1,000,000x in linear size (1000x and 0.001x here) must produce
    // the same vertex count, facet count and triangle count as the unit-scale
    // cube: offsets are normalised by an exact power of two before any tolerance is
    // applied, so topology reconstruction is independent of the caller's units.
    let unit =
        GemPolyhedron::from_planes(cube_planes(1.0)).expect("unit-scale cube must reconstruct");

    for &scale in &[1000.0_f32, 0.001_f32] {
        let scaled = GemPolyhedron::from_planes(cube_planes(scale))
            .unwrap_or_else(|e| panic!("cube scaled by {scale} must reconstruct: {e}"));
        assert_eq!(scaled.vertices.len(), unit.vertices.len());
        assert_eq!(scaled.facet_polygons, unit.facet_polygons);
        assert_eq!(scaled.triangle_indices, unit.triangle_indices);

        let expected_radius = unit.bounding_radius * scale;
        let rel_err = (scaled.bounding_radius - expected_radius).abs() / expected_radius;
        assert!(
            rel_err < 1e-6,
            "bounding radius should scale linearly (scale = {scale}): got {}, expected ~{expected_radius}",
            scaled.bounding_radius
        );
    }
}

#[test]
fn from_planes_round_brilliant_still_reconstructs_with_f64_solve() {
    // Sanity check on the real, non-trivial cutting instructions (57 facets, many
    // special points where more than 3 planes meet at once and rely on welding)
    // rather than just the trivial cube, across a wide scale sweep: the volume floor
    // is relative to the bounding radius, so no scale approaches it.
    let planes = StandardGemCuts::standard_round_brilliant();
    for k in [1e-6_f32, 1e-3, 1.0, 1e3, 1e6] {
        let scaled: Vec<GpuFacetPlane> = planes
            .iter()
            .map(|p| GpuFacetPlane {
                normal: p.normal,
                d: p.d * k,
            })
            .collect();
        let hull = GemPolyhedron::from_planes(scaled)
            .unwrap_or_else(|e| panic!("SRB scaled by {k} must reconstruct: {e}"));
        assert_eq!(hull.untouched_planes(), Vec::<usize>::new());
        assert_closed_manifold(&hull);
    }
}

/// Golden [`output_hash`] of `standard_round_brilliant()`. Pinned on x86-64 Windows
/// only, and compared only there (the constant and the exact-hash assertion are both
/// gated on that target). The reconstruction itself is IEEE-exact (the SIMD triple
/// solve is bit-identical to the scalar `glam` sequence, and `atan2` only orders ring
/// vertices, never enters an output value), but the input planes come from `f32`
/// `sin`/`cos` in the platform's libm, so other targets may differ by libm rounding.
/// There `srb_reconstruction_satisfies_its_structural_invariants` checks the
/// target-independent properties instead. A change here on the pinned platform means
/// the reconstruction's output bits changed: review the change, then re-pin.
#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
const SRB_OUTPUT_HASH: u64 = 0x59e3_e00f_6504_40ce;

#[test]
fn from_planes_is_bitwise_deterministic() {
    for (label, planes) in [
        ("srb", StandardGemCuts::standard_round_brilliant()),
        ("emerald", StandardGemCuts::emerald_cut()),
        ("cube", cube_planes(1.0)),
    ] {
        let hashes: Vec<u64> = (0..5)
            .map(|_| output_hash(&GemPolyhedron::from_planes(planes.clone()).expect(label)))
            .collect();
        assert!(
            hashes.iter().all(|&h| h == hashes[0]),
            "{label}: output bits differ between calls: {hashes:x?}"
        );
        #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
        if label == "srb" {
            assert_eq!(
                hashes[0], SRB_OUTPUT_HASH,
                "SRB output hash moved: got {:#018x}",
                hashes[0]
            );
        }
    }
}

/// Target-independent checks on the round brilliant reconstruction, which hold whatever
/// the platform's `sin`/`cos` round to:
///
/// - every plane contributes a facet and the result is a closed 2-manifold with
///   `V - E + F = 2` (`assert_closed_manifold`);
/// - the volume agrees with the pyramid decomposition about the origin,
///   `V = (1/3) * sum_i area_i * dist_i`, where `dist_i = -d_i` is plane `i`'s distance
///   from the origin (every `d_i < 0` and the origin is inside). This uses the facet
///   areas and the input offsets, not the triangle mesh `volume()` sums over, so it is an
///   independent route. Both results are `f32`-rounded (relative `6e-8` each, and the
///   sum over 57 facets adds at most that again per term), so the tolerance is `1e-5`
///   relative; the `1e-9` a full `f64` comparison would allow is not reachable through
///   the `f32` accessors.
#[test]
fn srb_reconstruction_satisfies_its_structural_invariants() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let hull = GemPolyhedron::from_planes(planes.clone()).expect("SRB must reconstruct");

    assert_eq!(hull.untouched_planes(), Vec::<usize>::new());
    assert_closed_manifold(&hull);

    let pyramid_volume: f64 = hull
        .facet_areas()
        .iter()
        .zip(&planes)
        .map(|(&area, plane)| f64::from(area) * f64::from(-plane.d))
        .sum::<f64>()
        / 3.0;
    let volume = f64::from(hull.volume());
    let rel = (volume - pyramid_volume).abs() / pyramid_volume;
    assert!(
        rel < 1e-5,
        "mesh volume {volume} vs pyramid decomposition {pyramid_volume}: relative {rel}"
    );
}

#[test]
fn pair_prune_visits_exactly_the_unpruned_triples() {
    // The walk's pair prune is only a speed-up: it must hand `from_planes` the very
    // same triples, dets and positions, bit for bit, as the full `O(P^3)` walk.
    let collect = |planes: &[(DVec3, f64)], prune: bool| {
        let mut seen: Vec<([usize; 3], u64, [u64; 3])> = Vec::new();
        let closed = for_each_feasible_triple(planes, ENUMERATION_DET_FLOOR, prune, |t, det, v| {
            seen.push((t, det.to_bits(), v.to_array().map(f64::to_bits)));
        });
        (closed.is_some(), seen)
    };
    let mut open_cube = cube_planes(1.0);
    open_cube.remove(4);
    for (label, planes) in [
        ("srb", StandardGemCuts::standard_round_brilliant()),
        ("emerald", StandardGemCuts::emerald_cut()),
        ("fib100", fibonacci_sphere(100)),
        ("open cube", open_cube),
    ] {
        let (halfspaces, _) = normalised_halfspaces(&planes);
        let full = collect(&halfspaces, false);
        assert!(!full.1.is_empty(), "{label}: fixture must have vertices");
        assert_eq!(collect(&halfspaces, true), full, "{label}");
    }
}

#[test]
fn from_planes_rejects_nan_and_inf_planes() {
    let with = |edit: &dyn Fn(&mut Vec<GpuFacetPlane>)| {
        let mut planes = cube_planes(1.0);
        edit(&mut planes);
        GemPolyhedron::from_planes(planes).expect_err("malformed plane must be rejected")
    };
    let extra = |normal: [f32; 3], d: f32| {
        move |p: &mut Vec<GpuFacetPlane>| p.push(GpuFacetPlane { normal, d })
    };

    assert_eq!(
        with(&|p| p[0].d = f32::NAN),
        BrepError::NonFinitePlane { index: 0 }
    );
    assert_eq!(
        with(&|p| p[2].d = f32::NEG_INFINITY),
        BrepError::NonFinitePlane { index: 2 }
    );
    assert_eq!(
        with(&|p| p[0].normal[0] = f32::NAN),
        BrepError::NonFinitePlane { index: 0 }
    );
    assert_eq!(
        with(&|p| p[1].normal[2] = f32::INFINITY),
        BrepError::NonFinitePlane { index: 1 }
    );
    assert_eq!(
        with(&extra([0.6, 0.0, 0.8], f32::NAN)),
        BrepError::NonFinitePlane { index: 6 }
    );
    assert_eq!(
        with(&extra([0.0, 0.0, 0.0], -1.0)),
        BrepError::NonUnitNormal {
            index: 6,
            length: 0.0
        }
    );
    assert_eq!(
        with(&|p| p[0].normal = [2.0, 0.0, 0.0]),
        BrepError::NonUnitNormal {
            index: 0,
            length: 2.0
        }
    );
    assert_eq!(
        with(&|p| p[3].d = -0.0),
        BrepError::NonNegativeOffset { index: 3, d: -0.0 }
    );
}

#[test]
fn coincident_check_is_scale_invariant() {
    // The coincidence test used to take an absolute `max(1.0)` floor,
    // so a cube at scale 1e12 had its opposite faces called coincident and a plane
    // 0.005 % outside a face was "coincident" at scale 1000. Both are now judged
    // purely relatively, after pow2 normalisation.
    for k in [1e-3_f32, 1.0, 1e3, 1e6, 1e12] {
        let hull = GemPolyhedron::from_planes(cube_planes(k))
            .unwrap_or_else(|e| panic!("cube at scale {k} must reconstruct: {e}"));
        assert_eq!(hull.vertices.len(), 8, "scale {k}");

        let mut planes = cube_planes(k);
        planes.push(GpuFacetPlane::new(Vec3::X, -k * 1.000_05));
        let hull = GemPolyhedron::from_planes(planes)
            .unwrap_or_else(|e| panic!("redundant plane at scale {k} must not be coincident: {e}"));
        assert_eq!(hull.untouched_planes(), vec![6], "scale {k}");
        let expected = 8.0 * f64::from(k).powi(3);
        let rel = (f64::from(hull.volume()) - expected).abs() / expected;
        assert!(
            rel < 1e-6,
            "scale {k}: volume {} vs {expected}",
            hull.volume()
        );
    }
    let mut planes = cube_planes(1e12);
    planes.push(planes[4]);
    assert_eq!(
        GemPolyhedron::from_planes(planes).expect_err("exact duplicate"),
        BrepError::CoincidentPlanes { i: 4, j: 6 }
    );
}

#[test]
fn origin_near_a_facet_still_reconstructs() {
    // chull's absolute degeneracy thresholds rejected these (P1-2): the nearest
    // plane's dual point dominated the dual cloud. The primal enumeration has no
    // dependence on where the origin sits inside the solid.
    let mut planes = cube_planes(1.0);
    planes[0].d = -0.01;
    let hull = GemPolyhedron::from_planes(planes).expect("off-centre cube must reconstruct");
    assert_eq!(hull.vertices.len(), 8);
    assert!(
        (hull.volume() - 4.04).abs() < 1e-5,
        "volume {}",
        hull.volume()
    );

    // SRB with its table pushed toward the origin until min|d| / max|d| = 0.06
    // (chull failed from 0.080 down).
    let mut srb = StandardGemCuts::standard_round_brilliant();
    let max_d = srb.iter().map(|p| -p.d).fold(0.0_f32, f32::max);
    for p in srb.iter_mut().filter(|p| p.normal[1] > 0.999) {
        p.d = -0.06 * max_d;
    }
    let hull = GemPolyhedron::from_planes(srb).expect("SRB with a low table must reconstruct");
    assert_closed_manifold(&hull);
}

#[test]
fn non_manifold_error_payload_is_deterministic() {
    // P3-7: the edge count used to live in a `HashMap`, so the reported edge was
    // whichever the per-process hasher visited first. This open patch has nine
    // bad edges ((0, 1) and (2, 3) are fine, each used twice); in every facet
    // order the smallest bad edge, (0, 3), is reported.
    //
    // Driven through `check_euler` directly: with incidence restricted to exact
    // polytope vertices and edges collapsing only under the weld, no plane input
    // tried here reaches a non-manifold mesh any more (the old reachable case, a
    // near-duplicate plane claiming a facet twice, is now simply untouched).
    let facets: Vec<Vec<u32>> = vec![
        vec![0, 1, 2, 3],
        vec![4, 5, 6],
        vec![0, 1, 7],
        vec![3, 2, 5],
    ];
    let expected = BrepError::NonManifoldEdge {
        edge: (0, 3),
        count: 1,
    };
    let mut permuted = facets;
    for _ in 0..4 {
        assert_eq!(check_euler(8, &permuted).expect_err("open patch"), expected);
        permuted.rotate_left(1);
        for facet in &mut permuted {
            facet.rotate_left(1);
        }
    }

    // The formerly non-manifold input: a second +X plane 2e-6 further out.
    let mut planes = cube_planes(1.0);
    planes.push(GpuFacetPlane::new(Vec3::X, -1.000_002));
    let hull = GemPolyhedron::from_planes(planes).expect("near-duplicate plane");
    assert_eq!(hull.untouched_planes(), vec![6]);
}

#[test]
fn redundant_plane_touching_a_vertex_is_untouched() {
    // A plane through the (1, 1, 1) corner only touches that one vertex. The corner
    // keeps the position of its best-conditioned triple, the three axis planes
    // (`|det| = 1` against `1/sqrt(3)`), so the volume stays exactly 8.
    let mut planes = cube_planes(1.0);
    planes.push(GpuFacetPlane::new(Vec3::ONE, -3f32.sqrt()));
    let hull = GemPolyhedron::from_planes(planes).expect("corner plane");
    assert_eq!(hull.untouched_planes(), vec![6]);
    assert_eq!(hull.vertices.len(), 8);
    assert!(hull.vertices.contains(&Vec3::ONE));
    assert_eq!(hull.volume(), 8.0);

    // One a relative 1e-6 deeper cuts off a corner tetrahedron with legs of 3e-6,
    // far below the weld radius: its three vertices weld into one (a documented
    // meet-tolerance effect), so the plane touches a single vertex and is still
    // reported untouched. The welded vertex sits on the cut, not at the old corner.
    let mut planes = cube_planes(1.0);
    planes.push(GpuFacetPlane::new(Vec3::ONE, -3f32.sqrt() * 0.999_999));
    let hull = GemPolyhedron::from_planes(planes).expect("micro corner cut");
    assert_eq!(hull.untouched_planes(), vec![6]);
    assert_eq!(hull.vertices.len(), 8);
    assert!(!hull.vertices.contains(&Vec3::ONE));
    assert!(
        (hull.volume() - 8.0).abs() < 1e-4,
        "volume {}",
        hull.volume()
    );
}

#[test]
fn vertices_are_feasible_for_every_plane() {
    for (label, planes) in [
        ("srb", StandardGemCuts::standard_round_brilliant()),
        ("emerald", StandardGemCuts::emerald_cut()),
        ("fib100", fibonacci_sphere(100)),
    ] {
        let hull = GemPolyhedron::from_planes(planes.clone()).expect(label);
        let tolerance = 1e-5 * f64::from(hull.bounding_radius);
        for (vi, v) in hull.vertices.iter().enumerate() {
            for (pi, plane) in planes.iter().enumerate() {
                let (n, m) = plane.to_halfspace_f64();
                let excess = n.dot(v.as_dvec3()) - m;
                assert!(
                    excess <= tolerance,
                    "{label}: vertex {vi} lies {excess:e} outside plane {pi}"
                );
            }
        }
    }
}

#[test]
fn volume_floor_is_scale_relative() {
    // The old absolute `volume < 1e-9` floor rejected a cube of half-extent 1e-4.
    for k in [1e-6_f32, 1e-4] {
        let hull = GemPolyhedron::from_planes(cube_planes(k))
            .unwrap_or_else(|e| panic!("cube of half-extent {k} must reconstruct: {e}"));
        let expected = 8.0 * f64::from(k).powi(3);
        let rel = (f64::from(hull.volume()) - expected).abs() / expected;
        assert!(rel < 1e-6, "half-extent {k}: volume {}", hull.volume());
    }
}

#[test]
fn cube_facet_areas_and_volume_are_exact() {
    let hull = GemPolyhedron::from_planes(cube_planes(1.0)).expect("cube");
    assert_eq!(hull.facet_areas(), vec![4.0; 6]);
    assert_eq!(hull.volume(), 8.0);
    assert_eq!(hull.bounding_radius, 3f32.sqrt());
    for (plane, poly) in hull.facet_planes.iter().zip(&hull.facet_polygons) {
        // Counter-clockwise seen from outside: the fan's normal points along the
        // plane's outward normal.
        let [a, b, c] = [poly[0], poly[1], poly[2]].map(|i| hull.vertices[i as usize]);
        let winding = (b - a).cross(c - a).dot(Vec3::from_array(plane.normal));
        assert!(winding > 0.0, "facet ring must be outward CCW: {poly:?}");
        assert_eq!(poly[0], *poly.iter().min().expect("non-empty"));
    }
}

#[test]
fn girdle_outline_of_cube_is_its_square_at_any_scale() {
    // The old absolute 1e-6 dedup collapsed a cube of half-extent 1e-7 to one point.
    for k in [1.0_f32, 1e-7] {
        let hull = GemPolyhedron::from_planes(cube_planes(k)).expect("cube");
        let outline = hull.girdle_outline();
        let mut corners: Vec<(f32, f32)> = outline.iter().map(|v| (v.x / k, v.z / k)).collect();
        corners.sort_by(|p, q| p.0.total_cmp(&q.0).then(p.1.total_cmp(&q.1)));
        assert_eq!(
            corners,
            vec![(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)],
            "half-extent {k}"
        );
    }
}

#[test]
fn vertex_meet_groups_order_is_stable() {
    // Every SRB plane is touched, so each vertex's meet group is exactly its
    // incident set, and vertices are sorted by incident set: the groups come out
    // strictly ascending, identically on every call.
    let groups = |_: usize| {
        let hull =
            GemPolyhedron::from_planes(StandardGemCuts::standard_round_brilliant()).expect("SRB");
        crate::geometry::meet_solver::vertex_meet_groups(&hull)
    };
    let first = groups(0);
    assert!(
        first.windows(2).all(|w| w[0] < w[1]),
        "groups must be strictly ascending"
    );
    assert!(first.iter().all(|g| g.len() >= 3 && g.is_sorted()));
    for call in 1..3 {
        assert_eq!(groups(call), first);
    }
}

#[test]
fn fibonacci_sphere_600_planes_is_ok_and_deterministic() {
    // chull returned non-convex hulls and flipped between Ok and Err from ~500 planes
    // up (600 always failed). The primal enumeration is exact about convexity.
    let planes = fibonacci_sphere(600);
    let first = GemPolyhedron::from_planes(planes.clone()).expect("600-plane sphere");
    let second = GemPolyhedron::from_planes(planes).expect("600-plane sphere");
    assert_eq!(output_hash(&first), output_hash(&second));
    assert_eq!(first.untouched_planes(), Vec::<usize>::new());
    assert_closed_manifold(&first);
    let sphere = 4.0 / 3.0 * std::f32::consts::PI;
    assert!(first.volume() > sphere && first.volume() < 1.01 * sphere);
}
