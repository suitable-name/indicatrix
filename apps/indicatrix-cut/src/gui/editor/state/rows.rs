//! The tier-list row builders, as Slint `EditorTierItem`s: thin adapters over
//! `indicatrix_editor::view_model::rows` (shared with the web app), which builds
//! the plain `TierRow`s -- see that module for what each builder does and when a
//! caller uses it.

use super::row_format::tier_items_from_rows;
use crate::EditorTierItem;
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;
use indicatrix_editor::view_model::{row_format::format_angle_cell, rows};

pub(in crate::gui::editor) use indicatrix_editor::view_model::rows::manufacturability_warnings_tagged;

/// [`rows::tier_items_stale_with_last_solved`] as Slint rows.
#[must_use]
pub(in crate::gui::editor) fn tier_items_stale_with_last_solved(
    design: &Design,
    n_d: f64,
    last_solved: Option<&[SolvedTier]>,
    dirty: &std::collections::BTreeSet<usize>,
) -> Vec<EditorTierItem> {
    tier_items_from_rows(rows::tier_items_stale_with_last_solved(
        design,
        n_d,
        last_solved,
        dirty,
    ))
}

/// [`rows::tier_items`] as Slint rows (solves `design` once).
pub(in crate::gui::editor) fn tier_items(design: &Design, n_d: f64) -> Vec<EditorTierItem> {
    tier_items_from_rows(rows::tier_items(design, n_d))
}

/// [`rows::tier_items_from_solved`] as Slint rows.
///
/// # Panics
///
/// `solved` must have one entry per tier `design` currently has, in the same
/// order -- see [`rows::tier_items_from_solved`].
pub(in crate::gui::editor) fn tier_items_from_solved(
    design: &Design,
    solved: &[SolvedTier],
    n_d: f64,
) -> Vec<EditorTierItem> {
    tier_items_from_rows(rows::tier_items_from_solved(design, solved, n_d))
}

/// Patches `EditorTierItem::proposed_angle` onto each already-pushed row one of
/// `changes` targets -- the Slint-row twin of [`rows::apply_proposed_angles`]
/// (same two-decimal [`format_angle_cell`] text), for a caller patching the live
/// model in place.
pub(in crate::gui::editor) fn apply_proposed_angles(
    tiers: &mut [EditorTierItem],
    changes: &[indicatrix_cut_core::AngleChange],
) {
    for change in changes {
        if let Some(row) = tiers.get_mut(change.index) {
            row.proposed_angle = format_angle_cell(change.to_deg).into();
        }
    }
}
