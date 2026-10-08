//! The helpers that assemble the final list: the uniform pool, each layout's own pool,
//! the flat ranking and the top-up after refinement.

use super::{
    Axis, CandidateDesign, CutOrder, CutPlan, FINAL_TOP, LayoutGroup, PlacedStone, RoughLayout,
    SingleFit, StonePose, final_ranking, final_ranking_min, flatten_groups, merge_and_rank,
    merge_and_rank_min, own_pool, partial_ranking, partial_ranking_min, rank_indices,
    rank_indices_min, uniform_pool,
};

fn design(entry_id: i64) -> CandidateDesign {
    CandidateDesign {
        entry_id,
        width: 1.0,
        length: 1.5,
        height: 0.7,
        volume: 0.9,
    }
}

fn pose() -> StonePose {
    StonePose {
        center_mm: [0.0; 3],
        axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        mm_per_unit: 1.0,
    }
}

fn stone(entry_id: i64, volume: f64) -> PlacedStone {
    PlacedStone {
        entry_id,
        piece_origin_mm: [0.0; 3],
        piece_size_mm: [1.0; 3],
        stone_size_mm: [1.0; 3],
        table_axis: Axis::Y,
        carat: volume / 10.0,
        volume_mm3: volume,
        pose: pose(),
    }
}

/// A layout of `(entry_id, volume)` stones.
fn layout(stones: &[(i64, f64)]) -> RoughLayout {
    let total: f64 = stones.iter().map(|s| s.1).sum();
    RoughLayout {
        cut_order: CutOrder::Xyz,
        stones: stones.iter().map(|&(id, v)| stone(id, v)).collect(),
        cut_plan: CutPlan { slabs: Vec::new() },
        total_carat: total / 10.0,
        total_volume_mm3: total,
        yield_fraction: 0.1,
        exact_fit: false,
    }
}

fn fit(entry_id: i64) -> SingleFit {
    SingleFit {
        entry_id,
        pose: pose(),
        volume_mm3: 1.0,
        carat: 0.1,
    }
}

#[test]
fn the_uniform_pool_starts_with_the_fitted_designs_and_has_no_repeats() {
    let designs: Vec<_> = (1..=5).map(design).collect();
    let front = vec![designs[0], designs[1], designs[2]];
    let pool = uniform_pool(&front, &[fit(4), fit(2), fit(99)], &designs);
    let ids: Vec<i64> = pool.iter().map(|d| d.entry_id).collect();
    // Fitted designs first (99 is not a candidate), then the rest of the front.
    assert_eq!(ids, vec![4, 2, 1, 3]);
}

#[test]
fn the_uniform_pool_is_capped_at_sixty_four() {
    let designs: Vec<_> = (1..=100).map(design).collect();
    let pool = uniform_pool(&designs, &[fit(90)], &designs);
    assert_eq!(pool.len(), 64);
    assert_eq!(pool[0].entry_id, 90);
}

#[test]
fn own_pool_keeps_only_the_designs_a_layout_uses() {
    let designs: Vec<_> = (1..=4).map(design).collect();
    let used = layout(&[(3, 1.0), (1, 1.0), (3, 1.0)]);
    let ids: Vec<i64> = own_pool(&designs, &used)
        .iter()
        .map(|d| d.entry_id)
        .collect();
    assert_eq!(ids, vec![1, 3]);
}

#[test]
fn rank_indices_name_the_layouts_merge_and_rank_returns() {
    let repeated = layout(&[(9, 4.0)]);
    let flat = vec![
        repeated.clone(),
        layout(&[(2, 30.0)]),
        repeated,
        layout(&[(3, 20.0)]),
        layout(&[(1, 3.0), (1, 3.0)]),
        layout(&[(1, 2.5), (1, 2.5), (1, 2.5)]),
        layout(&[(1, 2.0), (1, 2.0), (1, 2.0), (1, 2.0)]),
        layout(&[(1, 1.0)]),
        layout(&[(4, 1.0), (5, 60.0)]),
    ];
    // Hand-derived ranking. Volumes: #8 = 1 + 60 = 61, #1 = 30, #3 = 20, #6 = 4 x 2 = 8,
    // #5 = 3 x 2.5 = 7.5, #4 = 2 x 3 = 6, #0 = #2 = 4, #7 = 1. #2 repeats #0's composition
    // (9, 1) and is dropped; #6, #5, #4 and #7 all use the design set {1}, so the cap of
    // three keeps #6, #5, #4 and drops #7. Every limit returns a prefix of that list.
    let expected = [8, 1, 3, 6, 5, 4, 0];
    for limit in [1, 2, 3, 5, 20] {
        let indices = rank_indices(&flat, limit);
        assert_eq!(
            indices,
            expected[..expected.len().min(limit)],
            "limit {limit}"
        );
        let by_index: Vec<RoughLayout> = indices.iter().map(|&i| flat[i].clone()).collect();
        assert_eq!(
            by_index,
            merge_and_rank(flat.clone(), limit),
            "limit {limit}"
        );
    }
    let all = rank_indices(&flat, 20);
    assert!(
        all.contains(&0) && !all.contains(&2),
        "the first of two equal layouts stays"
    );
    // Four layouts share the design set {1}: only the best three stay.
    let of_set_one = all
        .iter()
        .filter(|&&i| flat[i].stones[0].entry_id == 1)
        .count();
    assert_eq!(of_set_one, 3);
    assert!(
        !all.contains(&7),
        "the weakest of the four is the one dropped"
    );
    assert_eq!(all[0], 8, "the pair worth 61 mm^3 is the best");
}

#[test]
fn the_final_ranking_tops_up_with_the_unrefined_candidates() {
    let flat: Vec<RoughLayout> = (1..=12)
        .map(|id| layout(&[(id, 10.0 + id as f64)]))
        .collect();
    let ranked = rank_indices(&flat, 5);
    assert_eq!(ranked.len(), 5);
    let refined: Vec<RoughLayout> = ranked.iter().map(|&i| flat[i].clone()).collect();

    let topped = final_ranking(refined.clone(), &flat, &ranked);
    assert_eq!(topped.len(), FINAL_TOP);
    // Volumes are 10 + id and every layout has its own design set, so the ten best are the
    // ids 12 down to 3: five from the refined list and ids 7 to 3 from the unrefined rest.
    let ids: Vec<i64> = topped.iter().map(|l| l.stones[0].entry_id).collect();
    assert_eq!(ids, (3..=12).rev().collect::<Vec<i64>>());

    // Nothing left to top up from: the refined list stands as it is.
    let everything: Vec<usize> = (0..flat.len()).collect();
    let alone = final_ranking(refined, &flat, &everything);
    assert_eq!(alone.len(), 5);
}

/// Twelve layouts: stone counts 1 to 4, distinct designs so the dedup and the set cap stay
/// out of the way. Layout `i` (0-based) holds `1 + i % 4` stones and is worth `100 - i` mm^3 a stone.
fn mixed_counts() -> Vec<RoughLayout> {
    (0..12_i64)
        .map(|i| {
            let stones: Vec<(i64, f64)> = (0..=i % 4)
                .map(|k| (i * 10 + k, 100.0 - i as f64))
                .collect();
            layout(&stones)
        })
        .collect()
}

#[test]
fn a_minimum_of_one_stone_is_exactly_the_plain_ranking() {
    let flat = mixed_counts();
    for limit in [1, 4, 10, 20] {
        assert_eq!(
            rank_indices_min(&flat, limit, 1),
            rank_indices(&flat, limit)
        );
        assert_eq!(
            rank_indices_min(&flat, limit, 0),
            rank_indices(&flat, limit),
            "0 reads as 1"
        );
        assert_eq!(
            merge_and_rank_min(flat.clone(), limit, 1),
            merge_and_rank(flat.clone(), limit)
        );
    }
}

#[test]
fn a_minimum_drops_the_small_layouts_before_the_limit_and_refills_from_the_rest() {
    let flat = mixed_counts();
    let kept = rank_indices_min(&flat, 20, 2);
    assert!(!kept.is_empty());
    assert!(kept.iter().all(|&i| flat[i].stones.len() >= 2));
    // Nine of the twelve hold two stones or more, and all of them qualify.
    assert_eq!(kept.len(), 9);
    // A limit smaller than the qualifying count is filled with qualifying layouts only
    // (the one-stone layouts that would have ranked in the top 3 are gone).
    let top = rank_indices_min(&flat, 3, 2);
    assert_eq!(top.len(), 3);
    assert!(top.iter().all(|&i| flat[i].stones.len() >= 2));
    assert_eq!(top, kept[..3]);
    // Above every layout: nothing.
    assert!(rank_indices_min(&flat, 20, 5).is_empty());
    assert!(merge_and_rank_min(flat, 20, 5).is_empty());
}

#[test]
fn the_final_ranking_top_up_refills_from_layouts_that_qualify() {
    let flat = mixed_counts();
    let ranked = rank_indices_min(&flat, 3, 2);
    let refined: Vec<RoughLayout> = ranked.iter().map(|&i| flat[i].clone()).collect();
    let topped = final_ranking_min(refined.clone(), &flat, &ranked, 2);
    assert_eq!(topped.len(), 9, "every qualifying layout, no more");
    assert!(topped.iter().all(|l| l.stones.len() >= 2));
    // With a minimum of one the top-up is the plain one.
    assert_eq!(
        final_ranking_min(refined.clone(), &flat, &ranked, 1),
        final_ranking(refined, &flat, &ranked)
    );
}

#[test]
fn the_partial_ranking_honours_the_minimum_too() {
    let groups = vec![LayoutGroup {
        pool: Vec::new(),
        layouts: mixed_counts(),
    }];
    assert_eq!(
        partial_ranking_min(&groups, Vec::new(), 1),
        partial_ranking(&groups, Vec::new())
    );
    let shown = partial_ranking_min(&groups, vec![layout(&[(500, 1000.0)])], 2);
    assert!(!shown.is_empty());
    assert!(shown.iter().all(|l| l.stones.len() >= 2));
    assert!(partial_ranking_min(&groups, Vec::new(), 9).is_empty());
}

#[test]
fn flatten_groups_remembers_the_group_of_every_layout() {
    let groups = vec![
        LayoutGroup {
            pool: vec![design(1)],
            layouts: vec![layout(&[(1, 1.0)]), layout(&[(1, 2.0)])],
        },
        LayoutGroup {
            pool: Vec::new(),
            layouts: vec![layout(&[(2, 3.0)])],
        },
    ];
    let (flat, group_of) = flatten_groups(&groups);
    assert_eq!(flat.len(), 3);
    assert_eq!(group_of, vec![0, 0, 1]);
}

#[test]
fn volumes_one_ulp_either_side_of_the_old_bucket_edge_tie() {
    // The volume key used to round the low 20 mantissa bits, so two adjacent doubles with
    // low bits 0x7_FFFF and 0x8_0000 (100.0 has all-zero low mantissa bits) landed in
    // different buckets and the larger always won, however the stone counts compared.
    let low = f64::from_bits(100.0_f64.to_bits() + 0x7_FFFF);
    let high = f64::from_bits(100.0_f64.to_bits() + 0x8_0000);
    assert_eq!(high.to_bits() - low.to_bits(), 1);

    // `two` totals `high` exactly (two exact halves), `one` totals `low`.
    let two = layout(&[(2, high / 2.0), (2, high / 2.0)]);
    let one = layout(&[(1, low)]);
    assert_eq!(two.total_volume_mm3.to_bits(), high.to_bits());
    assert_eq!(one.total_volume_mm3.to_bits(), low.to_bits());

    // Tied within 2^-32: fewer stones first, although the single stone is one ULP smaller.
    assert_eq!(rank_indices(&[two.clone(), one.clone()], 2), vec![1, 0]);
    assert_eq!(rank_indices(&[one.clone(), two.clone()], 2), vec![0, 1]);

    // A volume 1e-6 larger is no tie (far above 2^-32 = 2.3e-10): it ranks first.
    let x = high * (1.0 + 1e-6);
    let bigger = layout(&[(3, x / 2.0), (3, x / 2.0)]);
    assert_eq!(rank_indices(&[two, one, bigger], 3), vec![2, 1, 0]);
}

#[test]
fn a_chain_of_near_ties_does_not_fuse_into_one_group() {
    // Three single-stone layouts whose volumes are v, v(1 - 0.7t) and v(1 - 1.4t) with
    // t = 2^-32: each neighbouring pair is within the tolerance, the outer pair is not.
    // Ties are taken against the first volume of a group, so the third layout is ranked
    // by volume after the first two, whatever their compositions.
    let t = 1.0 / 4_294_967_296.0;
    let shrink = |k: f64| (-k).mul_add(t, 1.0);
    let volume = 100.0;
    let first = layout(&[(9, volume)]);
    let second = layout(&[(1, volume * shrink(0.7))]);
    let third = layout(&[(0, volume * shrink(1.4))]);
    // First and second tie and order by composition ((1, 1) before (9, 1)); the third is
    // outside the first's tolerance, so it comes last although its id is lowest.
    assert_eq!(rank_indices(&[first, second, third], 3), vec![1, 0, 2]);
}
