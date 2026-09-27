//! A tier note surviving a full edit-history sequence and a save/load round
//! trip, and the bounded history trail ([`crate::native::SaveExtras::history_entries`])
//! round-tripping through the sidecar.

use super::fixtures::simple_design;
use crate::{
    design::ConstraintTier,
    edit::{Edit, History},
    native::{SaveExtras, TierOverlay, load_paired, save_paired, save_paired_extended},
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// A note surviving a full `Edit::History` add-tier/remove-tier/move-tier
/// sequence, then a real save/load round trip -- the index-maintenance
/// contract `crate::edit::tests` already exercises against `History` alone,
/// checked here end to end through the native file format too.
///
/// Built on [`super::fixtures::unsolved_design`] (a single `MeetExisting` tier, no
/// scale-reference anchor anywhere) rather than [`super::fixtures::simple_design`]:
/// this test needs the draft save path deterministically, and `unsolved_design` can
/// never solve regardless of how its tiers get shuffled, while whether
/// `simple_design`'s own `MeetExisting` tier happens to solve after a reorder
/// is exactly the kind of thing that could flip out from under this test.
#[test]
fn tier_notes_survive_add_remove_and_reorder_then_a_save_load_round_trip() {
    let mut design = super::fixtures::unsolved_design();
    design.tiers.push(tier_named("B"));
    design.tiers.push(tier_named("C"));
    let mut history = History::new();

    // Tier 1 ("B") gets a note.
    history
        .apply(
            &mut design,
            Edit::SetTierNote {
                index: 1,
                note: Some("grind slowly".to_string()),
            },
        )
        .expect("set tier note must apply");

    // Insert a new tier before it: the note must follow to index 2.
    history
        .apply(
            &mut design,
            Edit::AddTier {
                index: 0,
                tier: tier_named("Z"),
            },
        )
        .expect("add must apply");
    assert_eq!(design.tier_note(1), None);
    assert_eq!(design.tier_note(2), Some("grind slowly"));

    // Remove tier 0 (the one just inserted): the note must shift back to 1.
    history
        .apply(&mut design, Edit::RemoveTier { index: 0 })
        .expect("remove must apply");
    assert_eq!(design.tier_note(1), Some("grind slowly"));

    // Move the noted tier (1) to the end: the note must follow it.
    let last = design.tiers.len() - 1;
    history
        .apply(&mut design, Edit::MoveTier { from: 1, to: last })
        .expect("move must apply");
    assert_eq!(design.tier_note(1), None);
    assert_eq!(design.tier_note(last), Some("grind slowly"));

    // This design has no scale-reference anchor anywhere, so it never solves --
    // `save_paired` therefore always falls back to a draft, which
    // `load_paired` then rebuilds `tiers`/`tier_notes` wholesale from the
    // sidecar's own array (`TierOverlay::AppliedFromDraft`).
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");
    assert!(saved.native.draft, "this design can never solve");
    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(loaded.tier_overlay, TierOverlay::AppliedFromDraft);
    assert_eq!(loaded.design.tier_note(last), Some("grind slowly"));
}

/// [`tier_notes_survive_add_remove_and_reorder_then_a_save_load_round_trip`]'s own
/// helper: a minimal, unnamed-index tier good enough to insert as filler.
fn tier_named(name: &str) -> ConstraintTier {
    ConstraintTier {
        angle_deg: 0.0,
        name: name.to_string(),
        indices: Vec::new(),
        constraint: MeetConstraint::MeetExisting,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// [`crate::native::SaveExtras::history_entries`] must reach the written sidecar's
/// `[history]` table and come back out of `load_paired` unchanged.
#[test]
fn history_entries_round_trip_through_save_and_load() {
    let design = simple_design();
    let entries = vec![
        "Set material to Diamond".to_string(),
        "Remove tier C1".to_string(),
    ];
    let extras = SaveExtras {
        custom_material: None,
        history_entries: &entries,
        custom_catalogue: &[],
    };
    let saved =
        save_paired_extended(&design, "design.asc", None, None, None, &extras).expect("must save");
    assert!(saved.native.history.is_some());

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(loaded.history_entries, entries);
}

/// An empty history (a brand-new design's first save) must write no `[history]`
/// table at all, and load back as an empty trail.
#[test]
fn an_empty_history_writes_no_history_table_at_all() {
    let design = simple_design();
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");
    assert!(saved.native.history.is_none());

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(loaded.history_entries, Vec::<String>::new());
}

/// [`crate::edit::History::description_log`] itself must accumulate one entry per
/// applied edit, in order -- the source `save_paired_extended`'s caller draws
/// `history_entries` from.
#[test]
fn history_description_log_accumulates_applied_edits_in_order() {
    let mut design = simple_design();
    let mut history = History::new();
    history
        .apply(
            &mut design,
            Edit::SetGirdleDiameterMm {
                girdle_diameter_mm: Some(7.0),
            },
        )
        .expect("must apply");
    history
        .apply(&mut design, Edit::SetPreformYOffset { y_offset: 0.3 })
        .expect("must apply");
    assert_eq!(history.description_log().len(), 2);
    assert_eq!(
        history.description_log()[0],
        "Set girdle diameter to 7.00 mm"
    );
    assert_eq!(history.description_log()[1], "Set preform offset to 0.30");
}
