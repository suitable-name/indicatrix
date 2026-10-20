//! The pure half of the GPU start-up test.
//!
//! It holds what the page does at load, how far a run has come, and the one sentence that
//! says what the app actually renders with. No DOM, no Workers, so all of it is tested
//! natively; the page's `render::gpu_gate` (and its `startup` and `status` modules) call it.
//!
//! # The start-up run
//!
//! At page load the page asks [`plan`] what to do:
//!
//! - [`StartupPlan::Off`]: the user switched GPU rendering off; nothing is started.
//! - [`StartupPlan::NoWebGpu`]: the browser has no WebGPU; no Worker is started for it.
//! - [`StartupPlan::Blocked`]: the last run was lost before it finished (the browser's GPU
//!   process died, or the GPU Worker never answered). It is NOT repeated by itself; the
//!   page shows the reason and keeps the manual "Forget GPU Test Result" / "Test GPU"
//!   route.
//! - [`StartupPlan::Run`]: spawn the GPU Worker and test.
//!
//! Before the Worker is spawned the page stores a crash marker under [`STARTUP_KEY`]
//! (the adapter is not known yet, so the per-adapter marker of `gpu_admission` cannot
//! exist). Once the adapter is known the per-adapter marker takes over and this one is
//! removed. A marker found at the next load blocks the run.
//!
//! When the adapter is known, [`adapter_plan`] reads its stored record: a new adapter gets
//! the full test (compile every kernel, then the 16x16 self-check), an adapter that passed
//! earlier gets the compile only (the self-check is skipped, the compile stays: it is the
//! step a lost GPU process is caught in), a failed one stays blocked.
//!
//! Progress is a [`Stage`] mapped to a fraction by [`progress`]; the sentence for the
//! render panel is [`summary`] and the status strip's short form is [`backend_line`].

use std::fmt::Write as _;

use crate::gpu_admission::{AdapterFacts, AdmissionRecord, AdmissionState};

/// The pseudo adapter key of the crash marker written before the GPU Worker is spawned.
/// Adapter keys are 16 hex digits, so this never collides with one.
pub const STARTUP_KEY: &str = "startup";

/// Reason shown when the start-up marker of the last run is still there.
pub const REASON_STARTUP_LOST: &str =
    "the browser lost the GPU while the GPU Worker was starting last time";
/// Reason shown when no browser storage takes the crash marker, so a crash could not be
/// remembered and the test is not run by itself.
pub const REASON_NO_STORAGE: &str =
    "the browser storage is not available, so a lost GPU could not be remembered";
/// Reason shown when the browser has no WebGPU.
pub const REASON_NO_WEBGPU: &str = "this browser has no WebGPU";

/// The gate's state as `RenderModel.gpu-state` carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuState {
    /// GPU rendering is off (or nothing is running for it).
    Off,
    /// An adapter is known and was never tested.
    Unknown,
    /// A test or the start-up compile is running.
    Testing,
    /// The adapter passed.
    Ok,
    /// The test failed, the adapter is unavailable or lost, or the run is blocked.
    Failed,
    /// GPU rendering is on and the Worker is starting.
    Starting,
}

impl GpuState {
    /// The `RenderModel.gpu-state` integer.
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::Off => 0,
            Self::Unknown => 1,
            Self::Testing => 2,
            Self::Ok => 3,
            Self::Failed => 4,
            Self::Starting => 5,
        }
    }

    /// The state of a `RenderModel.gpu-state` integer (anything unknown is `Off`).
    #[must_use]
    pub const fn from_code(code: i32) -> Self {
        match code {
            1 => Self::Unknown,
            2 => Self::Testing,
            3 => Self::Ok,
            4 => Self::Failed,
            5 => Self::Starting,
            _ => Self::Off,
        }
    }
}

/// What the page knows when it decides about the start-up run.
#[derive(Debug, Clone, Copy)]
pub struct StartupFacts<'a> {
    /// The "GPU Rendering" setting.
    pub enabled: bool,
    /// `navigator.gpu` exists.
    pub has_webgpu: bool,
    /// The record stored under [`STARTUP_KEY`], after the crash-marker conversion of the
    /// page load (a `Probing` or `Failed` record means the last run was lost).
    pub startup_record: Option<&'a AdmissionRecord>,
}

/// What to do at start-up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupPlan {
    /// GPU rendering is switched off.
    Off,
    /// The browser has no WebGPU.
    NoWebGpu,
    /// The last run was lost; the text is the reason to show. Not repeated by itself.
    Blocked(String),
    /// Spawn the GPU Worker and test.
    Run,
}

/// Decides the start-up run.
#[must_use]
pub fn plan(facts: &StartupFacts<'_>) -> StartupPlan {
    if !facts.enabled {
        return StartupPlan::Off;
    }
    if !facts.has_webgpu {
        return StartupPlan::NoWebGpu;
    }
    if let Some(record) = facts.startup_record
        && matches!(
            record.state,
            AdmissionState::Probing | AdmissionState::Failed
        )
    {
        let reason = record
            .reason
            .clone()
            .unwrap_or_else(|| REASON_STARTUP_LOST.to_string());
        return StartupPlan::Blocked(reason);
    }
    StartupPlan::Run
}

/// What to do once the adapter is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterPlan {
    /// A new adapter: compile every kernel, then run the self-check render.
    RunTest,
    /// The adapter passed earlier: compile every kernel again in this page's Worker (under
    /// the crash marker), without the self-check.
    CompileOnly,
    /// Never tested and the run was not asked to test by itself: wait for "Test GPU".
    WaitForUser,
    /// Failed earlier (or a crash marker was left): do not test again by itself; the text
    /// is the reason.
    Blocked(String),
}

/// Decides the run for the adapter's stored `record`.
///
/// `auto` is whether the run may start the test by itself (page load, or switching the
/// setting on); a retry after "Forget GPU Test Result" is not automatic.
#[must_use]
pub fn adapter_plan(record: &AdmissionRecord, auto: bool) -> AdapterPlan {
    match record.state {
        AdmissionState::Unknown => {
            if auto {
                AdapterPlan::RunTest
            } else {
                AdapterPlan::WaitForUser
            }
        }
        AdmissionState::Ok => AdapterPlan::CompileOnly,
        AdmissionState::Failed => AdapterPlan::Blocked(
            record
                .reason
                .clone()
                .unwrap_or_else(|| "unknown error".to_string()),
        ),
        AdmissionState::Probing => {
            AdapterPlan::Blocked("the browser lost the GPU during the last test".to_string())
        }
    }
}

/// What happens to the adapter's own crash marker when a run is ended on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterMarker {
    /// No compile of this page is running; the stored record is left as it is.
    Keep,
    /// The record goes back to this state (the state it had before the compile marked it
    /// `Probing`).
    Restore(AdmissionState),
}

/// The crash markers a page that is going away (`pagehide`) or switching the GPU off
/// releases, because the run is ending on purpose and not by a lost GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkerRelease {
    /// Remove the start-up marker ([`STARTUP_KEY`]).
    pub startup: bool,
    /// What to do with the adapter's record.
    pub adapter: AdapterMarker,
}

/// Which markers this page owns in its current phase and so releases when it ends the run
/// itself.
///
/// These are the start-up marker while the worker has not answered yet (`starting`), and the
/// adapter's record while the full test (`testing`: back to `Unknown`) or the compile of an
/// adapter that passed earlier (`admitting`: back to `Ok`) runs. A phase that is over leaves
/// its record alone (it holds the verdict). A real GPU-process crash ends the page without
/// this being called, so its markers stay and the next load reads them as a lost GPU.
#[must_use]
pub const fn release_markers(starting: bool, testing: bool, admitting: bool) -> MarkerRelease {
    let adapter = if testing {
        AdapterMarker::Restore(AdmissionState::Unknown)
    } else if admitting {
        AdapterMarker::Restore(AdmissionState::Ok)
    } else {
        AdapterMarker::Keep
    };
    MarkerRelease {
        startup: starting,
        adapter,
    }
}

/// The crash markers a page restored from the back/forward cache (`pageshow` with
/// `persisted`) writes again for a run that is still going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkerRearm {
    /// Write the start-up marker (`Probing` under [`STARTUP_KEY`]).
    pub startup: bool,
    /// Write the adapter's record in the `Probing` state again.
    pub adapter: bool,
}

/// The inverse of [`release_markers`].
///
/// For the same phase flags, these are the markers that `pagehide` cleared and that a
/// restored page must put back, so that a GPU lost during the rest of the run is remembered.
/// A finished phase has nothing to re-arm.
#[must_use]
pub const fn rearm_markers(starting: bool, testing: bool, admitting: bool) -> MarkerRearm {
    MarkerRearm {
        startup: starting,
        adapter: testing || admitting,
    }
}

/// A step of the run, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The GPU Worker is starting and asking the browser for an adapter.
    Adapter,
    /// The adapter answered; its limits are checked.
    Limits,
    /// `done` of `total` kernels compiled.
    Compile {
        /// Kernels answered so far.
        done: usize,
        /// Kernels in the set.
        total: usize,
    },
    /// The 16x16 test render runs.
    SelfCheck,
    /// Finished.
    Done,
}

/// How far the run is, `0.0..=1.0`, never going backwards from one stage to the next.
///
/// `with_self_check` is whether the run ends with the test render (the compile then stops
/// short of the top of the bar to leave room for it).
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "a kernel count of at most a few dozen is exact in f32"
)]
pub fn progress(stage: Stage, with_self_check: bool) -> f32 {
    const ADAPTER: f32 = 0.05;
    const LIMITS: f32 = 0.30;
    const COMPILE_START: f32 = 0.35;
    let compile_end: f32 = if with_self_check { 0.85 } else { 0.97 };
    match stage {
        Stage::Adapter => ADAPTER,
        Stage::Limits => LIMITS,
        Stage::Compile { done, total } => {
            if total == 0 {
                return COMPILE_START;
            }
            let share = (done.min(total) as f32) / (total as f32);
            (compile_end - COMPILE_START).mul_add(share, COMPILE_START)
        }
        Stage::SelfCheck => 0.92,
        Stage::Done => 1.0,
    }
}

/// The text next to the progress bar.
#[must_use]
pub fn stage_label(stage: Stage, adapter: Option<&str>) -> String {
    match stage {
        Stage::Adapter => "Starting the GPU worker and requesting the adapter".to_string(),
        Stage::Limits => adapter.map_or_else(
            || "Checking the adapter limits".to_string(),
            |name| format!("Checking the adapter limits ({name})"),
        ),
        Stage::Compile { done, total } => {
            format!("Compiling the GPU kernels ({done}/{total})")
        }
        Stage::SelfCheck => "Running a test render".to_string(),
        Stage::Done => "Done".to_string(),
    }
}

/// A readable adapter name: the browser's description, else vendor and architecture, else
/// the device, else the backend.
#[must_use]
pub fn adapter_display_name(facts: &AdapterFacts<'_>) -> String {
    let description = facts.description.trim();
    if !description.is_empty() {
        return description.to_string();
    }
    let vendor_arch = format!("{} {}", facts.vendor.trim(), facts.architecture.trim());
    let vendor_arch = vendor_arch.trim();
    if !vendor_arch.is_empty() {
        return vendor_arch.to_string();
    }
    let device = facts.device.trim();
    if !device.is_empty() {
        return device.to_string();
    }
    let backend = facts.backend.trim();
    if backend.is_empty() {
        "unnamed adapter".to_string()
    } else {
        format!("unnamed {backend} adapter")
    }
}

/// What [`summary`] and [`backend_line`] describe.
#[derive(Debug, Clone, Copy)]
pub struct SummaryInput<'a> {
    /// The "GPU Rendering" setting.
    pub enabled: bool,
    /// The browser has no WebGPU.
    pub no_webgpu: bool,
    /// The gate's state.
    pub state: GpuState,
    /// CPU render Workers.
    pub workers: u32,
    /// The adapter's readable name, once known.
    pub adapter: Option<&'a str>,
    /// The gate's own status line; for a failed state it carries the reason.
    pub detail: &'a str,
    /// The GPU takes part in the current render.
    pub admitted: bool,
    /// Why the current scene cannot use the GPU (anisotropic, zoned, ...), if it cannot.
    pub scene_reason: Option<&'a str>,
}

/// "7 workers" / "1 worker".
#[must_use]
pub fn workers_text(workers: u32) -> String {
    if workers == 1 {
        "1 worker".to_string()
    } else {
        format!("{workers} workers")
    }
}

fn with_stop(mut text: String) -> String {
    if !text.ends_with(['.', '!', '?']) {
        text.push('.');
    }
    text
}

/// The sentence that says what the app renders with, for the render panel and the toast at
/// the end of the run.
#[must_use]
pub fn summary(input: &SummaryInput<'_>) -> String {
    let workers = workers_text(input.workers);
    let gpu_name = input.adapter.unwrap_or("this graphics card");
    if !input.enabled {
        return format!("GPU rendering is off. Rendering on the CPU ({workers}).");
    }
    if input.no_webgpu {
        return format!("Rendering on the CPU ({workers}): {REASON_NO_WEBGPU}.");
    }
    match input.state {
        GpuState::Off | GpuState::Starting => {
            format!("Starting the GPU. Rendering on the CPU ({workers}) meanwhile.")
        }
        GpuState::Testing => input.adapter.map_or_else(
            || format!("Testing the GPU. Rendering on the CPU ({workers}) meanwhile."),
            |name| format!("Testing the GPU ({name}). Rendering on the CPU ({workers}) meanwhile."),
        ),
        GpuState::Unknown => {
            format!("Rendering on the CPU ({workers}): the GPU has not been tested yet.")
        }
        GpuState::Failed => {
            let detail = input.detail.trim();
            let detail = if detail.is_empty() {
                "the GPU is not available"
            } else {
                detail
            };
            with_stop(format!("Rendering on the CPU ({workers}): {detail}"))
        }
        GpuState::Ok => input.scene_reason.map_or_else(
            || {
                if input.admitted {
                    input.adapter.map_or_else(
                        || format!("Rendering on the GPU with {workers} on the CPU."),
                        |name| format!("Rendering on the GPU ({name}) with {workers} on the CPU."),
                    )
                } else {
                    format!(
                        "The GPU is ready ({gpu_name}); it joins the CPU workers ({workers}) \
                         once the first image has appeared."
                    )
                }
            },
            |reason| {
                let mut text = format!(
                    "Rendering on the CPU ({workers}): this stone cannot use the GPU ({reason})."
                );
                let _ = write!(text, " The GPU is ready ({gpu_name}).");
                text
            },
        ),
    }
}

/// The status strip's short form of [`summary`].
#[must_use]
pub fn backend_line(input: &SummaryInput<'_>) -> String {
    let workers = workers_text(input.workers);
    let cpu = format!("CPU \u{b7} {workers}");
    if !input.enabled {
        return format!("{cpu} (GPU off)");
    }
    if input.no_webgpu {
        return format!("{cpu} (no WebGPU)");
    }
    match input.state {
        GpuState::Off => cpu,
        GpuState::Starting => format!("{cpu} (GPU starting)"),
        GpuState::Unknown => format!("{cpu} (GPU not tested)"),
        GpuState::Testing => format!("{cpu} (GPU testing)"),
        GpuState::Failed => format!("{cpu} (GPU test failed)"),
        GpuState::Ok => input.scene_reason.map_or_else(
            || {
                if input.admitted {
                    format!("GPU + CPU \u{b7} {workers}")
                } else {
                    format!("{cpu} (GPU starting)")
                }
            },
            |reason| format!("{cpu} (GPU: {reason})"),
        ),
    }
}

#[cfg(test)]
mod tests;
