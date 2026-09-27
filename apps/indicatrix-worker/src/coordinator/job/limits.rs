//! Coordinator job limits: a global memory budget for
//! in-flight jobs ([`MemoryBudget`]) and one active job per viewer certificate, first in
//! first out ([`ViewerQueues`]).

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Condvar, Mutex, PoisonError},
    time::Duration,
};

/// Full-resolution `Vec3` buffers one coordinator job holds at once: the emitter's
/// pending delta, running total and spare, and the lane pool's merged sum. Chunk results
/// in flight (one per lane) and chunks parked ahead of the merge frontier come on top.
pub const FRAME_BUFFERS_PER_JOB: u64 = 4;

/// Bytes one `width x height` job is charged against [`MemoryBudget`]:
/// `width * height * 12 * FRAME_BUFFERS_PER_JOB`.
#[must_use]
pub fn job_bytes(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height) * 12 * FRAME_BUFFERS_PER_JOB
}

/// A global cap on the bytes in-flight coordinator jobs may hold.
pub struct MemoryBudget {
    cap: u64,
    used: Mutex<u64>,
}

/// Bytes reserved from a [`MemoryBudget`], returned on drop.
pub struct Reservation<'a> {
    budget: &'a MemoryBudget,
    bytes: u64,
}

impl MemoryBudget {
    /// A budget of `cap` bytes.
    #[must_use]
    pub const fn new(cap: u64) -> Self {
        Self {
            cap,
            used: Mutex::new(0),
        }
    }

    /// The cap in bytes.
    #[must_use]
    pub const fn cap(&self) -> u64 {
        self.cap
    }

    /// Reserves `bytes`, or returns the bytes already in use when that would exceed the
    /// cap (nothing is queued: the request is refused).
    ///
    /// # Errors
    ///
    /// `Err(in_use)` past the cap.
    pub fn try_reserve(&self, bytes: u64) -> Result<Reservation<'_>, u64> {
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        if used.saturating_add(bytes) > self.cap {
            return Err(*used);
        }
        *used += bytes;
        drop(used);
        Ok(Reservation {
            budget: self,
            bytes,
        })
    }

    /// Bytes currently reserved.
    #[must_use]
    pub fn in_use(&self) -> u64 {
        *self.used.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        let mut used = self
            .budget
            .used
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *used = used.saturating_sub(self.bytes);
    }
}

/// Per-viewer FIFO queues: at most one active job per viewer (certificate), the rest
/// waiting in arrival order (a fair queue across viewers is backlog).
#[derive(Default)]
pub struct ViewerQueues {
    queues: Mutex<(u64, Queues)>,
    changed: Condvar,
}

/// Each viewer's waiting and active tickets, front = active.
type Queues = HashMap<Arc<str>, VecDeque<u64>>;

/// A viewer's turn: its job is the active one until this is dropped.
pub struct Turn<'a> {
    queues: &'a ViewerQueues,
    viewer: Arc<str>,
    ticket: u64,
}

impl ViewerQueues {
    /// How often a waiting job re-checks its cancellation.
    const RECHECK: Duration = Duration::from_millis(50);

    /// Queues a job for `viewer` and blocks until it is at the front, or returns `None`
    /// (dequeued) as soon as `cancelled()` says so.
    pub fn wait_turn(&self, viewer: &Arc<str>, cancelled: impl Fn() -> bool) -> Option<Turn<'_>> {
        let mut guard = self.queues.lock().unwrap_or_else(PoisonError::into_inner);
        let ticket = guard.0;
        guard.0 += 1;
        guard
            .1
            .entry(Arc::clone(viewer))
            .or_default()
            .push_back(ticket);
        loop {
            if guard.1.get(viewer).and_then(VecDeque::front) == Some(&ticket) {
                return Some(Turn {
                    queues: self,
                    viewer: Arc::clone(viewer),
                    ticket,
                });
            }
            if cancelled() {
                Self::remove(&mut guard.1, viewer, ticket);
                drop(guard);
                self.changed.notify_all();
                return None;
            }
            guard = self
                .changed
                .wait_timeout(guard, Self::RECHECK)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Jobs queued or active for `viewer`.
    #[cfg(test)]
    #[must_use]
    pub fn queued(&self, viewer: &str) -> usize {
        self.queues
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .1
            .get(viewer)
            .map_or(0, VecDeque::len)
    }

    fn remove(queues: &mut Queues, viewer: &Arc<str>, ticket: u64) {
        if let Some(queue) = queues.get_mut(viewer) {
            queue.retain(|&t| t != ticket);
            if queue.is_empty() {
                queues.remove(viewer);
            }
        }
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let mut guard = self
            .queues
            .queues
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        ViewerQueues::remove(&mut guard.1, &self.viewer, self.ticket);
        drop(guard);
        self.queues.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn the_budget_refuses_past_its_cap_and_frees_on_drop() {
        let budget = MemoryBudget::new(100);
        let a = budget.try_reserve(60).unwrap();
        assert_eq!(budget.try_reserve(50).err(), Some(60));
        drop(a);
        assert_eq!(budget.in_use(), 0);
        assert!(budget.try_reserve(100).is_ok());
        assert_eq!(job_bytes(10, 10), 100 * 12 * FRAME_BUFFERS_PER_JOB);
    }

    #[test]
    fn one_viewer_runs_one_job_at_a_time_in_arrival_order() {
        let queues = Arc::new(ViewerQueues::default());
        let viewer: Arc<str> = Arc::from("laptop");
        let other: Arc<str> = Arc::from("desktop");
        let first = queues.wait_turn(&viewer, || false).unwrap();
        // Another viewer is never blocked by this one.
        drop(queues.wait_turn(&other, || false).unwrap());

        let started = Arc::new(AtomicBool::new(false));
        let waiter = {
            let (queues, viewer, started) = (
                Arc::clone(&queues),
                Arc::clone(&viewer),
                Arc::clone(&started),
            );
            std::thread::spawn(move || {
                let _turn = queues.wait_turn(&viewer, || false).unwrap();
                started.store(true, Ordering::SeqCst);
            })
        };
        std::thread::sleep(Duration::from_millis(150));
        assert!(!started.load(Ordering::SeqCst), "the second job must wait");
        assert_eq!(queues.queued("laptop"), 2);
        drop(first);
        waiter.join().unwrap();
        assert!(started.load(Ordering::SeqCst));
        assert_eq!(queues.queued("laptop"), 0);
    }

    #[test]
    fn a_cancelled_waiter_leaves_the_queue() {
        let queues = ViewerQueues::default();
        let viewer: Arc<str> = Arc::from("laptop");
        let _first = queues.wait_turn(&viewer, || false).unwrap();
        assert!(queues.wait_turn(&viewer, || true).is_none());
        assert_eq!(queues.queued("laptop"), 1);
    }
}
