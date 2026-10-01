//! The Slice tool: a line dragged across the stone becomes a PROVISIONAL facet, tweaked
//! with the same three handles, then kept (one `Edit::AddTier`) or discarded.
//!
//! # The provisional session
//!
//! [`ProvisionalSlice`] lives in the manipulation session ([`super::SESSION`]), never in
//! `EditorState`: it owns a clone of the committed design with the new tier appended at
//! `len()`, and it is rendered by submitting a replan of that clone under
//! [`PROVISIONAL_GENERATION`]. `gui::solid_sink` recognises the generation and keeps the
//! frame out of `solid_last_solved`, the tier table and the path tracer's planes, so
//! only the pick buffer, the hover/tier tables and the geometry (what is on screen)
//! follow it. The frame's solved masts reach the session through
//! [`note_masts`], because the sink does not cache them.
//!
//! A drag on the provisional facet edits the clone in place ([`apply_step`]) and
//! resubmits ([`resubmit`]): no history entry, no toast. While the session exists the
//! committed design must not change underneath it: [`on_landed`] compares the session's
//! `base_generation` with the editor's on every landed frame ([`expire_if_stale`] does
//! the same before a button).
//!
//! # Keeping the provisional picture on screen
//!
//! Three things could put the committed stone back over the provisional facet, and each
//! is answered here:
//!
//! - a frame of the COMMITTED design landing (the idle replan, a background solve, a
//!   selection replan): [`on_landed`] sees its generation and resubmits the provisional
//!   design ([`landed_action`]); a provisional frame can never cause that in turn;
//! - a redraw that reprojects the committed planes (a camera orbit, a view-mode
//!   switch, a solve landing): [`note_planes`] hands the provisional frame's planes to
//!   `SolidPreviewState::set_planes_override`, so every such redraw draws them;
//! - a replan that rebuilds the render style: the green outline is stored with
//!   `SolidPreviewState::set_outlines` and stamped on every draw.

mod keep;
mod provisional;

use super::{
    CTX, PROVISIONAL_GENERATION, SESSION, Shared, Target,
    drag::{AppliedEdit, GestureInputs, Step},
    frame_updates_mast_cache, handles, set_hint, tier_label,
};
use crate::{
    MainWindow, ManipulateModel, SolidPreviewModel,
    gui::{
        editor::{
            callbacks::resubmit_facet_overlay,
            view::{ReplanSource, submit_preview_replan, submit_preview_replan_chained},
        },
        show_toast,
        solid_preview::preview_state::FrameGeometry,
    },
};
use glam::Vec3;
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_editor::manipulate::{SliceSide, text};
use provisional::{ProvisionalSlice, rebuild_indices, set_angle, set_mast, turn};
use slint::ComponentHandle as _;
use std::{
    collections::BTreeSet,
    sync::{Arc, PoisonError},
};

pub(super) use keep::keep;
pub(super) use provisional::{
    DiscardReason, LandedAction, SliceLine, SliceState, Snapshot, cut_hides_tier, landed_action,
    plan_slice, replan_chain, session_outlives, surviving_facets, with_provisional_tier,
};
#[cfg(test)]
pub(super) use provisional::{keep_allowed, wheel_turn};

/// The hint after a line too short to define a plane.
const TOO_SHORT_HINT: &str =
    "That line is too short to cut a facet. Drag a longer line across the stone.";

/// The hint when there is no picture of the stone yet.
const NEEDS_PICTURE_HINT: &str =
    "Nothing to slice yet: solve the design first so the stone is on screen.";

/// The toast when an edit of the committed design ended the session.
const CHANGED_TOAST: &str = "Slice discarded -- the design changed";

/// The toast when a tier-list selection ended the session.
const SELECTION_TOAST: &str = "Slice discarded -- another tier was selected";

/// What the hint line and the refused Keep say while the provisional tier has no
/// facet on the stone: a fresh slice sits exactly at the tangency mast, so it cuts
/// nothing until its depth handle is dragged inward.
const NO_DEPTH_HINT: &str =
    "Drag the depth handle inward first -- the facet does not touch the stone yet";

/// The hint while the Cut slider hides the provisional tier (the mesh then holds only
/// the first tiers' planes).
const CUT_SLIDER_HINT: &str =
    "The Cut slider is hiding the new facet: move it to the end to see and adjust it.";

/// Runs `f` on the provisional session, if any. `f` must not touch the session itself.
fn with_provisional<R>(f: impl FnOnce(&mut ProvisionalSlice) -> R) -> Option<R> {
    SESSION.with(|cell| cell.borrow_mut().slice.provisional.as_mut().map(f))
}

/// [`with_provisional`] for a read-only look.
fn with_provisional_ref<R>(f: impl FnOnce(&ProvisionalSlice) -> R) -> Option<R> {
    SESSION.with(|cell| cell.borrow().slice.provisional.as_ref().map(f))
}

/// Whether a provisional slice exists.
pub(super) fn is_active() -> bool {
    SESSION.with(|cell| cell.borrow().slice.provisional.is_some())
}

/// The hint for the provisional tier as it stands.
fn provisional_hint(p: &ProvisionalSlice) -> String {
    let Some(tier) = p.tier() else {
        return String::new();
    };
    if p.surviving == Some(0) {
        return format!(
            "New tier {}: {NO_DEPTH_HINT}. Esc discards it.",
            tier_label(tier, p.tier_index)
        );
    }
    text::slice_provisional_hint(
        &tier_label(tier, p.tier_index),
        p.facet_count(),
        tier.angle_deg,
        p.snapped.index,
    )
}

/// The provisional hint, or `None` without a session.
pub(super) fn provisional_hint_text() -> Option<String> {
    with_provisional_ref(provisional_hint)
}

/// Writes the hint that belongs to no handle: the provisional tier's, else Slice mode's,
/// else nothing.
pub(super) fn resting_hint(ui: &MainWindow) {
    let model = ui.global::<ManipulateModel>();
    let cutoff = ui.global::<SolidPreviewModel>().get_tier_cutoff();
    let hidden_by_cut = with_provisional_ref(|p| cut_hides_tier(cutoff, p.tier_index));
    let hint = hidden_by_cut
        .filter(|&hidden| hidden)
        .map(|_| CUT_SLIDER_HINT.to_string())
        .or_else(provisional_hint_text)
        .or_else(|| {
            model
                .get_slice_mode()
                .then(|| text::slice_mode_hint(model.get_slice_symmetric()))
        })
        .unwrap_or_default();
    set_hint(ui, &hint);
}

/// The handle target on the provisional tier for the frame `geometry`, or `None`
/// without a session or before a frame's masts describe it.
pub(super) fn place_provisional(geometry: &FrameGeometry) -> Option<Target> {
    with_provisional(|p| {
        let map = p.facet_map()?;
        handles::target_for(geometry, &map, &p.design, p.tier_index, None, true)
    })
    .flatten()
}

/// What a handle press on the provisional tier starts from.
pub(super) fn gesture_inputs() -> Option<GestureInputs> {
    with_provisional_ref(|p| {
        let tier = p.tier()?;
        Some(GestureInputs {
            start_angle_deg: tier.angle_deg,
            masts: p.masts.clone(),
            restore: Some(Snapshot {
                tier: tier.clone(),
                snapped: p.snapped,
            }),
        })
    })
    .flatten()
}

/// The solved masts of a provisional-generation frame (see `gui::solid_sink`). Kept only
/// when they describe the session's design one to one.
pub(super) fn note_masts(masts: Vec<SolvedTier>) {
    with_provisional(|p| {
        if masts.len() == p.design.tiers.len() {
            p.masts = Some(masts);
            p.facet_map = None;
            p.masts_new = true;
        }
    });
}

/// Turns a landed provisional frame's masts into the green outline (and refreshes the
/// hint's facet count). Cheap when nothing is new.
pub(super) fn sync_outline(ui: &MainWindow, ctx: &Shared) {
    let geometry = ctx
        .geometry
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let outcome = with_provisional(|p| {
        if !std::mem::take(&mut p.masts_new) {
            return None;
        }
        let map = p.facet_map()?;
        let ids = map.facets_of_tier(p.tier_index).to_vec();
        // Which of the tier's planes touch the stone in the frame on screen.
        if let Some(geometry) = &geometry {
            p.surviving = surviving_facets(&ids, &geometry.facet_centroids, map.facet_count());
        }
        let changed = ids != p.outline;
        if changed {
            p.outline.clone_from(&ids);
        }
        Some((changed, ids))
    })
    .flatten();
    let Some((changed, ids)) = outcome else {
        return;
    };
    if changed {
        resubmit_facet_overlay(&ctx.preview_state, move |overlay| overlay.provisional = ids);
    }
    let hover_active = SESSION.with(|cell| cell.borrow().hover_hint_active);
    if !ui.global::<ManipulateModel>().get_dragging() && !hover_active {
        resting_hint(ui);
    }
}

/// Applies one throttled handle step to the provisional tier in place (no history, no
/// panel refresh). `None` when nothing changed.
pub(super) fn apply_step(step: Step) -> Option<AppliedEdit> {
    with_provisional(|p| {
        let changed = match step {
            Step::Angle(deg) => set_angle(p, deg),
            Step::Mast(mast) => set_mast(p, mast),
            Step::Teeth(more) => turn(p, more),
        };
        if changed {
            p.facet_map = None;
        }
        changed.then_some(AppliedEdit {
            generation: 0,
            replaced_meet: None,
        })
    })
    .flatten()
}

/// Puts the provisional tier back the way a handle press found it (Escape mid-drag) and
/// re-renders.
pub(super) fn restore_tier(ui: &MainWindow, ctx: &Shared, snapshot: Snapshot) {
    let restored = with_provisional(|p| {
        let index = p.tier_index;
        let Some(slot) = Arc::make_mut(&mut p.design).tiers.get_mut(index) else {
            return false;
        };
        *slot = snapshot.tier;
        p.snapped = snapshot.snapped;
        p.facet_map = None;
        true
    })
    .unwrap_or(false);
    if restored {
        resubmit(ui, ctx);
    }
}

/// Renders the provisional design: a replan stamped [`PROVISIONAL_GENERATION`], so the
/// sink and `submit_preview_replan_chained` keep it out of the committed caches. Once
/// the session holds the masts of a landed provisional frame the replan chains from
/// them with `dirty = {provisional tier}` (a subgraph re-solve that fits the preview
/// budget on a large design); the first replan after a session begins (or a Flip) is a
/// full solve -- see [`replan_chain`].
pub(super) fn resubmit(ui: &MainWindow, ctx: &Shared) {
    let Some((design, last_solved, dirty)) = with_provisional(|p| {
        p.awaiting = true;
        let (last_solved, dirty) =
            replan_chain(p.masts.as_deref(), p.design.tiers.len(), p.tier_index);
        (Arc::clone(&p.design), last_solved, dirty)
    }) else {
        return;
    };
    submit_preview_replan_chained(
        ui,
        &ctx.render_ctx,
        &ctx.preview_state,
        ReplanSource {
            design: &design,
            generation: PROVISIONAL_GENERATION,
            multi_selected: &BTreeSet::new(),
        },
        dirty,
        last_solved,
    );
}

/// Re-renders the committed design (the provisional outline goes away).
fn replan_committed(ui: &MainWindow, ctx: &Shared) {
    if let Ok(st) = ctx.state.try_borrow() {
        submit_preview_replan(
            ui,
            &ctx.render_ctx,
            &ctx.preview_state,
            &ctx.solid_last_solved,
            &st,
            BTreeSet::new(),
            false,
        );
    }
}

/// Removes the session and everything drawn for it (outline, a drag on it, the buttons).
fn tear_down(ui: &MainWindow, ctx: &Shared) -> Option<ProvisionalSlice> {
    let (session, dragging_it) = SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        let taken = session.slice.provisional.take();
        let dragging_it = taken.is_some() && session.drag.as_ref().is_some_and(|d| d.provisional);
        if dragging_it {
            session.drag = None;
        }
        (taken, dragging_it)
    });
    let session = session?;
    let model = ui.global::<ManipulateModel>();
    model.set_provisional_active(false);
    model.set_provisional_label("".into());
    if dragging_it {
        model.set_dragging(false);
        model.set_hovered_handle(-1);
    }
    resubmit_facet_overlay(&ctx.preview_state, |overlay| overlay.provisional.clear());
    // Redraws reproject the committed planes again.
    ctx.preview_state.set_planes_override(None);
    Some(session)
}

/// Drops the provisional slice (Discard, Escape, leaving Slice mode, an edit underneath
/// it). A no-op without a session.
pub(super) fn discard(ui: &MainWindow, ctx: &Shared, reason: DiscardReason) {
    let Some(session) = tear_down(ui, ctx) else {
        return;
    };
    let label = session
        .tier()
        .map_or_else(String::new, |tier| tier_label(tier, session.tier_index));
    if reason.replans() {
        replan_committed(ui, ctx);
    }
    handles::refresh_handles(ui, ctx);
    resting_hint(ui);
    let toast = match reason {
        DiscardReason::User => text::slice_discarded_toast(&label),
        DiscardReason::DesignChanged => CHANGED_TOAST.to_string(),
        DiscardReason::SelectionChanged => SELECTION_TOAST.to_string(),
    };
    show_toast(ui, &toast, "info");
}

/// A tier-list selection change is about to replan the committed design: end the
/// session first (its replan is the caller's).
pub(super) fn drop_for_selection(ui: &MainWindow) {
    let Some(ctx) = CTX.with(|cell| cell.borrow().clone()) else {
        return;
    };
    discard(ui, &ctx, DiscardReason::SelectionChanged);
}

/// Discards the session when the committed design has moved on since it began. Returns
/// whether it did.
pub(super) fn expire_if_stale(ui: &MainWindow, ctx: &Shared) -> bool {
    let Some(base) = with_provisional_ref(|p| p.base_generation) else {
        return false;
    };
    let stale = ctx
        .state
        .try_borrow()
        .is_ok_and(|st| !session_outlives(base, st.current_generation()));
    if stale {
        discard(ui, ctx, DiscardReason::DesignChanged);
    }
    stale
}

/// A solid-preview frame stamped `frame_generation` has landed: keeps a live provisional
/// session on screen. A frame of the COMMITTED design (an idle replan, a background
/// solve, a tier selection's replan) has replaced the provisional picture, so the
/// provisional design is rendered again (cheap: the plan chains from the session's own
/// masts); a session whose committed design moved on is discarded. Returns whether it
/// was discarded.
pub(super) fn on_landed(ui: &MainWindow, ctx: &Shared, frame_generation: u64) -> bool {
    let Some((base, awaiting)) = with_provisional_ref(|p| (p.base_generation, p.awaiting)) else {
        return false;
    };
    // A provisional-generation frame is the answer to the last provisional replan.
    if !frame_updates_mast_cache(frame_generation) {
        with_provisional(|p| p.awaiting = false);
    }
    // A design that is being edited right now cannot have moved on: assume it has not.
    let current = ctx
        .state
        .try_borrow()
        .map_or(base, |st| st.current_generation());
    match landed_action(Some(base), awaiting, current, frame_generation) {
        LandedAction::Ignore => false,
        LandedAction::Resubmit => {
            resubmit(ui, ctx);
            false
        }
        LandedAction::Discard => {
            discard(ui, ctx, DiscardReason::DesignChanged);
            true
        }
    }
}

/// Stops waiting for a provisional frame: a committed replan took the plan gate's slot.
pub(super) fn clear_awaiting() {
    with_provisional(|p| p.awaiting = false);
}

/// The planes a provisional-generation frame was drawn from (see
/// `gui::solid_sink`): while the session exists, every reprojecting redraw (a camera
/// orbit, a view-mode switch, a background solve) draws THESE instead of the committed
/// `RenderContext::active_planes`, so the provisional facet stays on screen. Only planes
/// that match the provisional design one to one are taken -- the rare provisional-
/// generation frame that carries committed planes (a redraw racing the first
/// provisional replan) is ignored.
pub(super) fn note_planes(planes: &[(Vec3, f32)]) {
    let accepted = with_provisional(|p| {
        let map = p.facet_map()?;
        (map.facet_count() == planes.len()).then(|| planes.to_vec())
    })
    .flatten();
    let Some(planes) = accepted else {
        return;
    };
    if let Some(ctx) = CTX.with(|cell| cell.borrow().clone()) {
        ctx.preview_state.set_planes_override(Some(planes));
    }
}

/// `ManipulateModel.slice_toggled`: `slice_mode` already holds its new value.
pub(super) fn toggled(ui: &MainWindow, ctx: &Shared) {
    if ui.global::<ManipulateModel>().get_slice_mode() {
        SESSION.with(|cell| cell.borrow_mut().hover_hint_active = false);
    } else {
        cancel_gesture(ui);
        discard(ui, ctx, DiscardReason::User);
    }
    // Slice mode shows only the provisional tier's handles; leaving it brings the
    // selected facet's own back.
    handles::refresh_handles(ui, ctx);
    resting_hint(ui);
}

/// Shows the rubber band from `start` to `end` (logical coordinates).
fn set_line(ui: &MainWindow, start: (f32, f32), end: (f32, f32)) {
    let model = ui.global::<ManipulateModel>();
    model.set_line_x0(start.0);
    model.set_line_y0(start.1);
    model.set_line_x1(end.0);
    model.set_line_y1(end.1);
    model.set_slice_line_visible(true);
}

/// `ManipulateModel.slice_begin`: the press that starts a line.
pub(super) fn begin(ui: &MainWindow, x: f32, y: f32) {
    if !ui.global::<ManipulateModel>().get_slice_mode() {
        return;
    }
    SESSION.with(|cell| cell.borrow_mut().slice.gesture = Some((x, y)));
    set_line(ui, (x, y), (x, y));
}

/// `ManipulateModel.slice_move`: the pointer moved while drawing the line.
pub(super) fn move_to(ui: &MainWindow, x: f32, y: f32) {
    if let Some(start) = SESSION.with(|cell| cell.borrow().slice.gesture) {
        set_line(ui, start, (x, y));
    }
}

/// `ManipulateModel.slice_cancel`: Escape (or a cancelled pointer) mid-line.
pub(super) fn cancel_gesture(ui: &MainWindow) {
    SESSION.with(|cell| cell.borrow_mut().slice.gesture = None);
    ui.global::<ManipulateModel>().set_slice_line_visible(false);
}

/// `ManipulateModel.slice_end`: the release that finishes the line -- the cut plane is
/// snapped and the provisional tier appears.
pub(super) fn end(ui: &MainWindow, ctx: &Shared, x: f32, y: f32) {
    let Some(start) = SESSION.with(|cell| cell.borrow_mut().slice.gesture.take()) else {
        return;
    };
    ui.global::<ManipulateModel>().set_slice_line_visible(false);
    let geometry = ctx
        .geometry
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let Some(geometry) = geometry else {
        set_hint(ui, NEEDS_PICTURE_HINT);
        return;
    };
    let line = SliceLine::new(
        handles::pointer_to_pick(ui, start.0, start.1),
        handles::pointer_to_pick(ui, x, y),
        &geometry,
    );
    // A second line replaces the first, against the same committed stone.
    let corners = with_provisional_ref(|p| Arc::clone(&p.base_corners))
        .unwrap_or_else(|| Arc::clone(&geometry.corner_points));
    install(ui, ctx, line, SliceSide::Right, corners);
}

/// Builds the session for `line` cutting away `side`, renders it and words the hint.
/// A degenerate line leaves everything as it was and says why.
fn install(
    ui: &MainWindow,
    ctx: &Shared,
    line: SliceLine,
    side: SliceSide,
    corners: Arc<Vec<Vec3>>,
) {
    let model = ui.global::<ManipulateModel>();
    let symmetric = model.get_slice_symmetric();
    let built = ctx.state.try_borrow().ok().and_then(|st| {
        let plan = plan_slice(&line, side, &corners, &st.design, symmetric)?;
        let (design, tier_index) = with_provisional_tier(&st.design, plan.tier)?;
        let label = tier_label(design.tiers.get(tier_index)?, tier_index);
        Some((
            design,
            tier_index,
            plan.snapped,
            st.current_generation(),
            label,
        ))
    });
    let Some((design, tier_index, snapped, base_generation, label)) = built else {
        set_hint(ui, TOO_SHORT_HINT);
        return;
    };
    SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        session.hover_hint_active = false;
        session.slice.provisional = Some(ProvisionalSlice::new(
            base_generation,
            design,
            tier_index,
            side,
            snapped,
            line,
            corners,
        ));
    });
    // A replaced session (a second line, Flip) must not keep drawing the old planes.
    ctx.preview_state.set_planes_override(None);
    model.set_provisional_label(label.into());
    model.set_provisional_active(true);
    resubmit(ui, ctx);
    resting_hint(ui);
}

/// `ManipulateModel.slice_flip`: the same line, the other side cut away.
pub(super) fn flip(ui: &MainWindow, ctx: &Shared) {
    if expire_if_stale(ui, ctx) {
        return;
    }
    let Some((line, side, corners)) =
        with_provisional_ref(|p| (p.line, p.side.flipped(), Arc::clone(&p.base_corners)))
    else {
        return;
    };
    install(ui, ctx, line, side, corners);
}

/// `ManipulateModel.slice_symmetric_toggled`: `slice_symmetric` holds its new value.
pub(super) fn symmetric_toggled(ui: &MainWindow, ctx: &Shared) {
    if expire_if_stale(ui, ctx) {
        return;
    }
    let symmetric = ui.global::<ManipulateModel>().get_slice_symmetric();
    if with_provisional(|p| rebuild_indices(p, symmetric)).unwrap_or(false) {
        resubmit(ui, ctx);
    }
    resting_hint(ui);
}
