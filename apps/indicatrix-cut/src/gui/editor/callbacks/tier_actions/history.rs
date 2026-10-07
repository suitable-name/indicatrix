//! Undo/Redo: the two `History` navigation callbacks, and the refresh every move along
//! the history (those two and the History tab's jump) ends in.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use slint::ComponentHandle;

use super::misc::{bump_form_reset_pulse, clamp_selection_to_tier_count};
use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            state::EditorState,
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};
use indicatrix_editor::{EditChange, session::TierIndexMap};

/// What one move along the history (an Undo, a Redo, or the History tab's jump over any
/// number of steps) did to the rows of the tier table: the change the session reported
/// and the renumbering of the rows it caused.
///
/// The session renumbers its own selections with the same map; this is how the
/// desktop's single selected row (`EditorModel.selected_tier_index`, which only the UI
/// holds) follows its tier too instead of keeping a row number that now names the tier
/// next to it.
pub(in crate::gui::editor) struct HistoryMove {
    change: EditChange,
    renumbering: TierIndexMap,
}

impl HistoryMove {
    /// The move `change` describes, with the `renumbering` the session returned for it.
    #[must_use]
    pub(in crate::gui::editor) const fn new(change: EditChange, renumbering: TierIndexMap) -> Self {
        Self {
            change,
            renumbering,
        }
    }

    /// Where the tier table's row `row` sits after the move, or `None` when the move
    /// removed its tier. A concave tier's row follows the flat tiers, as in the table.
    #[must_use]
    fn row_after(&self, row: usize) -> Option<usize> {
        self.renumbering.map_table_row(
            row,
            self.change.tier_count_before,
            self.change.tier_count_after,
            self.change.concave_count_after,
        )
    }
}

/// `EditorModel.selected_tier_index` after `moved`: the row its tier moved to, or `-1`
/// (no selection) when the move removed that tier. `selected` below zero is no
/// selection and stays so.
#[must_use]
fn selected_row_after(selected: i32, moved: &HistoryMove) -> i32 {
    let Ok(row) = usize::try_from(selected) else {
        return selected;
    };
    moved
        .row_after(row)
        .and_then(|after| i32::try_from(after).ok())
        .unwrap_or(-1)
}

/// Makes the selected row follow its tier through `moved`, the way
/// `misc::adjust_selection_after_remove` does for a removal: a moved row is rewritten (which raises `changed` and re-seeds the form from
/// the right tier), and a selection whose tier is gone is cleared with the form reset
/// pulse, since clearing an already-cleared selection raises nothing.
fn follow_selection(ui: &MainWindow, moved: &HistoryMove) {
    let model = ui.global::<EditorModel>();
    let selected = model.get_selected_tier_index();
    let after = selected_row_after(selected, moved);
    if after == selected {
        return;
    }
    model.set_selected_tier_index(after);
    if after < 0 {
        bump_form_reset_pulse(ui);
    }
}

/// What a move along the history (Undo, Redo, or the History tab's jump to a step) owes
/// the display once the design has changed.
///
/// A move can change any tier (or a whole structural `AddTier`/`RemoveTier`), so its blast
/// radius is not tracked precisely: this forces a full re-solve rather than guessing a
/// `dirty` set -- and, for the same reason, trusts no cached mast.
///
/// An out-of-range selection reaches `EditorInspector.clear_form` through
/// `clamp_selection_to_tier_count`'s own `form_reset_pulse` bump; a selection that
/// SURVIVED was already re-seeded by the refresh, since its own index did not move and so
/// raises no `changed`.
///
/// The selected row is only clamped here, which suits an edit that moves no rows (a
/// replaced schedule, a retarget). A caller that knows how the move renumbered the rows
/// (Undo, Redo, jump) uses [`refresh_after_renumbering_move`] instead, so the selection
/// follows its tier.
pub(in crate::gui::editor) fn refresh_after_history_move(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &EditorState,
) {
    refresh_editor_panel_stale(
        ui,
        render_ctx,
        state,
        &(0..state.design.tiers.len()).collect(),
    );
    clamp_selection_to_tier_count(
        ui,
        state.design.tiers.len() + state.design.concave_tiers.len(),
    );
    submit_preview_replan(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        state,
        BTreeSet::new(),
        true,
    );
}

/// [`refresh_after_history_move`] for a move whose row renumbering is known: the selected
/// row is first made to follow its tier through `moved`, then the display is refreshed as
/// for any move along the history.
///
/// The row is rewritten before the refresh, so the refresh sees the selection the cutter
/// now has: it re-seeds the form from the right tier, or, when the tier is gone, leaves the
/// form to the reset pulse [`follow_selection`] raised.
pub(in crate::gui::editor) fn refresh_after_renumbering_move(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &EditorState,
    moved: &HistoryMove,
) {
    follow_selection(ui, moved);
    refresh_after_history_move(ui, render_ctx, preview_state, solid_last_solved, state);
}

/// "Undo": a no-op (no toast, no viewport refresh) when there is nothing to undo --
/// `EditorView`'s own button is already disabled via `can_undo` in that case, so this
/// only guards against a stale click racing a state change, not the common path.
pub(in crate::gui::editor) fn setup_undo_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_undo(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        match st.undo_mapped() {
            Ok(Some((change, renumbering))) => {
                refresh_after_renumbering_move(
                    &ui,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                    &st,
                    &HistoryMove::new(change, renumbering),
                );
            }
            Ok(None) => {}
            // The recorded inverse edit failed to replay -- the design/history stack
            // are left as `EditorState::undo` found them (the failed edit stays on the
            // undo stack, see `History::undo`'s doc comment), so this is safe to just
            // report rather than panic.
            Err(e) => show_toast(&ui, &format!("Undo failed: {e}"), "error"),
        }
    });
}

/// "Redo": same no-op-when-nothing-to-do treatment as [`setup_undo_callback`].
pub(in crate::gui::editor) fn setup_redo_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_redo(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        match st.redo_mapped() {
            Ok(Some((change, renumbering))) => {
                // Same "blast radius unknown, trust nothing cached" reasoning as
                // `setup_undo_callback`'s matching arm, and the same selected-row
                // renumbering.
                refresh_after_renumbering_move(
                    &ui,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                    &st,
                    &HistoryMove::new(change, renumbering),
                );
            }
            Ok(None) => {}
            // See `setup_undo_callback`'s matching arm -- same recovery guarantee,
            // symmetrically for the redo stack.
            Err(e) => show_toast(&ui, &format!("Redo failed: {e}"), "error"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{ConstraintTier, Edit};

    fn tier(name: &str) -> ConstraintTier {
        ConstraintTier {
            angle_deg: -40.0,
            name: name.to_string(),
            indices: Vec::new(),
            constraint: MeetConstraint::ScaleReference(0.5),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    /// An editor holding five pavilion tiers named `P0` to `P4`.
    fn state_with_five_tiers() -> EditorState {
        let mut state = EditorState::fresh();
        for index in 0..5 {
            state
                .apply(Edit::AddTier {
                    index,
                    tier: tier(&format!("P{index}")),
                })
                .expect("a tier adds");
        }
        state
    }

    fn tier_name(state: &EditorState, row: i32) -> &str {
        &state.design.tiers[usize::try_from(row).expect("a row")].name
    }

    /// The reviewer's failure: P3 selected, a tier added above it, then Undo. Clamping alone
    /// left the row at 4, which is P4 once the undo has removed the added tier.
    #[test]
    fn undo_moves_the_selected_row_with_its_tier_and_agrees_with_the_session() {
        let mut state = state_with_five_tiers();
        state
            .apply(Edit::AddTier {
                index: 2,
                tier: tier("New"),
            })
            .expect("a tier adds");
        // P3 is row 4 now. The UI's selected row and the session's multi-selection both
        // name it.
        let mut selected = 4;
        assert_eq!(tier_name(&state, selected), "P3");
        state.multi_selected.insert(4);

        let (change, renumbering) = state
            .undo_mapped()
            .expect("the undo replays")
            .expect("something to undo");
        let moved = HistoryMove::new(change, renumbering);
        assert_eq!(
            tier_name(&state, selected),
            "P4",
            "the row left alone names the neighbour"
        );
        selected = selected_row_after(selected, &moved);
        assert_eq!(selected, 3);
        assert_eq!(tier_name(&state, selected), "P3");
        assert_eq!(
            state.multi_selected,
            std::collections::BTreeSet::from([3]),
            "the UI row agrees with the session's multi-selection"
        );

        // Redo puts the tier back above it: the row follows again.
        let (change, renumbering) = state
            .redo_mapped()
            .expect("the redo replays")
            .expect("something to redo");
        selected = selected_row_after(selected, &HistoryMove::new(change, renumbering));
        assert_eq!(selected, 4);
        assert_eq!(tier_name(&state, selected), "P3");
        assert_eq!(state.multi_selected, std::collections::BTreeSet::from([4]));
    }

    #[test]
    fn a_selection_on_a_tier_the_move_removes_is_cleared() {
        let mut state = state_with_five_tiers();
        state
            .apply(Edit::AddTier {
                index: 2,
                tier: tier("New"),
            })
            .expect("a tier adds");
        let (change, renumbering) = state
            .undo_mapped()
            .expect("the undo replays")
            .expect("something to undo");
        // Row 2 was the added tier, which the undo removed.
        assert_eq!(
            selected_row_after(2, &HistoryMove::new(change, renumbering)),
            -1
        );
    }

    #[test]
    fn no_selection_stays_no_selection() {
        let mut state = state_with_five_tiers();
        state.apply(Edit::RemoveTier { index: 0 }).expect("removes");
        let (change, renumbering) = state
            .undo_mapped()
            .expect("the undo replays")
            .expect("something to undo");
        let moved = HistoryMove::new(change, renumbering);
        assert_eq!(selected_row_after(-1, &moved), -1);
        assert_eq!(selected_row_after(-5, &moved), -5);
    }

    /// A jump over several steps moves the row through all of them, and the session's own
    /// selection ends up on the same tier.
    #[test]
    fn a_jump_moves_the_selected_row_through_every_step() {
        let mut state = state_with_five_tiers();
        // Step 6 moves P0 to row 3, step 7 inserts a tier at row 1: P0 is row 4.
        state
            .apply(Edit::MoveTier { from: 0, to: 3 })
            .expect("moves");
        state
            .apply(Edit::AddTier {
                index: 1,
                tier: tier("New"),
            })
            .expect("a tier adds");
        let mut selected = 4;
        assert_eq!(tier_name(&state, selected), "P0");
        state.multi_selected.insert(4);

        let (result, renumbering) = state.jump_to_mapped(5);
        let change = result.expect("the jump works").change.expect("it moved");
        let moved = HistoryMove::new(change, renumbering);
        selected = selected_row_after(selected, &moved);
        assert_eq!(selected, 0);
        assert_eq!(tier_name(&state, selected), "P0");
        assert_eq!(state.multi_selected, std::collections::BTreeSet::from([0]));
    }

    /// Opening a variant replaces the whole tier list: no flat row can be followed through
    /// it, so the selection is cleared; a variant that changes only the rough or the girdle
    /// size moves no row, and the selection stays on its tier.
    #[test]
    fn opening_a_variant_clears_the_selected_row_only_when_it_replaces_the_tiers() {
        let mut state = state_with_five_tiers();
        let mut replaced = indicatrix_cut_core::ScheduleState::of(&state.design);
        replaced.tiers[0].angle_deg = -41.0;
        let (change, renumbering) = state
            .session
            .try_apply_mapped(
                Edit::ReplaceSchedule(Box::new(replaced)),
                Some("Open variant \"Steeper\""),
            )
            .expect("the variant opens");
        assert_eq!(
            selected_row_after(2, &HistoryMove::new(change, renumbering)),
            -1,
            "the old row names nothing in the new tier list"
        );

        let (change, renumbering) = state
            .session
            .try_apply_mapped(
                Edit::SetGirdleDiameterMm {
                    girdle_diameter_mm: Some(7.0),
                },
                Some("Open variant \"Wider\""),
            )
            .expect("the variant opens");
        let moved = HistoryMove::new(change, renumbering);
        assert_eq!(selected_row_after(2, &moved), 2, "no row moved");
        assert_eq!(selected_row_after(-1, &moved), -1);
    }

    /// The verdict's fix applies its edit through the session: a fix that removes a tier
    /// must move the selected row with its tier, where clamping alone left the row on the
    /// tier next to it.
    #[test]
    fn a_fix_that_removes_a_tier_moves_the_selected_row_with_its_tier() {
        let mut state = state_with_five_tiers();
        let (change, renumbering) = state
            .session
            .try_apply_mapped(Edit::RemoveTier { index: 1 }, None)
            .expect("the fix applies");
        let moved = HistoryMove::new(change, renumbering);
        // P3 sat on row 3 and is on row 2 now; P1, the removed tier, leaves the selection.
        assert_eq!(tier_name(&state, selected_row_after(3, &moved)), "P3");
        assert_eq!(selected_row_after(1, &moved), -1);
        assert_eq!(selected_row_after(0, &moved), 0);
    }

    /// A concave tier's row follows the flat tiers, so a flat tier added above shifts it.
    #[test]
    fn a_concave_row_follows_its_tier_past_the_flat_rows() {
        let concave = indicatrix_cut_core::Design::concave_fixture()
            .concave_tiers
            .remove(0);
        // Four flat tiers and two concave ones (rows 4 and 5); a concave tier is added in
        // front of them, so each concave row moves down by one.
        let moved = HistoryMove::new(
            EditChange {
                generation: 9,
                tier_count_before: 4,
                tier_count_after: 4,
                concave_count_before: 2,
                concave_count_after: 3,
            },
            TierIndexMap::of(&Edit::AddConcaveTier {
                index: 0,
                tier: concave,
            }),
        );
        assert_eq!(selected_row_after(1, &moved), 1, "a flat row stays");
        assert_eq!(selected_row_after(4, &moved), 5);
        assert_eq!(selected_row_after(5, &moved), 6);
    }
}
