use super::*;
use crate::{
    design::{Design, ScheduleMeta},
    edit::History,
    preform::PreformSpec,
};
use indicatrix::{
    geometry::meet_solver::{Block, MeetConstraint, classify_blocks, meet_tier_inputs_from_asc},
    optics::materials::GemMaterial,
};

/// "RBC-445" (PC 13.156), 12 tiers, real prose `G` instructions ("G TCP"/"G PCP")
/// and named references that partially resolve -- byte-identical to the fixture
/// embedded in `design.rs`'s own `real_fixtures` test module, duplicated here
/// (rather than imported) because that module is a private `#[cfg(test)]` fixture
/// of a different module, not a reusable export. A real, small, genuinely
/// meet-derived design -- see this module's own doc comment's "Cost first" section
/// for why a SMALL meet-derived fixture (not the 103-tier CrackOtto-Step, which is
/// reserved for the cost probe) is the right size for exercising the search itself
/// quickly and repeatedly.
const RBC_445: &str = "GemCad 5.0\ng 96 0.0\ny 3 y\nI 1.54\n\
     H PC 13.156  RBC-445\n\
     H Richard B Conley, Facets, Oct 2013 p10\n\
     a -50.400000 0.61624001 92 n 1 68 60 36 28 4 G TCP\n\
     a -48.900000 0.63552822 88 n 2 72 56 40 24 8 G TCP\n\
     a -90.000000 0.91252100 92 n 3 68 60 36 28 4\n\
     a -90.000000 0.96225045 88 n 4 72 56 40 24 8\n\
     a -47.200000 0.59908304 95 n 5 65 63 33 31 1\n\
     a -43.000000 0.64485570 86 n 6 74 54 42 22 10 G PCP\n\
     a -47.266965 0.63949242 87 n 7 73 55 41 23 9\n\
     a 31.000000 0.62509296 4 n A 28 36 60 68 92\n\
     a 29.000000 0.62477630 8 n B 24 40 56 72 88\n\
     a 28.100000 0.60078994 2 n C 30 34 62 66 94\n\
     a 20.940747 0.56272062 14 n D 18 46 50 78 82\n\
     a 0.000000 0.40674031 96 n E\n";

/// Builds a real [`Design`] with GENUINE meet-derived structure from a raw `.asc`
/// text: every tier keeps its file's own real [`MeetConstraint`] (`MeetExisting`/
/// `MeetNamed`), except that each crown/pavilion/girdle [`Block`] present gets
/// EXACTLY ONE bootstrapped [`MeetConstraint::ScaleReference`] (that block's own
/// first tier's real recorded mast, when the file stated no explicit anchor of its
/// own) -- otherwise [`Design::solve`] could never produce a mast for that block at
/// all (see `design.rs`'s own module doc comment on scale anchoring). Same
/// technique as `design.rs`'s own private `design_with_real_meet_structure` test
/// helper, reimplemented here since that one is private to a different module.
fn design_with_real_meet_structure(text: &str) -> Design {
    let schedule = indicatrix_formats::asc::parse_asc(text).expect("fixture must parse");
    let mut inputs = meet_tier_inputs_from_asc(&schedule);
    let blocks = classify_blocks(&inputs);
    for block in [Block::Crown, Block::Pavilion, Block::Girdle] {
        let anchored = inputs
            .iter()
            .zip(&blocks)
            .any(|(t, &b)| b == block && matches!(t.constraint, MeetConstraint::ScaleReference(_)));
        if anchored {
            continue;
        }
        if let Some(i) = (0..inputs.len()).find(|&i| blocks[i] == block) {
            inputs[i].constraint = MeetConstraint::ScaleReference(schedule.tiers[i].mast);
        }
    }
    let tiers = inputs
        .into_iter()
        .zip(&schedule.tiers)
        .map(|(input, original)| crate::design::ConstraintTier {
            angle_deg: input.angle_deg,
            name: original.name.clone(),
            indices: input.indices,
            constraint: input.constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        })
        .collect();
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gemcad_version: schedule.gemcad_version.clone(),
            gear_teeth: schedule.gear_teeth,
            gear_reference_angle: schedule.gear_reference_angle,
            symmetry_order: schedule.symmetry_order,
            mirror: schedule.mirror,
            refractive_index: schedule.refractive_index,
            headers: schedule.headers.clone(),
            footnotes: schedule.footnotes,
        },
        tiers,
    )
}

fn rbc_445() -> Design {
    design_with_real_meet_structure(RBC_445)
}

// --- ObjectiveWeights::score ---

#[test]
fn score_is_the_normalized_weighted_sum_with_tilt_brilliance_flipped_to_a_loss() {
    let weights = ObjectiveWeights {
        windowing: 1.0,
        extinction: 1.0,
        tilt_brilliance: 1.0,
        yield_weight: 0.0,
    };
    let components = ObjectiveComponents {
        windowing_pct: 10.0,
        extinction_pct: 20.0,
        tilt_brilliance_pct: 70.0, // loss = 30.0
    };
    let score = weights.score(&components);
    assert!((score - 20.0).abs() < 1e-4, "got {score}");
}

#[test]
fn score_falls_back_to_equal_weights_when_all_weights_are_non_positive() {
    let weights = ObjectiveWeights {
        windowing: 0.0,
        extinction: 0.0,
        tilt_brilliance: 0.0,
        yield_weight: 0.0,
    };
    let components = ObjectiveComponents {
        windowing_pct: 30.0,
        extinction_pct: 30.0,
        tilt_brilliance_pct: 70.0,
    };
    let score = weights.score(&components);
    assert!((score - 30.0).abs() < 1e-4, "got {score}");
}

#[test]
fn score_only_depends_on_weight_ratios_not_absolute_scale() {
    let components = ObjectiveComponents {
        windowing_pct: 12.0,
        extinction_pct: 8.0,
        tilt_brilliance_pct: 60.0,
    };
    let a = ObjectiveWeights {
        windowing: 1.0,
        extinction: 2.0,
        tilt_brilliance: 3.0,
        yield_weight: 0.0,
    };
    let b = ObjectiveWeights {
        windowing: 10.0,
        extinction: 20.0,
        tilt_brilliance: 30.0,
        yield_weight: 0.0,
    };
    assert!((a.score(&components) - b.score(&components)).abs() < 1e-4);
}

// --- ObjectiveWeights::score_with_yield ---

/// With the default `yield_weight` (`0.0`), `score_with_yield` must reproduce
/// `score`'s own output bit for bit, REGARDLESS of what `yield_loss_pct` is
/// passed -- a zero-weight term must never perturb the result.
#[test]
fn score_with_yield_at_default_weight_matches_score_bit_for_bit() {
    let weights = ObjectiveWeights::default();
    let components = ObjectiveComponents {
        windowing_pct: 12.0,
        extinction_pct: 8.0,
        tilt_brilliance_pct: 60.0,
    };
    let plain = weights.score(&components);
    for yield_loss_pct in [0.0, 25.0, 100.0] {
        let with_yield = weights.score_with_yield(&components, yield_loss_pct);
        assert_eq!(
            plain.to_bits(),
            with_yield.to_bits(),
            "yield_loss_pct={yield_loss_pct} must not move the score at yield_weight=0.0"
        );
    }
}

/// A non-zero `yield_weight` must actually blend the yield term in: two
/// candidates with identical optical components but different yield loss must
/// score differently once `yield_weight > 0.0`, and the lower-yield-loss
/// candidate must score better (lower).
#[test]
fn score_with_yield_prefers_lower_yield_loss_at_positive_weight() {
    let weights = ObjectiveWeights {
        windowing: 1.0,
        extinction: 1.0,
        tilt_brilliance: 1.0,
        yield_weight: 1.0,
    };
    let components = ObjectiveComponents {
        windowing_pct: 10.0,
        extinction_pct: 10.0,
        tilt_brilliance_pct: 80.0,
    };
    let low_loss = weights.score_with_yield(&components, 10.0);
    let high_loss = weights.score_with_yield(&components, 90.0);
    assert!(
        low_loss < high_loss,
        "low_loss={low_loss} must score better than high_loss={high_loss}"
    );
}

// --- free_tier_indices ---

#[test]
fn free_tier_indices_excludes_scale_reference_and_vertical_tiers() {
    let design = rbc_445();
    let free = free_tier_indices(&design);
    // RBC-445 has real prose G-instructions on two tiers ("G TCP"/"G PCP"), but
    // `meet_tier_inputs_from_asc` classifies those as MeetNamed/MeetExisting (see
    // that function's own doc comment: it is not a stated scale dimension) -- the
    // ONLY ScaleReference tiers are the ones this fixture's own bootstrap loop
    // synthesizes, one per populated block. It also carries two real girdle facets
    // at -90.0, which `free_tier_indices` excludes because no candidate angle for
    // them could ever pass `candidate_angle_is_safe` (see that function's own doc
    // comment).
    for (i, tier) in design.tiers.iter().enumerate() {
        let pinned = matches!(tier.constraint, MeetConstraint::ScaleReference(_));
        let vertical = tier.angle_deg.abs() > candidate::MAX_SAFE_CANDIDATE_ANGLE_DEG;
        assert_eq!(
            free.contains(&i),
            !pinned && !vertical,
            "tier {i} ({} degrees, pinned {pinned}, vertical {vertical}) is on the wrong side \
             of the free/not-free split",
            tier.angle_deg
        );
    }
    assert!(
        !free.is_empty(),
        "the fixture must leave something for the optimizer to move"
    );
}

#[test]
fn a_freshly_imported_design_has_no_free_tiers() {
    let schedule = indicatrix_formats::asc::parse_asc(RBC_445).expect("fixture must parse");
    let imported = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    assert_eq!(free_tier_indices(&imported), Vec::<usize>::new());
}

// --- candidate_angle_is_safe ---

#[test]
fn crossing_zero_from_a_nonzero_angle_is_unsafe() {
    assert!(!candidate::candidate_angle_is_safe(-43.0, 0.5));
    assert!(!candidate::candidate_angle_is_safe(30.0, -0.5));
}

#[test]
fn staying_on_the_same_side_of_zero_is_safe() {
    assert!(candidate::candidate_angle_is_safe(-43.0, -44.0));
    assert!(candidate::candidate_angle_is_safe(30.0, 32.0));
}

#[test]
fn a_tier_authored_at_exactly_zero_may_move_either_direction() {
    assert!(candidate::candidate_angle_is_safe(0.0, 5.0));
    assert!(candidate::candidate_angle_is_safe(0.0, -5.0));
}

/// `-0.0` is the crate's own pavilion-culet marker (see
/// `ConstraintTier::standard_round_brilliant`'s culet), distinct from the side-less
/// `+0.0` a table facet starts at. A `-0.0` origin must stay pinned to the negative
/// side like any other negative angle.
#[test]
fn a_tier_authored_at_negative_zero_stays_pinned_to_the_negative_side() {
    assert!(candidate::candidate_angle_is_safe(-0.0, -2.0));
    assert!(!candidate::candidate_angle_is_safe(-0.0, 2.0));
}

/// A candidate must never land exactly on the girdle plane, from either side.
/// The current check uses `signum` to distinguish between `0.0` and `-0.0`,
/// ensuring that a step landing at zero is always rejected as required.
#[test]
fn a_candidate_landing_exactly_on_zero_is_never_safe() {
    assert!(!candidate::candidate_angle_is_safe(5.0, 0.0));
    assert!(!candidate::candidate_angle_is_safe(-5.0, -0.0));
}

#[test]
fn a_candidate_past_the_vertical_bound_is_unsafe_even_on_the_same_side() {
    // Same sign as the original in both cases -- only the magnitude bound (the
    // near-girdle vertical boundary, not the zero-crossing) should reject these.
    assert!(!candidate::candidate_angle_is_safe(85.0, 89.9));
    assert!(!candidate::candidate_angle_is_safe(-85.0, -89.9));
}

#[test]
fn a_candidate_just_inside_the_vertical_bound_is_safe() {
    assert!(candidate::candidate_angle_is_safe(85.0, 89.0));
    assert!(candidate::candidate_angle_is_safe(-85.0, -89.0));
}

// --- select_candidate_directions / evaluate_survivors ---

#[test]
fn select_candidate_directions_keeps_both_when_both_are_safe() {
    let design = rbc_445();
    // Tier 7 ("A"): angle 31.0 -- +/-1 degree stays on the same side and well clear
    // of the vertical bound, so both directions survive.
    let survivors = candidate::select_candidate_directions(&design, 7, 31.0, 1.0);
    let mut degs: Vec<f64> = survivors.iter().map(|(deg, _)| *deg).collect();
    degs.sort_by(f64::total_cmp);
    assert_eq!(degs, vec![30.0, 32.0]);
}

#[test]
fn select_candidate_directions_keeps_only_the_safe_direction() {
    let design = rbc_445();
    // Tier 0: angle -50.4 -- "+45" (-5.4) stays clear of both bounds, "-45" (-95.4)
    // passes the vertical bound, so only the plus direction survives.
    let survivors = candidate::select_candidate_directions(&design, 0, -50.4, 45.0);
    assert_eq!(survivors.len(), 1);
    assert!((survivors[0].0 - (-5.4)).abs() < 1e-9);
}

#[test]
fn select_candidate_directions_is_empty_when_both_directions_are_unsafe() {
    let design = rbc_445();
    // Tier 0: angle -50.4 -- "+200" (149.6) crosses the zero boundary AND passes the
    // vertical bound; "-200" (-250.4) passes the vertical bound too. Neither survives.
    let survivors = candidate::select_candidate_directions(&design, 0, -50.4, 200.0);
    assert_eq!(survivors.len(), 0);
}

/// The pre-rejected case (both directions unsafe, see
/// `select_candidate_directions_is_empty_when_both_directions_are_unsafe`) must
/// short-circuit to the same `(None, 0)` a fully-evaluated-but-rejected pair would
/// produce -- without ever entering `std::thread::scope`, since there is nothing left
/// to evaluate.
#[test]
fn evaluate_survivors_with_no_survivors_needs_no_thread_scope_and_matches_the_old_result() {
    let material = GemMaterial::diamond();
    let weights = ObjectiveWeights {
        windowing: 1.0,
        extinction: 1.0,
        tilt_brilliance: 1.0,
        yield_weight: 0.0,
    };
    let baseline = candidate::BaselineWarningCounts::default();
    let ctx = candidate::SearchContext {
        material: &material,
        weights: &weights,
        baseline_warnings: &baseline,
    };
    let (best, evaluations) = candidate::evaluate_survivors(Vec::new(), &ctx, f32::MAX);
    assert_eq!(best, None);
    assert_eq!(evaluations, 0);
}

/// A single surviving direction must be solved (evaluations == 1) without needing a
/// second candidate to pair with -- exercises the inline, no-`thread::scope` path.
#[test]
fn evaluate_survivors_with_one_survivor_evaluates_it_inline() {
    let design = rbc_445();
    let survivors = candidate::select_candidate_directions(&design, 0, -50.4, 45.0);
    assert_eq!(
        survivors.len(),
        1,
        "fixture must exercise the one-survivor case"
    );

    let material = GemMaterial::diamond();
    let weights = ObjectiveWeights {
        windowing: 1.0,
        extinction: 1.0,
        tilt_brilliance: 1.0,
        yield_weight: 0.0,
    };
    let baseline = candidate::BaselineWarningCounts::default();
    let ctx = candidate::SearchContext {
        material: &material,
        weights: &weights,
        baseline_warnings: &baseline,
    };
    let (_, evaluations) = candidate::evaluate_survivors(survivors, &ctx, f32::MAX);
    assert_eq!(evaluations, 1);
}

/// Two surviving directions must both be solved (evaluations == 2) via the
/// `std::thread::scope` path -- kept as a regression guard that splitting the
/// function did not change this behavior.
#[test]
fn evaluate_survivors_with_two_survivors_evaluates_both() {
    let design = rbc_445();
    let survivors = candidate::select_candidate_directions(&design, 7, 31.0, 1.0);
    assert_eq!(
        survivors.len(),
        2,
        "fixture must exercise the two-survivor case"
    );

    let material = GemMaterial::diamond();
    let weights = ObjectiveWeights {
        windowing: 1.0,
        extinction: 1.0,
        tilt_brilliance: 1.0,
        yield_weight: 0.0,
    };
    let baseline = candidate::BaselineWarningCounts::default();
    let ctx = candidate::SearchContext {
        material: &material,
        weights: &weights,
        baseline_warnings: &baseline,
    };
    let (_, evaluations) = candidate::evaluate_survivors(survivors, &ctx, f32::MAX);
    assert_eq!(evaluations, 2);
}

// --- seeded_permutation ---

#[test]
fn seeded_permutation_is_a_real_permutation() {
    let mut sorted = search::seeded_permutation(10, 12345);
    sorted.sort_unstable();
    assert_eq!(sorted, (0..10).collect::<Vec<_>>());
}

#[test]
fn seeded_permutation_is_deterministic_for_the_same_seed() {
    assert_eq!(
        search::seeded_permutation(20, 42),
        search::seeded_permutation(20, 42)
    );
}

#[test]
fn seeded_permutation_differs_across_seeds_almost_always() {
    assert_ne!(
        search::seeded_permutation(20, 1),
        search::seeded_permutation(20, 2)
    );
}

// --- SearchStage::to_code / from_code ---

#[test]
fn search_stage_code_round_trips_every_variant() {
    for stage in [
        SearchStage::BaselineFull,
        SearchStage::Coordinate,
        SearchStage::Polish,
        SearchStage::FinalFull,
    ] {
        assert_eq!(SearchStage::from_code(stage.to_code()), stage);
    }
}

#[test]
fn search_stage_from_code_defaults_an_unknown_value_to_coordinate() {
    assert_eq!(SearchStage::from_code(255), SearchStage::Coordinate);
}

// --- inclusive_max_evaluations ---

#[test]
fn inclusive_max_evaluations_adds_the_polish_stage_s_default_cap() {
    let config = OptimizeConfig {
        max_evaluations: 200,
        polish_start_step_deg: Some(0.5),
        polish_max_evaluations: None,
        ..OptimizeConfig::default()
    };
    // `run_polish_stage`'s own default: `3 * free_tier_count + 20`.
    assert_eq!(inclusive_max_evaluations(&config, 10), 200 + 3 * 10 + 20);
}

#[test]
fn inclusive_max_evaluations_excludes_polish_when_disabled() {
    let config = OptimizeConfig {
        max_evaluations: 200,
        polish_start_step_deg: None,
        ..OptimizeConfig::default()
    };
    assert_eq!(inclusive_max_evaluations(&config, 10), 200);
}

#[test]
fn inclusive_max_evaluations_honors_an_explicit_polish_cap() {
    let config = OptimizeConfig {
        max_evaluations: 200,
        polish_start_step_deg: Some(0.5),
        polish_max_evaluations: Some(50),
        ..OptimizeConfig::default()
    };
    assert_eq!(inclusive_max_evaluations(&config, 10), 250);
}

// --- optimize_design: acceptance-gate-shaped tests ---

/// A freshly-imported design (every tier `ScaleReference`) has nothing free to
/// move -- `optimize_design` must report that honestly (zero evaluations, unchanged
/// score) rather than erroring or silently pretending to have searched.
#[test]
fn optimize_design_on_a_freshly_imported_design_spends_no_evaluations() {
    let schedule = indicatrix_formats::asc::parse_asc(RBC_445).expect("fixture must parse");
    let imported = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    let material = GemMaterial::diamond();
    let outcome = optimize_design(
        &imported,
        &material,
        &OptimizeConfig::default(),
        &SearchHooks::default(),
    )
    .expect("a freshly imported, fully-anchored design must solve");
    assert_eq!(outcome.evaluations, 0);
    assert_eq!(outcome.changes.len(), 0);
    assert_eq!(outcome.before, outcome.after);
}

/// `baseline_report`'s [`SearchStage::BaselineFull`]
/// report must fire even when there turns out to be nothing free to search over --
/// otherwise a caller's progress ticker would show nothing at all for the one
/// [`ObjectiveFidelity::Full`] scoring this path still performs. No
/// [`SearchStage::FinalFull`] report follows, since the "nothing free" path never
/// reaches [`build_outcome`].
#[test]
fn optimize_design_reports_baseline_full_even_with_nothing_free_to_search() {
    let schedule = indicatrix_formats::asc::parse_asc(RBC_445).expect("fixture must parse");
    let imported = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    let material = GemMaterial::diamond();
    let reports = std::cell::RefCell::new(Vec::new());
    let on_progress = |evaluations: usize, stage: SearchStage| {
        reports.borrow_mut().push((evaluations, stage));
    };
    let hooks = SearchHooks {
        cancel: None,
        on_progress: Some(&on_progress),
    };
    let outcome = optimize_design(&imported, &material, &OptimizeConfig::default(), &hooks)
        .expect("a freshly imported, fully-anchored design must solve");
    assert_eq!(outcome.evaluations, 0);
    assert_eq!(*reports.borrow(), vec![(0, SearchStage::BaselineFull)]);
}

/// The core acceptance gate: starting from a deliberately PERTURBED (seeded, so
/// deterministic) copy of a real meet-derived design, the optimizer must not make
/// the objective worse, must stay within a small evaluation budget, must never
/// touch a pinned (`ScaleReference`) tier's angle, and must leave the design closed.
///
/// `#[ignore]`d for the same reason as `design.rs`'s own
/// `resolve_dirty_speed_on_a_large_real_design`: this calls
/// [`optimize_design`], which always measures [`ObjectiveFidelity::Full`] (a real
/// 724-sample raytraced sweep) twice for its before/after report, on top of up to
/// `max_evaluations` full [`Design::solve`] calls -- ~4s in `--release`, but well
/// over a minute unoptimized (this crate's own convention, see
/// `design.rs`, is that this class of cost belongs behind `--ignored --release`,
/// not in the default debug test loop `cargo check -p indicatrix-cut-core` iterates against).
/// Run with `cargo test -p indicatrix-cut-core --release --ignored --nocapture
/// optimize_design_never_worsens_the_score_and_never_touches_a_pinned_tier`.
#[test]
#[ignore = "real-fixture timing cost, see doc comment -- run with --release --ignored"]
fn optimize_design_never_worsens_the_score_and_never_touches_a_pinned_tier() {
    let mut design = rbc_445();
    let free = free_tier_indices(&design);
    // Deliberately perturb every free tier by a seeded pseudo-random amount, within
    // a range small enough to stay on the same side of zero for every one of RBC-
    // 445's free tiers (all of which start well clear of 0 degrees) -- see
    // `candidate_angle_is_safe`.
    let mut state = 777u64;
    for &i in &free {
        let r = (search::splitmix64_next(&mut state) % 1000) as f64 / 1000.0; // [0, 1)
        design.tiers[i].angle_deg = (r - 0.5).mul_add(6.0, design.tiers[i].angle_deg); // +/- 3 degrees
    }
    let pinned: Vec<(usize, f64)> = design
        .tiers
        .iter()
        .enumerate()
        .filter(|(_, t)| matches!(t.constraint, MeetConstraint::ScaleReference(_)))
        .map(|(i, t)| (i, t.angle_deg))
        .collect();

    let material = GemMaterial::diamond();
    let config = OptimizeConfig {
        seed: 42,
        max_evaluations: 120,
        ..OptimizeConfig::default()
    };
    let outcome = optimize_design(&design, &material, &config, &SearchHooks::default())
        .expect("perturbed design must still solve");

    // Acceptance-gate reporting (before/after per component, never just a blended
    // score) -- run with --nocapture to see it.
    println!(
        "RBC-445 (perturbed, seed 777): {} evaluations, {} tier(s) changed\n  \
         before: windowing={:.2}% extinction={:.2}% tilt_brilliance={:.2}% | score={:.3}\n  \
         after:  windowing={:.2}% extinction={:.2}% tilt_brilliance={:.2}% | score={:.3}",
        outcome.evaluations,
        outcome.changes.len(),
        outcome.before.windowing_pct,
        outcome.before.extinction_pct,
        outcome.before.tilt_brilliance_pct,
        outcome.before_score,
        outcome.after.windowing_pct,
        outcome.after.extinction_pct,
        outcome.after.tilt_brilliance_pct,
        outcome.after_score,
    );

    assert!(
        outcome.after_score <= outcome.before_score + 1e-4,
        "optimizer must not worsen the score: before={} after={}",
        outcome.before_score,
        outcome.after_score
    );
    // A soft cap, not a hard one: each tier decision evaluates its `+step`/`-step`
    // pair together (see `evaluate_candidate_pair`), so the loop's own
    // `evaluations >= max_evaluations` guard (checked once BEFORE a pair starts,
    // never mid-pair) can let the true count exceed `max_evaluations` by at most
    // one full pair (2) -- see `OptimizeConfig::max_evaluations`'s own doc comment.
    // `outcome.evaluations` also includes the polish stage's own, separately
    // budgeted evaluations (`config.polish_max_evaluations`, defaulting to `3 *
    // free.len() + 20` -- see `OptimizeConfig::polish_max_evaluations`'s own doc
    // comment), which this bound must account for on top of the coordinate stage's.
    let polish_cap = config
        .polish_start_step_deg
        .filter(|&step| step > 0.0)
        .map_or(0, |_| {
            config.polish_max_evaluations.unwrap_or(3 * free.len() + 20)
        });
    assert!(
        outcome.evaluations <= config.max_evaluations + 2 + polish_cap,
        "evaluations {} should not exceed the coordinate budget {} (+2) plus the polish budget {}",
        outcome.evaluations,
        config.max_evaluations,
        polish_cap
    );

    for change in &outcome.changes {
        assert!(
            pinned.iter().all(|&(pi, _)| pi != change.index),
            "optimizer must never change a pinned tier's angle (tier {})",
            change.index
        );
    }

    // Applying the outcome must still close and must not have moved any pinned
    // tier's angle at all.
    let mut history = History::new();
    let mut applied_design = design.clone();
    apply_optimize_outcome(&mut history, &mut applied_design, &outcome)
        .expect("applying the outcome must succeed against the same design it was computed from");
    assert!(applied_design.is_closed());
    for &(pi, angle) in &pinned {
        assert_eq!(applied_design.tiers[pi].angle_deg, angle);
    }
}

/// [`apply_optimize_outcome`] must go through `History` -- an applied optimization
/// is undoable as a single [`crate::edit::Edit::Batch`] step, exactly like any
/// other edit, rather than one `Ctrl+Z` per tier.
#[test]
fn apply_optimize_outcome_is_undoable_through_history() {
    let design = rbc_445();
    let outcome = OptimizeOutcome {
        before: ObjectiveComponents {
            windowing_pct: 10.0,
            extinction_pct: 10.0,
            tilt_brilliance_pct: 80.0,
        },
        before_score: 10.0,
        before_yield_loss_pct: 0.0,
        after: ObjectiveComponents {
            windowing_pct: 5.0,
            extinction_pct: 10.0,
            tilt_brilliance_pct: 80.0,
        },
        after_score: 5.0,
        after_yield_loss_pct: 0.0,
        evaluations: 4,
        changes: vec![AngleChange {
            index: 7, // tier "A", MeetExisting, free
            from_deg: design.tiers[7].angle_deg,
            to_deg: design.tiers[7].angle_deg + 1.0,
        }],
        cancelled: false,
        polish_evaluations: 0,
        polish_improvement: 0.0,
    };
    let mut history = History::new();
    let mut design = design;
    let original_angle = design.tiers[7].angle_deg;
    let applied =
        apply_optimize_outcome(&mut history, &mut design, &outcome).expect("apply must succeed");
    assert_eq!(applied, 1);
    assert!((design.tiers[7].angle_deg - (original_angle + 1.0)).abs() < 1e-9);
    assert!(history.undo(&mut design).unwrap());
    assert!((design.tiers[7].angle_deg - original_angle).abs() < 1e-9);
}

/// An Optimize outcome touching several tiers must undo as
/// ONE `Ctrl+Z`, not one press per changed tier. Applies a two-tier outcome and
/// checks that a single [`History::undo`] restores BOTH angles at once.
#[test]
fn apply_optimize_outcome_multi_tier_change_is_one_undo_step() {
    let design = rbc_445();
    let original_a = design.tiers[7].angle_deg; // tier "A"
    let original_b = design.tiers[8].angle_deg; // tier "B"
    let outcome = OptimizeOutcome {
        before: ObjectiveComponents {
            windowing_pct: 10.0,
            extinction_pct: 10.0,
            tilt_brilliance_pct: 80.0,
        },
        before_score: 10.0,
        before_yield_loss_pct: 0.0,
        after: ObjectiveComponents {
            windowing_pct: 5.0,
            extinction_pct: 10.0,
            tilt_brilliance_pct: 80.0,
        },
        after_score: 5.0,
        after_yield_loss_pct: 0.0,
        evaluations: 8,
        changes: vec![
            AngleChange {
                index: 7,
                from_deg: original_a,
                to_deg: original_a + 1.0,
            },
            AngleChange {
                index: 8,
                from_deg: original_b,
                to_deg: original_b - 0.5,
            },
        ],
        cancelled: false,
        polish_evaluations: 0,
        polish_improvement: 0.0,
    };
    let mut history = History::new();
    let mut design = design;
    let applied =
        apply_optimize_outcome(&mut history, &mut design, &outcome).expect("apply must succeed");
    assert_eq!(applied, 2);
    assert!((design.tiers[7].angle_deg - (original_a + 1.0)).abs() < 1e-9);
    assert!((design.tiers[8].angle_deg - (original_b - 0.5)).abs() < 1e-9);
    // One History entry covers both tiers: a single undo restores both angles.
    assert!(history.undo(&mut design).unwrap());
    assert!((design.tiers[7].angle_deg - original_a).abs() < 1e-9);
    assert!((design.tiers[8].angle_deg - original_b).abs() < 1e-9);
    // Nothing left to undo -- the batch really was ONE step, not two.
    assert!(!history.undo(&mut design).unwrap());
}

/// An [`AngleChange`] naming a tier index that no longer
/// exists must leave `design` completely untouched -- no partial application
/// of the changes that came before it in the batch.
#[test]
fn apply_optimize_outcome_rejects_out_of_range_index_without_mutating_design() {
    let design = rbc_445();
    let original_a = design.tiers[7].angle_deg;
    let tier_count = design.tiers.len();
    let outcome = OptimizeOutcome {
        before: ObjectiveComponents {
            windowing_pct: 10.0,
            extinction_pct: 10.0,
            tilt_brilliance_pct: 80.0,
        },
        before_score: 10.0,
        before_yield_loss_pct: 0.0,
        after: ObjectiveComponents {
            windowing_pct: 5.0,
            extinction_pct: 10.0,
            tilt_brilliance_pct: 80.0,
        },
        after_score: 5.0,
        after_yield_loss_pct: 0.0,
        evaluations: 8,
        changes: vec![
            AngleChange {
                index: 7,
                from_deg: original_a,
                to_deg: original_a + 1.0,
            },
            AngleChange {
                index: tier_count + 5,
                from_deg: 0.0,
                to_deg: 1.0,
            },
        ],
        cancelled: false,
        polish_evaluations: 0,
        polish_improvement: 0.0,
    };
    let mut history = History::new();
    let mut design = design;
    let err = apply_optimize_outcome(&mut history, &mut design, &outcome)
        .expect_err("an out-of-range index must be rejected");
    assert_eq!(err.index, tier_count + 5);
    assert_eq!(err.tier_count, tier_count);
    assert!((design.tiers[7].angle_deg - original_a).abs() < 1e-9);
    assert!(!history.undo(&mut design).unwrap());
}

// --- polish::run_polish -------------------------------------------------
//
// `run_polish` is generic over the scoring closure (see `polish`'s own module doc
// comment for why), so every test in this section exercises it directly against a
// plain synthetic function -- no `Design`, no `solve`, no `#[ignore]` needed.

/// The Nelder-Mead polish stage substantially closes the gap on a diagonal ridge a
/// pure axis-aligned (coordinate) search stalls on -- see the parent module's
/// "Coordinate descent stalls on diagonal ridges" section.
///
/// `f(a, b) = (a - b)^2 + 0.01 * (a + b - c)^2` has its minimum at `a == b == c /
/// 2`; its Hessian is ill-conditioned (eigenvalues roughly 4.02 and 0.02, a ~200:1
/// ratio), so a search that only ever moves one of `a`/`b` at a time makes real but
/// very slow progress along the shallow `a == b` valley -- once its step size
/// floors out at a fixed minimum, whatever residual remains along that valley is
/// where it stalls. `(0.0, 6.0)` below stands in for such a stalled point: clearly
/// not optimal, and not on the ridge line either.
#[test]
fn polish_stage_closes_most_of_the_gap_on_a_diagonal_ridge_from_a_stalled_point() {
    let c = 10.0;
    let f = |p: &[f64]| 0.01f64.mul_add((p[0] + p[1] - c).powi(2), (p[0] - p[1]).powi(2)) as f32;
    let start = [0.0, 6.0];
    let start_score = f(&start);

    // The stand-in stalled point is nowhere near the ridge's minimum.
    assert!(
        start_score > 10.0,
        "fixture must actually be far from optimal, got {start_score}"
    );

    let result = polish::run_polish(&start, start_score, 0.5, 300, 1e-6, &|| false, f);

    assert!(
        result.score < start_score / 10.0,
        "polish must substantially improve on the stalled point: start={start_score} end={}",
        result.score
    );
    let midpoint = f64::midpoint(result.point[0], result.point[1]);
    assert!(
        (result.point[0] - result.point[1]).abs() < 0.5,
        "polish should settle close to the ridge line a == b, got {:?}",
        result.point
    );
    assert!(
        (midpoint - c / 2.0).abs() < 0.5,
        "polish should approach the ridge's true minimum near a == b == c/2, got {:?}",
        result.point
    );
}

#[test]
fn polish_stage_is_deterministic_for_identical_inputs() {
    let c = 10.0;
    let f = |p: &[f64]| 0.01f64.mul_add((p[0] + p[1] - c).powi(2), (p[0] - p[1]).powi(2)) as f32;
    let start = [0.0, 6.0];
    let run = || polish::run_polish(&start, f(&start), 0.5, 100, 1e-6, &|| false, f);
    let a = run();
    let b = run();
    assert_eq!(a.point, b.point);
    assert!((a.score - b.score).abs() < f32::EPSILON);
    assert_eq!(a.evaluations, b.evaluations);
    assert_eq!(a.cancelled, b.cancelled);
}

/// `is_cancelled` is polled at the very top of the main loop, before the initial
/// simplex is ever touched further -- only the one evaluation per dimension spent
/// building that initial simplex should ever run.
#[test]
fn polish_stage_stops_when_cancelled() {
    let f = |p: &[f64]| p[1].mul_add(p[1], p[0] * p[0]) as f32;
    let start = [10.0, 10.0];
    let result = polish::run_polish(&start, f(&start), 1.0, 1000, 1e-9, &|| true, f);
    assert!(result.cancelled);
    assert_eq!(result.evaluations, start.len());
}

/// A point the scoring closure rejects (here, anything outside a synthetic `|a|,
/// |b| <= 5` box, scored `f32::INFINITY` -- exactly how
/// `candidate::build_free_angle_candidate` treats an out-of-bounds simplex point,
/// never clamping it back in) must never end up as `run_polish`'s own reported
/// result, even when the unconstrained minimum lies outside that box.
#[test]
fn polish_stage_never_returns_a_point_the_evaluator_rejected() {
    let f = |p: &[f64]| {
        if p[0].abs() > 5.0 || p[1].abs() > 5.0 {
            f32::INFINITY
        } else {
            (p[1] - 10.0).mul_add(p[1] - 10.0, (p[0] - 10.0).powi(2)) as f32
        }
    };
    let start = [0.0, 0.0];
    let result = polish::run_polish(&start, f(&start), 1.0, 200, 1e-6, &|| false, f);
    assert!(
        result.score.is_finite(),
        "polish must never settle on a rejected (infinite-score) point"
    );
    assert!(
        result.score <= f(&start),
        "polish must not regress on its own starting point"
    );
}

/// [`candidate::build_free_angle_candidate`] -- the real, `Design`-backed bridge
/// [`run_polish_stage`] uses to turn one simplex vertex into a scoreable candidate
/// -- must only ever write `free`'s own tier indices, exactly like every other
/// angle-only edit this module makes (see the parent module's doc comment, "A
/// tier's `angle_deg` is the only field this module ever changes on a free tier").
#[test]
fn build_free_angle_candidate_never_touches_a_tier_outside_free() {
    let design = rbc_445();
    let free = free_tier_indices(&design);
    let reference: Vec<f64> = free.iter().map(|&i| design.tiers[i].angle_deg).collect();
    let angles: Vec<f64> = reference.iter().map(|&a| a + 0.1).collect();
    let candidate = candidate::build_free_angle_candidate(&design, &free, &reference, &angles)
        .expect("a small +0.1 degree nudge must stay within bounds for every free tier");
    for (i, tier) in design.tiers.iter().enumerate() {
        if !free.contains(&i) {
            assert_eq!(
                candidate.tiers[i].angle_deg, tier.angle_deg,
                "tier {i} is not free and must be untouched"
            );
        }
    }
}

/// [`candidate::build_free_angle_candidate`] rejects a candidate outright (returns
/// `None`, rather than clamping it back in bounds) the moment any single free
/// tier's proposed angle is unsafe relative to its own reference angle -- the same
/// zero-crossing/vertical-bound gate [`candidate::candidate_angle_is_safe`] applies
/// to the coordinate stage's own candidates.
#[test]
fn build_free_angle_candidate_rejects_an_out_of_bounds_angle_in_any_position() {
    let design = rbc_445();
    let free = free_tier_indices(&design);
    let reference: Vec<f64> = free.iter().map(|&i| design.tiers[i].angle_deg).collect();
    assert_ne!(
        reference[0], 0.0,
        "fixture must exercise a real zero-crossing, not the always-safe authored-at-zero case"
    );
    // Cross zero on the first free tier only -- every other position keeps its own
    // reference angle unchanged (a safe, zero-delta "move").
    let mut angles = reference.clone();
    angles[0] = -reference[0].signum() * 0.5;
    assert!(
        candidate::build_free_angle_candidate(&design, &free, &reference, &angles).is_none(),
        "a single unsafe axis must reject the whole candidate point"
    );
}

// --- Cost probe: ignored by default, run explicitly --------------------
//
// `cargo test -p indicatrix-cut-core --release --ignored --nocapture cost_probe`
//
// Measures one full objective evaluation (Design::solve + evaluate_objective) end
// to end, at both fidelities, on a small real meet-derived design (RBC-445, 12
// tiers) and the large one `design.rs`'s own benchmark already measured a 5.9s full
// solve against (CrackOtto-Step, 103 tiers) -- see the module doc comment's "Cost
// first" section for the numbers from the run this was written against.
mod cost_probe {
    use super::*;
    use std::time::Instant;

    const CRACKOTTO_STEP: &str = include_str!("../optimize_cost_probe_crackotto_step.asc");

    fn report(name: &str, design: &Design) {
        let material = GemMaterial::diamond();

        let start = Instant::now();
        let solved = design.solve().expect("must solve");
        let solve_time = start.elapsed();

        let planes = design.planes_from_solved(&solved);
        let gpu_planes = objective::to_gpu_planes(&planes);

        let start = Instant::now();
        let _ = evaluate_objective(&gpu_planes, &material, ObjectiveFidelity::Fast);
        let fast_metrics_time = start.elapsed();

        let start = Instant::now();
        let _ = evaluate_objective(&gpu_planes, &material, ObjectiveFidelity::Full);
        let full_metrics_time = start.elapsed();

        println!(
            "{name} ({} tiers): solve={solve_time:?} fast_metrics={fast_metrics_time:?} \
             full_metrics={full_metrics_time:?} | one Fast eval (solve+fast)={:?} | \
             one Full eval (solve+full, e.g. the before/after report)={:?}",
            design.tiers.len(),
            solve_time + fast_metrics_time,
            solve_time + full_metrics_time,
        );
    }

    #[test]
    #[ignore = "timing measurement, not a correctness check -- run with \
                --release --ignored --nocapture, see module doc comment"]
    fn cost_probe_small_real_meet_derived_design() {
        report("RBC-445", &rbc_445());
    }

    #[test]
    #[ignore = "timing measurement, not a correctness check -- run with \
                --release --ignored --nocapture, see module doc comment"]
    fn cost_probe_large_real_meet_derived_design() {
        let design = design_with_real_meet_structure(CRACKOTTO_STEP);
        assert_eq!(
            design.tiers.len(),
            103,
            "fixture must have its real tier count"
        );
        report("CrackOtto-Step", &design);
    }
}
