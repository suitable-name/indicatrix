//! Tests for [`SampleCursor`] (moved with it from `indicatrix-cut`), plus the
//! pool-side `requeue`/`claim_any` pile.

use super::SampleCursor;
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
    cursor.requeue(10, 0);
    assert_eq!(cursor.claim_local(100), None);
    assert_eq!(cursor.claim_any(100), None);
    assert!(cursor.fully_claimed());
}

#[test]
fn claim_local_bounded_splits_a_long_retried_range_into_want_sized_pieces() {
    let cursor = SampleCursor::new(0, 0);
    cursor.return_to_local(100, 10);
    assert_eq!(cursor.claim_local_bounded(4), Some((100, 4)));
    assert_eq!(cursor.claim_local_bounded(4), Some((104, 4)));
    assert_eq!(cursor.claim_local_bounded(4), Some((108, 2)));
    assert_eq!(cursor.claim_local_bounded(4), None);
}

#[test]
fn claim_local_bounded_of_zero_touches_nothing() {
    let cursor = SampleCursor::new(0, 10);
    cursor.return_to_local(50, 3);
    assert_eq!(cursor.claim_local_bounded(0), None);
    assert_eq!(cursor.claim_local_bounded(8), Some((50, 3)));
    assert_eq!(cursor.claim_local_bounded(8), Some((0, 8)));
}

#[test]
fn claim_any_drains_requeued_ranges_first_and_splits_them() {
    let cursor = SampleCursor::new(0, 20);
    assert_eq!(cursor.claim_any(8), Some((0, 8)));
    cursor.requeue(3, 5);
    assert_eq!(cursor.claim_any(2), Some((3, 2)));
    assert_eq!(cursor.claim_any(10), Some((5, 3)));
    assert_eq!(cursor.claim_any(10), Some((8, 10)));
    assert_eq!(cursor.claim_any(10), Some((18, 2)));
    assert!(cursor.fully_claimed());
    assert_eq!(cursor.claim_any(10), None);
}

#[test]
fn requeued_ranges_are_invisible_to_claim_and_claim_local() {
    let cursor = SampleCursor::new(0, 0);
    cursor.requeue(40, 4);
    assert!(!cursor.fully_claimed());
    assert_eq!(cursor.claim(10), None);
    assert_eq!(cursor.claim_local(10), None);
    assert_eq!(cursor.claim_any(10), Some((40, 4)));
    assert!(cursor.fully_claimed());
}

/// The core correctness property: hammer `claim` from many threads at once, each
/// grabbing small, deliberately uneven chunks, and prove the union of everything
/// handed out is EXACTLY `[0, total)` -- no overlap, no gap, nothing over budget.
#[test]
fn concurrent_claims_never_overlap_never_gap_and_never_exceed_the_budget() {
    let total = 100_003u32; // deliberately not a round number or a multiple of any stride below
    let cursor = SampleCursor::new(0, total);
    let thread_count = 8;
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
    assert_partition(&all, 0, total);
}

/// Same concurrent-hammering property, through `claim_local` instead of `claim`.
#[test]
fn concurrent_claim_local_still_partitions_the_budget_exactly() {
    let total = 5_000u32;
    let cursor = SampleCursor::new(0, total);

    let claimed: Vec<Vec<(u32, u32)>> = thread::scope(|s| {
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
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut all: Vec<(u32, u32)> = claimed.into_iter().flatten().collect();
    all.sort_unstable();
    assert_partition(&all, 0, total);
}

/// Concurrent `claim_any` callers that each requeue part of every claim they take (a
/// failing lane's remainder) still partition the budget exactly once overall.
#[test]
fn concurrent_claim_any_with_requeues_partitions_the_budget_exactly() {
    let total = 20_011u32;
    let cursor = SampleCursor::new(0, total);
    let claimed: Vec<Vec<(u32, u32)>> = thread::scope(|s| {
        let handles: Vec<_> = (0..6u32)
            .map(|i| {
                let cursor = &cursor;
                s.spawn(move || {
                    let mut mine = Vec::new();
                    let mut n = 0u32;
                    while let Some((start, count)) = cursor.claim_any(13 + i * 7) {
                        n += 1;
                        // Every third claim "fails" after half of its range.
                        if n.is_multiple_of(3) && count > 1 {
                            let done = count / 2;
                            mine.push((start, done));
                            cursor.requeue(start + done, count - done);
                        } else {
                            mine.push((start, count));
                        }
                    }
                    mine
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut all: Vec<(u32, u32)> = claimed.into_iter().flatten().collect();
    all.sort_unstable();
    assert_partition(&all, 0, total);
}

/// Asserts `sorted` (sorted by start) tiles `[start, end)` exactly: adjacent, non-empty,
/// no overlap, no gap, and every index seen exactly once.
fn assert_partition(sorted: &[(u32, u32)], start: u32, end: u32) {
    let mut pos = start;
    for (s, c) in sorted {
        assert_eq!(*s, pos, "gap or overlap at {s} (expected {pos})");
        assert!(*c > 0, "a claimed range must never be empty");
        pos += *c;
    }
    assert_eq!(
        pos, end,
        "the ranges must cover the whole budget exactly once"
    );
    let mut seen = HashSet::new();
    for (s, c) in sorted {
        for idx in *s..(*s + *c) {
            assert!(seen.insert(idx), "sample index {idx} was claimed twice");
        }
    }
}
