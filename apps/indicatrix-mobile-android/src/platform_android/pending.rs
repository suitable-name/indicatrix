//! The request table: it connects an asynchronous answer from the Kotlin `MainActivity`
//! to the Rust callback waiting for it, and it stages the files other apps send us.
//!
//! Every asynchronous request (pick files, save a file) gets a `u64` id, and its
//! callback waits here until Kotlin answers through one of the `native*` exports in
//! [`natives`](super::natives). Id [`INCOMING_ID`] is reserved: Kotlin uses it for files
//! that arrive without a request ("Open with", "Share to"). Those files are held until
//! a handler is registered with [`set_incoming_handler`] and are delivered at once
//! after that.
//!
//! Callbacks always run after the table lock is released, on whatever thread Kotlin
//! called from.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use indicatrix_mobile_core::platform::{
    DocumentsCallback, IncomingCallback, PickedFile, PlatformError, SaveCallback, SaveOutcome,
};

/// The id Kotlin uses for files nobody asked for (incoming intents).
pub(super) const INCOMING_ID: u64 = 0;

/// One request waiting for Kotlin.
enum Pending {
    /// A file pick; the files arrive one by one before the request is finished.
    Documents {
        done: DocumentsCallback,
        files: Vec<PickedFile>,
    },
    /// A save dialog.
    Save { done: SaveCallback },
}

/// The registered receiver of incoming files, shareable across threads.
type Handler = Arc<dyn Fn(Vec<PickedFile>) + Send + Sync>;

/// Files from other apps: the batch being assembled, the finished batches nobody could
/// take yet, and the receiver.
struct Incoming {
    handler: Option<Handler>,
    staged: Vec<PickedFile>,
    backlog: Vec<PickedFile>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static TABLE: Mutex<BTreeMap<u64, Pending>> = Mutex::new(BTreeMap::new());
static INCOMING: Mutex<Incoming> = Mutex::new(Incoming {
    handler: None,
    staged: Vec::new(),
    backlog: Vec::new(),
});

/// Locks `mutex`, ignoring poisoning (the data stays consistent: every critical section
/// below is a plain insert, remove or take).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Registers a file-pick request and returns its id.
pub(super) fn register_documents(done: DocumentsCallback) -> u64 {
    let id = next_id();
    lock(&TABLE).insert(
        id,
        Pending::Documents {
            done,
            files: Vec::new(),
        },
    );
    id
}

/// Registers a save request and returns its id.
pub(super) fn register_save(done: SaveCallback) -> u64 {
    let id = next_id();
    lock(&TABLE).insert(id, Pending::Save { done });
    id
}

/// Adds one file to request `id`, or to the incoming batch when `id` is
/// [`INCOMING_ID`]. Files for an unknown or finished request are dropped.
pub(super) fn push_file(id: u64, file: PickedFile) {
    if id == INCOMING_ID {
        lock(&INCOMING).staged.push(file);
        return;
    }
    match lock(&TABLE).get_mut(&id) {
        Some(Pending::Documents { files, .. }) => files.push(file),
        _ => tracing::warn!(
            id,
            "a picked file arrived for a request that is not waiting"
        ),
    }
}

/// Ends a file-pick request (or the incoming batch for [`INCOMING_ID`]). With an
/// `error` the files collected so far are discarded.
pub(super) fn finish_documents(id: u64, error: Option<String>) {
    if id == INCOMING_ID {
        finish_incoming(error);
        return;
    }
    let pending = lock(&TABLE).remove(&id);
    match pending {
        Some(Pending::Documents { done, files }) => match error {
            None => done(Ok(files)),
            Some(message) => done(Err(PlatformError::new(message))),
        },
        Some(Pending::Save { .. }) | None => {
            tracing::warn!(
                id,
                "a pick result arrived for a request that is not waiting"
            );
        }
    }
}

/// Ends a save request: `error` set means failure, `display_name` set means saved,
/// neither means the user cancelled.
pub(super) fn finish_save(id: u64, display_name: Option<String>, error: Option<String>) {
    let pending = lock(&TABLE).remove(&id);
    match pending {
        Some(Pending::Save { done }) => {
            let outcome = match (error, display_name) {
                (Some(message), _) => Err(PlatformError::new(message)),
                (None, Some(display_name)) => Ok(SaveOutcome::Saved { display_name }),
                (None, None) => Ok(SaveOutcome::Cancelled),
            };
            done(outcome);
        }
        Some(Pending::Documents { .. }) | None => {
            tracing::warn!(
                id,
                "a save result arrived for a request that is not waiting"
            );
        }
    }
}

/// Fails request `id` from the Rust side (the Kotlin call itself failed). Does nothing
/// when the request is already gone.
pub(super) fn fail(id: u64, error: PlatformError) {
    let pending = lock(&TABLE).remove(&id);
    match pending {
        Some(Pending::Documents { done, .. }) => done(Err(error)),
        Some(Pending::Save { done }) => done(Err(error)),
        None => {}
    }
}

/// Registers the receiver of incoming files and delivers whatever arrived before it.
pub(super) fn set_incoming_handler(handler: IncomingCallback) {
    let handler: Handler = Arc::from(handler);
    let backlog = {
        let mut incoming = lock(&INCOMING);
        incoming.handler = Some(Arc::clone(&handler));
        std::mem::take(&mut incoming.backlog)
    };
    if !backlog.is_empty() {
        handler(backlog);
    }
}

/// Closes the incoming batch: delivers it, or keeps it until a handler exists.
fn finish_incoming(error: Option<String>) {
    let (handler, batch) = {
        let mut incoming = lock(&INCOMING);
        let batch = std::mem::take(&mut incoming.staged);
        if let Some(message) = error {
            tracing::warn!(%message, "an incoming file could not be read");
            return;
        }
        if batch.is_empty() {
            return;
        }
        match incoming.handler.clone() {
            Some(handler) => (handler, batch),
            None => {
                incoming.backlog.extend(batch);
                return;
            }
        }
    };
    handler(batch);
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn file(name: &str) -> PickedFile {
        PickedFile {
            display_name: name.to_owned(),
            bytes: vec![1, 2, 3],
        }
    }

    #[test]
    fn documents_request_collects_files_then_completes() {
        let (tx, rx) = mpsc::channel();
        let id = register_documents(Box::new(move |result| {
            let _ = tx.send(result);
        }));
        push_file(id, file("a.asc"));
        push_file(id, file("b.asc"));
        finish_documents(id, None);
        let files = rx.recv().expect("callback ran").expect("no error");
        assert_eq!(files.len(), 2);
        assert_eq!(files[1].display_name, "b.asc");
    }

    #[test]
    fn documents_request_with_error_discards_files() {
        let (tx, rx) = mpsc::channel();
        let id = register_documents(Box::new(move |result| {
            let _ = tx.send(result);
        }));
        push_file(id, file("a.asc"));
        finish_documents(id, Some("too large".to_owned()));
        let error = rx.recv().expect("callback ran").expect_err("an error");
        assert_eq!(error.message, "too large");
    }

    #[test]
    fn empty_documents_request_means_cancelled() {
        let (tx, rx) = mpsc::channel();
        let id = register_documents(Box::new(move |result| {
            let _ = tx.send(result);
        }));
        finish_documents(id, None);
        assert!(
            rx.recv()
                .expect("callback ran")
                .expect("no error")
                .is_empty()
        );
    }

    #[test]
    fn save_outcomes_map_to_the_contract() {
        for (name, error, expected) in [
            (
                Some("x.isdd"),
                None,
                Some(Ok(SaveOutcome::Saved {
                    display_name: "x.isdd".to_owned(),
                })),
            ),
            (None, None, Some(Ok(SaveOutcome::Cancelled))),
            (
                None,
                Some("disk full"),
                Some(Err(PlatformError::new("disk full"))),
            ),
        ] {
            let (tx, rx) = mpsc::channel();
            let id = register_save(Box::new(move |result| {
                let _ = tx.send(result);
            }));
            finish_save(id, name.map(str::to_owned), error.map(str::to_owned));
            assert_eq!(Some(rx.recv().expect("callback ran")), expected);
        }
    }

    #[test]
    fn failing_a_request_calls_back_once() {
        let (tx, rx) = mpsc::channel();
        let id = register_save(Box::new(move |result| {
            let _ = tx.send(result);
        }));
        fail(id, PlatformError::new("no activity"));
        fail(id, PlatformError::new("again"));
        assert_eq!(
            rx.recv().expect("callback ran"),
            Err(PlatformError::new("no activity"))
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn incoming_files_wait_for_the_handler() {
        // One test owns the shared INCOMING state from start to end.
        push_file(INCOMING_ID, file("early.asc"));
        finish_documents(INCOMING_ID, None);

        let (tx, rx) = mpsc::channel();
        set_incoming_handler(Box::new(move |files| {
            let _ = tx.send(files);
        }));
        let delivered = rx.recv().expect("backlog delivered");
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].display_name, "early.asc");

        push_file(INCOMING_ID, file("late.gem"));
        finish_documents(INCOMING_ID, None);
        let delivered = rx.recv().expect("live batch delivered");
        assert_eq!(delivered[0].display_name, "late.gem");
    }
}
