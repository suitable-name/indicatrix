//! Walks of the lessons in `tiers/editing.rs`.

use super::{Sim, act, read, walk};
use crate::guide::TIERS_MULTI_SELECTED;
use indicatrix_cut_core::Edit;

#[test]
fn a_cheater_offset_is_set_and_cleared() {
    let sim = walk(
        "tiers-cheater-offset",
        vec![
            read(),
            act(|sim| {
                let index = sim.row("Crown Main");
                sim.apply(Edit::SetCheaterOffset {
                    index,
                    offset_deg: Some(0.5),
                });
            }),
            act(|sim| {
                let index = sim.row("Crown Main");
                sim.apply(Edit::SetCheaterOffset {
                    index,
                    offset_deg: None,
                });
            }),
        ],
    );
    assert_eq!(
        sim.session.design.cheater_offset_deg(sim.row("Crown Main")),
        None
    );
}

#[test]
fn a_tier_note_is_written_and_cleared() {
    let sim = walk(
        "tiers-tier-notes",
        vec![
            act(|sim| {
                let index = sim.row("Pavilion Main");
                sim.apply(Edit::SetTierNote {
                    index,
                    note: Some("check the meet here".to_owned()),
                });
            }),
            act(|sim| {
                let index = sim.row("Pavilion Main");
                sim.apply(Edit::SetTierNote { index, note: None });
            }),
            read(),
        ],
    );
    assert_eq!(sim.session.design.tier_note(sim.row("Pavilion Main")), None);
}

#[test]
fn a_duplicate_sits_below_its_source_and_can_be_changed() {
    let sim = walk(
        "tiers-duplicate",
        vec![
            act(|sim| {
                let row = sim.row("Crown Main");
                sim.session
                    .duplicate_tier(row)
                    .expect("the tier duplicates")
                    .expect("the tier exists");
            }),
            act(|sim| sim.set_angle("Crown Main (2)", "30")),
            read(),
        ],
    );
    assert_eq!(
        sim.row("Crown Main (2)"),
        sim.row("Crown Main") + 1,
        "the copy sits right below the original"
    );
}

#[test]
fn moving_a_tier_changes_the_cutting_order() {
    let sim = walk(
        "tiers-move",
        vec![
            act(|sim| {
                let row = sim.row("Girdle");
                sim.session
                    .move_tier(row, -1)
                    .expect("the tier moves")
                    .expect("the tier is not at the top");
            }),
            act(|sim| {
                let row = sim.row("Pavilion Main");
                sim.session
                    .move_tier(row, -1)
                    .expect("the tier moves")
                    .expect("the tier is not at the top");
            }),
            read(),
        ],
    );
    assert_eq!(sim.row("Pavilion Main"), 0);
    assert!(
        sim.row("Girdle") < sim.row("Crown Main"),
        "the girdle stays above the crown"
    );
}

#[test]
fn a_removed_tier_comes_back_with_undo() {
    walk(
        "tiers-delete",
        vec![
            act(|sim| {
                let row = sim.row("Table");
                sim.session
                    .remove_tier(row)
                    .expect("nothing meets the table");
            }),
            act(Sim::undo),
            read(),
        ],
    );
}

#[test]
fn a_group_of_tiers_is_offset_and_deleted_together() {
    let sim = walk(
        "tiers-multi-select",
        vec![
            act(|sim| {
                let crown = sim.row("Crown Main");
                let pavilion = sim.row("Pavilion Main");
                sim.session.multi_selected.extend([crown, pavilion]);
                sim.events.push(TIERS_MULTI_SELECTED.to_owned());
            }),
            act(|sim| {
                let group: Vec<usize> = sim.session.multi_selected.iter().copied().collect();
                let now = sim.tick();
                sim.session
                    .nudge_angles(&group, 1.0, now)
                    .expect("the offset applies");
            }),
            act(|sim| {
                let removed = sim
                    .session
                    .remove_multi_selected()
                    .expect("nothing else meets the group");
                assert_eq!(removed, 2);
            }),
            read(),
        ],
    );
    let names: Vec<&str> = sim
        .session
        .design
        .tiers
        .iter()
        .map(|tier| tier.name.as_str())
        .collect();
    assert_eq!(names, ["Girdle", "Table"]);
}
