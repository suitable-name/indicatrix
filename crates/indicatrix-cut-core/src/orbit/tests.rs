use super::*;
use crate::{
    design::{ConstraintTier, Design, ScheduleMeta},
    edit::{Edit, History},
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

fn meta(gear_teeth: i32, symmetry_order: u32, mirror: bool) -> ScheduleMeta {
    ScheduleMeta {
        gemcad_version: "5.0".to_string(),
        gear_teeth,
        symmetry_order,
        mirror,
        refractive_index: 1.62,
        ..ScheduleMeta::default()
    }
}

fn tier_with(indices: &[f64], detached: &[f64]) -> ConstraintTier {
    ConstraintTier {
        angle_deg: -41.0,
        name: "P1".to_string(),
        indices: indices.to_vec(),
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: detached.to_vec(),
    }
}

/// A tier holding exactly one full orbit (`sym=4`, no mirror, gear=96)
/// must decompose into a single complete unit -- the 79.0%-of-corpus
/// `exact` case.
#[test]
fn one_complete_orbit_is_a_single_complete_unit() {
    let m = meta(96, 4, false);
    let units = orbit_units(&[0.0, 24.0, 48.0, 72.0], &m);
    assert_eq!(units.len(), 1);
    assert!(units[0].is_complete());
    assert_eq!(units[0].expected_len, 4);
}

/// Mirroring an off-axis facet must produce TWO residue clusters (the
/// facet and its mirror image), each needing `symmetry_order` members --
/// the correction this module's brief called out explicitly (a naive
/// single-cluster read would misclassify this as broken).
#[test]
fn mirrored_offaxis_facet_folds_two_clusters_into_one_complete_orbit() {
    let m = meta(96, 4, true);
    // base=8 (off-axis: step=24, axis residues are 0 and 12) and its
    // mirror image at 96-8=88, each carrying all 4 rotations.
    let indices = vec![8.0, 32.0, 56.0, 80.0, 88.0, 64.0, 40.0, 16.0];
    let units = orbit_units(&indices, &m);
    assert_eq!(units.len(), 1, "one orbit, folded across its mirror pair");
    assert!(units[0].is_complete());
    assert_eq!(units[0].expected_len, 8);
}

/// A tier folding two distinct, individually-complete orbits under the
/// same stated symmetry (the 9.3%-of-corpus `clean_fold` case) must
/// report two complete units, not "unrelated".
#[test]
fn two_folded_complete_orbits_are_two_complete_units() {
    let m = meta(96, 4, false);
    let indices = vec![
        96.0, 88.0, 80.0, 72.0, 64.0, 56.0, 48.0, 40.0, 32.0, 24.0, 16.0, 8.0,
    ];
    let units = orbit_units(&indices, &m);
    assert_eq!(units.len(), 3);
    assert!(units.iter().all(OrbitUnit::is_complete));
}

/// A half-populated orbit (`partial`) must report exactly that:
/// present, incomplete -- never silently topped up.
#[test]
fn half_populated_orbit_is_incomplete_not_hidden() {
    let m = meta(96, 12, false);
    let units = orbit_units(&[3.0, 19.0, 35.0, 51.0, 67.0, 83.0], &m);
    assert_eq!(units.len(), 1);
    assert!(!units[0].is_complete());
    assert_eq!(units[0].members.len(), 6);
    assert_eq!(units[0].expected_len, 12);
}

/// Removing one occurrence of a complete orbit must remove the whole
/// orbit, not just the clicked occurrence -- Acceptance gate 1
/// (`symmetry_order` stays true, facet count stays coherent).
#[test]
fn removing_one_member_of_a_complete_orbit_removes_the_whole_orbit() {
    let mut design = Design::new(
        PreformSpec::block(1.0, 1.0, 2.0),
        meta(96, 4, false),
        vec![tier_with(&[0.0, 24.0, 48.0, 72.0], &[])],
    );
    let mut history = History::new();
    let edit = design
        .remove_orbit_member(0, 24.0)
        .expect("position 24.0 is present");
    history.apply(&mut design, edit).expect("edit must apply");
    assert_eq!(design.tiers[0].indices, [] as [f64; 0]);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[0].indices, vec![0.0, 24.0, 48.0, 72.0]);
}

/// Adding one occurrence expands to the position's whole orbit, so a
/// brand-new facet is never left half-symmetric.
#[test]
fn adding_one_member_expands_to_its_whole_orbit() {
    let mut design = Design::new(
        PreformSpec::block(1.0, 1.0, 2.0),
        meta(96, 4, false),
        vec![tier_with(&[], &[])],
    );
    let mut history = History::new();
    let edit = design.add_orbit_member(0, 0.0).expect("tier 0 exists");
    history.apply(&mut design, edit).expect("edit must apply");
    assert_eq!(design.tiers[0].indices, vec![0.0, 24.0, 48.0, 72.0]);
}

/// A detached occurrence must survive undo/redo of *other* edits, and
/// removing it must touch only itself -- Acceptance gate 2.
#[test]
fn detached_member_survives_undo_redo_and_is_removed_alone() {
    let mut design = Design::new(
        PreformSpec::block(1.0, 1.0, 2.0),
        meta(96, 4, false),
        vec![tier_with(&[0.0, 24.0, 48.0, 72.0], &[])],
    );
    let mut history = History::new();

    let detach = design
        .detach_orbit_member(0, 24.0)
        .expect("position present");
    history.apply(&mut design, detach).expect("must apply");
    assert_eq!(design.tiers[0].detached, vec![24.0]);

    // An unrelated edit, then undo/redo across it -- detachment must
    // not be an artifact of "nothing else happened since".
    let rename = Edit::ModifyTier {
        index: 0,
        tier: {
            let mut t = design.tiers[0].clone();
            t.name = "P1a".to_string();
            t
        },
    };
    history.apply(&mut design, rename).expect("must apply");
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(
        design.tiers[0].detached,
        vec![24.0],
        "detach survives undo of a later edit"
    );
    assert!(history.redo(&mut design).unwrap());
    assert_eq!(
        design.tiers[0].detached,
        vec![24.0],
        "detach survives redo too"
    );

    let remove = design
        .remove_orbit_member(0, 24.0)
        .expect("position present");
    history.apply(&mut design, remove).expect("must apply");
    assert_eq!(
        design.tiers[0].indices,
        vec![0.0, 48.0, 72.0],
        "detached position removed alone, siblings untouched"
    );
}

/// Removing a NON-detached sibling of an already-detached occurrence
/// must sweep only the attached members, never the detached one. Tier
/// `[0,24,48,72]`, detach 24, remove 48: 24 must survive (it never re-enters the
/// sweep), and since the unit is no longer attached-complete (only 3 of its 4
/// members are attached), 0/72 are not swept either -- only 48 itself is removed.
#[test]
fn removing_a_sibling_of_a_detached_member_never_sweeps_the_detached_one() {
    let mut design = Design::new(
        PreformSpec::block(1.0, 1.0, 2.0),
        meta(96, 4, false),
        vec![tier_with(&[0.0, 24.0, 48.0, 72.0], &[])],
    );
    let mut history = History::new();

    let detach = design
        .detach_orbit_member(0, 24.0)
        .expect("position 24.0 is present");
    history.apply(&mut design, detach).expect("must apply");
    assert_eq!(design.tiers[0].detached, vec![24.0]);

    let remove = design
        .remove_orbit_member(0, 48.0)
        .expect("position 48.0 is present");
    history.apply(&mut design, remove).expect("must apply");
    assert!(
        design.tiers[0].indices.contains(&24.0),
        "the detached position must survive: {:?}",
        design.tiers[0].indices
    );
    assert_eq!(design.tiers[0].indices, vec![0.0, 24.0, 72.0]);
    assert_eq!(design.tiers[0].detached, vec![24.0]);
}

/// Undoing a detach must restore the exact prior `detached` set.
#[test]
fn undoing_detach_restores_attachment() {
    let mut design = Design::new(
        PreformSpec::block(1.0, 1.0, 2.0),
        meta(96, 4, false),
        vec![tier_with(&[0.0, 24.0, 48.0, 72.0], &[])],
    );
    let mut history = History::new();
    let detach = design.detach_orbit_member(0, 24.0).unwrap();
    history.apply(&mut design, detach).unwrap();
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[0].detached, [] as [f64; 0]);
}

/// Acceptance gate 3: import a real `.asc` schedule, complete a partial
/// orbit through the normal `History`-mediated edit path, export back
/// to `.asc` text, and re-import -- the completed orbit (and every
/// solved mast) must come back exactly, not approximately.
#[test]
fn import_orbit_edit_export_reimport_round_trips() {
    let text = "GemCad 5.0\n\
         g 4 0.0\n\
         y 4 n\n\
         I 1.62\n\
         a 90.000000 1.00000000 0 1 2 3 G Set girdle thickness\n\
         a -30.000000 0.60000000 0 1 2 G Set stone size\n";
    let schedule = indicatrix_formats::asc::parse_asc(text).expect("must parse");
    let preform = PreformSpec::block(2.0, 1.0, 2.0);
    let mut design = Design::from_asc_schedule(preform, &schedule);
    // Both tiers stated an explicit anchor, so this is fully solved
    // before any orbit edit -- the partial orbit (indices `0 1 2`
    // against `sym=4`'s expected 4) is a data fact, not a solver gap.
    assert_eq!(
        design.orbit_units(1).unwrap()[0].members.len(),
        3,
        "fixture must start with a partial (3-of-4) orbit"
    );

    let mut history = History::new();
    let edit = design.add_orbit_member(1, 3.0).expect("tier 1 exists");
    history.apply(&mut design, edit).expect("must apply");
    assert_eq!(design.tiers[1].indices, vec![0.0, 1.0, 2.0, 3.0]);
    assert_eq!(
        design.meta.symmetry_order, 4,
        "gate 1: completing the orbit must never touch symmetry_order"
    );

    let solved_before = design.solve().expect("both tiers are anchored");
    let exported = design.to_asc_schedule().expect("must export");
    let reparsed =
        indicatrix_formats::asc::parse_asc(&indicatrix_formats::asc::to_asc_string(&exported))
            .expect("exported text must reparse");
    let reimported = Design::from_asc_schedule(preform, &reparsed);

    assert_eq!(reimported.tiers[1].indices, design.tiers[1].indices);
    assert_eq!(reimported.meta.symmetry_order, design.meta.symmetry_order);
    assert_eq!(reimported.meta.mirror, design.meta.mirror);
    let solved_after = reimported.solve().expect("both tiers are anchored");
    for (before, after) in solved_before.iter().zip(&solved_after) {
        assert!(
            (before.mast - after.mast).abs() < 1e-9,
            "mast must survive export/reimport exactly (to float rounding)"
        );
    }
}

/// Acceptance gate 4: a design whose tier does NOT form a clean orbit
/// (a `mixed_fold` shape -- two residue clusters, neither complete, the
/// genuinely-incoherent case this module's docs measure at 1.8% of real
/// tiers) must round-trip through import/export UNCHANGED when no
/// orbit edit is ever applied -- nothing in this module runs
/// automatically, so there is nothing to silently "correct" it.
#[test]
fn non_orbit_design_exports_unchanged_without_an_explicit_edit() {
    let text = "GemCad 5.0\n\
         g 8 0.0\n\
         y 4 n\n\
         I 1.62\n\
         a 90.000000 1.00000000 0 1 2 3 4 5 6 7 G Set girdle thickness\n\
         a -30.000000 0.60000000 0 3 G Set stone size\n";
    let schedule = indicatrix_formats::asc::parse_asc(text).expect("must parse");
    let preform = PreformSpec::block(2.0, 1.0, 2.0);
    let design = Design::from_asc_schedule(preform, &schedule);

    // step = gear_teeth_abs/symmetry_order = 8/4 = 2; indices 0 and 3
    // land on residues 0 and 1 respectively -- two different,
    // individually-incomplete units, i.e. genuinely not a clean orbit.
    let units = design.orbit_units(1).unwrap();
    assert_eq!(units.len(), 2);
    assert!(units.iter().all(|u| !u.is_complete()));

    let exported = design.to_asc_schedule().expect("must export");
    let reparsed =
        indicatrix_formats::asc::parse_asc(&indicatrix_formats::asc::to_asc_string(&exported))
            .expect("exported text must reparse");
    let reimported = Design::from_asc_schedule(preform, &reparsed);

    assert_eq!(
        reimported.tiers[1].indices, design.tiers[1].indices,
        "a non-orbit tier's indices must survive export/reimport exactly, uncorrected"
    );
}

/// The tier-wide detach/reattach convenience (what `indicatrix-cut`'s
/// one "Detach" button per row applies) must touch every occurrence at
/// once and be fully reversible.
#[test]
fn detach_all_in_tier_then_reattach_round_trips() {
    let mut design = Design::new(
        PreformSpec::block(1.0, 1.0, 2.0),
        meta(96, 4, false),
        vec![tier_with(&[0.0, 24.0, 48.0, 72.0], &[])],
    );
    let mut history = History::new();

    let detach = design.detach_all_in_tier(0).expect("tier 0 exists");
    history.apply(&mut design, detach).expect("must apply");
    assert_eq!(design.tiers[0].detached, vec![0.0, 24.0, 48.0, 72.0]);

    let reattach = design.reattach_all_in_tier(0).expect("tier 0 exists");
    history.apply(&mut design, reattach).expect("must apply");
    assert_eq!(design.tiers[0].detached, [] as [f64; 0]);

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[0].detached, vec![0.0, 24.0, 48.0, 72.0]);
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design.tiers[0].detached, [] as [f64; 0]);
}

/// A position landing at (or within float noise of) the gear boundary
/// must be recognised as the same occurrence as one authored exactly at
/// `0` -- `rem_euclid` wraps a geometric `0` around to `gear_teeth_abs -
/// epsilon`, and a linear `(a - b).abs()` comparison would see that as
/// almost `gear_teeth_abs` apart instead of adjacent.
#[test]
fn adding_a_gear_boundary_duplicate_collapses_to_one_member() {
    let mut design = Design::new(
        PreformSpec::block(1.0, 1.0, 2.0),
        meta(96, 1, false),
        vec![tier_with(&[0.0], &[])],
    );
    let mut history = History::new();
    // 96.0 - 1e-9 is, geometrically, the same index-wheel tooth as the
    // already-present 0.0 -- just computed from "the other side" of the
    // gear boundary, the way a rotation sweep's `rem_euclid` would land
    // it.
    let edit = design
        .add_orbit_member(0, 96.0 - 1e-9)
        .expect("tier 0 exists");
    history.apply(&mut design, edit).expect("edit must apply");
    assert_eq!(
        design.tiers[0].indices,
        vec![0.0],
        "the boundary-adjacent duplicate must collapse into the existing member"
    );
}

/// A residue just past the tolerance gap a plain linear comparison
/// leaves open (`step - 2e-3`, between the snap tolerance `1e-3` and the
/// cluster-match tolerance `4e-3`) must still cluster with a residue near `0`.
#[test]
fn residue_near_step_boundary_clusters_with_residue_near_zero() {
    let m = meta(100, 1, false);
    // step = gear_teeth_abs / symmetry_order = 100 / 1 = 100, so these
    // indices already *are* their own residues: 99.998 is `step - 2e-3`,
    // outside the snap tolerance's reach but within the cluster-match
    // tolerance of 0.001.
    let units = orbit_units(&[0.001, 99.998], &m);
    assert_eq!(
        units.len(),
        1,
        "step-2e-3 must cluster with 0.001, not form a second unit"
    );
    assert_eq!(units[0].members, vec![0.001, 99.998]);
}

/// A facet essentially on the mirror axis, but whose residue happens to
/// fall on the high side of the step boundary (wrapping close to `0`
/// rather than sitting at a small positive value), must still be
/// recognised as on-axis -- one complete `symmetry_order`-member unit,
/// not a spurious second mirror-image cluster that can never fill.
#[test]
fn mirror_axis_check_wraps_across_the_step_boundary() {
    let m = meta(96, 4, true);
    // base = 96 - 0.0025: step = 24, so its residue is 96 mod 24 minus
    // 0.0025 wrapped, i.e. step - 2.5e-3 -- inside the gap
    // `(step - 4e-3, step - 1e-3)` a naive comparison would miss, and
    // geometrically on the mirror axis (residue ~= 0 the "other way round").
    let base: f64 = 96.0 - 0.0025;
    let indices = vec![
        base,
        (base + 24.0).rem_euclid(96.0),
        (base + 48.0).rem_euclid(96.0),
        (base + 72.0).rem_euclid(96.0),
    ];
    let units = orbit_units(&indices, &m);
    assert_eq!(
        units.len(),
        1,
        "an on-axis facet must fold to one unit even when its residue wraps near the step boundary"
    );
    assert_eq!(
        units[0].expected_len, 4,
        "on-axis units are not mirror-doubled"
    );
    assert!(units[0].is_complete());
}

/// Out-of-range tier indices are rejected the same way every other
/// `Edit` builder in this crate rejects them.
#[test]
fn out_of_range_tier_is_rejected() {
    let design = Design::new(
        PreformSpec::block(1.0, 1.0, 2.0),
        meta(96, 4, false),
        vec![],
    );
    assert!(design.add_orbit_member(0, 0.0).is_err());
    assert!(design.remove_orbit_member(0, 0.0).is_err());
    assert!(design.detach_orbit_member(0, 0.0).is_err());
    assert!(design.reattach_orbit_member(0, 0.0).is_err());
    assert!(design.detach_all_in_tier(0).is_err());
    assert!(design.reattach_all_in_tier(0).is_err());
    assert!(design.orbit_units(0).is_err());
}

/// `rotate_indices`/`mirror_indices` (the pure functions) must wrap modulo
/// the gear and leave a `gear_teeth_abs == 0` schedule's values untouched.
#[test]
fn rotate_and_mirror_indices_wrap_modulo_the_gear() {
    assert_eq!(rotate_indices(&[0.0, 90.0], 6.0, 96), vec![6.0, 0.0]);
    assert_eq!(mirror_indices(&[0.0, 6.0, 90.0], 96), vec![0.0, 90.0, 6.0]);
    // No usable gear: values pass through unchanged.
    assert_eq!(rotate_indices(&[1.0, 2.0], 6.0, 0), vec![1.0, 2.0]);
    assert_eq!(mirror_indices(&[1.0, 2.0], 0), vec![1.0, 2.0]);
}

/// `Design::rotate_indices` must rotate both `indices` and `detached` by the
/// same `k_teeth`, wrapping and re-sorting, and reject an out-of-range tier.
#[test]
fn design_rotate_indices_rotates_indices_and_detached_together() {
    let design = Design::new(
        PreformSpec::block(1.0, 1.0, 2.0),
        meta(96, 4, false),
        vec![tier_with(&[0.0, 24.0, 48.0, 72.0], &[0.0])],
    );
    let edit = design.rotate_indices(0, 6.0).expect("tier 0 exists");
    let Edit::SetIndices {
        indices, detached, ..
    } = edit
    else {
        panic!("expected SetIndices");
    };
    assert_eq!(indices, vec![6.0, 30.0, 54.0, 78.0]);
    assert_eq!(detached, vec![6.0]);
    assert!(design.rotate_indices(1, 6.0).is_err());
}

/// `Design::mirror_indices` must negate (mod gear) both `indices` and
/// `detached`, and reject an out-of-range tier.
#[test]
fn design_mirror_indices_mirrors_indices_and_detached_together() {
    let design = Design::new(
        PreformSpec::block(1.0, 1.0, 2.0),
        meta(96, 4, false),
        vec![tier_with(&[6.0, 30.0], &[6.0])],
    );
    let edit = design.mirror_indices(0).expect("tier 0 exists");
    let Edit::SetIndices {
        indices, detached, ..
    } = edit
    else {
        panic!("expected SetIndices");
    };
    assert_eq!(indices, vec![66.0, 90.0]);
    assert_eq!(detached, vec![90.0]);
    assert!(design.mirror_indices(1).is_err());
}
