use glam::DVec3;

use super::rows::{ClipRegion, PartialSolver, caliper_extents};
use crate::rough_plan::{
    CandidateDesign,
    lp::{ScaleRow, max_scale},
    piece::{ASSIGNMENTS, Norm, stone_scale},
    tests::Lcg,
};

fn brute_force_search(
    b_min: [f64; 3],
    b_max: [f64; 3],
    s: [f64; 3],
    d: [f64; 3],
    planes: &[(DVec3, f64)],
) -> f64 {
    let grid_steps: u32 = 15;
    let f_grid = f64::from(grid_steps);
    let mut brute_max_k = 0.0f64;

    for ix in 0..=grid_steps {
        let fx = f64::from(ix) / f_grid;
        let tx = fx.mul_add(s[0], b_min[0]);
        for iy in 0..=grid_steps {
            let fy = f64::from(iy) / f_grid;
            let ty = fy.mul_add(s[1], b_min[1]);
            for iz in 0..=grid_steps {
                let fz = f64::from(iz) / f_grid;
                let tz = fz.mul_add(s[2], b_min[2]);
                let t = DVec3::new(tx, ty, tz);

                let mut k_at_t = f64::INFINITY;
                k_at_t = k_at_t.min((b_max[0] - tx) / (d[0] * 0.5));
                k_at_t = k_at_t.min((tx - b_min[0]) / (d[0] * 0.5));
                k_at_t = k_at_t.min((b_max[1] - ty) / (d[1] * 0.5));
                k_at_t = k_at_t.min((ty - b_min[1]) / (d[1] * 0.5));
                k_at_t = k_at_t.min((b_max[2] - tz) / (d[2] * 0.5));
                k_at_t = k_at_t.min((tz - b_min[2]) / (d[2] * 0.5));

                for &(n, m) in planes {
                    let support = n.x.abs().mul_add(
                        d[0] * 0.5,
                        n.y.abs().mul_add(d[1] * 0.5, n.z.abs() * d[2] * 0.5),
                    );
                    let margin = m - n.dot(t);
                    k_at_t = k_at_t.min(margin / support);
                }

                if k_at_t > brute_max_k {
                    brute_max_k = k_at_t;
                }
            }
        }
    }
    brute_max_k
}

fn check_partial_piece_case(rng: &mut Lcg, case_idx: usize) {
    let ox = rng.range(0.0, 10.0);
    let oy = rng.range(0.0, 10.0);
    let oz = rng.range(0.0, 10.0);
    let sx = rng.range(4.0, 12.0);
    let sy = rng.range(4.0, 12.0);
    let sz = rng.range(4.0, 12.0);

    let b_min = [ox, oy, oz];
    let b_max = [ox + sx, oy + sy, oz + sz];

    let d = [
        rng.range(1.0, 3.0),
        rng.range(1.0, 3.0),
        rng.range(1.0, 3.0),
    ];

    let plane_count = if rng.next_u64().is_multiple_of(2) {
        1
    } else {
        2
    };
    let mut planes = Vec::with_capacity(plane_count);

    for _ in 0..plane_count {
        let mut nx = rng.range(-1.0, 1.0);
        let mut ny = rng.range(-1.0, 1.0);
        let mut nz = rng.range(-1.0, 1.0);
        let len = nx.mul_add(nx, ny.mul_add(ny, nz * nz)).sqrt();
        if len < 1e-4 {
            nx = 1.0;
            ny = 0.0;
            nz = 0.0;
        } else {
            nx /= len;
            ny /= len;
            nz /= len;
        }
        let n = DVec3::new(nx, ny, nz);

        let p_center = DVec3::new(
            f64::midpoint(b_min[0], b_max[0]),
            f64::midpoint(b_min[1], b_max[1]),
            f64::midpoint(b_min[2], b_max[2]),
        );
        let m = n.dot(p_center) + rng.range(-0.25 * sx, 0.25 * sx);
        planes.push((n, m));
    }

    let violated: Vec<usize> = (0..planes.len()).collect();
    let region = ClipRegion {
        min: b_min,
        max: b_max,
        planes: &planes,
        violated: &violated,
    };
    let lp_result = PartialSolver::new().solve(&region, d);
    let brute_max_k = brute_force_search(b_min, b_max, [sx, sy, sz], d, &planes);

    if let Some((k_lp, t_lp)) = lp_result {
        // The stone box lies in the piece box and behind every plane. Checked
        // from the geometry, not from the solver's own rows.
        for i in 0..3 {
            let half = k_lp * d[i] * 0.5;
            assert!(
                t_lp[i] - half >= b_min[i] - 1e-7 && t_lp[i] + half <= b_max[i] + 1e-7,
                "case {case_idx}: LP stone leaves the piece box on axis {i}"
            );
        }
        for &(n, m) in &planes {
            let support = n.x.abs().mul_add(
                d[0] * 0.5,
                n.y.abs().mul_add(d[1] * 0.5, n.z.abs() * d[2] * 0.5),
            );
            let lhs = k_lp.mul_add(support, n.dot(DVec3::from(t_lp)));
            assert!(
                lhs <= m + 1e-7,
                "case {case_idx}: LP returned infeasible point: {lhs} > {m}"
            );
        }

        assert!(
            k_lp >= brute_max_k - 1e-4,
            "case {case_idx}: LP {k_lp} worse than brute force {brute_max_k}"
        );
    } else {
        assert!(
            brute_max_k <= 0.0,
            "case {case_idx}: LP found infeasible but brute force found k={brute_max_k}"
        );
    }
}

#[test]
fn partial_piece_lp_matches_brute_force_grid_search_for_30_random_pieces() {
    let mut rng = Lcg(888_123);
    for case_idx in 0..30 {
        check_partial_piece_case(&mut rng, case_idx);
    }
}

#[test]
fn box_only_lp_scale_matches_the_interior_scale_for_every_assignment() {
    // With no plane in the way the LP must agree with `stone_scale`, which the
    // interior pieces use: the caliper extents have to follow the assignment
    // (design dimension j runs along rough axis ASSIGNMENTS[orient][j]), also
    // for the two assignments that are not their own inverse.
    let design = CandidateDesign {
        entry_id: 1,
        width: 1.0,
        length: 1.5,
        height: 0.6,
        volume: 0.5,
    };
    let norm = Norm::of(&design);
    let (b_min, b_max) = ([1.0, 2.0, 3.0], [8.0, 5.0, 8.0]);
    let region = ClipRegion {
        min: b_min,
        max: b_max,
        planes: &[],
        violated: &[],
    };
    let mut solver = PartialSolver::new();
    for orient in 0..ASSIGNMENTS.len() {
        let extents = caliper_extents(&norm, orient);
        let (k, t) = solver.solve(&region, extents).expect("a box is feasible");
        let expected = stone_scale(&norm, orient, [7.0, 3.0, 5.0]);
        assert!(
            (k - expected).abs() < 1e-9,
            "assignment {orient}: LP scale {k} != box scale {expected}"
        );
        for i in 0..3 {
            let half = k * extents[i] * 0.5;
            assert!(
                t[i] - half >= b_min[i] - 1e-9 && t[i] + half <= b_max[i] + 1e-9,
                "assignment {orient}: stone leaves the box on axis {i}"
            );
        }
    }
}

/// Rows of a unit cube stone (half extent `0.5` per axis at unit scale) in the corner
/// tetrahedron `x, y, z >= 0`, `x + y + z <= 12`, the slanted face given as the unit
/// normal `(1, 1, 1) / sqrt(3)`.
fn tetrahedron_rows() -> [ScaleRow; 4] {
    let inv_sqrt3 = 1.0 / 3.0_f64.sqrt();
    let wall = |axis: usize| {
        let mut normal = [0.0; 3];
        normal[axis] = -1.0;
        ScaleRow {
            normal,
            support: 0.5,
            offset: 0.0,
        }
    };
    [
        wall(0),
        wall(1),
        wall(2),
        ScaleRow {
            normal: [inv_sqrt3; 3],
            support: 1.5 * inv_sqrt3,
            offset: 12.0 * inv_sqrt3,
        },
    ]
}

#[test]
fn the_cube_in_a_corner_tetrahedron_has_the_hand_derived_scale_and_centre() {
    // The walls need t_i >= k / 2 on every axis. The slanted face needs
    // t_x + t_y + t_z + 1.5 k <= 12, and the three walls force t_x + t_y + t_z >= 1.5 k,
    // so 3 k <= 12: k = 4 is the most, reached only with every t_i = k / 2 = 2.
    let (k, t) = max_scale(&tetrahedron_rows()).expect("the tetrahedron holds a cube");
    assert!((k - 4.0).abs() < 1e-9, "scale {k}");
    for (axis, &c) in t.iter().enumerate() {
        assert!((c - 2.0).abs() < 1e-9, "centre {axis} is {c}");
    }
}

#[test]
fn the_lp_reports_no_solution_for_malformed_or_unbounded_rows() {
    let row = |normal: [f64; 3], support: f64, offset: f64| ScaleRow {
        normal,
        support,
        offset,
    };
    // Nothing constrains the stone.
    assert_eq!(max_scale(&[]), None);
    // A NaN or infinite offset is not a plane.
    let walls = tetrahedron_rows();
    for bad in [f64::NAN, f64::INFINITY] {
        let mut rows = walls;
        rows[3].offset = bad;
        assert_eq!(max_scale(&rows), None, "offset {bad}");
    }
    // A negative support would grow the stone into the wall.
    let mut rows = walls;
    rows[0].support = -0.5;
    assert_eq!(max_scale(&rows), None);
    // An unbounded region: the only constraint is the upper face x <= 10, so the stone
    // can be scaled without limit by moving far to the left.
    let open = [row([1.0, 0.0, 0.0], 0.5, 10.0)];
    assert_eq!(max_scale(&open), None);
}
