//! The tier list's inline angle cell and the coalesced angle-nudge path.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use indicatrix_cut_core::Edit;
use slint::{ComponentHandle, SharedString};

use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            edit_intent, loading,
            stall_guard::stall_guard,
            state::{EditorState, angle_nudge_coalesce_key},
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// The tier list's inline angle cell (`editor_view.slint`'s `TierAngleCell`): commits
/// on Enter or focus loss as one [`Edit::ModifyTier`] (angle only, everything else on
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
            let Some(current) = st.design.tiers.get(index) else {
                return;
            };
            match loading::parse_angle_only(&text) {
                Ok(angle_deg) => {
                    // Bit-exact, not `==` (clippy::float_cmp): the parsed text
                    // round-tripping to a genuinely unchanged value is the only case this
                    // needs to catch -- committing a real no-op should not spend an undo
                    // slot -- and comparing bit patterns rather than magnitudes sidesteps
                    // that lint without needing an epsilon whose size would be arbitrary
                    // here.
                    if angle_deg.to_bits() == current.angle_deg.to_bits() {
                        // Committing back the SAME value is a
                        // real interaction boundary (the cutter opened the cell,
                        // looked, and closed it) -- end any scroll-wheel nudge
                        // coalescing run in progress rather than leaving it open
                        // for a later, unrelated nudge to merge into.
                        st.history.end_coalesce_run();
                        // Without this toast, a committed-but-unchanged edit would
                        // be silent and indistinguishable from a dropped one.
                        show_toast(&ui, "No change.", "info");
                        return;
                    }
                    let mut tier = current.clone();
                    tier.angle_deg = angle_deg;
                    match st.apply(Edit::ModifyTier { index, tier }) {
                        Ok(()) => {
                            refresh_editor_panel_stale(
                                &ui,
                                &render_ctx,
                                &st,
                                &BTreeSet::from([index]),
                            );
                            submit_preview_replan(
                                &ui,
                                &render_ctx,
                                &preview_state,
                                &solid_last_solved,
                                &st,
                                BTreeSet::from([index]),
                                false,
                            );
                        }
                        Err(e) => show_toast(&ui, &e.to_string(), "error"),
                    }
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
        });
}

/// Clamps a nudged angle to the ORIGINAL tier's crown/pavilion side instead of
/// letting it cross zero -- `meet_solver::blocks::tier_sides`'s side rule
/// (negative is pavilion, non-negative crown, `-0.0` forces pavilion) means a
/// nudge that crosses zero silently reclassifies the tier into the other block
/// with no confirmation and no visible change other than the sign. `-0.0`/`0.0`
/// are used as the two boundary values so the clamped result still carries the
/// correct side under that same unsigned-zero rule, rather than merely being
/// "close to zero" with an arbitrary sign.
const fn clamp_nudge_to_side(current: f64, nudged: f64) -> f64 {
    if current.is_sign_negative() == nudged.is_sign_negative() {
        return nudged;
    }
    if current.is_sign_negative() {
        -0.0
    } else {
        0.0
    }
}

/// A tier's short label for [`clamp_nudge_to_side`]'s explanatory toast --
/// `"tier 5 (Girdle)"` when named, else `"tier 5"` (1-based, matching the tier
/// table's own `#` column).
///
/// `pub(super)` since the Save Tier form and tier-detach paths label a tier the
/// same way.
pub(super) fn tier_nudge_label(tier: &indicatrix_cut_core::ConstraintTier, index: usize) -> String {
    if tier.name.is_empty() {
        format!("tier {}", index + 1)
    } else {
        format!("tier {} ({})", index + 1, tier.name)
    }
}

/// The tier list's angle-nudge path -- the inline cell's Up/Down/wheel and the tier
/// form's Angle field's Up/Down (see `editor_view.slint`'s `TierAngleCell::step`,
/// `TierAngleCell::nudge`, and the form's own `LineEdit.key-pressed`) all forward
/// here as `(anchor_index, delta_deg)`.
///
/// When `anchor_index` is part of a multi-select group of two or more
/// (`EditorState::multi_selected`), every selected tier is nudged together as ONE
/// undoable [`Edit::RetargetAngles`] -- reusing that existing "several tiers, one
/// undo step, exact per-tier inverse" primitive rather than a new `Edit::Batch`
/// variant, since `RetargetAngles` already is exactly that (see its own doc comment
/// in `indicatrix_cut_core::Edit`). A lone tier still goes through the same
/// `RetargetAngles` path with a single-element `changes` vec, so there is only one
/// code path here rather than a single/multi split.
///
/// Applied through [`EditorState::apply_coalescing`] (not [`EditorState::apply`]) so
/// several nudges typed/scrolled in quick succession collapse into one undo step --
/// see [`angle_nudge_coalesce_key`] for how the coalescing key is derived from the
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
/// posted), so [`clamp_nudge_to_side`]'s zero-crossing clamp always judges the
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
    let mut clamped_labels: Vec<String> = Vec::new();
    let changes: Option<Vec<(usize, f64, f64)>> = targets
        .iter()
        .map(|&index| {
            st.design.tiers.get(index).map(|tier| {
                let wanted = tier.angle_deg + delta_deg;
                let nudged = clamp_nudge_to_side(tier.angle_deg, wanted);
                if nudged != wanted {
                    clamped_labels.push(tier_nudge_label(tier, index));
                }
                (index, tier.angle_deg, nudged)
            })
        })
        .collect();
    let Some(changes) = changes else {
        return;
    };
    if changes.is_empty() {
        return;
    }
    let key = angle_nudge_coalesce_key(targets);
    match st.apply_coalescing(Edit::RetargetAngles { changes }, key) {
        Ok(()) => {
            let dirty: BTreeSet<usize> = targets.iter().copied().collect();
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
            // tier belongs to (`clamp_nudge_to_side`'s own doc comment), so
            // a nudge that would cross zero is clamped there instead of
            // silently reclassifying the tier -- explain the stop instead
            // of leaving it looking like the nudge simply refused to move.
            if !clamped_labels.is_empty() {
                show_toast(
                    ui,
                    &format!(
                        "{} stopped at 0° -- nudging further would move it into the \
                         other block. Type the angle directly (e.g. \"-0\") to cross \
                         blocks on purpose.",
                        clamped_labels.join(", ")
                    ),
                    "info",
                );
            }
        }
        Err(e) => show_toast(ui, &e.to_string(), "error"),
    }
}
