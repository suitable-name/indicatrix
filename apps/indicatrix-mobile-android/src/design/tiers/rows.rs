//! The tier list's rows: core rows -> Slint rows, kept in step with the state.

use std::cmp::Reverse;

use indicatrix_mobile_core::{
    app_state::AppState,
    design_state::{Badge, BadgeLevel, MobileTierRow, TierBadges, tier_rows},
};

use super::ui::{self, RowsKey, sync_model};
use crate::TierRowData;

/// Writes the rows to the model unless they are what was pushed last.
pub(super) fn push(state: &AppState) {
    let key = RowsKey {
        generation: state.session.current_generation(),
        solve_revision: state.solve.revision(),
        selected: state.selected_tier,
        selected_concave: state.session.selected_concave,
        multi: state.session.multi_selected.clone(),
    };
    if ui::with(|ui| ui.rows_key.as_ref() == Some(&key)) {
        return;
    }
    let rows = tier_rows(state);
    let data: Vec<TierRowData> = rows.iter().map(row_data).collect();
    ui::with(|ui| {
        sync_model(&ui.rows_model, data);
        ui.rows = rows;
        ui.rows_key = Some(key);
    });
}

/// Forgets what was pushed, so the next [`push`] rebuilds the rows.
pub(super) fn invalidate() {
    ui::with(|ui| ui.rows_key = None);
}

/// One core row as the Slint struct.
fn row_data(row: &MobileTierRow) -> TierRowData {
    let [first, second] = pills(&row.badges);
    let (lead_text, lead_level) = pill_parts(first);
    let (trail_text, trail_level) = pill_parts(second);
    TierRowData {
        index: i32::try_from(row.index).unwrap_or(0),
        code: row.code.as_str().into(),
        name: row.name.as_str().into(),
        angle: format!("{}\u{b0}", row.angle_text).into(),
        block: row.block.as_str().into(),
        indices: row.indices_text.as_str().into(),
        mast: row.mast_text.as_str().into(),
        summary: if row.is_concave {
            row.tool_line.as_str().into()
        } else {
            row.constraint_text.as_str().into()
        },
        pill_a: lead_text,
        pill_a_level: lead_level,
        pill_b: trail_text,
        pill_b_level: trail_level,
        selected: row.is_selected,
        multi: row.is_multi_selected,
        stale: row.is_stale,
        concave: row.is_concave,
        detached: row.is_detached,
    }
}

/// How urgent a badge is; the most urgent two are shown.
const fn severity(level: BadgeLevel) -> u8 {
    match level {
        BadgeLevel::Problem => 3,
        BadgeLevel::Check => 2,
        BadgeLevel::Info => 1,
        BadgeLevel::Good => 0,
    }
}

/// The (up to) two most urgent badges of a row; equally urgent ones keep the order solve,
/// warning, margin.
fn pills(badges: &TierBadges) -> [Option<&Badge>; 2] {
    let mut all: Vec<&Badge> = [&badges.solve, &badges.warning, &badges.margin]
        .into_iter()
        .flatten()
        .collect();
    all.sort_by_key(|badge| Reverse(severity(badge.level)));
    [all.first().copied(), all.get(1).copied()]
}

/// A pill's text and level for Slint; empty text means no pill.
fn pill_parts(badge: Option<&Badge>) -> (slint::SharedString, i32) {
    badge.map_or_else(
        || (slint::SharedString::default(), 3),
        |badge| (badge.text.as_str().into(), badge.level.index()),
    )
}

#[cfg(test)]
mod tests {
    use indicatrix_mobile_core::design_state::{Badge, BadgeLevel, TierBadges};

    use super::*;

    fn badge(text: &str, level: BadgeLevel) -> Badge {
        Badge {
            text: text.to_owned(),
            level,
        }
    }

    #[test]
    fn the_two_most_urgent_badges_win_in_a_stable_order() {
        let badges = TierBadges {
            solve: Some(badge("Estimate", BadgeLevel::Check)),
            margin: Some(badge("+3.1", BadgeLevel::Good)),
            warning: Some(badge("Thin girdle", BadgeLevel::Problem)),
        };
        let [first, second] = pills(&badges);
        assert_eq!(first.map(|b| b.text.as_str()), Some("Thin girdle"));
        assert_eq!(second.map(|b| b.text.as_str()), Some("Estimate"));
    }

    #[test]
    fn equally_urgent_badges_keep_solve_warning_margin_order() {
        let badges = TierBadges {
            solve: Some(badge("a", BadgeLevel::Check)),
            warning: Some(badge("b", BadgeLevel::Check)),
            margin: Some(badge("c", BadgeLevel::Check)),
        };
        let [first, second] = pills(&badges);
        assert_eq!(first.map(|b| b.text.as_str()), Some("a"));
        assert_eq!(second.map(|b| b.text.as_str()), Some("b"));
    }

    #[test]
    fn no_badges_give_no_pills() {
        let badges = TierBadges::default();
        let [first, second] = pills(&badges);
        assert!(first.is_none() && second.is_none());
        assert_eq!(severity(BadgeLevel::Info), 1);
    }
}
