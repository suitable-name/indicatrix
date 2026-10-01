use super::{
    piece::{Norm, stone_value},
    *,
};
use crate::yield_metrics::carat_weight;

/// A local linear congruential generator (no dependency).
pub(super) struct Lcg(pub u64);

impl Lcg {
    pub(super) fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    pub(super) fn unit(&mut self) -> f64 {
        self.next_u64() as f64 / (1u64 << 53) as f64
    }

    pub(super) fn range(&mut self, lo: f64, hi: f64) -> f64 {
        (hi - lo).mul_add(self.unit(), lo)
    }
}

pub(super) fn random_designs(rng: &mut Lcg, n: usize) -> Vec<CandidateDesign> {
    (0..n)
        .map(|i| {
            let width = rng.range(1.0, 3.0);
            let length = width * rng.range(1.0, 2.5);
            let height = width * rng.range(0.3, 1.5);
            CandidateDesign {
                entry_id: i as i64 + 1,
                width,
                length,
                height,
                volume: width * length * height * rng.range(0.3, 0.9),
            }
        })
        .collect()
}

pub(super) fn box_design(entry_id: i64) -> CandidateDesign {
    CandidateDesign {
        entry_id,
        width: 1.0,
        length: 1.0,
        height: 1.0,
        volume: 1.0,
    }
}

pub(super) fn settings_with(count: u8, kerf: f64, allowance: f64, min_width: f64) -> PlanSettings {
    PlanSettings {
        count,
        kerf_mm: kerf,
        allowance_mm: allowance,
        skin_mm: 0.0,
        min_width_mm: min_width,
        specific_gravity: 2.65,
    }
}

pub(super) fn cube(edge: f64) -> RoughBlock {
    RoughBlock {
        x_mm: edge,
        y_mm: edge,
        z_mm: edge,
    }
}

pub(super) fn plan(
    rough: &RoughBlock,
    settings: &PlanSettings,
    designs: &[CandidateDesign],
) -> Vec<RoughLayout> {
    plan_rough(rough, settings, designs, &mut |_| true).expect("not cancelled")
}

pub(super) fn close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * a.abs().max(b.abs()).max(1e-300)
}

#[test]
fn k1_matches_closed_form() {
    let mut rng = Lcg(7);
    let designs = random_designs(&mut rng, 25);
    let rough = RoughBlock {
        x_mm: 10.0,
        y_mm: 7.0,
        z_mm: 5.0,
    };
    let settings = settings_with(1, 0.0, 0.0, 0.01);
    let perms = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let r = rough.sizes();
    let mut expected = 0.0f64;
    for d in &designs {
        let dims = [d.width, d.length, d.height];
        for perm in perms {
            let sigma = (0..3)
                .map(|j| r[perm[j]] / dims[j])
                .fold(f64::INFINITY, f64::min);
            expected = expected.max(d.volume * sigma * sigma * sigma);
        }
    }
    let results = plan(&rough, &settings, &designs);
    assert!(results.len() <= 10);
    assert_eq!(results[0].stone_count(), 1);
    assert!(
        close(results[0].total_volume_mm3, expected, 1e-9),
        "got {} expected {expected}",
        results[0].total_volume_mm3
    );
}

#[test]
fn pareto_dominator_never_loses_on_any_piece() {
    let d = CandidateDesign {
        entry_id: 1,
        width: 1.0,
        length: 2.0,
        height: 1.5,
        volume: 1.2,
    };
    // Different width, smaller l and h, larger fill: dominates d.
    let e = CandidateDesign {
        entry_id: 2,
        width: 2.0,
        length: 3.0,
        height: 2.4,
        volume: 10.0,
    };
    assert_eq!(pareto_front(&[d, e]), vec![e]);
    assert_eq!(pareto_front(&[e, d]), vec![e]);
    let (nd, ne) = (Norm::of(&d), Norm::of(&e));
    let mut rng = Lcg(11);
    for _ in 0..500 {
        let p = [
            rng.range(0.1, 9.0),
            rng.range(0.1, 9.0),
            rng.range(0.1, 9.0),
        ];
        let w_min = rng.range(0.0, 3.0);
        for orient in 0..6 {
            let vd = stone_value(&nd, orient, p, w_min);
            let ve = stone_value(&ne, orient, p, w_min);
            assert!(
                ve >= vd,
                "e lost to d at {p:?}, orient {orient}, w_min {w_min}"
            );
        }
    }
    // Incomparable designs both stay.
    let flat = CandidateDesign {
        entry_id: 3,
        width: 1.0,
        length: 1.0,
        height: 0.5,
        volume: 0.4,
    };
    assert_eq!(pareto_front(&[d, flat]).len(), 2);
    // Exact duplicates (same normalised shape) keep the lower entry id.
    let twin = CandidateDesign {
        entry_id: 0,
        width: 2.0,
        length: 4.0,
        height: 3.0,
        volume: 9.6,
    };
    assert_eq!(pareto_front(&[d, twin]), vec![twin]);
    assert_eq!(pareto_front(&[twin, d]), vec![twin]);
}

#[test]
fn refinement_finds_a_kink_and_not_the_convex_middle() {
    // A 4 x 4 x 6 block cut into two 4 x 4 x 3 pieces of a cube-shaped design.
    // Moving the cut x along z gives min(4, x)^3 + min(4, 6 - x)^3: the middle
    // (x = 3, value 54) is a local MINIMUM and the maximum (72) sits at the
    // kinks x = 4 or x = 2.
    let stone = PlacedStone {
        entry_id: 1,
        piece_origin_mm: [0.0; 3],
        piece_size_mm: [4.0, 4.0, 3.0],
        stone_size_mm: [3.0; 3],
        table_axis: Axis::Z,
        carat: 0.0,
        volume_mm3: 27.0,
        pose: StonePose {
            center_mm: [2.0, 2.0, 1.5],
            axes: [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]],
            mm_per_unit: 3.0,
        },
    };
    let layout = RoughLayout {
        cut_order: CutOrder::Xyz,
        stones: vec![stone, stone],
        cut_plan: CutPlan {
            slabs: vec![SlabCut {
                thickness_mm: 4.0,
                bars: vec![BarCut {
                    width_mm: 4.0,
                    pieces_mm: vec![3.0, 3.0],
                }],
            }],
        },
        total_carat: 0.0,
        total_volume_mm3: 54.0,
        yield_fraction: 0.0,
        exact_fit: false,
    };
    let rough = RoughBlock {
        x_mm: 4.0,
        y_mm: 4.0,
        z_mm: 6.0,
    };
    let settings = settings_with(2, 0.0, 0.0, 0.01);
    let refined = refine(&rough, &settings, &[box_design(1)], &layout);
    assert!(
        close(refined.total_volume_mm3, 72.0, 1e-9),
        "{}",
        refined.total_volume_mm3
    );
    let pieces = &refined.cut_plan.slabs[0].bars[0].pieces_mm;
    assert!(close(pieces[0] + pieces[1], 6.0, 1e-12));
    let big = pieces[0].max(pieces[1]);
    assert!(close(big, 4.0, 1e-6), "pieces {pieces:?}");
}

pub(super) fn shuffled<T>(items: &mut [T], rng: &mut Lcg) {
    for i in (1..items.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

#[test]
fn input_order_does_not_change_the_result() {
    let mut rng = Lcg(21);
    let designs = random_designs(&mut rng, 18);
    let rough = RoughBlock {
        x_mm: 12.0,
        y_mm: 9.0,
        z_mm: 8.0,
    };
    let settings = settings_with(5, 0.3, 0.2, 1.0);
    let baseline = plan(&rough, &settings, &designs);
    assert_ne!(baseline.len(), 0);
    for seed in 1..4 {
        let mut shuffled_designs = designs.clone();
        shuffled(&mut shuffled_designs, &mut Lcg(seed));
        assert_eq!(plan(&rough, &settings, &shuffled_designs), baseline);
    }
}

#[test]
fn box_design_in_a_cube_with_no_losses_ties_across_counts() {
    let rough = cube(10.0);
    let settings = settings_with(8, 0.0, 0.0, 0.01);
    let results = plan(&rough, &settings, &[box_design(1)]);
    // Every count fills the cube, so the tie-break picks the fewest stones.
    assert_eq!(results[0].stone_count(), 1);
    assert!(close(results[0].yield_fraction, 1.0, 1e-9));
    // The 8-stone layout exists and is 2 x 2 x 2 cells of 5 mm.
    let uniform =
        uniform_layouts(&rough, &settings, &[box_design(1)], &mut |_| true).expect("not cancelled");
    let eight = uniform
        .iter()
        .find(|l| l.stone_count() == 8)
        .expect("8-stone layout");
    assert!(close(eight.yield_fraction, 1.0, 1e-9));
    for stone in &eight.stones {
        assert!(stone.piece_size_mm.iter().all(|s| close(*s, 5.0, 1e-9)));
    }
}

#[test]
fn box_design_in_a_cube_with_kerf_prefers_the_uncut_block() {
    let rough = cube(10.0);
    let settings = settings_with(8, 0.3, 0.0, 0.01);
    let results = plan(&rough, &settings, &[box_design(1)]);
    assert_eq!(results[0].stone_count(), 1);
    assert!(close(results[0].total_volume_mm3, 1000.0, 1e-9));
    let uniform =
        uniform_layouts(&rough, &settings, &[box_design(1)], &mut |_| true).expect("not cancelled");
    let eight = uniform
        .iter()
        .find(|l| l.stone_count() == 8)
        .expect("8-stone layout");
    assert!(close(eight.total_volume_mm3, 9.7f64.powi(3), 1e-12));
}

#[test]
fn a_rough_narrower_than_min_width_plus_allowance_is_empty() {
    // Usable x is 1.3 - 0.4 = 0.9 < 1.0 mm; the box design has l = h = 1, so
    // no assignment can hide the thin axis.
    let rough = RoughBlock {
        x_mm: 1.3,
        y_mm: 5.0,
        z_mm: 5.0,
    };
    let settings = settings_with(4, 0.3, 0.2, 1.0);
    let results = plan_rough(&rough, &settings, &[box_design(1)], &mut |_| true);
    assert_eq!(results, Some(Vec::new()));
}

#[test]
fn a_long_bar_takes_several_stones() {
    let rough = RoughBlock {
        x_mm: 40.0,
        y_mm: 6.0,
        z_mm: 6.0,
    };
    let settings = settings_with(10, 0.3, 0.2, 1.0);
    let results = plan(&rough, &settings, &[box_design(1)]);
    assert_ne!(results.len(), 0);
    // Precondition: one stone fills the cross-section only, a cube of 6 - 2 x 0.2 = 5.6 mm.
    let single = 5.6_f64.powi(3);
    // Along 40 mm six pieces of (40.3 / 6 - 0.3) = 6.4 mm leave boxes of 6.0 mm, wider than
    // the 5.6 mm cross-section, so six full cubes are cut: 6 x 5.6^3 = 1053.7 mm^3. Seven
    // pieces are already narrower than 5.6 mm, so nothing beats six cubes by much, and even
    // a coarse grid finds at least five of them.
    assert!(
        results[0].stone_count() > 1,
        "{} stones",
        results[0].stone_count()
    );
    assert!(
        results[0].total_volume_mm3 > 5.0 * single,
        "{} mm^3 is not at least five cubes",
        results[0].total_volume_mm3
    );
}

#[test]
fn refinement_never_decreases_the_objective() {
    let mut rng = Lcg(33);
    let designs = random_designs(&mut rng, 14);
    let rough = RoughBlock {
        x_mm: 17.0,
        y_mm: 11.0,
        z_mm: 9.0,
    };
    let settings = settings_with(9, 0.3, 0.2, 0.8);
    let front = pareto_front(&designs);
    let grid = choose_grid(&rough, &settings);
    let table = build_piece_table(&grid, &front, &mut |_| true).expect("table");
    let mut layouts = Vec::new();
    for order in CutOrder::ALL {
        let found = plan_rough_for_order(&grid, &table, &front, order, 9, &mut |_| true)
            .expect("not cancelled");
        layouts.extend(found);
    }
    assert_ne!(layouts.len(), 0);
    let uniform =
        uniform_layouts(&rough, &settings, &designs, &mut |_| true).expect("not cancelled");
    assert_ne!(uniform.len(), 0);
    let mut improved = 0;
    for layout in &layouts {
        let refined = refine(&rough, &settings, &front, layout);
        assert!(refined.total_volume_mm3 >= layout.total_volume_mm3);
        improved += usize::from(refined.total_volume_mm3 > layout.total_volume_mm3);
    }
    for layout in &uniform {
        let refined = refine(&rough, &settings, &designs, layout);
        assert!(refined.total_volume_mm3 >= layout.total_volume_mm3);
        improved += usize::from(refined.total_volume_mm3 > layout.total_volume_mm3);
    }
    // The unit grid quantises every DP cut, so among dozens of layouts the continuous
    // refinement must find a strictly better one; a refinement that only ever returned its
    // input would pass the `>=` checks above.
    assert!(
        improved > 0,
        "no layout of {} improved",
        layouts.len() + uniform.len()
    );
}

#[test]
fn the_same_input_gives_bitwise_identical_output() {
    let mut rng = Lcg(5);
    let designs = random_designs(&mut rng, 16);
    let rough = RoughBlock {
        x_mm: 14.0,
        y_mm: 10.0,
        z_mm: 8.0,
    };
    let settings = settings_with(7, 0.3, 0.2, 1.0);
    let first = plan(&rough, &settings, &designs);
    let second = plan(&rough, &settings, &designs);
    assert_eq!(first, second);
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(a.total_volume_mm3.to_bits(), b.total_volume_mm3.to_bits());
        assert_eq!(a.total_carat.to_bits(), b.total_carat.to_bits());
    }
}

#[test]
fn results_are_distinct_capped_and_ranked() {
    let mut rng = Lcg(9);
    let designs = random_designs(&mut rng, 30);
    let rough = RoughBlock {
        x_mm: 16.0,
        y_mm: 12.0,
        z_mm: 10.0,
    };
    let settings = settings_with(12, 0.3, 0.2, 1.0);
    let results = plan(&rough, &settings, &designs);
    assert!(!results.is_empty() && results.len() <= 10);
    let mut compositions: Vec<Vec<(i64, usize)>> =
        results.iter().map(RoughLayout::composition).collect();
    let mut per_set = std::collections::BTreeMap::new();
    for composition in &compositions {
        let set: Vec<i64> = composition.iter().map(|(id, _)| *id).collect();
        *per_set.entry(set).or_insert(0usize) += 1;
    }
    assert!(per_set.values().all(|&n| n <= SAME_SET_CAP), "{per_set:?}");
    compositions.sort();
    compositions.dedup();
    assert_eq!(compositions.len(), results.len());
    for pair in results.windows(2) {
        assert!(pair[0].total_volume_mm3 >= pair[1].total_volume_mm3 * (1.0 - 1e-9));
    }
}

#[test]
fn returning_false_from_progress_cancels_at_every_stage() {
    let mut rng = Lcg(17);
    let designs = random_designs(&mut rng, 10);
    let rough = cube(12.0);
    let settings = settings_with(4, 0.3, 0.2, 1.0);
    let mut calls = 0usize;
    let full = plan_rough(&rough, &settings, &designs, &mut |_| {
        calls += 1;
        true
    });
    assert!(full.is_some());
    for stop in [1, 2, 3, calls / 4, calls / 2, calls - 1, calls] {
        let mut seen = 0usize;
        let cancelled = plan_rough(&rough, &settings, &designs, &mut |_| {
            seen += 1;
            seen < stop
        });
        assert!(cancelled.is_none(), "stop {stop} of {calls}");
    }
}

/// The geometric invariants of one layout of `rough`.
///
/// A sawn layout's pieces lie inside the skin, are a kerf apart and hold a stone that is at
/// least one allowance smaller per side. An exact single-stone fit is one stone whose
/// bounding box is its piece and lies inside the region inset by skin and allowance. Both
/// report totals that add up.
pub(super) fn assert_layout_geometry(
    layout: &RoughLayout,
    rough: &RoughBlock,
    settings: &PlanSettings,
) {
    let inset = settings.skin_mm + settings.allowance_mm;
    let eps = if layout.exact_fit { 1e-6 } else { 1e-9 };
    if layout.exact_fit {
        assert_eq!(layout.stones.len(), 1, "an exact fit is one stone");
    }
    let mut volume = 0.0;
    for (i, a) in layout.stones.iter().enumerate() {
        for axis in 0..3 {
            let end = a.piece_origin_mm[axis] + a.piece_size_mm[axis];
            if layout.exact_fit {
                assert!(a.piece_origin_mm[axis] >= inset - eps);
                assert!(end <= rough.sizes()[axis] - inset + eps);
                assert!(
                    (a.stone_size_mm[axis] - a.piece_size_mm[axis]).abs() <= eps,
                    "an exact fit's piece is its bounding box"
                );
            } else {
                assert!(a.piece_origin_mm[axis] >= settings.skin_mm - eps);
                assert!(end <= rough.sizes()[axis] - settings.skin_mm + eps);
                assert!(
                    a.stone_size_mm[axis]
                        <= 2.0f64.mul_add(-settings.allowance_mm, a.piece_size_mm[axis]) + eps
                );
            }
        }
        volume += a.volume_mm3;
        for b in &layout.stones[i + 1..] {
            let apart = (0..3).any(|axis| {
                let a_end = a.piece_origin_mm[axis] + a.piece_size_mm[axis];
                let b_end = b.piece_origin_mm[axis] + b.piece_size_mm[axis];
                a_end + settings.kerf_mm <= b.piece_origin_mm[axis] + eps
                    || b_end + settings.kerf_mm <= a.piece_origin_mm[axis] + eps
            });
            assert!(apart, "pieces overlap or lack a kerf gap");
        }
        assert!(a.stone_size_mm[a.table_axis.index()] > 0.0);
    }
    assert!(close(layout.total_volume_mm3, volume, 1e-12));
    assert!(close(
        layout.total_carat,
        carat_weight(volume, settings.specific_gravity),
        1e-9
    ));
    assert!(close(
        layout.yield_fraction,
        volume / rough.volume_mm3(),
        1e-12
    ));
}

#[test]
fn layouts_are_geometrically_valid() {
    let mut rng = Lcg(3);
    let designs = random_designs(&mut rng, 20);
    let rough = RoughBlock {
        x_mm: 25.0,
        y_mm: 14.0,
        z_mm: 11.0,
    };
    let settings = PlanSettings {
        skin_mm: 0.4,
        ..settings_with(12, 0.3, 0.2, 1.0)
    };
    let layouts = plan(&rough, &settings, &designs);
    assert_ne!(layouts.len(), 0);
    for layout in &layouts {
        assert!(!layout.exact_fit, "the standalone block planner saws");
        assert_layout_geometry(layout, &rough, &settings);
    }
}

#[test]
fn the_grid_targets_six_units_per_expected_piece() {
    let rough = cube(20.0);
    assert_eq!(
        choose_grid(&rough, &settings_with(99, 0.3, 0.2, 1.0)).cells(),
        [30; 3]
    );
    assert_eq!(
        choose_grid(&rough, &settings_with(20, 0.3, 0.2, 1.0)).cells(),
        [18; 3]
    );
    assert_eq!(
        choose_grid(&rough, &settings_with(1, 0.3, 0.2, 1.0)).cells(),
        [8; 3]
    );
    let bar = RoughBlock {
        x_mm: 40.0,
        y_mm: 6.0,
        z_mm: 6.0,
    };
    let cells = choose_grid(&bar, &settings_with(10, 0.3, 0.2, 1.0)).cells();
    assert!(cells[0] > cells[1] && cells.iter().all(|c| (8..=48).contains(c)));
}

#[test]
fn leave_one_out_alternatives_drop_the_most_used_design() {
    let mut rng = Lcg(41);
    let designs = random_designs(&mut rng, 12);
    let rough = cube(14.0);
    let settings = settings_with(4, 0.3, 0.2, 1.0);
    let front = pareto_front(&designs);
    let grid = choose_grid(&rough, &settings);
    let table = build_piece_table(&grid, &front, &mut |_| true).expect("table");
    let mut mixed = Vec::new();
    for order in CutOrder::ALL {
        mixed.extend(
            plan_rough_for_order(&grid, &table, &front, order, 4, &mut |_| true).expect("order"),
        );
    }
    let best = merge_and_rank(mixed, 1).remove(0);
    let groups = plan_alternatives(&grid, &designs, &best, 4, &mut |_| true).expect("alternatives");
    assert_ne!(groups.len(), 0);
    let mut removed: Vec<i64> = Vec::new();
    let mut current = best;
    for group in &groups {
        removed.push(super::rank::most_used_design(&current).expect("a stone"));
        // Every removed id is absent from the pool and from every layout.
        assert!(group.pool.iter().all(|d| !removed.contains(&d.entry_id)));
        // The pool is the Pareto front of the full list minus the removed ids.
        let remaining: Vec<CandidateDesign> = designs
            .iter()
            .filter(|d| !removed.contains(&d.entry_id))
            .copied()
            .collect();
        assert_eq!(group.pool, pareto_front(&remaining));
        assert_ne!(group.layouts.len(), 0);
        for layout in &group.layouts {
            assert!(layout.stones.iter().all(|s| !removed.contains(&s.entry_id)));
        }
        current = merge_and_rank(group.layouts.clone(), 1).remove(0);
    }
}

#[test]
fn a_design_dominated_only_by_the_removed_one_returns_in_round_one() {
    // A (id 1) dominates C (id 2); B (id 3) is incomparable with both.
    let a = box_design(1);
    let c = CandidateDesign {
        volume: 0.9,
        ..box_design(2)
    };
    let b = CandidateDesign {
        entry_id: 3,
        width: 1.0,
        length: 2.0,
        height: 0.5,
        volume: 0.8,
    };
    let designs = [a, c, b];
    let front = pareto_front(&designs);
    assert!(front.iter().all(|d| d.entry_id != 2));
    let rough = cube(10.0);
    let settings = settings_with(4, 0.0, 0.0, 0.01);
    let grid = choose_grid(&rough, &settings);
    let table = build_piece_table(&grid, &front, &mut |_| true).expect("table");
    let mut mixed = Vec::new();
    for order in CutOrder::ALL {
        mixed.extend(
            plan_rough_for_order(&grid, &table, &front, order, 4, &mut |_| true).expect("order"),
        );
    }
    let best = merge_and_rank(mixed, 1).remove(0);
    assert!(best.stones.iter().all(|s| s.entry_id == 1));
    let groups = plan_alternatives(&grid, &designs, &best, 4, &mut |_| true).expect("alternatives");
    assert_ne!(groups.len(), 0);
    assert!(groups[0].pool.iter().any(|d| d.entry_id == 2));
    assert!(
        groups[0]
            .layouts
            .iter()
            .any(|l| l.stones.iter().any(|s| s.entry_id == 2))
    );
}

#[test]
fn the_final_list_is_topped_up_when_refinement_merges_compositions() {
    // 30 single-stone layouts, each of its own design; the shared pool also
    // holds design 100, which beats them all, so refining the top 20 collapses
    // them into one composition. The unrefined layouts ranked 21..30 must fill
    // the list back up to 10.
    let rough = cube(10.0);
    let settings = settings_with(1, 0.0, 0.0, 0.01);
    let mut designs: Vec<CandidateDesign> = (1..=30_i32)
        .map(|i| CandidateDesign {
            volume: f64::from(i).mul_add(-0.01, 0.9),
            ..box_design(i64::from(i))
        })
        .collect();
    designs.push(box_design(100));
    let layouts: Vec<RoughLayout> = designs[..30]
        .iter()
        .map(|d| {
            let stone = PlacedStone {
                entry_id: d.entry_id,
                piece_origin_mm: [0.0; 3],
                piece_size_mm: [10.0; 3],
                stone_size_mm: [10.0; 3],
                table_axis: Axis::Z,
                carat: 0.0,
                volume_mm3: d.volume * 1000.0,
                pose: StonePose {
                    center_mm: [5.0, 5.0, 5.0],
                    axes: [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]],
                    mm_per_unit: 10.0,
                },
            };
            RoughLayout {
                cut_order: CutOrder::Xyz,
                stones: vec![stone],
                cut_plan: CutPlan {
                    slabs: vec![SlabCut {
                        thickness_mm: 10.0,
                        bars: vec![BarCut {
                            width_mm: 10.0,
                            pieces_mm: vec![10.0],
                        }],
                    }],
                },
                total_carat: 0.0,
                total_volume_mm3: d.volume * 1000.0,
                yield_fraction: 0.0,
                exact_fit: false,
            }
        })
        .collect();
    let groups = [LayoutGroup {
        pool: designs.clone(),
        layouts,
    }];
    let results =
        finish_plan(&rough, &settings, &designs, &groups, &mut |_| true).expect("not cancelled");
    assert_eq!(results.len(), 10);
    assert_eq!(results[0].composition(), vec![(100, 1)]);
    let mut compositions: Vec<Vec<(i64, usize)>> =
        results.iter().map(RoughLayout::composition).collect();
    compositions.sort();
    compositions.dedup();
    assert_eq!(compositions.len(), 10);
}

#[test]
fn cut_orders_print_their_stages() {
    assert_eq!(CutOrder::Xyz.to_string(), "X, then Y, then Z");
    assert_eq!(CutOrder::Zxy.to_string(), "Z, then X, then Y");
    for order in CutOrder::ALL {
        assert_eq!(CutOrder::from_axes(order.axes()), order);
        assert_eq!(CutOrder::ALL[order.index()], order);
    }
}

#[test]
fn all_placed_stones_have_orthonormal_right_handed_poses() {
    let mut rng = Lcg(42);
    let designs = random_designs(&mut rng, 10);
    let rough = RoughBlock {
        x_mm: 12.0,
        y_mm: 10.0,
        z_mm: 8.0,
    };
    let settings = settings_with(6, 0.3, 0.2, 0.5);
    let results = plan(&rough, &settings, &designs);
    assert_ne!(results.len(), 0);
    for layout in &results {
        for stone in &layout.stones {
            let axes = stone.pose.axes;
            for (i, axis) in axes.iter().enumerate() {
                let len_sq = axis[0].mul_add(axis[0], axis[1].mul_add(axis[1], axis[2] * axis[2]));
                assert!((len_sq - 1.0).abs() < 1e-9, "axis {i} not unit: {len_sq}");
            }
            for i in 0..3 {
                for j in (i + 1)..3 {
                    let dot = axes[i][0].mul_add(
                        axes[j][0],
                        axes[i][1].mul_add(axes[j][1], axes[i][2] * axes[j][2]),
                    );
                    assert!(dot.abs() < 1e-9, "axes {i},{j} not orthogonal: {dot}");
                }
            }
            let c0 = axes[1][1].mul_add(axes[2][2], -axes[1][2] * axes[2][1]);
            let c1 = axes[1][0].mul_add(axes[2][2], -axes[1][2] * axes[2][0]);
            let c2 = axes[1][0].mul_add(axes[2][1], -axes[1][1] * axes[2][0]);
            let det = axes[0][0].mul_add(c0, axes[0][2].mul_add(c2, -axes[0][1] * c1));
            assert!((det - 1.0).abs() < 1e-9, "det(axes) != +1: {det}");
        }
    }
}
