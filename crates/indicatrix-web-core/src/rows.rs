//! Pure helpers of the page's tier table, nudges, schedule readout and history list, kept
//! here (not in the wasm-only page crate) so they run under `cargo test`.
//!
//! - **Row ids**: [`resolve_row`] and [`row_id`] map the table's row ids (flat tiers first,
//!   then concave ones) to a [`TierRef`] and back; [`CONCAVE_EDIT_REFUSED`] is what an edit
//!   of a concave row says instead of acting.
//! - **Relations**: [`driven_refusal`] words the refusal of a direct angle edit of a driven
//!   tier, and [`with_followers`] widens an edit's tier set to the tiers that follow it.
//! - **Schedule**: [`schedule_notes`] is the small line under a cutting-instructions row.
//! - **History**: [`labels_newest_first`] and [`jump_message`] for the History tab.

use indicatrix_cut_core::{Design, design::TierRef};
use indicatrix_editor::view_model::CuttingRow;
use std::collections::BTreeSet;

/// What an editing action on a concave row says instead of acting: a concave tier lives in
/// `design.concave_tiers`, which the web editor does not edit yet.
pub const CONCAVE_EDIT_REFUSED: &str = "Concave tiers are edited on the desktop for now";

/// Which tier a row id names, for a design with `flat_count` flat and `concave_count`
/// concave tiers.
///
/// The table lists the flat tiers first and the concave ones after them, and a row's id
/// (`TierRowData::index`, what every `TierTableModel` callback sends back) is its position
/// in that list: a flat row's id is its index in `design.tiers`, a concave row's id is
/// `flat_count` plus its index in `design.concave_tiers`. The two lists therefore never share
/// an id, and a concave id is never a valid flat index. `None` for a negative id or one past
/// the last row.
#[must_use]
pub fn resolve_row(raw: i32, flat_count: usize, concave_count: usize) -> Option<TierRef> {
    let raw = usize::try_from(raw).ok()?;
    if raw < flat_count {
        return Some(TierRef::Flat(raw));
    }
    let concave = raw - flat_count;
    (concave < concave_count).then_some(TierRef::Concave(concave))
}

/// The row id of `tier` -- the inverse of [`resolve_row`]. `None` when it does not fit an
/// `i32`.
#[must_use]
pub fn row_id(tier: TierRef, flat_count: usize) -> Option<i32> {
    match tier {
        TierRef::Flat(index) => i32::try_from(index).ok(),
        TierRef::Concave(index) => i32::try_from(flat_count.checked_add(index)?).ok(),
    }
}

/// Why tier `tier`'s angle takes no direct edit, when it follows a relation.
///
/// Names the tier, its relation and the tier(s) driving it, as the desktop's refusal does.
/// `None` for a free angle (or a tier that does not exist).
#[must_use]
pub fn driven_refusal(design: &Design, tier: usize) -> Option<String> {
    let relation = design.relation_text(tier)?;
    let label = design.relation_label(tier);
    let drivers: Vec<String> = design
        .relation_drivers(tier)
        .into_iter()
        .map(|driver| design.relation_label(driver))
        .collect();
    let driven_by = if drivers.is_empty() {
        String::new()
    } else {
        format!(", driven by {}", drivers.join(", "))
    };
    Some(format!(
        "{label} follows a relation ({label} = {relation}{driven_by}), so its angle cannot be \
         edited directly. Change the tier it follows, or edit the relation in the Tier form."
    ))
}

/// `seeds` plus every tier whose angle follows one of them, directly or through followers.
///
/// These are the rows an edit of a seed moved, which the table must not show with their
/// old masts. `design` is the design AFTER the edit.
#[must_use]
pub fn with_followers(design: &Design, seeds: impl IntoIterator<Item = usize>) -> BTreeSet<usize> {
    let mut all: BTreeSet<usize> = seeds.into_iter().collect();
    let mut frontier: Vec<usize> = all.iter().copied().collect();
    while let Some(tier) = frontier.pop() {
        for dependant in design.relation_dependants(tier) {
            if all.insert(dependant) {
                frontier.push(dependant);
            }
        }
    }
    all
}

/// The small line under a schedule row.
///
/// It is a concave tier's tool line (the four `second_line` columns joined with two spaces,
/// as the desktop shows them under the facet line), then the tier's notes. `ScheduleRow` has
/// no field of its own for the tool line, so it rides in the notes text, which the Schedule
/// tab already wraps under the row.
#[must_use]
pub fn schedule_notes(row: &CuttingRow) -> String {
    let Some(second) = &row.second_line else {
        return row.notes.clone();
    };
    let tool = second.join("  ");
    if row.notes.is_empty() {
        tool
    } else {
        format!("{tool}\n{}", row.notes)
    }
}

/// The history labels as the tab lists them: newest first, so row `i` of `n` is step `n - i`.
#[must_use]
pub fn labels_newest_first(labels: impl DoubleEndedIterator<Item = String>) -> Vec<String> {
    labels.rev().collect()
}

/// The status line after a jump to history step `position` (0 is the start).
#[must_use]
pub fn jump_message(position: usize) -> String {
    if position == 0 {
        "Went back to the start of the history.".to_string()
    } else {
        format!("Went to step {position}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{PreformSpec, design::ConstraintTier};

    fn tier(name: &str, angle_deg: f64) -> ConstraintTier {
        ConstraintTier {
            angle_deg,
            name: name.to_string(),
            indices: vec![0.0],
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    /// A design whose tiers have these names and angles.
    fn design_with(tiers: &[(&str, f64)]) -> Design {
        let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
        design.tiers = tiers.iter().map(|&(n, a)| tier(n, a)).collect();
        design.ensure_tier_ids();
        design
    }

    /// Gives the tier at `index` the relation `text`.
    fn relate(design: &mut Design, index: usize, text: &str) {
        let relation = design.parse_relation(text).expect("relation reads");
        let id = design.tier_ids[index];
        design.tier_relations.insert(id, relation);
    }

    #[test]
    fn flat_rows_keep_their_index_and_concave_rows_follow_them() {
        // Three flat tiers, two concave ones: ids 0..3 are flat, 3 and 4 are concave.
        assert_eq!(resolve_row(0, 3, 2), Some(TierRef::Flat(0)));
        assert_eq!(resolve_row(2, 3, 2), Some(TierRef::Flat(2)));
        assert_eq!(resolve_row(3, 3, 2), Some(TierRef::Concave(0)));
        assert_eq!(resolve_row(4, 3, 2), Some(TierRef::Concave(1)));
    }

    #[test]
    fn ids_outside_the_table_name_nothing() {
        assert_eq!(resolve_row(-1, 3, 2), None);
        assert_eq!(resolve_row(5, 3, 2), None);
        assert_eq!(resolve_row(0, 0, 0), None);
        // No concave tiers: an id past the flat rows is nothing, never a flat tier.
        assert_eq!(resolve_row(3, 3, 0), None);
    }

    #[test]
    fn a_concave_id_is_never_a_flat_index() {
        for flat in 0..6 {
            for concave in 0..4 {
                for raw in 0..flat + concave {
                    let id = i32::try_from(raw).expect("small");
                    let is_flat = matches!(resolve_row(id, flat, concave), Some(TierRef::Flat(_)));
                    assert_eq!(is_flat, raw < flat);
                }
            }
        }
    }

    #[test]
    fn row_id_inverts_resolve_row() {
        for flat in 0..5 {
            for concave in 0..4 {
                for raw in 0..flat + concave {
                    let id = i32::try_from(raw).expect("small");
                    let tier = resolve_row(id, flat, concave).expect("in range");
                    assert_eq!(row_id(tier, flat), Some(id));
                }
            }
        }
    }

    #[test]
    fn row_id_refuses_an_overflowing_id() {
        assert_eq!(row_id(TierRef::Concave(usize::MAX), 1), None);
        assert_eq!(row_id(TierRef::Flat(usize::MAX), 0), None);
    }

    #[test]
    fn a_free_tier_has_no_refusal() {
        let design = design_with(&[("C1", 40.0), ("C2", 10.0)]);
        assert_eq!(driven_refusal(&design, 0), None);
        assert_eq!(driven_refusal(&design, 1), None);
        assert_eq!(driven_refusal(&design, 9), None);
    }

    #[test]
    fn a_driven_tier_is_refused_naming_its_driver() {
        let mut design = design_with(&[("C1", 40.0), ("C2", 10.0), ("C3", 30.0)]);
        relate(&mut design, 1, "C1 - 2");
        let message = driven_refusal(&design, 1).expect("C2 is driven");
        assert!(
            message.starts_with("C2 follows a relation (C2 = "),
            "{message}"
        );
        assert!(message.contains(", driven by C1)"), "{message}");
        assert!(message.contains("cannot be edited directly"), "{message}");
        // The driver itself and an unrelated tier stay free.
        assert_eq!(driven_refusal(&design, 0), None);
        assert_eq!(driven_refusal(&design, 2), None);
    }

    #[test]
    fn followers_are_found_through_a_chain() {
        // A follows B, B follows C.
        let mut design = design_with(&[("A", 1.0), ("B", 1.0), ("C", 10.0)]);
        relate(&mut design, 0, "B + 1");
        relate(&mut design, 1, "C + 1");
        assert_eq!(
            with_followers(&design, [2]),
            BTreeSet::from([0, 1, 2]),
            "editing C moves B and, through B, A"
        );
        assert_eq!(with_followers(&design, [1]), BTreeSet::from([0, 1]));
        assert_eq!(with_followers(&design, [0]), BTreeSet::from([0]));
    }

    #[test]
    fn followers_of_nothing_are_nothing() {
        let design = design_with(&[("A", 1.0)]);
        assert!(with_followers(&design, std::iter::empty()).is_empty());
        assert_eq!(with_followers(&design, [0, 0]), BTreeSet::from([0]));
    }

    fn row(notes: &str, second: Option<[&str; 4]>) -> CuttingRow {
        CuttingRow {
            notes: notes.to_string(),
            second_line: second.map(|s| s.map(str::to_string)),
            ..CuttingRow::default()
        }
    }

    #[test]
    fn a_flat_row_keeps_its_notes_only() {
        assert_eq!(schedule_notes(&row("polish", None)), "polish");
        assert_eq!(schedule_notes(&row("", None)), "");
    }

    #[test]
    fn a_concave_row_shows_its_tool_line_before_the_notes() {
        let second = Some(["CYL", "+10.00", "X = 0.000", "D/W = 0.400"]);
        assert_eq!(
            schedule_notes(&row("", second)),
            "CYL  +10.00  X = 0.000  D/W = 0.400"
        );
        assert_eq!(
            schedule_notes(&row("check", second)),
            "CYL  +10.00  X = 0.000  D/W = 0.400\ncheck"
        );
    }

    #[test]
    fn history_labels_list_newest_first() {
        let labels = ["a", "b", "c"].into_iter().map(str::to_string);
        assert_eq!(labels_newest_first(labels), ["c", "b", "a"]);
        assert_eq!(labels_newest_first(std::iter::empty()).len(), 0);
    }

    #[test]
    fn a_jump_is_worded_by_its_position() {
        assert_eq!(jump_message(0), "Went back to the start of the history.");
        assert_eq!(jump_message(3), "Went to step 3.");
    }
}
