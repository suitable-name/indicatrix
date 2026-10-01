//! The discard-and-rederive property, checked at real-fixture scale: how well
//! meet constraints alone recover a design's real recorded masts, and the
//! separate (exact, not tolerance-bound) property that a pinned import
//! reproduces its original masts exactly.

use super::fixtures::real_fixtures;
use crate::{design::Design, preform::PreformSpec};
use indicatrix::geometry::meet_solver::{
    Block, MeetConstraint, MeetTierInput, SolveStrategy, SolvedTier, classify_blocks,
    meet_tier_inputs_from_asc, solve_meet_points,
};

/// Each expected-FAIL fixture's own measured `worst_meet_derived_rel_err`,
/// pinned to within 10% -- the plain pass/fail boolean the gate below
/// checks would not notice the solver getting dramatically WORSE while still
/// (correctly) reporting "FAIL" at the 10% tolerance; this catches that. Values
/// are this test's own measured figures, re-derived, not copied blind from an
/// earlier report -- see [`discard_and_resolve_gate_on_real_fixtures`]'s own
/// doc comment for why a genuine regression should change this number with an
/// explanation, never silence the check.
const EXPECTED_FAIL_WORST_ERR: &[(&str, f64)] = &[
    ("Mini Square Barion #4", 0.6428),
    ("Six Main Hilite LB", 0.2954),
    ("RBC-445", 0.5670),
];

/// The real, corpus-scale version of the discard-and-rederive property: five real
/// `.asc` files pulled from the user's own `facet_diagrams.sqlite` catalogue
/// (embedded in [`super::fixtures::real_fixtures`], so this test needs no external
/// file), covering a spread from the corpus's tier-count distribution (2, 8, 9, 12
/// and 20 tiers).
///
/// Each design's real recorded masts are discarded, `Design::solve` rederives them
/// from angles/indices/constraints alone, and every meet-derived tier's relative
/// error against the file's own real mast is checked against a **10% tolerance**,
/// the same bar `crates/indicatrix/examples/meet_solver_validation/main.rs` reports
/// against.
///
/// A corpus-wide figure (about 10.8% of 2,881 designs over that bar) comes from an
/// external run against a catalogue that is not in this repository; it cannot be
/// reproduced here and is not asserted. The checked-in gate covers only these five
/// fixtures, of which one solves within the bar and three are pinned as documented
/// limitations. The assertions below encode which designs solve cleanly and which
/// do not, at today's solver behavior -- a real regression (in either direction)
/// should change this test, not the tolerance.
#[test]
fn discard_and_resolve_gate_on_real_fixtures() {
    let mut passed = 0;
    for (name, text, expect_pass) in real_fixtures::ALL {
        let worst_err = worst_meet_derived_rel_err(name, text);
        // Octahedron's crown and pavilion are each a single tier, so both
        // bootstrap as their own `ScaleReference` anchor and there is nothing
        // left for the solver to derive -- `None` here, a vacuous "PASS" that
        // proves nothing about the solver, documented rather than silently
        // folded into a real pass the way the old `f64::max`-fold accumulator
        // did (it also silently dropped a `NaN` mast the same way).
        let Some(worst_err) = worst_err else {
            assert_eq!(
                name, "Octahedron",
                "{name}: zero meet-derived tiers -- every tier bootstrapped as its own anchor, \
                 so this fixture tests nothing; pick a different fixture or explain why zero is \
                 expected here"
            );
            println!("{name}: zero meet-derived tiers -- vacuous PASS, tests nothing");
            assert!(
                expect_pass,
                "{name}: a vacuous case must be marked expect_pass"
            );
            passed += 1;
            continue;
        };
        let this_passes = worst_err <= real_fixtures::TOLERANCE;
        println!(
            "{name}: worst meet-derived tier rel. err {worst_err:.4} ({})",
            if this_passes { "PASS" } else { "FAIL" }
        );
        assert_eq!(
            this_passes,
            expect_pass,
            "{name}: expected {} at {} tolerance, got worst rel. err {worst_err:.4} -- the \
             solver's real-design behavior changed; update the expectation only with a \
             genuine explanation of why, never to silence this",
            if expect_pass { "PASS" } else { "FAIL" },
            real_fixtures::TOLERANCE,
        );
        if !expect_pass {
            let &(_, expected) = EXPECTED_FAIL_WORST_ERR
                .iter()
                .find(|&&(n, _)| n == name)
                .unwrap_or_else(|| {
                    panic!("{name}: no pinned expected worst_err for a FAIL fixture")
                });
            let rel_diff = (worst_err - expected).abs() / expected.abs().max(1e-6);
            assert!(
                rel_diff <= 0.10,
                "{name}: worst_err {worst_err:.4} drifted more than 10% from the pinned \
                 {expected:.4} -- a real regression (in either direction), not a reason to loosen \
                 this bound"
            );
        }
        passed += usize::from(this_passes);
    }

    println!(
        "\ndiscard-and-resolve gate: {passed}/{} real fixtures pass at {} tolerance ({:.0}%) \
         -- corpus-wide (full 2,881 designs): 10.8%",
        real_fixtures::ALL.len(),
        real_fixtures::TOLERANCE,
        100.0 * passed as f64 / real_fixtures::ALL.len() as f64
    );
}

/// Checked on the same five real fixtures the gate test above uses (a different
/// property of them, not a substitute for it): importing a real `.asc` file via
/// [`crate::design::Design::from_asc_schedule`] and re-solving must reproduce **every**
/// original recorded mast exactly, not within a tolerance -- every tier is pinned as
/// a [`MeetConstraint::ScaleReference`], so `Design::solve` never touches geometry
/// for any of them; the only float slop possible is a `String`/parse round trip,
/// hence the tight `1e-9` bound rather than the gate test's 10% one.
#[test]
fn importing_pins_every_tier_and_resolving_reproduces_the_original_masts_exactly() {
    for (name, text) in real_fixtures::ALL
        .iter()
        .map(|&(name, text, _)| (name, text))
    {
        let schedule = indicatrix_formats::asc::parse_asc(text)
            .unwrap_or_else(|e| panic!("{name}: must parse: {e}"));
        let design = Design::from_asc_schedule(PreformSpec::block(1.0, 1.0, 1.0), &schedule);
        assert!(
            design
                .tiers
                .iter()
                .all(|t| matches!(t.constraint, MeetConstraint::ScaleReference(_))),
            "{name}: import must pin every tier, no exceptions"
        );
        let solved = design
            .solve()
            .unwrap_or_else(|e| panic!("{name}: every tier is its own anchor: {e}"));
        assert_eq!(
            solved.len(),
            schedule.tiers.len(),
            "{name}: tier count must match"
        );
        for (i, (solved, original)) in solved.iter().zip(&schedule.tiers).enumerate() {
            assert!(
                (solved.mast - original.mast).abs() < 1e-9,
                "{name}: tier {i} mast {} != original {} (exact reproduction required)",
                solved.mast,
                original.mast
            );
            assert_eq!(solved.strategy, SolveStrategy::ScaleReference);
        }
    }
}

/// Parses `text` and reproduces `Design::from_asc_schedule`'s OLD discard-and-rederive
/// anchoring **directly against `indicatrix`'s `meet_solver` API**, not through
/// `Design::from_asc_schedule` itself: one `ScaleReference` anchor per
/// crown/pavilion/girdle block, bootstrapped from that block's own first tier's real
/// recorded mast whenever the file stated no explicit anchor of its own, with every
/// other tier's real mast discarded and re-derived by `solve_meet_points` from
/// constraints alone.
///
/// This is deliberately **not** what `Design::from_asc_schedule` does any more --
/// real import now pins every tier's mast exactly. This helper exists solely so
/// [`discard_and_resolve_gate_on_real_fixtures`] keeps measuring the same "how well
/// do meet constraints alone recover a design's real masts" property
/// `crates/indicatrix/examples/meet_solver_validation/main.rs` measures at corpus scale,
/// independent of what the editor's own import path does with the real masts it
/// still has available.
///
/// Returns the worst (max) relative error among the resulting meet-derived tiers
/// against the file's own real recorded masts, or `None` when there are no
/// meet-derived tiers at all (every block bootstrapped as its own anchor --
/// see [`discard_and_resolve_gate_on_real_fixtures`]'s own `Octahedron`
/// handling). `ScaleReference` tiers (the bootstrapped anchors) are excluded --
/// they are the anchor itself, not something the solver derived.
///
/// Asserts every meet-derived mast is finite rather than folding with
/// `f64::max` (which silently drops a `NaN` accumulator input, understating --
/// or entirely hiding -- a real solver blowup as if it were `0.0`, the best
/// possible score).
///
/// # Panics
///
/// If any meet-derived tier's solved mast, or its own relative error, is not
/// finite -- a real solver failure this gate must not silently score as `0.0`.
fn worst_meet_derived_rel_err(name: &str, text: &str) -> Option<f64> {
    let schedule = indicatrix_formats::asc::parse_asc(text)
        .unwrap_or_else(|e| panic!("{name}: must parse: {e}"));
    let mut inputs: Vec<MeetTierInput> = meet_tier_inputs_from_asc(&schedule);
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

    let solved: Vec<SolvedTier> = solve_meet_points(schedule.gear_teeth_abs(), &inputs);

    let mut worst: Option<f64> = None;
    for ((input, solved), original) in inputs.iter().zip(&solved).zip(&schedule.tiers) {
        if matches!(input.constraint, MeetConstraint::ScaleReference(_)) {
            continue;
        }
        assert!(
            solved.mast.is_finite(),
            "{name}: a meet-derived tier's solved mast is not finite ({}) -- a real solver \
             failure, not something an `f64::max` fold should ever hide as `0.0`",
            solved.mast
        );
        let rel_err = (solved.mast - original.mast).abs() / original.mast.abs().max(1e-6);
        assert!(
            rel_err.is_finite(),
            "{name}: a meet-derived tier's relative error is not finite ({rel_err})"
        );
        worst = Some(worst.map_or(rel_err, |w: f64| w.max(rel_err)));
    }
    worst
}
