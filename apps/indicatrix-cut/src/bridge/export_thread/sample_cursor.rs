//! [`SampleCursor`]: the shared claim point local and remote both draw disjoint sample
//! sub-ranges from during an export's concurrent phase.
//!
//! # Why a shared claim point, not a one-shot split
//!
//! A fixed up-front split (measured once, dispatched once) can leave a fast engine's
//! assigned slice finished early with nothing further to claim while the export keeps
//! going, sitting idle for the rest of it. `SampleCursor` replaces that with a shared
//! claim point both engines pull fresh work from whenever free, for as long as any
//! remains.
//!
//! # Disjointness by construction
//!
//! [`claim`](Self::claim) and [`claim_local`](Self::claim_local) hand out ranges via a
//! single atomic `fetch_add`, so any number of callers on any number of threads can
//! never receive overlapping indices, by construction, with no reliance on callers
//! coordinating access any other way. Disjointness
//! matters because both the CPU tracer and GPU megakernel derive each sample's
//! pixel-jitter/RNG draws from its *absolute* index: overlapping ranges would redraw
//! identical samples and silently bias the average toward whatever indices got traced
//! twice -- wrong, but not obviously wrong.
//!
//! # A failed remote chunk's remainder only ever goes to LOCAL
//!
//! [`return_to_local`](Self::return_to_local) pushes a range remote failed to finish
//! onto a separate pile only [`claim_local`](Self::claim_local) drains, never back onto
//! the shared pool [`claim`](Self::claim) draws from. Otherwise remote could re-claim
//! its own just-failed range on its very next iteration -- an infinite loop for a
//! persistent (not transient) failure. Routing it to a local-only pile guarantees it is
//! retried at most once more, locally.

use std::{
    collections::VecDeque,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU32, Ordering},
    },
};

/// See this module's doc comment. Every method takes `&self` -- callers share one
/// instance (no `Arc` needed since `thread::scope` guarantees every borrower finishes
/// before this does) across the local loop and the remote lane's own thread.
pub(super) struct SampleCursor {
    /// Absolute index of the next sample NEITHER engine has claimed yet. Every claim is
    /// one `fetch_add` against this -- see [`claim`](Self::claim).
    next: AtomicU32,
    /// One past the last sample in this export's budget (exclusive). A claim is never
    /// honoured past this, however large a range was requested.
    end: u32,
    /// Ranges a remote chunk failed to finish, reserved for the local lane alone -- see
    /// this module's doc comment.
    local_retry: Mutex<VecDeque<(u32, u32)>>,
}

impl SampleCursor {
    /// Builds a cursor over `[start, end)`. `start` is wherever a prior sequential
    /// phase (remote/hybrid calibration) left `samples_done`; `end` is the export's
    /// total `samples_per_pixel`.
    pub(super) const fn new(start: u32, end: u32) -> Self {
        Self {
            next: AtomicU32::new(start),
            end,
            local_retry: Mutex::new(VecDeque::new()),
        }
    }

    /// Claims up to `want` fresh samples for EITHER engine, returning `Some((start,
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
    /// `start >= end` and reports `None`.
    pub(super) fn claim(&self, want: u32) -> Option<(u32, u32)> {
        if want == 0 {
            return None;
        }
        let start = self.next.fetch_add(want, Ordering::Relaxed);
        if start >= self.end {
            return None;
        }
        Some((start, want.min(self.end - start)))
    }

    /// Claims for the LOCAL lane only: a previously remote-failed range first, falling
    /// back to [`claim`](Self::claim) once that retry pile is empty. See this module's
    /// doc comment for why a retried range never goes back to remote.
    pub(super) fn claim_local(&self, want: u32) -> Option<(u32, u32)> {
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

    /// Whether the SHARED pool has nothing left to claim -- `true` once every sample up
    /// to `end` has been handed out to some engine. Says nothing about the local-only
    /// retry pile, which is `claim_local`'s alone. A cheap, lock-free read for callers
    /// deciding whether waiting around (e.g. `remote::dispatch::pause_remote_lane`)
    /// could still yield work.
    pub(super) fn shared_pool_exhausted(&self) -> bool {
        self.next.load(Ordering::Relaxed) >= self.end
    }

    /// Returns `[start, start + count)` to the queue for GUARANTEED local processing
    /// after a remote chunk failed to finish it -- never re-offered to remote. A `count`
    /// of `0` is a no-op (nothing to retry) rather than an empty queue entry.
    pub(super) fn return_to_local(&self, start: u32, count: u32) {
        if count == 0 {
            return;
        }
        self.local_retry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back((start, count));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashSet, thread};

    #[test]
    fn claim_hands_out_the_full_range_in_one_call_when_it_all_fits() {
        let cursor = SampleCursor::new(0, 100);
        assert_eq!(cursor.claim(1000), Some((0, 100)));
        assert_eq!(cursor.claim(1), None, "the budget is now exhausted");
    }

    #[test]
    fn claim_never_exceeds_the_budget_even_when_a_smaller_amount_remains() {
        let cursor = SampleCursor::new(90, 100);
        assert_eq!(cursor.claim(30), Some((90, 10)));
        assert_eq!(cursor.claim(30), None);
    }

    #[test]
    fn claim_starts_from_a_nonzero_offset() {
        // Mirrors a cursor built after some sequential calibration already advanced
        // `samples_done` past 0 before the concurrent phase begins.
        let cursor = SampleCursor::new(500, 600);
        assert_eq!(cursor.claim(50), Some((500, 50)));
        assert_eq!(cursor.claim(50), Some((550, 50)));
        assert_eq!(cursor.claim(50), None);
    }

    #[test]
    fn claim_of_zero_samples_is_always_none_and_never_advances_the_cursor() {
        let cursor = SampleCursor::new(0, 100);
        assert_eq!(cursor.claim(0), None);
        // The cursor must still be at 0 -- a `claim(0)` must not have consumed budget.
        assert_eq!(cursor.claim(100), Some((0, 100)));
    }

    #[test]
    fn claim_local_prefers_the_retry_pile_over_fresh_work() {
        let cursor = SampleCursor::new(0, 100);
        cursor.return_to_local(40, 10);
        // The retried range comes back before any fresh claim, even though fresh
        // budget was available first.
        assert_eq!(cursor.claim_local(5), Some((40, 10)));
        assert_eq!(cursor.claim_local(5), Some((0, 5)));
    }

    #[test]
    fn returned_ranges_are_invisible_to_claim() {
        // The whole point of `return_to_local` is that a remote-failed range is never
        // re-offered to remote -- `claim` (the shared pool both engines draw from) must
        // never see it.
        let cursor = SampleCursor::new(0, 0);
        cursor.return_to_local(10, 5);
        assert_eq!(cursor.claim(100), None);
        assert_eq!(cursor.claim_local(100), Some((10, 5)));
    }

    #[test]
    fn a_returned_remainder_is_reclaimable_exactly_once() {
        let cursor = SampleCursor::new(0, 0);
        cursor.return_to_local(200, 7);
        assert_eq!(cursor.claim_local(100), Some((200, 7)));
        // A second call must NOT see the same range again -- it was drained by the
        // first `pop_front`, and the shared pool has nothing left either.
        assert_eq!(cursor.claim_local(100), None);
        assert_eq!(cursor.claim(100), None);
    }

    #[test]
    fn zero_count_returns_are_a_no_op() {
        let cursor = SampleCursor::new(0, 0);
        cursor.return_to_local(10, 0);
        assert_eq!(cursor.claim_local(100), None);
    }

    /// The core correctness property: hammer `claim` from many threads at once, each
    /// grabbing small, deliberately uneven chunks, and prove the union of everything
    /// handed out is EXACTLY `[0, total)` -- no overlap, no gap, nothing over budget.
    #[test]
    fn concurrent_claims_never_overlap_never_gap_and_never_exceed_the_budget() {
        let total = 100_003u32; // deliberately not a round number or a multiple of any stride below
        let cursor = SampleCursor::new(0, total);
        let thread_count = 8;
        // Different threads request different chunk sizes -- closer to the real mix of
        // local's fixed batch size and remote's own adaptively-sized chunks than every
        // thread claiming identically.
        let strides = [17u32, 40, 101, 256, 33, 512, 7, 999];

        let claimed: Vec<Vec<(u32, u32)>> = thread::scope(|s| {
            let handles: Vec<_> = (0..thread_count)
                .map(|i| {
                    let cursor = &cursor;
                    let want = strides[i % strides.len()];
                    s.spawn(move || {
                        let mut mine = Vec::new();
                        while let Some(range) = cursor.claim(want) {
                            mine.push(range);
                        }
                        mine
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        let mut all: Vec<(u32, u32)> = claimed.into_iter().flatten().collect();
        all.sort_unstable();

        // No overlap, no gap: consecutive ranges must be exactly adjacent.
        let mut cursor_pos = 0u32;
        for (start, count) in &all {
            assert_eq!(
                *start, cursor_pos,
                "expected the next range to start exactly at {cursor_pos}, found a gap \
                 or overlap at {start}"
            );
            assert!(*count > 0, "a claimed range must never be empty");
            cursor_pos += *count;
        }
        assert_eq!(
            cursor_pos, total,
            "the union of every claimed range must cover the whole budget exactly once"
        );

        // Every individual sample index appears in exactly one claimed range -- the
        // same property as above, checked the other way (a set rather than adjacency),
        // as a second independent proof against overlap.
        let mut seen = HashSet::with_capacity(total as usize);
        for (start, count) in &all {
            for idx in *start..(*start + *count) {
                assert!(seen.insert(idx), "sample index {idx} was claimed twice");
            }
        }
        assert_eq!(seen.len(), total as usize);
    }

    /// Same concurrent-hammering property, through `claim_local` instead of `claim` --
    /// proves the retry-pile-then-fresh-pool fallback stays disjoint under real
    /// concurrency, not just when exercised one call at a time.
    #[test]
    fn concurrent_claim_local_still_partitions_the_budget_exactly() {
        let total = 5_000u32;
        let cursor = SampleCursor::new(0, total);

        thread::scope(|s| {
            let mut handles = Vec::new();
            for i in 0..4 {
                let cursor = &cursor;
                let want = 30 + i as u32 * 11;
                handles.push(s.spawn(move || {
                    let mut mine = Vec::new();
                    while let Some(range) = cursor.claim_local(want) {
                        mine.push(range);
                    }
                    mine
                }));
            }

            let claimed: Vec<Vec<(u32, u32)>> =
                handles.into_iter().map(|h| h.join().unwrap()).collect();
            let mut all: Vec<(u32, u32)> = claimed.into_iter().flatten().collect();
            all.sort_unstable();

            let mut cursor_pos = 0u32;
            for (start, count) in &all {
                assert_eq!(*start, cursor_pos);
                cursor_pos += *count;
            }
            assert_eq!(cursor_pos, total);
        });
    }
}
