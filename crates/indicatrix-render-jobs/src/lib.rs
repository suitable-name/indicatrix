//! Render jobs for Indicatrix.
//!
//! A render job is a frozen description of one still picture or one tilt video: the
//! fully resolved scene (material, facet planes, lighting, camera), the export
//! settings and the place the result goes. It is a plain JSON file (`*.job.json`).
//!
//! This crate holds everything about jobs that does not need a window, a database or a
//! renderer:
//!
//! - [`job`]: the job file types and their validation;
//! - [`codec`]: job file text to and from [`RenderJobFile`], with a format-version check;
//! - [`state`]: the job state machine and what each row command does;
//! - [`order`]: queue ordering and the next job to run;
//! - [`paths`]: collision-free output names, job file names and the frame folder marker;
//! - [`script`]: the `.sh` and `.ps1` script writer and its quoting rules;
//! - [`run`]: the progress and outcome types, and the progress line formatting.
//!
//! It is used by the desktop render queue (`indicatrix-cut`) and by the command-line
//! renderer (`indicatrix-cli render` and `indicatrix-cli tilt-video`). It stays pure
//! (no Slint, no vault, no I/O) so its tests run in seconds and both consumers can
//! share one definition of the job file, the state rules and the script text.

pub mod codec;
pub mod job;
pub mod order;
pub mod paths;
pub mod run;
pub mod script;
pub mod state;

pub use codec::{JobFileError, from_text, to_text};
pub use job::{
    ComputeChoice, JobColorSpace, JobKind, LocalEngines, RenderJobFile, StillJob, TiltVideoJob,
    TransferChoice,
};
pub use run::{FailureKind, JobOutcome, JobProgress, JobSink};
pub use state::JobState;
