//! Shared real `.asc` fixtures for [`super`]'s topic modules, and
//! [`design_with_real_meet_structure`], the helper that turns one of them into
//! a real [`Design`] with GENUINE meet-derived structure (as opposed to
//! [`Design::from_asc_schedule`]'s all-pinned import).

use crate::{
    design::{ConstraintTier, Design, ScheduleMeta},
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::{
    Block, MeetConstraint, classify_blocks, meet_tier_inputs_from_asc,
};

/// Builds a real `Design` with GENUINE meet-derived structure --
/// `MeetExisting`/`MeetNamed` tiers exactly as the file's own `G`-field text
/// classifies them, with one bootstrapped [`MeetConstraint::ScaleReference`] per
/// crown/pavilion/girdle block (that block's own first tier's real recorded mast,
/// when the file stated no explicit anchor) -- the same technique
/// `worst_meet_derived_rel_err` uses, reimplemented here against
/// `Design`/`ConstraintTier` directly because the tests using this need a real,
/// edit-and-undo-able [`Design`], not a rederived mast list.
///
/// This is deliberately NOT what [`Design::from_asc_schedule`] produces (that pins
/// every tier to a `ScaleReference`): [`crate::resolve::affected_tiers`] treats
/// every `ScaleReference` tier as a root and everything else as affected, so a
/// subgraph-resolve equivalence test run against an all-pinned import would
/// exercise nothing but roots.
pub(super) fn design_with_real_meet_structure(name: &str, text: &str) -> Design {
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

/// The five real `.asc` fixtures `discard_resolve_gate::discard_and_resolve_gate_on_real_fixtures`
/// checks, pulled out of `mod tests` proper so their combined ~90 lines of
/// embedded source text don't count against that test's own
/// `clippy::too_many_lines` budget.
pub(super) mod real_fixtures {
    /// Not invented here: the exact bar
    /// `crates/indicatrix/examples/meet_solver_validation/main.rs` itself reports
    /// against ("every meet-derived tier within 10%", both in its per-design
    /// success section and in `print_verified_extras`).
    pub(in crate::design::tests) const TOLERANCE: f64 = 0.10;

    /// (name, source text, expected to pass at [`TOLERANCE`]) -- the
    /// expectation is exactly what was measured when this test was written;
    /// see `discard_resolve_gate::discard_and_resolve_gate_on_real_fixtures`'s
    /// doc comment.
    pub(in crate::design::tests) const ALL: [(&str, &str, bool); 5] = [
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
    // corpus's dominant, hardest case (`MeetExisting` tiers carry
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

/// The one large (103-tier) real fixture `resolve_dirty_speed_on_a_large_real_design`
/// measures against, pulled out on its own so its ~110 embedded lines don't count
/// against that test's `clippy::too_many_lines` budget.
pub(super) mod large_fixture {
    // "PC 05.115 CrackOtto-Step" by Ottorino Invernizzi -- pulled read-only from
    // the user's own `facet_diagrams.sqlite` catalogue's `attached_files` table
    // (the real `.asc` file GemCAD would open), embedded below so this test needs
    // no external file, no database, and no network. The design behind the
    // 5.9-second full-solve measurement above.
    pub(in crate::design::tests) const PC_05_115_CRACKOTTO_STEP: &str = "GemCad 5.0\n\
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
