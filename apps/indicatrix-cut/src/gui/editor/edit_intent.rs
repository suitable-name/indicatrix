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
//! setup_nudge_angle_callback`, `gui::editor::mod::setup_tier_cutoff_callback`,
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
    time::Duration,
};

/// How often the drain timer ticks while an intent is queued -- one frame at
/// 60 Hz, matching `stall_guard::STALL_THRESHOLD`'s own frame budget.
const DRAIN_INTERVAL: Duration = Duration::from_millis(16);

/// One coalescable UI edit intent -- see the module doc comment.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum EditIntent {
    /// An angle nudge (Up/Down key or scroll wheel) on `targets` (a lone tier, or
    /// every tier in a multi-selected group -- see `setup_nudge_angle_callback`'s
    /// own doc comment for why a multi-select nudge is one `targets` set rather
    /// than one intent per tier). Repeated nudges to the SAME `targets` set within
    /// one drain window sum their `delta_deg` instead of each triggering their own
    /// apply/refresh/replan.
    NudgeAngle { targets: Vec<usize>, delta_deg: f64 },
    /// The Cut slider's tier-cutoff step (`SolidPreviewModel.tier_cutoff`). The
    /// live value is read fresh off the Slint property when the drain fires (the
    /// slider binding itself already writes it on every tick, `changed`-handler or
    /// not), so `count` only needs to distinguish "something changed" for
    /// [`EditIntent::merge`] -- carried anyway so a test can assert last-wins
    /// without a live `MainWindow`. Last-wins: only the final value posted in a
    /// drain window is ever the one a tick observes.
    CutOff { count: i32 },
    /// The Retarget dialog's crown-shift slider (`RetargetModel.crown_fraction`,
    /// read alongside `RetargetModel.scale_crown_by_ratio` at drain time -- see
    /// `setup_retarget_proposal_changed_callback`'s own doc comment). No payload:
    /// every post coalesces into the same single pending marker: last-wins.
    RetargetCrown,
}

impl EditIntent {
    /// Merges `incoming` into `self` in place when they share the same
    /// coalescing key, returning `true` on success. `false` means the two are NOT
    /// mergeable (e.g. two `NudgeAngle`s naming different `targets`), and
    /// [`EditIntentQueue::post`] instead REPLACES the pending intent outright,
    /// silently dropping whatever `self` described -- the same "newer supersedes
    /// older, unread" tradeoff `solve_service::Mailbox::put` already makes for the
    /// analogous solve-request case. In practice this replacement branch is
    /// reached only when two different targets are nudged within the same 16 ms
    /// window (a human cannot do this; only relevant to a scripted/automated
    /// input burst), so losing the superseded intent's own delta is an accepted,
    /// documented edge case rather than a silent correctness bug in ordinary use.
    fn merge(&mut self, incoming: &Self) -> bool {
        match (self, incoming) {
            (
                Self::NudgeAngle { targets, delta_deg },
                Self::NudgeAngle {
                    targets: other_targets,
                    delta_deg: other_delta,
                },
            ) if targets == other_targets => {
                *delta_deg += other_delta;
                true
            }
            (Self::CutOff { count }, Self::CutOff { count: other_count }) => {
                *count = *other_count;
                true
            }
            (Self::RetargetCrown, Self::RetargetCrown) => true,
            _ => false,
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives [`EditIntentQueue::post`]/its internal merge logic directly,
    /// without a live `slint::Timer` tick (this crate's own house rule: no
    /// windowing backend in the test environment) -- exercises exactly the
    /// coalescing decision [`EditIntent::merge`]/[`EditIntentQueue::post`] make,
    /// the same thing a real 16 ms tick would observe once it fires.
    fn coalesce(initial: EditIntent, posts: impl IntoIterator<Item = EditIntent>) -> EditIntent {
        let mut pending = initial;
        for intent in posts {
            if !pending.merge(&intent) {
                pending = intent;
            }
        }
        pending
    }

    #[test]
    fn fifty_nudges_on_the_same_tier_coalesce_to_one_summed_delta() {
        let first = EditIntent::NudgeAngle {
            targets: vec![3],
            delta_deg: 0.5,
        };
        let rest = (0..49).map(|_| EditIntent::NudgeAngle {
            targets: vec![3],
            delta_deg: 0.5,
        });
        let result = coalesce(first, rest);
        assert_eq!(
            result,
            EditIntent::NudgeAngle {
                targets: vec![3],
                delta_deg: 25.0
            },
            "50 nudges of 0.5 deg each must coalesce to one intent summing to 25.0 deg"
        );
    }

    #[test]
    fn a_nudge_burst_on_a_different_tier_replaces_rather_than_merges() {
        // Documented tradeoff (see `EditIntent::merge`'s own doc comment): a
        // different `targets` set is NOT mergeable, so the queue drops the
        // earlier, un-applied intent rather than losing track of the burst
        // entirely.
        let first = EditIntent::NudgeAngle {
            targets: vec![1],
            delta_deg: 1.0,
        };
        let result = coalesce(
            first,
            [EditIntent::NudgeAngle {
                targets: vec![2],
                delta_deg: 2.0,
            }],
        );
        assert_eq!(
            result,
            EditIntent::NudgeAngle {
                targets: vec![2],
                delta_deg: 2.0
            }
        );
    }

    #[test]
    fn a_cutoff_slider_burst_drains_to_the_last_value() {
        let first = EditIntent::CutOff { count: 10 };
        let result = coalesce(first, (11..=40).map(|count| EditIntent::CutOff { count }));
        assert_eq!(result, EditIntent::CutOff { count: 40 });
    }

    #[test]
    fn retarget_crown_posts_always_coalesce_to_one_marker() {
        let result = coalesce(EditIntent::RetargetCrown, [EditIntent::RetargetCrown; 0]);
        assert_eq!(result, EditIntent::RetargetCrown);
        let result = coalesce(
            EditIntent::RetargetCrown,
            std::iter::repeat_n(EditIntent::RetargetCrown, 20),
        );
        assert_eq!(result, EditIntent::RetargetCrown);
    }

    // `EditIntentQueue::post` itself (as opposed to the pure `merge`/`coalesce`
    // decision logic exercised above) is NOT driven end-to-end here: it calls
    // `slint::Timer::start`, which needs a live windowing backend/event loop this
    // crate's own test environment does not have (see `solve_service.rs`'s own
    // test module doc comment for the identical constraint on `SolveService::new`/
    // `submit`). The coalescing decision `post` makes before ever touching the
    // timer is exactly what `coalesce`/the tests above already verify.
}
