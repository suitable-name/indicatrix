//! Coordinator job limits: a global memory budget for
//! in-flight jobs ([`MemoryBudget`]) and one active job per viewer certificate, first in
//! first out ([`ViewerQueues`]).

use super::Coordinator;
use indicatrix_dispatch::ParkedBudget;
use std::{
    collections::{HashMap, VecDeque},
    fmt,
    sync::{Arc, Condvar, Mutex, PoisonError},
    time::Duration,
};

/// Bytes of one full-resolution `Vec3` radiance pixel.
const BYTES_PER_PIXEL: u64 = 12;

/// Full-resolution `Vec3` buffers one coordinator job holds at once regardless of how
/// many lanes it runs: the emitter's pending delta, running total and spare, and the
/// lane pool's merged sum. The per-lane buffers ([`FRAME_BUFFERS_PER_LANE`]) and chunks
/// parked ahead of the merge frontier (charged through [`JobParkedBudget`]) come on top.
pub const FRAME_BUFFERS_PER_JOB: u64 = 4;

/// Full-resolution `Vec3` buffers one lane of a job can hold at the same time, counted
/// from the code:
///
/// - a joined worker lane: the reply accumulator of the request in flight
///   (`lanes::chunk::Reply::new`'s `sum`), the running total the chunk's requests fold
///   into (`lanes::JoinedWorkerLane::run_on`'s `total`, which becomes the chunk result)
///   and the copy held back for the emitter until the merge confirms the chunk
///   (`producer::SinkLane::render_chunk`'s `pending`, which a chunk the merger refused
///   leaves in place while the next chunk already runs);
/// - the coordinator's own lane: the tracer's pending delta
///   (`stream_emit::SharedState::new`), the returned sum (`stream_emit::trace_range`'s
///   `sum`) and the hybrid calibration buffer (`calibrate_hybrid_split`'s `calib_buf`),
///   with the same `pending` copy replacing the delta once the chunk returns.
///
/// Both kinds peak at three, so the estimate charges three for every lane.
pub const FRAME_BUFFERS_PER_LANE: u64 = 3;

/// Bytes the lane-independent part of a `width x height` job is charged against
/// [`MemoryBudget`]: `width * height * 12 * FRAME_BUFFERS_PER_JOB`.
///
/// A job with a reserved viewer contribution (`FinalImageRequest.viewer_samples > 0`)
/// and a job with an HDR map are charged more on top of this, and every lane adds
/// [`lane_bytes`]; [`job_estimate_bytes`] is the whole admission figure.
#[must_use]
pub fn job_bytes(width: u32, height: u32) -> u64 {
    frame_bytes(width, height) * FRAME_BUFFERS_PER_JOB
}

/// Bytes `lanes` lanes of a `width x height` job are charged:
/// `width * height * 12 * FRAME_BUFFERS_PER_LANE` each.
#[must_use]
pub fn lane_bytes(width: u32, height: u32, lanes: u32) -> u64 {
    frame_bytes(width, height) * FRAME_BUFFERS_PER_LANE * u64::from(lanes)
}

/// Bytes the viewer's reserved contribution holds while it is in flight: one full-resolution
/// buffer (`stream_emit::ContributionSlot`).
#[must_use]
pub fn contribution_bytes(width: u32, height: u32) -> u64 {
    frame_bytes(width, height)
}

/// Bytes of the HDR map a job pins for its whole run: the encoded file (shared by every
/// lane through one `Arc`, see `assets::HeldAsset::bytes`) and, when the own lane
/// renders it, the decoded texels (`width * height * 12`). The importance-sampling
/// tables built beside the texels are not included.
#[must_use]
pub fn hdr_bytes(encoded_len: u64, decoded_texels: Option<u64>) -> u64 {
    encoded_len.saturating_add(decoded_texels.map_or(0, |texels| texels.saturating_mul(12)))
}

/// Everything a job is charged against [`MemoryBudget`]: the lane-independent frame
/// buffers ([`job_bytes`]), `lanes` lanes ([`lane_bytes`]) and `extra_bytes` (the
/// [`contribution_bytes`] and [`hdr_bytes`] the job holds). The producer reserves the
/// lane-independent part and `extra_bytes` when the job starts and the lane part once
/// the lanes are checked out; the sum is what must fit.
#[must_use]
pub fn job_estimate_bytes(width: u32, height: u32, lanes: u32, extra_bytes: u64) -> u64 {
    job_bytes(width, height)
        .saturating_add(lane_bytes(width, height, lanes))
        .saturating_add(extra_bytes)
}

/// Bytes of one full-resolution `Vec3` frame.
fn frame_bytes(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height) * BYTES_PER_PIXEL
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

    /// Charges `bytes` against the cap without an RAII guard: for a caller (a
    /// [`indicatrix_dispatch::Merger`]'s parked chunks, see [`JobParkedBudget`]) that
    /// reserves and releases from entirely different call sites -- not one scope a
    /// [`Reservation`] could span. `false` past the cap; nothing is charged then.
    pub fn charge(&self, bytes: u64) -> bool {
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        if used.saturating_add(bytes) > self.cap {
            return false;
        }
        *used += bytes;
        true
    }

    /// Releases `bytes` charged through [`Self::charge`].
    pub fn uncharge(&self, bytes: u64) {
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        *used = used.saturating_sub(bytes);
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

/// Adapts [`MemoryBudget`] to [`indicatrix_dispatch::ParkedBudget`] so a job's
/// [`indicatrix_dispatch::Merger`] charges its parked (full-frame) buffers against the
/// same coordinator-wide cap [`job_bytes`]'s fixed [`FRAME_BUFFERS_PER_JOB`] charge
/// uses: a stalled lane's straggling frontier would otherwise park an
/// unbounded number of full-frame buffers even though the job's own fixed reservation
/// never grows.
pub struct JobParkedBudget(pub Arc<Coordinator>);

impl fmt::Debug for JobParkedBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobParkedBudget").finish_non_exhaustive()
    }
}

impl ParkedBudget for JobParkedBudget {
    fn reserve(&self, bytes: u64) -> bool {
        self.0.budget().charge(bytes)
    }

    fn release(&self, bytes: u64) {
        self.0.budget().uncharge(bytes);
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
    fn charge_and_uncharge_share_the_cap_with_try_reserve() {
        let budget = MemoryBudget::new(100);
        let reservation = budget.try_reserve(60).unwrap();
        assert!(budget.charge(40));
        assert_eq!(budget.in_use(), 100);
        assert!(!budget.charge(1), "past the shared cap");
        budget.uncharge(40);
        assert_eq!(budget.in_use(), 60);
        drop(reservation);
        assert_eq!(budget.in_use(), 0);
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
        // The waiter reports each pass of its wait loop (`wait_turn` polls `cancelled`
        // once per pass while it is not at the front), so the assertion below runs only
        // after it has provably been queued behind `first` and waited through several
        // passes -- however late the thread was scheduled.
        let (pass_tx, pass_rx) = std::sync::mpsc::channel();
        let waiter = {
            let (queues, viewer, started) = (
                Arc::clone(&queues),
                Arc::clone(&viewer),
                Arc::clone(&started),
            );
            std::thread::spawn(move || {
                let _turn = queues
                    .wait_turn(&viewer, || {
                        let _ = pass_tx.send(());
                        false
                    })
                    .unwrap();
                started.store(true, Ordering::SeqCst);
            })
        };
        for pass in 1..=3 {
            pass_rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|_| {
                    panic!("the second job never reached wait pass {pass} of its viewer's queue")
                });
        }
        assert!(!started.load(Ordering::SeqCst), "the second job must wait");
        assert_eq!(queues.queued("laptop"), 2);
        drop(first);
        waiter.join().unwrap();
        assert!(started.load(Ordering::SeqCst));
        assert_eq!(queues.queued("laptop"), 0);
    }

    /// The default `--max-job-memory-mib` (2 GiB) in bytes.
    const DEFAULT_BUDGET: u64 = 2048 * 1024 * 1024;

    #[test]
    fn the_estimate_charges_every_lane_its_frame_buffers() {
        let (w, h) = (100, 10);
        let frame = 100 * 10 * 12;
        assert_eq!(lane_bytes(w, h, 1), frame * FRAME_BUFFERS_PER_LANE);
        assert_eq!(lane_bytes(w, h, 0), 0);
        assert_eq!(
            job_estimate_bytes(w, h, 5, 7),
            frame * (FRAME_BUFFERS_PER_JOB + 5 * FRAME_BUFFERS_PER_LANE) + 7
        );
        assert_eq!(contribution_bytes(w, h), frame);
        assert_eq!(hdr_bytes(1000, None), 1000);
        assert_eq!(hdr_bytes(1000, Some(10)), 1000 + 120);
    }

    #[test]
    fn an_8k_job_on_ten_lanes_exceeds_the_default_budget_while_1080p_on_two_lanes_fits() {
        let budget = MemoryBudget::new(DEFAULT_BUDGET);
        let huge = job_estimate_bytes(7680, 4320, 10, 0);
        assert!(huge > DEFAULT_BUDGET, "{huge} bytes must not fit");
        assert!(budget.try_reserve(huge).is_err());

        let small = job_estimate_bytes(1920, 1080, 2, 0);
        assert!(small < DEFAULT_BUDGET, "{small} bytes must fit");
        assert!(budget.try_reserve(small).is_ok());
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
