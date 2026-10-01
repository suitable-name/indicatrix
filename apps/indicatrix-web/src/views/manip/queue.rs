//! The single-slot, coalesce-on-post, drain-on-a-16-ms-timer queue a handle drag posts
//! its pointer values into, so a burst of pointer moves costs ONE apply/replan per frame.
//!
//! The web counterpart of the desktop's `EditIntentQueue`: the same
//! `EditIntent::merge` decides how a new value folds into the pending one, and the same
//! `DRAIN_INTERVAL` paces the drain. The timer starts on the first post of a burst and
//! stops itself on the first tick that finds nothing pending.

use super::drag;
use crate::app::Ctx;
use indicatrix_editor::edit_intent::{DRAIN_INTERVAL, EditIntent};
use slint::{Timer, TimerMode};
use std::cell::RefCell;

thread_local! {
    /// The intent waiting for the next tick.
    static PENDING: RefCell<Option<EditIntent>> = const { RefCell::new(None) };
    /// The drain timer.
    static DRAIN: Timer = Timer::default();
}

/// Coalesces `intent` into whatever is pending (see `EditIntent::merge`), and starts the
/// drain timer if the queue was empty (this is the first post of a new burst).
pub(super) fn post(ctx: &Ctx, intent: EditIntent) {
    let was_empty = PENDING.with(|cell| {
        let mut pending = cell.borrow_mut();
        let was_empty = pending.is_none();
        let merged = pending
            .as_mut()
            .is_some_and(|existing| existing.merge(&intent));
        if !merged {
            *pending = Some(intent);
        }
        was_empty
    });
    if !was_empty {
        // The timer is already running and will pick up the just-merged value.
        return;
    }
    let ctx = ctx.clone();
    DRAIN.with(|timer| timer.start(TimerMode::Repeated, DRAIN_INTERVAL, move || tick(&ctx)));
}

/// One drain tick: applies what is pending, or stops the timer when nothing is.
fn tick(ctx: &Ctx) {
    let Some(intent) = PENDING.with(|cell| cell.borrow_mut().take()) else {
        DRAIN.with(Timer::stop);
        return;
    };
    drag::apply_intent(ctx, &intent);
}

/// Drops whatever is pending (a drag ended: its last value was applied directly).
pub(super) fn clear() {
    PENDING.with(|cell| *cell.borrow_mut() = None);
    DRAIN.with(Timer::stop);
}
