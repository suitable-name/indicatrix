use super::*;
use crate::{
    edit::{Edit, History},
    material::MaterialSelection,
    preform::{PreformShape, PreformSpec},
};
use indicatrix::geometry::meet_solver::{
    Block, MeetConstraint, MeetTierInput, SolveStrategy, SolvedTier, classify_blocks,
    meet_tier_inputs_from_asc, solve_meet_points,
};

/// A fresh design (preform, no tiers) must already be a closed, positive-
/// volume solid -- the whole point of modeling the preform as a real plane
/// set instead of leaving the viewport empty until the first facet exists.
/// With no tiers at all, no block is even present, so there is nothing for
/// `solve` to complain about missing an anchor for.
#[test]
fn fresh_design_alone_is_closed() {
    let design = Design::fresh(PreformSpec::cylinder(96, 1.0, 1.0, 0.8), 96, 8, 1.54);
    assert_eq!(design.tiers.len(), 0);
    assert!(
        design
            .solve()
            .expect("no blocks, nothing to anchor")
            .is_empty()
    );
    assert!(design.is_closed());
    let metrics = design
        .measure()
        .expect("fresh design must measure")
        .expect("fresh design must be closed");
    assert!(metrics.volume > 0.0);
    // With no facets at all, the design's own planes are exactly the
    // preform's.
    assert_eq!(design.planes().unwrap(), design.preform.planes());
}

/// Adding a single tier with an explicit scale-reference constraint (a
/// real authored dimension, not a meet reference) extends -- never
/// replaces -- the preform's own planes, and the resulting arrangement
/// still closes.
#[test]
fn schedule_planes_extend_the_preform() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(ConstraintTier {
        angle_deg: 0.0,
        name: "T".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(0.32),
        imported_meet: None,
        detached: Vec::new(),
    });
    let planes = design.planes().expect("a single anchored tier must solve");
    assert_eq!(planes.len(), design.preform.planes().len() + 1);
    assert!(
        planes[..design.preform.planes().len()]
            .iter()
            .zip(&design.preform.planes())
            .all(|(a, b)| a == b)
    );
    // A lone table facet, cutting into (not through) an oversized block
    // preform, still leaves a closed solid.
    assert!(design.is_closed());
}

/// `PreformShape` is re-exported for callers building a `PreformSpec`
/// directly (exercised here just to confirm the re-export compiles and
/// matches).
#[test]
fn preform_shape_round_trips_through_design() {
    let design = Design::fresh(PreformSpec::block(0.5, 1.0, 0.4), 48, 2, 1.5);
    assert_eq!(design.preform.shape, PreformShape::Block);
}

/// A schedule with a meet-derived tier and no stated scale reference at
/// all must fail closed (a named `MissingAnchor`), not silently fall back
/// to the solver's own internal default scale.
#[test]
fn solve_reports_the_missing_anchor_block_rather_than_a_silent_default() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(ConstraintTier {
        angle_deg: 30.0,
        name: "C1".to_string(),
        indices: vec![0.0, 24.0, 48.0, 72.0],
        constraint: MeetConstraint::MeetExisting,
        imported_meet: None,
        detached: Vec::new(),
    });
    let err = design.solve().expect_err("crown has no scale reference");
    assert_eq!(err.blocks, vec![Block::Crown]);
    assert!(
        !design.is_closed(),
        "planes()/status() must fail closed too"
    );
}

/// Importing a real `.asc` schedule whose tiers already state explicit
/// scale-reference instructions must classify them as
/// `MeetConstraint::ScaleReference` directly (no synthesized anchor needed), and
/// re-solving must reproduce the original masts.
///
/// Every tier here is a stated anchor, so nothing is actually meet-*derived*
/// (`ScaleReference` copies its given value straight through `Design::solve` with no
/// geometry involved) -- see `discard_and_resolve_gate_on_real_fixtures` below for a
/// version that actually exercises meet-derived tiers on real fixtures.
#[test]
fn importing_an_asc_schedule_with_stated_anchors_round_trips_masts() {
    let schedule = indicatrix_formats::asc::parse_asc(
        "GemCad 5.0\n\
         g 4 0.0\n\
         y 1 n\n\
         I 1.62\n\
         a 90.000000 1.00000000 0 1 2 3 G Set girdle thickness\n\
         a 0.000000 0.60000000 G Set stone size\n\
         a -0.000000 0.55000000 G Set stone size\n",
    )
    .expect("must parse");
    let design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    for tier in &design.tiers {
        assert!(
            matches!(tier.constraint, MeetConstraint::ScaleReference(_)),
            "every tier here states an explicit anchor instruction"
        );
        // A stated scale reference has nothing left to adopt -- `constraint`
        // already reflects it.
        assert_eq!(tier.imported_meet, None);
    }
    let solved = design.solve().expect("every block is anchored");
    for (solved, original) in solved.iter().zip(&schedule.tiers) {
        assert!((solved.mast - original.mast).abs() < 1e-9);
    }
}

/// Importing a schedule with NO stated anchor instruction at all must still
/// pin every tier to its own real recorded mast (see
/// `Design::from_asc_schedule`'s doc comment) -- not `apply_ratio_anchors`'
/// estimate, and not a silent default -- while stashing each tier's
/// classified `MeetExisting` in [`ConstraintTier::imported_meet`] so the
/// editor can still show/offer what the file's geometry implies, even
/// though nothing here is actually meet-*derived* any more.
#[test]
fn importing_an_unanchored_asc_schedule_pins_every_tier_to_its_real_mast() {
    let schedule = indicatrix_formats::asc::parse_asc(
        "GemCad 5.0\n\
         g 4 0.0\n\
         y 1 n\n\
         I 1.62\n\
         a 90.000000 1.00000000 0 1 2 3\n\
         a 0.000000 0.60000000\n\
         a -0.000000 0.55000000\n",
    )
    .expect("must parse");
    let design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    // None of these tiers stated an anchor instruction, yet every one must
    // still be pinned to its own real recorded mast, with the file's
    // implicit "meet existing" classification preserved for one-click
    // adoption rather than silently lost.
    for (tier, original) in design.tiers.iter().zip(&schedule.tiers) {
        match tier.constraint {
            MeetConstraint::ScaleReference(v) => {
                assert!((v - original.mast).abs() < 1e-9);
            }
            ref other => panic!("expected a pinned ScaleReference, got {other:?}"),
        }
        assert_eq!(tier.imported_meet, Some(MeetConstraint::MeetExisting));
    }
    assert!(design.solve().is_ok());
}

/// The real, corpus-scale version of the discard-and-rederive property: five real
/// `.asc` files pulled from the user's own `facet_diagrams.sqlite` catalogue
/// (embedded below, so this test needs no external file), covering a spread from
/// the corpus's tier-count distribution (2, 8, 9, 12 and 20 tiers).
///
/// Each design's real recorded masts are discarded, `Design::solve` rederives them
/// from angles/indices/constraints alone, and every meet-derived tier's relative
/// error against the file's own real mast is checked against a **10% tolerance**,
/// the same bar `crates/indicatrix/examples/meet_solver_validation.rs` reports
/// against.
///
/// Running that harness against the full 2,881-design corpus puts only **312
/// designs (10.8%)** over that bar; this test's own 5-fixture sample lands at 2/5
/// (40%) -- close enough, for n=5, that there is no reason to think these fixtures
/// are cherry-picked. **This is reported, not papered over**: the assertions below
/// encode which of these five designs solve cleanly and which do not, at today's
/// solver behavior -- a real regression (in either direction) should change this
/// test, not the tolerance.
#[test]
fn discard_and_resolve_gate_on_real_fixtures() {
    let mut passed = 0;
    for (name, text, expect_pass) in real_fixtures::ALL {
        let worst_err = worst_meet_derived_rel_err(name, text);
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
        passed += usize::from(this_passes);
    }

    println!(
        "\ndiscard-and-resolve gate: {passed}/{} real fixtures pass at {} tolerance ({:.0}%) \
         -- corpus-wide (Report A, full 2,881 designs): 10.8%",
        real_fixtures::ALL.len(),
        real_fixtures::TOLERANCE,
        100.0 * passed as f64 / real_fixtures::ALL.len() as f64
    );
}

/// Checked on the same five real fixtures the gate test above uses (a different
/// property of them, not a substitute for it): importing a real `.asc` file via
/// [`Design::from_asc_schedule`] and re-solving must reproduce **every** original
/// recorded mast exactly, not within a tolerance -- every tier is pinned as a
/// [`MeetConstraint::ScaleReference`], so `Design::solve` never touches geometry for
/// any of them; the only float slop possible is a `String`/parse round trip, hence
/// the tight `1e-9` bound rather than the gate test's 10% one.
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
/// `crates/indicatrix/examples/meet_solver_validation.rs` measures at corpus scale,
/// independent of what the editor's own import path does with the real masts it
/// still has available.
///
/// Returns the worst (max) relative error among the resulting meet-derived tiers
/// against the file's own real recorded masts. `ScaleReference` tiers (the
/// bootstrapped anchors) are excluded -- they are the anchor itself, not something
/// the solver derived.
fn worst_meet_derived_rel_err(name: &str, text: &str) -> f64 {
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

    inputs
        .iter()
        .zip(&solved)
        .zip(&schedule.tiers)
        .filter(|((input, _), _)| !matches!(input.constraint, MeetConstraint::ScaleReference(_)))
        .map(|((_, solved), original)| {
            (solved.mast - original.mast).abs() / original.mast.abs().max(1e-6)
        })
        .fold(0.0_f64, f64::max)
}

/// The five real `.asc` fixtures [`discard_and_resolve_gate_on_real_fixtures`]
/// checks, pulled out of `mod tests` proper so their combined ~90 lines of
/// embedded source text don't count against that test's own
/// `clippy::too_many_lines` budget.
mod real_fixtures {
    /// Not invented here: the exact bar
    /// `crates/indicatrix/examples/meet_solver_validation.rs` itself reports
    /// against ("every meet-derived tier within 10%", both in its per-design
    /// success section and in `print_verified_extras`).
    pub(super) const TOLERANCE: f64 = 0.10;

    /// (name, source text, expected to pass at [`TOLERANCE`]) -- the
    /// expectation is exactly what was measured when this test was written;
    /// see [`super::discard_and_resolve_gate_on_real_fixtures`]'s doc comment.
    pub(super) const ALL: [(&str, &str, bool); 5] = [
        ("Octahedron", OCTAHEDRON, true),
        ("Mini Square Barion #4", MINI_SQUARE_BARION, false),
        ("Six Main Hilite LB", SIX_MAIN_HILITE, false),
        ("RBC-445", RBC_445, false),
        ("Briolette of India Replica", BRIOLETTE, true),
    ];

    // "Octahedron" (PC 11.020) -- 2 tiers, the simplest possible non-trivial
    // shape: crown and pavilion are each a single facet, related by mirror
    // symmetry, so there is exactly one meet-derived tier per block and no
    // ambiguity about which vertex it must pass through.
    const OCTAHEDRON: &str = "GemCad 4.41\ng 96 48.0\ny 4 n\nI 1.54\n\
         H PC 11.020  Octahedron\n\
         H Steele, Norman W; Seattle F Design, Oct 76\n\
         a 54.74 0.68041 0 72 48 24\n\
         a -54.74 0.68041 0 24 48 72\n";

    // "Mini Square Barion #4" (PC 11.051) -- 8 tiers across crown and
    // pavilion, no stated scale reference or named meet references at all
    // (every tier is implicit `MeetExisting`), which is exactly the
    // corpus's dominant, hardest case (Report A: `MeetExisting` tiers carry
    // the worst median relative error of any `ConstraintKind`).
    const MINI_SQUARE_BARION: &str = "GemCad 4.41\ng 96 0.0\ny 4 y\nI 1.54\n\
         H PC 11.051  Mini Square Barion #4\n\
         H (Watermeyer) Steele, Norman W; Seattle F Design, Dec 87\n\
         H Based on Watermeyer, Basil, FACETS, Sep 87 p10 diagram only\n\
         a -90.00 1.00000 0 72 48 24\n\
         a -68.00 0.86149 0 72 48 24\n\
         a -43.00 0.82799 86 10 62 82 38 58 14 34\n\
         a -41.00 0.85443 90 6 66 78 42 54 18 30\n\
         a -43.50 0.85132 0 72 48 24\n\
         a 47.00 0.88285 0 24 48 72\n\
         a 37.00 0.83439 0 24 48 72\n\
         a 0.00 0.62190 0\n";

    // "Six Main Hilite LB" (PC 01.298) -- 9 tiers, negative gear (mirrored
    // index convention), no stated anchor either.
    const SIX_MAIN_HILITE: &str = "GemCad 4.51\ng -96 48.0\ny 6 y\nI 1.54\n\
         H PC 01.298 Six Main Hilite LB\n\
         H Long, R.H. & Steele, N.W.: Seattle F Design, Nov 83, p3\n\
         a 37.00 0.60182 0 16 32 48 64 80\n\
         a 42.00 0.66340 2 14 18 30 34 46 50 62 66 78 82 94\n\
         a 48.93 0.74746 8 24 40 56 72 88\n\
         a 22.00 0.50989 4 12 20 28 36 44 52 60 68 76 84 92\n\
         a 0.00 0.33119 48\n\
         a -43.00 0.71100 0 80 64 48 32 16\n\
         a -45.50 0.73494 94 82 78 66 62 50 46 34 30 18 14 2\n\
         a -48.37 0.76739 88 72 56 40 24 8\n\
         a -90.00 0.99144 94 88 82 78 72 66 62 56 50 46 40 34 30 24 18 14 8 2\n";

    // "RBC-445" (PC 13.156) -- 12 tiers with real prose `G` instructions
    // ("G TCP" / "G PCP", i.e. "table center point"/"pavilion center
    // point", not a stated scale dimension) and named references (`n 1`,
    // `n A`, ...) that partially resolve.
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

    // "Briolette of India Replica" -- 20 tiers, and almost every facet is
    // explicitly NAMED (`n P`, `n 1`, `n 2`, ...), but naming a facet is not the
    // same as this file ever instructing it to meet another named one: there is
    // no `G Meet ...` field anywhere in this raw text, so
    // `meet_tier_inputs_from_asc` classifies every tier here as `MeetExisting`
    // (implicit vertex incidence), not `MeetNamed`.
    const BRIOLETTE: &str = "GemCad 4.56\ng 96 0.0\ny 16 n\nI 2.15\n\
         H Briolette of India Replica\n\
         H by Robert W. Strickland  11/19/96. Based on photo in\n\
         H GIA Diamond Dictionary, 3rd Ed., p. 29.\n\
         H TFG Newsletter, Vol. 17 No. 4, Oct.-Dec. 96, p. 21\n\
         a -90.00 0.48700 9 15 21 27 33 39 45 51 57 63 69 75 81 87 93 3 n P\n\
         a -83.33 0.52267 3 n 1 9 15 21 27 33 39 45 51 57 63 69 75 81 87 93\n\
         a -79.71 0.54850 6 12 18 24 30 36 42 48 54 60 66 72 78 84 90 0 n 2\n\
         a -66.18 0.65694 24 30 36 42 48 54 60 66 72 78 84 90 0 n 3 6 12 18\n\
         a -62.24 0.68964 3 n 4 9 15 21 27 33 39 45 51 57 63 69 75 81 87 93\n\
         a -57.93 0.72861 3 n 5 9 15 21 27 33 39 45 51 57 63 69 75 81 87 93\n\
         a -55.00 0.75845 24 30 36 42 48 54 60 66 72 78 84 90 0 n 6 6 12 18\n\
         a -39.51 0.90361 24 30 36 42 48 54 60 66 72 78 84 90 0 6 n 7 12 18\n\
         a -37.66 0.91970 93 87 81 75 69 63 57 51 45 39 33 27 21 15 9 3 n 8\n\
         a -18.03 1.04662 21 75 9 63 93 51 81 39 69 27 57 15 45 3 n 9 33 87\n\
         a -17.08 1.05050 0 84 72 60 48 36 24 n A 12\n\
         a 86.77 0.47681 6 12 18 24 30 36 42 48 54 60 66 72 78 84 90 0 n a\n\
         a 80.47 0.47085 0 n b 6 12 18 24 30 36 42 48 54 60 66 72 78 84 90\n\
         a 77.27 0.47477 3 n c 9 15 21 27 33 39 45 51 57 63 69 75 81 87 93\n\
         a 72.34 0.49036 3 n d 9 15 21 27 33 39 45 51 57 63 69 75 81 87 93\n\
         a 69.96 0.50432 0 n e 6 12 18 24 30 36 42 48 54 60 66 72 78 84 90\n\
         a 64.62 0.54541 24 30 36 42 48 54 60 66 72 78 84 90 0 n f 6 12 18\n\
         a 62.68 0.56467 9 15 21 27 33 39 45 51 57 63 69 75 81 87 93 3 n g\n\
         a 53.04 0.66763 3 n h 9 15 21 27 33 39 45 51 57 63 69 75 81 87 93\n\
         a 51.38 0.68661 0 12 24 n i 36 48 60 72 84\n";
}

/// Builds a real `Design` with GENUINE meet-derived structure --
/// `MeetExisting`/`MeetNamed` tiers exactly as the file's own `G`-field text
/// classifies them, with one bootstrapped [`MeetConstraint::ScaleReference`] per
/// crown/pavilion/girdle block (that block's own first tier's real recorded mast,
/// when the file stated no explicit anchor) -- the same technique
/// `worst_meet_derived_rel_err` above uses, reimplemented here against
/// `Design`/`ConstraintTier` directly because the tests below need a real,
/// edit-and-undo-able [`Design`], not a rederived mast list.
///
/// This is deliberately NOT what [`Design::from_asc_schedule`] produces (that pins
/// every tier to a `ScaleReference`): [`crate::resolve::affected_tiers`] treats
/// every `ScaleReference` tier as a root and everything else as affected, so a
/// subgraph-resolve equivalence test run against an all-pinned import would
/// exercise nothing but roots.
fn design_with_real_meet_structure(name: &str, text: &str) -> Design {
    let schedule = indicatrix_formats::asc::parse_asc(text)
        .unwrap_or_else(|e| panic!("{name}: must parse: {e}"));
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
        .map(|(input, original)| ConstraintTier {
            angle_deg: input.angle_deg,
            name: original.name.clone(),
            indices: input.indices,
            constraint: input.constraint,
            imported_meet: None,
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

/// The core acceptance gate: for EVERY tier of EVERY one of the five small real
/// fixtures above (51 edits total: 2+8+9+12+20 tiers), perturbing that tier's angle
/// by a real, non-trivial amount and re-solving via [`Design::resolve_dirty`] must
/// produce EXACTLY the same masts -- `f64::to_bits()` equality, not a tolerance --
/// as a full [`Design::solve()`] on the identically-edited design, or (if one
/// errors) the exact same [`MissingAnchor`].
///
/// Checked against designs built with GENUINE `MeetExisting`/`MeetNamed` structure
/// ([`design_with_real_meet_structure`]), not pinned import, so
/// [`crate::resolve::affected_tiers`] is actually exercised against non-
/// `ScaleReference` tiers -- an all-`ScaleReference` design would pass trivially.
///
/// Bit-exact equality is not a hopeful tolerance choice: for every tier
/// `affected_tiers` marks unaffected (necessarily a `ScaleReference`), the
/// substitution [`Design::resolve_dirty`] performs is a literal no-op, so the
/// substituted input list is bit-for-bit identical to what [`Design::solve`] feeds
/// `solve_meet_points` directly. This test is what caught an earlier, narrower
/// affected-tiers rule (file-order/named-reference edges only) failing on three of
/// these five fixtures by up to 3.8% relative -- see `crate::resolve`'s module doc
/// comment ("Rejected: a per-tier dependency graph").
///
/// Split into two `#[test]`s below by fixture size to keep the default (`cargo
/// test`, debug/unoptimized) suite fast: `meet_solver`'s candidate-vertex
/// enumeration is cubic in plane count, and running this check unoptimized for
/// every tier of the two largest fixtures (12 and 20 tiers) took multiple minutes
/// of wall time -- see
/// [`subgraph_resolve_matches_full_solve_on_the_larger_small_fixtures`]'s own doc
/// comment for why it is `#[ignore]`d rather than trimmed down instead.
fn assert_subgraph_resolve_matches_full_solve_for_every_tier(name: &str, text: &str) {
    let design = design_with_real_meet_structure(name, text);
    let baseline = design
        .solve()
        .unwrap_or_else(|e| panic!("{name}: baseline must solve: {e}"));

    for i in 0..design.tiers.len() {
        let mut edited = design.clone();
        // A real, non-trivial perturbation -- not a no-op edit -- so this
        // actually exercises re-solving, not just confirming an
        // unchanged design reproduces itself.
        edited.tiers[i].angle_deg += 0.5;

        let dirty = std::collections::BTreeSet::from([i]);
        let subgraph = edited.resolve_dirty(&baseline, &dirty);
        let full = edited.solve();

        match (&subgraph, &full) {
            (Err(sub_err), Err(full_err)) => {
                assert_eq!(
                    sub_err, full_err,
                    "{name}: tier {i}: both paths failed, but disagreed on why"
                );
            }
            (Ok(_), Err(e)) | (Err(e), Ok(_)) => {
                panic!(
                    "{name}: tier {i}: one path solved and the other didn't ({e}): \
                     subgraph_ok={} full_ok={}",
                    subgraph.is_ok(),
                    full.is_ok()
                );
            }
            (Ok(subgraph), Ok(full)) => {
                assert_eq!(
                    subgraph.len(),
                    full.len(),
                    "{name}: tier {i}: length mismatch"
                );
                for (t, (sub, full)) in subgraph.iter().zip(full).enumerate() {
                    assert_eq!(
                        sub.mast.to_bits(),
                        full.mast.to_bits(),
                        "{name}: editing tier {i} -- tier {t}'s subgraph mast {} != \
                         full-solve mast {} (an \"affected\" tier must use its real, \
                         unsubstituted constraint, byte for byte -- see \
                         crate::resolve's module docs)",
                        sub.mast,
                        full.mast
                    );
                }
            }
        }
    }
}

#[test]
fn subgraph_resolve_matches_full_solve_on_the_smaller_real_fixtures() {
    for (name, text, _) in [
        real_fixtures::ALL[0], // Octahedron, 2 tiers
        real_fixtures::ALL[1], // Mini Square Barion #4, 8 tiers
        real_fixtures::ALL[2], // Six Main Hilite LB, 9 tiers
    ] {
        assert_subgraph_resolve_matches_full_solve_for_every_tier(name, text);
    }
}

/// The same check as
/// [`subgraph_resolve_matches_full_solve_on_the_smaller_real_fixtures`], against the
/// two LARGER real fixtures (RBC-445, 12 tiers; Briolette of India Replica, 20
/// tiers) -- `#[ignore]`d because exhaustively checking every tier of both
/// unoptimized takes multiple minutes (`meet_solver`'s candidate-vertex enumeration
/// is cubic in plane count, and this test forces `2 * (12 + 20) = 64` full/subgraph
/// solve pairs). Run explicitly with `cargo test -p indicatrix-cut-core --release --
/// --ignored subgraph_resolve_matches_full_solve_on_the_larger` (finishes in under
/// 20s in `--release`) whenever [`crate::resolve::affected_tiers`] or
/// `Design::resolve_dirty` changes -- an earlier, narrower affected-tiers rule was
/// caught failing here (RBC-445 tier 7 and Briolette tier 11, both by several
/// percent relative).
#[test]
#[ignore = "exhaustive over 12+20 tiers; multi-minute in an unoptimized \
            build -- run with --release --ignored, see doc comment"]
fn subgraph_resolve_matches_full_solve_on_the_larger_small_fixtures() {
    for (name, text, _) in [
        real_fixtures::ALL[3], // RBC-445, 12 tiers
        real_fixtures::ALL[4], // Briolette of India Replica, 20 tiers
    ] {
        assert_subgraph_resolve_matches_full_solve_for_every_tier(name, text);
    }
}

/// Minimality, the half [`crate::resolve::affected_tiers`]'s wide rule still
/// delivers: every [`MeetConstraint::ScaleReference`] tier OTHER than the one being
/// edited stays completely untouched -- not "close", the same `SolvedTier.mast` bit
/// pattern -- regardless of how many meet-derived tiers exist elsewhere. A
/// hand-built three-tier design (an independent crown anchor `A`, `B` which
/// explicitly names `A` in a `MeetNamed`, and `C`, an unrelated pavilion anchor)
/// makes this checkable directly: editing `A` must leave `C` exactly as it was,
/// even though `B` (not a `ScaleReference`) is also re-solved alongside `A`.
#[test]
fn resolve_dirty_touches_only_the_edited_tier_and_non_anchor_tiers() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(ConstraintTier {
        angle_deg: 30.0,
        name: "A".to_string(),
        indices: vec![0.0, 24.0, 48.0, 72.0],
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        detached: Vec::new(),
    });
    design.tiers.push(ConstraintTier {
        angle_deg: 45.0,
        name: "B".to_string(),
        indices: vec![0.0, 24.0, 48.0, 72.0],
        constraint: MeetConstraint::MeetNamed(vec!["A".to_string()]),
        imported_meet: None,
        detached: Vec::new(),
    });
    design.tiers.push(ConstraintTier {
        angle_deg: -30.0,
        name: "C".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(0.4),
        imported_meet: None,
        detached: Vec::new(),
    });
    let baseline = design.solve().expect("hand-built design must solve");

    let mut edited = design.clone();
    edited.tiers[0].constraint = MeetConstraint::ScaleReference(0.6); // edit A
    let dirty = std::collections::BTreeSet::from([0]);
    let subgraph = edited.resolve_dirty(&baseline, &dirty).expect("must solve");
    let full = edited.solve().expect("must solve");

    // A itself must reflect the edit.
    assert_eq!(subgraph[0].mast, 0.6);
    // C, unrelated to A, must be untouched -- exactly, not approximately.
    assert_eq!(subgraph[2].mast.to_bits(), baseline[2].mast.to_bits());
    // And the whole result must match a full solve of the same edit
    // (this design's own instance of the acceptance gate above).
    for (sub, full) in subgraph.iter().zip(&full) {
        assert_eq!(sub.mast.to_bits(), full.mast.to_bits());
    }
}

/// Speed, on a real, large design: PC 05.115 "CrackOtto-Step", 103 tiers, pulled
/// read-only from the user's own `facet_diagrams.sqlite` catalogue's attached `.asc`
/// file (see [`large_fixture`] for provenance) -- a full [`Design::solve`] against
/// this design measures **5.9 seconds**, which is why the editor has an explicit
/// "Solve" button rather than solving on every keystroke.
///
/// Every one of this design's 103 tiers is implicit `MeetExisting` (no `G`-field
/// text at all) -- the corpus's dominant, hardest case -- so after bootstrapping one
/// [`MeetConstraint::ScaleReference`] anchor per crown/pavilion/girdle block (same
/// technique as [`design_with_real_meet_structure`]), all but 2-3 of the 103 tiers
/// are non-anchor. Per [`crate::resolve::affected_tiers`]'s doc comment, editing ANY
/// tier here therefore affects essentially the whole non-anchor remainder -- this
/// benchmark puts a real number on exactly how much (or little)
/// [`Design::resolve_dirty`] still saves in that regime, not a best case.
///
/// `#[ignore]`d (like the two large fixtures above) so the default suite stays
/// fast; run with `cargo test -p indicatrix-cut-core --release -- --ignored
/// --nocapture resolve_dirty_speed_on_a_large_real_design` for the numbers.
#[test]
#[ignore = "timing measurement, not a correctness check -- run with \
            --release --ignored --nocapture, see doc comment"]
fn resolve_dirty_speed_on_a_large_real_design() {
    let design =
        design_with_real_meet_structure("CrackOtto-Step", large_fixture::PC_05_115_CRACKOTTO_STEP);
    assert_eq!(
        design.tiers.len(),
        103,
        "fixture must have its real tier count"
    );

    let baseline = design.solve().expect("must solve to get a starting point");

    let time = |f: &dyn Fn() -> Vec<indicatrix::geometry::meet_solver::SolvedTier>| {
        let start = std::time::Instant::now();
        let result = f();
        (result, start.elapsed())
    };

    let (_, full_time) = time(&|| design.solve().expect("full solve"));

    // Deliberately avoids tiers 0-12 (exactly +-90 degrees, the "G1".."G13" girdle
    // facets) and tier 50 (exactly 0 degrees, "Table"): a +0.5 edit there
    // reclassifies the tier's crown/pavilion/girdle block entirely, which can
    // remove the very anchor this benchmark's fixture was bootstrapped with --
    // not the case this benchmark exists to time. 20, 60 and 95 sit safely inside
    // one block each (54.6, -70.0, -43.4 degrees).
    for &tier_index in &[20usize, 60, 95] {
        let mut edited = design.clone();
        edited.tiers[tier_index].angle_deg += 0.5;
        let dirty = std::collections::BTreeSet::from([tier_index]);
        let (subgraph_result, subgraph_time) = time(&|| {
            edited
                .resolve_dirty(&baseline, &dirty)
                .expect("subgraph resolve")
        });
        let (full_result, edited_full_time) = time(&|| edited.solve().expect("full solve"));

        for (sub, full) in subgraph_result.iter().zip(&full_result) {
            assert_eq!(sub.mast.to_bits(), full.mast.to_bits());
        }

        println!(
            "CrackOtto-Step (103 tiers): edit tier {tier_index}: full solve \
             {edited_full_time:?}, subgraph resolve {subgraph_time:?} \
             ({:.1}x)",
            edited_full_time.as_secs_f64() / subgraph_time.as_secs_f64().max(1e-12)
        );
    }
    println!("CrackOtto-Step (103 tiers): baseline full solve (unedited): {full_time:?}");

    // The other half of the honest picture (see `crate::resolve`'s module
    // docs, "What this still buys"): the SAME 103 planes, but imported
    // the normal way a user would actually open this file in the editor --
    // `Design::from_asc_schedule` pins every tier's constraint to
    // `ScaleReference` at its own real recorded mast, so immediately after
    // loading, every tier except the one being edited is structurally immune.
    let schedule = indicatrix_formats::asc::parse_asc(large_fixture::PC_05_115_CRACKOTTO_STEP)
        .expect("fixture must parse");
    let imported = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    assert!(
        imported
            .tiers
            .iter()
            .all(|t| matches!(t.constraint, MeetConstraint::ScaleReference(_))),
        "import must pin every tier -- see Design::from_asc_schedule's doc comment"
    );
    let imported_baseline = imported.solve().expect("every tier is its own anchor");

    let mut edited_import = imported.clone();
    edited_import.tiers[20].angle_deg += 0.5;
    let dirty = std::collections::BTreeSet::from([20]);
    let (subgraph_result, import_subgraph_time) = time(&|| {
        edited_import
            .resolve_dirty(&imported_baseline, &dirty)
            .expect("subgraph resolve")
    });
    let (full_result, import_full_time) = time(&|| edited_import.solve().expect("full solve"));
    for (sub, full) in subgraph_result.iter().zip(&full_result) {
        assert_eq!(sub.mast.to_bits(), full.mast.to_bits());
    }
    println!(
        "CrackOtto-Step (103 tiers), FRESHLY IMPORTED (every tier pinned): edit tier 20: \
         full solve {import_full_time:?}, subgraph resolve {import_subgraph_time:?} \
         ({:.0}x -- both cheap: an all-ScaleReference solve never enters \
         meet_solver's expensive candidate search at all, see below)",
        import_full_time.as_secs_f64() / import_subgraph_time.as_secs_f64().max(1e-12)
    );

    // The scenario in between -- and the one this speedup is actually FOR: a
    // partially-adopted design. Half the tiers (the pavilion block, tiers
    // 51-102) get "adopted" back to their real, meet-derived constraint via
    // `design_with_real_meet_structure`'s technique (simulating a user who used
    // `Edit::SetConstraint`'s one-click adoption on that half), while the other
    // half (crown/girdle, tiers 0-50) stays exactly as import pinned it. Editing
    // a STILL-PINNED crown/girdle tier should be cheap via subgraph resolve (the
    // expensive pavilion remainder is never touched) but expensive via a full
    // solve (which re-derives the pavilion's ~50 meet-derived tiers regardless
    // of what was edited).
    let mut half_adopted = imported;
    let real_pavilion_inputs = meet_tier_inputs_from_asc(&schedule);
    // Tier 51 (the pavilion's first tier) stays exactly as import pinned it, so
    // the pavilion block keeps a real anchor once the rest of it (52-102) is
    // adopted back to genuine `MeetExisting`.
    for i in 52..103 {
        half_adopted.tiers[i].constraint = real_pavilion_inputs[i].constraint.clone();
    }
    let half_adopted_baseline = half_adopted
        .solve()
        .expect("pavilion's own first tier is still that block's ScaleReference anchor");

    let mut edited_half = half_adopted.clone();
    edited_half.tiers[20].angle_deg += 0.5; // still-pinned crown tier
    let dirty = std::collections::BTreeSet::from([20]);
    let (subgraph_result, half_subgraph_time) = time(&|| {
        edited_half
            .resolve_dirty(&half_adopted_baseline, &dirty)
            .expect("subgraph resolve")
    });
    let (full_result, half_full_time) = time(&|| edited_half.solve().expect("full solve"));
    for (sub, full) in subgraph_result.iter().zip(&full_result) {
        assert_eq!(sub.mast.to_bits(), full.mast.to_bits());
    }
    println!(
        "CrackOtto-Step (103 tiers), HALF ADOPTED (pavilion meet-derived, crown/girdle \
         still pinned): edit a still-pinned crown tier (20): full solve \
         {half_full_time:?}, subgraph resolve {half_subgraph_time:?} ({:.1}x)",
        half_full_time.as_secs_f64() / half_subgraph_time.as_secs_f64().max(1e-12)
    );
}

/// The one large (103-tier) real fixture [`resolve_dirty_speed_on_a_large_real_design`]
/// measures against, pulled out on its own so its ~110 embedded lines don't count
/// against that test's `clippy::too_many_lines` budget.
mod large_fixture {
    // "PC 05.115 CrackOtto-Step" by Ottorino Invernizzi -- pulled read-only from
    // the user's own `facet_diagrams.sqlite` catalogue's `attached_files` table
    // (the real `.asc` file GemCAD would open), embedded below so this test needs
    // no external file, no database, and no network. The design behind the
    // 5.9-second full-solve measurement above.
    pub(super) const PC_05_115_CRACKOTTO_STEP: &str = "GemCad 5.0\n\
         g 96 0.0\n\
         y 1 y\n\
         I 1.54\n\
         H PC 05.115  CrackOtto-Step\n\
         H by Ottorino Invernizzi\n\
         H Inspired by Crackerjack profile of Dennis Durham\n\
         H 18-08-2013\n\
         a 90.000000 0.85251694 95 1 n G1\n\
         a 90.000000 0.86751392 92 4 n G2\n\
         a 90.000000 0.88806594 90 6 n G3\n\
         a 90.000000 0.93763232 83 13 n G4\n\
         a 90.000000 0.91583703 78 18 n G5\n\
         a 90.000000 0.82922829 72 24 n G6\n\
         a -90.000000 0.79437697 70 26 n G7\n\
         a -90.000000 0.76895427 68 28 n G8\n\
         a -90.000000 0.75539330 66 30 n G9\n\
         a -90.000000 0.75660908 64 32 n G10\n\
         a -90.000000 0.77606095 62 34 n G11\n\
         a -90.000000 0.81857882 60 36 n G12\n\
         a -90.000000 0.88150371 58 38 n G13\n\
         a 54.556157 0.79319520 1 n A 95\n\
         a 54.556157 0.80541300 4 n B 92\n\
         a 54.556157 0.82215641 6 n C 90\n\
         a 53.941981 0.85814514 13 n D 83\n\
         a 54.556157 0.84478108 18 n E 78\n\
         a 54.556157 0.77422230 24 n F 72\n\
         a 54.556157 0.74582948 26 n G 70\n\
         a 54.556157 0.72511800 28 n H 68\n\
         a 54.556157 0.71407010 30 n I 66\n\
         a 54.556157 0.71506057 32 n J 64\n\
         a 54.556157 0.73090771 34 n K 62\n\
         a 54.556157 0.76554634 36 n L 60\n\
         a 54.301884 0.81514875 38 n M 58\n\
         a 42.000000 0.75534621 1 n N 95\n\
         a 42.000000 0.76538114 4 n O 92\n\
         a 42.000000 0.77913313 6 n P 90\n\
         a 42.616309 0.81328503 13 n Q 83\n\
         a 42.000000 0.79771561 18 n R 78\n\
         a 42.000000 0.73976306 24 n S 72\n\
         a 42.000000 0.71644297 26 n T 70\n\
         a 42.000000 0.69943186 28 n U 68\n\
         a 42.000000 0.69035781 30 n V 66\n\
         a 42.000000 0.69117132 32 n W 64\n\
         a 42.000000 0.70418717 34 n X 62\n\
         a 42.000000 0.73263717 36 n Y 60\n\
         a 34.361611 0.74429226 1 n Z 95\n\
         a 34.361611 0.75275676 4 n aa 92\n\
         a 34.361611 0.76435661 6 n bb 90\n\
         a 34.361611 0.79233257 13 n cc 83\n\
         a 34.361611 0.78003100 18 n ee 78\n\
         a 34.361611 0.73114782 24 n ff 72\n\
         a 34.361611 0.71147724 26 n gg 70\n\
         a 34.361611 0.69712831 28 n hh 68\n\
         a 34.361611 0.68947431 30 n ii 66\n\
         a 34.361611 0.69016051 32 n jj 64\n\
         a 34.361611 0.70113942 34 n kk 62\n\
         a 34.361611 0.72513710 36 n ll 60\n\
         a 0.000000 0.58330110 96 n Table\n\
         a -70.000000 0.75291899 95 n 1 1\n\
         a -70.000000 0.76701154 92 n 2 4\n\
         a -70.000000 0.78632412 90 n 3 6\n\
         a -70.000000 0.83290128 83 n 4 13\n\
         a -70.000000 0.81242041 78 n 5 18\n\
         a -70.000000 0.73103482 72 n 6 24\n\
         a -70.000000 0.69828529 70 n 7 26\n\
         a -70.000000 0.67439576 68 n 8 28\n\
         a -70.000000 0.66165262 66 n 9 30\n\
         a -70.000000 0.66279508 64 n 10 32\n\
         a -70.000000 0.68107386 62 n 11 34\n\
         a -70.000000 0.72102759 60 n 12 36\n\
         a -70.000000 0.78015764 58 n 13 38\n\
         a -61.000000 0.70717749 95 n 14 1\n\
         a -61.000000 0.72029414 92 n 15 4\n\
         a -61.000000 0.73826934 90 n 16 6\n\
         a -61.000000 0.78162108 83 n 17 13\n\
         a -61.000000 0.76255848 78 n 18 18\n\
         a -61.000000 0.68680878 72 n 19 24\n\
         a -61.000000 0.65632712 70 n 20 26\n\
         a -61.000000 0.63409193 68 n 21 28\n\
         a -61.000000 0.62223124 66 n 22 30\n\
         a -61.000000 0.62329459 64 n 23 32\n\
         a -61.000000 0.64030758 62 n 24 34\n\
         a -61.000000 0.67749454 60 n 25 36\n\
         a -61.000000 0.73252989 58 n 26 38\n\
         a -52.000000 0.67564096 95 n 27 1\n\
         a -52.000000 0.68745874 92 n 28 4\n\
         a -52.000000 0.70365395 90 n 29 6\n\
         a -52.000000 0.74271279 83 n 30 13\n\
         a -52.000000 0.72553786 78 n 31 18\n\
         a -52.000000 0.65728925 72 n 32 24\n\
         a -52.000000 0.62982603 70 n 33 26\n\
         a -52.000000 0.60979267 68 n 34 28\n\
         a -52.000000 0.59910648 66 n 35 30\n\
         a -52.000000 0.60006453 64 n 36 32\n\
         a -52.000000 0.61539282 62 n 37 34\n\
         a -52.000000 0.64889735 60 n 38 36\n\
         a -52.000000 0.69848284 58 n 39 38\n\
         a -43.000000 0.65722075 95 n 40 1\n\
         a -43.000000 0.66744867 92 n 41 4\n\
         a -43.000000 0.68146511 90 n 42 6\n\
         a -43.000000 0.71526930 83 n 43 13\n\
         a -42.720496 0.69934396 78 n 44 18\n\
         a -42.245899 0.63927606 72 n 45 24\n\
         a -42.419466 0.61625138 70 n 46 26\n\
         a -42.725477 0.59970507 68 n 47 28\n\
         a -43.000000 0.59098259 66 n 48 30\n\
         a -43.000000 0.59181175 64 n 49 32\n\
         a -43.000000 0.60507789 62 n 50 34\n\
         a -43.000000 0.63407501 60 n 51 36\n\
         a -43.000000 0.67698968 58 n 52 38\n\
         F See preform 05.112\n\
         F For information: ottoinve@alice.it\n";
}

// --- FreshDesignSpec / Design::fresh_from_spec / Design::fresh compatibility ---

/// [`Design::fresh_from_spec`] must build exactly the gear/symmetry/mirror/
/// material the spec names, with no tiers yet -- the same "closed on its own"
/// property [`fresh_design_alone_is_closed`] already checks for
/// [`Design::fresh`].
#[test]
fn fresh_from_spec_builds_the_requested_gear_symmetry_mirror_and_material() {
    let spec = FreshDesignSpec {
        gear_teeth: 80,
        symmetry_order: 8,
        mirror: false,
        material: MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        },
        preform: PreformSpec::block(1.0, 1.0, 2.0),
    };
    let design = Design::fresh_from_spec(spec);

    assert_eq!(design.meta.gear_teeth, 80);
    assert_eq!(design.meta.symmetry_order, 8);
    assert!(!design.meta.mirror);
    assert_eq!(design.material.name.as_deref(), Some("Quartz"));
    assert_eq!(design.tiers, [] as [ConstraintTier; 0]);
    assert!(
        design.is_closed(),
        "a fresh design must already be a closed solid"
    );
}

/// [`Design::fresh`] is documented as a thin wrapper over
/// [`Design::fresh_from_spec`] kept for source compatibility -- this pins its exact
/// behavior: `mirror` always `true`, no material selection, and the passed
/// `refractive_index` landing verbatim on `meta.refractive_index`, not derived from
/// a material.
#[test]
fn fresh_is_a_thin_wrapper_that_matches_its_pre_a2_behavior_exactly() {
    let design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 8, 1.62);
    assert_eq!(design.meta.gear_teeth, 96);
    assert_eq!(design.meta.symmetry_order, 8);
    assert!(design.meta.mirror);
    assert_eq!(design.meta.refractive_index, 1.62);
    assert_eq!(design.material, MaterialSelection::none());
    assert_eq!(design.tiers, [] as [ConstraintTier; 0]);
}

// --- Design::effective_refractive_index ---

#[test]
fn effective_refractive_index_prefers_the_override_over_everything_else() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 8, 1.54);
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: Some(1.70),
    };
    assert_eq!(design.effective_refractive_index(), 1.70);
}

#[test]
fn effective_refractive_index_falls_back_to_the_resolved_material_when_there_is_no_override() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 8, 1.54);
    design.material = MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    let expected = crate::material::built_in_refractive_index("Quartz").unwrap();
    assert!((design.effective_refractive_index() - expected).abs() < 1e-9);
    assert_ne!(
        design.effective_refractive_index(),
        design.meta.refractive_index,
        "the resolved material's own n_D must win over the legacy schedule field"
    );
}

/// With neither an override nor a material name that resolves, the legacy
/// `ScheduleMeta::refractive_index` is what survives -- the untouched-import
/// round-trip case.
#[test]
fn effective_refractive_index_falls_back_to_the_legacy_schedule_value_when_unset() {
    let design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 8, 1.62);
    assert_eq!(design.material, MaterialSelection::none());
    assert_eq!(design.effective_refractive_index(), 1.62);

    let mut unresolved = design;
    unresolved.material = MaterialSelection {
        name: Some("Not A Real Material".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    assert_eq!(unresolved.effective_refractive_index(), 1.62);
}

/// `to_asc_schedule` must write the EFFECTIVE refractive index, not the raw legacy
/// field, exercised end to end through a real solved export.
#[test]
fn to_asc_schedule_writes_the_effective_refractive_index_not_the_legacy_field() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 8, 1.54);
    design.tiers.push(ConstraintTier {
        angle_deg: 0.0,
        name: "T".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        detached: Vec::new(),
    });
    design.material.refractive_index_override = Some(1.90);

    let schedule = design
        .to_asc_schedule()
        .expect("single anchored tier must solve");
    assert_eq!(schedule.refractive_index, 1.90);
    assert_ne!(schedule.refractive_index, design.meta.refractive_index);
}

/// A "golden `.asc` export for a retargeted design": applying a `RetargetAngles`
/// edit and exporting must reflect the NEW angle in the resulting `.asc` schedule
/// for the retargeted tier, while every untouched tier's own export stays
/// byte-identical.
#[test]
fn to_asc_schedule_reflects_a_retargeted_angle() {
    let mut design = Design::fresh(PreformSpec::block(2.0, 1.0, 2.0), 96, 8, 1.54);
    design.tiers.push(ConstraintTier {
        angle_deg: 0.0,
        name: "T".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        detached: Vec::new(),
    });
    design.tiers.push(ConstraintTier {
        angle_deg: -40.0,
        name: "P1".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(0.9),
        imported_meet: None,
        detached: Vec::new(),
    });

    let before_schedule = design
        .to_asc_schedule()
        .expect("must solve before retarget");

    let mut history = History::new();
    history
        .apply(
            &mut design,
            Edit::RetargetAngles {
                changes: vec![(1, -40.0, -45.0)],
            },
        )
        .expect("retarget must apply");

    let after_schedule = design.to_asc_schedule().expect("must solve after retarget");
    assert_eq!(after_schedule.tiers[1].angle_deg, -45.0);
    // The untouched table tier's own export must be completely unaffected.
    assert_eq!(after_schedule.tiers[0], before_schedule.tiers[0]);

    // Undo must restore the exact original schedule, angle included.
    assert!(history.undo(&mut design).unwrap());
    let restored_schedule = design.to_asc_schedule().expect("must solve after undo");
    assert_eq!(restored_schedule, before_schedule);
}
