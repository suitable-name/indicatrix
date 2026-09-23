//! High-resolution still export, running entirely off the interactive render thread.
//!
//! The live viewport (`render_thread.rs`) ties `RenderContext.width`/`height` to the
//! widget and progressively accumulates samples into a matching buffer, so an export
//! must not touch that buffer or block it. This module instead: takes its own
//! read-only [`SceneSnapshot`] captured once under a short lock; owns its own
//! accumulation buffer sized to the export's own dimensions; runs on its own
//! `thread::spawn` worker (parallelized internally with `thread::scope`); and reports
//! progress/cancellation via an [`ExportHandle`].
//!
//! # Wide-gamut export
//!
//! `run_export`'s tone-mapping branches on the caller's `ColorSpace`: `Srgb` (default)
//! goes through [`tonemap_to_rgba`]'s `xyz_to_srgb_gamma` path.
//! Any other space routes through [`tonemap_wide_gamut`] (`ColorSpace::encode` with
//! `ToneMap::AcesFilmic { exposure: 1.0 }`), which reproduces `xyz_to_srgb_gamma`'s
//! tone-mapping exactly so the wide-gamut path only changes gamut primaries and
//! transfer curve, never brightness. [`save_png`] embeds an ICC profile
//! (`bridge::icc_profile::build`) for any non-`Srgb` space so pixel values are never
//! silently misinterpreted as sRGB.
//!
//! `ColorSpace::AcesCg` is deliberately not offered by `export_dialog.slint`'s picker:
//! it is scene-linear, meant to feed further compositing -- quantizing scene-linear
//! light to 8 bits per channel would cause severe banding.
//!
//! Submodules: [`params`] (validation, default output path), [`scene_snapshot`]
//! ([`SceneSnapshot`] capture), [`batch`] (hybrid calibration/dispatch, CPU scanline
//! tracer), [`tonemap_png`] (tone-map, PNG/ICC output), [`worker`] (`run_export`
//! itself, the concurrent local+remote dispatch/merge orchestration). This file keeps
//! the public entry point (`spawn_export`) and its handle/outcome types.

// `pub`, not plain `mod`: `bridge::preview_render` reuses `batch::render_batch` for its
// own CPU fallback. Every other item in `batch` stays `pub(super)`. Plain `pub` rather
// than `pub(crate)` costs nothing extra since `bridge` itself is a private module.
pub mod batch;
pub mod filename_template;
// Builds the ICC profile embedded in a Display P3/Rec.2020 PNG so it isn't silently
// misinterpreted as sRGB. Nested here since `export_thread` is its only consumer.
mod icc_profile;
mod params;
mod preview;
pub mod remote;
mod sample_cursor;
mod scene_snapshot;
mod tonemap_png;
mod worker;

#[cfg(test)]
mod hybrid_export_tests;
#[cfg(test)]
mod tests;

pub use filename_template::{DEFAULT_TEMPLATE, TemplateContext, resolve_export_path};
pub use params::{ComputeTarget, ExportParams, validate_export_params};
pub use remote::probe_remote;
pub use scene_snapshot::SceneSnapshot;

use crate::settings::{LocalComputeTarget, WorkerSettings};
use indicatrix::color::ColorSpace;
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer, Weak};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};
use worker::run_export;

#[derive(Debug)]
pub enum ExportOutcome {
    Completed(PathBuf),
    Cancelled,
    Failed(String),
}

/// One progress update from an in-flight export, delivered to `on_progress` after each
/// sample batch. `preview` is `Some` only when `PreviewThrottle`'s ~2/sec rate limit
/// allows a regenerated thumbnail (see the `preview` submodule); callers should leave
/// the currently-displayed image alone on `None` rather than clear it.
pub struct ExportProgress {
    /// 0.0..=1.0, `samples_done as f32 / samples_total as f32`.
    pub fraction: f32,
    pub samples_done: u32,
    pub samples_total: u32,
    /// A small sRGB thumbnail of the combined local (CPU+GPU) and, while remote is
    /// also in flight, remote accumulation so far -- see `preview::downsample_preview`.
    pub preview: Option<SharedPixelBuffer<Rgba8Pixel>>,
    /// A one-off, user-facing status line about the REMOTE half of this export (e.g.
    /// a fallback to local-only, or a recovered mid-export worker failure). `None` on
    /// every other tick; callers should show it once rather than treat a later `None`
    /// as "clear the message".
    pub note: Option<String>,
}

/// Handle returned by `spawn_export`. Cancelling is cooperative: the worker checks the
/// flag between sample batches, so cancellation lands within one batch's worth of work
/// rather than instantly.
pub struct ExportHandle {
    cancel: Arc<AtomicBool>,
}

impl ExportHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Spawns the export worker thread. `on_progress` is invoked on the UI event loop with
/// an [`ExportProgress`] after each sample batch; `on_done` is invoked exactly once,
/// when the export finishes, is cancelled, or fails. `color_space` selects the output
/// PNG's gamut/transfer curve -- see this module's doc comment. `compute_target`/
/// `workers` select and configure the remote engine -- see [`ComputeTarget`].
///
/// # `on_done` fires on every exit path, including a panic
///
/// The worker body runs inside `std::panic::catch_unwind`: without it, a panic in
/// `run_export` would unwind the thread without ever calling `on_done`, permanently
/// leaving `RenderContext::export_active` stuck `true` and freezing the live viewport.
/// Reporting a caught panic as `ExportOutcome::Failed` keeps `gui::render_export`'s
/// reset unconditional.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of one export request's own identity \
              (scene, output params/path/colour-space, the new compute-target/workers \
              choice, and the two UI callbacks) -- bundling them into a struct would \
              just move the same count into field access, not reduce it"
)]
pub fn spawn_export<T, P, D>(
    ui_weak: Weak<T>,
    scene: SceneSnapshot,
    params: ExportParams,
    color_space: ColorSpace,
    output_path: PathBuf,
    compute_target: ComputeTarget,
    workers: Vec<WorkerSettings>,
    local_compute: LocalComputeTarget,
    on_progress: P,
    on_done: D,
) -> ExportHandle
where
    T: ComponentHandle + 'static,
    P: Fn(&T, ExportProgress) + Send + 'static + Clone,
    D: Fn(&T, ExportOutcome) + Send + 'static + Clone,
{
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = cancel.clone();
    let ui_weak_done = ui_weak.clone();

    thread::spawn(move || {
        let progress_ui_weak = ui_weak;
        let report_progress = move |progress: ExportProgress| {
            let on_progress = on_progress.clone();
            let _ = progress_ui_weak.upgrade_in_event_loop(move |ui| on_progress(&ui, progress));
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_export(
                &scene,
                params,
                color_space,
                &output_path,
                compute_target,
                &workers,
                local_compute,
                &cancel_worker,
                report_progress,
            )
        }))
        .unwrap_or_else(|payload| {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "the export worker panicked".to_string());
            ExportOutcome::Failed(format!("Export failed unexpectedly: {message}"))
        });
        let _ = ui_weak_done.upgrade_in_event_loop(move |ui| on_done(&ui, outcome));
    });

    ExportHandle { cancel }
}
