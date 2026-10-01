//! One handle drag, from pointer-down to release or Escape: what the gesture captured,
//! how each throttled value reaches the design, and the live feedback (the hint line and
//! the outline on the tiers that follow).

use super::{VIEWS, place::pointer_to_pick, queue, set_hint, slice, with_views};
use crate::{
    AppWindow, ManipulateModel,
    app::{
        Ctx, coalesce_now,
        push::{MessageKind, show_message},
    },
    editor::edit::{Dirty, finish_edit},
    views::{request_refresh, state::ViewsState},
};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_editor::{
    edit_intent::EditIntent,
    manipulate::{
        ActiveDrag, DragValue, GestureInputs, HandleKind, HandleTarget, ProvisionalSlice,
        drag_value,
        gesture::{apply_step, drain_value, kind_from_int, kind_to_int, snap_mode},
        target::masts_aligned,
        text,
    },
};
use indicatrix_solid::preview::view::facets_of_tiers;
use slint::ComponentHandle;
use std::collections::BTreeSet;

/// What an applied drag step tells the rest of the app -- the path every edit ends in
/// (`editor::edit::finish_edit`): the stale rows of the tier table, the header's design
/// summary, the debounced auto-solve and save, and the views' replan (which is also what
/// lands the live re-solve the handles follow).
pub(super) fn notify_edit(ctx: &Ctx, dirty: Dirty) {
    finish_edit(ctx, dirty);
}

/// The solved masts of the last planned frame when they describe exactly `tier_count`
/// tiers (a stale cache would put every facet id on the wrong tier).
fn aligned_masts(views: &ViewsState, tier_count: usize) -> Option<Vec<SolvedTier>> {
    views
        .cache
        .as_ref()
        .filter(|(_, masts)| masts_aligned(tier_count, masts.len()))
        .map(|(_, masts)| masts.clone())
}

/// What a press on a committed tier's handle captures. It also starts a fresh
/// coalescing run: an earlier edit of this tier inside the window must not absorb the
/// gesture, and the gesture must not leak into the next edit either.
fn committed_inputs(ctx: &Ctx, target: &HandleTarget) -> Option<GestureInputs> {
    let (start_angle_deg, tier_count) = {
        let mut app = ctx.state.try_borrow_mut().ok()?;
        let design = app.design.as_mut()?;
        let angle = design.session.design.tiers.get(target.tier)?.angle_deg;
        design.session.history.end_coalesce_run();
        (angle, design.session.design.tiers.len())
    };
    let masts = VIEWS.with(|cell| aligned_masts(&cell.borrow(), tier_count));
    Some(GestureInputs {
        start_angle_deg,
        masts,
        restore: None,
    })
}

/// `ManipulateModel.drag-begin`: takes the gesture's snapshot and grants (or refuses) it.
fn begin(ctx: &Ctx, kind: i32, x: f32, y: f32) {
    let (Some(kind), Some(ui)) = (kind_from_int(kind), ctx.ui.upgrade()) else {
        return;
    };
    let grabbed = VIEWS.with(|cell| {
        let views = cell.borrow();
        views
            .manip
            .target
            .clone()
            .zip(pointer_to_pick(&views, x, y))
    });
    let Some((target, pointer)) = grabbed else {
        return;
    };
    if kind == HandleKind::Index && target.frame.is_indexless() {
        return;
    }
    let inputs = if target.provisional {
        VIEWS.with(|cell| {
            cell.borrow()
                .manip
                .provisional
                .as_ref()
                .and_then(ProvisionalSlice::gesture_inputs)
        })
    } else {
        committed_inputs(ctx, &target)
    };
    let Some(inputs) = inputs else {
        return;
    };
    let Some(drag) = ActiveDrag::begin(kind, &target, inputs, pointer, coalesce_now()) else {
        set_hint(&ui, text::NEEDS_SOLVE_HINT);
        return;
    };
    let provisional_hint = drag
        .provisional
        .then(|| {
            VIEWS.with(|cell| {
                cell.borrow()
                    .manip
                    .provisional
                    .as_ref()
                    .map(ProvisionalSlice::hint)
            })
        })
        .flatten();
    let hint = drag.opening_hint(provisional_hint);
    with_views(|views| {
        views.manip.drag = Some(drag);
        views.manip.hover_hint_active = false;
    });
    let model = ui.global::<ManipulateModel>();
    model.set_dragging(true);
    model.set_hovered_handle(kind_to_int(kind));
    set_hint(&ui, &hint);
}

/// `ManipulateModel.drag-move`: turns the pointer into a value and posts it; the queue
/// drains at most one per frame into [`apply_intent`].
fn move_to(ctx: &Ctx, x: f32, y: f32, shift: bool) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let snap = snap_mode(shift, ui.global::<ManipulateModel>().get_snap_off());
    let posted = with_views(|views| {
        let pointer = pointer_to_pick(views, x, y)?;
        let drag = views.manip.drag.as_mut()?;
        let value = drag_value(&drag.start, pointer, snap);
        drag.requested = Some(value);
        Some((drag.tier, value))
    });
    if let Some((tier, value)) = posted {
        queue::post(ctx, EditIntent::DragTier { tier, value });
    }
}

/// The queue's drain: applies the newest value of this frame.
pub(super) fn apply_intent(ctx: &Ctx, intent: &EditIntent) {
    let EditIntent::DragTier { tier, value } = *intent else {
        return;
    };
    // Taken out of the views while it is worked on, so nothing below can hit a second
    // borrow of them.
    let Some(mut drag) = with_views(|views| views.manip.drag.take()) else {
        return;
    };
    if drag.tier == tier {
        apply_value(ctx, &mut drag, value);
        if let Some(ui) = ctx.ui.upgrade() {
            with_views(|views| refresh_feedback(&ui, views, &mut drag));
            request_refresh(ctx);
        }
    }
    with_views(|views| views.manip.drag = Some(drag));
}

/// Applies `value` to the design (when it differs from the last one), then tells the app
/// and asks for the live replan of the dragged tier.
fn apply_value(ctx: &Ctx, drag: &mut ActiveDrag, value: DragValue) {
    if drag.provisional {
        // The provisional tier lives in the Slice tool's session: same throttling and
        // apply-only-on-change rule, but no `EditorSession`, history or app notice.
        let applied = with_views(|views| {
            views.manip.provisional.as_mut().is_some_and(|slice| {
                drain_value(&mut drag.progress, value, |step| slice.apply_step(step))
            })
        });
        if applied {
            slice::resubmit(ctx);
        }
        return;
    }
    let (tier, now) = (drag.tier, drag.gesture_now);
    let mut failure: Option<String> = None;
    let applied = {
        let Ok(mut app) = ctx.state.try_borrow_mut() else {
            return;
        };
        let Some(design) = app.design.as_mut() else {
            return;
        };
        drain_value(&mut drag.progress, value, |step| {
            match apply_step(&mut design.session, tier, step, now) {
                Ok(edit) => edit,
                Err(error) => {
                    failure = Some(error.to_string());
                    None
                }
            }
        })
    };
    if applied {
        notify_edit(ctx, Dirty::one(tier));
    }
    if let Some(message) = failure {
        show_message(ctx, MessageKind::Error, &message);
    }
}

/// Words the live hint ("P1 -> 41.3 deg, 3 other tiers follow") and outlines the tiers
/// whose solved mast has moved since the press.
fn refresh_feedback(ui: &AppWindow, views: &mut ViewsState, drag: &mut ActiveDrag) {
    if drag.provisional {
        if let Some(hint) = views.manip.provisional.as_ref().map(ProvisionalSlice::hint) {
            set_hint(ui, &hint);
        }
        return;
    }
    let now = views
        .cache
        .as_ref()
        .map(|(_, masts)| masts.as_slice())
        .filter(|masts| masts.len() == drag.start_masts.len());
    let (hint, moved) = drag.live_feedback(now);
    set_hint(ui, &hint);
    if moved != drag.outlined {
        let tiers: BTreeSet<usize> = moved.iter().copied().collect();
        views.overlay.moved = facets_of_tiers(&views.facet_tier, &tiers);
        views.overlay_dirty = true;
        drag.outlined = moved;
    }
}

/// A frame landed mid-drag: its masts show which tiers follow now.
pub(super) fn update_feedback(ui: &AppWindow, views: &mut ViewsState) {
    let Some(mut drag) = views.manip.drag.take() else {
        return;
    };
    refresh_feedback(ui, views, &mut drag);
    views.manip.drag = Some(drag);
}

/// Clears everything the drag showed: the outline, the hint, the lit handle.
fn finish_ui(ctx: &Ctx, drag: &ActiveDrag) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    with_views(|views| {
        views.manip.hovered = None;
        views.manip.hover_hint_active = false;
        if !drag.outlined.is_empty() {
            views.overlay.moved.clear();
        }
        // An overlay pass even when nothing was outlined: mid-drag frames left the
        // handles where the press found them, and the frame that lands now (which does
        // re-place them) may otherwise not come.
        views.overlay_dirty = true;
    });
    let model = ui.global::<ManipulateModel>();
    model.set_dragging(false);
    model.set_hovered_handle(-1);
    with_views(|views| slice::resting_hint(&ui, views));
    request_refresh(ctx);
}

/// Ends the coalescing run of the committed design's history: the gesture is its own
/// undo step and must not absorb (or be absorbed by) the next edit.
fn end_coalesce_run(ctx: &Ctx) {
    if let Ok(mut app) = ctx.state.try_borrow_mut()
        && let Some(design) = app.design.as_mut()
    {
        design.session.history.end_coalesce_run();
    }
}

/// `ManipulateModel.drag-end`: flushes the newest pointer value, keeps the gesture as
/// its one undo step and says what it did.
fn end(ctx: &Ctx) {
    let Some(mut drag) = with_views(|views| views.manip.drag.take()) else {
        return;
    };
    queue::clear();
    if let Some(value) = drag.requested {
        apply_value(ctx, &mut drag, value);
    }
    if !drag.provisional {
        end_coalesce_run(ctx);
    }
    finish_ui(ctx, &drag);
    // A provisional drag has no history entry and no toast: the hint line and the Keep
    // button are its feedback.
    if let Some(toast) = drag.done_toast() {
        show_message(ctx, MessageKind::Info, &toast);
    }
}

/// `ManipulateModel.drag-cancel`: undoes the gesture's one step, if it applied any.
fn cancel(ctx: &Ctx) {
    let Some(drag) = with_views(|views| views.manip.drag.take()) else {
        return;
    };
    queue::clear();
    if drag.provisional {
        if let Some(snapshot) = drag.restore.clone() {
            slice::restore_tier(ctx, snapshot);
        }
    } else if drag.progress.applied_any {
        undo_gesture(ctx, &drag);
    } else {
        end_coalesce_run(ctx);
    }
    finish_ui(ctx, &drag);
}

/// What undoing a cancelled gesture came to.
enum Undone {
    /// Something else changed the design since the gesture's last edit.
    Skipped,
    /// The gesture's step was undone.
    Restored,
    /// Nothing to undo.
    Nothing,
    /// The undo failed.
    Failed(String),
}

/// Undoes the gesture's coalesced step, provided it is still the newest thing that
/// happened to the design, and redraws everything from a replan (the same refresh the
/// Undo button runs).
fn undo_gesture(ctx: &Ctx, drag: &ActiveDrag) {
    let outcome = {
        let Ok(mut app) = ctx.state.try_borrow_mut() else {
            return;
        };
        let Some(design) = app.design.as_mut() else {
            return;
        };
        design.session.history.end_coalesce_run();
        if Some(design.session.current_generation()) == drag.progress.applied_generation {
            match design.session.undo() {
                Ok(Some(_)) => Undone::Restored,
                Ok(None) => Undone::Nothing,
                Err(error) => Undone::Failed(error.to_string()),
            }
        } else {
            Undone::Skipped
        }
    };
    match outcome {
        Undone::Skipped => show_message(ctx, MessageKind::Info, text::DRAG_CANCEL_SKIPPED_TOAST),
        Undone::Restored => {
            notify_edit(ctx, Dirty::All);
            show_message(ctx, MessageKind::Info, text::DRAG_CANCELLED_TOAST);
        }
        Undone::Nothing => {}
        Undone::Failed(error) => show_message(
            ctx,
            MessageKind::Error,
            &text::drag_cancel_failed_toast(&error),
        ),
    }
}

/// Wires the drag callbacks: begin, move (through the 16 ms intent queue), end, cancel.
pub(super) fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<ManipulateModel>();
    let c = ctx.clone();
    model.on_drag_begin(move |kind, x, y| begin(&c, kind, x, y));
    let c = ctx.clone();
    model.on_drag_move(move |x, y, shift| move_to(&c, x, y, shift));
    let c = ctx.clone();
    model.on_drag_end(move || end(&c));
    let c = ctx.clone();
    model.on_drag_cancel(move || cancel(&c));
}
