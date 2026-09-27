//! [`render_image_rgba`]: one finished RGBA8 image for the still export and the tilt
//! video, through whichever transfer applies:
//!
//! - **Full data**: [`render_accumulation`] (local CPU/GPU plus the remote's float
//!   radiance, merged), then `tonemap_accumulation` -- unchanged behaviour.
//! - **Final picture only**: one `FinalImageRequest`; the remote renders and tone-maps
//!   with the same `indicatrix::renderer::tonemap::tonemap_accumulation`, the viewer
//!   decodes the PNG to RGBA8. The local lanes do not take part.
//!
//! Either way the caller writes the RGBA through its own PNG writer, so ICC embedding
//! and file layout never depend on which side tone-mapped.

use super::{
    core::render_accumulation,
    types::{Accumulation, AccumulationCarry, AccumulationOutcome},
};
use crate::{
    bridge::{
        export_thread::{
            ExportProgress,
            params::{ExportParams, RemoteSelection},
            remote::{
                self, FinalPictureFollowUp, TransferPlan, final_picture_follow_up,
                final_picture_refused, plan_export_transfer, remember_final_picture_refused,
                run_final_image_request,
            },
            scene_snapshot::SceneSnapshot,
            tonemap_png::tonemap_accumulation,
        },
        remote::remote_can_render,
    },
    settings::{LocalComputeTarget, WorkerSettings},
};
use indicatrix::{color::ColorSpace, renderer::gpu_backend::GpuBackend};
use std::sync::atomic::AtomicBool;

/// One image's outcome from [`render_image_rgba`].
pub enum RenderedImage {
    /// Tone-mapped RGBA8, `width * height * 4` bytes, in the requested colour space.
    Rgba(Vec<u8>),
    /// `cancel` was observed before the image finished.
    Cancelled,
    /// The image failed outright (only reachable under `ComputeTarget::RemoteOnly`).
    Failed(String),
}

/// Renders one image at `(cam_yaw, cam_pitch)` to RGBA8 for `color_space` -- see the
/// module doc comment for the two transfers and [`plan_export_transfer`] /
/// [`final_picture_follow_up`] for how one is chosen and what a failed final picture
/// falls back to. `gpu`/`carry` are the caller's, exactly as for
/// [`render_accumulation`], so a multi-frame video keeps its calibration AND its
/// "this remote refused final pictures" memory across frames.
#[expect(
    clippy::too_many_arguments,
    reason = "the same per-image identity `render_accumulation` takes (scene, pose, \
              params, remote selection, GPU backend, local compute, carry, cancel, \
              progress) plus the output colour space; bundling would only move the count"
)]
pub fn render_image_rgba(
    scene: &SceneSnapshot,
    cam_yaw: f32,
    cam_pitch: f32,
    params: ExportParams,
    color_space: ColorSpace,
    remote: &RemoteSelection,
    gpu: &GpuBackend,
    local_compute: LocalComputeTarget,
    carry: &mut AccumulationCarry,
    cancel: &AtomicBool,
    mut report_progress: impl FnMut(ExportProgress),
) -> RenderedImage {
    let worker = remote.worker.as_ref();
    let plan = plan_export_transfer(
        remote.transfer,
        remote.compute_target,
        worker.is_some(),
        // Only a map that cannot be sent at all rules final pictures out up front; a
        // remote's own HDR support is checked against its WELCOME when the request is
        // dispatched (a remote without it fails the attempt, which falls back).
        remote_can_render(scene.env_map.as_ref(), true).is_err(),
        carry.final_picture_declined || worker.is_some_and(final_picture_refused),
    );
    match (plan, worker) {
        (TransferPlan::FinalPicture, Some(worker)) => {
            let outcome = final_picture(
                scene,
                (cam_yaw, cam_pitch),
                params,
                color_space,
                worker,
                cancel,
                &mut report_progress,
            );
            match final_picture_follow_up(outcome, remote.compute_target) {
                FinalPictureFollowUp::Use(rgba) => return RenderedImage::Rgba(rgba),
                FinalPictureFollowUp::Cancelled => return RenderedImage::Cancelled,
                FinalPictureFollowUp::Fail(message) => return RenderedImage::Failed(message),
                FinalPictureFollowUp::FallBackToFullData { note, remember } => {
                    if remember {
                        remember_final_picture_refused(worker);
                    }
                    carry.final_picture_declined = true;
                    carry.transfer_noted = true;
                    report_note(&mut report_progress, params.samples_per_pixel, note);
                }
            }
        }
        (TransferPlan::FullDataRefusedBefore, _) if !carry.transfer_noted => {
            carry.transfer_noted = true;
            report_note(
                &mut report_progress,
                params.samples_per_pixel,
                "The remote does not support final-picture transfer; using full data.".to_string(),
            );
        }
        _ => {}
    }
    full_data(
        scene,
        (cam_yaw, cam_pitch),
        params,
        color_space,
        remote,
        gpu,
        local_compute,
        carry,
        cancel,
        report_progress,
    )
}

/// The full-data transfer: [`render_accumulation`] then `tonemap_accumulation`.
#[expect(
    clippy::too_many_arguments,
    reason = "forwards `render_image_rgba`'s own arguments unchanged"
)]
fn full_data(
    scene: &SceneSnapshot,
    (cam_yaw, cam_pitch): (f32, f32),
    params: ExportParams,
    color_space: ColorSpace,
    remote: &RemoteSelection,
    gpu: &GpuBackend,
    local_compute: LocalComputeTarget,
    carry: &mut AccumulationCarry,
    cancel: &AtomicBool,
    report_progress: impl FnMut(ExportProgress),
) -> RenderedImage {
    match render_accumulation(
        scene,
        cam_yaw,
        cam_pitch,
        params,
        remote.compute_target,
        remote.worker.as_ref(),
        gpu,
        local_compute,
        carry,
        cancel,
        report_progress,
    ) {
        AccumulationOutcome::Cancelled => RenderedImage::Cancelled,
        AccumulationOutcome::Failed(message) => RenderedImage::Failed(message),
        AccumulationOutcome::Completed(Accumulation {
            accum,
            samples_per_pixel,
        }) => RenderedImage::Rgba(tonemap_accumulation(
            params.width,
            params.height,
            samples_per_pixel,
            &accum,
            color_space,
        )),
    }
}

/// The final-picture transfer: one `FinalImageRequest` for the whole sample budget,
/// progress from the remote's `PROGRESS` heartbeats.
fn final_picture(
    scene: &SceneSnapshot,
    (cam_yaw, cam_pitch): (f32, f32),
    params: ExportParams,
    color_space: ColorSpace,
    worker: &WorkerSettings,
    cancel: &AtomicBool,
    report_progress: &mut impl FnMut(ExportProgress),
) -> remote::FinalPictureOutcome {
    let ExportParams {
        width,
        height,
        samples_per_pixel,
        ..
    } = params;
    let state = remote::scene_state_from_snapshot(scene, width, height, cam_yaw, cam_pitch);
    run_final_image_request(
        worker,
        state,
        samples_per_pixel,
        color_space,
        cancel,
        |samples_done| {
            let done = samples_done.min(samples_per_pixel);
            report_progress(ExportProgress {
                fraction: done as f32 / samples_per_pixel.max(1) as f32,
                samples_done: done,
                samples_total: samples_per_pixel,
                preview: None,
                note: None,
            });
        },
    )
}

/// Surfaces a one-off transfer note through the ordinary progress channel.
fn report_note(report_progress: &mut impl FnMut(ExportProgress), total: u32, note: String) {
    report_progress(ExportProgress {
        fraction: 0.0,
        samples_done: 0,
        samples_total: total,
        preview: None,
        note: Some(note),
    });
}
