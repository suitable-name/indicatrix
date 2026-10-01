//! A single background thread fed through a one-slot "latest request wins" mailbox.

use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, PoisonError, mpsc},
};
use tracing::error;

/// A single background thread fed through a one-slot "latest request wins"
/// mailbox. Dropping it drops the wake-up sender, which ends the thread once it
/// finishes whatever it is processing.
///
/// A panic inside the handler is caught and logged; the thread keeps serving later
/// requests, so one bad job never silences the worker.
pub struct LatestWorker<R: Send + 'static> {
    slot: Arc<Mutex<Option<R>>>,
    wake: mpsc::Sender<()>,
}

/// The readable text of a panic payload.
fn panic_text(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("a payload that is not text")
}

impl<R: Send + 'static> LatestWorker<R> {
    /// Spawns the thread, running `handle` for each request it picks up. A panic in
    /// `handle` is logged and the thread carries on with the next request; `on_panic`
    /// runs on the worker thread after such a panic. The owner posts whatever it needs
    /// to the UI thread from there (for example an error for the request that was
    /// lost). A panic inside `on_panic` is caught and logged as well.
    pub fn spawn_with_panic_hook(
        name: &str,
        mut handle: impl FnMut(R) + Send + 'static,
        on_panic: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let slot: Arc<Mutex<Option<R>>> = Arc::new(Mutex::new(None));
        let (wake, rx) = mpsc::channel::<()>();
        let thread_slot = Arc::clone(&slot);
        let thread_name = name.to_string();
        let spawned = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                for () in rx {
                    let request = thread_slot
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take();
                    let Some(request) = request else { continue };
                    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| handle(request))) {
                        error!(
                            "The {thread_name} thread panicked on a request ({}); it keeps running.",
                            panic_text(payload.as_ref())
                        );
                        if let Err(hook_payload) = catch_unwind(AssertUnwindSafe(&on_panic)) {
                            error!(
                                "The {thread_name} panic hook panicked too ({}).",
                                panic_text(hook_payload.as_ref())
                            );
                        }
                    }
                }
            });
        if let Err(spawn_error) = spawned {
            error!("Could not start the {name} thread: {spawn_error}");
        }
        Self { slot, wake }
    }

    /// Replaces any not-yet-started request with `request` and wakes the thread.
    /// Returns `false` when the thread is gone (it never started), so nothing will
    /// ever handle the request.
    pub fn submit(&self, request: R) -> bool {
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(request);
        self.wake.send(()).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Long enough for a loaded machine, short enough that a hang fails the test.
    const WAIT: Duration = Duration::from_secs(10);

    /// A worker whose panic hook does nothing.
    fn plain(name: &str, handle: impl FnMut(u32) + Send + 'static) -> LatestWorker<u32> {
        LatestWorker::spawn_with_panic_hook(name, handle, || {})
    }

    #[test]
    fn a_panicking_request_does_not_stop_the_next_one() {
        let (seen_tx, seen_rx) = mpsc::channel::<u32>();
        let (hook_tx, hook_rx) = mpsc::channel::<()>();
        let worker = LatestWorker::spawn_with_panic_hook(
            "test-panic",
            move |n: u32| {
                assert_ne!(n, 0, "the first request panics on purpose");
                let _ = seen_tx.send(n);
            },
            move || {
                let _ = hook_tx.send(());
            },
        );
        assert!(worker.submit(0));
        hook_rx
            .recv_timeout(WAIT)
            .expect("the panic hook runs after the handler panicked");
        assert!(worker.submit(1));
        assert_eq!(seen_rx.recv_timeout(WAIT), Ok(1));
    }

    #[test]
    fn the_plain_spawn_also_survives_a_panic() {
        let (seen_tx, seen_rx) = mpsc::channel::<u32>();
        let (first_tx, first_rx) = mpsc::channel::<()>();
        let worker = plain("test-plain-panic", move |n: u32| {
            if n == 0 {
                let _ = first_tx.send(());
                panic!("the first request panics on purpose");
            }
            let _ = seen_tx.send(n);
        });
        assert!(worker.submit(0));
        first_rx.recv_timeout(WAIT).expect("the first request ran");
        assert!(worker.submit(1));
        assert_eq!(seen_rx.recv_timeout(WAIT), Ok(1));
    }

    #[test]
    fn only_the_latest_waiting_request_is_handled() {
        let (seen_tx, seen_rx) = mpsc::channel::<u32>();
        let (started_tx, started_rx) = mpsc::channel::<()>();
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let worker = plain("test-latest", move |n: u32| {
            let _ = seen_tx.send(n);
            if n == 0 {
                let _ = started_tx.send(());
                let _ = gate_rx.recv();
            }
        });
        assert!(worker.submit(0));
        started_rx
            .recv_timeout(WAIT)
            .expect("the first request started");
        // The handler is blocked inside request 0; these 100 replace one another in the
        // one-slot mailbox, so only the last of them (100) is ever picked up.
        for n in 1..=100 {
            assert!(worker.submit(n));
        }
        gate_tx.send(()).expect("the handler is waiting");
        assert_eq!(seen_rx.recv_timeout(WAIT), Ok(0));
        assert_eq!(seen_rx.recv_timeout(WAIT), Ok(100));
        assert_eq!(
            seen_rx.recv_timeout(Duration::from_millis(200)),
            Err(mpsc::RecvTimeoutError::Timeout),
            "nothing else was handled"
        );
    }

    #[test]
    fn dropping_the_worker_ends_its_thread() {
        let (alive_tx, alive_rx) = mpsc::channel::<()>();
        let worker = plain("test-drop", move |_: u32| {
            // Holding the sender in the handler ties its lifetime to the thread's.
            let _ = &alive_tx;
        });
        assert_eq!(
            alive_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout),
            "the thread is alive while the worker is"
        );
        drop(worker);
        assert_eq!(
            alive_rx.recv_timeout(WAIT),
            Err(mpsc::RecvTimeoutError::Disconnected),
            "the handler, and with it the sender, is gone once the thread ended"
        );
    }

    #[test]
    fn submitting_to_a_worker_without_a_thread_reports_it() {
        let (wake, rx) = mpsc::channel::<()>();
        drop(rx);
        let worker = LatestWorker::<u32> {
            slot: Arc::new(Mutex::new(None)),
            wake,
        };
        assert!(!worker.submit(1));
    }
}
