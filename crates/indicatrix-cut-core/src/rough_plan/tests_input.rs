//! Input handling of the block planner: sanitising, the Pareto front on hand-built sets,
//! validation, absurd settings, the minimum-width boundary and the plain-block entry point
//! with exact single-stone fits.

use super::{
    pareto::sanitize,
    piece::{Norm, best_pick, stone_value},
    refine::KinkFrame,
    tests::{
        Lcg, assert_layout_geometry, box_design, cube, plan, random_designs, settings_with,
        shuffled,
    },
    *,
};

/// A design of unit width whose normalised shape is `(1, l, h, f)`.
fn shaped_design(entry_id: i64, l: f64, h: f64, f: f64) -> CandidateDesign {
    CandidateDesign {
        entry_id,
        width: 1.0,
        length: l,
        height: h,
        volume: f,
    }
}

#[test]
fn the_front_of_five_hand_built_designs() {
    // All five have unit width, so (length, height, volume) are (l, h, f).
    let base = shaped_design(1, 1.0, 1.0, 1.0);
    // Longer but fuller than the base: incomparable, stays.
    let fuller = shaped_design(2, 2.0, 1.0, 1.5);
    // Longer than the base at the same height and fill: the base dominates it.
    let longer = shaped_design(3, 2.0, 1.0, 1.0);
    // The base's shape under a higher id: an exact duplicate, the lower id (the base) stays.
    let twin = shaped_design(4, 1.0, 1.0, 1.0);
    // Flatter than the base but holds less: incomparable, stays.
    let flat = shaped_design(5, 1.0, 0.5, 0.4);
    let expected = vec![base, fuller, flat];
    assert_eq!(pareto_front(&[base, fuller, longer, twin, flat]), expected);
    assert_eq!(pareto_front(&[flat, twin, longer, fuller, base]), expected);
    assert_eq!(pareto_front(&[longer, flat, base, twin, fuller]), expected);
}

#[test]
fn the_front_of_three_thousand_designs_is_exactly_the_antichain() {
    // 1000 designs with l = 1 + i/1000 and h = 2 - i/1000 (fill 1): a longer design is
    // always lower, so none dominates another. 2000 twins repeat those shapes with fill 0.5,
    // so each is dominated by its original. The front is exactly the 1000 originals.
    let originals: Vec<CandidateDesign> = (0..1000_i32)
        .map(|i| {
            shaped_design(
                i64::from(i) + 1,
                f64::from(i).mul_add(0.001, 1.0),
                f64::from(i).mul_add(-0.001, 2.0),
                1.0,
            )
        })
        .collect();
    let mut all = originals.clone();
    for k in 0..2000_usize {
        all.push(CandidateDesign {
            entry_id: 1001 + i64::try_from(k).expect("small"),
            volume: 0.5,
            ..originals[k % 1000]
        });
    }
    assert_eq!(all.len(), 3000);
    shuffled(&mut all, &mut Lcg(3000));
    let front = pareto_front(&all);
    assert_eq!(front.len(), 1000);
    assert_eq!(front, originals);
}

#[test]
fn a_single_design_is_its_own_front() {
    let d = random_designs(&mut Lcg(12), 1)[0];
    assert_eq!(pareto_front(&[d]), vec![d]);
    assert_eq!(pareto_front(&[]), Vec::<CandidateDesign>::new());
}

#[test]
fn sanitize_drops_invalid_designs_and_merges_identical_repeats() {
    let valid = random_designs(&mut Lcg(8), 1)[0];
    let with_id = |entry_id: i64| CandidateDesign { entry_id, ..valid };
    let mixed = [
        valid,
        CandidateDesign {
            width: f64::NAN,
            ..with_id(2)
        },
        CandidateDesign {
            volume: f64::INFINITY,
            ..with_id(3)
        },
        CandidateDesign {
            height: 0.0,
            ..with_id(4)
        },
        CandidateDesign {
            length: -1.0,
            ..with_id(5)
        },
    ];
    assert_eq!(sanitize(&mixed), vec![valid]);
    // The same id and shape twice is one design; the same id with another shape stays twice.
    assert_eq!(sanitize(&[valid, valid]), vec![valid]);
    let other = CandidateDesign {
        volume: valid.volume * 0.5,
        ..valid
    };
    assert_eq!(sanitize(&[other, valid, valid]).len(), 2);
    assert_eq!(pareto_front(&[valid, valid]), vec![valid]);
}

#[test]
fn invalid_designs_do_not_change_the_plan() {
    let valid = random_designs(&mut Lcg(8), 1)[0];
    let with_id = |entry_id: i64| CandidateDesign { entry_id, ..valid };
    let mixed = [
        CandidateDesign {
            width: f64::NAN,
            ..with_id(2)
        },
        valid,
        CandidateDesign {
            volume: f64::INFINITY,
            ..with_id(3)
        },
        CandidateDesign {
            height: 0.0,
            ..with_id(4)
        },
        CandidateDesign {
            length: -1.0,
            ..with_id(5)
        },
    ];
    let rough = cube(12.0);
    let settings = settings_with(3, 0.3, 0.2, 0.5);
    let alone = plan(&rough, &settings, &[valid]);
    assert_ne!(alone.len(), 0);
    assert_eq!(plan(&rough, &settings, &mixed), alone);
}

#[test]
fn a_reversed_width_and_length_plan_like_the_ordered_pair() {
    let reversed = CandidateDesign {
        entry_id: 1,
        width: 3.0,
        length: 2.0,
        height: 1.2,
        volume: 2.0,
    };
    let ordered = CandidateDesign {
        width: 2.0,
        length: 3.0,
        ..reversed
    };
    let rough = cube(12.0);
    let settings = settings_with(3, 0.3, 0.2, 0.5);
    let a = plan(&rough, &settings, &[reversed]);
    assert_ne!(a.len(), 0);
    assert_eq!(a, plan(&rough, &settings, &[ordered]));
}

#[test]
fn no_designs_and_only_invalid_designs_give_an_empty_result() {
    let rough = cube(12.0);
    let settings = settings_with(3, 0.3, 0.2, 0.5);
    let nan = CandidateDesign {
        width: f64::NAN,
        ..box_design(1)
    };
    let flat = CandidateDesign {
        height: 0.0,
        ..box_design(2)
    };
    assert_eq!(
        plan_rough(&rough, &settings, &[], &mut |_| true),
        Some(Vec::new())
    );
    assert_eq!(
        plan_rough(&rough, &settings, &[nan, flat], &mut |_| true),
        Some(Vec::new())
    );
}

#[test]
fn settings_and_extents_are_validated() {
    let good = settings_with(4, 0.3, 0.2, 1.0);
    assert_eq!(good.validate(), Ok(()));
    assert_eq!(
        PlanSettings { count: 0, ..good }.validate(),
        Err(PlanInputError::CountOutOfRange(0))
    );
    assert_eq!(
        PlanSettings { count: 100, ..good }.validate(),
        Err(PlanInputError::CountOutOfRange(100))
    );
    for count in [1, 99] {
        assert_eq!(PlanSettings { count, ..good }.validate(), Ok(()));
    }
    let field_of = |bad: PlanSettings| match bad.validate() {
        Err(PlanInputError::Setting { field, .. }) => field,
        other => panic!("expected a setting error, got {other:?}"),
    };
    for bad in [f64::NAN, f64::INFINITY, -1.0] {
        assert_eq!(
            field_of(PlanSettings {
                kerf_mm: bad,
                ..good
            }),
            "kerf_mm"
        );
        assert_eq!(
            field_of(PlanSettings {
                allowance_mm: bad,
                ..good
            }),
            "allowance_mm"
        );
        assert_eq!(
            field_of(PlanSettings {
                skin_mm: bad,
                ..good
            }),
            "skin_mm"
        );
        assert_eq!(
            field_of(PlanSettings {
                specific_gravity: bad,
                ..good
            }),
            "specific_gravity"
        );
    }
    for bad in [f64::NAN, f64::INFINITY, -1.0, 0.0] {
        assert_eq!(
            field_of(PlanSettings {
                min_width_mm: bad,
                ..good
            }),
            "min_width_mm"
        );
    }
    // Zero kerf, allowance and skin are fine.
    assert_eq!(settings_with(2, 0.0, 0.0, 0.01).validate(), Ok(()));

    assert_eq!(cube(5.0).validate(), Ok(()));
    let thin = RoughBlock {
        x_mm: 4.0,
        y_mm: 0.0,
        z_mm: 4.0,
    };
    assert_eq!(
        thin.validate(),
        Err(PlanInputError::Extent {
            axis: Axis::Y,
            value: 0.0
        })
    );
    for bad in [f64::NAN, f64::INFINITY, -2.0] {
        let block = RoughBlock {
            z_mm: bad,
            ..cube(5.0)
        };
        assert!(
            matches!(
                block.validate(),
                Err(PlanInputError::Extent { axis: Axis::Z, .. })
            ),
            "{bad}"
        );
    }
}

#[test]
fn absurd_inputs_return_an_empty_or_finite_result_without_panicking() {
    let rough = cube(10.0);
    let designs = [box_design(1)];
    let good = settings_with(4, 0.3, 0.2, 1.0);
    // Rejected by validation: nothing is cut.
    for bad in [
        PlanSettings {
            kerf_mm: f64::NAN,
            ..good
        },
        PlanSettings {
            kerf_mm: -1.0,
            ..good
        },
        PlanSettings {
            min_width_mm: 0.0,
            ..good
        },
        PlanSettings { count: 0, ..good },
    ] {
        assert_eq!(
            plan_rough(&rough, &bad, &designs, &mut |_| true),
            Some(Vec::new()),
            "{bad:?}"
        );
    }
    let flat_rough = RoughBlock {
        z_mm: f64::NAN,
        ..rough
    };
    assert_eq!(
        plan_rough(&flat_rough, &good, &designs, &mut |_| true),
        Some(Vec::new())
    );
    // Valid but absurd: whatever comes back has finite totals.
    for absurd in [
        PlanSettings {
            kerf_mm: 1e9,
            ..good
        },
        PlanSettings {
            allowance_mm: 1e9,
            ..good
        },
        PlanSettings {
            skin_mm: 100.0,
            ..good
        },
        PlanSettings {
            min_width_mm: 1e9,
            ..good
        },
    ] {
        let layouts = plan_rough(&rough, &absurd, &designs, &mut |_| true).expect("not cancelled");
        assert!(layouts.len() <= FINAL_TOP, "{absurd:?}");
        for layout in &layouts {
            assert!(layout.total_volume_mm3.is_finite(), "{absurd:?}");
            assert!(layout.total_carat.is_finite(), "{absurd:?}");
            assert!(layout.yield_fraction.is_finite(), "{absurd:?}");
        }
    }
}

#[test]
fn the_grid_of_a_thin_slab_at_k99_shrinks_to_45_45_7() {
    // 100 x 100 x 2 mm at K = 99: the ideal grid is [48, 48, 8] (six units per expected
    // piece, clamped to 48 on the long axes; 2 mm of 27.1 mm geometric mean gives 2.2,
    // raised to the floor of 8). Its operation estimate summed over the six orders is
    // 3.06e9 > 3e9 (the orders with the 8-unit axis as stage A cost 2.69e8 each, those with
    // it as stage B 1.40e8 each, those with it as stage C 1.12e9 each), so the grid shrinks
    // once by 5 % (48 -> 45, 8 -> 7) to 2.46e9, which fits.
    let slab = RoughBlock {
        x_mm: 100.0,
        y_mm: 100.0,
        z_mm: 2.0,
    };
    let grid = choose_grid(&slab, &settings_with(99, 0.3, 0.2, 1.0));
    assert_eq!(grid.cells(), [45, 45, 7]);
}

#[test]
fn a_stone_at_the_minimum_width_is_feasible_on_every_path() {
    let up = |x: f64| f64::from_bits(x.to_bits() + 1);
    let down = |x: f64| f64::from_bits(x.to_bits() - 1);
    let norm = Norm::of(&box_design(1));
    let mw = 1.0_f64;

    // The shared value: a box of width mw -+ 1 ULP is feasible, one 1e-9 short is not.
    for width in [down(mw), mw, up(mw)] {
        assert!(
            stone_value(&norm, 0, [width, 9.0, 9.0], mw) > 0.0,
            "{width}"
        );
    }
    assert!(stone_value(&norm, 0, [mw, 9.0, 9.0], up(mw)) > 0.0);
    assert!(stone_value(&norm, 0, [mw, 9.0, 9.0], up(up(mw))) > 0.0);
    assert!(stone_value(&norm, 0, [mw * (1.0 - 1e-9), 9.0, 9.0], mw) < 0.0);
    assert!(stone_value(&norm, 0, [mw, 9.0, 9.0], mw * (1.0 + 1e-9)) < 0.0);

    // The table's pick (best_pick over the front).
    assert!(best_pick(&[norm], [mw, 9.0, 9.0], up(mw)).is_some());
    assert!(best_pick(&[norm], [mw, 9.0, 9.0], mw * (1.0 + 1e-9)).is_none());

    // The uniform pass: one cell of exactly 4 mm holds a 4 mm stone whose minimum width is
    // one ULP above it, and none when the minimum is 1e-9 relative above.
    let edge = 4.0_f64;
    let uniform = |min_width: f64| {
        uniform_layouts(
            &cube(edge),
            &settings_with(1, 0.0, 0.0, min_width),
            &[box_design(1)],
            &mut |_| true,
        )
        .expect("not cancelled")
    };
    assert_eq!(uniform(up(edge)).len(), 1);
    assert_eq!(uniform(edge * (1.0 + 1e-9)).len(), 0);

    // The whole block plan, through the table, the DP and the refinement.
    let planned = |min_width: f64| {
        plan(
            &cube(edge),
            &settings_with(1, 0.0, 0.0, min_width),
            &[box_design(1)],
        )
    };
    assert_eq!(planned(up(edge)).len(), 1);
    assert_eq!(planned(edge * (1.0 + 1e-9)).len(), 0);

    // The refinement's feasibility kink lands where the stone is feasible: a cube design
    // (assignment 0) in a piece 5 mm across, allowance 0.2 mm per side, minimum width 1 mm.
    let frame = KinkFrame {
        ord: [0, 1, 2],
        allowance: 0.2,
        min_width: mw,
    };
    let mut kinks = Vec::new();
    frame.push(&norm, 0, [0.0, 5.0, 5.0], 0, None, &mut kinks);
    // One kink where the other axes bind (4.6 mm) and one where the width reaches the
    // minimum (1.0 mm); both positions lie along stage 1, the moving axis.
    assert_eq!(kinks.len(), 2);
    for x in kinks {
        let usable = [x, 5.0, 5.0].map(|s| 2.0_f64.mul_add(-0.2, s));
        assert!(stone_value(&norm, 0, usable, mw) > 0.0, "kink {x}");
    }
}

#[test]
#[ignore = "the 99-stone DP takes minutes without --release; run with --release -- --ignored"]
fn k99_on_a_thirty_millimetre_cube_is_sorted_capped_valid_and_deterministic() {
    let mut rng = Lcg(99);
    let designs = random_designs(&mut rng, 4);
    let rough = cube(30.0);
    let settings = settings_with(99, 0.3, 0.2, 1.0);
    let first = plan(&rough, &settings, &designs);
    let second = plan(&rough, &settings, &designs);
    assert!(!first.is_empty() && first.len() <= FINAL_TOP);
    for pair in first.windows(2) {
        assert!(pair[0].total_volume_mm3 >= pair[1].total_volume_mm3 * (1.0 - 1e-9));
    }
    let mut per_set = std::collections::BTreeMap::new();
    for layout in &first {
        let set: Vec<i64> = layout.composition().iter().map(|(id, _)| *id).collect();
        *per_set.entry(set).or_insert(0usize) += 1;
        assert!(layout.stone_count() <= 99);
        assert_layout_geometry(layout, &rough, &settings);
    }
    assert!(per_set.values().all(|&n| n <= SAME_SET_CAP), "{per_set:?}");
    assert_eq!(first.len(), second.len());
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(a.total_volume_mm3.to_bits(), b.total_volume_mm3.to_bits());
        assert_eq!(a.total_carat.to_bits(), b.total_carat.to_bits());
        assert_eq!(a, b);
    }
}

/// The eight corners of a unit cube centred on the origin, as a design hull.
fn cube_hull(entry_id: i64) -> DesignHull {
    DesignHull {
        entry_id,
        vertices: (0..8)
            .map(|i| {
                let sign = |bit: usize| if i & bit == 0 { -0.5 } else { 0.5 };
                [sign(1), sign(2), sign(4)]
            })
            .collect(),
        volume: 1.0,
        width: 1.0,
    }
}

#[test]
fn the_plain_block_with_hulls_keeps_every_layout_geometrically_valid() {
    let rough = RoughBlock {
        x_mm: 14.0,
        y_mm: 10.0,
        z_mm: 8.0,
    };
    let settings = PlanSettings {
        skin_mm: 0.4,
        ..settings_with(4, 0.3, 0.2, 1.0)
    };
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: rough.x_mm,
            y_mm: rough.y_mm,
            z_mm: rough.z_mm,
        },
        Vec::new(),
    );
    let designs = [box_design(1)];
    let hulls = [cube_hull(1)];
    let input = PlanInput {
        model: &model,
        settings: &settings,
        designs: &designs,
        hulls: &hulls,
    };
    let planned = plan::plan(&input, &mut |_| true).expect("not cancelled");
    assert!(!planned.is_empty() && planned.len() <= FINAL_TOP);
    for layout in &planned {
        assert_layout_geometry(layout, &rough, &settings);
    }

    // The exact fits themselves (they may or may not survive the ranking against the equal
    // sawn cube): built straight from the single-stone fit, each is marked exact and sits
    // inside the region inset by skin and allowance.
    let inset = settings.skin_mm + settings.allowance_mm;
    let region = model.usable_halfspaces(inset).expect("region");
    let coarse = model
        .coarse_usable_halfspaces(inset)
        .expect("coarse region");
    let fits = fit_single_stones(&region, &coarse, &hulls, &settings, 3, &mut |_| true)
        .expect("not cancelled");
    assert!(
        !fits.is_empty(),
        "a 12.8 x 8.8 x 6.8 mm region holds a cube"
    );
    let exact = single_fit_layouts(&fits, &hulls, 3, rough.volume_mm3());
    assert_eq!(exact.len(), fits.len());
    for layout in &exact {
        assert!(layout.exact_fit);
        assert_layout_geometry(layout, &rough, &settings);
    }
    // Sawn layouts are never marked exact.
    let sawn = plan(&rough, &settings, &designs);
    assert!(sawn.iter().all(|l| !l.exact_fit));
}

#[test]
fn a_missing_hull_does_not_use_up_a_single_fit_place() {
    // Three fits, the first without a hull: asking for two layouts still gives two.
    let hulls = [cube_hull(2), cube_hull(3)];
    let pose = StonePose {
        center_mm: [5.0, 5.0, 5.0],
        axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        mm_per_unit: 2.0,
    };
    let fit = |entry_id: i64| SingleFit {
        entry_id,
        pose,
        volume_mm3: 8.0,
        carat: 0.04,
    };
    let fits = [fit(1), fit(2), fit(3)];
    let layouts = single_fit_layouts(&fits, &hulls, 2, 1000.0);
    let ids: Vec<i64> = layouts.iter().map(|l| l.stones[0].entry_id).collect();
    assert_eq!(ids, vec![2, 3]);
}
