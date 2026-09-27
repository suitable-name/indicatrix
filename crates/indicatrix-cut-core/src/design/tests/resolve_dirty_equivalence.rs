//! The core `resolve_dirty`-vs-full-`solve` acceptance gate: on real,
//! genuinely meet-derived designs, an edited tier's subgraph resolve must
//! reproduce a full solve bit for bit, and every untouched anchor tier must
//! stay completely untouched.

use super::fixtures::{design_with_real_meet_structure, real_fixtures};
use crate::{
    design::{ConstraintTier, Design},
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// The core acceptance gate: for EVERY tier of EVERY one of the five small real
/// fixtures ([`super::fixtures::real_fixtures`], 51 edits total: 2+8+9+12+20
/// tiers), perturbing that tier's angle by a real, non-trivial amount and
/// re-solving via [`crate::design::Design::resolve_dirty`] must produce EXACTLY
/// the same masts -- `f64::to_bits()` equality, not a tolerance -- as a full
/// [`crate::design::Design::solve`] on the identically-edited design, or (if one
/// errors) the exact same `MissingAnchor`.
///
/// Checked against designs built with GENUINE `MeetExisting`/`MeetNamed` structure
/// ([`super::fixtures::design_with_real_meet_structure`]), not pinned import, so
/// [`crate::resolve::affected_tiers`] is actually exercised against non-
/// `ScaleReference` tiers -- an all-`ScaleReference` design would pass trivially.
///
/// Bit-exact equality is not a hopeful tolerance choice: for every tier
/// `affected_tiers` marks unaffected (necessarily a `ScaleReference`), the
/// substitution [`crate::design::Design::resolve_dirty`] performs is a literal
/// no-op, so the substituted input list is bit-for-bit identical to what
/// [`crate::design::Design::solve`] feeds `solve_meet_points` directly. This test is
/// what caught an earlier, narrower affected-tiers rule (file-order/named-reference
/// edges only) failing on three of these five fixtures by up to 3.8% relative --
/// see `crate::resolve`'s module doc comment ("Rejected: a per-tier dependency
/// graph").
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
        original_notes: None,
        detached: Vec::new(),
    });
    design.tiers.push(ConstraintTier {
        angle_deg: 45.0,
        name: "B".to_string(),
        indices: vec![0.0, 24.0, 48.0, 72.0],
        constraint: MeetConstraint::MeetNamed(vec!["A".to_string()]),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });
    design.tiers.push(ConstraintTier {
        angle_deg: -30.0,
        name: "C".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(0.4),
        imported_meet: None,
        original_notes: None,
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
