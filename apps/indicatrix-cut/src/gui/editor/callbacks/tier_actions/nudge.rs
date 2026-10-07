//! The tier list's inline angle cell and the coalesced angle-nudge path.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use indicatrix_editor::session::InlineAngle;
use slint::{ComponentHandle, SharedString};

use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            edit_intent,
            relation_ui::{
                DRIVEN_ANGLE_HINT, edit_error_text, refresh_with_followers, split_driven_targets,
                with_followers,
            },
            stall_guard::stall_guard,
            state::EditorState,
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// The tier list's inline angle cell (`editor_view.slint`'s `TierAngleCell`): commits
/// on Enter or focus loss as one `Edit::ModifyTier` (angle only, everything else on
/// the tier untouched) through `EditorState::apply`. Invalid text is reported via
/// toast and left uncommitted -- since [`refresh_editor_panel_stale`] is skipped on
/// that path, `editor_tiers` (and so the cell's own display) is untouched too, which
/// only reverts the cell's local scratch text because `TierAngleCell` recreates its
/// `LineEdit` (and re-seeds it from the row's real `angle_deg`) on every fresh
/// `editor_tiers` push -- see that component's own doc comment. A parsed value
/// identical to the tier's current angle is a silent no-op (no `Edit`, no refresh):
/// committing an unchanged value should not spend an undo slot.
pub(in crate::gui::editor) fn setup_inline_set_angle_callback(
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
    ui.global::<EditorModel>()
        .on_inline_set_angle(move |index: i32, text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let mut st = state.borrow_mut();
            // The parse, the bit-exact "unchanged" check (which also ends any
            // scroll-wheel nudge coalescing run) and the one `Edit::ModifyTier` live in
            // `EditorSession::set_tier_angle_from_text`, shared with the web app.
            match st.set_tier_angle_from_text(index, &text) {
                Ok(InlineAngle::Missing) => {}
                // Without this toast, a committed-but-unchanged edit would be silent and
                // indistinguishable from a dropped one.
                Ok(InlineAngle::NoChange) => show_toast(&ui, "No change.", "info"),
                // A relation typed into the cell (`=P1-2`) moves the tier and everything
                // that follows it, so the followers are refreshed too.
                Ok(InlineAngle::Applied(_)) => refresh_with_followers(
                    &ui,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                    &st,
                    [index],
                ),
                Err(e) => show_toast(&ui, &e, "error"),
            }
        });
}

// `clamp_nudge_to_side` (a nudge stops at 0 degrees instead of crossing blocks) and
// the clamped tier's label moved to `indicatrix_editor::session` with the nudge
// itself (`EditorSession::nudge_angles`); `tier_nudge_label` is re-exported here at
// its old path for the Save Tier form and tier-detach paths.
pub(super) use indicatrix_editor::session::tier_nudge_label;

/// The tier list's angle-nudge path -- the inline cell's Up/Down/wheel (see
/// `tier_angle_cell.slint`'s `TierAngleCell::step`/`TierAngleCell.nudge`) forwards
/// here as `(anchor_index, delta_deg)`. The tier FORM's own Angle field Up/Down
/// (`editor_inspector/tier_form_tab.slint`'s `LineEdit.key-pressed`) does NOT --
/// it asks `SliderModel.stepped_angle_text` (`indicatrix_editor::slider_ranges::
/// stepped_angle_text`), a pure function over the field's own local scratch text,
/// since the form is a staging area with nothing committed to `Design` yet; this
/// path only ever nudges an EXISTING, already-saved tier.
///
/// `delta_deg` is a change of the angle the row SHOWS. The tier table prints a pavilion
/// tier's angle without its minus sign, so Up (a positive delta) makes the shown number
/// bigger for a crown and a pavilion tier alike; the stored sign stays
/// (`EditorSession::nudge_displayed_angles`).
///
/// When `anchor_index` is part of a multi-select group of two or more
/// (`EditorState::multi_selected`), every selected tier is nudged together as ONE
/// undoable `Edit::RetargetAngles` -- reusing that existing "several tiers, one
/// undo step, exact per-tier inverse" primitive rather than a new `Edit::Batch`
/// variant, since `RetargetAngles` already is exactly that (see its own doc comment
/// in `indicatrix_cut_core::Edit`). A lone tier still goes through the same
/// `RetargetAngles` path with a single-element `changes` vec, so there is only one
/// code path here rather than a single/multi split.
///
/// Applied through [`EditorState::apply_coalescing`] (not [`EditorState::apply`]) so
/// several nudges typed/scrolled in quick succession collapse into one undo step --
/// see `indicatrix_editor::session::angle_nudge_coalesce_key` for how the coalescing key is derived from the
/// nudge's actual target set, distinguishing a lone tier's nudge from a
/// multi-selected group containing that same tier.
///
/// `History::apply_coalescing`
/// already merged the UNDO step for a fast nudge burst, but every tick still ran
/// its own apply/refresh/replan cycle on the UI thread (each of those clones the
/// whole `Design` twice -- `view::submit_preview_replan_for`'s own doc comment).
/// This now posts an [`edit_intent::EditIntent::NudgeAngle`] into a queue this
/// function builds once, and [`apply_nudge_intent`] (the actual apply/clamp/
/// refresh/replan/toast logic, moved out of this closure unchanged) runs at most
/// once per 16ms drain tick, against the SUMMED `delta_deg` of everything posted
/// since the last tick -- see [`edit_intent::EditIntentQueue`]'s own doc comment.
pub(in crate::gui::editor) fn setup_nudge_angle_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let intent_queue = {
        let state = Rc::clone(state);
        let render_ctx = Arc::clone(render_ctx);
        let preview_state = Arc::clone(preview_state);
        let solid_last_solved = Arc::clone(solid_last_solved);
        let ui_weak = ui.as_weak();
        edit_intent::EditIntentQueue::new(move |intent| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let edit_intent::EditIntent::NudgeAngle { targets, delta_deg } = intent else {
                return;
            };
            apply_nudge_intent(
                &ui,
                &state,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                &targets,
                delta_deg,
            );
        })
    };
    let state = Rc::clone(state);
    ui.global::<EditorModel>()
        .on_nudge_angle(move |anchor_index: i32, delta_deg: f32| {
            stall_guard("on_nudge_angle", || {
                let Ok(anchor_index) = usize::try_from(anchor_index) else {
                    return;
                };
                let st = state.borrow();
                let is_multi_target =
                    st.multi_selected.len() > 1 && st.multi_selected.contains(&anchor_index);
                let targets: Vec<usize> = if is_multi_target {
                    st.multi_selected.iter().copied().collect()
                } else {
                    vec![anchor_index]
                };
                drop(st);
                intent_queue.post(edit_intent::EditIntent::NudgeAngle {
                    targets,
                    delta_deg: f64::from(delta_deg),
                });
            });
        });
}

/// [`setup_nudge_angle_callback`]'s actual apply/clamp/refresh/replan/toast work,
/// run once per drained [`edit_intent::EditIntent::NudgeAngle`] against the
/// SUMMED `delta_deg` of the whole coalesced burst -- see that function's own doc
/// comment. `targets`/`delta_deg` are read fresh against the design's CURRENT
/// angle at drain time (not whatever it was when the first tick of the burst
/// posted), so the zero-crossing clamp always judges the
/// real, final position, exactly as if the summed delta had been applied in one
/// step -- which, after this change, it is.
fn apply_nudge_intent(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    targets: &[usize],
    delta_deg: f64,
) {
    let mut st = state.borrow_mut();
    // A tier whose angle follows a relation takes no direct nudge: say why, once, and
    // nudge the free tiers of the group as usual.
    let (driven, targets) = split_driven_targets(&st.design, targets);
    let targets = targets.as_slice();
    if targets.is_empty() {
        drop(st);
        if !driven.is_empty() {
            show_toast(ui, DRIVEN_ANGLE_HINT, "info");
        }
        return;
    }
    match st.nudge_angles(targets, delta_deg) {
        Ok(None) => {}
        Ok(Some(outcome)) => {
            // The tiers that follow a nudged tier moved with it.
            let dirty: BTreeSet<usize> = with_followers(&st.design, targets.iter().copied());
            refresh_editor_panel_stale(ui, render_ctx, &st, &dirty);
            submit_preview_replan(
                ui,
                render_ctx,
                preview_state,
                solid_last_solved,
                &st,
                dirty,
                false,
            );
            // The angle's sign is the only thing that says which block a
            // tier belongs to (`indicatrix_editor::session::clamp_nudge_to_side`),
            // so a nudge that would cross zero is clamped there instead of
            // silently reclassifying the tier -- explain the stop instead
            // of leaving it looking like the nudge simply refused to move.
            if !outcome.clamped_labels.is_empty() {
                show_toast(
                    ui,
                    &format!(
                        "{} stopped at 0° -- nudging further would move it into the \
                         other block. Type the angle directly (e.g. \"-0\") to cross \
                         blocks on purpose.",
                        outcome.clamped_labels.join(", ")
                    ),
                    "info",
                );
            }
            // Last, so it is the toast left on screen: part of the group did not move.
            if !driven.is_empty() {
                show_toast(ui, DRIVEN_ANGLE_HINT, "info");
            }
        }
        // A follower that would leave 0-90 degrees refuses the nudge in words (the
        // session kept the reason).
        Err(e) => {
            let message = edit_error_text(&mut st, &e);
            show_toast(ui, &message, "error");
        }
    }
}
