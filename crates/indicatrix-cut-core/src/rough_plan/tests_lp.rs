use super::{
    lp::{LpScratch, ScaleRow, max_scale, max_scale_with_scratch, solve},
    piece::{ASSIGNMENTS, Norm, stone_scale},
};

#[test]
fn test_lp_unit_cube_region_unit_cube_stone() {
    // Unit cube [0, 1]^3: six bounding planes
    // Cube stone: centered at origin, width 1.0 (extents [-0.5, 0.5]), support 0.5 along all axes
    let rows = [
        ScaleRow {
            normal: [1.0, 0.0, 0.0],
            support: 0.5,
            offset: 1.0,
        },
        ScaleRow {
            normal: [-1.0, 0.0, 0.0],
            support: 0.5,
            offset: 0.0,
        },
        ScaleRow {
            normal: [0.0, 1.0, 0.0],
            support: 0.5,
            offset: 1.0,
        },
        ScaleRow {
            normal: [0.0, -1.0, 0.0],
            support: 0.5,
            offset: 0.0,
        },
        ScaleRow {
            normal: [0.0, 0.0, 1.0],
            support: 0.5,
            offset: 1.0,
        },
        ScaleRow {
            normal: [0.0, 0.0, -1.0],
            support: 0.5,
            offset: 0.0,
        },
    ];

    let (k, t) = max_scale(&rows).expect("must find feasible scale");
    assert!((k - 1.0).abs() < 1e-12, "expected k = 1.0, got {k}");
    assert!(
        (t[0] - 0.5).abs() < 1e-12,
        "expected t.x = 0.5, got {}",
        t[0]
    );
    assert!(
        (t[1] - 0.5).abs() < 1e-12,
        "expected t.y = 0.5, got {}",
        t[1]
    );
    assert!(
        (t[2] - 0.5).abs() < 1e-12,
        "expected t.z = 0.5, got {}",
        t[2]
    );
}

/// The six box planes of `[inset, extents - inset]` for a stone with the given per-axis
/// supports. With `inset > 0` every low-side offset is negative.
fn box_rows(extents: [f64; 3], supports: [f64; 3], inset: f64) -> [ScaleRow; 6] {
    [
        ScaleRow {
            normal: [1.0, 0.0, 0.0],
            support: supports[0],
            offset: extents[0] - inset,
        },
        ScaleRow {
            normal: [-1.0, 0.0, 0.0],
            support: supports[0],
            offset: -inset,
        },
        ScaleRow {
            normal: [0.0, 1.0, 0.0],
            support: supports[1],
            offset: extents[1] - inset,
        },
        ScaleRow {
            normal: [0.0, -1.0, 0.0],
            support: supports[1],
            offset: -inset,
        },
        ScaleRow {
            normal: [0.0, 0.0, 1.0],
            support: supports[2],
            offset: extents[2] - inset,
        },
        ScaleRow {
            normal: [0.0, 0.0, -1.0],
            support: supports[2],
            offset: -inset,
        },
    ]
}

#[test]
fn test_lp_box_stone_in_box_region_matches_stone_scale() {
    let mut state: u64 = 0x9876_5432_1ABC_DEF0;
    let mut next_f64 = |min: f64, max: f64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let norm = (state >> 11) as f64 / (1u64 << 53) as f64;
        norm.mul_add(max - min, min)
    };

    for _ in 0..20 {
        let extents_p = [
            next_f64(5.0, 50.0),
            next_f64(5.0, 50.0),
            next_f64(5.0, 50.0),
        ];
        let ratio_l = next_f64(1.0, 3.0);
        let ratio_h = next_f64(0.5, 2.0);
        let norm = Norm {
            l: ratio_l,
            h: ratio_h,
            f: 0.5,
        };

        for (orient, assignment) in ASSIGNMENTS.iter().enumerate() {
            let expected_k = stone_scale(&norm, orient, extents_p);
            let mut supports = [0.0; 3];
            supports[assignment[0]] = 0.5 * 1.0;
            supports[assignment[1]] = 0.5 * ratio_l;
            supports[assignment[2]] = 0.5 * ratio_h;

            let rows = box_rows(extents_p, supports, 0.0);
            let (scale_k, _) = max_scale(&rows).expect("must solve box stone in box region");
            assert!(
                (scale_k - expected_k).abs() < 1e-12,
                "orient {orient}: LP scale {scale_k} differs from stone_scale {expected_k}"
            );
        }
    }
}

#[test]
fn test_lp_inset_box_region_with_negative_offsets_matches_stone_scale() {
    // Insetting a box by 0.2 on every side makes each low-side offset negative (an
    // artificial-variable row) and leaves a box 0.4 smaller per axis.
    let inset = 0.2;
    let mut state: u64 = 0x1357_9BDF_2468_ACE0;
    let mut next_f64 = |min: f64, max: f64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let norm = (state >> 11) as f64 / (1u64 << 53) as f64;
        norm.mul_add(max - min, min)
    };

    for _ in 0..20 {
        let extents_p = [
            next_f64(5.0, 50.0),
            next_f64(5.0, 50.0),
            next_f64(5.0, 50.0),
        ];
        let inner = extents_p.map(|e| 2.0f64.mul_add(-inset, e));
        let ratio_l = next_f64(1.0, 3.0);
        let ratio_h = next_f64(0.5, 2.0);
        let norm = Norm {
            l: ratio_l,
            h: ratio_h,
            f: 0.5,
        };

        for (orient, assignment) in ASSIGNMENTS.iter().enumerate() {
            let expected_k = stone_scale(&norm, orient, inner);
            let mut supports = [0.0; 3];
            supports[assignment[0]] = 0.5;
            supports[assignment[1]] = 0.5 * ratio_l;
            supports[assignment[2]] = 0.5 * ratio_h;

            let rows = box_rows(extents_p, supports, inset);
            let (scale_k, t) = max_scale(&rows).expect("must solve inset box region");
            assert!(
                (scale_k - expected_k).abs() < 1e-9,
                "orient {orient}: LP scale {scale_k} differs from stone_scale {expected_k}"
            );
            for (axis, (&centre, &support)) in t.iter().zip(&supports).enumerate() {
                let reach = scale_k * support;
                assert!(
                    centre - reach >= inset - 1e-9
                        && centre + reach <= extents_p[axis] - inset + 1e-9,
                    "orient {orient}: stone leaves the inset box on axis {axis}"
                );
            }
        }
    }
}

/// A `[0.2, 9.8]^3` box (low offsets negative) with the corner cut `x + y + z >= 12`, for a
/// cube stone of half-width `0.5 k`. The stone must sit in the far corner: `t_i + 0.5 k <= 9.8`
/// on every axis and `sum(t) - 1.5 k >= 12`, which gives `k = 5.8` at `t = (6.9, 6.9, 6.9)`.
fn corner_cut_rows() -> Vec<ScaleRow> {
    let inv_sqrt3 = 1.0 / 3.0_f64.sqrt();
    let mut rows = box_rows([10.0; 3], [0.5; 3], 0.2).to_vec();
    rows.push(ScaleRow {
        normal: [-inv_sqrt3, -inv_sqrt3, -inv_sqrt3],
        support: 1.5 * inv_sqrt3,
        offset: -12.0 * inv_sqrt3,
    });
    rows
}

#[test]
fn test_lp_corner_cut_region_with_negative_offsets_vs_brute_force() {
    let rows = corner_cut_rows();
    let (k_lp, t_lp) = max_scale(&rows).expect("corner-cut region is feasible");

    assert!((k_lp - 5.8).abs() < 1e-9, "expected k = 5.8, got {k_lp}");
    for (axis, c) in t_lp.iter().enumerate() {
        assert!((c - 6.9).abs() < 1e-9, "axis {axis}: expected 6.9, got {c}");
    }

    // Brute force: for every centre on a 0.1 mm grid the best scale is the smallest
    // slack-to-support ratio over the rows.
    let mut best_brute = 0.0_f64;
    for i in 2..=98 {
        for j in 2..=98 {
            for m in 2..=98 {
                let t = [f64::from(i) * 0.1, f64::from(j) * 0.1, f64::from(m) * 0.1];
                let mut k_cand = f64::INFINITY;
                for row in &rows {
                    let dot = row.normal[2]
                        .mul_add(t[2], row.normal[1].mul_add(t[1], row.normal[0] * t[0]));
                    let slack = row.offset - dot;
                    if slack < -1e-9 {
                        k_cand = -1.0;
                        break;
                    }
                    k_cand = k_cand.min(slack / row.support);
                }
                best_brute = best_brute.max(k_cand);
            }
        }
    }
    assert!(
        k_lp >= best_brute - 1e-6,
        "LP scale {k_lp} must be at least brute force {best_brute}"
    );
    assert!(
        k_lp <= best_brute + 1e-6,
        "LP scale {k_lp} exceeds brute force {best_brute}: the grid contains the optimum"
    );
}

#[test]
fn test_lp_feasible_negative_offset_and_infeasible_with_support() {
    // x <= 1 - 0.5 k and x >= 0.5 + 0.5 k: feasible for k <= 0.5, centre x = 0.75.
    let feasible = [
        ScaleRow {
            normal: [1.0, 0.0, 0.0],
            support: 0.5,
            offset: 1.0,
        },
        ScaleRow {
            normal: [-1.0, 0.0, 0.0],
            support: 0.5,
            offset: -0.5,
        },
    ];
    let (k, t) = max_scale(&feasible).expect("region is feasible");
    assert!((k - 0.5).abs() < 1e-12, "expected k = 0.5, got {k}");
    assert!(
        (t[0] - 0.75).abs() < 1e-12,
        "expected x = 0.75, got {}",
        t[0]
    );

    // x <= 0 and x >= 1 cannot both hold even for a zero-size stone; the stone has
    // support, so a wrong phase 1 that believed the region feasible would return a scale.
    let infeasible = [
        ScaleRow {
            normal: [1.0, 0.0, 0.0],
            support: 0.5,
            offset: 0.0,
        },
        ScaleRow {
            normal: [-1.0, 0.0, 0.0],
            support: 0.5,
            offset: -1.0,
        },
    ];
    assert_eq!(max_scale(&infeasible), None);

    // The same contradiction hidden among feasible box rows.
    let mut hidden = box_rows([10.0; 3], [0.5; 3], 0.2).to_vec();
    hidden.push(ScaleRow {
        normal: [1.0, 1.0, 0.0],
        support: 1.0,
        offset: -3.0,
    });
    assert_eq!(max_scale(&hidden), None);
}

#[test]
fn test_lp_scratch_growth_matches_fresh_scratch() {
    let small = box_rows([1.0; 3], [0.5; 3], 0.0).to_vec();
    let large = corner_cut_rows();
    let mut medium = box_rows([10.0; 3], [0.5, 1.0, 0.25], 0.2).to_vec();
    medium.extend_from_slice(&large[..]);

    // Bit patterns of a solver result, so results compare exactly.
    let result_bits = |result: Option<(f64, [f64; 3])>| {
        result.map(|(k, t)| [k.to_bits(), t[0].to_bits(), t[1].to_bits(), t[2].to_bits()])
    };

    let problems = [&small, &large, &small, &medium, &large, &small, &medium];
    let mut reused = LpScratch::new();
    for (idx, rows) in problems.iter().enumerate() {
        let mut fresh = LpScratch::new();
        let on_reused = result_bits(max_scale_with_scratch(rows, &mut reused));
        let on_fresh = result_bits(max_scale_with_scratch(rows, &mut fresh));
        assert!(on_fresh.is_some(), "problem {idx} must be feasible");
        assert_eq!(
            on_reused, on_fresh,
            "problem {idx}: a reused scratch must give the fresh-scratch result"
        );
    }

    // And the large problem after the small one is right, not just consistent.
    let mut scratch = LpScratch::new();
    let (k_small, _) = max_scale_with_scratch(&small, &mut scratch).expect("small problem");
    assert!((k_small - 1.0).abs() < 1e-12);
    let (k_large, _) = max_scale_with_scratch(&large, &mut scratch).expect("large problem");
    assert!(
        (k_large - 5.8).abs() < 1e-9,
        "expected k = 5.8, got {k_large}"
    );
}

#[test]
fn test_lp_capacity_is_stable_across_row_counts() {
    let small = box_rows([1.0; 3], [0.5; 3], 0.0).to_vec();
    let large = corner_cut_rows();
    let mut scratch = LpScratch::new();
    // Warm up on the largest problem, then alternate sizes: no further allocation.
    let _ = max_scale_with_scratch(&large, &mut scratch).expect("large problem");
    let cap_tableau = scratch.tableau.capacity();
    let cap_basic = scratch.basic_vars.capacity();

    for i in 0..300 {
        let rows = if i % 3 == 0 { &large } else { &small };
        let _ = max_scale_with_scratch(rows, &mut scratch).expect("feasible problem");
        assert_eq!(
            scratch.tableau.capacity(),
            cap_tableau,
            "tableau grew at {i}"
        );
        assert_eq!(
            scratch.basic_vars.capacity(),
            cap_basic,
            "basic_vars grew at {i}"
        );
    }
}

#[test]
fn test_lp_tetrahedron_region_cube_stone_vs_brute_force() {
    let inv_sqrt3 = 1.0 / (3.0_f64).sqrt();
    let rows = [
        ScaleRow {
            normal: [-1.0, 0.0, 0.0],
            support: 0.5,
            offset: 0.0,
        },
        ScaleRow {
            normal: [0.0, -1.0, 0.0],
            support: 0.5,
            offset: 0.0,
        },
        ScaleRow {
            normal: [0.0, 0.0, -1.0],
            support: 0.5,
            offset: 0.0,
        },
        ScaleRow {
            normal: [inv_sqrt3, inv_sqrt3, inv_sqrt3],
            support: 1.5 * inv_sqrt3,
            offset: 12.0 * inv_sqrt3,
        },
    ];

    let (k_lp, t_lp) = max_scale(&rows).expect("must find feasible scale in tetrahedron");

    // By hand: the three axis rows give t_i >= k / 2, the slanted row gives
    // t_x + t_y + t_z + 1.5 k <= 12, so 3 k <= 12: k = 4 exactly, at t = (2, 2, 2).
    assert!((k_lp - 4.0).abs() < 1e-9, "k = {k_lp}");
    for (axis, t) in t_lp.iter().enumerate() {
        assert!((t - 2.0).abs() < 1e-9, "t[{axis}] = {t}");
    }

    // Feasibility check
    for (idx, row) in rows.iter().enumerate() {
        let dot_t = f64::mul_add(
            row.normal[2],
            t_lp[2],
            f64::mul_add(row.normal[1], t_lp[1], row.normal[0] * t_lp[0]),
        );
        let lhs = f64::mul_add(k_lp, row.support, dot_t);
        assert!(
            lhs <= row.offset + 1e-9,
            "row {idx} violated: lhs={lhs}, offset={}",
            row.offset
        );
    }

    // Brute-force 40^3 grid search over target_t
    let grid_steps = 40;
    let grid_steps_f64 = f64::from(grid_steps);
    let mut best_k_brute = 0.0;
    for i in 0..=grid_steps {
        let coord_x = 12.0 * f64::from(i) / grid_steps_f64;
        for j in 0..=(grid_steps - i) {
            let coord_y = 12.0 * f64::from(j) / grid_steps_f64;
            for m in 0..=(grid_steps - i - j) {
                let coord_z = 12.0 * f64::from(m) / grid_steps_f64;
                let target_t = [coord_x, coord_y, coord_z];

                let mut k_cand = f64::INFINITY;
                for row in &rows {
                    let dot = f64::mul_add(
                        row.normal[2],
                        target_t[2],
                        f64::mul_add(row.normal[1], target_t[1], row.normal[0] * target_t[0]),
                    );
                    let rem = row.offset - dot;
                    if rem < 0.0 {
                        k_cand = -1.0;
                        break;
                    }
                    k_cand = k_cand.min(rem / row.support);
                }
                if k_cand > best_k_brute {
                    best_k_brute = k_cand;
                }
            }
        }
    }

    assert!(
        k_lp >= best_k_brute - 1e-6,
        "LP scale {k_lp} must be at least brute force {best_k_brute} - 1e-6"
    );
}

#[test]
fn test_lp_empty_region_returns_none() {
    // x <= 0 and -x <= -1 (x >= 1) -> contradictory empty region
    let rows = [
        ScaleRow {
            normal: [1.0, 0.0, 0.0],
            support: 0.0,
            offset: 0.0,
        },
        ScaleRow {
            normal: [-1.0, 0.0, 0.0],
            support: 0.0,
            offset: -1.0,
        },
    ];
    assert_eq!(max_scale(&rows), None);
}

#[test]
fn test_lp_degenerate_duplicate_rows() {
    let mut rows = Vec::new();
    let base_rows = [
        ScaleRow {
            normal: [1.0, 0.0, 0.0],
            support: 0.5,
            offset: 1.0,
        },
        ScaleRow {
            normal: [-1.0, 0.0, 0.0],
            support: 0.5,
            offset: 0.0,
        },
        ScaleRow {
            normal: [0.0, 1.0, 0.0],
            support: 0.5,
            offset: 1.0,
        },
        ScaleRow {
            normal: [0.0, -1.0, 0.0],
            support: 0.5,
            offset: 0.0,
        },
        ScaleRow {
            normal: [0.0, 0.0, 1.0],
            support: 0.5,
            offset: 1.0,
        },
        ScaleRow {
            normal: [0.0, 0.0, -1.0],
            support: 0.5,
            offset: 0.0,
        },
    ];

    for row in &base_rows {
        rows.push(*row);
        rows.push(*row);
        rows.push(*row);
    }

    let (k, t) = max_scale(&rows).expect("must solve degenerate duplicate rows");
    assert!((k - 1.0).abs() < 1e-12);
    assert!((t[0] - 0.5).abs() < 1e-12);
    assert!((t[1] - 0.5).abs() < 1e-12);
    assert!((t[2] - 0.5).abs() < 1e-12);
}

#[test]
fn test_lp_determinism() {
    let rows = [
        ScaleRow {
            normal: [1.0, 0.0, 0.0],
            support: 0.5,
            offset: 1.0,
        },
        ScaleRow {
            normal: [-1.0, 0.0, 0.0],
            support: 0.5,
            offset: 0.0,
        },
        ScaleRow {
            normal: [0.0, 1.0, 0.0],
            support: 0.5,
            offset: 1.0,
        },
        ScaleRow {
            normal: [0.0, -1.0, 0.0],
            support: 0.5,
            offset: 0.0,
        },
        ScaleRow {
            normal: [0.0, 0.0, 1.0],
            support: 0.5,
            offset: 1.0,
        },
        ScaleRow {
            normal: [0.0, 0.0, -1.0],
            support: 0.5,
            offset: 0.0,
        },
    ];

    let res1 = max_scale(&rows).expect("res1");
    let res2 = max_scale(&rows).expect("res2");

    assert_eq!(res1.0.to_bits(), res2.0.to_bits());
    assert_eq!(res1.1[0].to_bits(), res2.1[0].to_bits());
    assert_eq!(res1.1[1].to_bits(), res2.1[1].to_bits());
    assert_eq!(res1.1[2].to_bits(), res2.1[2].to_bits());
}

#[test]
fn test_lp_no_allocation_growth() {
    let rows = [
        ScaleRow {
            normal: [1.0, 0.0, 0.0],
            support: 0.5,
            offset: 1.0,
        },
        ScaleRow {
            normal: [-1.0, 0.0, 0.0],
            support: 0.5,
            offset: 0.0,
        },
        ScaleRow {
            normal: [0.0, 1.0, 0.0],
            support: 0.5,
            offset: 1.0,
        },
        ScaleRow {
            normal: [0.0, -1.0, 0.0],
            support: 0.5,
            offset: 0.0,
        },
        ScaleRow {
            normal: [0.0, 0.0, 1.0],
            support: 0.5,
            offset: 1.0,
        },
        ScaleRow {
            normal: [0.0, 0.0, -1.0],
            support: 0.5,
            offset: 0.0,
        },
    ];

    let mut scratch = LpScratch::new();
    let _ = max_scale_with_scratch(&rows, &mut scratch).expect("first call");

    let cap_tableau = scratch.tableau.capacity();
    let cap_basic = scratch.basic_vars.capacity();

    for i in 0..1000 {
        let mut modified_rows = rows;
        modified_rows[0].offset = f64::from(i).mul_add(0.01, 1.0);
        let _ = max_scale_with_scratch(&modified_rows, &mut scratch);
        assert_eq!(
            scratch.tableau.capacity(),
            cap_tableau,
            "tableau capacity grew on iteration {i}"
        );
        assert_eq!(
            scratch.basic_vars.capacity(),
            cap_basic,
            "basic_vars capacity grew on iteration {i}"
        );
    }
}

#[test]
fn test_lp_unbounded_scale_returns_none() {
    // `x <= 1` with a stone of no support: nothing limits k, so the maximum does not exist.
    // The entering column k has a zero coefficient in the only row, so no row can leave.
    let rows = [ScaleRow {
        normal: [1.0, 0.0, 0.0],
        support: 0.0,
        offset: 1.0,
    }];
    assert_eq!(max_scale(&rows), None);

    // Still unbounded with a bounded box around t: only the rows with support matter for k,
    // and here every support is zero.
    let boxed = box_rows([10.0; 3], [0.0; 3], 0.0);
    assert_eq!(max_scale(&boxed), None);
}

#[test]
fn test_lp_iteration_cap_returns_none() {
    // The unit cube needs several pivots (k enters degenerately at t = 0, then the centre
    // has to move to 0.5 on every axis), so a cap of 0 or 1 iteration cannot finish, while
    // the production cap `50 * (rows + 8)` does.
    let rows = box_rows([1.0; 3], [0.5; 3], 0.0);
    let mut scratch = LpScratch::new();
    assert_eq!(solve(&rows, &mut scratch, 0), None);
    assert_eq!(solve(&rows, &mut scratch, 1), None);
    let cap = 50 * (rows.len() + 8);
    let (k, t) = solve(&rows, &mut scratch, cap).expect("the production cap suffices");
    assert!((k - 1.0).abs() < 1e-12, "expected k = 1.0, got {k}");
    assert!(t.iter().all(|c| (c - 0.5).abs() < 1e-12), "t = {t:?}");
}

#[test]
fn test_lp_non_finite_rows_are_rejected() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for component in 0..3 {
            let mut rows = box_rows([10.0; 3], [0.5; 3], 0.2);
            rows[2].normal[component] = bad;
            assert_eq!(max_scale(&rows), None, "normal[{component}] = {bad}");
        }
        let mut support = box_rows([10.0; 3], [0.5; 3], 0.2);
        support[1].support = bad;
        assert_eq!(max_scale(&support), None, "support = {bad}");
        let mut offset = box_rows([10.0; 3], [0.5; 3], 0.2);
        offset[3].offset = bad;
        assert_eq!(max_scale(&offset), None, "offset = {bad}");
    }
}

/// The inset box `[0.2, 9.8]^3` (three rows with negative offsets, so three artificials)
/// plus 144 non-binding rows: cube-stone halfspaces `n . t + k h <= 100` for the first 144
/// integer directions `(a, b, c)` in `-3..=3` (in lexicographic order, the zero vector
/// left out), normalised, with the cube support `h = 0.5 (|a| + |b| + |c|) / |(a, b, c)|`.
/// The centre can be at most `5 sqrt 3` from the origin and the support term at most
/// `0.87 * 9.6`, so every extra row stays far below 100 and only the box binds.
fn many_row_problem() -> Vec<ScaleRow> {
    let mut rows = box_rows([10.0; 3], [0.5; 3], 0.2).to_vec();
    let mut extra = 0;
    for a in -3_i32..=3 {
        for b in -3_i32..=3 {
            for c in -3_i32..=3 {
                if (a, b, c) == (0, 0, 0) || extra == 144 {
                    continue;
                }
                let len = f64::from(a * a + b * b + c * c).sqrt();
                rows.push(ScaleRow {
                    normal: [f64::from(a) / len, f64::from(b) / len, f64::from(c) / len],
                    support: 0.5 * f64::from(a.abs() + b.abs() + c.abs()) / len,
                    offset: 100.0,
                });
                extra += 1;
            }
        }
    }
    rows
}

#[test]
fn test_lp_one_hundred_fifty_rows_with_inset_matches_the_box() {
    let rows = many_row_problem();
    assert_eq!(rows.len(), 150);
    // Only the inset box binds: a cube stone of half-width 0.5 k needs 0.5 k <= 4.8, so
    // k = 9.6 with the centre at 5 on every axis.
    let (k, t) = max_scale(&rows).expect("the 150-row problem is feasible");
    assert!((k - 9.6).abs() < 1e-9, "expected k = 9.6, got {k}");
    for (axis, c) in t.iter().enumerate() {
        assert!((c - 5.0).abs() < 1e-9, "axis {axis}: expected 5, got {c}");
    }
}
