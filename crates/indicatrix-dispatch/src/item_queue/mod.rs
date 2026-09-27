//! [`ItemQueue`]: whole-item work distribution over N lanes (batch previews, tilt-curve
//! sets, anything that cannot be split into sample ranges).
//!
//! Generalised from the desktop batch's `WorkQueue` (one local pile, one remote lane,
//! remote failures go to local only) to N equal lanes:
//!
//! - Items are handed out one at a time; whichever lane is free claims the next, so a
//!   faster lane naturally does more and no throughput model is needed.
//! - A failed item goes back to a retry pile, **never to a lane that already failed
//!   it** -- the N-lane form of "a remote failure is retried locally only". An item that
//!   fails for a persistent reason is therefore tried at most once per lane.
//! - An item every still-running lane has failed is **abandoned** and handed back to
//!   the caller to report as failed.
//! - A lane with nothing claimable waits while any item is in flight (it may fail and
//!   become claimable), and exits otherwise; exiting counts as retiring the lane.
//!
//! # No item is lost or duplicated
//!
//! Every item leaves the queue exactly once, through exactly one of:
//! [`ItemQueue::complete`], [`FailOutcome::Abandoned`], the list [`ItemQueue::retire`]
//! returns, [`ItemQueue::take_abandoned`] (items abandoned when a lane exited from
//! [`ItemQueue::claim`]), or [`ItemQueue::close`]. All state lives under one mutex.

use std::{
    collections::VecDeque,
    sync::{Condvar, Mutex, MutexGuard, PoisonError},
};

#[cfg(test)]
mod tests;

/// One claimed item. Hand it back through [`ItemQueue::complete`] or
/// [`ItemQueue::fail`]; the queue counts it as in flight until then, so dropping a
/// ticket instead would keep idle lanes waiting for it forever.
#[derive(Debug)]
#[must_use = "hand the ticket back through ItemQueue::complete or ItemQueue::fail"]
pub struct ItemTicket<T> {
    item: T,
    /// Lanes that already failed this item.
    failed_on: Vec<usize>,
}

impl<T> ItemTicket<T> {
    /// The claimed item.
    #[must_use]
    pub const fn item(&self) -> &T {
        &self.item
    }

    /// How many lanes failed this item before.
    #[must_use]
    pub const fn previous_failures(&self) -> usize {
        self.failed_on.len()
    }
}

/// What [`ItemQueue::fail`] did with the item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailOutcome<T> {
    /// Another running lane that has not failed it yet will get it.
    Requeued,
    /// Every running lane has failed it; report it as failed.
    Abandoned(T),
}

/// A snapshot of the queue's bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueCounts {
    /// Items not yet claimed (fresh plus retry).
    pub pending: usize,
    /// Items claimed and not yet completed or failed.
    pub in_flight: usize,
    /// Items completed.
    pub completed: usize,
    /// Items abandoned (all of them, however they were handed back).
    pub abandoned: usize,
}

#[derive(Debug)]
struct Entry<T> {
    item: T,
    failed_on: Vec<usize>,
}

#[derive(Debug)]
struct QueueState<T> {
    fresh: VecDeque<Entry<T>>,
    retry: VecDeque<Entry<T>>,
    /// `live[lane]`: the lane may still claim.
    live: Vec<bool>,
    in_flight: usize,
    completed: usize,
    abandoned: usize,
    /// Items abandoned when a lane exited from `claim`, awaiting `take_abandoned`.
    abandoned_unclaimed: Vec<T>,
    closed: bool,
}

impl<T> QueueState<T> {
    fn is_live(&self, lane: usize) -> bool {
        self.live.get(lane).copied().unwrap_or(false)
    }

    /// Whether no running lane is left that has not failed `entry`.
    fn nobody_left_for(&self, entry: &Entry<T>) -> bool {
        self.live
            .iter()
            .enumerate()
            .all(|(lane, &live)| !live || entry.failed_on.contains(&lane))
    }

    /// The next item for `lane`: a retry it has not failed first, then fresh work.
    fn take_for(&mut self, lane: usize) -> Option<Entry<T>> {
        let retried = self
            .retry
            .iter()
            .position(|entry| !entry.failed_on.contains(&lane))
            .and_then(|index| self.retry.remove(index));
        retried.or_else(|| self.fresh.pop_front())
    }

    /// Marks `lane` as no longer claiming and removes every item nobody running can
    /// take any more.
    fn retire(&mut self, lane: usize) -> Vec<T> {
        if let Some(live) = self.live.get_mut(lane) {
            *live = false;
        }
        let mut dropped = Vec::new();
        let retry = std::mem::take(&mut self.retry);
        for entry in retry {
            if self.nobody_left_for(&entry) {
                dropped.push(entry.item);
            } else {
                self.retry.push_back(entry);
            }
        }
        if !self.live.iter().any(|&live| live) {
            dropped.extend(self.fresh.drain(..).map(|entry| entry.item));
        }
        self.abandoned += dropped.len();
        dropped
    }
}

/// See the module doc. Shared by reference between the lane threads.
#[derive(Debug)]
pub struct ItemQueue<T> {
    state: Mutex<QueueState<T>>,
    wake: Condvar,
}

impl<T> ItemQueue<T> {
    /// A queue of `items` for lanes `0..lane_count`.
    pub fn new(items: impl IntoIterator<Item = T>, lane_count: usize) -> Self {
        Self {
            state: Mutex::new(QueueState {
                fresh: items
                    .into_iter()
                    .map(|item| Entry {
                        item,
                        failed_on: Vec::new(),
                    })
                    .collect(),
                retry: VecDeque::new(),
                live: vec![true; lane_count],
                in_flight: 0,
                completed: 0,
                abandoned: 0,
                abandoned_unclaimed: Vec::new(),
                closed: false,
            }),
            wake: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, QueueState<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Claims the next item for `lane`, blocking while nothing is claimable for it but
    /// another item is in flight. `None` means this lane is done for good (nothing it
    /// can take is left, the lane was retired, or the queue was closed); items only it
    /// could still have taken are then abandoned (see [`Self::take_abandoned`]).
    pub fn claim(&self, lane: usize) -> Option<ItemTicket<T>> {
        let mut state = self.lock();
        loop {
            if state.closed || !state.is_live(lane) {
                return None;
            }
            if let Some(entry) = state.take_for(lane) {
                state.in_flight += 1;
                return Some(ItemTicket {
                    item: entry.item,
                    failed_on: entry.failed_on,
                });
            }
            if state.in_flight == 0 {
                let dropped = state.retire(lane);
                state.abandoned_unclaimed.extend(dropped);
                drop(state);
                self.wake.notify_all();
                return None;
            }
            state = self
                .wake
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// The item was processed successfully; returns it.
    pub fn complete(&self, ticket: ItemTicket<T>) -> T {
        let mut state = self.lock();
        state.in_flight -= 1;
        state.completed += 1;
        drop(state);
        self.wake.notify_all();
        ticket.item
    }

    /// `lane` failed the item: it goes back for a running lane that has not failed it,
    /// or is abandoned when there is none.
    #[must_use = "an abandoned item must be reported"]
    pub fn fail(&self, lane: usize, ticket: ItemTicket<T>) -> FailOutcome<T> {
        let mut entry = Entry {
            item: ticket.item,
            failed_on: ticket.failed_on,
        };
        if !entry.failed_on.contains(&lane) {
            entry.failed_on.push(lane);
        }
        let mut state = self.lock();
        state.in_flight -= 1;
        let outcome = if state.nobody_left_for(&entry) {
            state.abandoned += 1;
            FailOutcome::Abandoned(entry.item)
        } else {
            state.retry.push_back(entry);
            FailOutcome::Requeued
        };
        drop(state);
        self.wake.notify_all();
        outcome
    }

    /// Takes `lane` out of the rotation (it keeps failing, its worker went away) and
    /// returns the items that no running lane can take any more.
    #[must_use = "the returned items were abandoned and must be reported"]
    pub fn retire(&self, lane: usize) -> Vec<T> {
        let dropped = self.lock().retire(lane);
        self.wake.notify_all();
        dropped
    }

    /// Items abandoned when a lane exited from [`Self::claim`] (drained by this call).
    pub fn take_abandoned(&self) -> Vec<T> {
        std::mem::take(&mut self.lock().abandoned_unclaimed)
    }

    /// Stops the queue (cancellation): every later [`Self::claim`] returns `None`, and
    /// every item not yet claimed -- plus any not yet taken with
    /// [`Self::take_abandoned`] -- is returned. Items in flight still come back through
    /// [`Self::complete`] / [`Self::fail`].
    #[must_use = "the returned items were never processed"]
    pub fn close(&self) -> Vec<T> {
        let mut state = self.lock();
        state.closed = true;
        let mut left = std::mem::take(&mut state.abandoned_unclaimed);
        left.extend(state.retry.drain(..).map(|entry| entry.item));
        left.extend(state.fresh.drain(..).map(|entry| entry.item));
        drop(state);
        self.wake.notify_all();
        left
    }

    /// The queue's current bookkeeping.
    #[must_use]
    pub fn counts(&self) -> QueueCounts {
        let state = self.lock();
        let counts = QueueCounts {
            pending: state.fresh.len() + state.retry.len(),
            in_flight: state.in_flight,
            completed: state.completed,
            abandoned: state.abandoned,
        };
        drop(state);
        counts
    }
}
