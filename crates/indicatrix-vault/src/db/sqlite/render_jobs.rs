//! Storage for `render_jobs` -- the desktop render queue (order, state, progress, paths,
//! timestamps and the frozen job snapshot).
//!
//! See `Database::migrate_render_jobs_table`'s doc comment (in `super::migrations`) for
//! why the table has no foreign key to designs, and `crate::model::render_job` for the
//! models. The vault never parses a job's snapshot: it is opaque JSON text.
//!
//! Every method that changes a row by id returns how many rows changed: 0 for a missing
//! id (for example the job was deleted in another window), which is not an error here.

use super::Database;
use crate::model::render_job::{
    NewRenderJob, RENDER_JOB_KINDS, RENDER_JOB_STATES, RenderJob, RenderJobMeta,
};
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};

/// The metadata columns of a list or get query, in the order [`meta_of`] reads them.
const META_COLUMNS: &str = "job_id, position, kind, label, summary, state, frames_done, \
     frames_total, output_path, result_path, error_text, created_at, updated_at, \
     started_at, finished_at";

/// Reads the `INTEGER` column `index` of `row` as a `u32`.
fn u32_col(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u32> {
    let value: i64 = row.get(index)?;
    u32::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

/// Reads [`META_COLUMNS`] from `row`.
fn meta_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<RenderJobMeta> {
    Ok(RenderJobMeta {
        job_id: row.get(0)?,
        position: row.get(1)?,
        kind: row.get(2)?,
        label: row.get(3)?,
        summary: row.get(4)?,
        state: row.get(5)?,
        frames_done: u32_col(row, 6)?,
        frames_total: u32_col(row, 7)?,
        output_path: row.get(8)?,
        result_path: row.get(9)?,
        error_text: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        started_at: row.get(13)?,
        finished_at: row.get(14)?,
    })
}

impl Database {
    /// Appends a job to the end of the queue in state `queued`, returning its new
    /// `job_id`. Both `created_at` and `updated_at` are `now` (Unix seconds).
    ///
    /// # Errors
    ///
    /// Returns an error if `job.kind` is not one of [`RENDER_JOB_KINDS`], if
    /// `job.label` is empty, or if the underlying `INSERT` fails.
    pub fn add_render_job(&self, job: &NewRenderJob<'_>, now: i64) -> Result<i64> {
        if !RENDER_JOB_KINDS.contains(&job.kind) {
            anyhow::bail!("unknown render job kind '{}'", job.kind);
        }
        if job.label.is_empty() {
            anyhow::bail!("a render job needs a label");
        }
        self.conn
            .execute(
                "INSERT INTO render_jobs (
                     position, kind, label, summary, state, frames_done, frames_total,
                     output_path, created_at, updated_at, snapshot_version, snapshot
                 ) VALUES (
                     COALESCE((SELECT MAX(position) FROM render_jobs), 0) + 1,
                     ?1, ?2, ?3, 'queued', 0, ?4, ?5, ?6, ?6, ?7, ?8
                 )",
                params![
                    job.kind,
                    job.label,
                    job.summary,
                    i64::from(job.frames_total),
                    job.output_path,
                    now,
                    i64::from(job.snapshot_version),
                    job.snapshot
                ],
            )
            .with_context(|| format!("Failed to insert render job '{}'", job.label))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Lists every render job without its snapshot, in queue order:
    /// `position, job_id`.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing the statement or reading rows fails.
    pub fn list_render_jobs(&self) -> Result<Vec<RenderJobMeta>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {META_COLUMNS} FROM render_jobs ORDER BY position, job_id"
        ))?;
        let rows = stmt.query_map([], meta_of)?;
        let mut metas = Vec::new();
        for row in rows {
            metas.push(row.context("Failed to read render job metadata row")?);
        }
        Ok(metas)
    }

    /// Fetches a full render job by its ID, snapshot included. Returns `None` if no job
    /// with `id` exists.
    ///
    /// # Errors
    ///
    /// Returns an error if querying fails or if a stored count or the snapshot version
    /// cannot be converted to `u32`.
    pub fn get_render_job(&self, id: i64) -> Result<Option<RenderJob>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {META_COLUMNS}, snapshot_version, snapshot
                     FROM render_jobs WHERE job_id = ?1"
                ),
                params![id],
                |row| {
                    Ok(RenderJob {
                        meta: meta_of(row)?,
                        snapshot_version: u32_col(row, 15)?,
                        snapshot: row.get(16)?,
                    })
                },
            )
            .optional()
            .with_context(|| format!("Failed to get render job for job_id: {id}"))
    }

    /// Sets the state of job `id`, its `error_text` (`None` clears it) and `updated_at`.
    ///
    /// `running` also sets `started_at` to `now`. `queued`, `paused` and `running` clear
    /// `finished_at`; `done`, `failed` and `cancelled` set it to `now`. Returns how many
    /// rows changed: 0 when the job no longer exists.
    ///
    /// # Errors
    ///
    /// Returns an error if `state` is not one of [`RENDER_JOB_STATES`] or the underlying
    /// `UPDATE` fails.
    pub fn set_render_job_state(
        &self,
        id: i64,
        state: &str,
        error: Option<&str>,
        now: i64,
    ) -> Result<usize> {
        if !RENDER_JOB_STATES.contains(&state) {
            anyhow::bail!("unknown render job state '{state}'");
        }
        self.conn
            .execute(
                "UPDATE render_jobs
                 SET state = ?1,
                     error_text = ?2,
                     updated_at = ?3,
                     started_at = CASE WHEN ?1 = 'running' THEN ?3 ELSE started_at END,
                     finished_at = CASE
                         WHEN ?1 IN ('done', 'failed', 'cancelled') THEN ?3
                         WHEN ?1 IN ('queued', 'paused', 'running') THEN NULL
                     END
                 WHERE job_id = ?4",
                params![state, error, now, id],
            )
            .with_context(|| format!("Failed to set the state of render job {id} to '{state}'"))
    }

    /// Stores how many frames job `id` has finished, and `updated_at`. Returns how many
    /// rows changed: 0 when the job no longer exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `UPDATE` fails.
    pub fn set_render_job_progress(&self, id: i64, frames_done: u32, now: i64) -> Result<usize> {
        self.conn
            .execute(
                "UPDATE render_jobs SET frames_done = ?1, updated_at = ?2 WHERE job_id = ?3",
                params![i64::from(frames_done), now, id],
            )
            .with_context(|| format!("Failed to store the progress of render job {id}"))
    }

    /// Stores what job `id` actually wrote (`None` clears it), and `updated_at`. Returns
    /// how many rows changed: 0 when the job no longer exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `UPDATE` fails.
    pub fn set_render_job_result(
        &self,
        id: i64,
        result_path: Option<&str>,
        now: i64,
    ) -> Result<usize> {
        self.conn
            .execute(
                "UPDATE render_jobs SET result_path = ?1, updated_at = ?2 WHERE job_id = ?3",
                params![result_path, now, id],
            )
            .with_context(|| format!("Failed to store the result of render job {id}"))
    }

    /// Puts job `id` back in the queue from the start: state `queued`, no frames done,
    /// and no error, result, start or finish time. Returns how many rows changed: 0
    /// when the job no longer exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `UPDATE` fails.
    pub fn reset_render_job(&self, id: i64, now: i64) -> Result<usize> {
        self.conn
            .execute(
                "UPDATE render_jobs
                 SET state = 'queued', frames_done = 0, error_text = NULL,
                     result_path = NULL, started_at = NULL, finished_at = NULL,
                     updated_at = ?1
                 WHERE job_id = ?2",
                params![now, id],
            )
            .with_context(|| format!("Failed to reset render job {id}"))
    }

    /// Renumbers the queue: each id in `ids` gets `position = index + 1`, in one
    /// transaction. An id with no job is skipped; a job not listed keeps its position.
    ///
    /// # Errors
    ///
    /// Returns an error if the transaction cannot start or commit, or an `UPDATE`
    /// fails; nothing is changed in that case.
    pub fn reorder_render_jobs(&self, ids: &[i64]) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("Failed to start the render job reorder")?;
        {
            let mut stmt = tx
                .prepare("UPDATE render_jobs SET position = ?1 WHERE job_id = ?2")
                .context("Failed to prepare the render job reorder")?;
            for (index, id) in ids.iter().enumerate() {
                let position = i64::try_from(index).unwrap_or(i64::MAX - 1) + 1;
                stmt.execute(params![position, id])
                    .with_context(|| format!("Failed to move render job {id}"))?;
            }
        }
        tx.commit()
            .context("Failed to commit the render job reorder")?;
        Ok(())
    }

    /// Deletes job `id`. Returns how many rows were deleted: 0 when the job no longer
    /// exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `DELETE` fails.
    pub fn delete_render_job(&self, id: i64) -> Result<usize> {
        self.conn
            .execute("DELETE FROM render_jobs WHERE job_id = ?1", params![id])
            .with_context(|| format!("Failed to delete render job {id}"))
    }

    /// Deletes every job that is `done` or `cancelled`. Returns how many rows were
    /// deleted.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `DELETE` fails.
    pub fn delete_finished_render_jobs(&self) -> Result<usize> {
        self.conn
            .execute(
                "DELETE FROM render_jobs WHERE state IN ('done', 'cancelled')",
                [],
            )
            .context("Failed to delete the finished render jobs")
    }

    /// Turns every `running` job into `paused` with `note` as its `error_text`, clears
    /// its `finished_at` and sets `updated_at` to `now`. Returns how many jobs changed.
    ///
    /// The app calls this at start-up: a job found running was cut off when the app
    /// closed.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `UPDATE` fails.
    pub fn interrupt_running_render_jobs(&self, note: &str, now: i64) -> Result<usize> {
        self.conn
            .execute(
                "UPDATE render_jobs
                 SET state = 'paused', error_text = ?1, finished_at = NULL, updated_at = ?2
                 WHERE state = 'running'",
                params![note, now],
            )
            .context("Failed to pause the interrupted render jobs")
    }

    /// The `output_path` of every job, in queue order, so a new job's file or folder name
    /// can avoid every name already reserved.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing the statement or reading rows fails.
    pub fn render_job_output_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT output_path FROM render_jobs ORDER BY position, job_id")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut paths = Vec::new();
        for row in rows {
            paths.push(row.context("Failed to read a render job output path")?);
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (Database, std::path::PathBuf) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "indicatrix-vault-render-jobs-test-{}-{n}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        (db, path)
    }

    fn cleanup(db: Database, path: &std::path::Path) {
        drop(db);
        std::fs::remove_file(path).ok();
    }

    fn still<'a>(label: &'a str, snapshot: &'a str) -> NewRenderJob<'a> {
        NewRenderJob {
            kind: "still",
            label,
            summary: "1920 x 1080 · 1024 samples",
            frames_total: 1,
            output_path: "C:\\out\\a.png",
            snapshot_version: 1,
            snapshot,
        }
    }

    fn meta(db: &Database, id: i64) -> RenderJobMeta {
        db.get_render_job(id).unwrap().expect("job exists").meta
    }

    fn ids(db: &Database) -> Vec<i64> {
        db.list_render_jobs()
            .unwrap()
            .iter()
            .map(|job| job.job_id)
            .collect()
    }

    #[test]
    fn j2_t1_add_get_and_list_round_trip_every_column() {
        let (db, path) = temp_db();
        let first = db.add_render_job(&still("Alpha", "{}"), 1_000).unwrap();
        let snapshot = "{\"a\":\"ü·×\"}";
        let video = db
            .add_render_job(
                &NewRenderJob {
                    kind: "tilt_video",
                    label: "Bärion · tilt video",
                    summary: "181 frames",
                    frames_total: 181,
                    output_path: "C:\\out\\frames",
                    snapshot_version: 3,
                    snapshot,
                },
                2_000,
            )
            .unwrap();

        let loaded = db.get_render_job(video).unwrap().expect("job exists");
        assert_eq!(
            loaded,
            RenderJob {
                meta: RenderJobMeta {
                    job_id: video,
                    position: 2,
                    kind: "tilt_video".to_string(),
                    label: "Bärion · tilt video".to_string(),
                    summary: "181 frames".to_string(),
                    state: "queued".to_string(),
                    frames_done: 0,
                    frames_total: 181,
                    output_path: "C:\\out\\frames".to_string(),
                    result_path: None,
                    error_text: None,
                    created_at: 2_000,
                    updated_at: 2_000,
                    started_at: None,
                    finished_at: None,
                },
                snapshot_version: 3,
                snapshot: snapshot.to_string(),
            }
        );
        assert_eq!(db.get_render_job(99_999).unwrap(), None);

        let list = db.list_render_jobs().unwrap();
        assert_eq!(ids(&db), vec![first, video]);
        assert_eq!(list[0].position, 1);
        cleanup(db, &path);
    }

    #[test]
    fn j2_t1_state_changes_set_and_clear_the_timestamps() {
        let (db, path) = temp_db();
        let job = db.add_render_job(&still("Alpha", "{}"), 1_000).unwrap();

        assert_eq!(
            db.set_render_job_state(job, "running", None, 3_000)
                .unwrap(),
            1
        );
        let m = meta(&db, job);
        assert_eq!(
            (m.state.as_str(), m.started_at, m.finished_at, m.updated_at),
            ("running", Some(3_000), None, 3_000)
        );
        db.set_render_job_state(job, "done", None, 4_000).unwrap();
        let m = meta(&db, job);
        assert_eq!(
            (m.state.as_str(), m.started_at, m.finished_at),
            ("done", Some(3_000), Some(4_000))
        );
        db.set_render_job_state(job, "paused", Some("note"), 5_000)
            .unwrap();
        let m = meta(&db, job);
        assert_eq!(
            (m.state.as_str(), m.finished_at, m.error_text.as_deref()),
            ("paused", None, Some("note"))
        );
        db.set_render_job_state(job, "failed", Some("boom"), 5_500)
            .unwrap();
        assert_eq!(meta(&db, job).finished_at, Some(5_500));
        cleanup(db, &path);
    }

    #[test]
    fn j2_t1_progress_result_and_reset() {
        let (db, path) = temp_db();
        let job = db.add_render_job(&still("Alpha", "{}"), 1_000).unwrap();
        db.set_render_job_state(job, "failed", Some("boom"), 2_000)
            .unwrap();

        assert_eq!(db.set_render_job_progress(job, 1, 6_000).unwrap(), 1);
        assert_eq!(
            db.set_render_job_result(job, Some("C:\\out\\a.png"), 6_100)
                .unwrap(),
            1
        );
        let m = meta(&db, job);
        assert_eq!(
            (m.frames_done, m.result_path.as_deref(), m.updated_at),
            (1, Some("C:\\out\\a.png"), 6_100)
        );
        db.set_render_job_result(job, None, 6_200).unwrap();
        assert_eq!(meta(&db, job).result_path, None);

        // Reset clears all four fields and keeps the identity columns.
        db.set_render_job_result(job, Some("x.png"), 6_300).unwrap();
        assert_eq!(db.reset_render_job(job, 7_000).unwrap(), 1);
        let m = meta(&db, job);
        assert_eq!(m.state, "queued");
        assert_eq!(m.frames_done, 0);
        assert_eq!(
            (m.error_text, m.result_path, m.started_at, m.finished_at),
            (None, None, None, None)
        );
        assert_eq!(
            (m.updated_at, m.created_at, m.label.as_str()),
            (7_000, 1_000, "Alpha")
        );
        cleanup(db, &path);
    }

    #[test]
    fn j2_t1_reorder_delete_and_clear_finished() {
        let (db, path) = temp_db();
        let first = db.add_render_job(&still("Alpha", "{}"), 1).unwrap();
        let second = db.add_render_job(&still("Beta", "{}"), 1).unwrap();

        db.reorder_render_jobs(&[second, first]).unwrap();
        let positions: Vec<(i64, i64)> = db
            .list_render_jobs()
            .unwrap()
            .iter()
            .map(|job| (job.job_id, job.position))
            .collect();
        assert_eq!(positions, vec![(second, 1), (first, 2)]);
        assert_eq!(
            db.render_job_output_paths().unwrap(),
            vec!["C:\\out\\a.png".to_string(); 2]
        );

        // A job added after a reorder goes last, and can be deleted.
        let third = db.add_render_job(&still("Gamma", "{}"), 8_000).unwrap();
        assert_eq!(meta(&db, third).position, 3);
        assert_eq!(db.delete_render_job(third).unwrap(), 1);
        assert_eq!(db.get_render_job(third).unwrap(), None);

        // Clearing finished jobs removes only done and cancelled ones.
        let done = db.add_render_job(&still("Done", "{}"), 9_000).unwrap();
        let cancelled = db.add_render_job(&still("Cancelled", "{}"), 9_000).unwrap();
        let failed = db.add_render_job(&still("Failed", "{}"), 9_000).unwrap();
        db.set_render_job_state(done, "done", None, 9_100).unwrap();
        db.set_render_job_state(cancelled, "cancelled", None, 9_100)
            .unwrap();
        db.set_render_job_state(failed, "failed", Some("x"), 9_100)
            .unwrap();
        assert_eq!(db.delete_finished_render_jobs().unwrap(), 2);
        assert_eq!(ids(&db), vec![second, first, failed]);
        cleanup(db, &path);
    }

    #[test]
    fn j2_t1_a_missing_id_changes_nothing_and_bad_words_are_refused() {
        let (db, path) = temp_db();
        let job = db.add_render_job(&still("Alpha", "{}"), 1).unwrap();

        assert_eq!(
            db.set_render_job_state(99_999, "paused", None, 1).unwrap(),
            0
        );
        assert_eq!(db.set_render_job_progress(99_999, 1, 1).unwrap(), 0);
        assert_eq!(db.set_render_job_result(99_999, None, 1).unwrap(), 0);
        assert_eq!(db.reset_render_job(99_999, 1).unwrap(), 0);
        assert_eq!(db.delete_render_job(99_999).unwrap(), 0);
        db.reorder_render_jobs(&[99_999]).unwrap();

        // Bad words are refused before the statement, and by the CHECK constraint.
        assert!(db.set_render_job_state(job, "bogus", None, 1).is_err());
        assert_eq!(meta(&db, job).state, "queued");
        let movie = NewRenderJob {
            kind: "movie",
            ..still("Bad kind", "{}")
        };
        assert!(db.add_render_job(&movie, 1).is_err());
        assert!(db.add_render_job(&still("", "{}"), 1).is_err());
        let raw = db.conn.execute(
            "INSERT INTO render_jobs (position, kind, label, state, frames_total, output_path,
                 created_at, updated_at, snapshot_version, snapshot)
             VALUES (99, 'still', 'raw', 'bogus', 1, 'p', 1, 1, 1, '{}')",
            [],
        );
        assert!(raw.is_err(), "the CHECK constraint must reject a bad state");
        cleanup(db, &path);
    }

    #[test]
    fn j2_t2_migration() {
        const COLUMNS: [&str; 17] = [
            "job_id",
            "position",
            "kind",
            "label",
            "summary",
            "state",
            "frames_done",
            "frames_total",
            "output_path",
            "result_path",
            "error_text",
            "created_at",
            "updated_at",
            "started_at",
            "finished_at",
            "snapshot_version",
            "snapshot",
        ];
        let index_count = |db: &Database| -> i64 {
            db.conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'index' AND name = 'idx_render_jobs_position'",
                    [],
                    |row| row.get(0),
                )
                .unwrap()
        };

        let (db, path) = temp_db();
        for column in COLUMNS {
            assert!(
                Database::column_exists(&db.conn, "render_jobs", column).unwrap(),
                "render_jobs.{column} missing"
            );
        }
        assert_eq!(index_count(&db), 1);

        let stamp = db.library_stamp().unwrap();
        db.conn.execute_batch("DROP TABLE render_jobs;").unwrap();
        assert!(!Database::column_exists(&db.conn, "render_jobs", "job_id").unwrap());
        db.migrate_render_jobs_table().unwrap();
        db.migrate_render_jobs_table().unwrap();
        for column in COLUMNS {
            assert!(Database::column_exists(&db.conn, "render_jobs", column).unwrap());
        }
        assert_eq!(index_count(&db), 1);
        db.add_render_job(&still("After", "{}"), 1).unwrap();
        assert_eq!(db.library_stamp().unwrap(), stamp);

        drop(db);
        let reopened = Database::new(Some(path.to_str().unwrap())).unwrap();
        assert_eq!(reopened.library_stamp().unwrap(), stamp);
        assert_eq!(reopened.list_render_jobs().unwrap().len(), 1);
        cleanup(reopened, &path);
    }

    #[test]
    fn j2_t3_interrupt_running_jobs() {
        let (db, path) = temp_db();
        let running = db.add_render_job(&still("Running", "{}"), 100).unwrap();
        let queued = db.add_render_job(&still("Queued", "{}"), 100).unwrap();
        let done = db.add_render_job(&still("Done", "{}"), 100).unwrap();
        db.set_render_job_state(running, "running", None, 200)
            .unwrap();
        db.set_render_job_state(done, "done", None, 200).unwrap();

        let note = "The app closed while this job was running.";
        assert_eq!(db.interrupt_running_render_jobs(note, 300).unwrap(), 1);

        let m = meta(&db, running);
        assert_eq!(m.state, "paused");
        assert_eq!(m.error_text.as_deref(), Some(note));
        assert_eq!((m.finished_at, m.updated_at), (None, 300));
        assert_eq!(m.started_at, Some(200), "the start time is kept");
        let m = meta(&db, queued);
        assert_eq!(
            (m.state.as_str(), m.error_text, m.updated_at),
            ("queued", None, 100)
        );
        let m = meta(&db, done);
        assert_eq!((m.state.as_str(), m.finished_at), ("done", Some(200)));

        assert_eq!(db.interrupt_running_render_jobs("again", 400).unwrap(), 0);
        cleanup(db, &path);
    }
}
