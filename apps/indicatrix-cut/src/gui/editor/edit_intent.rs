//! Coalesce-at-the-source UI edit queue: batches UI edits into single batches
//! applied once per frame rather than once per raw UI event.
//!
//! Before this module, an angle nudge (Up/Down/wheel), a Cut-slider drag tick, and
//! a Retarget crown-slider drag tick each triggered their own full
//! apply/refresh/replan cycle -- `history::History::apply_coalescing` already
//! merges the resulting UNDO steps, but the UI-thread work (cloning `Design`,
//! rebuilding Slint models, submitting a solid-preview replan) still ran once per
//! raw event, at whatever rate the widget fired `changed`, not once per rendered
//! frame.
//!
//! [`EditIntentQueue`] fixes that: [`EditIntentQueue::post`] coalesces a new
//! [`EditIntent`] into whatever this queue already has pending (merging when
//! [`EditIntent::merge`] applies -- e.g. two nudges on the SAME tier sum their
//! angle deltas -- or replacing it outright otherwise, e.g. a slider step always
//! keeps only the newest position), and starts a 16 ms (one frame at 60 Hz)
//! `slint::Timer` the first time the queue goes from empty to non-empty. Each tick
//! takes whatever is pending (there is at most one, by construction) and hands it
//! to the `on_drain` closure supplied to [`EditIntentQueue::new`] -- exactly once,
//! regardless of how many `post` calls landed since the previous tick. Once a tick
//! finds nothing pending (no `post` since the last drain), the timer stops itself
//! -- see [`EditIntentQueue::post`]'s own doc comment for why the tick closure
//! holds only a `Weak` handle back to the queue that owns it, never a strong `Rc`
//! cycle.
//!
//! Each of the three call sites this module serves (`callbacks::tier_actions::
//! setup_nudge_angle_callback`, `gui::editor::setup::setup_tier_cutoff_callback`,
//! `callbacks::retarget_actions::setup_retarget_proposal_changed_callback`) owns
//! its OWN [`EditIntentQueue`] instance, built once when that callback is wired up
//! and captured by the `on_*` closure exactly like every other long-lived
//! `Rc`/`Arc` handle in this crate. A queue only ever receives ONE [`EditIntent`]
//! variant in practice (each call site posts only its own kind), so there is no
//! cross-variant interference to reason about even though [`EditIntent`] is one
//! shared enum, not three separate types -- sharing the enum (and this engine)
//! keeps the coalescing behaviour identical across all three instead of three
//! independently-drifting implementations.

use slint::{Timer, TimerMode};
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

// The intent enum and its coalescing decision (`EditIntent::merge`) moved to
// `indicatrix_editor::edit_intent`, shared with the web app; this module keeps the
// `slint::Timer`-driven queue. `DRAIN_INTERVAL` is one frame at 60 Hz, matching
// `stall_guard::STALL_THRESHOLD`'s own frame budget.
use indicatrix_editor::edit_intent::DRAIN_INTERVAL;
pub(super) use indicatrix_editor::edit_intent::EditIntent;

/// A single-slot, coalesce-on-post, drain-on-a-16ms-timer queue -- see the module
/// doc comment. `on_drain` is registered once, at [`EditIntentQueue::new`], and
/// invoked at most once per [`DRAIN_INTERVAL`] tick, only when something is
/// actually pending.
pub(super) struct EditIntentQueue {
    pending: RefCell<Option<EditIntent>>,
    on_drain: Box<dyn Fn(EditIntent)>,
    timer: Timer,
    /// A `Weak` handle to this SAME [`EditIntentQueue`], stashed via
    /// [`Rc::new_cyclic`] at construction time -- see [`Self::post`]'s own doc
    /// comment for why the timer's tick closure needs one.
    self_weak: Weak<Self>,
}

impl EditIntentQueue {
    /// Builds a new, empty queue. The drain timer does not start until the first
    /// [`Self::post`] -- see that method's own doc comment.
    pub(super) fn new(on_drain: impl Fn(EditIntent) + 'static) -> Rc<Self> {
        Rc::new_cyclic(|weak_self| Self {
            pending: RefCell::new(None),
            on_drain: Box::new(on_drain),
            timer: Timer::default(),
            self_weak: weak_self.clone(),
        })
    }

    /// Coalesces `intent` into whatever this queue already has pending (see
    /// [`EditIntent::merge`]), and starts the 16 ms drain timer if the queue was
    /// empty (this is the first post of a new burst). Every subsequent post
    /// inside the same burst is cheap: it only updates `pending` under a short
    /// `RefCell` borrow, since the already-running timer will pick it up on its
    /// next tick regardless.
    ///
    /// # Why the timer's tick closure holds a `Weak`, not a strong `Rc`
    ///
    /// The tick closure needs to reach this SAME [`EditIntentQueue`] again once it
    /// fires -- but it is registered on `self.timer`, a field `EditIntentQueue`
    /// itself owns, so a closure capturing a strong `Rc` back to `self` would be a
    /// permanent reference cycle (this struct would never drop, `Timer` included,
    /// for the life of the process). Capturing [`Self::self_weak`] instead avoids
    /// the cycle entirely: `Weak::upgrade` fails harmlessly once every OTHER owner
    /// (the `on_*` Slint callback closure that built this queue, alive for the
    /// app's lifetime in practice) has dropped its `Rc`, rather than the queue
    /// being kept alive by its own timer.
    pub(super) fn post(&self, intent: EditIntent) {
        let was_empty = {
            let mut pending = self.pending.borrow_mut();
            let was_empty = pending.is_none();
            let merged = pending
                .as_mut()
                .is_some_and(|existing| existing.merge(&intent));
            if !merged {
                *pending = Some(intent);
            }
            was_empty
        };
        if !was_empty {
            // The timer is already running (or about to tick) and will pick up
            // the just-merged value -- nothing else to do.
            return;
        }
        let queue = self.self_weak.clone();
        self.timer
            .start(TimerMode::Repeated, DRAIN_INTERVAL, move || {
                let Some(queue) = queue.upgrade() else {
                    return;
                };
                let Some(intent) = queue.pending.borrow_mut().take() else {
                    queue.timer.stop();
                    return;
                };
                (queue.on_drain)(intent);
            });
    }
}
