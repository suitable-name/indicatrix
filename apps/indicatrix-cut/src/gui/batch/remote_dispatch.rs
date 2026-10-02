//! What the preview and tilt batches share about running SEVERAL remote dispatchers at
//! once.
//!
//! One dispatcher keeps one picture (or design) in flight on the remote and waits for
//! it, so a remote that renders a small picture in a fraction of the round trip sits
//! idle most of the time. A batch therefore spawns `AppSettings::remote_batch_lanes`
//! dispatchers over its one shared [`WorkQueue`](super::batch_queue::WorkQueue); each
//! claims, sends, waits and writes on its own, and `claim_shared` hands every item to
//! exactly one of them. Two pieces of state belong to the group rather than to any one
//! dispatcher:
//!
//! - [`DispatcherGroup`]: how many dispatchers are still running, so the local lanes'
//!   "no more requeues are coming" flag is raised only when the LAST one has ended;
//! - [`RemoteStatus`]: the remote half of the progress snapshot -- dispatchers running,
//!   items in flight, and the title of the item started most recently.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The remote dispatchers of one batch. Raises the batch's "remote lane done" flag when
/// the last of them ends, however it ends -- the flag is what lets the local lanes stop
/// waiting for further requeues (see `gui::batch::batch_queue`'s module doc comment).
pub struct DispatcherGroup<'a> {
    running: AtomicUsize,
    done: &'a AtomicBool,
}

impl<'a> DispatcherGroup<'a> {
    /// A group of `count` dispatchers that raises `done` when the last of them ends. A
    /// `count` of zero never raises it: a batch with no remote lane pre-sets the flag
    /// itself.
    #[must_use]
    pub const fn new(count: usize, done: &'a AtomicBool) -> Self {
        Self {
            running: AtomicUsize::new(count),
            done,
        }
    }

    /// A guard a dispatcher holds for its whole run: dropping it counts that dispatcher
    /// out. Drop is the signal, so a dispatcher that panics still lets the batch finish.
    /// Declare the guard first in the dispatcher, so it drops after everything the
    /// dispatcher requeued for the local lanes.
    #[must_use = "the dispatcher is counted out when the guard drops"]
    pub const fn guard(&self) -> DispatcherGuard<'_, 'a> {
        DispatcherGuard { group: self }
    }
}

/// Counts one dispatcher out of its [`DispatcherGroup`] when dropped.
pub struct DispatcherGuard<'g, 'a> {
    group: &'g DispatcherGroup<'a>,
}

impl Drop for DispatcherGuard<'_, '_> {
    fn drop(&mut self) {
        // `AcqRel`: the last dispatcher out must see every requeue the others made
        // before they counted themselves out, and the local lanes that read `done` with
        // `Acquire` must see them too.
        if self.group.running.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.group.done.store(true, Ordering::Release);
        }
    }
}

/// The remote half of a batch's progress: shared by every dispatcher, snapshotted by
/// `push_progress` for the dialog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteStatus {
    /// Dispatcher threads currently running.
    dispatchers: u32,
    /// Items sent to the remote and not yet answered.
    in_flight: u32,
    /// The title of the item a dispatcher started most recently, or a notice that the
    /// remote is being waited out; empty once the last dispatcher has ended.
    title: String,
}

impl RemoteStatus {
    /// A dispatcher thread has started.
    pub const fn dispatcher_started(&mut self) {
        self.dispatchers = self.dispatchers.saturating_add(1);
    }

    /// A dispatcher thread has ended; the last one out clears the title.
    pub fn dispatcher_ended(&mut self) {
        self.dispatchers = self.dispatchers.saturating_sub(1);
        if self.dispatchers == 0 {
            self.title.clear();
        }
    }

    /// A dispatcher has sent the item called `title` to the remote.
    pub fn item_started(&mut self, title: &str) {
        self.in_flight = self.in_flight.saturating_add(1);
        title.clone_into(&mut self.title);
    }

    /// The remote has answered, or failed, an item a dispatcher sent.
    pub const fn item_ended(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    /// Replaces the title with `text` without counting an item -- a notice, such as the
    /// remote being waited out after failures.
    pub fn note(&mut self, text: &str) {
        text.clone_into(&mut self.title);
    }

    /// Whether any dispatcher is running.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.dispatchers > 0
    }

    /// How many items are on the remote right now.
    #[must_use]
    pub const fn in_flight(&self) -> u32 {
        self.in_flight
    }

    /// The most recently started item's title, or the latest notice.
    #[must_use]
    pub const fn title(&self) -> &str {
        self.title.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::batch::batch_queue::WorkQueue;
    use std::{
        panic::{AssertUnwindSafe, catch_unwind},
        sync::Mutex,
        time::Duration,
    };

    #[test]
    fn done_is_raised_only_when_the_last_dispatcher_ends() {
        let done = AtomicBool::new(false);
        let group = DispatcherGroup::new(3, &done);
        let first = group.guard();
        let second = group.guard();
        let third = group.guard();
        drop(first);
        assert!(!done.load(Ordering::Acquire));
        drop(second);
        assert!(!done.load(Ordering::Acquire));
        drop(third);
        assert!(done.load(Ordering::Acquire));
    }

    #[test]
    fn concurrent_dispatchers_raise_done_exactly_once_all_are_out() {
        let done = AtomicBool::new(false);
        let group = DispatcherGroup::new(8, &done);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    let _guard = group.guard();
                    std::thread::sleep(std::time::Duration::from_millis(5));
                });
            }
        });
        assert!(done.load(Ordering::Acquire));
    }

    #[test]
    fn a_panicking_dispatcher_still_counts_out() {
        let done = AtomicBool::new(false);
        let group = DispatcherGroup::new(1, &done);
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let _guard = group.guard();
            panic!("dispatcher failed");
        }));
        assert!(outcome.is_err());
        assert!(
            done.load(Ordering::Acquire),
            "the local lanes must not wait forever on a dead dispatcher"
        );
    }

    #[test]
    fn an_empty_group_never_raises_the_flag() {
        let done = AtomicBool::new(false);
        let _group = DispatcherGroup::new(0, &done);
        assert!(!done.load(Ordering::Acquire));
    }

    #[test]
    fn the_status_is_active_while_any_dispatcher_runs() {
        let mut status = RemoteStatus::default();
        assert!(!status.is_active());
        status.dispatcher_started();
        status.dispatcher_started();
        assert!(status.is_active());
        status.dispatcher_ended();
        assert!(status.is_active(), "one dispatcher is still running");
        status.dispatcher_ended();
        assert!(!status.is_active());
        status.dispatcher_ended();
        assert!(!status.is_active(), "an extra end saturates at zero");
    }

    #[test]
    fn items_in_flight_are_counted_and_the_title_follows_the_latest_start() {
        let mut status = RemoteStatus::default();
        status.dispatcher_started();
        status.item_started("Round Brilliant");
        status.item_started("Princess");
        assert_eq!(status.in_flight(), 2);
        assert_eq!(status.title(), "Princess");
        status.item_ended();
        assert_eq!(status.in_flight(), 1);
        assert_eq!(
            status.title(),
            "Princess",
            "an ending leaves the title alone"
        );
        status.item_ended();
        status.item_ended();
        assert_eq!(status.in_flight(), 0, "an extra end saturates at zero");
    }

    #[test]
    fn the_last_dispatcher_out_clears_the_title() {
        let mut status = RemoteStatus::default();
        status.dispatcher_started();
        status.dispatcher_started();
        status.item_started("Oval");
        status.dispatcher_ended();
        assert_eq!(
            status.title(),
            "Oval",
            "another dispatcher is still running"
        );
        status.dispatcher_ended();
        assert_eq!(status.title(), "");
    }

    /// Four dispatchers drain one queue, handing every third item back for the local
    /// lane. The local lane follows the stop rule the engines use -- read the flag, THEN
    /// claim, and stop only on an empty claim after an observed `true` -- so it must see
    /// every requeue, and no item may be served twice or not at all.
    #[test]
    fn the_local_lane_outlives_every_requeue_of_several_dispatchers() {
        const ITEMS: u32 = 600;
        let queue = WorkQueue::new(0..ITEMS);
        let done = AtomicBool::new(false);
        let group = DispatcherGroup::new(4, &done);
        let remote_served = Mutex::new(Vec::new());
        let local_served = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    let _guard = group.guard();
                    while let Some(item) = queue.claim_shared() {
                        if item % 3 == 0 {
                            std::thread::sleep(Duration::from_micros(40));
                            queue.return_to_local(item);
                        } else {
                            remote_served.lock().unwrap().push(item);
                        }
                    }
                });
            }
            scope.spawn(|| {
                loop {
                    let remote_finished = done.load(Ordering::Acquire);
                    match queue.claim_local() {
                        Some(item) => local_served.lock().unwrap().push(item),
                        None if remote_finished => break,
                        None => std::thread::yield_now(),
                    }
                }
            });
        });
        let mut served = remote_served.into_inner().unwrap();
        served.extend(local_served.into_inner().unwrap());
        served.sort_unstable();
        assert_eq!(served, (0..ITEMS).collect::<Vec<_>>());
    }

    #[test]
    fn a_notice_replaces_the_title_without_counting_an_item() {
        let mut status = RemoteStatus::default();
        status.dispatcher_started();
        status.note("Remote paused after 2 failure(s) -- retrying in 2s");
        assert_eq!(status.in_flight(), 0);
        assert!(status.title().starts_with("Remote paused"));
    }
}
