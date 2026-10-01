//! One handle drag, from pointer-down to release or Escape: what the gesture captured,
//! how each throttled value reaches the design, and the live feedback (the hint line
//! and the outline on the tiers that follow).

use super::{
    SESSION, Shared, Target, handles, kind_from_int, kind_to_int, set_hint, slice, snap_mode,
};
use crate::{
    MainWindow, ManipulateModel,
    gui::{
        editor::{
            callbacks::resubmit_facet_overlay,
            edit_intent::{EditIntent, EditIntentQueue},
            state::{EditorState, coalesce_timestamp},
            view::{refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
    },
};
use indicatrix::geometry::meet_solver::{MeetConstraint, SolvedTier};
use indicatrix_cut_core::EditError;
use indicatrix_editor::manipulate::{
    DragStart, DragValue, HandleKind, drag_value, moved_tiers, text,
};
use slint::ComponentHandle as _;
use std::{collections::BTreeSet, time::Duration};

/// A tier whose solved mast moved by more than this counts as following the drag.
const MOVED_TOLERANCE: f64 = 1e-6;

/// The toast after an Escape that restored the design.
const CANCELLED_TOAST: &str = "Drag cancelled. The design is back the way it was.";

/// The hint when a depth drag cannot start.
const NEEDS_SOLVE_HINT: &str = "Solve the design first: a depth drag needs the tier's solved mast.";

/// One edit a throttled drag value asks for, relative to what the gesture has already
/// applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Step {
    /// Set the tier's angle to this many degrees.
    Angle(f64),
    /// Pin the tier's mast at this value.
    Mast(f64),
    /// Turn the tier's index-wheel positions by this many MORE whole teeth.
    Teeth(i64),
}

/// What one applied [`Step`] reported back.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct AppliedEdit {
    /// The session generation after the edit.
    pub(super) generation: u64,
    /// The meet constraint a depth pin replaced (only the gesture's first pin has one).
    pub(super) replaced_meet: Option<MeetConstraint>,
}

/// What a gesture has applied so far. The decision logic ([`drain_value`]) lives on
/// this plain struct so it can be tested without a window.
#[derive(Debug, Default)]
pub(super) struct DragProgress {
    /// The newest value handed to the apply hook (applied or found unchanged).
    pub(super) last_value: Option<DragValue>,
    /// Whole teeth the index handle has turned the tier by since the press.
    pub(super) teeth_applied: i64,
    /// Whether any edit actually reached the design.
    pub(super) applied_any: bool,
    /// The session generation after the newest applied edit.
    pub(super) applied_generation: Option<u64>,
    /// The first meet constraint a depth pin of this gesture replaced.
    pub(super) replaced_meet: Option<MeetConstraint>,
}

impl DragProgress {
    /// The edit `value` asks for, or `None` when it equals the last value handed out
    /// (no redundant undo churn) or, for the index handle, asks for no further turn.
    /// Index values are running totals from the press, so the step is the difference to
    /// what is already applied.
    pub(super) fn plan(&mut self, value: DragValue) -> Option<Step> {
        if self.last_value == Some(value) {
            return None;
        }
        self.last_value = Some(value);
        match value {
            DragValue::AngleDeg(deg) => Some(Step::Angle(deg)),
            DragValue::Mast(mast) => Some(Step::Mast(mast)),
            DragValue::IndexTeeth(total) => {
                let more = total - self.teeth_applied;
                (more != 0).then_some(Step::Teeth(more))
            }
        }
    }

    /// Records that `step` reached the design.
    pub(super) fn record(&mut self, step: Step, applied: AppliedEdit) {
        self.applied_any = true;
        self.applied_generation = Some(applied.generation);
        if let Step::Teeth(more) = step {
            self.teeth_applied += more;
        }
        if self.replaced_meet.is_none() {
            self.replaced_meet = applied.replaced_meet;
        }
    }
}

/// The whole per-value decision of a drain tick: plan the edit `value` asks for, hand it
/// to `apply` once, and record what came back. Returns whether an edit reached the
/// design. `apply` returns `None` for "nothing changed" and after reporting an error.
pub(super) fn drain_value(
    progress: &mut DragProgress,
    value: DragValue,
    apply: impl FnOnce(Step) -> Option<AppliedEdit>,
) -> bool {
    let Some(step) = progress.plan(value) else {
        return false;
    };
    let Some(applied) = apply(step) else {
        return false;
    };
    progress.record(step, applied);
    true
}

/// A gesture in progress.
pub(super) struct ActiveDrag {
    pub(super) start: DragStart,
    pub(super) tier: usize,
    pub(super) label: String,
    /// The solved masts at the press, to tell which tiers follow the drag.
    pub(super) start_masts: Vec<SolvedTier>,
    pub(super) progress: DragProgress,
    /// The newest value the pointer asks for, applied or not yet (the release flushes it).
    pub(super) requested: Option<DragValue>,
    /// The tiers currently outlined as following the drag.
    pub(super) outlined: Vec<usize>,
    /// The clock every edit of the gesture is stamped with, so a pointer held still for
    /// longer than the coalescing window cannot split the drag into two undo steps.
    pub(super) gesture_now: Duration,
    /// Whether the gesture edits the provisional slice tier (a clone kept by
    /// [`slice`], no history entry, no toast) instead of `EditorState`.
    pub(super) provisional: bool,
    /// The provisional tier as it was at the press, for Escape.
    pub(super) restore: Option<slice::Snapshot>,
}

/// What a press on a handle captures from the design it will edit.
pub(super) struct GestureInputs {
    /// The tier's angle now.
    pub(super) start_angle_deg: f64,
    /// The solved masts, when they describe the design one to one.
    pub(super) masts: Option<Vec<SolvedTier>>,
    /// The provisional tier to restore on Escape (`None` for a committed tier, whose
    /// gesture is undone through the history).
    pub(super) restore: Option<slice::Snapshot>,
}

/// The value a handle has before any pointer travel.
pub(super) const fn start_value(
    kind: HandleKind,
    start_angle_deg: f64,
    start_mast: f64,
) -> DragValue {
    match kind {
        HandleKind::Angle => DragValue::AngleDeg(start_angle_deg),
        HandleKind::Depth => DragValue::Mast(start_mast),
        HandleKind::Index => DragValue::IndexTeeth(0),
    }
}

/// The value the hint line shows: the newest the pointer asked for.
fn live_value(drag: &ActiveDrag) -> DragValue {
    let untouched = start_value(
        drag.start.kind,
        drag.start.start_angle_deg,
        drag.start.start_mast,
    );
    drag.requested
        .or(drag.progress.last_value)
        .unwrap_or(untouched)
}

/// The value the toast reports: the index handle's running turn, else the last value
/// the gesture handed to the design.
fn final_value(drag: &ActiveDrag) -> DragValue {
    match drag.start.kind {
        HandleKind::Index => DragValue::IndexTeeth(drag.progress.teeth_applied),
        _ => live_value(drag),
    }
}

/// What the gesture on `target` starts from. A committed tier also starts a fresh
/// coalescing run: an earlier edit of this tier inside the window must not absorb the
/// gesture, and the gesture must not leak into the next edit either. A provisional tier
/// has no history at all.
fn gesture_inputs(ctx: &Shared, target: &Target) -> Option<GestureInputs> {
    if target.provisional {
        return slice::gesture_inputs();
    }
    let mut st = ctx.state.try_borrow_mut().ok()?;
    let start_angle_deg = st.design.tiers.get(target.tier)?.angle_deg;
    let masts = handles::aligned_masts(ctx, st.design.tiers.len());
    st.history.end_coalesce_run();
    Some(GestureInputs {
        start_angle_deg,
        masts,
        restore: None,
    })
}

/// `ManipulateModel.drag_begin`: takes the gesture's snapshot and grants (or refuses) it.
pub(super) fn begin(ui: &MainWindow, ctx: &Shared, kind: i32, x: f32, y: f32) {
    let Some(kind) = kind_from_int(kind) else {
        return;
    };
    let Some(target) = SESSION.with(|cell| cell.borrow().target.clone()) else {
        return;
    };
    if kind == HandleKind::Index && target.frame.is_indexless() {
        return;
    }
    let Some(GestureInputs {
        start_angle_deg,
        masts,
        restore,
    }) = gesture_inputs(ctx, &target)
    else {
        return;
    };
    if kind == HandleKind::Depth && masts.is_none() {
        set_hint(ui, NEEDS_SOLVE_HINT);
        return;
    }
    let start_masts = masts.unwrap_or_default();
    // An unsolved design has no masts: the mast falls back to 0 (depth drags were
    // refused above), which the angle and index drags never read.
    let start_mast = start_masts.get(target.tier).map_or(0.0, |s| s.mast);
    let drag = ActiveDrag {
        start: DragStart {
            kind,
            start_angle_deg,
            start_mast,
            pointer: handles::pointer_to_pick(ui, x, y),
            layout: target.layout,
        },
        tier: target.tier,
        label: target.label,
        start_masts,
        progress: DragProgress::default(),
        requested: None,
        outlined: Vec::new(),
        gesture_now: coalesce_timestamp(),
        provisional: target.provisional,
        restore,
    };
    let hint = if drag.provisional {
        slice::provisional_hint_text().unwrap_or_default()
    } else {
        text::drag_live_hint(
            kind,
            &drag.label,
            &start_value(kind, start_angle_deg, start_mast),
            0,
        )
    };
    SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        session.drag = Some(drag);
        session.hover_hint_active = false;
    });
    let model = ui.global::<ManipulateModel>();
    model.set_dragging(true);
    model.set_hovered_handle(kind_to_int(kind));
    set_hint(ui, &hint);
}

/// `ManipulateModel.drag_move`: turns the pointer into a value and posts it; the queue
/// drains at most one per frame into [`apply_intent`].
pub(super) fn move_to(ui: &MainWindow, queue: &EditIntentQueue, x: f32, y: f32, shift: bool) {
    let snap = snap_mode(shift, ui.global::<ManipulateModel>().get_snap_off());
    let pointer = handles::pointer_to_pick(ui, x, y);
    let posted = SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        let drag = session.drag.as_mut()?;
        let value = drag_value(&drag.start, pointer, snap);
        drag.requested = Some(value);
        Some((drag.tier, value))
    });
    if let Some((tier, value)) = posted {
        queue.post(EditIntent::DragTier { tier, value });
    }
}

/// The queue's drain: applies the newest value of this frame.
pub(super) fn apply_intent(ui: &MainWindow, ctx: &Shared, intent: &EditIntent) {
    let EditIntent::DragTier { tier, value } = *intent else {
        return;
    };
    // Taken out of the session while it is worked on, so nothing below can hit a
    // second borrow of it.
    let Some(mut drag) = SESSION.with(|cell| cell.borrow_mut().drag.take()) else {
        return;
    };
    if drag.tier == tier {
        apply_value(ui, ctx, &mut drag, value);
        refresh_feedback(ui, ctx, &mut drag);
    }
    SESSION.with(|cell| cell.borrow_mut().drag = Some(drag));
}

/// Performs one planned [`Step`] on `tier`.
fn apply_step(
    st: &mut EditorState,
    tier: usize,
    step: Step,
    now: Duration,
) -> Result<Option<AppliedEdit>, EditError> {
    Ok(match step {
        Step::Angle(deg) => st
            .set_tier_angle(tier, deg, now)?
            .map(|change| AppliedEdit {
                generation: change.generation,
                replaced_meet: None,
            }),
        Step::Mast(mast) => st.pin_tier_mast(tier, mast, now)?.map(|pin| AppliedEdit {
            generation: pin.change.generation,
            replaced_meet: pin.replaced_meet,
        }),
        Step::Teeth(more) => st
            .rotate_tier_indices(tier, more, now)?
            .map(|change| AppliedEdit {
                generation: change.generation,
                replaced_meet: None,
            }),
    })
}

/// Applies `value` to the design (when it differs from the last one), then refreshes the
/// panel and submits the live replan for the dragged tier.
fn apply_value(ui: &MainWindow, ctx: &Shared, drag: &mut ActiveDrag, value: DragValue) {
    if drag.provisional {
        // The provisional tier lives in `slice`'s session: same throttling and
        // apply-only-on-change rule, but no `EditorState`, history or panel refresh.
        if drain_value(&mut drag.progress, value, slice::apply_step) {
            slice::resubmit(ui, ctx);
        }
        return;
    }
    let Ok(mut st) = ctx.state.try_borrow_mut() else {
        return;
    };
    let (tier, now) = (drag.tier, drag.gesture_now);
    let mut failure: Option<String> = None;
    let applied = drain_value(&mut drag.progress, value, |step| {
        match apply_step(&mut st, tier, step, now) {
            Ok(edit) => edit,
            Err(error) => {
                failure = Some(error.to_string());
                None
            }
        }
    });
    if applied {
        let dirty = BTreeSet::from([tier]);
        refresh_editor_panel_stale(ui, &ctx.render_ctx, &st, &dirty);
        submit_preview_replan(
            ui,
            &ctx.render_ctx,
            &ctx.preview_state,
            &ctx.solid_last_solved,
            &st,
            dirty,
            false,
        );
    }
    drop(st);
    if let Some(message) = failure {
        show_toast(ui, &message, "error");
    }
}

/// Every facet id of the given tiers, in tier order.
fn facets_of_tiers(ctx: &Shared, tiers: &[usize]) -> Vec<u32> {
    let Ok(st) = ctx.state.try_borrow() else {
        return Vec::new();
    };
    let Some(map) = handles::facet_map_for(ctx, st.current_generation(), &st.design) else {
        return Vec::new();
    };
    tiers
        .iter()
        .flat_map(|&tier| map.facets_of_tier(tier).iter().copied())
        .collect()
}

/// Words the live hint ("P1 -> 41.3 deg, 3 other tiers follow") and outlines the tiers
/// whose solved mast has moved since the press.
fn refresh_feedback(ui: &MainWindow, ctx: &Shared, drag: &mut ActiveDrag) {
    if drag.provisional {
        if let Some(hint) = slice::provisional_hint_text() {
            set_hint(ui, &hint);
        }
        return;
    }
    let moved = handles::aligned_masts(ctx, drag.start_masts.len()).map_or_else(Vec::new, |now| {
        moved_tiers(&drag.start_masts, &now, drag.tier, MOVED_TOLERANCE)
    });
    let hint = text::drag_live_hint(drag.start.kind, &drag.label, &live_value(drag), moved.len());
    set_hint(ui, &hint);
    if moved != drag.outlined {
        let ids = facets_of_tiers(ctx, &moved);
        drag.outlined = moved;
        resubmit_facet_overlay(&ctx.preview_state, |overlay| overlay.moved = ids);
    }
}

/// A solid-preview frame landed mid-drag: its masts show which tiers follow now.
pub(super) fn update_feedback(ui: &MainWindow, ctx: &Shared) {
    let Some(mut drag) = SESSION.with(|cell| cell.borrow_mut().drag.take()) else {
        return;
    };
    refresh_feedback(ui, ctx, &mut drag);
    SESSION.with(|cell| cell.borrow_mut().drag = Some(drag));
}

/// Clears everything the drag showed: the outline, the hint, the lit handle.
fn finish_ui(ui: &MainWindow, ctx: &Shared, drag: &ActiveDrag) {
    if !drag.outlined.is_empty() {
        resubmit_facet_overlay(&ctx.preview_state, |overlay| overlay.moved.clear());
    }
    let model = ui.global::<ManipulateModel>();
    model.set_dragging(false);
    model.set_hovered_handle(-1);
    SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        session.hovered = None;
        session.hover_hint_active = false;
    });
    slice::resting_hint(ui);
}

/// `ManipulateModel.drag_end`: flushes the newest pointer value, keeps the gesture as
/// its one undo step and says what it did.
pub(super) fn end(ui: &MainWindow, ctx: &Shared) {
    let Some(mut drag) = SESSION.with(|cell| cell.borrow_mut().drag.take()) else {
        return;
    };
    if let Some(value) = drag.requested {
        apply_value(ui, ctx, &mut drag, value);
    }
    if !drag.provisional
        && let Ok(mut st) = ctx.state.try_borrow_mut()
    {
        st.history.end_coalesce_run();
    }
    finish_ui(ui, ctx, &drag);
    // A provisional drag has no history entry and no toast: the hint line and the
    // Keep button are its feedback.
    if drag.progress.applied_any && !drag.provisional {
        let toast = text::drag_done_toast(
            drag.start.kind,
            &drag.label,
            &final_value(&drag),
            drag.progress.replaced_meet.as_ref(),
        );
        show_toast(ui, &toast, "info");
    }
}

/// `ManipulateModel.drag_cancel`: undoes the gesture's one step, if it applied any.
pub(super) fn cancel(ui: &MainWindow, ctx: &Shared) {
    let Some(drag) = SESSION.with(|cell| cell.borrow_mut().drag.take()) else {
        return;
    };
    if drag.provisional {
        if let Some(tier) = drag.restore.clone() {
            slice::restore_tier(ui, ctx, tier);
        }
    } else if drag.progress.applied_any {
        undo_gesture(ui, ctx, &drag);
    } else if let Ok(mut st) = ctx.state.try_borrow_mut() {
        st.history.end_coalesce_run();
    }
    finish_ui(ui, ctx, &drag);
}

/// Undoes the gesture's coalesced step, provided it is still the newest thing that
/// happened to the design, and redraws everything from a full solve (the same refresh
/// the Undo button runs).
fn undo_gesture(ui: &MainWindow, ctx: &Shared, drag: &ActiveDrag) {
    let Ok(mut st) = ctx.state.try_borrow_mut() else {
        return;
    };
    st.history.end_coalesce_run();
    if Some(st.current_generation()) != drag.progress.applied_generation {
        drop(st);
        show_toast(
            ui,
            "Something else changed the design during the drag, so Escape left it alone. Use Undo to step back.",
            "info",
        );
        return;
    }
    match st.undo() {
        Ok(true) => {
            refresh_editor_panel_stale(
                ui,
                &ctx.render_ctx,
                &st,
                &(0..st.design.tiers.len()).collect(),
            );
            submit_preview_replan(
                ui,
                &ctx.render_ctx,
                &ctx.preview_state,
                &ctx.solid_last_solved,
                &st,
                BTreeSet::new(),
                true,
            );
            drop(st);
            show_toast(ui, CANCELLED_TOAST, "info");
        }
        Ok(false) => {}
        Err(error) => {
            drop(st);
            show_toast(ui, &format!("Could not cancel the drag: {error}"), "error");
        }
    }
}
