//! Acceptance-gate tests for [`super::super::optimize_design`] itself, and for
//! [`super::super::apply_optimize_outcome`] turning an [`OptimizeOutcome`] into
//! real, undoable `History` edits.

use super::{
    super::{
        AngleChange, ObjectiveComponents, OptimizeConfig, OptimizeOutcome, SearchHooks,
        SearchStage, apply_optimize_outcome, free_tier_indices, optimize_design, search,
    },
    fixtures::{RBC_445, rbc_445},
};
use crate::{design::Design, edit::History, preform::PreformSpec};
use indicatrix::{geometry::meet_solver::MeetConstraint, optics::materials::GemMaterial};

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

/// `apply_optimize_outcome` must go through `History` -- an applied optimization
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
