//! Tests for [`ItemQueue`].

use super::*;
use std::{thread, time::Duration};

fn done(queue: &ItemQueue<i32>, lane: usize) -> Option<i32> {
    queue.claim(lane).map(|ticket| queue.complete(ticket))
}

#[test]
fn a_single_lane_drains_in_fifo_order() {
    let queue = ItemQueue::new([1, 2, 3], 1);
    assert_eq!(done(&queue, 0), Some(1));
    assert_eq!(done(&queue, 0), Some(2));
    assert_eq!(done(&queue, 0), Some(3));
    assert_eq!(done(&queue, 0), None);
    assert_eq!(
        queue.counts(),
        QueueCounts {
            pending: 0,
            in_flight: 0,
            completed: 3,
            abandoned: 0
        }
    );
}

#[test]
fn a_failed_item_is_never_offered_again_to_the_lane_that_failed_it() {
    let queue = ItemQueue::new([1, 2], 2);
    let first = queue.claim(0).unwrap();
    assert_eq!(*first.item(), 1);
    assert_eq!(queue.fail(0, first), FailOutcome::Requeued);
    // Lane 0 skips its own failure and takes fresh work instead.
    let second = queue.claim(0).unwrap();
    assert_eq!(*second.item(), 2);
    // Lane 1 gets the retried item first, and sees it failed once before.
    let retried = queue.claim(1).unwrap();
    assert_eq!((*retried.item(), retried.previous_failures()), (1, 1));
    assert_eq!(queue.complete(retried), 1);
    assert_eq!(queue.complete(second), 2);
    assert_eq!(done(&queue, 0), None);
}

#[test]
fn an_item_every_running_lane_failed_is_abandoned() {
    let queue = ItemQueue::new([7], 2);
    let ticket = queue.claim(0).unwrap();
    assert_eq!(queue.fail(0, ticket), FailOutcome::Requeued);
    let ticket = queue.claim(1).unwrap();
    assert_eq!(queue.fail(1, ticket), FailOutcome::Abandoned(7));
    assert_eq!(queue.counts().abandoned, 1);
    assert!(queue.claim(0).is_none());
}

#[test]
fn retiring_lanes_abandons_what_nobody_left_can_take() {
    let queue = ItemQueue::new([1, 2, 3], 2);
    let ticket = queue.claim(1).unwrap();
    assert_eq!(queue.fail(1, ticket), FailOutcome::Requeued);
    // Only lane 0 could still take item 1; retiring it abandons 1 and, with no lane
    // left at all, every fresh item too.
    let mut dropped = queue.retire(0);
    dropped.sort_unstable();
    assert_eq!(dropped, [1]);
    assert_eq!(done(&queue, 1), Some(2));
    let mut last = queue.retire(1);
    last.sort_unstable();
    assert_eq!(last, [3]);
    assert!(queue.claim(0).is_none());
    assert!(queue.claim(1).is_none());
}

#[test]
fn a_lane_with_nothing_left_waits_for_an_in_flight_failure() {
    let queue = ItemQueue::new([5], 2);
    let ticket = queue.claim(0).unwrap();
    thread::scope(|scope| {
        let waiter = scope.spawn(|| done(&queue, 1));
        thread::sleep(Duration::from_millis(30));
        assert_eq!(queue.fail(0, ticket), FailOutcome::Requeued);
        assert_eq!(waiter.join().unwrap(), Some(5));
    });
    assert_eq!(done(&queue, 0), None);
}

#[test]
fn an_exiting_lane_counts_as_retired_for_abandonment() {
    // Lane 1 fails item 9 and exits (nothing it can take, nothing in flight); lane 0
    // then fails it too -- nobody running is left for it, so it is abandoned rather
    // than stuck in the retry pile.
    let queue = ItemQueue::new([9], 2);
    let ticket = queue.claim(1).unwrap();
    assert_eq!(queue.fail(1, ticket), FailOutcome::Requeued);
    assert!(queue.claim(1).is_none());
    let ticket = queue.claim(0).unwrap();
    assert_eq!(queue.fail(0, ticket), FailOutcome::Abandoned(9));
    assert!(queue.claim(0).is_none());
}

#[test]
fn close_returns_every_unclaimed_item_and_stops_claims() {
    let queue = ItemQueue::new(0..5, 2);
    let ticket = queue.claim(0).unwrap();
    let mut left = queue.close();
    left.sort_unstable();
    assert_eq!(left, [1, 2, 3, 4]);
    assert!(queue.claim(1).is_none());
    assert_eq!(queue.complete(ticket), 0);
}

/// Many lanes, deterministic per-lane failures (lane 7 fails everything): every item
/// ends up completed or abandoned exactly once.
#[test]
fn concurrent_lanes_with_failures_never_lose_or_duplicate_an_item() {
    const ITEMS: i32 = 2_000;
    const LANES: usize = 8;
    let queue = ItemQueue::new(0..ITEMS, LANES);
    let results: Vec<(Vec<i32>, Vec<i32>)> = thread::scope(|scope| {
        let handles: Vec<_> = (0..LANES)
            .map(|lane| {
                let queue = &queue;
                scope.spawn(move || {
                    let (mut completed, mut abandoned) = (Vec::new(), Vec::new());
                    while let Some(ticket) = queue.claim(lane) {
                        let item = *ticket.item();
                        let fails = lane == 7 || (item + lane as i32) % 5 == 0;
                        if fails {
                            if let FailOutcome::Abandoned(item) = queue.fail(lane, ticket) {
                                abandoned.push(item);
                            }
                        } else {
                            completed.push(queue.complete(ticket));
                        }
                    }
                    (completed, abandoned)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut all: Vec<i32> = queue.take_abandoned();
    for (completed, abandoned) in results {
        all.extend(completed);
        all.extend(abandoned);
    }
    all.sort_unstable();
    assert_eq!(
        all,
        (0..ITEMS).collect::<Vec<_>>(),
        "no item lost or duplicated"
    );
    let counts = queue.counts();
    assert_eq!((counts.pending, counts.in_flight), (0, 0));
    assert_eq!(counts.completed + counts.abandoned, ITEMS as usize);
    assert_eq!(
        counts.abandoned, 0,
        "some lane can always finish each item here"
    );
}
