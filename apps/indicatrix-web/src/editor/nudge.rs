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

use super::{
    edit::{Dirty, finish_edit, with_app},
    table,
};
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

// The relation helpers are pure and native-tested in `indicatrix_web_core::rows`.
pub use indicatrix_web_core::rows::{driven_refusal, with_followers};

/// The inline cell's Enter / focus-loss commit for tier `index`.
///
/// A concave row is refused (see [`table::editable_tier`]). A tier whose angle follows a
/// relation takes no plain angle: text that is not itself a relation (`=...`) is refused,
/// naming the tier that drives it.
pub fn commit_angle(ctx: &Ctx, index: i32, text: &str) {
    let Some(index) = table::editable_tier(ctx, index) else {
        return;
    };
    let refusal = (!text.trim_start().starts_with('='))
        .then(|| {
            ctx.state
                .borrow()
                .design
                .as_ref()
                .and_then(|d| driven_refusal(&d.session.design, index))
        })
        .flatten();
    if let Some(message) = refusal {
        show_message(ctx, MessageKind::Info, &message);
        return;
    }
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
    // A concave anchor never reaches the queue (a negative one is the batch bar's Offset).
    if anchor >= 0 && table::editable_tier(ctx, anchor).is_none() {
        return;
    }
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
    // A tier whose angle follows a relation takes no direct nudge: say which one and why,
    // and nudge the free tiers of the group as usual.
    let (driven, targets): (Vec<usize>, Vec<usize>) = {
        let app = ctx.state.borrow();
        match app.design.as_ref() {
            Some(design) => targets
                .iter()
                .copied()
                .partition(|&tier| design.session.design.is_tier_driven(tier)),
            None => (Vec::new(), targets),
        }
    };
    let refusal = driven.first().and_then(|&tier| {
        ctx.state
            .borrow()
            .design
            .as_ref()
            .and_then(|d| driven_refusal(&d.session.design, tier))
    });
    if targets.is_empty() {
        if let Some(message) = refusal {
            show_message(ctx, MessageKind::Info, &message);
        }
        return;
    }
    // The delta moves the number the row SHOWS (a pavilion angle is printed without its
    // minus sign), like the desktop's nudge.
    let result = with_app(ctx, |app| {
        Some(app.design.as_mut()?.session.nudge_displayed_angles(
            &targets,
            delta_deg,
            coalesce_now(),
        ))
    });
    match result {
        None | Some(Ok(None)) => {}
        Some(Ok(Some(outcome))) => {
            // The tiers that follow a nudged tier moved with it.
            let dirty = ctx.state.borrow().design.as_ref().map_or_else(
                || targets.iter().copied().collect(),
                |d| with_followers(&d.session.design, targets.iter().copied()),
            );
            finish_edit(ctx, Dirty::Tiers(dirty));
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
            // Last, so it is the message left on screen: part of the group did not move.
            if let Some(message) = refusal {
                show_message(ctx, MessageKind::Info, &message);
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
