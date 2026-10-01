//! Shared test fixtures and reference implementations for
//! [`super::measure_solid`] and [`super::build_solid_mesh`], plus the
//! per-topic test modules that use them.

use glam::DVec3;

use super::{
    BLANK_HALF_EXTENT, EPS_FEAS, MIN_TRIPLE_DET, SolidMesh, SolidStatus, VERTEX_DEDUP,
    build_solid_mesh, measure_solid,
    mesh::face_area,
    vertices::{SolidVertex, dedup_planes, feasible_vertices, for_each_feasible_triple},
};
use crate::geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane};

mod dedup;
mod measure;
mod mesh;
mod proportions;

// -----------------------------------------------------------------------
// The `VertexAccumulator` x-sorted index in `insert_if_new` must make the
// exact same accept/reject decision, in the exact same insertion order,
// as the O(V^2) linear scan it replaces. Proven here by running both side
// by side over the module's own fixtures plus real cutting instructions and
// comparing the resulting vertex lists bit-for-bit.
// -----------------------------------------------------------------------

/// Pre-optimization dedup, kept only as a reference: a duplicate is any
/// already-accepted vertex within [`VERTEX_DEDUP`] on every axis, found
/// by scanning the full accepted set (this is exactly the body
/// `flush_solid_batch` had before `VertexAccumulator` existed).
fn flush_solid_batch_linear_reference(
    batch: &crate::simd::TripleBatch,
    soa: &crate::simd::PlanesSoA64,
    verts: &mut Vec<SolidVertex>,
) -> Option<()> {
    let sol = crate::simd::solve_triple_batch(batch);
    for lane in 0..batch.len {
        if sol.det[lane].abs() < MIN_TRIPLE_DET {
            continue;
        }
        let v = DVec3::new(sol.vx[lane], sol.vy[lane], sol.vz[lane]);
        if v.abs().max_element() > BLANK_HALF_EXTENT + 1.0 {
            continue;
        }
        if crate::simd::any_violation(soa, v, EPS_FEAS) {
            continue;
        }
        if v.abs().max_element() > BLANK_HALF_EXTENT - 1.0 {
            return None;
        }
        let dup = verts
            .iter()
            .any(|s| (s.v - v).abs().max_element() < VERTEX_DEDUP);
        if !dup {
            verts.push(SolidVertex { v });
        }
    }
    Some(())
}

/// [`feasible_vertices`], but deduped by [`flush_solid_batch_linear_reference`]
/// instead of `VertexAccumulator`. Otherwise byte-for-byte the same
/// function (same plane augmentation, same batching loop).
fn feasible_vertices_linear_reference(planes: &[(DVec3, f64)]) -> Option<Vec<DVec3>> {
    let mut all: Vec<(DVec3, f64)> = planes.to_vec();
    for n in [
        DVec3::X,
        DVec3::NEG_X,
        DVec3::Y,
        DVec3::NEG_Y,
        DVec3::Z,
        DVec3::NEG_Z,
    ] {
        all.push((n, BLANK_HALF_EXTENT));
    }

    let mut soa = crate::simd::PlanesSoA64::with_capacity(all.len());
    for &(n, m) in &all {
        soa.push(n, m, 0);
    }

    let p = all.len();
    let mut verts: Vec<SolidVertex> = Vec::new();
    let mut batch = crate::simd::TripleBatch::default();
    for a in 0..p {
        for b in (a + 1)..p {
            for c in (b + 1)..p {
                let (pa, pb, pc) = (all[a], all[b], all[c]);
                if batch.push((pa.0, pa.1), (pb.0, pb.1), (pc.0, pc.1)) {
                    flush_solid_batch_linear_reference(&batch, &soa, &mut verts)?;
                    batch = crate::simd::TripleBatch::default();
                }
            }
        }
    }
    if batch.len > 0 {
        flush_solid_batch_linear_reference(&batch, &soa, &mut verts)?;
    }
    Some(verts.into_iter().map(|s| s.v).collect())
}

/// Runs both the production (`VertexAccumulator`-indexed) and reference
/// (linear-scan) dedup over `planes` and asserts byte-identical vertex
/// lists, in order.
fn assert_dedup_matches_reference(planes: &[(DVec3, f64)], label: &str) {
    let deduped = dedup_planes(planes);
    let fast = feasible_vertices(&deduped).map(|v| v.into_iter().map(|s| s.v).collect::<Vec<_>>());
    let reference = feasible_vertices_linear_reference(&deduped);

    match (fast, reference) {
        (None, None) => {}
        (Some(f), Some(r)) => {
            assert_eq!(
                f.len(),
                r.len(),
                "{label}: vertex count differs (indexed {} vs linear-scan reference {})",
                f.len(),
                r.len()
            );
            for (i, (fv, rv)) in f.iter().zip(r.iter()).enumerate() {
                assert_eq!(
                    (fv.x.to_bits(), fv.y.to_bits(), fv.z.to_bits()),
                    (rv.x.to_bits(), rv.y.to_bits(), rv.z.to_bits()),
                    "{label}: vertex {i} differs (indexed {fv:?} vs reference {rv:?})"
                );
            }
        }
        (f, r) => panic!(
            "{label}: indexed and reference dedup disagree on boundedness (indexed {:?}, reference {:?})",
            f.is_some(),
            r.is_some()
        ),
    }
}

/// Builds real cutting instructions' plane arrangement (tier normals via
/// `meet_solver::tier_instance_normals`, offsets from `solve_meet_points`'s
/// solved masts) the same way `SolveContext::config_score` does when the
/// solver scores a candidate configuration against a design's printed
/// proportions -- this is the actual production caller of `measure_solid`
/// that makes `feasible_vertices` dedup real numbers of colliding
/// candidate vertices, not just the module's small hand-built fixtures.
fn planes_from_asc_schedule(text: &str) -> Vec<(DVec3, f64)> {
    let schedule = indicatrix_formats::asc::parse_asc(text).expect("fixture schedule parses");
    let mut tiers = crate::geometry::meet_solver::meet_tier_inputs_from_asc(&schedule);
    for j in [0usize, 1, 2] {
        if let Some(t) = schedule.tiers.get(j) {
            tiers[j].constraint =
                crate::geometry::meet_solver::MeetConstraint::ScaleReference(t.mast);
        }
    }
    let normals =
        crate::geometry::meet_solver::tier_instance_normals(schedule.gear_teeth_abs(), &tiers);
    let solved = crate::geometry::meet_solver::solve_meet_points(schedule.gear_teeth_abs(), &tiers);
    normals
        .iter()
        .zip(solved.iter().map(|s| s.mast))
        .flat_map(|(ns, m)| ns.iter().map(move |&n| (n, m)))
        .collect()
}

// -----------------------------------------------------------------------
// `build_solid_mesh`: mesh extraction from the plane arrangement
// -----------------------------------------------------------------------

/// Signed volume via the standard triangle-mesh divergence theorem (`V =
/// (1/6) * sum over triangles of v0 . (v1 x v2)`), valid when every
/// triangle winds counterclockwise as seen from outside the solid --
/// exactly what a centroid fan over `face_ring`'s angle-sorted ring
/// produces (the ring itself is already wound that way; see
/// `build_solid_mesh`'s doc comment on triangulation and this module's
/// own reasoning about `basis_u x basis_v = normal`). Used only by tests,
/// as an independent cross-check against [`measure_solid`]'s
/// plane-offset-and-area formula.
fn mesh_divergence_volume(mesh: &SolidMesh) -> f64 {
    let mut acc = 0.0f64;
    for tri in mesh.indices.as_chunks::<3>().0 {
        let v0 = mesh.positions[tri[0] as usize];
        let v1 = mesh.positions[tri[1] as usize];
        let v2 = mesh.positions[tri[2] as usize];
        acc += v0.dot(v1.cross(v2));
    }
    acc / 6.0
}

/// Every undirected edge of a triangle mesh must be shared by exactly two
/// triangles for the mesh to be watertight (closed, manifold).
///
/// Compares edges by vertex *position*, not by index: `build_solid_mesh`
/// deliberately duplicates each vertex once per owning facet so every
/// copy can carry that facet's own flat normal (see `SolidMesh::normals`
/// doc comment), so the two triangles on either side of a real edge
/// almost always reference two DIFFERENT index pairs that happen to sit
/// at the same position (one pair from each facet's own vertex block) --
/// only a spoke edge internal to one facet's centroid fan reuses the
/// same indices on both its triangles. Position comparison treats both
/// cases uniformly.
///
/// `O(n^2)` plain-loop matching -- meshes in this module's tests are a
/// few dozen triangles, so this trades asymptotic efficiency for staying
/// in the same plain-loops-no-hashing style as the production code it's
/// checking.
fn assert_watertight(mesh: &SolidMesh, label: &str) {
    type PosKey = (u64, u64, u64);
    let key = |i: u32| -> PosKey {
        let p = mesh.positions[i as usize];
        (p.x.to_bits(), p.y.to_bits(), p.z.to_bits())
    };
    let mut edges: Vec<(PosKey, PosKey)> = Vec::new();
    for tri in mesh.indices.as_chunks::<3>().0 {
        for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            let (ka, kb) = (key(a), key(b));
            edges.push(if ka <= kb { (ka, kb) } else { (kb, ka) });
        }
    }
    for e in &edges {
        let count = edges.iter().filter(|other| *other == e).count();
        assert_eq!(
            count, 2,
            "{label}: edge {e:?} is shared by {count} triangles, not 2 (not watertight)"
        );
    }
}

/// Builds the plane-arrangement vertex list the same way `measure_solid`
/// does internally (`dedup_planes` then `feasible_vertices`), for tests
/// that need to call the private `face_area` directly as an oracle.
fn verts_for(planes: &[(DVec3, f64)]) -> Vec<SolidVertex> {
    feasible_vertices(&dedup_planes(planes)).expect("fixture must be bounded")
}

/// A `Closed` mesh's per-face triangle-area sum must equal
/// [`face_area`]'s figure for that plane, and the mesh's own
/// divergence-theorem volume must equal [`measure_solid`]'s -- both
/// within a tight tolerance (not bit-exact: the mesh sums triangle areas
/// in a different order than `face_area`'s single shoelace pass, so
/// floating-point round-off can differ in the last few bits even though
/// both compute the same polygon's area).
fn assert_mesh_matches_measure_solid(planes: &[(DVec3, f64)], label: &str) {
    let status = build_solid_mesh(planes);
    let SolidStatus::Closed(mesh) = status else {
        panic!("{label}: expected a closed mesh, got {status:?}");
    };
    assert_watertight(&mesh, label);

    let verts = verts_for(planes);
    for &(facet_idx, ref ring) in &mesh.rings {
        let (normal, offset) = planes[facet_idx];
        let want_area = face_area(normal, offset, &verts);
        let centroid = ring.iter().copied().sum::<DVec3>() / ring.len() as f64;
        let mut got_area = 0.0f64;
        for i in 0..ring.len() {
            let a = ring[i] - centroid;
            let b = ring[(i + 1) % ring.len()] - centroid;
            got_area = 0.5f64.mul_add(a.cross(b).length(), got_area);
        }
        assert!(
            (got_area - want_area).abs() < 1e-9,
            "{label}: facet {facet_idx} triangle-area sum {got_area} != face_area {want_area}"
        );
    }

    let want_volume = measure_solid(planes).expect("must measure").volume;
    let got_volume = mesh_divergence_volume(&mesh);
    assert!(
        (got_volume - want_volume).abs() < 1e-6,
        "{label}: mesh divergence volume {got_volume} != measure_solid volume {want_volume}"
    );
}

// -----------------------------------------------------------------------
// Duplicate planes and the pair prune
// -----------------------------------------------------------------------

/// Half-space form of a reference cut's planes, as `measure_solid` takes them.
fn halfspaces(planes: Vec<GpuFacetPlane>) -> Vec<(DVec3, f64)> {
    planes
        .into_iter()
        .map(GpuFacetPlane::to_halfspace_f64)
        .collect()
}

/// Repeating any one plane of the Standard Round Brilliant must not change the
/// measured volume: the copy is bit-identical, so whatever the `f32`
/// normalisation did to its length, it has to be recognised and dropped rather
/// than counted as a second face.
#[test]
fn a_duplicated_plane_leaves_the_round_brilliant_volume_unchanged() {
    let base = halfspaces(StandardGemCuts::standard_round_brilliant());
    let want = measure_solid(&base)
        .expect("the brilliant must measure")
        .volume;
    for (i, &plane) in base.iter().enumerate() {
        let mut doubled = base.clone();
        doubled.push(plane);
        let got = measure_solid(&doubled)
            .unwrap_or_else(|| panic!("plane {i} duplicated: must still measure"))
            .volume;
        assert!(
            (got - want).abs() <= 1e-12 * want.abs(),
            "plane {i} duplicated: volume {got} differs from {want}"
        );
    }
}

/// One visited triple: plane indices, determinant bits and vertex bits.
type TripleRecord = ([usize; 3], u64, [u64; 3]);

/// Runs the arrangement walk and returns whether it completed plus every
/// triple it visited, in visiting order, compared by bit pattern.
fn walk_triples(
    planes: &[(DVec3, f64)],
    det_floor: f64,
    prune_pairs: bool,
) -> (bool, Vec<TripleRecord>) {
    let mut visited = Vec::new();
    let completed = for_each_feasible_triple(planes, det_floor, prune_pairs, |t, det, v| {
        visited.push((
            t,
            det.to_bits(),
            [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()],
        ));
    })
    .is_some();
    (completed, visited)
}

/// The pair prune only skips work: with and without it the walk must visit the
/// same triples, with the same determinants and vertices, in the same order.
#[test]
fn pair_prune_visits_the_same_triples_as_the_exhaustive_walk() {
    for (label, planes) in [
        (
            "round brilliant",
            StandardGemCuts::standard_round_brilliant(),
        ),
        ("emerald", StandardGemCuts::emerald_cut()),
    ] {
        let planes = halfspaces(planes);
        for det_floor in [MIN_TRIPLE_DET, 1e-8] {
            let exhaustive = walk_triples(&planes, det_floor, false);
            let pruned = walk_triples(&planes, det_floor, true);
            assert!(exhaustive.0, "{label}: the cut must be bounded");
            assert!(
                !exhaustive.1.is_empty(),
                "{label}: the walk must find vertices"
            );
            assert_eq!(
                exhaustive, pruned,
                "{label}, det floor {det_floor}: pruned walk diverged from the exhaustive one"
            );
        }
    }
}
