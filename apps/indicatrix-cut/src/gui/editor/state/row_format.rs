//! The `EditorModel.tiers`/`multi_selected_count` push helpers and the thin
//! adapters from `indicatrix_editor::view_model`'s plain rows/chips to the Slint
//! `EditorTierItem`/`IndexChipItem`. The per-tier formatting itself lives in
//! `indicatrix_editor::view_model::row_format` (shared with the web app).

use crate::{EditorTierItem, IndexChipItem, MainWindow};
use indicatrix_editor::view_model::{IndexChip, TierRow};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

/// One plain [`TierRow`] as the Slint `EditorTierItem` the tier table renders --
/// a field-for-field copy.
pub(in crate::gui::editor) fn tier_item_from_row(row: TierRow) -> EditorTierItem {
    EditorTierItem {
        index: row.index,
        angle_deg: row.angle_deg.into(),
        angle_full: row.angle_full.into(),
        name: row.name.into(),
        indices: row.indices.into(),
        indices_full: row.indices_full.into(),
        constraint_kind: row.constraint_kind,
        constraint_text: row.constraint_text.into(),
        mast: row.mast.into(),
        mast_mm: row.mast_mm.into(),
        mast_full: row.mast_full.into(),
        strategy: row.strategy.into(),
        strategy_is_uncertain: row.strategy_is_uncertain,
        strategy_detail: row.strategy_detail.into(),
        needs_anchor: row.needs_anchor,
        imported_meet_text: row.imported_meet_text.into(),
        orbit_status: row.orbit_status.into(),
        orbit_incomplete: row.orbit_incomplete,
        is_detached: row.is_detached,
        block: row.block.into(),
        margin_text: row.margin_text.into(),
        risk_level: row.risk_level,
        meet_partners_text: row.meet_partners_text.into(),
        warning_text: row.warning_text.into(),
        multi_selected: row.multi_selected,
        proposed_angle: row.proposed_angle.into(),
    }
}

/// Maps a whole plain row list with [`tier_item_from_row`].
pub(in crate::gui::editor) fn tier_items_from_rows(rows: Vec<TierRow>) -> Vec<EditorTierItem> {
    rows.into_iter().map(tier_item_from_row).collect()
}

/// [`indicatrix_editor::view_model::row_format::index_chip_items`], mapped to the
/// inspector's Slint chips -- see that function for the detached-flag rule.
#[must_use]
pub(in crate::gui::editor) fn index_chip_items(
    indices: &[f64],
    detached: &[f64],
) -> Vec<IndexChipItem> {
    indicatrix_editor::view_model::row_format::index_chip_items(indices, detached)
        .into_iter()
        .map(|chip: IndexChip| IndexChipItem {
            position: chip.position,
            label: chip.label.into(),
            detached: chip.detached,
        })
        .collect()
}

/// Patches [`EditorTierItem::multi_selected`] onto every row in `rows` from
/// `multi_selected` -- the post-pass a full tier-list rebuild ([`super::rows::
/// tier_items`]/[`super::rows::tier_items_stale`]) needs to survive with the live
/// multi-select highlight intact, since neither builder itself knows about
/// `EditorState::multi_selected` (both always set the flag to `false`; see their
/// own doc comments). Every call site that replaces the WHOLE `editor_tiers` model
/// applies this immediately afterward: `view::refresh_editor_panel`/
/// `push_stale_content`, and `auto_solve`'s background-solve completion.
/// `setup_toggle_multi_select_callback` is the one exception -- it patches the
/// flag onto an ALREADY-pushed model in place instead, using this same function,
/// since toggling a selection changes nothing about `Design` and must never
/// re-run a full tier-list rebuild.
pub(in crate::gui::editor) fn apply_multi_selection(
    rows: &mut [EditorTierItem],
    multi_selected: &std::collections::BTreeSet<usize>,
) {
    for row in rows {
        row.multi_selected =
            usize::try_from(row.index).is_ok_and(|index| multi_selected.contains(&index));
    }
}

/// Pushes `EditorModel.multi_selected_count` -- the tier table's "N selected"
/// header indicator (`editor_tier_table.slint`) -- kept a SEPARATE call from
/// [`apply_multi_selection`] rather than folded into it, since one of that
/// function's call sites (`auto_solve`'s background-solve worker thread) runs off
/// the UI thread and must never touch a Slint global; every caller of THIS
/// function, by contrast, already runs on the UI thread (the two `view::` refresh
/// paths, the toggle/selection-changed callbacks in `callbacks::tier_actions`, and
/// `auto_solve`'s UI-thread completion handler).
pub(in crate::gui::editor) fn push_multi_selected_count(ui: &MainWindow, count: usize) {
    ui.global::<crate::EditorModel>()
        .set_multi_selected_count(i32::try_from(count).unwrap_or(i32::MAX));
}

/// Pushes `rows` into `EditorModel.tiers`, reusing the existing model via
/// [`slint::Model::set_row_data`] when the row count is unchanged instead of
/// replacing the whole `ModelRc` -- an ordinary edit (`ModifyTier`), an undo/redo
/// that doesn't change the tier count, or a background-solve completion never
/// resizes the list, and Slint only recreates a `for` loop's per-row component
/// tree when the MODEL ITSELF changes identity, not when one row's data does. A
/// wholesale replacement tears down and rebuilds every row's component tree on every
/// refresh, including one mid-inline-edit -- dropping keyboard focus
/// out of an open inline angle edit (`editor_tier_table.slint`'s `TierAngleCell`)
/// on every refresh, which is exactly what the same-length reuse path above avoids.
/// A structural edit that actually changes the tier count
/// (`AddTier`/`RemoveTier`, or undoing/redoing one) still needs a real replacement
/// -- `set_row_data` cannot resize a model -- so that case still replaces the model
/// wholesale.
///
/// Explicitly invokes `EditorModel.recompute_dirty` afterward rather than relying
/// on `editor.slint`'s own `changed tiers => { recompute_dirty(); }` watcher to
/// catch it: that watcher only fires when the `tiers` PROPERTY itself is
/// reassigned (the `set_tiers` branch below), never when `set_row_data` merely
/// mutates the SAME `ModelRc`'s contents in place -- without this explicit call,
/// the dirty/"Unsaved" indicator would stop updating for the common case (an
/// edit that doesn't change the tier count) the moment that branch is taken.
pub(in crate::gui::editor) fn push_tiers(ui: &MainWindow, rows: Vec<EditorTierItem>) {
    push_rows(
        &ui.global::<crate::EditorModel>().get_tiers(),
        rows,
        |model| {
            ui.global::<crate::EditorModel>().set_tiers(model);
        },
    );
    ui.global::<crate::EditorModel>().invoke_recompute_dirty();
}

/// The general form of [`push_tiers`]'s own in-place-update trick (see that
/// function's own doc comment for the full "why" -- rebuilding a Slint `for`
/// loop's whole component tree on every refresh dropped keyboard focus out of an
/// open inline edit): reuses `current` via [`slint::Model::set_row_data`] when its
/// row count already matches `rows`, calling `set` with a fresh `ModelRc` only
/// when the length actually changed (an add/remove, not an ordinary edit).
///
/// `set` is called ONLY on that replace path, never on the reuse path -- exactly
/// matching [`push_tiers`]'s own original behaviour (see its doc comment on why
/// `EditorModel.tiers`'s reassignment is what fires `editor.slint`'s `changed
/// tiers` watcher, and why reusing `current` in place must not also trigger it
/// again for nothing). A caller pushing a property with no such watcher (every
/// other use below) still benefits: skipping the property write when nothing
/// structural changed is itself the point, whether or not Slint's own property
/// setter would already have elided a same-model reassignment.
///
/// `view::push_stale_content`/
/// `push_manufacturability_and_preform_scratch`/`push_selected_tier_chips`
/// (`view.rs`, not this file) route `EditorModel.cutting_rows`/
/// `manufacturability_warnings`/`manufacturability_warning_tiers`/
/// `selected_tier_chips` through this same generic helper instead of each
/// replacing the model with a brand-new `ModelRc<VecModel<_>>` on every single
/// refresh, even a same-length one -- this is what lets each of
/// those call sites share the identical incremental-update behaviour `push_tiers`
/// already had, without four near-duplicate copies of the same length-check.
///
/// Callers still own deciding WHAT to push (the `Vec<T>` computation itself is
/// unchanged); this only changes HOW it reaches `EditorModel`.
pub(in crate::gui::editor) fn push_rows<T: Clone + 'static>(
    current: &ModelRc<T>,
    rows: Vec<T>,
    set: impl FnOnce(ModelRc<T>),
) {
    if current.row_count() == rows.len() {
        for (index, row) in rows.into_iter().enumerate() {
            current.set_row_data(index, row);
        }
    } else {
        set(ModelRc::new(VecModel::from(rows)));
    }
}
