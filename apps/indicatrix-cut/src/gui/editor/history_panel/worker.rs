//! The thread that draws the History tab's pictures.
//!
//! One thread, one request at a time, so it never competes with the solver or the renderer
//! for more than one core. The UI thread never waits for it:
//!
//! - [`Worker::request`] only pushes a key on a short queue and wakes the thread;
//! - the thread takes the NEWEST request first (the rows on screen now, not the ones a fast
//!   scroll already left behind), works out the design at that step from the current
//!   [`RenderSnapshot`], solves and draws it;
//! - [`Worker::take_finished`] hands the finished pictures back, whenever the UI thread asks.
//!
//! The queue is bounded: past [`MAX_QUEUED`] the oldest request is dropped and reported
//! back as [`Outcome::Stale`], so its row simply asks again the next time it is shown.
//! A request whose step no longer exists in the current snapshot (the history moved on) is
//! answered the same way instead of drawing the wrong design.

use super::{
    render::{Pixels, ThumbRenderer, render_step},
    thumbs::{START_REVISION, ThumbKey},
};
use indicatrix_editor::HistorySnapshot;
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    thread,
};
use tracing::{error, warn};

/// How many requests may wait for the thread. A little more than the rows that fit on a
/// screen, so a fast scroll overflows it only with rows long since scrolled away.
pub(super) const MAX_QUEUED: usize = 48;

/// The design and history a picture is worked out from, and what it must still match.
pub(super) struct RenderSnapshot {
    /// The design epoch the snapshot belongs to.
    epoch: u64,
    /// The picture edge in physical pixels.
    edge_px: u32,
    /// `revisions[p]` is the revision of step `p` (position 0 has [`START_REVISION`]).
    revisions: Vec<u64>,
    session: HistorySnapshot,
}

impl RenderSnapshot {
    /// A snapshot of `session` for design epoch `epoch`, drawing at `edge_px`.
    #[must_use]
    pub(super) fn new(epoch: u64, edge_px: u32, session: HistorySnapshot) -> Self {
        let revisions = std::iter::once(START_REVISION)
            .chain(session.entries().iter().map(|entry| entry.revision))
            .collect();
        Self {
            epoch,
            edge_px,
            revisions,
            session,
        }
    }

    /// Whether `key` still names a picture this snapshot can draw: same design, same size,
    /// and the step at that position is still the one the key was made for.
    #[must_use]
    pub(super) fn accepts(&self, key: &ThumbKey) -> bool {
        key.epoch == self.epoch
            && key.edge_px == self.edge_px
            && self.revisions.get(key.position) == Some(&key.revision)
    }
}

/// How one request ended.
pub(super) enum Outcome {
    /// The picture.
    Ready(Pixels),
    /// The design at that step cannot be drawn (it does not solve).
    CannotDraw,
    /// The request no longer matches the history; ask again if the row is still shown.
    Stale,
}

/// A finished request.
pub(super) struct Finished {
    pub(super) key: ThumbKey,
    pub(super) outcome: Outcome,
}

/// Requests waiting for the thread, newest last.
#[derive(Debug, Default)]
pub(super) struct WorkQueue {
    pending: Vec<ThumbKey>,
}

impl WorkQueue {
    /// Adds a request, unless the same one already waits. Beyond [`MAX_QUEUED`] the oldest
    /// request is dropped and returned.
    pub(super) fn push(&mut self, key: ThumbKey) -> Option<ThumbKey> {
        if self.pending.contains(&key) {
            return None;
        }
        self.pending.push(key);
        (self.pending.len() > MAX_QUEUED).then(|| self.pending.remove(0))
    }

    /// Takes the newest request.
    pub(super) fn pop_newest(&mut self) -> Option<ThumbKey> {
        self.pending.pop()
    }

    /// How many requests wait.
    #[cfg(test)]
    pub(super) const fn len(&self) -> usize {
        self.pending.len()
    }
}

/// Everything the UI thread and the worker thread share, behind one mutex. The worker holds
/// the mutex only for bookkeeping, never while it solves or draws.
#[derive(Default)]
struct State {
    queue: WorkQueue,
    snapshot: Option<Arc<RenderSnapshot>>,
    finished: Vec<Finished>,
    closed: bool,
}

struct Shared {
    state: Mutex<State>,
    /// Signalled when a request is queued or the worker is closed.
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Blocks until there is a request; `None` once the worker is closed.
    fn next_job(&self) -> Option<(ThumbKey, Option<Arc<RenderSnapshot>>)> {
        let mut state = self
            .wake
            .wait_while(self.lock(), |state| {
                state.queue.pending.is_empty() && !state.closed
            })
            .unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return None;
        }
        let key = state.queue.pop_newest()?;
        Some((key, state.snapshot.clone()))
    }

    fn finish(&self, finished: Finished) {
        self.lock().finished.push(finished);
    }
}

/// The handle the UI thread holds. Dropping it ends the thread once it finishes the request
/// it is on.
pub(super) struct Worker {
    shared: Arc<Shared>,
}

impl Worker {
    /// Starts the thread. If it cannot be started the pictures simply never arrive (the
    /// rows keep their "Drawing" placeholder) and a warning is logged.
    #[must_use]
    pub(super) fn spawn() -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let thread_shared = Arc::clone(&shared);
        if let Err(spawn_error) = thread::Builder::new()
            .name("history-thumbnails".into())
            .spawn(move || run(&thread_shared))
        {
            warn!("could not start the history thumbnail thread: {spawn_error}");
        }
        Self { shared }
    }

    /// Replaces the snapshot requests are worked out from.
    pub(super) fn set_snapshot(&self, snapshot: Arc<RenderSnapshot>) {
        self.shared.lock().snapshot = Some(snapshot);
    }

    /// Queues a picture. Never blocks on the thread's work.
    pub(super) fn request(&self, key: ThumbKey) {
        {
            let mut state = self.shared.lock();
            if let Some(dropped) = state.queue.push(key) {
                state.finished.push(Finished {
                    key: dropped,
                    outcome: Outcome::Stale,
                });
            }
        }
        self.shared.wake.notify_one();
    }

    /// Takes every request the thread has finished since the last call.
    pub(super) fn take_finished(&self) -> Vec<Finished> {
        std::mem::take(&mut self.shared.lock().finished)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.wake.notify_all();
    }
}

/// The thread's loop.
fn run(shared: &Shared) {
    let mut renderer = ThumbRenderer::default();
    while let Some((key, snapshot)) = shared.next_job() {
        let outcome = answer(&mut renderer, snapshot.as_deref(), &key);
        shared.finish(Finished { key, outcome });
    }
}

/// Draws the picture `key` names from `snapshot`, or says why not. A panic in the solver or
/// the renderer is logged and answered as "cannot draw" so one bad design cannot silence the
/// thread.
fn answer(
    renderer: &mut ThumbRenderer,
    snapshot: Option<&RenderSnapshot>,
    key: &ThumbKey,
) -> Outcome {
    let Some(snapshot) = snapshot.filter(|snapshot| snapshot.accepts(key)) else {
        return Outcome::Stale;
    };
    match catch_unwind(AssertUnwindSafe(|| {
        render_step(renderer, &snapshot.session, key.position, key.edge_px)
    })) {
        Ok(Some(pixels)) => Outcome::Ready(pixels),
        Ok(None) => Outcome::CannotDraw,
        Err(_) => {
            error!(
                "Drawing the history picture for step {} panicked; the row shows \"Does not solve\".",
                key.position
            );
            Outcome::CannotDraw
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::Edit;
    use indicatrix_editor::EditorSession;
    use std::time::{Duration, Instant};

    fn key(position: usize, revision: u64) -> ThumbKey {
        ThumbKey {
            epoch: 1,
            position,
            revision,
            label: format!("step {position}"),
            edge_px: 32,
        }
    }

    /// A session with two steps and a snapshot for it.
    fn two_step_snapshot() -> (EditorSession, Arc<RenderSnapshot>) {
        let mut session = EditorSession::fresh();
        for offset in [0.1, 0.2] {
            session
                .apply(Edit::SetPreformYOffset { y_offset: offset })
                .expect("edit must apply");
        }
        let snapshot = Arc::new(RenderSnapshot::new(1, 32, session.history_snapshot()));
        (session, snapshot)
    }

    fn wait_for(worker: &Worker, count: usize) -> Vec<Finished> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut all = Vec::new();
        while all.len() < count && Instant::now() < deadline {
            all.extend(worker.take_finished());
            thread::sleep(Duration::from_millis(5));
        }
        all
    }

    #[test]
    fn the_queue_hands_out_the_newest_request_first_and_ignores_duplicates() {
        let mut queue = WorkQueue::default();
        for position in 1..=3 {
            assert!(queue.push(key(position, 0)).is_none());
        }
        assert!(queue.push(key(2, 0)).is_none());
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.pop_newest(), Some(key(3, 0)));
        assert_eq!(queue.pop_newest(), Some(key(2, 0)));
        assert_eq!(queue.pop_newest(), Some(key(1, 0)));
        assert_eq!(queue.pop_newest(), None);
    }

    #[test]
    fn an_overflowing_queue_drops_its_oldest_request() {
        let mut queue = WorkQueue::default();
        for position in 0..MAX_QUEUED {
            assert!(queue.push(key(position, 0)).is_none());
        }
        assert_eq!(queue.push(key(MAX_QUEUED, 0)), Some(key(0, 0)));
        assert_eq!(queue.len(), MAX_QUEUED);
    }

    #[test]
    fn a_snapshot_accepts_only_the_keys_that_still_match_its_history() {
        let (session, snapshot) = two_step_snapshot();
        let entries = session.history_entries();
        let good = |position: usize| ThumbKey {
            epoch: 1,
            position,
            revision: if position == 0 {
                START_REVISION
            } else {
                entries[position - 1].revision
            },
            label: String::new(),
            edge_px: 32,
        };
        assert!(snapshot.accepts(&good(0)));
        assert!(snapshot.accepts(&good(2)));
        let mut other_design = good(1);
        other_design.epoch = 2;
        assert!(!snapshot.accepts(&other_design));
        let mut other_size = good(1);
        other_size.edge_px = 64;
        assert!(!snapshot.accepts(&other_size));
        let mut moved_on = good(1);
        moved_on.revision += 1;
        assert!(!snapshot.accepts(&moved_on));
        let mut past_the_end = good(1);
        past_the_end.position = 3;
        assert!(!snapshot.accepts(&past_the_end));
    }

    #[test]
    fn the_thread_draws_requested_steps_and_stale_requests_are_answered_not_drawn() {
        let (session, snapshot) = two_step_snapshot();
        let entries = session.history_entries();
        let worker = Worker::spawn();
        worker.set_snapshot(snapshot);
        worker.request(ThumbKey {
            epoch: 1,
            position: 2,
            revision: entries[1].revision,
            label: entries[1].label.clone(),
            edge_px: 32,
        });
        let mut stale = key(1, entries[0].revision + 100);
        stale.edge_px = 32;
        worker.request(stale.clone());
        let finished = wait_for(&worker, 2);
        assert_eq!(finished.len(), 2, "both requests were answered");
        for done in finished {
            if done.key == stale {
                assert!(matches!(done.outcome, Outcome::Stale));
            } else {
                assert!(matches!(done.outcome, Outcome::Ready(ref pixels) if pixels.width() == 32));
            }
        }
    }

    #[test]
    fn a_request_without_any_snapshot_is_answered_stale() {
        let worker = Worker::spawn();
        worker.request(key(1, 0));
        let finished = wait_for(&worker, 1);
        assert_eq!(finished.len(), 1);
        assert!(matches!(finished[0].outcome, Outcome::Stale));
    }
}
