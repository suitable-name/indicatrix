//! [`RedrawGate`]: coalesces a burst of "please redraw" requests that would otherwise
//! each queue their own closure on the Slint UI event loop
//! (`Weak::upgrade_in_event_loop` queues rather than runs immediately) into at most one
//! pending closure, which always acts on the LATEST submitted payload.
//!
//! Without this, every incoming redraw-worthy event queued its own closure
//! unconditionally, so a burst arriving faster than Slint could drain its event queue
//! let that queue -- and the full-buffer clone + tonemap cost each entry carried --
//! grow without bound. Used by both the local render path (via
//! `frame_helpers::push_frame_to_ui`) and the remote path
//! (`orchestrator::tick::update::handle_remote_update`), whose payload type `T` differs:
//! a ready-to-display pixel buffer for local, `()` for remote (which re-derives its
//! redraw from the live, already-shared accumulator instead).
//!
//! No Slint type anywhere in this module, so it is directly unit-testable.
//!
//! Two cooperating primitives:
//!
//! - `pending`: an `AtomicBool` set by whichever [`RedrawGate::submit`] call wins the
//!   compare-exchange; cleared by [`RedrawGate::take`], called from inside the enqueued
//!   closure right before it does real work. Every OTHER `submit` in between returns
//!   `None` and must NOT enqueue a second closure, since the pending one will read the
//!   latest slot value once it runs anyway.
//! - `generation`: an `AtomicU64` bumped on every `submit` (whether or not it won
//!   `pending`), exposed via [`RedrawGate::current_generation`] so a caller can confirm
//!   a burst was actually coalesced.
use std::sync::{
    Mutex, PoisonError,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

pub struct RedrawGate<T> {
    slot: Mutex<Option<T>>,
    pending: AtomicBool,
    generation: AtomicU64,
}

impl<T> RedrawGate<T> {
    pub const fn new() -> Self {
        Self {
            slot: Mutex::new(None),
            pending: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        }
    }

    /// Records `value` as the latest payload to redraw with ("latest wins") and bumps
    /// the generation. Returns `Some(generation)` exactly when THIS call must enqueue a
    /// Slint closure (none currently pending); `None` means a previously enqueued
    /// closure will pick up this value once it runs, so the caller must not enqueue.
    pub fn submit(&self, value: T) -> Option<u64> {
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(value);
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then_some(generation)
    }

    /// Called from inside the queued closure before real work: clears `pending` (so a
    /// future `submit` can enqueue the next closure) and takes the latest submitted
    /// payload, which may be newer than the one that caused this closure to be
    /// enqueued. `None` only if called without a matching prior `submit`.
    pub fn take(&self) -> Option<T> {
        self.pending.store(false, Ordering::Release);
        self.slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// The number of `submit` calls made so far, including coalesced ones. Exposed for
    /// tests only -- production callers act entirely on `submit`'s return value.
    #[cfg(test)]
    pub fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_submit_must_enqueue() {
        let gate: RedrawGate<u32> = RedrawGate::new();
        assert_eq!(gate.submit(1), Some(1));
        assert!(gate.is_pending());
    }

    #[test]
    fn a_second_submit_before_take_must_not_enqueue_again() {
        let gate: RedrawGate<u32> = RedrawGate::new();
        assert!(gate.submit(1).is_some());
        assert_eq!(
            gate.submit(2),
            None,
            "a closure is already pending -- must not enqueue a second one"
        );
        assert_eq!(
            gate.submit(3),
            None,
            "still pending -- must not enqueue a third one either"
        );
    }

    #[test]
    fn every_submit_bumps_generation_even_when_coalesced() {
        let gate: RedrawGate<u32> = RedrawGate::new();
        gate.submit(1);
        gate.submit(2);
        gate.submit(3);
        assert_eq!(
            gate.current_generation(),
            3,
            "generation must advance for every submit, not just the ones that enqueue"
        );
    }

    #[test]
    fn take_returns_the_latest_value_not_the_first_one() {
        let gate: RedrawGate<u32> = RedrawGate::new();
        assert!(gate.submit(1).is_some());
        assert_eq!(gate.submit(2), None, "coalesced");
        assert_eq!(gate.submit(3), None, "coalesced");

        assert_eq!(
            gate.take(),
            Some(3),
            "the one closure that does run must see the LATEST payload, not the one \
             that originally caused it to be enqueued"
        );
    }

    #[test]
    fn take_clears_pending_so_a_later_submit_can_enqueue_again() {
        let gate: RedrawGate<u32> = RedrawGate::new();
        assert!(gate.submit(1).is_some());
        assert!(gate.is_pending());
        gate.take();
        assert!(!gate.is_pending());
        assert!(
            gate.submit(2).is_some(),
            "pending was cleared, a fresh closure may be queued"
        );
    }

    #[test]
    fn take_on_an_empty_gate_returns_none() {
        let gate: RedrawGate<u32> = RedrawGate::new();
        assert_eq!(gate.take(), None);
    }

    #[test]
    fn a_burst_of_n_submits_coalesces_to_exactly_one_pending_closure() {
        let gate: RedrawGate<u32> = RedrawGate::new();
        let mut enqueued = 0;
        for i in 0..50 {
            if gate.submit(i).is_some() {
                enqueued += 1;
            }
        }
        assert_eq!(enqueued, 1, "only the first of a burst may enqueue");
        assert_eq!(
            gate.take(),
            Some(49),
            "and it must see the last value submitted"
        );
    }

    #[test]
    fn a_second_burst_after_take_enqueues_exactly_once_more() {
        let gate: RedrawGate<u32> = RedrawGate::new();
        for i in 0..10 {
            gate.submit(i);
        }
        gate.take();
        let mut enqueued = 0;
        for i in 10..20 {
            if gate.submit(i).is_some() {
                enqueued += 1;
            }
        }
        assert_eq!(enqueued, 1);
        assert_eq!(gate.take(), Some(19));
    }

    /// `T = ()` is the shape the remote path uses -- a pure signal with no payload,
    /// since it re-derives everything from the live, already-shared accumulator.
    #[test]
    fn works_as_a_pure_signal_with_a_unit_payload() {
        let gate: RedrawGate<()> = RedrawGate::new();
        assert!(gate.submit(()).is_some());
        assert_eq!(gate.submit(()), None, "coalesced");
        assert_eq!(gate.take(), Some(()));
        assert!(gate.submit(()).is_some(), "pending was cleared");
    }
}
