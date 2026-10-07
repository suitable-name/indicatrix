//! Render jobs: model types for the desktop render queue stored in `render_jobs`.
//!
//! See `crate::db::sqlite::Database::add_render_job` and its sibling methods for the
//! storage side. The vault does not parse a job's snapshot: it is opaque text, like
//! `design_lighting.settings_json`.

/// Every state word the `render_jobs.state` column accepts, in display order.
pub const RENDER_JOB_STATES: [&str; 6] =
    ["queued", "running", "paused", "done", "failed", "cancelled"];

/// Every kind word the `render_jobs.kind` column accepts.
pub const RENDER_JOB_KINDS: [&str; 2] = ["still", "tilt_video"];

/// Metadata of a render job: every column except the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderJobMeta {
    /// The unique primary key of the job.
    pub job_id: i64,
    /// Queue order, ascending. Gaps are allowed.
    pub position: i64,
    /// One of [`RENDER_JOB_KINDS`].
    pub kind: String,
    /// Name the job list shows.
    pub label: String,
    /// One-line detail text for lists, written when the job is added.
    pub summary: String,
    /// One of [`RENDER_JOB_STATES`].
    pub state: String,
    /// Frames finished so far (a still counts 0 or 1).
    pub frames_done: u32,
    /// Frames the job renders in total (1 for a still).
    pub frames_total: u32,
    /// The PNG path (still) or the frame folder (tilt video).
    pub output_path: String,
    /// What was actually written: the picture, the video, or the frame folder.
    pub result_path: Option<String>,
    /// Why the job failed, or the note left when it was interrupted.
    pub error_text: Option<String>,
    /// Unix seconds when the job was added.
    pub created_at: i64,
    /// Unix seconds of the last change to the row.
    pub updated_at: i64,
    /// Unix seconds when the job last started running.
    pub started_at: Option<i64>,
    /// Unix seconds when the job last stopped for good (done, failed or cancelled).
    pub finished_at: Option<i64>,
}

/// A complete render job including its frozen snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderJob {
    /// Job metadata.
    pub meta: RenderJobMeta,
    /// Version number of the snapshot format (for compatibility checks).
    pub snapshot_version: u32,
    /// The job file as JSON text, opaque to the vault.
    pub snapshot: String,
}

/// The values needed to add a job to the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewRenderJob<'a> {
    /// One of [`RENDER_JOB_KINDS`].
    pub kind: &'a str,
    /// Name the job list shows. Must not be empty.
    pub label: &'a str,
    /// One-line detail text for lists.
    pub summary: &'a str,
    /// Frames the job renders in total (1 for a still).
    pub frames_total: u32,
    /// The PNG path (still) or the frame folder (tilt video).
    pub output_path: &'a str,
    /// Version number of the snapshot format.
    pub snapshot_version: u32,
    /// The job file as JSON text.
    pub snapshot: &'a str,
}
