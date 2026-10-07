use super::{
    build::weld, geometry::area_vector, intersect::first_self_intersections, repair,
    triangulate::triangulate, *,
};
use crate::rough_plan::shape::mesh_fixture::{C_SHAPE_OBJ, CUBE_OBJ, icosphere, parse_obj};

fn mesh_of(text: &str) -> RoughMesh {
    let (points, tris) = parse_obj(text);
    RoughMesh::new(&points, &tris).expect("a closed mesh")
}

/// Whether `p` is in the C-shaped fixture: the 20 mm cube minus the notch
/// `x > 10, 5 < y < 15`.
fn in_c_shape(p: DVec3) -> bool {
    (0.0..=20.0).contains(&p.x)
        && (0.0..=20.0).contains(&p.y)
        && (0.0..=20.0).contains(&p.z)
        && !(p.x > 10.0 && p.y > 5.0 && p.y < 15.0)
}

/// The points and triangles of the cube `[lo, hi]` on every axis, wound outward, or inward
/// when `inward`.
fn cube_shell(lo: f64, hi: f64, inward: bool) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let (mut points, mut tris) = parse_obj(CUBE_OBJ);
    for p in &mut points {
        *p = *p * ((hi - lo) / 20.0) + DVec3::splat(lo);
    }
    if inward {
        for tri in &mut tris {
            tri.swap(1, 2);
        }
    }
    (points, tris)
}

/// One mesh of several cube shells, each `(lo, hi, wound inward)`, in the order given.
fn shells(parts: &[(f64, f64, bool)]) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let (mut points, mut tris) = (Vec::new(), Vec::new());
    for &(lo, hi, inward) in parts {
        let (shell_points, shell_tris) = cube_shell(lo, hi, inward);
        let base = points.len() as u32;
        points.extend(shell_points);
        tris.extend(shell_tris.into_iter().map(|tri| tri.map(|v| v + base)));
    }
    (points, tris)
}

#[test]
fn the_fixtures_have_their_hand_volumes() {
    // 20 x 20 x 20 minus the 10 x 10 x 20 notch.
    assert!((mesh_of(C_SHAPE_OBJ).volume() - 6000.0).abs() < 1e-9);
    assert!((mesh_of(CUBE_OBJ).volume() - 8000.0).abs() < 1e-9);
}

#[test]
fn a_flipped_mesh_is_turned_outward() {
    let (points, mut tris) = parse_obj(C_SHAPE_OBJ);
    for tri in &mut tris {
        tri.swap(1, 2);
    }
    let mesh = RoughMesh::new(&points, &tris).expect("flipped meshes are accepted");
    assert!((mesh.volume() - 6000.0).abs() < 1e-9);
    assert!(mesh.contains_point(DVec3::new(2.0, 2.0, 2.0)));
}

#[test]
fn an_open_mesh_is_rejected() {
    // A hole as big as a face of the cube is far over the fill limit.
    let (points, mut tris) = parse_obj(C_SHAPE_OBJ);
    tris.pop();
    assert_eq!(RoughMesh::new(&points, &tris), Err(MeshError::Open));
}

#[test]
fn a_non_orientable_surface_stays_inconsistent() {
    // A Moebius band: four quads round a ring, bottom 0..4 and top 4..8, the last one
    // joining bottom 3 to top 0 and top 3 to bottom 0.
    let mut tris: Vec<[u32; 3]> = vec![
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 4, 0],
        [3, 0, 7],
    ];
    assert_eq!(repair::wind_consistently(&mut tris), None);
}

#[test]
fn an_edge_of_three_faces_is_rejected() {
    let (points, mut tris) = parse_obj(CUBE_OBJ);
    // A fin on the edge of vertices 0 and 1.
    let mut points = points;
    points.push(DVec3::new(-5.0, -5.0, -5.0));
    tris.push([0, 1, (points.len() - 1) as u32]);
    assert_eq!(RoughMesh::new(&points, &tris), Err(MeshError::NonManifold));
}

#[test]
fn bad_input_is_named() {
    let (points, tris) = parse_obj(CUBE_OBJ);
    assert_eq!(RoughMesh::new(&points, &[]), Err(MeshError::NoFaces));
    let mut bad = tris;
    bad[2][1] = 99;
    assert_eq!(RoughMesh::new(&points, &bad), Err(MeshError::BadFace(2)));
    let flat = [points[0], points[1], points[2]];
    assert_eq!(RoughMesh::new(&flat, &[[0, 1, 2]]), Err(MeshError::Open));
    let line = [DVec3::ZERO, DVec3::X, DVec3::X * 2.0];
    assert_eq!(
        RoughMesh::new(&line, &[[0, 1, 2]]),
        Err(MeshError::Degenerate)
    );
    let many = vec![[0, 1, 2]; MAX_MESH_TRIANGLES + 1];
    assert_eq!(
        RoughMesh::new(&points, &many),
        Err(MeshError::TooManyTriangles(MAX_MESH_TRIANGLES + 1))
    );
}

#[test]
fn duplicate_vertices_are_welded() {
    // Every triangle with its own copies of its corners.
    let (points, tris) = parse_obj(C_SHAPE_OBJ);
    let split: Vec<DVec3> = tris
        .iter()
        .flat_map(|tri| tri.map(|v| points[v as usize]))
        .collect();
    let split_tris: Vec<[u32; 3]> = (0..tris.len() as u32)
        .map(|i| [3 * i, 3 * i + 1, 3 * i + 2])
        .collect();
    let mesh = RoughMesh::new(&split, &split_tris).expect("welds shut");
    assert_eq!(mesh.vertices().len(), 16);
    assert!((mesh.volume() - 6000.0).abs() < 1e-9);
}

#[test]
fn copies_of_a_vertex_on_both_sides_of_a_weld_cell_boundary_still_weld() {
    // The 20 mm cube welds within 2e-6 mm. Every triangle gets its own copies of its
    // corners, and the copies of the even and the odd triangles differ by 2e-9 mm in x,
    // placed so that the pair straddles a cell boundary of a rounded grid (half a cell
    // past a vertex plane) or of a floored one (on the plane itself).
    let (points, tris) = parse_obj(CUBE_OBJ);
    let quantum = WELD_FRACTION * 20.0;
    for offset in [0.0, 0.25 * quantum, 0.5 * quantum] {
        let mut split = Vec::new();
        for (i, tri) in tris.iter().enumerate() {
            let dx = if i % 2 == 0 { -1e-9 } else { 1e-9 };
            for &v in tri {
                let p = points[v as usize];
                split.push(DVec3::new(p.x + offset + dx, p.y, p.z));
            }
        }
        let split_tris: Vec<[u32; 3]> = (0..tris.len() as u32)
            .map(|i| [3 * i, 3 * i + 1, 3 * i + 2])
            .collect();
        let mesh = RoughMesh::new(&split, &split_tris)
            .unwrap_or_else(|err| panic!("offset {offset}: {err}"));
        assert_eq!(mesh.vertices().len(), 8, "offset {offset}");
        assert!((mesh.volume() - 8000.0).abs() < 1e-2, "{}", mesh.volume());
    }
}

#[test]
fn welding_joins_what_is_close_and_keeps_what_is_not() {
    let scale = 20.0;
    let quantum = WELD_FRACTION * scale;
    let points = [
        DVec3::ZERO,
        // Close to the first on x, across a cell boundary, and on y below zero.
        DVec3::new(0.4 * quantum, 0.0, 0.0),
        DVec3::new(10.0 * quantum, 0.0, 0.0),
        DVec3::new(0.0, -0.4 * quantum, 0.0),
    ];
    let (verts, welded) = weld(&points, &[[0, 1, 2], [3, 2, 1]], scale);
    assert_eq!(verts, [DVec3::ZERO, points[2]]);
    assert_eq!(welded, [[0, 0, 1], [0, 1, 0]]);
}

#[test]
fn a_cube_with_a_cubic_cavity_has_one_volume_whichever_way_the_cavity_is_wound() {
    let centre = DVec3::splat(10.0);
    for inward in [true, false] {
        let (points, tris) = shells(&[(0.0, 20.0, false), (5.0, 15.0, inward)]);
        let mesh = RoughMesh::new(&points, &tris).expect("two closed shells");
        assert!(
            (mesh.volume() - 7000.0).abs() < 1e-9,
            "cavity wound inward {inward}: {}",
            mesh.volume()
        );
        assert!(mesh.contains_point(DVec3::splat(2.0)));
        assert!(!mesh.contains_point(centre), "the cavity is air");
        // The planes keep the material on the inner side: the outer faces point out of the
        // cube, the cavity faces point into the void.
        for (t, tri) in mesh.triangles().iter().enumerate() {
            let in_cavity = tri.iter().all(|&v| {
                let p = mesh.vertices()[v as usize];
                p.min_element() >= 5.0 - 1e-9 && p.max_element() <= 15.0 + 1e-9
            });
            let (n, d) = mesh.triangle_plane(t as u32);
            assert_eq!(n.dot(centre) > d, in_cavity, "triangle {t}");
        }
        // A cut through the cavity takes the half of both: 10 x 20 x 20 less 5 x 10 x 10.
        assert!((mesh.volume_within(&[(DVec3::X, 10.0)]) - 3500.0).abs() < 1e-6);
    }
}

#[test]
fn a_single_shell_is_turned_outward_as_before() {
    // A lone closed shell keeps its winding when it is outward, and is turned when not.
    for inward in [false, true] {
        let (points, tris) = shells(&[(0.0, 20.0, inward)]);
        let mesh = RoughMesh::new(&points, &tris).expect("closed");
        assert!((mesh.volume() - 8000.0).abs() < 1e-9, "inward {inward}");
        for t in 0..mesh.triangles().len() as u32 {
            let (n, d) = mesh.triangle_plane(t);
            assert!(
                n.dot(DVec3::splat(10.0)) < d,
                "inward {inward}, triangle {t}"
            );
        }
    }
}

#[test]
fn nested_shells_are_turned_by_their_depth_whatever_the_file_says() {
    // A 20 mm body, a 12 mm cavity in it and a 4 mm island in the cavity: the bodies less
    // the cavity, 8000 - 1728 + 64 mm^3 (the island is 12 - 8 = 4 mm across, not 8).
    let nest = [(0.0, 20.0), (4.0, 16.0), (8.0, 12.0)];
    let cube = |(lo, hi): (f64, f64)| (hi - lo).powi(3);
    let expected = cube(nest[0]) - cube(nest[1]) + cube(nest[2]);
    assert!((expected - 6336.0).abs() < 1e-9, "{expected}");
    for winding in 0..8_u32 {
        for reversed in [false, true] {
            let mut parts: Vec<(f64, f64, bool)> = nest
                .iter()
                .enumerate()
                .map(|(i, &(lo, hi))| (lo, hi, (winding & (1 << i)) != 0))
                .collect();
            if reversed {
                parts.reverse();
            }
            let (points, tris) = shells(&parts);
            let mesh = RoughMesh::new(&points, &tris).expect("closed shells");
            let what = format!("winding {winding:03b}, reversed {reversed}");
            assert!(
                (mesh.volume() - expected).abs() < 1e-9,
                "{what}: volume {} against {expected}",
                mesh.volume()
            );
            assert!(mesh.contains_point(DVec3::splat(2.0)), "{what}");
            assert!(!mesh.contains_point(DVec3::splat(6.0)), "{what}: cavity");
            assert!(mesh.contains_point(DVec3::splat(10.0)), "{what}: island");
        }
    }
}

#[test]
fn two_separate_bodies_add_whichever_way_each_is_wound() {
    let mut parts = [(0.0, 10.0, false), (20.0, 30.0, false)];
    for first in [false, true] {
        for second in [false, true] {
            parts[0].2 = first;
            parts[1].2 = second;
            let (points, tris) = shells(&parts);
            let mesh = RoughMesh::new(&points, &tris).expect("closed shells");
            assert!((mesh.volume() - 2000.0).abs() < 1e-9, "{first} {second}");
        }
    }
}

#[test]
fn two_bodies_that_touch_corner_to_corner_still_add() {
    // The cubes share the welded vertex (10, 10, 10), which is the second cube's first
    // vertex, and no edge, so they are two shells.
    for first in [false, true] {
        for second in [false, true] {
            let (points, tris) = shells(&[(0.0, 10.0, first), (10.0, 20.0, second)]);
            let mesh = RoughMesh::new(&points, &tris).expect("two closed shells");
            assert_eq!(mesh.vertices().len(), 15, "the corner is welded");
            assert!((mesh.volume() - 2000.0).abs() < 1e-9, "{first} {second}");
        }
    }
}

/// The points and triangles of an octahedron of radius `r` about `centre`, wound outward
/// (inward when `inward`), whose first vertex is its top apex.
fn octahedron(centre: DVec3, r: f64, inward: bool) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let points = [
        DVec3::new(0.0, 0.0, r),
        DVec3::new(r, 0.0, 0.0),
        DVec3::new(0.0, r, 0.0),
        DVec3::new(-r, 0.0, 0.0),
        DVec3::new(0.0, -r, 0.0),
        DVec3::new(0.0, 0.0, -r),
    ]
    .map(|p| p + centre);
    let mut tris = [
        [0, 1, 2],
        [0, 2, 3],
        [0, 3, 4],
        [0, 4, 1],
        [5, 2, 1],
        [5, 3, 2],
        [5, 4, 3],
        [5, 1, 4],
    ];
    if inward {
        for tri in &mut tris {
            tri.swap(1, 2);
        }
    }
    (points.to_vec(), tris.to_vec())
}

#[test]
fn a_body_whose_apex_rests_on_another_shell_is_still_a_body() {
    // A 20 mm cube and, under it, an octahedron (a body of 288 mm^3) whose top apex rests
    // on the cube's bottom face, off its diagonal. The apex is the octahedron's first vertex
    // and every fixed ray runs up from it into the cube, so a probe at that vertex reads the
    // octahedron as lying inside the cube: a cavity, whose volume would be subtracted. The
    // probe is a point inside a face of the shell instead, and the octahedron stays a body
    // whichever way the file winds it and whichever shell comes first.
    let centre = DVec3::new(12.0, 6.0, -6.0);
    for inward in [false, true] {
        for cube_first in [true, false] {
            let parts = [
                cube_shell(0.0, 20.0, false),
                octahedron(centre, 6.0, inward),
            ];
            let order = if cube_first { [0, 1] } else { [1, 0] };
            let mut points: Vec<DVec3> = Vec::new();
            let mut tris: Vec<[u32; 3]> = Vec::new();
            for part in order {
                let (shell_points, shell_tris) = &parts[part];
                let base = points.len() as u32;
                points.extend(shell_points);
                tris.extend(shell_tris.iter().map(|tri| tri.map(|v| v + base)));
            }
            let mesh = RoughMesh::new(&points, &tris).expect("two closed shells");
            let what = format!("octahedron wound inward {inward}, cube first {cube_first}");
            assert!(
                (mesh.volume() - 8288.0).abs() < 1e-9,
                "{what}: volume {}",
                mesh.volume()
            );
            assert!(mesh.contains_point(DVec3::splat(2.0)), "{what}: cube");
            assert!(mesh.contains_point(centre), "{what}: octahedron");
            assert!(!mesh.contains_point(DVec3::new(18.0, 18.0, -6.0)), "{what}");
            // Every plane faces out of the body it belongs to.
            for (t, tri) in mesh.triangles().iter().enumerate() {
                let in_octahedron = tri.iter().any(|&v| mesh.vertices()[v as usize].z < -1.0);
                let inside = if in_octahedron {
                    centre
                } else {
                    DVec3::splat(10.0)
                };
                let (n, d) = mesh.triangle_plane(t as u32);
                assert!(n.dot(inside) < d, "{what}: triangle {t}");
            }
        }
    }
}

#[test]
fn points_inside_and_outside_are_told_apart_even_on_a_lattice_of_edges() {
    // Coordinates on the planes of the notch walls and the faces, so axis-aligned
    // rays would run along edges and through vertices; the fixed irrational rays
    // must not care.
    let mesh = mesh_of(C_SHAPE_OBJ);
    let at = [2.5, 5.0, 7.5, 10.0, 12.5, 15.0, 17.5];
    let mut checked = 0;
    for &x in &at {
        for &y in &at {
            for &z in &at {
                let p = DVec3::new(x, y, z);
                // Points on a wall may go either way.
                let on_wall = x == 10.0 || y == 5.0 || y == 15.0;
                if on_wall {
                    continue;
                }
                assert_eq!(mesh.contains_point(p), in_c_shape(p), "{p}");
                checked += 1;
            }
        }
    }
    assert!(checked > 100);
    assert!(!mesh.contains_point(DVec3::new(-1.0, 10.0, 10.0)));
    assert!(!mesh.contains_point(DVec3::new(15.0, 10.0, 25.0)));
}

#[test]
fn box_states_follow_the_surface() {
    let mesh = mesh_of(C_SHAPE_OBJ);
    let state = |min: [f64; 3], max: [f64; 3], inset: f64| mesh.box_state(min, max, inset);
    // Well inside the left arm, the lower arm and the back.
    assert_eq!(state([1.0; 3], [4.0; 3], 0.0), BoxState::Clear);
    assert_eq!(
        state([12.0, 0.5, 1.0], [18.0, 4.0, 19.0], 0.0),
        BoxState::Clear
    );
    // Wholly in the notch.
    assert_eq!(
        state([12.0, 7.0, 1.0], [18.0, 13.0, 19.0], 0.0),
        BoxState::Air
    );
    // Straddling the notch wall at x = 10.
    assert_eq!(
        state([8.0, 6.0, 1.0], [12.0, 9.0, 4.0], 0.0),
        BoxState::Crossing
    );
    // Resting on a wall is clear without a clearance and crossing with one.
    assert_eq!(
        state([2.0, 2.0, 2.0], [10.0, 5.0, 8.0], 0.0),
        BoxState::Clear
    );
    assert_eq!(
        state([2.0, 2.0, 2.0], [10.0, 5.0, 8.0], 0.1),
        BoxState::Crossing
    );
    // A box that flush to the outer face with no clearance is clear.
    assert_eq!(state([0.0; 3], [4.0; 3], 0.0), BoxState::Clear);
}

#[test]
fn polytope_in_the_notch_is_blocked_and_a_stone_in_the_arm_is_not() {
    let mesh = mesh_of(C_SHAPE_OBJ);
    // The cube 12..18 x 7..13 x 1..19 as planes: in the notch.
    let cube = |lo: [f64; 3], hi: [f64; 3]| -> Vec<(DVec3, f64)> {
        let mut planes = Vec::new();
        for axis in 0..3 {
            let mut n = DVec3::ZERO;
            n[axis] = 1.0;
            planes.push((n, hi[axis]));
            planes.push((-n, -lo[axis]));
        }
        planes
    };
    let mut blockers = Vec::new();
    // Entirely in air: nothing of the surface is inside it, the centre is outside.
    mesh.polytope_blockers(
        &cube([12.0, 7.0, 1.0], [18.0, 13.0, 19.0]),
        0.0,
        &mut blockers,
    );
    assert_eq!(blockers, [] as [u32; 0]);
    assert!(!mesh.contains_point(DVec3::new(15.0, 10.0, 10.0)));
    // Reaching into the wall at x = 10 (two triangles of the wall and the notch faces).
    mesh.polytope_blockers(&cube([8.0, 6.0, 1.0], [12.0, 9.0, 4.0]), 0.0, &mut blockers);
    assert_ne!(blockers, [] as [u32; 0]);
    // Inside the lower arm: clear, and with a clearance as well.
    mesh.polytope_blockers(
        &cube([11.0, 1.0, 1.0], [19.0, 4.0, 19.0]),
        0.5,
        &mut blockers,
    );
    assert_eq!(blockers, [] as [u32; 0]);
    // The clearance alone can block: 0.5 from the notch floor at y = 5 with a 1 mm margin.
    mesh.polytope_blockers(
        &cube([11.0, 1.0, 1.0], [19.0, 4.5, 19.0]),
        1.0,
        &mut blockers,
    );
    assert_ne!(blockers, [] as [u32; 0]);
    // The row to add is the one the stone passes least far: here only the notch floor
    // (y = 5, outward normal +y) blocks; a plane already known is never chosen again.
    let reach = |n: DVec3, d: f64| n.dot(DVec3::new(15.0, 4.5, 10.0)) - d;
    let (n, d) = mesh
        .least_violated_blocker(&blockers, &[], reach)
        .expect("a blocker");
    assert!((n - DVec3::Y).length() < 1e-12 && (d - 5.0).abs() < 1e-12);
    assert_eq!(
        mesh.least_violated_blocker(&blockers, &[(n, d)], reach),
        None
    );
}

// ---- blocker rows ----

/// The rows `blocker_rows` returns for the stone box `[min, max]` (kept `inset` clear) of
/// `mesh`, with the stone centred at `centre`; the blockers are returned alongside.
fn rows_for_box(
    mesh: &RoughMesh,
    min: [f64; 3],
    max: [f64; 3],
    inset: f64,
    centre: DVec3,
) -> (Vec<u32>, Vec<(DVec3, f64)>) {
    let mut blockers = Vec::new();
    mesh.box_blockers(min, max, inset, &mut blockers);
    let half = (DVec3::from(max) - DVec3::from(min)) * 0.5;
    let rows = mesh.blocker_rows(
        &blockers,
        &[],
        |n, d| n.dot(centre) <= d - inset,
        |n, d| n.dot(centre) + n.abs().dot(half) - (d - inset),
    );
    (blockers, rows)
}

#[test]
fn blocker_rows_add_the_wall_the_centre_is_on_the_material_side_of() {
    let mesh = mesh_of(C_SHAPE_OBJ);
    // A stone reaching from the lower arm into the notch meets only the notch floor y = 5.
    let (min, max) = ([12.0, 2.0, 5.0], [18.0, 8.0, 15.0]);
    let (blockers, rows) = rows_for_box(&mesh, min, max, 0.2, DVec3::new(15.0, 3.0, 10.0));
    assert_ne!(blockers, Vec::<u32>::new());
    for &t in &blockers {
        let (n, d) = mesh.planes[t as usize];
        assert!((n - DVec3::Y).length() < 1e-12 && (d - 5.0).abs() < 1e-12);
    }
    assert_ne!(rows, Vec::<(DVec3, f64)>::new());
    for &(n, _) in &rows {
        assert!((n - DVec3::Y).length() < 1e-12, "only +y walls: {n:?}");
    }
}

#[test]
fn blocker_rows_never_add_facing_walls_for_a_centre_in_the_notch() {
    let mesh = mesh_of(C_SHAPE_OBJ);
    // A stone centred in the notch with a 3.5 mm clearance meets all three walls.
    let (min, max) = ([12.0, 8.0, 5.0], [18.0, 12.0, 15.0]);
    let centre = DVec3::new(15.0, 10.0, 10.0);
    let (blockers, rows) = rows_for_box(&mesh, min, max, 3.5, centre);
    let walls: Vec<DVec3> = blockers
        .iter()
        .map(|&t| mesh.planes[t as usize].0)
        .collect();
    for wall in [DVec3::X, DVec3::Y, -DVec3::Y] {
        assert!(
            walls.iter().any(|n| (*n - wall).length() < 1e-12),
            "{wall:?}"
        );
    }
    // The centre is in the air of the notch: none of the three notch walls is
    // centre-satisfied, so only the least-violated wall (and its own kin) is returned for
    // them. The inflated box also reaches the outer face x = 20 (to x = 21.5), which the
    // centre is on the material side of: that one is legitimately a centre-satisfied row.
    for &t in &blockers {
        let (n, d) = mesh.planes[t as usize];
        if n.dot(centre) <= d - 3.5 {
            assert!(
                (n - DVec3::X).length() < 1e-12 && (d - 20.0).abs() < 1e-12,
                "a notch wall is centre-satisfied: {n:?} {d}"
            );
        }
    }
    assert_ne!(rows, Vec::<(DVec3, f64)>::new());
    for (i, a) in rows.iter().enumerate() {
        for b in &rows[i + 1..] {
            assert!(a.0.dot(b.0) >= -0.5, "facing walls added together");
        }
    }
}

#[test]
fn blocker_rows_clear_every_corner_contact_of_a_cube_in_a_sphere() {
    let (points, tris) = icosphere(3, 10.0);
    let mesh = RoughMesh::new(&points, &tris).expect("a closed mesh");
    // A 12 mm cube pokes out of the 10 mm sphere at its eight corners.
    let (blockers, rows) = rows_for_box(&mesh, [-6.0; 3], [6.0; 3], 0.0, DVec3::ZERO);
    assert_ne!(blockers, Vec::<u32>::new());
    assert!(rows.len() >= 8 && rows.len() <= 64, "{} rows", rows.len());
    for sx in [-1.0, 1.0] {
        for sy in [-1.0, 1.0] {
            for sz in [-1.0, 1.0] {
                assert!(
                    rows.iter()
                        .any(|&(n, _)| n.x * sx > 0.0 && n.y * sy > 0.0 && n.z * sz > 0.0),
                    "no row for the octant {sx} {sy} {sz}"
                );
            }
        }
    }
}

#[test]
fn volume_within_cuts_matches_hand_value() {
    let mesh = mesh_of(C_SHAPE_OBJ);
    assert!((mesh.volume_within(&[]) - 6000.0).abs() < 1e-9);
    let plane = |n: DVec3, m: f64| (n, m);
    let close = |a: f64, b: f64| (a - b).abs() < 1e-6 * b.max(1.0);

    // x <= 5: the left slab, 5 x 20 x 20, no notch in it.
    assert!(close(mesh.volume_within(&[plane(DVec3::X, 5.0)]), 2000.0));
    // x <= 15: the full left half (10 x 20 x 20) plus the two arms' 5 mm: 2 x (5 x 5 x 20).
    let expect = 5000.0;
    assert!(close(mesh.volume_within(&[plane(DVec3::X, 15.0)]), expect));
    // A cut through the notch's floor plane y <= 5: 20 x 5 x 20.
    assert!(close(mesh.volume_within(&[plane(DVec3::Y, 5.0)]), 2000.0));
    // z <= 10: half of everything.
    assert!(close(mesh.volume_within(&[plane(DVec3::Z, 10.0)]), 3000.0));
    // Along the notch wall x <= 10 (coincident with a face), and beyond everything.
    assert!(close(mesh.volume_within(&[plane(DVec3::X, 10.0)]), 4000.0));
    assert!(close(mesh.volume_within(&[plane(DVec3::X, 25.0)]), 6000.0));
    // Two cuts: x <= 15 and y <= 10 keep the lower half's arm: x <= 10 part is
    // 10 x 10 x 20 = 2000, and x in 10..15 with y <= 5 is 5 x 5 x 20 = 500.
    let two = [plane(DVec3::X, 15.0), plane(DVec3::Y, 10.0)];
    assert!(close(mesh.volume_within(&two), 2500.0));
    // A slanted cut x + y <= 10: the triangle below it, 50 x 20 (the notch starts at
    // x > 10 so it does not interfere).
    let slant = DVec3::new(1.0, 1.0, 0.0) / 2.0_f64.sqrt();
    assert!(close(
        mesh.volume_within(&[plane(slant, 10.0 / 2.0_f64.sqrt())]),
        1000.0
    ));
    // A cut that leaves nothing.
    assert_eq!(mesh.volume_within(&[plane(DVec3::X, -1.0)]), 0.0);
}

#[test]
fn the_clipped_surface_encloses_the_same_volume() {
    let mesh = mesh_of(C_SHAPE_OBJ);
    let cuts = [
        (DVec3::X, 15.0),
        (
            DVec3::new(0.0, 1.0, 1.0) / 2.0_f64.sqrt(),
            20.0 / 2.0_f64.sqrt(),
        ),
    ];
    let surface = mesh.clipped_surface(&cuts);
    assert_eq!(surface.caps.len(), 2);
    let tetra = |tri: &[DVec3; 3]| tri[0].dot(tri[1].cross(tri[2])) / 6.0;
    let from_surface: f64 = surface
        .triangles
        .iter()
        .chain(surface.caps.iter().flat_map(|cap| &cap.triangles))
        .map(tetra)
        .sum();
    let exact = mesh.volume_within(&cuts);
    assert!(
        (from_surface - exact).abs() < 1e-6 * exact,
        "{from_surface} vs {exact}"
    );
    // With no cuts the surface is the mesh.
    let whole = mesh.clipped_surface(&[]);
    assert_eq!(whole.triangles.len(), mesh.triangles().len());
    assert_eq!(whole.caps, [] as [SurfaceCap; 0]);
}

#[test]
fn pruned_volumes_are_bit_identical_to_the_full_scan() {
    use crate::rough_plan::shape::mesh_fixture::noisy_c_shape;
    let (points, tris) = noisy_c_shape(3, 0.2);
    let mesh = RoughMesh::new(&points, &tris).expect("a closed mesh");
    let slant = DVec3::new(1.0, 2.0, 0.5).normalize();
    let boxed = |lo: [f64; 3], hi: [f64; 3]| {
        let mut cuts = Vec::new();
        for axis in 0..3 {
            let unit = [DVec3::X, DVec3::Y, DVec3::Z][axis];
            cuts.push((unit, hi[axis]));
            cuts.push((-unit, -lo[axis]));
        }
        cuts
    };
    let cases = [
        boxed([0.0; 3], [20.0; 3]),
        boxed([2.0, 3.0, 1.0], [9.0, 14.0, 17.0]),
        boxed([8.0, 4.0, 0.0], [18.0, 16.0, 20.0]),
        boxed([12.0, 6.0, 1.0], [16.0, 10.0, 4.0]),
        boxed([-5.0, -5.0, -5.0], [30.0, 30.0, 30.0]),
        vec![(DVec3::X, 15.0), (slant, 14.0)],
        vec![(DVec3::X, 10.0)],
        vec![(DVec3::new(0.0, 1.0, 0.0), 5.0)],
    ];
    for cuts in &cases {
        let pruned = mesh.volume_within_pruned(cuts, true);
        let full = mesh.volume_within_pruned(cuts, false);
        assert_eq!(
            pruned.to_bits(),
            full.to_bits(),
            "{cuts:?}: {pruned} {full}"
        );
    }
}

#[test]
fn a_large_cap_outline_is_triangulated_quickly() {
    // A 3000-pointed star: half its vertices are reflex, so a cubic ear clip would
    // never finish.
    let count = 3000;
    let ring: Vec<DVec3> = (0..count)
        .map(|i| {
            let angle = std::f64::consts::TAU * f64::from(i) / f64::from(count);
            let radius = if i % 2 == 0 { 10.0 } else { 8.0 };
            DVec3::new(radius * angle.cos(), radius * angle.sin(), 0.0)
        })
        .collect();
    let triangles = triangulate(std::slice::from_ref(&ring), DVec3::Z, 0.0);
    assert_eq!(triangles.len(), count as usize - 2);
    let area: f64 = triangles
        .iter()
        .map(|t| (t[1] - t[0]).cross(t[2] - t[0]).z * 0.5)
        .sum();
    let outline = DVec3::Z.dot(area_vector(&ring));
    assert!(
        (area - outline).abs() < 1e-6 * outline,
        "{area} vs {outline}"
    );
}

#[test]
fn moving_a_mesh_that_collapses_gives_none() {
    let mesh = mesh_of(C_SHAPE_OBJ);
    assert!(mesh.translated(DVec3::splat(f64::NAN)).is_none());
    assert!(mesh.scaled(0.0).is_none() && mesh.scaled(f64::NAN).is_none());
    let moved = mesh.translated(DVec3::new(1.0, 2.0, 3.0)).expect("moves");
    assert!((moved.volume() - 6000.0).abs() < 1e-9);
    let doubled = mesh.scaled(2.0).expect("scales");
    assert!((doubled.volume() - 48000.0).abs() < 1e-6);
}

// ---- repair ----

#[test]
fn a_closed_mesh_is_not_repaired_and_has_no_note() {
    assert_eq!(mesh_of(C_SHAPE_OBJ).repair_note(), None);
    assert_eq!(mesh_of(CUBE_OBJ).repair_note(), None);
}

#[test]
fn one_face_turned_the_wrong_way_is_turned_back() {
    let (points, mut tris) = parse_obj(C_SHAPE_OBJ);
    tris[3].swap(1, 2);
    let mesh = RoughMesh::new(&points, &tris).expect("the winding is repaired");
    assert!((mesh.volume() - 6000.0).abs() < 1e-9);
    let note = mesh.repair_note().expect("a note");
    assert!(note.contains("turned 1 face "), "{note}");
    // The repaired mesh is the closed one, bit for bit.
    assert_eq!(mesh.triangles(), mesh_of(C_SHAPE_OBJ).triangles());
}

#[test]
fn a_mostly_flipped_component_is_turned_the_short_way() {
    // All but one face reversed: the repair turns the one, then the shell is turned out.
    let (points, mut tris) = parse_obj(C_SHAPE_OBJ);
    for tri in tris.iter_mut().skip(1) {
        tri.swap(1, 2);
    }
    let mesh = RoughMesh::new(&points, &tris).expect("repaired");
    assert!((mesh.volume() - 6000.0).abs() < 1e-9);
    assert!(
        mesh.repair_note()
            .expect("a note")
            .contains("turned 1 face ")
    );
}

#[test]
fn a_hair_wide_gap_is_welded_shut() {
    // Every triangle with its own corners, the odd ones 1e-5 mm away: further apart than
    // the ordinary weld (2e-6 mm on this mesh), inside the retry (2e-5 mm).
    let (points, tris) = parse_obj(C_SHAPE_OBJ);
    let mut split = Vec::new();
    for (i, tri) in tris.iter().enumerate() {
        let shift = if i % 2 == 1 {
            DVec3::new(1e-5, 0.0, 0.0)
        } else {
            DVec3::ZERO
        };
        split.extend(tri.map(|v| points[v as usize] + shift));
    }
    let split_tris: Vec<[u32; 3]> = (0..tris.len() as u32)
        .map(|i| [3 * i, 3 * i + 1, 3 * i + 2])
        .collect();
    let mesh = RoughMesh::new(&split, &split_tris).expect("the gaps are closed");
    assert!((mesh.volume() - 6000.0).abs() < 1e-2);
    let note = mesh.repair_note().expect("a note");
    assert!(note.contains("joining") && note.contains("mm"), "{note}");
}

#[test]
fn a_gap_wider_than_the_retry_stays_open() {
    let (points, tris) = parse_obj(C_SHAPE_OBJ);
    let mut split = Vec::new();
    for (i, tri) in tris.iter().enumerate() {
        let shift = if i % 2 == 1 {
            DVec3::new(1e-3, 0.0, 0.0)
        } else {
            DVec3::ZERO
        };
        split.extend(tri.map(|v| points[v as usize] + shift));
    }
    let split_tris: Vec<[u32; 3]> = (0..tris.len() as u32)
        .map(|i| [3 * i, 3 * i + 1, 3 * i + 2])
        .collect();
    assert_eq!(RoughMesh::new(&split, &split_tris), Err(MeshError::Open));
}

/// The 10 mm sphere with this many triangles removed from the first one on, each next to the
/// last (a hole of a few small faces). A level-5 icosphere's faces are about 0.33 mm.
fn sphere_with_hole(removed: usize) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let (points, mut tris) = icosphere(5, 10.0);
    let mut gone = tris.remove(0);
    for _ in 1..removed {
        let Some(at) = tris
            .iter()
            .position(|tri| tri.iter().filter(|v| gone.contains(v)).count() == 2)
        else {
            break;
        };
        gone = tris.remove(at);
    }
    (points, tris)
}

#[test]
fn a_small_hole_is_filled_and_the_volume_is_nearly_the_spheres() {
    let sphere = (4.0 / 3.0) * std::f64::consts::PI * 1000.0;
    for removed in [1, 2] {
        let (points, tris) = sphere_with_hole(removed);
        let mesh = RoughMesh::new(&points, &tris).expect("the hole is filled");
        let note = mesh.repair_note().expect("a note");
        assert!(note.contains("filled 1 hole"), "{note}");
        assert!(note.contains("mm across"), "{note}");
        // The icosphere's own facets lose 0.3 % or so of the volume.
        assert!(
            (mesh.volume() - sphere).abs() < 0.01 * sphere,
            "{}",
            mesh.volume()
        );
        assert!(mesh.contains_point(DVec3::ZERO));
        // Determinism: the same repair twice.
        assert_eq!(RoughMesh::new(&points, &tris), Ok(mesh));
    }
}

#[test]
fn a_hole_over_the_limit_is_not_filled() {
    // Fifty small faces missing is far over 5 % of the diagonal.
    let (points, tris) = sphere_with_hole(50);
    assert_eq!(RoughMesh::new(&points, &tris), Err(MeshError::Open));
}

#[test]
fn a_hole_with_a_pinched_boundary_is_not_filled() {
    // Two faces that touch at one vertex only: the boundary visits that vertex twice.
    let (points, mut tris) = icosphere(5, 10.0);
    let first = tris.remove(0);
    let at = tris
        .iter()
        .position(|tri| tri.iter().filter(|v| first.contains(v)).count() == 1)
        .expect("a face touching at a corner");
    tris.remove(at);
    assert_eq!(RoughMesh::new(&points, &tris), Err(MeshError::Open));
}

#[test]
fn the_hole_loops_come_out_in_order_of_their_lowest_vertex() {
    // Three separate single-triangle holes, given the high vertex numbers first.
    let tris = [[5, 6, 7], [8, 9, 10], [1, 2, 3]];
    let loops = repair::boundary_loops(&tris).expect("simple loops");
    let starts: Vec<u32> = loops.iter().map(|ring| ring[0]).collect();
    assert_eq!(starts, [1, 5, 8]);
    assert_eq!(loops[1], [5, 6, 7]);
}

// ---- self-intersection ----

/// A triangle soup of `triangles` (three corners each, no shared vertex numbers).
fn soup(triangles: &[[DVec3; 3]]) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let points: Vec<DVec3> = triangles.iter().flatten().copied().collect();
    let tris = (0..triangles.len() as u32)
        .map(|i| [3 * i, 3 * i + 1, 3 * i + 2])
        .collect();
    (points, tris)
}

#[test]
fn two_triangles_that_pierce_each_other_are_found() {
    let flat = [
        DVec3::new(0.0, 0.0, 0.0),
        DVec3::new(10.0, 0.0, 0.0),
        DVec3::new(0.0, 10.0, 0.0),
    ];
    let spike = [
        DVec3::new(2.0, 2.0, -5.0),
        DVec3::new(6.0, 2.0, 5.0),
        DVec3::new(2.0, 6.0, 5.0),
    ];
    let (points, tris) = soup(&[flat, spike]);
    let found = first_self_intersections(&points, &tris, 10.0, 16);
    assert_eq!(found, [(0, 1)]);
    assert_eq!(first_self_intersections(&points, &tris, 10.0, 16), found);
}

#[test]
fn a_vertex_resting_on_a_face_is_touching_not_crossing() {
    let flat = [
        DVec3::new(0.0, 0.0, 0.0),
        DVec3::new(10.0, 0.0, 0.0),
        DVec3::new(0.0, 10.0, 0.0),
    ];
    let resting = [
        DVec3::new(2.0, 2.0, 0.0),
        DVec3::new(6.0, 2.0, -5.0),
        DVec3::new(2.0, 6.0, -5.0),
    ];
    let (points, tris) = soup(&[flat, resting]);
    assert_eq!(
        first_self_intersections(&points, &tris, 10.0, 16),
        Vec::<(u32, u32)>::new()
    );
}

#[test]
fn triangles_that_share_a_vertex_are_not_compared() {
    let a = [
        DVec3::ZERO,
        DVec3::new(10.0, 0.0, 0.0),
        DVec3::new(0.0, 10.0, 0.0),
    ];
    let points = vec![
        a[0],
        a[1],
        a[2],
        DVec3::new(2.0, 2.0, -5.0),
        DVec3::new(2.0, 6.0, 5.0),
    ];
    // The second triangle uses vertex 1 of the first and pierces it.
    let tris = [[0, 1, 2], [1, 3, 4]];
    assert_eq!(
        first_self_intersections(&points, &tris, 10.0, 16),
        Vec::<(u32, u32)>::new()
    );
}

#[test]
fn coplanar_overlapping_triangles_are_found_and_disjoint_ones_are_not() {
    let a = [
        DVec3::ZERO,
        DVec3::new(10.0, 0.0, 0.0),
        DVec3::new(0.0, 10.0, 0.0),
    ];
    let overlap = [
        DVec3::new(2.0, 2.0, 0.0),
        DVec3::new(12.0, 2.0, 0.0),
        DVec3::new(2.0, 12.0, 0.0),
    ];
    let apart = overlap.map(|p| p + DVec3::new(20.0, 0.0, 0.0));
    let (points, tris) = soup(&[a, overlap]);
    assert_eq!(first_self_intersections(&points, &tris, 10.0, 16), [(0, 1)]);
    let (points, tris) = soup(&[a, apart]);
    assert_eq!(
        first_self_intersections(&points, &tris, 10.0, 16),
        Vec::<(u32, u32)>::new()
    );
    // The same triangle twice, under other vertex numbers.
    let (points, tris) = soup(&[a, a]);
    assert_eq!(first_self_intersections(&points, &tris, 10.0, 16), [(0, 1)]);
}

#[test]
fn the_search_stops_at_the_limit() {
    let flat = [
        DVec3::ZERO,
        DVec3::new(10.0, 0.0, 0.0),
        DVec3::new(0.0, 10.0, 0.0),
    ];
    let spikes: Vec<[DVec3; 3]> = (0..5)
        .map(|k| {
            let shift = DVec3::new(0.0, 0.0, f64::from(k) * 1e-3);
            [
                DVec3::new(2.0, 2.0, -5.0) + shift,
                DVec3::new(6.0, 2.0, 5.0) + shift,
                DVec3::new(2.0, 6.0, 5.0) + shift,
            ]
        })
        .collect();
    let mut all = vec![flat];
    all.extend(spikes);
    let (points, tris) = soup(&all);
    assert_eq!(first_self_intersections(&points, &tris, 10.0, 3).len(), 3);
}

#[test]
fn two_cubes_overlapping_by_half_are_a_self_intersecting_mesh() {
    let (points, tris) = shells(&[(0.0, 10.0, false), (5.0, 15.0, false)]);
    let error = RoughMesh::new(&points, &tris).expect_err("the cubes cross");
    let MeshError::SelfIntersecting { count, first_at } = error else {
        panic!("not a crossing: {error:?}");
    };
    assert!(count >= 1);
    // The first crossing is on a surface of the first cube, inside the second one's box.
    let [x, y, z] = first_at.map(|v| v as f64 / 100.0);
    assert!((0.0..=10.0).contains(&x) && (0.0..=10.0).contains(&y) && (0.0..=10.0).contains(&z));
    let text = error.to_string();
    assert!(
        text.contains("crosses itself") && text.contains("near x"),
        "{text}"
    );
}

#[test]
fn a_cube_with_a_nested_cavity_does_not_cross_itself() {
    let (points, tris) = shells(&[(0.0, 20.0, false), (5.0, 10.0, true)]);
    let mesh = RoughMesh::new(&points, &tris).expect("a hollow cube");
    assert!((mesh.volume() - (8000.0 - 125.0)).abs() < 1e-9);
    assert_eq!(mesh.repair_note(), None);
}

#[test]
fn the_fixtures_and_smooth_scans_do_not_cross_themselves() {
    for text in [C_SHAPE_OBJ, CUBE_OBJ] {
        let (points, tris) = parse_obj(text);
        assert_eq!(
            first_self_intersections(&points, &tris, 20.0, 16),
            Vec::<(u32, u32)>::new()
        );
    }
    let (points, tris) = icosphere(3, 10.0);
    assert_eq!(
        first_self_intersections(&points, &tris, 20.0, 16),
        Vec::<(u32, u32)>::new()
    );
    let (points, tris) = crate::rough_plan::shape::mesh_fixture::noisy_c_shape(2, 0.3);
    assert_eq!(
        first_self_intersections(&points, &tris, 20.0, 16),
        Vec::<(u32, u32)>::new()
    );
}

// ---- inclusions ----

/// The cube `[lo, hi]` on every axis as a solid wound outward.
fn solid(lo: f64, hi: f64) -> RoughMesh {
    let (points, tris) = cube_shell(lo, hi, false);
    RoughMesh::new(&points, &tris).expect("a closed cube")
}

/// The 20 mm cube with the inclusion cube `[lo, hi]` in it.
fn cube_with_inclusion(lo: f64, hi: f64) -> RoughMesh {
    RoughMesh::with_inclusions(&solid(0.0, 20.0), &[solid(lo, hi)]).expect("the inclusion fits")
}

#[test]
fn an_inclusion_leaves_the_usable_volume_and_keeps_the_gross_volume() {
    let mesh = cube_with_inclusion(8.0, 12.0);
    assert_eq!(mesh.inclusion_count(), 1);
    assert!(
        (mesh.volume() - (8000.0 - 64.0)).abs() < 1e-9,
        "{}",
        mesh.volume()
    );
    assert!((mesh.inclusion_volume() - 64.0).abs() < 1e-9);
    assert!(
        (mesh.gross_volume() - 8000.0).abs() < 1e-9,
        "{}",
        mesh.gross_volume()
    );
    // The mesh without inclusions has no gross volume of its own to speak of.
    let plain = solid(0.0, 20.0);
    assert_eq!(plain.gross_volume().to_bits(), plain.volume().to_bits());
    assert_eq!(plain.inclusion_count(), 0);
    assert_eq!(plain.outer_triangles().len(), plain.triangles().len());
    assert_eq!(plain.outer_vertices().len(), plain.vertices().len());
}

#[test]
fn the_outer_part_of_a_mesh_with_inclusions_is_the_mesh_it_was_added_to() {
    let outer = solid(0.0, 20.0);
    let mesh = RoughMesh::with_inclusions(&outer, &[solid(3.0, 6.0), solid(10.0, 14.0)])
        .expect("two inclusions");
    assert_eq!(mesh.inclusion_count(), 2);
    assert_eq!(mesh.outer_vertices(), outer.vertices());
    assert_eq!(mesh.outer_triangles(), outer.triangles());
    assert_eq!(mesh.inclusion_shells(), &[12, 24]);
    assert_eq!(
        mesh.inclusions()[1].vertices(),
        solid(10.0, 14.0).vertices()
    );
}

#[test]
fn stones_stay_out_of_an_inclusion_and_in_the_rest() {
    let mesh = cube_with_inclusion(8.0, 12.0);
    // The inclusion is not material to place into: its centre is air to the mesh, its walls
    // block, and a box beside it is clear.
    assert!(!mesh.contains_point(DVec3::splat(10.0)));
    assert!(mesh.contains_point(DVec3::splat(3.0)));
    assert_eq!(mesh.box_state([6.0; 3], [14.0; 3], 0.0), BoxState::Crossing);
    assert_eq!(mesh.box_state([9.0; 3], [11.0; 3], 0.0), BoxState::Air);
    assert_eq!(mesh.box_state([1.0; 3], [5.0; 3], 0.0), BoxState::Clear);
    // A clearance reaches the inclusion before the box touches it.
    assert_eq!(mesh.box_state([1.0; 3], [7.9; 3], 0.0), BoxState::Clear);
    assert_eq!(mesh.box_state([1.0; 3], [7.9; 3], 0.2), BoxState::Crossing);
    let mut blockers = Vec::new();
    mesh.box_blockers([6.0; 3], [14.0; 3], 0.0, &mut blockers);
    assert_eq!(blockers.len(), 12, "every triangle of the inclusion");
    mesh.box_blockers([1.0; 3], [5.0; 3], 0.0, &mut blockers);
    assert_eq!(blockers, Vec::<u32>::new());
    // The planes of the inclusion face the way a cavity's do: into the void.
    let (n, d) = mesh.triangle_plane(mesh.inclusion_shells()[0]);
    assert!(n.dot(DVec3::splat(10.0)) > d, "{n} {d}");
}

#[test]
fn a_cut_through_an_inclusion_takes_half_of_it_in_both_volumes() {
    let mesh = cube_with_inclusion(8.0, 12.0);
    let cuts = [(DVec3::X, 10.0)];
    assert!((mesh.volume_within(&cuts) - (4000.0 - 32.0)).abs() < 1e-6);
    assert!((mesh.inclusion_volume_within(&cuts) - 32.0).abs() < 1e-6);
    assert!((mesh.gross_volume_within(&cuts) - 4000.0).abs() < 1e-6);
    // No cuts: the whole.
    assert_eq!(
        mesh.gross_volume_within(&[]).to_bits(),
        mesh.gross_volume().to_bits()
    );
}

#[test]
fn an_inclusion_that_reaches_the_surface_is_refused() {
    let outer = solid(0.0, 20.0);
    // Through the +x face.
    assert_eq!(
        RoughMesh::with_inclusions(&outer, &[solid(15.0, 25.0)]),
        Err(MeshError::InclusionReachesSurface(0))
    );
    // The second of two is the one named.
    assert_eq!(
        RoughMesh::with_inclusions(&outer, &[solid(3.0, 5.0), solid(15.0, 25.0)]),
        Err(MeshError::InclusionReachesSurface(1))
    );
    let text = MeshError::InclusionReachesSurface(0).to_string();
    assert!(
        text.contains("must be cut away") && text.contains("notch in the rough's own mesh"),
        "{text}"
    );
}

#[test]
fn an_inclusion_outside_the_rough_or_in_a_hollow_is_refused() {
    let outer = solid(0.0, 20.0);
    assert_eq!(
        RoughMesh::with_inclusions(&outer, &[solid(30.0, 40.0)]),
        Err(MeshError::InclusionOutside(0))
    );
    // In the hollow of a cube with a cavity.
    let (points, tris) = shells(&[(0.0, 20.0, false), (5.0, 15.0, true)]);
    let hollow = RoughMesh::new(&points, &tris).expect("a hollow cube");
    assert_eq!(
        RoughMesh::with_inclusions(&hollow, &[solid(8.0, 12.0)]),
        Err(MeshError::InclusionOutside(0))
    );
    // And inside an inclusion that is already there.
    let with_one = cube_with_inclusion(5.0, 15.0);
    assert_eq!(
        RoughMesh::with_inclusions(&with_one, &[solid(8.0, 12.0)]),
        Err(MeshError::InclusionOutside(0))
    );
}

#[test]
fn inclusions_that_cross_or_nest_in_one_call_are_refused() {
    let outer = solid(0.0, 20.0);
    assert_eq!(
        RoughMesh::with_inclusions(&outer, &[solid(5.0, 10.0), solid(8.0, 14.0)]),
        Err(MeshError::InclusionsOverlap)
    );
    assert_eq!(
        RoughMesh::with_inclusions(&outer, &[solid(5.0, 15.0), solid(8.0, 12.0)]),
        Err(MeshError::InclusionsOverlap)
    );
    // Side by side is fine.
    assert!(RoughMesh::with_inclusions(&outer, &[solid(2.0, 6.0), solid(10.0, 14.0)]).is_ok());
}

#[test]
fn too_many_triangles_with_the_inclusions_are_refused() {
    let outer = solid(0.0, 20.0);
    let (points, tris) = icosphere(4, 3.0);
    let inclusion = RoughMesh::new(
        &points
            .iter()
            .map(|&p| p + DVec3::splat(10.0))
            .collect::<Vec<_>>(),
        &tris,
    )
    .expect("a sphere");
    let bodies = vec![inclusion; MAX_MESH_TRIANGLES / tris.len() + 1];
    assert!(matches!(
        RoughMesh::with_inclusions(&outer, &bodies),
        Err(MeshError::TooManyTriangles(_))
    ));
}

#[test]
fn removing_inclusions_gives_back_exactly_the_earlier_meshes() {
    let outer = solid(0.0, 20.0);
    let one = RoughMesh::with_inclusions(&outer, &[solid(3.0, 6.0)]).expect("one");
    let two = RoughMesh::with_inclusions(&one, &[solid(10.0, 14.0)]).expect("two");
    assert_eq!(two.inclusion_count(), 2);
    assert_eq!(two.without_inclusion(1).as_ref(), Some(&one));
    assert_eq!(one.without_inclusion(0).as_ref(), Some(&outer));
    // Removing the first leaves the second, as if it had been added alone.
    let alone = RoughMesh::with_inclusions(&outer, &[solid(10.0, 14.0)]).expect("alone");
    assert_eq!(two.without_inclusion(0).as_ref(), Some(&alone));
    assert_eq!(two.without_inclusion(2), None);
    assert_eq!(outer.without_inclusion(0), None);
}

#[test]
fn the_inclusions_move_and_scale_with_the_mesh() {
    let mesh = cube_with_inclusion(8.0, 12.0);
    let big = mesh.scaled(2.0).expect("scales");
    assert!((big.gross_volume() - 64_000.0).abs() < 1e-6);
    assert!((big.inclusion_volume() - 512.0).abs() < 1e-6);
    assert_eq!(big.inclusion_count(), 1);
    assert_eq!(big.outer_triangles().len(), 12);
    let moved = mesh.translated(DVec3::new(5.0, -3.0, 2.0)).expect("moves");
    assert!((moved.volume() - mesh.volume()).abs() < 1e-9);
    assert!((moved.gross_volume() - 8000.0).abs() < 1e-9);
    assert!(
        !moved.contains_point(DVec3::new(15.0, 7.0, 12.0)),
        "the inclusion moved too"
    );
    assert!(moved.contains_point(DVec3::new(10.0, 10.0, 10.0)));
}

#[test]
fn a_margin_grows_a_cube_by_the_margin_on_every_face() {
    let body = solid(8.0, 12.0);
    let grown = body.grown(0.3).expect("a grown cube");
    let (lo, hi) = grown.bounds();
    assert!((lo - DVec3::splat(7.7)).abs().max_element() < 1e-8, "{lo}");
    assert!((hi - DVec3::splat(12.3)).abs().max_element() < 1e-8, "{hi}");
    assert!((grown.volume() - 4.6_f64.powi(3)).abs() < 1e-6);
    // No margin, or one that is not a number, changes nothing.
    assert_eq!(body.grown(0.0).as_ref(), Ok(&body));
    assert_eq!(body.grown(f64::NAN).as_ref(), Ok(&body));
    assert_eq!(body.grown(-1.0).as_ref(), Ok(&body));
}

#[test]
fn a_margin_keeps_a_stone_away_from_an_inclusion() {
    let outer = solid(0.0, 20.0);
    let plain = RoughMesh::with_inclusions(&outer, &[solid(8.0, 12.0)]).expect("fits");
    let padded = RoughMesh::with_inclusions(&outer, &[solid(8.0, 12.0).grown(0.3).expect("grows")])
        .expect("fits");
    // A box 0.1 mm from the inclusion's +x face: clear without a margin, in the way with one.
    let (min, max) = ([12.1, 8.0, 8.0], [14.0, 12.0, 12.0]);
    assert_eq!(plain.box_state(min, max, 0.0), BoxState::Clear);
    assert_eq!(padded.box_state(min, max, 0.0), BoxState::Crossing);
    let mut blockers = Vec::new();
    plain.box_blockers(min, max, 0.0, &mut blockers);
    assert_eq!(blockers, Vec::<u32>::new());
    padded.box_blockers(min, max, 0.0, &mut blockers);
    assert_ne!(blockers, Vec::<u32>::new());
    // The gross volume is the outer cube's either way.
    assert!((padded.gross_volume() - 8000.0).abs() < 1e-9);
}

#[test]
fn an_inclusion_whose_margin_reaches_the_surface_is_refused() {
    // The inclusion fits as given; grown by 0.3 mm it pokes through the rough's surface.
    let outer = solid(0.0, 20.0);
    assert!(RoughMesh::with_inclusions(&outer, &[solid(0.1, 5.0)]).is_ok());
    let close = solid(0.1, 5.0).grown(0.3).expect("grows");
    assert_eq!(
        RoughMesh::with_inclusions(&outer, &[close]),
        Err(MeshError::InclusionReachesSurface(0))
    );
}
