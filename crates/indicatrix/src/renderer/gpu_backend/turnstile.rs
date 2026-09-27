//! The FIFO ticket lock [`backend::GpuBackend`](super::backend::GpuBackend) drives for
//! chunk-level fairness among concurrent GPU requests -- see the parent module's doc
//! comment's "Concurrency: chunk-level fairness" section for the correctness argument
//! this exists to support.

/// How many chunks one [`super::backend::GpuBackend::try_accumulate_cancellable`] "turn"
/// dispatches before releasing the renderer and rejoining the back of the [`Turnstile`]
/// queue.
///
/// `1` would lose the chunk pipeline's overlap entirely: `accumulate_turn`'s dispatch
/// loop only overlaps chunk i's GPU work with chunk i-1's readback (see
/// `renderer::gpu::frame`'s "Overlapped chunk pipeline" doc section), and a turn always
/// drains whatever it dispatches before returning (see the parent module's doc comment) --
/// with a one-chunk turn, that "previous" chunk is always ITS OWN, so the CPU would
/// submit and then immediately block on the very dispatch it just queued, exactly the
/// un-pipelined behaviour the double-buffering exists to avoid. `2` keeps that one
/// window of overlap (dispatch chunk 0, dispatch chunk 1 while draining chunk 0, drain
/// chunk 1) inside every turn, while still yielding often enough that a large request
/// can't monopolise the GPU for long -- at `super::gpu::frame::TARGET_CHUNK_MS`'s
/// default, at most ~300ms of wall-clock GPU time per turn once the chunk-timing EMA has
/// converged, far less on the first (uncalibrated) turn of a session. See "Scene
/// re-upload: the fairness cost" above for what a larger value would trade against:
/// fewer re-uploads, coarser fairness.
pub(super) const CHUNKS_PER_TURN: usize = 2;

/// A FIFO ticket lock.
///
/// [`Self::take_ticket`] hands out ticket numbers in call order; [`Self::wait_for_turn`]
/// blocks the calling thread until its ticket is the one being served, returning an RAII
/// [`TurnstileTurn`] that advances to the next ticket (waking every other waiter) on
/// drop. Unlike a plain `Mutex` -- which makes no ordering promise among blocked waiters,
/// so the OS scheduler can and does let one thread relock it repeatedly ahead of others
/// already queued -- this guarantees admissions happen in the exact order threads asked
/// for one. That is what lets [`super::backend::GpuBackend::try_accumulate_cancellable`]'s
/// "take a turn, dispatch a few chunks, rejoin the back of the queue" loop alternate
/// FAIRLY among several concurrent requests rather than one starving the rest. Std-only:
/// an `AtomicU64` ticket counter plus a `Mutex`+`Condvar` "now serving" pair, the
/// standard ticket-lock construction -- no new dependency.
pub(super) struct Turnstile {
    next_ticket: std::sync::atomic::AtomicU64,
    now_serving: std::sync::Mutex<u64>,
    turn_taken: std::sync::Condvar,
}

impl Turnstile {
    pub(super) const fn new() -> Self {
        Self {
            next_ticket: std::sync::atomic::AtomicU64::new(0),
            now_serving: std::sync::Mutex::new(0),
            turn_taken: std::sync::Condvar::new(),
        }
    }

    /// Claims the next ticket, in call order. Cheap and non-blocking: ordering among
    /// callers is decided HERE, at call time, not later when [`Self::wait_for_turn`]
    /// actually blocks -- two threads calling this back-to-back are served in that same
    /// order regardless of how long either later waits.
    pub(super) fn take_ticket(&self) -> u64 {
        self.next_ticket
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Blocks until `ticket` is being served, then returns a guard whose `Drop` advances
    /// `now_serving` and wakes every waiter -- so a turn is released exactly once, even
    /// if the caller returns early (a `?` on a GPU error, a cancellation), never leaked
    /// and never released twice.
    pub(super) fn wait_for_turn(&self, ticket: u64) -> TurnstileTurn<'_> {
        let mut serving = self
            .now_serving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *serving != ticket {
            serving = self
                .turn_taken
                .wait(serving)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        drop(serving);
        TurnstileTurn { turnstile: self }
    }
}

/// RAII: holding this value IS holding the [`Turnstile`]'s current turn. `notify_all`
/// (not `notify_one`) on drop -- of the threads waiting on tickets other than the very
/// next one, none can proceed yet, so waking all of them just to have them re-check and
/// go back to sleep is the standard, simple ticket-lock shape; with turns held only for a
/// bounded [`CHUNKS_PER_TURN`]-chunk slice, that wasted wakeup is not worth optimising
/// away with a per-ticket condvar.
pub(super) struct TurnstileTurn<'a> {
    turnstile: &'a Turnstile,
}

impl Drop for TurnstileTurn<'_> {
    fn drop(&mut self) {
        let mut serving = self
            .turnstile
            .now_serving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *serving += 1;
        drop(serving);
        self.turnstile.turn_taken.notify_all();
    }
}
