//! The durable job store: one directory of files, one set of three per job.
//!
//! ```text
//! <id>.job.json     the frozen job (`indicatrix_render_jobs::RenderJobFile`), written once
//! <id>.state.json   owner, state, progress, timestamps, note -- rewritten on every change
//! <id>.png          the result, once the job is done
//! ```
//!
//! `<id>` is the 32-hex job id. The job file is exactly what the desktop render queue
//! stores for a still picture, so the same validation (`RenderJobFile::validate`) and the
//! same states (`indicatrix_render_jobs::JobState`) apply; only the owner and the progress
//! are new, and they live in the state file. The state file is the commit point of a
//! submission: a job file without one (a crash between the two writes) is removed at start-up.
//!
//! # Recovery
//!
//! `open` reads every state file. A job that was `Running` when the server stopped goes back
//! to `Queued` from sample 0 with an explanatory note (a render is deterministic, so the
//! picture is the same). `Done` jobs whose PNG is missing become `Failed`. Terminal states are
//! left as they are. A job whose files do not parse is deleted with a warning, not served.
//!
//! # Retention and limits
//!
//! A terminal job is deleted [`Limits::retention_secs`] after it ended. Submissions are
//! refused past [`Limits::max_active_per_owner`] unfinished jobs for one owner,
//! [`Limits::max_jobs`] stored in total, or [`Limits::max_stored_bytes`] of results.

use indicatrix_net::messages::{
    JobId, JobInfo, JobPhase, JobSubmit, MAX_JOB_LIST, MAX_JOB_RESULT_LEN, WireColorSpace,
    derive_job_id, hash_hex, job_id_hex, parse_job_id_hex,
};
use indicatrix_render_jobs::{
    ComputeChoice, JobColorSpace, JobKind, JobState, RenderJobFile, StillJob, TransferChoice,
    from_text,
    job::{DesignInfo, HdrSource, JOB_FILE_SUFFIX, JobCompute},
    to_text,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// The suffix of a job's state file.
const STATE_SUFFIX: &str = ".state.json";
/// The suffix of a job's result.
const RESULT_SUFFIX: &str = ".png";
/// The state file format this build writes. 2: the backdrop is the lit tray (nothing in the
/// state file changed shape; an old worker refuses the store instead of replaying it wrongly).
const STATE_FORMAT: u64 = 2;
/// The oldest state format still read. A format-1 job was frozen under the card rule: its job
/// file has no `scene_backdrop_model` marker, so `load_job` converts its card level (`0.23`,
/// `8.0`) to the tray of the file's own preset and pose.
const OLDEST_STATE_FORMAT: u64 = 1;

/// The note left on a job a restart put back in the queue.
pub const RESTART_NOTE: &str =
    "The server restarted while this job was rendering, so it renders again from the start.";
/// The note left on a job the owner cancelled.
pub const CANCELLED_NOTE: &str = "Cancelled by its owner.";

/// The most pixels a durable job may have (4096 x 4096): bounds the result's size.
pub const MAX_JOB_PIXELS: u64 = 4096 * 4096;

/// How many jobs, how much, and for how long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Seconds a finished job is kept after it ended.
    pub retention_secs: i64,
    /// Unfinished jobs one owner may hold.
    pub max_active_per_owner: usize,
    /// Jobs stored in total.
    pub max_jobs: usize,
    /// Bytes of results stored in total.
    pub max_stored_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            retention_secs: 7 * 24 * 3600,
            max_active_per_owner: 8,
            max_jobs: 256,
            max_stored_bytes: 8 * 1024 * 1024 * 1024,
        }
    }
}

/// Why a store call did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    /// No such job for this owner (missing, expired or someone else's).
    Unknown,
    /// The job is in a state that does not allow the request.
    State(String),
    /// The submission itself is invalid.
    Invalid(String),
    /// The store is full.
    Limit(String),
    /// A file could not be read or written.
    Io(String),
}

/// A job handed to the executor.
#[derive(Debug, Clone)]
pub struct Claim {
    /// The job.
    pub id: JobId,
    /// Raised when the owner cancels the running job.
    pub cancel: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
struct Record {
    owner: String,
    state: JobState,
    samples_done: u32,
    samples_total: u32,
    width: u32,
    height: u32,
    label: String,
    created_unix: i64,
    finished_unix: Option<i64>,
    result_len: Option<u32>,
    note: String,
}

struct Inner {
    records: BTreeMap<JobId, Record>,
    queue: VecDeque<JobId>,
    running: Option<(JobId, Arc<AtomicBool>)>,
}

/// The store: see the module doc comment.
pub struct JobStore {
    dir: PathBuf,
    limits: Limits,
    inner: Mutex<Inner>,
    wake: Condvar,
}

const fn terminal(state: JobState) -> bool {
    matches!(
        state,
        JobState::Done | JobState::Failed | JobState::Cancelled
    )
}

const fn phase(state: JobState) -> JobPhase {
    match state {
        JobState::Queued | JobState::Paused => JobPhase::Queued,
        JobState::Running => JobPhase::Running,
        JobState::Done => JobPhase::Done,
        JobState::Failed => JobPhase::Failed,
        JobState::Cancelled => JobPhase::Cancelled,
    }
}

/// The current time in unix seconds.
#[must_use]
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

fn state_to_text(record: &Record) -> String {
    serde_json::json!({
        "format": STATE_FORMAT,
        "owner": record.owner,
        "state": record.state.as_str(),
        "samples_done": record.samples_done,
        "samples_total": record.samples_total,
        "width": record.width,
        "height": record.height,
        "label": record.label,
        "created_unix": record.created_unix,
        "finished_unix": record.finished_unix,
        "result_len": record.result_len,
        "note": record.note,
    })
    .to_string()
}

fn state_from_text(text: &str) -> Option<Record> {
    let value: Value = serde_json::from_str(text).ok()?;
    if !(OLDEST_STATE_FORMAT..=STATE_FORMAT).contains(&value.get("format")?.as_u64()?) {
        return None;
    }
    let number = |key: &str| u32::try_from(value.get(key)?.as_u64()?).ok();
    Some(Record {
        owner: value.get("owner")?.as_str()?.to_owned(),
        state: JobState::parse(value.get("state")?.as_str()?)?,
        samples_done: number("samples_done")?,
        samples_total: number("samples_total")?,
        width: number("width")?,
        height: number("height")?,
        label: value.get("label")?.as_str()?.to_owned(),
        created_unix: value.get("created_unix")?.as_i64()?,
        finished_unix: value.get("finished_unix").and_then(Value::as_i64),
        result_len: value
            .get("result_len")
            .and_then(Value::as_u64)
            .and_then(|length| u32::try_from(length).ok()),
        note: value
            .get("note")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    })
}

/// Writes `bytes` to `path` through a temporary file and a rename, so a crash never leaves a
/// half-written file under the real name.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let result = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

impl JobStore {
    /// Opens (creating it if needed) the store at `dir` and recovers what a previous run left;
    /// see the module doc comment.
    ///
    /// # Errors
    ///
    /// The I/O error if `dir` cannot be created or listed.
    pub fn open(dir: &Path, limits: Limits, now: i64) -> std::io::Result<Self> {
        fs::create_dir_all(dir)?;
        let mut records = BTreeMap::new();
        let mut waiting: Vec<(i64, JobId)> = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(STATE_SUFFIX).and_then(parse_job_id_hex) else {
                continue;
            };
            let Some(mut record) = fs::read_to_string(entry.path())
                .ok()
                .and_then(|text| state_from_text(&text))
            else {
                tracing::warn!("jobs: removing the unreadable state file {name}");
                remove_job_files(dir, &id);
                continue;
            };
            let job_reads = fs::read_to_string(dir.join(job_file_name(&id)))
                .is_ok_and(|text| from_text(&text).is_ok());
            if !job_reads {
                tracing::warn!(
                    "jobs: removing job {}: its job file does not read",
                    job_id_hex(&id)
                );
                remove_job_files(dir, &id);
                continue;
            }
            let interrupted = matches!(record.state, JobState::Running | JobState::Paused);
            if interrupted {
                record.state = JobState::Queued;
                record.samples_done = 0;
                RESTART_NOTE.clone_into(&mut record.note);
            }
            let result_lost =
                record.state == JobState::Done && !dir.join(result_file_name(&id)).is_file();
            if result_lost {
                record.state = JobState::Failed;
                record.result_len = None;
                "The finished picture is missing from the server.".clone_into(&mut record.note);
                record.finished_unix.get_or_insert(now);
            }
            if interrupted || result_lost {
                let _ = write_atomic(
                    &dir.join(state_file_name(&id)),
                    state_to_text(&record).as_bytes(),
                );
            }
            if record.state == JobState::Queued {
                waiting.push((record.created_unix, id));
            }
            records.insert(id, record);
        }
        // Job files whose state file never landed belong to a submission that did not finish.
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let orphan = name
                .strip_suffix(JOB_FILE_SUFFIX)
                .and_then(parse_job_id_hex)
                .is_some_and(|id| !records.contains_key(&id));
            let temporary = Path::new(&name)
                .extension()
                .is_some_and(|extension| extension == "tmp");
            if orphan || temporary {
                let _ = fs::remove_file(entry.path());
            }
        }
        waiting.sort_unstable();
        let store = Self {
            dir: dir.to_path_buf(),
            limits,
            inner: Mutex::new(Inner {
                records,
                queue: waiting.into_iter().map(|(_, id)| id).collect(),
                running: None,
            }),
            wake: Condvar::new(),
        };
        store.purge_expired(now);
        Ok(store)
    }

    /// The directory the store lives in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Wakes a waiting executor (after its stop flag was raised).
    pub fn wake_all(&self) {
        self.wake.notify_all();
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn persist(&self, id: &JobId, record: &Record) -> Result<(), JobError> {
        write_atomic(
            &self.dir.join(state_file_name(id)),
            state_to_text(record).as_bytes(),
        )
        .map_err(|error| JobError::Io(format!("could not save the job state: {error}")))
    }

    fn info(&self, id: &JobId, record: &Record) -> JobInfo {
        JobInfo {
            job_id: *id,
            phase: phase(record.state),
            samples_done: record.samples_done,
            samples_total: record.samples_total,
            width: record.width,
            height: record.height,
            label: record.label.clone(),
            created_unix: record.created_unix,
            finished_unix: record.finished_unix,
            expires_unix: record
                .finished_unix
                .map(|finished| finished.saturating_add(self.limits.retention_secs)),
            result_len: record.result_len,
            note: record.note.clone(),
        }
    }

    /// Stores a new job for `owner`, or returns the one this token already made.
    ///
    /// # Errors
    ///
    /// [`JobError::Invalid`] for a submission the job file rules refuse, [`JobError::Limit`]
    /// when the store is full, [`JobError::Io`] when the files cannot be written.
    pub fn submit(&self, owner: &str, request: &JobSubmit, now: i64) -> Result<JobInfo, JobError> {
        let id = derive_job_id(owner, &request.client_token);
        let mut inner = self.lock();
        if let Some(existing) = inner.records.get(&id) {
            return if existing.owner == owner {
                Ok(self.info(&id, existing))
            } else {
                Err(JobError::Invalid("that job token is not usable".to_owned()))
            };
        }
        let active = inner
            .records
            .values()
            .filter(|record| record.owner == owner && !terminal(record.state))
            .count();
        if active >= self.limits.max_active_per_owner {
            return Err(JobError::Limit(format!(
                "this owner already has {active} unfinished jobs (the limit is {}); wait for one or cancel it",
                self.limits.max_active_per_owner
            )));
        }
        if inner.records.len() >= self.limits.max_jobs {
            return Err(JobError::Limit(format!(
                "the server stores at most {} jobs; delete a finished one first",
                self.limits.max_jobs
            )));
        }
        let stored: u64 = inner
            .records
            .values()
            .filter_map(|record| record.result_len)
            .map(u64::from)
            .sum();
        if stored >= self.limits.max_stored_bytes {
            return Err(JobError::Limit(
                "the server's job result storage is full; delete a finished job first".to_owned(),
            ));
        }
        let job = job_file(&id, request, now)?;
        let text = to_text(&job).map_err(|error| JobError::Invalid(error.to_string()))?;
        // The job file first, the state file second: the state file is the commit point.
        write_atomic(&self.dir.join(job_file_name(&id)), text.as_bytes())
            .map_err(|error| JobError::Io(format!("could not save the job: {error}")))?;
        let record = Record {
            owner: owner.to_owned(),
            state: JobState::Queued,
            samples_done: 0,
            samples_total: request.samples,
            width: request.scene.width,
            height: request.scene.height,
            label: request.label.clone(),
            created_unix: now,
            finished_unix: None,
            result_len: None,
            note: String::new(),
        };
        if let Err(error) = self.persist(&id, &record) {
            remove_job_files(&self.dir, &id);
            return Err(error);
        }
        let info = self.info(&id, &record);
        inner.records.insert(id, record);
        inner.queue.push_back(id);
        drop(inner);
        self.wake.notify_one();
        Ok(info)
    }

    /// How `owner`'s job stands.
    ///
    /// # Errors
    ///
    /// [`JobError::Unknown`].
    pub fn status(&self, owner: &str, id: &JobId) -> Result<JobInfo, JobError> {
        let inner = self.lock();
        let record = owned(&inner, owner, id)?;
        let info = self.info(id, record);
        drop(inner);
        Ok(info)
    }

    /// `owner`'s jobs, newest first, at most [`MAX_JOB_LIST`].
    #[must_use]
    pub fn list(&self, owner: &str) -> Vec<JobInfo> {
        let inner = self.lock();
        let mut jobs: Vec<JobInfo> = inner
            .records
            .iter()
            .filter(|(_, record)| record.owner == owner)
            .map(|(id, record)| self.info(id, record))
            .collect();
        drop(inner);
        jobs.sort_by(|a, b| {
            b.created_unix
                .cmp(&a.created_unix)
                .then_with(|| a.job_id.cmp(&b.job_id))
        });
        jobs.truncate(MAX_JOB_LIST);
        jobs
    }

    /// Stops `owner`'s queued or running job.
    ///
    /// # Errors
    ///
    /// [`JobError::Unknown`], or [`JobError::State`] when the job already ended.
    pub fn cancel(&self, owner: &str, id: &JobId, now: i64) -> Result<JobInfo, JobError> {
        let mut inner = self.lock();
        let state = owned(&inner, owner, id)?.state;
        match state {
            JobState::Running => {
                if let Some((running, flag)) = &inner.running
                    && running == id
                {
                    flag.store(true, Ordering::Release);
                }
            }
            JobState::Queued | JobState::Paused => {
                inner.queue.retain(|queued| queued != id);
                let record = inner.records.get_mut(id).ok_or(JobError::Unknown)?;
                record.state = JobState::Cancelled;
                record.finished_unix = Some(now);
                CANCELLED_NOTE.clone_into(&mut record.note);
                let record = record.clone();
                self.persist(id, &record)?;
            }
            JobState::Done | JobState::Failed | JobState::Cancelled => {
                return Err(JobError::State(
                    "only a waiting or running job can be cancelled".to_owned(),
                ));
            }
        }
        let record = owned(&inner, owner, id)?;
        let info = self.info(id, record);
        drop(inner);
        Ok(info)
    }

    /// Deletes `owner`'s job and its result.
    ///
    /// # Errors
    ///
    /// [`JobError::Unknown`], or [`JobError::State`] while the job is running.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the files go while the lock is held, so a resubmission of the same token cannot interleave"
    )]
    pub fn delete(&self, owner: &str, id: &JobId) -> Result<(), JobError> {
        let mut inner = self.lock();
        if owned(&inner, owner, id)?.state == JobState::Running {
            return Err(JobError::State(
                "a running job cannot be deleted; cancel it first".to_owned(),
            ));
        }
        inner.queue.retain(|queued| queued != id);
        inner.records.remove(id);
        remove_job_files(&self.dir, id);
        Ok(())
    }

    /// `owner`'s finished picture.
    ///
    /// # Errors
    ///
    /// [`JobError::Unknown`], [`JobError::State`] when the job is not `Done`, or
    /// [`JobError::Io`] when the file cannot be read.
    pub fn result(&self, owner: &str, id: &JobId) -> Result<(JobInfo, Vec<u8>), JobError> {
        let info = {
            let inner = self.lock();
            let record = owned(&inner, owner, id)?;
            if record.state != JobState::Done {
                return Err(JobError::State(format!(
                    "the job is {}, not done",
                    phase(record.state).label().to_lowercase()
                )));
            }
            let info = self.info(id, record);
            drop(inner);
            info
        };
        let bytes = fs::read(self.dir.join(result_file_name(id)))
            .map_err(|error| JobError::Io(format!("could not read the picture: {error}")))?;
        if info.result_len.map(|length| length as usize) != Some(bytes.len()) {
            return Err(JobError::Io(
                "the stored picture differs from its record".to_owned(),
            ));
        }
        Ok((info, bytes))
    }

    /// The frozen job file of `id` (the executor's input).
    ///
    /// # Errors
    ///
    /// A sentence if the file cannot be read or no longer validates.
    pub fn load_job(&self, id: &JobId) -> Result<RenderJobFile, String> {
        let text = fs::read_to_string(self.dir.join(job_file_name(id)))
            .map_err(|error| format!("the job file could not be read: {error}"))?;
        from_text(&text).map_err(|error| error.to_string())
    }

    /// Waits up to `wait` for a queued job and marks it running. `None` on timeout (the
    /// caller does its housekeeping and asks again) or when `stop` is raised.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the guard is handed to the condition variable and back on every wait, and the state file is written while it is held"
    )]
    pub fn claim_next(&self, stop: &AtomicBool, wait: Duration) -> Option<Claim> {
        let mut inner = self.lock();
        loop {
            if stop.load(Ordering::Acquire) {
                return None;
            }
            if let Some(id) = inner.queue.pop_front() {
                let Some(record) = inner.records.get_mut(&id) else {
                    continue;
                };
                if record.state != JobState::Queued {
                    continue;
                }
                record.state = JobState::Running;
                let record = record.clone();
                if let Err(error) = self.persist(&id, &record) {
                    tracing::warn!("jobs: {error:?}");
                }
                let cancel = Arc::new(AtomicBool::new(false));
                inner.running = Some((id, Arc::clone(&cancel)));
                drop(inner);
                return Some(Claim { id, cancel });
            }
            let (guard, timeout) = self
                .wake
                .wait_timeout(inner, wait)
                .unwrap_or_else(PoisonError::into_inner);
            inner = guard;
            if timeout.timed_out() {
                return None;
            }
        }
    }

    /// Records how many samples per pixel the running job has traced (memory only).
    pub fn set_progress(&self, id: &JobId, samples_done: u32) {
        if let Some(record) = self.lock().records.get_mut(id) {
            record.samples_done = samples_done.min(record.samples_total);
        }
    }

    /// Ends the running job `id` as done, with its picture.
    ///
    /// # Errors
    ///
    /// [`JobError::Limit`] when the picture exceeds [`MAX_JOB_RESULT_LEN`], else
    /// [`JobError::Io`] when it cannot be written (the caller then fails the job).
    pub fn finish_done(&self, id: &JobId, png: &[u8], now: i64) -> Result<(), JobError> {
        let length = u32::try_from(png.len())
            .ok()
            .filter(|length| *length <= MAX_JOB_RESULT_LEN)
            .ok_or_else(|| {
                JobError::Limit(format!(
                    "the finished picture is {} MiB, over the {} MiB result limit",
                    png.len() / (1024 * 1024),
                    MAX_JOB_RESULT_LEN / (1024 * 1024)
                ))
            })?;
        write_atomic(&self.dir.join(result_file_name(id)), png)
            .map_err(|error| JobError::Io(format!("could not save the picture: {error}")))?;
        self.finish(id, JobState::Done, String::new(), Some(length), now);
        Ok(())
    }

    /// Ends the running job `id` as failed, with a sentence.
    pub fn finish_failed(&self, id: &JobId, note: &str, now: i64) {
        self.finish(id, JobState::Failed, note.to_owned(), None, now);
    }

    /// Ends the running job `id` as cancelled.
    pub fn finish_cancelled(&self, id: &JobId, now: i64) {
        self.finish(
            id,
            JobState::Cancelled,
            CANCELLED_NOTE.to_owned(),
            None,
            now,
        );
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "the state file is written while the lock is held, so two writers of one record cannot reorder"
    )]
    fn finish(&self, id: &JobId, state: JobState, note: String, length: Option<u32>, now: i64) {
        let mut inner = self.lock();
        if inner
            .running
            .as_ref()
            .is_some_and(|(running, _)| running == id)
        {
            inner.running = None;
        }
        let Some(record) = inner.records.get_mut(id) else {
            // Deleted while it ran (cannot happen through the API, but never panic).
            let _ = fs::remove_file(self.dir.join(result_file_name(id)));
            return;
        };
        record.state = state;
        record.note = note;
        record.finished_unix = Some(now);
        record.result_len = length;
        if state == JobState::Done {
            record.samples_done = record.samples_total;
        }
        let record = record.clone();
        if let Err(error) = self.persist(id, &record) {
            tracing::warn!("jobs: {error:?}");
        }
    }

    /// Deletes every terminal job whose retention ended at or before `now`. Returns how many.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the files go while the lock is held, so a resubmission of the same token cannot interleave"
    )]
    pub fn purge_expired(&self, now: i64) -> usize {
        let mut inner = self.lock();
        let expired: Vec<JobId> = inner
            .records
            .iter()
            .filter(|(_, record)| {
                terminal(record.state)
                    && record.finished_unix.is_some_and(|finished| {
                        finished.saturating_add(self.limits.retention_secs) <= now
                    })
            })
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            inner.records.remove(id);
            remove_job_files(&self.dir, id);
        }
        expired.len()
    }
}

fn owned<'a>(inner: &'a Inner, owner: &str, id: &JobId) -> Result<&'a Record, JobError> {
    inner
        .records
        .get(id)
        .filter(|record| record.owner == owner)
        .ok_or(JobError::Unknown)
}

fn job_file_name(id: &JobId) -> String {
    format!("{}{JOB_FILE_SUFFIX}", job_id_hex(id))
}

fn state_file_name(id: &JobId) -> String {
    format!("{}{STATE_SUFFIX}", job_id_hex(id))
}

fn result_file_name(id: &JobId) -> String {
    format!("{}{RESULT_SUFFIX}", job_id_hex(id))
}

fn remove_job_files(dir: &Path, id: &JobId) {
    for name in [job_file_name(id), state_file_name(id), result_file_name(id)] {
        let _ = fs::remove_file(dir.join(name));
    }
}

/// The frozen job a submission stands for: the desktop render queue's still-picture job.
fn job_file(id: &JobId, request: &JobSubmit, now: i64) -> Result<RenderJobFile, JobError> {
    let color_space = match request.color_space {
        WireColorSpace::Srgb => JobColorSpace::Srgb,
        WireColorSpace::DisplayP3 => JobColorSpace::DisplayP3,
        WireColorSpace::Rec2020 => JobColorSpace::Rec2020,
        WireColorSpace::AcesCg => {
            return Err(JobError::Invalid(
                "durable jobs write sRGB, Display P3 or Rec. 2020 pictures, not ACEScg".to_owned(),
            ));
        }
    };
    let pixels = u64::from(request.scene.width) * u64::from(request.scene.height);
    if pixels > MAX_JOB_PIXELS {
        return Err(JobError::Invalid(format!(
            "a durable job is at most {MAX_JOB_PIXELS} pixels (got {pixels})"
        )));
    }
    let hdr = request.scene.hdr().map(|environment| HdrSource {
        path: format!("{}.hdr", hash_hex(&environment.content_hash)),
        sha256_hex: hash_hex(&environment.content_hash),
    });
    Ok(RenderJobFile {
        engine: request.scene.engine,
        format: indicatrix_render_jobs::format_for_engine(request.scene.engine),
        token: job_id_hex(id),
        label: request.label.clone(),
        created_at: now,
        design: DesignInfo {
            title: request.label.clone(),
            ..DesignInfo::default()
        },
        scene: request.scene.clone(),
        scene_backdrop_model: indicatrix_render_jobs::job::BACKDROP_MODEL_TRAY,
        hdr,
        compute: JobCompute {
            target: ComputeChoice::Local,
            transfer: TransferChoice::FinalPicture,
            contribute_local: false,
        },
        output: result_file_name(id),
        kind: JobKind::Still(StillJob {
            samples_per_pixel: request.samples,
            color_space,
            preset_label: String::new(),
        }),
        optical_diagnostics: None,
    })
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
