use super::*;

const TOLERANCE: f64 = 2e-6;

fn cube(half_height: f64) -> (Vec<Plane>, SolidMesh) {
    let planes = vec![
        (DVec3::X, 1.0),
        (-DVec3::X, 1.0),
        (DVec3::Y, half_height),
        (-DVec3::Y, half_height),
        (DVec3::Z, 1.0),
        (-DVec3::Z, 1.0),
    ];
    let points = [
        DVec3::new(1.0, half_height, 1.0),
        DVec3::new(1.0, half_height, -1.0),
        DVec3::new(1.0, -half_height, -1.0),
        DVec3::new(1.0, -half_height, 1.0),
        DVec3::new(-1.0, half_height, 1.0),
        DVec3::new(-1.0, half_height, -1.0),
        DVec3::new(-1.0, -half_height, -1.0),
        DVec3::new(-1.0, -half_height, 1.0),
    ];
    let rings = [
        [0, 1, 2, 3],
        [4, 7, 6, 5],
        [0, 4, 5, 1],
        [3, 2, 6, 7],
        [0, 3, 7, 4],
        [1, 5, 6, 2],
    ];
    let mesh = SolidMesh {
        rings: rings
            .into_iter()
            .enumerate()
            .map(|(face, ring)| (face, ring.into_iter().map(|i| points[i]).collect()))
            .collect(),
        ..SolidMesh::default()
    };
    (planes, mesh)
}

fn topology(mesh: &SolidMesh, planes: &[Plane]) -> (Vec<Junction>, Vec<(usize, Vec<usize>)>) {
    let nodes = junctions(mesh, planes, TOLERANCE, &SolveControl::default()).unwrap();
    let faces = cycles(mesh, planes, planes, &nodes, 2.0, TOLERANCE).unwrap();
    (nodes, faces)
}

#[test]
fn repeated_samples_of_one_supported_junction_do_not_create_edges() {
    let (planes, mesh) = cube(1.0);
    let expected = topology(&mesh, &planes);
    let mut repeated = mesh;
    let corner = repeated.rings[0].1[0];
    repeated.rings[0]
        .1
        .insert(1, corner - DVec3::Y * (TOLERANCE * 0.25));
    repeated.rings[0].1.push(corner);
    assert_eq!(topology(&repeated, &planes), expected);
}

#[test]
fn distinct_short_edges_keep_their_endpoints() {
    let (planes, mesh) = cube(8.0 * TOLERANCE);
    let (nodes, faces) = topology(&mesh, &planes);
    assert_eq!(nodes.len(), 8);
    assert_eq!(faces.len(), 6);
    assert!(faces.iter().all(|(_, cycle)| cycle.len() == 4));
    assert!(faces.iter().any(|(_, cycle)| cycle.windows(2).any(|edge| {
        let length = nodes[edge[0]].point.distance(nodes[edge[1]].point);
        length > TOLERANCE && length < 20.0 * TOLERANCE
    })));
}

#[test]
fn nearby_different_support_sets_are_not_silently_merged() {
    let (mut planes, mut mesh) = cube(1.0);
    planes.push((DVec3::X, TOLERANCE.mul_add(0.9, 1.0)));
    let corner = mesh.rings[0].1[0];
    mesh.rings[0]
        .1
        .insert(1, corner - DVec3::X * (TOLERANCE * 0.2));
    let error = junctions(&mesh, &planes, TOLERANCE, &SolveControl::default()).unwrap_err();
    assert!(
        matches!(error, PreservationViolation::InvalidBaseline(ref reason)
        if reason == "ambiguous nearby junctions")
    );
}

#[test]
fn a_nonconsecutive_repeated_junction_is_not_a_face_boundary() {
    let (planes, mut mesh) = cube(1.0);
    let corner = mesh.rings[0].1[0];
    mesh.rings[0].1.insert(2, corner);
    let nodes = junctions(&mesh, &planes, TOLERANCE, &SolveControl::default()).unwrap();
    assert!(cycles(&mesh, &planes, &planes, &nodes, 2.0, TOLERANCE).is_err());
}

#[test]
fn infeasible_measurement_samples_are_filtered_in_both_nodes_and_rings() {
    let (planes, mesh) = cube(1.0);
    let expected = topology(&mesh, &planes);
    let mut noisy = mesh;
    let corner = noisy.rings[0].1[0];
    noisy.rings[0]
        .1
        .insert(1, corner + DVec3::X * (TOLERANCE * 2.0));
    assert_eq!(topology(&noisy, &planes), expected);
}

#[test]
fn an_inactive_measurement_ring_is_not_captured_as_a_live_facet() {
    let (mut planes, mut mesh) = cube(1.0);
    let expected = topology(&mesh, &planes);
    planes.push((DVec3::Y, TOLERANCE.mul_add(3.0, 1.0)));
    mesh.rings.push((6, mesh.rings[2].1.clone()));
    assert_eq!(topology(&mesh, &planes), expected);
}

// --- junctions from the f64 arrangement, not from mesh samples ---

/// The identification radius relative to the reproduction bar, as in production
/// (`5e-5` against `2e-6` of the stone).
const IDENTIFY_RADIUS: f64 = 25.0 * TOLERANCE;

fn arrangement(planes: &[Plane], role: Role) -> Result<Vec<Junction>, PreservationViolation> {
    arrangement_junctions(planes, 2.0, IDENTIFY_RADIUS, role, &SolveControl::default())
}

/// The cube with its corner (1, 1, 1) cut off by a plane that passes `clearance`
/// beyond the three neighbouring corners, so the neighbours survive and sit
/// `clearance` inside the new, live face.
fn chamfered(clearance: f64) -> Vec<Plane> {
    let (mut planes, _) = cube(1.0);
    let normal = DVec3::new(1.0, 1.0, 1.0).normalize();
    planes.push((normal, normal.dot(DVec3::new(-1.0, 1.0, 1.0)) + clearance));
    planes
}

#[test]
fn the_arrangement_finds_every_corner_and_face_of_a_cube() {
    let (planes, _) = cube(1.0);
    let nodes = arrangement(&planes, Role::Baseline).unwrap();
    assert_eq!(nodes.len(), 8);
    assert!(nodes.iter().all(|node| node.support.len() == 3));
    let faces = face_cycles(&planes, &nodes, 2.0, TOLERANCE, Role::Baseline).unwrap();
    assert_eq!(faces.len(), 6);
    assert!(
        faces
            .iter()
            .all(|(surface, cycle)| cycle.len() == 4 && supports_face(&nodes, cycle, *surface))
    );
}

#[test]
fn the_normalisation_extent_does_not_change_the_junctions() {
    // A rough much larger than the stone only widens the walk's feasibility slack.
    let (planes, _) = cube(1.0);
    let small = arrangement_junctions(
        &planes,
        2.0,
        TOLERANCE,
        Role::Baseline,
        &SolveControl::default(),
    )
    .unwrap();
    let large = arrangement_junctions(
        &planes,
        4096.0,
        TOLERANCE,
        Role::Baseline,
        &SolveControl::default(),
    )
    .unwrap();
    assert_eq!(small.len(), large.len());
    assert!(
        small
            .iter()
            .zip(&large)
            .all(|(a, b)| a.support == b.support && a.point.distance(b.point) <= TOLERANCE)
    );
}

#[test]
fn a_plane_through_a_corner_within_tolerance_joins_its_support() {
    let (mut planes, _) = cube(1.0);
    let corner = DVec3::ONE;
    let normal = corner.normalize();
    // Tangent at the corner and half a tolerance outside it: a four-plane meet that
    // rounding has opened up by less than the incidence tolerance.
    planes.push((normal, TOLERANCE.mul_add(0.5, normal.dot(corner))));
    let nodes = arrangement(&planes, Role::Baseline).unwrap();
    assert_eq!(nodes.len(), 8);
    assert_eq!(
        nodes.iter().filter(|node| node.support.len() == 4).count(),
        1
    );
}

#[test]
fn a_live_facet_within_the_identification_radius_joins_the_support_with_its_slack() {
    // The chamfer misses the three corners next to the cut one by `clearance`; its own
    // vertices sit `clearance * sqrt 3` from them along the cube's edges. Within the
    // radius each corner and its chamfer vertex are one four-plane meet, whose slack is
    // the miss at the cluster's position (the corner's or the vertex's, whichever the
    // walk finds first).
    for clearance in [1.5 * TOLERANCE, 10.0 * TOLERANCE] {
        for role in [Role::Baseline, Role::Candidate] {
            let nodes = arrangement(&chamfered(clearance), role).unwrap();
            assert_eq!(nodes.len(), 7, "clearance {clearance}: {nodes:?}");
            let met: Vec<_> = nodes.iter().filter(|n| n.support.len() == 4).collect();
            assert_eq!(met.len(), 3, "clearance {clearance}: {nodes:?}");
            for node in met {
                let ulp = 4.0 * f64::EPSILON;
                let along_edge = clearance * 3f64.sqrt();
                assert!(
                    (node.slack - clearance).abs() <= ulp || (node.slack - along_edge).abs() <= ulp,
                    "clearance {clearance}: slack {}",
                    node.slack
                );
            }
            assert!(
                nodes
                    .iter()
                    .filter(|n| n.support.len() == 3)
                    .all(|n| n.slack <= 4.0 * f64::EPSILON)
            );
        }
    }
}

#[test]
fn a_live_facet_clear_of_every_junction_is_captured() {
    let nodes = arrangement(&chamfered(100.0 * TOLERANCE), Role::Baseline).unwrap();
    assert_eq!(nodes.len(), 10);
}

#[test]
fn the_triple_points_of_one_meet_ten_tolerances_apart_are_one_junction() {
    // A chamfer cutting the corner (1, 1, 1) so slightly that its three vertices are ten
    // tolerances apart: an imported meet opened up by quantisation, not a facet.
    let (mut planes, _) = cube(1.0);
    let normal = DVec3::ONE.normalize();
    let cut = 10.0 * TOLERANCE / 6f64.sqrt();
    planes.push((normal, normal.dot(DVec3::ONE) - cut));
    let split = arrangement_junctions(
        &planes,
        2.0,
        TOLERANCE,
        Role::Baseline,
        &SolveControl::default(),
    )
    .unwrap();
    assert_eq!(split.len(), 10, "at the reproduction bar: {split:?}");
    let nodes = arrangement(&planes, Role::Baseline).unwrap();
    assert_eq!(nodes.len(), 8, "{nodes:?}");
    let meet: Vec<_> = nodes.iter().filter(|n| n.support.len() == 4).collect();
    assert_eq!(meet.len(), 1, "{nodes:?}");
    assert!(meet[0].point.distance(DVec3::ONE) <= IDENTIFY_RADIUS);
    assert!(meet[0].slack > TOLERANCE && meet[0].slack <= IDENTIFY_RADIUS);
}

#[test]
fn an_inactive_plane_just_outside_a_face_is_not_a_near_miss() {
    let (mut planes, _) = cube(1.0);
    planes.push((DVec3::Y, TOLERANCE.mul_add(1.5, 1.0)));
    assert_eq!(arrangement(&planes, Role::Baseline).unwrap().len(), 8);
}

#[test]
fn candidate_topology_failures_do_not_blame_the_baseline() {
    // Three planes of a cube leave the stone open: nothing encloses it.
    let (planes, _) = cube(1.0);
    let error = arrangement(&planes[..3], Role::Candidate).unwrap_err();
    assert!(
        matches!(error, PreservationViolation::CandidateTopology(_)),
        "{error}"
    );
    let error = arrangement(&planes[..3], Role::Baseline).unwrap_err();
    assert!(
        matches!(error, PreservationViolation::InvalidBaseline(_)),
        "{error}"
    );
}

fn junction_at(point: DVec3, support: &[usize]) -> Junction {
    Junction {
        point,
        support: support.to_vec(),
        basis: [0, 1, 2],
        slack: 0.0,
    }
}

#[test]
fn a_junction_whose_support_another_carries_in_full_is_dropped() {
    let meet = DVec3::new(0.5, 0.25, -0.5);
    let scattered = junction_at(meet + DVec3::Y * (3.0 * TOLERANCE), &[0, 1, 2]);
    let kept = drop_covered(vec![
        junction_at(meet, &[0, 1, 2, 3, 4, 5]),
        scattered,
        // The same meet clustered a second time: equal supports keep the first.
        junction_at(meet + DVec3::X * (1.5 * TOLERANCE), &[0, 1, 2, 3, 4, 5]),
        // Overlapping, neither carries the other: both stay.
        junction_at(DVec3::ZERO, &[0, 1, 6]),
        junction_at(DVec3::X, &[5, 6, 7]),
    ]);
    let supports: Vec<_> = kept
        .iter()
        .map(|junction| junction.support.clone())
        .collect();
    assert_eq!(
        supports,
        [vec![0, 1, 2, 3, 4, 5], vec![0, 1, 6], vec![5, 6, 7]]
    );
    assert_eq!(kept[0].point, meet);
}

#[test]
fn no_junction_of_an_arrangement_is_covered_by_another() {
    // Every arrangement that capture or validation resolves is a canonical set: an
    // original junction can then find exactly one candidate point carrying its support.
    let (cube_planes, _) = cube(1.0);
    let mut tangent = cube_planes.clone();
    let corner_normal = DVec3::ONE.normalize();
    tangent.push((
        corner_normal,
        TOLERANCE.mul_add(0.5, corner_normal.dot(DVec3::ONE)),
    ));
    // Three more planes through the corner (1, 1, 1): a six-plane meet.
    let mut six = cube_planes.clone();
    for normal in [
        DVec3::new(1.0, 1.0, 0.0),
        DVec3::new(0.0, 1.0, 1.0),
        DVec3::new(1.0, 0.0, 1.0),
    ] {
        let normal = normal.normalize();
        six.push((normal, normal.dot(DVec3::ONE)));
    }
    for planes in [cube_planes, tangent, six, chamfered(10.0 * TOLERANCE)] {
        for role in [Role::Baseline, Role::Candidate] {
            let nodes = arrangement(&planes, role).unwrap();
            for (at, node) in nodes.iter().enumerate() {
                assert!(
                    nodes
                        .iter()
                        .enumerate()
                        .all(|(other_at, other)| other_at == at
                            || !is_subset(&node.support, &other.support)),
                    "junction {at} is covered by another: {nodes:?}"
                );
            }
        }
    }
}

#[test]
fn a_six_plane_meet_is_one_junction() {
    let (mut planes, _) = cube(1.0);
    for normal in [
        DVec3::new(1.0, 1.0, 0.0),
        DVec3::new(0.0, 1.0, 1.0),
        DVec3::new(1.0, 0.0, 1.0),
    ] {
        let normal = normal.normalize();
        planes.push((normal, normal.dot(DVec3::ONE)));
    }
    let nodes = arrangement(&planes, Role::Baseline).unwrap();
    assert_eq!(nodes.len(), 8);
    assert_eq!(
        nodes.iter().filter(|node| node.support.len() == 6).count(),
        1,
        "{nodes:?}"
    );
}

#[test]
fn the_arrangement_honours_cancellation() {
    let (planes, _) = cube(1.0);
    let stop = || true;
    let error = arrangement_junctions(
        &planes,
        2.0,
        TOLERANCE,
        Role::Baseline,
        &SolveControl::with_stop(&stop),
    )
    .unwrap_err();
    assert!(matches!(error, PreservationViolation::Cancelled));
}
