//! [`SampleCursor`]: the shared claim point every backend contributing to one image
//! draws disjoint absolute sample sub-ranges from -- the desktop export's local and
//! remote lanes, the live viewport's epoch (`LiveEpoch` in `indicatrix-cut`), and every
//! lane of a [`crate::LanePool`].
//!
//! Moved here from `indicatrix-cut`'s `bridge::sample_cursor`, which now re-exports it,
//! so the coordinator can share the exact same claim semantics.
//!
//! # Why a shared claim point, not a one-shot split
//!
//! A fixed up-front split (measured once, dispatched once) can leave a fast engine's
//! assigned slice finished early with nothing further to claim while the render keeps
//! going, sitting idle for the rest of it. `SampleCursor` replaces that with a shared
//! claim point every engine pulls fresh work from whenever free, for as long as any
//! remains.
//!
//! # Disjointness by construction
//!
//! [`claim`](SampleCursor::claim) and [`claim_local`](SampleCursor::claim_local) hand
//! out ranges via a single atomic `fetch_add`, so any number of callers on any number
//! of threads can never receive overlapping indices, by construction, with no reliance
//! on callers coordinating access any other way. Disjointness matters because both the
//! CPU tracer and GPU megakernel derive each sample's pixel-jitter/RNG draws from its
//! *absolute* index: overlapping ranges would redraw identical samples and silently
//! bias the average toward whatever indices got traced twice -- wrong, but not
//! obviously wrong.
//!
//! # Two retry piles
//!
//! A range a lane failed to finish must be traced again, exactly once, by someone.
//!
//! - [`return_to_local`](SampleCursor::return_to_local) (the desktop's single-remote
//!   model) pushes it onto a pile only [`claim_local`](SampleCursor::claim_local) /
//!   [`claim_local_bounded`](SampleCursor::claim_local_bounded) drain, never back onto
//!   the shared pool [`claim`](SampleCursor::claim) draws from. Otherwise remote could
//!   re-claim its own just-failed range on its very next iteration -- an infinite loop
//!   for a persistent (not transient) failure. Routing it to a local-only pile
//!   guarantees it is retried at most once more, locally.
//! - [`requeue`](SampleCursor::requeue) (the [`crate::LanePool`] model, where every
//!   lane is equal) pushes it onto a pile [`claim_any`](SampleCursor::claim_any) drains
//!   before fresh work. The pool itself prevents the infinite loop: a failing lane
//!   backs off and is retired after a bounded number of consecutive failures.
//!
//! The two models are never mixed on one cursor in practice; [`claim`] alone never
//! sees either pile.
//!
//! [`claim`]: SampleCursor::claim

use std::{
    collections::VecDeque,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU32, Ordering},
    },
};

#[cfg(test)]
mod tests;

/// See this module's doc comment. Every method takes `&self`, so one instance can be
/// shared (by reference inside a `thread::scope`, or behind an `Arc`) across every
/// lane's own thread.
#[derive(Debug)]
pub struct SampleCursor {
    /// Absolute index of the next sample NO engine has claimed yet. Every claim is one
    /// `fetch_add` against this -- see [`claim`](Self::claim).
    next: AtomicU32,
    /// One past the last sample in this budget (exclusive). A claim is never honoured
    /// past this, however large a range was requested.
    end: u32,
    /// Ranges a remote chunk failed to finish, reserved for the local lane alone -- see
    /// this module's doc comment.
    local_retry: Mutex<VecDeque<(u32, u32)>>,
    /// Ranges a pool lane failed to finish, open to every lane via
    /// [`claim_any`](Self::claim_any).
    requeued: Mutex<VecDeque<(u32, u32)>>,
}

impl SampleCursor {
    /// Builds a cursor over `[start, end)`. For an export, `start` is wherever a prior
    /// sequential phase (remote/hybrid calibration) left `samples_done` and `end` is
    /// the export's total `samples_per_pixel`; for a live epoch it is `[0,
    /// target_samples)`; for a coordinator job it is the request's
    /// `[first_sample, first_sample + samples)`.
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self {
            next: AtomicU32::new(start),
            end,
            local_retry: Mutex::new(VecDeque::new()),
            requeued: Mutex::new(VecDeque::new()),
        }
    }

    /// One past the last sample of this budget.
    #[must_use]
    pub const fn end(&self) -> u32 {
        self.end
    }

    /// Claims up to `want` fresh samples for ANY engine, returning `Some((start,
    /// count))` with `1 <= count <= want`, or `None` if the budget is already
    /// exhausted. Never returns a range past `end`, however large `want` is.
    ///
    /// `want == 0` always returns `None` rather than a zero-length range.
    ///
    /// # Why `fetch_add` alone is enough
    ///
    /// One atomic read-modify-write, unconditionally advancing `next` by `want` and
    /// then checking whether the range it was handed starts past `end`. `fetch_add` is
    /// inherently exclusive, so every call receives a DISTINCT `start` with no CAS
    /// retry loop needed. `next` can end up advanced PAST `end` (the last claim before
    /// exhaustion typically requests more than remains) -- harmless, since the
    /// returned `count` is clamped to `end - start` and every later claim sees
    /// `start >= end` and reports `None`. `saturating` is not needed in practice
    /// (budgets are far below `u32::MAX`), but `fetch_add` wraps, so a budget near
    /// `u32::MAX` must not be used.
    pub fn claim(&self, want: u32) -> Option<(u32, u32)> {
        if want == 0 {
            return None;
        }
        let start = self.next.fetch_add(want, Ordering::Relaxed);
        if start >= self.end {
            return None;
        }
        Some((start, want.min(self.end - start)))
    }

    /// Claims for the LOCAL lane only: a previously remote-failed range first (WHOLE,
    /// however long it is), falling back to [`claim`](Self::claim) once that retry
    /// pile is empty. See this module's doc comment for why a retried range never goes
    /// back to remote. The export's local lane batches whatever it gets; the live
    /// viewport uses [`claim_local_bounded`](Self::claim_local_bounded) instead.
    pub fn claim_local(&self, want: u32) -> Option<(u32, u32)> {
        let retried = self
            .local_retry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front();
        if retried.is_some() {
            return retried;
        }
        self.claim(want)
    }

    /// Like [`claim_local`](Self::claim_local), but never hands out more than `want`
    /// samples even from the retry pile: a longer retried range is split, its head
    /// returned now and its tail pushed back to the FRONT of the pile for the next
    /// call. The live viewport traces one short frame per claim, so a whole failed
    /// remote chunk (possibly hundreds of samples) must not land in a single frame.
    /// `want == 0` returns `None` without touching anything.
    pub fn claim_local_bounded(&self, want: u32) -> Option<(u32, u32)> {
        if want == 0 {
            return None;
        }
        pop_bounded(&self.local_retry, want).or_else(|| self.claim(want))
    }

    /// Claims for a [`crate::LanePool`] lane: up to `want` samples of a requeued range
    /// first (split like [`claim_local_bounded`](Self::claim_local_bounded)), falling
    /// back to fresh work from [`claim`](Self::claim). Never touches the local-only
    /// retry pile. `want == 0` returns `None` without touching anything.
    pub fn claim_any(&self, want: u32) -> Option<(u32, u32)> {
        if want == 0 {
            return None;
        }
        pop_bounded(&self.requeued, want).or_else(|| self.claim(want))
    }

    /// Whether the SHARED pool has nothing left to claim -- `true` once every sample up
    /// to `end` has been handed out to some engine. Says nothing about the retry
    /// piles. A cheap, lock-free read for callers deciding whether waiting around could
    /// still yield work.
    pub fn shared_pool_exhausted(&self) -> bool {
        self.next.load(Ordering::Relaxed) >= self.end
    }

    /// Whether nothing at all is claimable right now: the shared pool is exhausted and
    /// both retry piles are empty. A range currently being traced may still come back
    /// through [`requeue`](Self::requeue) or [`return_to_local`](Self::return_to_local)
    /// if its lane fails.
    pub fn fully_claimed(&self) -> bool {
        self.shared_pool_exhausted()
            && self
                .requeued
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
            && self
                .local_retry
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
    }

    /// Returns `[start, start + count)` to the queue for GUARANTEED local processing
    /// after a remote chunk failed to finish it -- never re-offered to remote. A `count`
    /// of `0` is a no-op (nothing to retry) rather than an empty queue entry.
    pub fn return_to_local(&self, start: u32, count: u32) {
        if count == 0 {
            return;
        }
        self.local_retry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back((start, count));
    }

    /// Returns `[start, start + count)` to the pile every [`claim_any`](Self::claim_any)
    /// caller drains first, after a pool lane failed to finish it. A `count` of `0` is
    /// a no-op.
    pub fn requeue(&self, start: u32, count: u32) {
        if count == 0 {
            return;
        }
        self.requeued
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back((start, count));
    }
}

/// Pops at most `want` samples off the front of `pile`, pushing a longer entry's tail
/// back to the front for the next caller. `want` must be non-zero.
fn pop_bounded(pile: &Mutex<VecDeque<(u32, u32)>>, want: u32) -> Option<(u32, u32)> {
    let mut pile = pile.lock().unwrap_or_else(PoisonError::into_inner);
    let head = pile.pop_front();
    let claimed = match head {
        Some((start, count)) if count > want => {
            pile.push_front((start + want, count - want));
            Some((start, want))
        }
        other => other,
    };
    drop(pile);
    claimed
}
