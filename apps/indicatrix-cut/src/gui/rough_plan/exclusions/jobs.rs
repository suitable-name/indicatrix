//! The exclusion thread: reads and writes of the designs excluded from the planner, one job
//! at a time, in the order they were asked for, off the UI thread.
//!
//! The library database lock may be held for seconds by an import batch, a search or the
//! planner's own scan, and waiting for it on the UI thread would freeze the window, at the
//! moment it opens or the moment a pill is clicked. A job queue (and not one thread per
//! job) keeps a read asked for after a write from overtaking it, so the answers arrive in
//! the order of the jobs and the last one is what the library holds.

use crate::{
    RoughPlannerWindow,
    gui::rough_plan::{host::on_host, saved::panic_message},
};
use indicatrix_vault::db::sqlite::Database;
use slint::Weak;
use std::{
    cell::Cell,
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, PoisonError, mpsc},
};
use tracing::warn;

/// What a write changed, for the words said once it is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Change {
    /// One design was excluded or restored.
    One {
        /// The design's library id.
        id: i64,
        /// The title it was called by when the click was made.
        before: String,
        /// Whether it was excluded (`true`) or restored.
        excluded: bool,
    },
    /// Every excluded design was restored.
    All,
}

/// One request to the exclusion thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Job {
    /// Read the excluded designs.
    Read,
    /// Mark (`excluded`) or unmark `ids`, then read the excluded designs.
    Write {
        /// The designs to mark.
        ids: Vec<i64>,
        /// Whether to exclude them or to restore them.
        excluded: bool,
        /// What to say when it is done.
        change: Change,
    },
}

impl Job {
    /// Whether the job writes.
    #[must_use]
    pub(super) const fn is_write(&self) -> bool {
        matches!(self, Self::Write { .. })
    }
}

/// The excluded designs with their titles, or why they could not be read.
pub(super) type Listed = Result<BTreeMap<i64, String>, String>;

/// What a job found: the library's state after it.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Answer {
    /// For a write job, what it did (the change) or why it could not (the message for the
    /// error line); `None` for a read.
    pub(super) write: Option<Result<Change, String>>,
    /// The excluded designs after the job, with their titles.
    pub(super) listed: Listed,
}

/// The message for a write that failed.
fn write_failure(detail: &str) -> String {
    format!("Could not change the planner exclusion: {detail}")
}

/// The excluded designs with their titles, as the library database has them now. The
/// database lock is taken and released in here.
fn read_excluded(db: &Mutex<Database>) -> Listed {
    let db = db.lock().unwrap_or_else(PoisonError::into_inner);
    let ids: Vec<i64> = db
        .planner_excluded_ids()
        .map_err(|e| format!("{e:#}"))?
        .into_iter()
        .collect();
    db.entry_titles_for(&ids).map_err(|e| format!("{e:#}"))
}

/// Writes the mark of every design in `ids` under one database lock, released on return.
fn write_marks(db: &Mutex<Database>, ids: &[i64], excluded: bool) -> Result<(), String> {
    let db = db.lock().unwrap_or_else(PoisonError::into_inner);
    ids.iter().try_for_each(|&id| {
        db.set_planner_excluded(id, excluded)
            .map_err(|e| write_failure(&format!("{e:#}")))
    })
}

/// Runs `job` against the library: the write, if it is one, then the read of the list. The
/// list is read after a failed write too, so the window shows what the library holds.
pub(super) fn run_job(db: &Mutex<Database>, job: Job) -> Answer {
    let write = match job {
        Job::Read => None,
        Job::Write {
            ids,
            excluded,
            change,
        } => Some(write_marks(db, &ids, excluded).map(|()| change)),
    };
    Answer {
        write,
        listed: read_excluded(db),
    }
}

/// The answer for a job whose run panicked (a bug): it says so, so the window does not wait
/// for a write that never finishes.
fn panicked(was_write: bool, detail: &str) -> Answer {
    warn!("Rough planner: an exclusion job panicked: {detail}");
    let message = format!("the exclusions thread stopped unexpectedly ({detail})");
    Answer {
        write: was_write.then(|| Err(write_failure(&message))),
        listed: Err(message),
    }
}

/// Starts the thread that runs the jobs sent on the returned channel in order and hands
/// each answer to `deliver` (on that thread). The thread ends when the sender is dropped.
/// A panic in a job is answered like any other failure and the thread carries on.
///
/// `None` for the sender when the thread could not be started.
fn spawn_queue(
    db: Arc<Mutex<Database>>,
    deliver: impl Fn(Answer) + Send + 'static,
) -> Option<mpsc::Sender<Job>> {
    let (jobs, inbox) = mpsc::channel::<Job>();
    let spawned = std::thread::Builder::new()
        .name("rough-exclusions".to_string())
        .spawn(move || {
            for job in inbox {
                let was_write = job.is_write();
                let answer = catch_unwind(AssertUnwindSafe(|| run_job(&db, job)))
                    .unwrap_or_else(|payload| panicked(was_write, &panic_message(&*payload)));
                deliver(answer);
            }
        });
    match spawned {
        Ok(_handle) => Some(jobs),
        Err(error) => {
            warn!("Rough planner: could not start the exclusions thread: {error}");
            None
        }
    }
}

/// The queue the planner window sends its exclusion jobs to.
pub(in crate::gui::rough_plan) struct ExclusionWorker {
    /// The thread's inbox; `None` when the thread could not be started.
    jobs: Option<mpsc::Sender<Job>>,
    /// Writes sent whose answers have not arrived yet.
    pending_writes: Cell<usize>,
}

impl ExclusionWorker {
    /// Starts the thread; its answers are applied on `window`'s UI thread, through
    /// `on_host`, in the order of the jobs.
    #[must_use]
    pub(in crate::gui::rough_plan) fn new(
        window: Weak<RoughPlannerWindow>,
        db: Arc<Mutex<Database>>,
    ) -> Self {
        let jobs = spawn_queue(db, move |answer| {
            let _ = window.upgrade_in_event_loop(move |_window| {
                on_host(|host| super::apply_answer(host, answer));
            });
        });
        Self {
            jobs,
            pending_writes: Cell::new(0),
        }
    }

    /// Queues `job`. Returns `false` when the thread is gone, so nothing will answer.
    pub(super) fn submit(&self, job: Job) -> bool {
        let is_write = job.is_write();
        let sent = self
            .jobs
            .as_ref()
            .is_some_and(|jobs| jobs.send(job).is_ok());
        if sent && is_write {
            self.pending_writes.set(self.pending_writes.get() + 1);
        }
        sent
    }

    /// Whether a write was sent and its answer has not arrived yet.
    #[must_use]
    pub(super) const fn writes_pending(&self) -> bool {
        self.pending_writes.get() > 0
    }

    /// The answer of a write arrived.
    pub(super) fn write_answered(&self) {
        self.pending_writes
            .set(self.pending_writes.get().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_vault::model::entry::FacetingDiagramEntry;
    use std::time::Duration;

    /// Long enough for a loaded machine, short enough that a hang fails the test.
    const WAIT: Duration = Duration::from_secs(10);

    /// An in-memory library with one design per title; their ids, in order.
    fn library(titles: &[&str]) -> (Arc<Mutex<Database>>, Vec<i64>) {
        let db = Database::new(Some(":memory:")).expect("an in-memory library opens");
        let ids = titles
            .iter()
            .enumerate()
            .map(|(index, title)| {
                db.save_diagram_entry(
                    &FacetingDiagramEntry {
                        title: (*title).to_string(),
                        url: format!("local://{title}-{index}.asc"),
                        design_id: String::new(),
                    },
                    "local-import",
                )
                .expect("the design is saved")
            })
            .collect();
        (Arc::new(Mutex::new(db)), ids)
    }

    fn one(id: i64, excluded: bool) -> Change {
        Change::One {
            id,
            before: format!("Design #{id}"),
            excluded,
        }
    }

    fn write(ids: &[i64], excluded: bool, change: Change) -> Job {
        Job::Write {
            ids: ids.to_vec(),
            excluded,
            change,
        }
    }

    #[test]
    fn a_read_lists_the_excluded_designs_with_their_titles() {
        let (db, ids) = library(&["Oval", "Round"]);
        assert_eq!(
            run_job(&db, Job::Read),
            Answer {
                write: None,
                listed: Ok(BTreeMap::new()),
            }
        );
        db.lock()
            .expect("lock")
            .set_planner_excluded(ids[1], true)
            .expect("excluded");
        let answer = run_job(&db, Job::Read);
        assert_eq!(answer.write, None);
        assert_eq!(
            answer.listed,
            Ok(BTreeMap::from([(ids[1], "Round".to_string())]))
        );
    }

    #[test]
    fn a_write_marks_the_designs_and_answers_with_the_list_after_it() {
        let (db, ids) = library(&["Oval", "Round", "Cushion"]);
        let change = one(ids[0], true);
        let answer = run_job(&db, write(&[ids[0]], true, change.clone()));
        assert_eq!(answer.write, Some(Ok(change)));
        assert_eq!(
            answer.listed,
            Ok(BTreeMap::from([(ids[0], "Oval".to_string())]))
        );
        // Restoring all takes every mark off in one job.
        run_job(&db, write(&[ids[1]], true, one(ids[1], true)));
        let answer = run_job(&db, write(&[ids[0], ids[1]], false, Change::All));
        assert_eq!(answer.write, Some(Ok(Change::All)));
        assert_eq!(answer.listed, Ok(BTreeMap::new()));
    }

    #[test]
    fn a_failed_write_says_so_and_the_list_is_still_read() {
        let (db, ids) = library(&["Oval"]);
        run_job(&db, write(&[ids[0]], true, one(ids[0], true)));
        // A design the library does not have cannot be excluded.
        let answer = run_job(&db, write(&[ids[0] + 100], true, one(ids[0] + 100, true)));
        let message = answer
            .write
            .expect("a write job answers")
            .expect_err("the library refuses a design it does not have");
        assert!(
            message.starts_with("Could not change the planner exclusion:"),
            "{message}"
        );
        assert_eq!(
            answer.listed,
            Ok(BTreeMap::from([(ids[0], "Oval".to_string())])),
            "the mark that is there stays in the list"
        );
    }

    #[test]
    fn a_panic_is_answered_as_a_failed_write_or_a_failed_read() {
        let failed_write = panicked(true, "boom");
        let message = failed_write.write.expect("a write answers").unwrap_err();
        assert!(message.contains("boom"), "{message}");
        assert!(message.starts_with("Could not change the planner exclusion:"));
        assert!(failed_write.listed.is_err());
        let read = panicked(false, "boom");
        assert_eq!(read.write, None);
        assert!(read.listed.unwrap_err().contains("boom"));
    }

    #[test]
    fn jobs_are_answered_in_the_order_they_were_asked_for() {
        let (db, ids) = library(&["Oval", "Round"]);
        let (answers_tx, answers_rx) = mpsc::channel::<Answer>();
        let jobs = spawn_queue(Arc::clone(&db), move |answer| {
            let _ = answers_tx.send(answer);
        })
        .expect("the thread starts");
        // Exclude, read, restore, read: a read never overtakes the write before it.
        jobs.send(write(&[ids[0]], true, one(ids[0], true)))
            .expect("sent");
        jobs.send(Job::Read).expect("sent");
        jobs.send(write(&[ids[0]], false, one(ids[0], false)))
            .expect("sent");
        jobs.send(Job::Read).expect("sent");
        let listed = |answer: Answer| answer.listed.expect("readable").len();
        let sizes: Vec<usize> = (0..4)
            .map(|_| listed(answers_rx.recv_timeout(WAIT).expect("an answer")))
            .collect();
        assert_eq!(sizes, vec![1, 1, 0, 0]);
        // Dropping the sender ends the thread: the answer channel closes with it.
        drop(jobs);
        assert!(answers_rx.recv_timeout(WAIT).is_err());
    }

    #[test]
    fn a_job_that_cannot_be_queued_is_reported() {
        let (jobs, inbox) = mpsc::channel::<Job>();
        drop(inbox);
        let worker = ExclusionWorker {
            jobs: Some(jobs),
            pending_writes: Cell::new(0),
        };
        assert!(!worker.submit(Job::Read));
        assert!(!worker.submit(write(&[1], true, Change::All)));
        assert!(
            !worker.writes_pending(),
            "a write that was not queued is not awaited"
        );
        let without_thread = ExclusionWorker {
            jobs: None,
            pending_writes: Cell::new(0),
        };
        assert!(!without_thread.submit(Job::Read));
    }

    #[test]
    fn a_write_is_pending_until_its_answer_arrives() {
        let (jobs, _inbox) = mpsc::channel::<Job>();
        let worker = ExclusionWorker {
            jobs: Some(jobs),
            pending_writes: Cell::new(0),
        };
        assert!(worker.submit(Job::Read));
        assert!(!worker.writes_pending(), "a read is not a write");
        assert!(worker.submit(write(&[1], true, Change::All)));
        assert!(worker.writes_pending());
        worker.write_answered();
        assert!(!worker.writes_pending());
        worker.write_answered();
        assert!(
            !worker.writes_pending(),
            "an extra answer does not go below none"
        );
    }
}
