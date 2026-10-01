//! The Slice tool: a line dragged across the stone becomes a PROVISIONAL facet, tweaked
//! with the same three handles, then kept (one `Edit::AddTier`) or discarded.
//!
//! The session itself is `indicatrix_editor::manipulate::ProvisionalSlice` (shared with
//! the desktop): a design clone with the new tier appended, edited in place. This module
//! is the web wiring around it -- the line gesture, the buttons, and the rules that end
//! a session ([`expire`]). The views render the clone in place of the committed design
//! while it exists (`refresh::replan_provisional`).

use super::{
    VIEWS, cutoff, drag::notify_edit, place, place::pointer_to_pick, set_hint, with_views,
};
use crate::{
    AppWindow, ManipulateModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
    },
    editor::edit::Dirty,
    views::{request_refresh, select_tier, state::ViewsState},
};
use indicatrix_cut_core::{ConstraintTier, Edit};
use indicatrix_editor::manipulate::{
    DiscardReason, ProvisionalSlice, SliceLine, SliceSide, Snapshot,
    gesture::tier_label,
    provisional::{CornerPoints, resting_hint as resting_hint_text, session_outlives},
    text,
};
use slint::ComponentHandle;
use std::sync::Arc;

/// Writes the hint that belongs to no handle: the Cut slider's warning, the provisional
/// tier's, else Slice mode's, else nothing.
pub(super) fn resting_hint(ui: &AppWindow, views: &ViewsState) {
    let model = ui.global::<ManipulateModel>();
    let hint = resting_hint_text(
        views.manip.provisional.as_ref(),
        cutoff(ui),
        model.get_slice_mode(),
        model.get_slice_symmetric(),
    );
    set_hint(ui, &hint);
}

/// Turns a shown provisional frame's masts into the green outline (and refreshes the
/// hint's facet count). Returns whether anything new landed. Cheap when nothing is new.
pub(super) fn sync_outline(views: &mut ViewsState) -> bool {
    let Some(session) = views.manip.provisional.as_mut() else {
        return false;
    };
    let Some((changed, ids)) = session.take_outline_update(views.geometry.as_ref()) else {
        return false;
    };
    if changed {
        views.overlay.provisional = ids;
        views.overlay_dirty = true;
    }
    true
}

/// Renders the provisional design again: a replan stamped `PROVISIONAL_GENERATION`
/// (`refresh::replan_provisional`), chained from the session's own masts.
pub(super) fn resubmit(ctx: &Ctx) {
    with_views(|views| {
        views.force_replan = true;
        views.manip.provisional_followup = false;
    });
    request_refresh(ctx);
}

/// Puts the provisional tier back the way a handle press found it (Escape mid-drag) and
/// re-renders.
pub(super) fn restore_tier(ctx: &Ctx, snapshot: Snapshot) {
    let restored = with_views(|views| {
        views
            .manip
            .provisional
            .as_mut()
            .is_some_and(|session| session.restore(snapshot))
    });
    if restored {
        resubmit(ctx);
    }
}

/// Removes the session and everything drawn for it (outline, a drag on it, the
/// buttons), and owes the views a replan of the committed design.
fn tear_down(ui: &AppWindow, views: &mut ViewsState) -> Option<ProvisionalSlice> {
    let session = views.manip.provisional.take()?;
    views.manip.provisional_selected = None;
    views.manip.provisional_followup = false;
    let dragging_it = views.manip.drag.as_ref().is_some_and(|d| d.provisional);
    if dragging_it {
        views.manip.drag = None;
    }
    let model = ui.global::<ManipulateModel>();
    model.set_provisional_active(false);
    model.set_provisional_label("".into());
    if dragging_it {
        model.set_dragging(false);
        model.set_hovered_handle(-1);
    }
    views.overlay.provisional.clear();
    views.overlay_dirty = true;
    views.force_replan = true;
    Some(session)
}

/// Drops the provisional slice (Discard, Escape, leaving Slice mode, an edit underneath
/// it). A no-op without a session.
fn discard(ctx: &Ctx, reason: DiscardReason) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let Some(session) = with_views(|views| tear_down(&ui, views)) else {
        return;
    };
    let label = session.label().unwrap_or_default();
    with_views(|views| {
        // The selected facet's own handles come back with the next frame.
        place::hide(&ui, views);
        resting_hint(&ui, views);
    });
    request_refresh(ctx);
    show_message(ctx, MessageKind::Info, &reason.toast(&label));
}

/// Ends the session when the committed design or the selected tier has moved on since it
/// began (a provisional slice cannot outlive either). Runs at the top of every
/// `refresh`, before it looks at the app.
pub fn expire(ctx: &Ctx) {
    let reason = {
        let Ok(app) = ctx.state.try_borrow() else {
            return;
        };
        VIEWS.with(|cell| {
            let views = cell.borrow();
            let session = views.manip.provisional.as_ref()?;
            if !session_outlives(session.base_generation, app.generation().unwrap_or(0)) {
                Some(DiscardReason::DesignChanged)
            } else if app.selected_tier != views.manip.provisional_selected {
                Some(DiscardReason::SelectionChanged)
            } else {
                None
            }
        })
    };
    if let Some(reason) = reason {
        discard(ctx, reason);
    }
}

/// Discards the session when the committed design has moved on since it began (checked
/// before every button, for a press between two `refresh` polls). Returns whether it did.
fn expire_if_stale(ctx: &Ctx) -> bool {
    let stale = {
        let Ok(app) = ctx.state.try_borrow() else {
            return false;
        };
        VIEWS.with(|cell| {
            cell.borrow()
                .manip
                .provisional
                .as_ref()
                .is_some_and(|session| {
                    !session_outlives(session.base_generation, app.generation().unwrap_or(0))
                })
        })
    };
    if stale {
        discard(ctx, DiscardReason::DesignChanged);
    }
    stale
}

/// `ManipulateModel.slice-toggled`: `slice-mode` already holds its new value.
fn toggled(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    if ui.global::<ManipulateModel>().get_slice_mode() {
        with_views(|views| views.manip.hover_hint_active = false);
    } else {
        cancel_gesture(ctx);
        discard(ctx, DiscardReason::User);
    }
    // Slice mode shows only the provisional tier's handles; leaving it brings the
    // selected facet's own back.
    place::refresh_now(ctx);
    with_views(|views| resting_hint(&ui, views));
}

/// Shows the rubber band from `start` to `end` (logical coordinates).
fn set_line(ui: &AppWindow, start: (f32, f32), end: (f32, f32)) {
    let model = ui.global::<ManipulateModel>();
    model.set_line_x0(start.0);
    model.set_line_y0(start.1);
    model.set_line_x1(end.0);
    model.set_line_y1(end.1);
    model.set_slice_line_visible(true);
}

/// `ManipulateModel.slice-begin`: the press that starts a line.
fn begin(ctx: &Ctx, x: f32, y: f32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    if !ui.global::<ManipulateModel>().get_slice_mode() {
        return;
    }
    with_views(|views| views.manip.slice_gesture = Some((x, y)));
    set_line(&ui, (x, y), (x, y));
}

/// `ManipulateModel.slice-move`: the pointer moved while drawing the line.
fn move_to(ctx: &Ctx, x: f32, y: f32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    if let Some(start) = VIEWS.with(|cell| cell.borrow().manip.slice_gesture) {
        set_line(&ui, start, (x, y));
    }
}

/// `ManipulateModel.slice-cancel`: Escape (or a cancelled pointer) mid-line.
fn cancel_gesture(ctx: &Ctx) {
    with_views(|views| views.manip.slice_gesture = None);
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<ManipulateModel>().set_slice_line_visible(false);
    }
}

/// `ManipulateModel.slice-end`: the release that finishes the line -- the cut plane is
/// snapped and the provisional tier appears.
fn end(ctx: &Ctx, x: f32, y: f32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let Some(start) = with_views(|views| views.manip.slice_gesture.take()) else {
        return;
    };
    ui.global::<ManipulateModel>().set_slice_line_visible(false);
    let drawn = with_views(|views| {
        let geometry = views.geometry.as_ref()?;
        let line = SliceLine::new(
            pointer_to_pick(views, start.0, start.1)?,
            pointer_to_pick(views, x, y)?,
            geometry,
        );
        // A second line replaces the first, against the same committed stone.
        let corners = views.manip.provisional.as_ref().map_or_else(
            || Arc::clone(&geometry.corner_points),
            |session| Arc::clone(&session.base_corners),
        );
        Some((line, corners))
    });
    let Some((line, corners)) = drawn else {
        set_hint(&ui, text::SLICE_NEEDS_PICTURE_HINT);
        return;
    };
    install(ctx, line, SliceSide::Right, corners);
}

/// Builds the session for `line` cutting away `side`, renders it and words the hint. A
/// degenerate line leaves everything as it was and says why.
fn install(ctx: &Ctx, line: SliceLine, side: SliceSide, corners: CornerPoints) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<ManipulateModel>();
    let symmetric = model.get_slice_symmetric();
    let built = {
        let app = ctx.state.borrow();
        app.design
            .as_ref()
            .and_then(|design| {
                ProvisionalSlice::build(
                    line,
                    side,
                    corners,
                    &design.session.design,
                    design.session.current_generation(),
                    symmetric,
                )
            })
            .map(|session| (session, app.selected_tier))
    };
    let Some((session, selected)) = built else {
        set_hint(&ui, text::SLICE_TOO_SHORT_HINT);
        return;
    };
    let label = session.label().unwrap_or_default();
    with_views(|views| {
        views.manip.hover_hint_active = false;
        views.manip.provisional = Some(session);
        views.manip.provisional_selected = selected;
        views.manip.provisional_followup = false;
        views.force_replan = true;
    });
    model.set_provisional_label(label.as_str().into());
    model.set_provisional_active(true);
    request_refresh(ctx);
    with_views(|views| resting_hint(&ui, views));
}

/// `ManipulateModel.slice-flip`: the same line, the other side cut away.
fn flip(ctx: &Ctx) {
    if expire_if_stale(ctx) {
        return;
    }
    let again = VIEWS.with(|cell| {
        cell.borrow().manip.provisional.as_ref().map(|session| {
            (
                session.line,
                session.side.flipped(),
                Arc::clone(&session.base_corners),
            )
        })
    });
    if let Some((line, side, corners)) = again {
        install(ctx, line, side, corners);
    }
}

/// `ManipulateModel.slice-symmetric-toggled`: `slice-symmetric` holds its new value.
fn symmetric_toggled(ctx: &Ctx) {
    if expire_if_stale(ctx) {
        return;
    }
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let symmetric = ui.global::<ManipulateModel>().get_slice_symmetric();
    let rebuilt = with_views(|views| {
        views
            .manip
            .provisional
            .as_mut()
            .is_some_and(|session| session.rebuild_indices(symmetric))
    });
    if rebuilt {
        resubmit(ctx);
    }
    with_views(|views| resting_hint(&ui, views));
}

/// `ManipulateModel.slice-keep`: commits the provisional tier as ONE `Edit::AddTier`.
fn keep(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    if ui.global::<ManipulateModel>().get_dragging() || expire_if_stale(ctx) {
        return;
    }
    let Some((tier, facet_count, may_keep)) = VIEWS.with(|cell| {
        cell.borrow()
            .manip
            .provisional
            .as_ref()
            .and_then(|session| {
                Some((
                    session.tier()?.clone(),
                    session.facet_count(),
                    session.may_keep(),
                ))
            })
    }) else {
        return;
    };
    // A freshly sliced tier sits at the tangency mast: its facet has no area, so keeping
    // it would add a tier that cuts nothing. Enter and the button both land here.
    if !may_keep {
        set_hint(&ui, text::SLICE_NO_DEPTH_HINT);
        show_message(ctx, MessageKind::Info, text::SLICE_NO_DEPTH_HINT);
        return;
    }
    let applied = {
        let Ok(mut app) = ctx.state.try_borrow_mut() else {
            return;
        };
        let Some(design) = app.design.as_mut() else {
            return;
        };
        let index = design.session.design.tiers.len();
        design.session.history.end_coalesce_run();
        design
            .session
            .apply(Edit::AddTier {
                index,
                tier: tier.clone(),
            })
            .map(|_| index)
    };
    match applied {
        Ok(index) => finish_keep(ctx, &ui, index, &tier, facet_count),
        Err(error) => show_message(ctx, MessageKind::Error, &error.to_string()),
    }
}

/// After a successful Keep: drops the session, selects the new row, leaves Slice mode and
/// says what happened (and that Undo removes it).
fn finish_keep(ctx: &Ctx, ui: &AppWindow, index: usize, tier: &ConstraintTier, count: usize) {
    drop(with_views(|views| tear_down(ui, views)));
    ui.global::<ManipulateModel>().set_slice_mode(false);
    // The new row is selected before the edit's refresh, so the tier table and the views
    // both land on it.
    select_tier(ctx, Some(index));
    notify_edit(ctx, Dirty::one(index));
    with_views(|views| {
        place::hide(ui, views);
        resting_hint(ui, views);
    });
    let toast = text::slice_kept_toast(&tier_label(tier, index), count, tier.angle_deg);
    show_message(ctx, MessageKind::Info, &toast);
}

/// Wires the Slice tool: mode toggle, the line gesture, and the provisional tier's
/// buttons.
pub(super) fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<ManipulateModel>();
    let c = ctx.clone();
    model.on_slice_toggled(move || toggled(&c));
    let c = ctx.clone();
    model.on_slice_begin(move |x, y| begin(&c, x, y));
    let c = ctx.clone();
    model.on_slice_move(move |x, y| move_to(&c, x, y));
    let c = ctx.clone();
    model.on_slice_end(move |x, y| end(&c, x, y));
    let c = ctx.clone();
    model.on_slice_cancel(move || cancel_gesture(&c));
    let c = ctx.clone();
    model.on_slice_flip(move || flip(&c));
    let c = ctx.clone();
    model.on_slice_keep(move || keep(&c));
    let c = ctx.clone();
    model.on_slice_discard(move || discard(&c, DiscardReason::User));
    let c = ctx.clone();
    model.on_slice_symmetric_toggled(move || symmetric_toggled(&c));
}
