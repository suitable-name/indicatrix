//! [`run_export`]: the still-image export's thin wrapper around
//! [`super::final_picture::render_image_rgba`] -- render to RGBA8 (either transfer),
//! then PNG/ICC write.

use super::{
    final_picture::{RenderedImage, render_image_rgba},
    types::AccumulationCarry,
};
use crate::{
    bridge::export_thread::{
        ExportOutcome, ExportProgress, SceneSnapshot,
        params::{ExportParams, RemoteSelection},
        tonemap_png::save_png,
    },
    settings::LocalComputeTarget,
};
use indicatrix::{color::ColorSpace, renderer::gpu_backend::GpuBackend};
use std::{path::Path, sync::atomic::AtomicBool};

/// Renders one still image: acquires this export's own [`GpuBackend`] and a fresh
/// [`AccumulationCarry`] (a still export is always a single call, so there is nothing
/// to carry calibration between), delegates the render to [`render_image_rgba`] (the
/// local+remote full-data core, or the remote's own final picture), then writes the RGBA as PNG through [`save_png`], so ICC embedding is the same
/// whichever side tone-mapped. The tilt performance video (`gui::tilt::video_export`)
/// calls [`render_image_rgba`] directly instead, reusing ONE
/// [`GpuBackend`]/[`AccumulationCarry`] pair across every frame of the sweep -- see that
/// module's own doc comment.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of one export request's own identity \
              (scene, output params/path/color-space, the remote selection, the \
              local-compute choice, cancellation, and progress reporting) -- bundling \
              them into a struct would just move the same count into field access, \
              not reduce it"
)]
pub fn run_export(
    scene: &SceneSnapshot,
    params: ExportParams,
    color_space: ColorSpace,
    output_path: &Path,
    remote: &RemoteSelection,
    local_compute: LocalComputeTarget,
    cancel: &AtomicBool,
    report_progress: impl FnMut(ExportProgress),
) -> ExportOutcome {
    // Acquired once for this (single-frame) export: adapter acquisition and shader
    // compilation are far too slow to repeat per batch, let alone per export.
    //
    // `LocalComputeTarget::Cpu` never acquires an adapter -- a disabled backend
    // declines every `try_accumulate`, so `render_accumulation`'s hybrid calibration
    // never runs and the CPU tracer carries the whole export. `Gpu` and `CpuGpu` both
    // acquire; what separates them is `render_accumulation`'s own calibration gate,
    // not the backend.
    let gpu = match local_compute {
        LocalComputeTarget::Cpu => GpuBackend::disabled(),
        LocalComputeTarget::CpuGpu | LocalComputeTarget::Gpu => GpuBackend::acquire(),
    };
    let mut carry = AccumulationCarry::default();
    let rgba = match render_image_rgba(
        scene,
        scene.yaw,
        scene.pitch,
        params,
        color_space,
        remote,
        &gpu,
        local_compute,
        &mut carry,
        cancel,
        report_progress,
    ) {
        RenderedImage::Cancelled => return ExportOutcome::Cancelled,
        RenderedImage::Failed(message) => return ExportOutcome::Failed(message),
        RenderedImage::Rgba(rgba) => rgba,
    };

    if let Some(parent) = output_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return ExportOutcome::Failed(format!("Could not create output directory: {e}"));
    }

    match save_png(output_path, params.width, params.height, &rgba, color_space) {
        Ok(()) => ExportOutcome::Completed(output_path.to_path_buf()),
        Err(e) => ExportOutcome::Failed(format!("Failed to write PNG: {e}")),
    }
}
