//! Shared device budget. Live tracing and exports compete for the same permits.
//! A permit is released at a bounded chunk boundary, including while cooling.

use std::{
    sync::{Condvar, Mutex, PoisonError},
    time::Duration,
};

struct Budget {
    capacity: usize,
    active: usize,
    closed: bool,
}

pub struct ComputePolicy {
    budget: Mutex<Budget>,
    wake: Condvar,
}

impl ComputePolicy {
    #[must_use]
    pub const fn new(capacity: usize) -> Self {
        Self {
            budget: Mutex::new(Budget {
                capacity,
                active: 0,
                closed: false,
            }),
            wake: Condvar::new(),
        }
    }

    pub fn set_capacity(&self, capacity: usize) {
        self.budget
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .capacity = capacity;
        self.wake.notify_all();
    }

    #[must_use]
    pub fn paused(&self) -> bool {
        let budget = self.budget.lock().unwrap_or_else(PoisonError::into_inner);
        budget.capacity == 0 || budget.closed
    }

    /// Waits off the UI thread, checking cancellation at least every 50 ms.
    pub fn acquire(&self, cancelled: &dyn Fn() -> bool) -> Option<ComputePermit<'_>> {
        loop {
            // Never call engine cancellation while holding the budget lock: engines
            // may hold their own control lock when updating this budget.
            if cancelled() {
                return None;
            }
            let mut budget = self.budget.lock().unwrap_or_else(PoisonError::into_inner);
            if budget.closed {
                return None;
            }
            if budget.active < budget.capacity {
                budget.active += 1;
                return Some(ComputePermit(self));
            }
            drop(
                self.wake
                    .wait_timeout(budget, Duration::from_millis(50))
                    .unwrap_or_else(PoisonError::into_inner),
            );
        }
    }

    pub fn close(&self) {
        self.budget
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed = true;
        self.wake.notify_all();
    }
}

pub struct ComputePermit<'a>(&'a ComputePolicy);

impl Drop for ComputePermit<'_> {
    fn drop(&mut self) {
        self.0
            .budget
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active -= 1;
        self.0.wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_budget_obeys_cancellation_and_releases_capacity() {
        let policy = ComputePolicy::new(1);
        let permit = policy.acquire(&|| false).expect("capacity");
        assert!(policy.acquire(&|| true).is_none());
        drop(permit);
        policy.set_capacity(0);
        assert!(policy.paused());
        assert!(policy.acquire(&|| true).is_none());
        policy.set_capacity(1);
        assert!(policy.acquire(&|| false).is_some());
        policy.close();
        assert!(policy.acquire(&|| false).is_none());
    }
}
