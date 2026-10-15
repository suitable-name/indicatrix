//! Small concurrency helpers: a generation counter that lets newer work supersede older
//! work, and a single-thread FIFO job queue.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// A shared counter that names the newest request.
///
/// A background job copies the value
/// [`Generation::advance`] returned when it was started and checks
/// [`Generation::is_current`] before publishing, so a result that a newer request has
/// since superseded is dropped. Clones share the same counter.
#[derive(Debug, Clone, Default)]
pub struct Generation {
    counter: Arc<AtomicU64>,
}

impl Generation {
    /// Starts a new request and returns its generation (the first one is 1).
    #[must_use]
    pub fn advance(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// The newest generation handed out (0 before any [`Self::advance`]).
    #[must_use]
    pub fn current(&self) -> u64 {
        self.counter.load(Ordering::Acquire)
    }

    /// Whether `generation` is still the newest one.
    #[must_use]
    pub fn is_current(&self, generation: u64) -> bool {
        self.current() == generation
    }
}

/// How long [`join_briefly`] waits for a thread to finish.
const JOIN_WAIT: Duration = Duration::from_secs(2);

/// Joins `handle` if its thread finishes within two seconds, else leaves it running
/// (logged).
///
/// Never panics: at process exit Windows ends every other thread before
/// thread-local destructors run, and `JoinHandle::join` on such a thread aborts the
/// process; a thread that has really finished is always reported as finished.
pub fn join_briefly(handle: JoinHandle<()>, name: &str) {
    let deadline = Instant::now() + JOIN_WAIT;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            tracing::warn!(thread = name, "thread did not finish in time; left running");
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if handle.join().is_err() {
        tracing::warn!(thread = name, "thread ended with a panic");
    }
}

type Job = Box<dyn FnOnce() + Send + 'static>;

/// One named background thread that runs submitted jobs one after the other, in the
/// order they were submitted.
///
/// A job that panics is logged and skipped; the queue keeps
/// running. Dropping the queue lets the jobs already submitted finish, then ends and
/// joins the thread (waiting at most two seconds, see [`join_briefly`]).
pub struct BackgroundQueue {
    name: String,
    sender: Option<Sender<Job>>,
    handle: Option<JoinHandle<()>>,
}

impl BackgroundQueue {
    /// Starts the queue's thread, named `name`. If the system refuses to start a thread
    /// the failure is logged and every later job is discarded.
    #[must_use]
    pub fn new(name: &str) -> Self {
        let (sender, receiver) = mpsc::channel::<Job>();
        let thread_name = name.to_owned();
        let spawned = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                for job in receiver {
                    if catch_unwind(AssertUnwindSafe(job)).is_err() {
                        tracing::error!(queue = %thread_name, "a background job panicked");
                    }
                }
            });
        let handle = match spawned {
            Ok(handle) => Some(handle),
            Err(error) => {
                tracing::error!(queue = name, %error, "could not start the background thread");
                None
            }
        };
        Self {
            name: name.to_owned(),
            sender: Some(sender),
            handle,
        }
    }

    /// Queues `job` behind the jobs already submitted.
    pub fn submit(&self, job: impl FnOnce() + Send + 'static) {
        let Some(sender) = &self.sender else {
            return;
        };
        if sender.send(Box::new(job)).is_err() {
            tracing::warn!(queue = %self.name, "background job discarded: the queue thread is gone");
        }
    }
}

impl Drop for BackgroundQueue {
    fn drop(&mut self) {
        // Closing the channel ends the thread's loop once the queued jobs are done.
        drop(self.sender.take());
        if let Some(handle) = self.handle.take() {
            // The queue can be dropped by one of its own jobs; joining ourselves would
            // deadlock, so let the thread finish on its own then.
            if handle.thread().id() != std::thread::current().id() {
                join_briefly(handle, &self.name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn generation_supersedes() {
        let generation = Generation::default();
        assert_eq!(generation.current(), 0);
        let first = generation.advance();
        assert_eq!(first, 1);
        assert!(generation.is_current(first));
        let second = generation.advance();
        assert_eq!(second, 2);
        assert!(!generation.is_current(first));
        assert!(generation.is_current(second));
    }

    #[test]
    fn generation_clones_share_one_counter_across_threads() {
        let generation = Generation::default();
        let first = generation.advance();
        let other = generation.clone();
        let second = std::thread::spawn(move || other.advance())
            .join()
            .expect("the advancing thread must not panic");
        assert!(second > first);
        assert!(!generation.is_current(first));
        assert!(generation.is_current(second));
    }

    #[test]
    fn queue_runs_jobs_in_order() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let queue = BackgroundQueue::new("test-fifo");
        for i in 0..50 {
            let seen = Arc::clone(&seen);
            queue.submit(move || seen.lock().expect("lock").push(i));
        }
        drop(queue); // joins after the queued jobs finished
        let seen = seen.lock().expect("lock");
        assert_eq!(*seen, (0..50).collect::<Vec<_>>());
    }

    #[test]
    fn queue_survives_a_panicking_job() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let queue = BackgroundQueue::new("test-panic");
        {
            let seen = Arc::clone(&seen);
            queue.submit(move || seen.lock().expect("lock").push(1));
        }
        queue.submit(|| panic!("deliberate test panic"));
        {
            let seen = Arc::clone(&seen);
            queue.submit(move || seen.lock().expect("lock").push(2));
        }
        drop(queue);
        assert_eq!(*seen.lock().expect("lock"), vec![1, 2]);
    }

    #[test]
    fn queue_thread_has_the_given_name() {
        let name = Arc::new(Mutex::new(None));
        let queue = BackgroundQueue::new("test-named");
        {
            let name = Arc::clone(&name);
            queue.submit(move || {
                *name.lock().expect("lock") = std::thread::current().name().map(str::to_owned);
            });
        }
        drop(queue);
        assert_eq!(name.lock().expect("lock").as_deref(), Some("test-named"));
    }
}
