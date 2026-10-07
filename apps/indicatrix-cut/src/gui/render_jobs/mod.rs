//! Render jobs: a frozen description of one still picture or one tilt video, rendered
//! later by the queue or by a script.
//!
//! The Slint-free core. The job file, the state machine, ordering, name reservation and the
//! script writer live in the shared crate `indicatrix-render-jobs`; what needs the renderer
//! lives here:
//!
//! - [`convert`]: the live app's types (`SceneSnapshot`, `RemoteSelection`, `ColorSpace`)
//!   to and from the job file, and the HDR map check;
//! - [`execute`]: the one executor that renders a job. A still is `run_export`; a tilt
//!   video is the direct export's frame loop, made resumable;
//! - [`headless`]: the public entry point `indicatrix-cli` calls, with no window.
//!
//! The queue and its window are built on those:
//!
//! - [`controller`]: starts jobs one at a time, follows them, applies the row commands and
//!   the close guard;
//! - [`rows`]: every text and button flag of the Jobs window, as plain data;
//! - [`wiring`]: the callbacks of `RenderJobsModel` and writing the data into it;
//! - [`capture_still`] and [`capture_video`]: "Add to Queue" in the export dialog and the
//!   tilt video section;
//! - [`script_export`]: the script folder for `indicatrix-cli`.

pub mod headless;

mod capture_still;
mod capture_video;
pub(crate) mod controller;
pub(crate) mod convert;
pub(crate) mod execute;
mod rows;
mod script_export;
mod wiring;

pub(crate) use controller::{stop_for_app_close, work_at_risk};
pub(crate) use wiring::setup_render_jobs;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod ui_tests;
