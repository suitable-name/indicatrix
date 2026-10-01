//! Angle editing in the tier table: the inline angle cell's commit and the nudge (Up /
//! Down in the open editor, the wheel), exactly as the desktop does them.
//!
//! - **Commit** (`EditorSession::set_tier_angle_from_text`): the text is parsed like the
//!   tier form's Angle field; a bit-identical value spends no undo step ("No change.").
//! - **Nudge** (`EditorSession::nudge_angles`): one `Edit::RetargetAngles` keyed by the
//!   target set, coalesced on `performance.now()` so a burst of ticks is ONE undo step. A
//!   nudge of a row inside a multi-selection of two or more moves the whole group. A nudge
//!   that would cross 0 degrees stops there (it would silently move the tier into the
//!   other block) and says so.
//!
//! Wheel ticks and key repeats arrive faster than a frame, so they are also coalesced at
//! the source ([`EditIntent`]): ticks on the same targets are summed and applied once per
//! [`DRAIN_INTERVAL`], the same queue the desktop keeps.

use super::edit::{Dirty, finish_edit, with_app};
use crate::{
    TierTableModel,
    app::{
        Ctx, coalesce_now,
        push::{MessageKind, show_message},
    },
};
use indicatrix_editor::{
    edit_intent::{DRAIN_INTERVAL, EditIntent},
    session::InlineAngle,
};
use slint::{Timer, TimerMode};
use std::cell::RefCell;

thread_local! {
    /// The nudge waiting for the next drain.
    static PENDING: RefCell<Option<EditIntent>> = const { RefCell::new(None) };
    /// Fires the drain one [`DRAIN_INTERVAL`] after the first tick of a burst.
    static DRAIN: Timer = Timer::default();
}

/// The inline cell's Enter / focus-loss commit for tier `index`.
pub fn commit_angle(ctx: &Ctx, index: i32, text: &str) {
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    let result = with_app(ctx, |app| {
        Some(
            app.design
                .as_mut()?
                .session
                .set_tier_angle_from_text(index, text),
        )
    });
    match result {
        None | Some(Ok(InlineAngle::Missing)) => {}
        // Without a message a committed-but-unchanged edit would be indistinguishable
        // from a dropped one.
        Some(Ok(InlineAngle::NoChange)) => show_message(ctx, MessageKind::Info, "No change."),
        Some(Ok(InlineAngle::Applied(_))) => finish_edit(ctx, Dirty::one(index)),
        Some(Err(message)) => show_message(ctx, MessageKind::Error, &message),
    }
}

/// The rows a nudge anchored at row `anchor` moves: the whole multi-selection when the
/// anchor belongs to a group of two or more, else just the anchor. A negative anchor (the
/// batch bar's Offset with nothing selected) anchors on the group's first member.
fn nudge_targets(ctx: &Ctx, anchor: i32) -> Vec<usize> {
    let app = ctx.state.borrow();
    let multi = app
        .design
        .as_ref()
        .map(|d| &d.session.multi_selected)
        .filter(|m| m.len() > 1);
    match (usize::try_from(anchor).ok(), multi) {
        (Some(anchor), Some(multi)) if multi.contains(&anchor) => multi.iter().copied().collect(),
        (Some(anchor), _) => vec![anchor],
        (None, Some(multi)) => multi.iter().copied().collect(),
        (None, None) => Vec::new(),
    }
}

/// Posts a nudge of `delta_deg` for the rows anchored at `anchor`; it is applied by the
/// next drain, summed with every tick that arrives before it.
fn post_nudge(ctx: &Ctx, anchor: i32, delta_deg: f32) {
    let targets = nudge_targets(ctx, anchor);
    if targets.is_empty() {
        return;
    }
    let intent = EditIntent::NudgeAngle {
        targets,
        delta_deg: f64::from(delta_deg),
    };
    PENDING.with(|cell| {
        let mut pending = cell.borrow_mut();
        let merged = pending
            .as_mut()
            .is_some_and(|existing| existing.merge(&intent));
        if !merged {
            *pending = Some(intent);
        }
    });
    let c = ctx.clone();
    DRAIN.with(|timer| {
        if !timer.running() {
            timer.start(TimerMode::SingleShot, DRAIN_INTERVAL, move || drain(&c));
        }
    });
}

/// Applies the pending nudge: one edit for the whole burst, with the zero-crossing clamp
/// judged against the tiers' angles NOW.
fn drain(ctx: &Ctx) {
    let Some(EditIntent::NudgeAngle { targets, delta_deg }) =
        PENDING.with(|cell| cell.borrow_mut().take())
    else {
        return;
    };
    let result = with_app(ctx, |app| {
        Some(
            app.design
                .as_mut()?
                .session
                .nudge_angles(&targets, delta_deg, coalesce_now()),
        )
    });
    match result {
        None | Some(Ok(None)) => {}
        Some(Ok(Some(outcome))) => {
            finish_edit(ctx, Dirty::Tiers(targets.iter().copied().collect()));
            // The angle's sign says which block a tier belongs to, so a nudge that would
            // cross zero is clamped instead; explain the stop.
            if !outcome.clamped_labels.is_empty() {
                show_message(
                    ctx,
                    MessageKind::Info,
                    &format!(
                        "{} stopped at 0\u{b0} -- nudging further would move it into the \
                         other block. Type the angle directly (e.g. \"-0\") to cross \
                         blocks on purpose.",
                        outcome.clamped_labels.join(", ")
                    ),
                );
            }
        }
        Some(Err(e)) => show_message(ctx, MessageKind::Error, &e.to_string()),
    }
}

/// Wires the inline-edit callbacks of `TierTableModel`.
pub fn wire(model: &TierTableModel<'_>, ctx: &Ctx) {
    let c = ctx.clone();
    model.on_commit_angle(move |index, text| commit_angle(&c, index, &text));
    let c = ctx.clone();
    model.on_nudge(move |index, delta| post_nudge(&c, index, delta));
}
