//! [`render_image_rgba`]: one finished RGBA8 image for the still export and the tilt
//! video, through whichever transfer applies:
//!
//! - **Full data**: [`render_accumulation`] (local CPU/GPU plus the remote's float
//!   radiance, merged), then `tonemap_accumulation` -- unchanged behaviour.
//! - **Final picture only**: one `FinalImageRequest`; the remote renders and tone-maps
//!   with the same `indicatrix::renderer::tonemap::tonemap_accumulation`, the viewer
//!   decodes the PNG to RGBA8. The local lanes do not take part -- UNLESS `contribution_allowed`
//!   says the viewer may contribute its own reserved-tail share (v16), in which case
//!   [`final_picture`] forks: the local lanes trace that tail concurrently while the
//!   remote handles the rest, and the sum is uploaded for the server to fold in before
//!   tone-mapping. Either way one finished PNG comes back.
//!
//! Either way the caller writes the RGBA through its own PNG writer, so ICC embedding
//! and file layout never depend on which side tone-mapped.

use super::{
    core::{render_accumulation, render_local_share},
    types::{Accumulation, AccumulationCarry, AccumulationOutcome},
};
use crate::{
    bridge::{
        export_thread::{
            ExportProgress,
            params::{ComputeTarget, ExportParams, RemoteSelection},
            remote::{
                self, FinalPictureFollowUp, LocalShare, TransferPlan, final_picture_follow_up,
                final_picture_refused, plan_export_transfer, record_split_rates,
                remember_final_picture_refused, run_final_image_request, split_rates, viewer_share,
            },
            scene_snapshot::SceneSnapshot,
            tonemap_png::tonemap_accumulation,
        },
        remote::{hdr_asset, remote_can_render},
    },
    settings::{LocalComputeTarget, WorkerSettings},
};
use indicatrix::{color::ColorSpace, renderer::gpu_backend::GpuBackend};
use indicatrix_net::SceneState;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::Instant,
};

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
                remote.compute_target,
                remote.contribute_local,
                worker,
                gpu,
                local_compute,
                &mut carry.hybrid_frac,
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

/// The final-picture transfer: one `FinalImageRequest`, progress from the remote's
/// `PROGRESS` heartbeats plus the coordinator's own forced periodic `PREVIEW` (see
/// `remote::final_image::run_final_image_request`'s doc comment) -- the same thumbnail
/// slot the full-data transfer's `PreviewThrottle` fills, so the export dialog shows a
/// live look either way.
///
/// # v16: the viewer's own contribution
///
/// When [`contribution_allowed`] says yes, this reserves a tail of the sample budget
/// for the viewer's own local lanes ([`viewer_share`]) and forks: one thread traces that
/// tail locally ([`render_local_share`]) while THIS thread dispatches the
/// `FinalImageRequest` for the server's own (smaller) share and uploads the local sum
/// the moment it's ready (`run_final_image_request`'s own `LocalShare` polling). Either
/// way the server tone-maps and returns one finished picture -- a declined/failed
/// contribution guard, or the local share simply not finishing in time, only ever costs
/// some fraction of the sample budget being rendered by the remote instead of the
/// viewer, never the export itself.
#[expect(
    clippy::too_many_arguments,
    reason = "the export's own identity (scene, pose, output params, the remote \
              selection's compute-target/contribute-local choice, GPU backend, local \
              compute choice, the carried hybrid-split estimate, cancel and progress) -- \
              the v16 fork needs every one of `render_accumulation`'s own local-side \
              inputs in addition to what this function already took"
)]
fn final_picture(
    scene: &SceneSnapshot,
    pose: (f32, f32),
    params: ExportParams,
    color_space: ColorSpace,
    compute_target: ComputeTarget,
    contribute_local: bool,
    worker: &WorkerSettings,
    gpu: &GpuBackend,
    local_compute: LocalComputeTarget,
    hybrid_frac: &mut Option<f64>,
    cancel: &AtomicBool,
    report_progress: &mut impl FnMut(ExportProgress),
) -> remote::FinalPictureOutcome {
    let (cam_yaw, cam_pitch) = pose;
    let ExportParams {
        width,
        height,
        samples_per_pixel,
        ..
    } = params;
    let state = remote::scene_state_from_snapshot(scene, width, height, cam_yaw, cam_pitch);
    let pixels = width.saturating_mul(height);

    let viewer = if contribution_allowed(
        scene,
        pose,
        width,
        height,
        &state,
        compute_target,
        contribute_local,
    ) {
        let (local_rate, remote_rate) = split_rates(worker, pixels);
        viewer_share(samples_per_pixel, local_rate, remote_rate)
    } else {
        0
    };

    if viewer == 0 {
        return run_final_image_request(
            worker,
            state,
            samples_per_pixel,
            color_space,
            cancel,
            None,
            |remote_done, local_done, preview| {
                report_combined_progress(
                    report_progress,
                    remote_done,
                    local_done,
                    samples_per_pixel,
                    preview,
                );
            },
        );
    }

    let outcome = final_picture_with_local_share(
        scene,
        pose,
        params,
        color_space,
        worker,
        state,
        gpu,
        local_compute,
        hybrid_frac,
        viewer,
        pixels,
        cancel,
        report_progress,
    );
    if let remote::FinalPictureOutcome::Completed {
        reclaimed_samples, ..
    } = &outcome
        && *reclaimed_samples > 0
    {
        report_note(
            report_progress,
            samples_per_pixel,
            format!(
                "The coordinator rendered {reclaimed_samples} of this machine's {viewer} \
                 samples itself (the local share was not ready in time)."
            ),
        );
    }
    outcome
}

/// Whether the viewer may contribute its own local samples to this final-picture export
/// (v16) -- every failure just means `viewer_share` is never even asked for and
/// `viewer_samples` stays `0`, falling back to today's pure remote-tone-mapped picture;
/// nothing here is fatal to the export.
///
/// - (a) `contribute_local` must be on, and `compute_target` must be `Both` --
///   `RemoteOnly` has no local lane to contribute from at all.
/// - (b) The local tracer's `SceneState` and the one actually sent to the remote must be
///   the EXACT same bytes -- comparing the wire-encoded form is simpler, and stays
///   correct automatically if `SceneState` ever grows a field, than comparing every
///   field by hand.
/// - (c) HDR: the local tracer must be able to render this environment at all
///   (`remote_can_render`, the same guard `render_image_rgba` already applies), AND the
///   request's own `environment` must be the exact map the local tracer would resolve
///   for this scene -- otherwise a stale/different local map would light the
///   viewer-rendered tail differently from a server rendering the request's own.
/// - (d) Material resolution is NOT checked here: a `&SceneSnapshot` exists only after
///   `SceneSnapshot::capture` returned `Ok`, which already refuses an unresolved
///   material (see that function's own doc comment) -- every `SceneSnapshot` this
///   function ever sees already satisfies it, by construction.
fn contribution_allowed(
    scene: &SceneSnapshot,
    (cam_yaw, cam_pitch): (f32, f32),
    width: u32,
    height: u32,
    state: &SceneState,
    compute_target: ComputeTarget,
    contribute_local: bool,
) -> bool {
    if !contribute_local || !matches!(compute_target, ComputeTarget::Both) {
        return false;
    }
    let local_state = remote::scene_state_from_snapshot(scene, width, height, cam_yaw, cam_pitch);
    let mut local_bytes = Vec::new();
    let mut remote_bytes = Vec::new();
    if indicatrix_net::messages::write_message(&mut local_bytes, &local_state).is_err()
        || indicatrix_net::messages::write_message(&mut remote_bytes, state).is_err()
        || local_bytes != remote_bytes
    {
        return false;
    }
    if remote_can_render(scene.env_map.as_ref(), true).is_err() {
        return false;
    }
    state.environment == hdr_asset::scene_environment(scene.env_map.as_ref())
}

/// The v16 fork: the viewer's own local lanes trace the reserved tail
/// (`samples_per_pixel - viewer`..`samples_per_pixel`) on a scoped thread while this one
/// dispatches the `FinalImageRequest` for the server's own share, uploading the local
/// sum the moment it's ready. Also records this attempt's measured local/remote rates
/// ([`record_split_rates`]) so the NEXT final-picture export against the same remote
/// starts [`viewer_share`] from a real split instead of the default guess.
#[expect(
    clippy::too_many_arguments,
    reason = "forwards `final_picture`'s own arguments, plus the already-built `state` \
              (so it isn't rebuilt), `viewer`'s share and the export's pixel count -- \
              splitting further would only move field access into yet another struct"
)]
fn final_picture_with_local_share(
    scene: &SceneSnapshot,
    pose: (f32, f32),
    params: ExportParams,
    color_space: ColorSpace,
    worker: &WorkerSettings,
    state: SceneState,
    gpu: &GpuBackend,
    local_compute: LocalComputeTarget,
    hybrid_frac: &mut Option<f64>,
    viewer: u32,
    pixels: u32,
    cancel: &AtomicBool,
    report_progress: &mut impl FnMut(ExportProgress),
) -> remote::FinalPictureOutcome {
    let samples_per_pixel = params.samples_per_pixel;
    let first = samples_per_pixel - viewer;
    let stop = AtomicBool::new(false);
    let done = AtomicU32::new(0);
    let (tx, rx) = mpsc::channel();
    let start = Instant::now();
    let mut frac = *hybrid_frac;
    // Set the instant `remote_done` (the SERVER's own progress) first reaches `first`
    // -- the server's whole share is finished -- timed from `start`; `None` if that
    // point is never observed (the export ends before/without a fresh `PROGRESS` at
    // or past it).
    let mut remote_rate: Option<f64> = None;

    let outcome = thread::scope(|s| {
        s.spawn(|| {
            if let Some(sum) = render_local_share(
                scene,
                pose,
                params,
                first,
                viewer,
                gpu,
                local_compute,
                &mut frac,
                &stop,
                &done,
            ) {
                let _ = tx.send((sum, start.elapsed()));
            }
        });
        let local_share = LocalShare {
            samples: viewer,
            done: &done,
            result: rx,
            stop: &stop,
        };
        let outcome = run_final_image_request(
            worker,
            state,
            samples_per_pixel,
            color_space,
            cancel,
            Some(&local_share),
            |remote_done, local_done, preview| {
                if remote_rate.is_none() && remote_done >= first {
                    remote_rate = Some(f64::from(first) / start.elapsed().as_secs_f64().max(1e-9));
                }
                report_combined_progress(
                    report_progress,
                    remote_done,
                    local_done,
                    samples_per_pixel,
                    preview,
                );
            },
        );
        stop.store(true, Ordering::Relaxed);
        outcome
    });

    *hybrid_frac = frac;
    let local_done = done.load(Ordering::Relaxed);
    let local_rate =
        (local_done > 0).then(|| f64::from(local_done) / start.elapsed().as_secs_f64().max(1e-9));
    record_split_rates(worker, pixels, local_rate, remote_rate);

    outcome
}

/// Combines the remote lane's own `PROGRESS`/`PREVIEW`-reported count with a v16 local
/// share's running total into ONE progress tick. `min` covers the very end of a request
/// with a local share in play: the coordinator's own `PROGRESS` only includes the folded
/// -in contribution once it has actually merged, so the raw sum can transiently exceed
/// `samples` right at completion. Reduces to `remote_done` alone (already `<= samples`)
/// whenever there is no local share (`local_done` is always `0` then).
fn report_combined_progress(
    report_progress: &mut impl FnMut(ExportProgress),
    remote_done: u32,
    local_done: u32,
    samples: u32,
    preview: Option<SharedPixelBuffer<Rgba8Pixel>>,
) {
    let done = (remote_done + local_done).min(samples);
    report_progress(ExportProgress {
        fraction: done as f32 / samples.max(1) as f32,
        samples_done: done,
        samples_total: samples,
        preview,
        note: None,
    });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::render_thread::RenderContext;
    use std::sync::Mutex;

    fn scene() -> SceneSnapshot {
        SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("Diamond resolves")
    }

    /// (a) `RemoteOnly` and the settings toggle being off must each refuse outright
    /// (no local lane to contribute from, or the cutter turned it off); (c) a request
    /// `SceneState` whose `environment` disagrees with what the local tracer resolves
    /// for this scene (a stale/foreign HDR content hash) must also refuse -- a viewer-
    /// rendered tail lit by the wrong map would silently corrupt the picture. A plain,
    /// matching, HDR-free scene under `Both` with the toggle on is the one case that's
    /// actually allowed.
    #[test]
    fn contribution_guard_refuses_remote_only_toggle_off_and_mismatched_hdr_hash() {
        let scene = scene();
        let (width, height) = (8, 6);
        let pose = (0.3_f32, 0.2_f32);
        let state = remote::scene_state_from_snapshot(&scene, width, height, pose.0, pose.1);

        assert!(
            !contribution_allowed(
                &scene,
                pose,
                width,
                height,
                &state,
                ComputeTarget::RemoteOnly,
                true
            ),
            "RemoteOnly has no local lane to contribute from"
        );
        assert!(
            !contribution_allowed(
                &scene,
                pose,
                width,
                height,
                &state,
                ComputeTarget::Both,
                false
            ),
            "the settings toggle being off must refuse"
        );
        assert!(
            contribution_allowed(
                &scene,
                pose,
                width,
                height,
                &state,
                ComputeTarget::Both,
                true
            ),
            "Both + the toggle on + a matching, HDR-free scene must be allowed"
        );

        let mismatched_hdr = SceneState {
            environment: indicatrix_net::scene::SceneEnvironment::Hdr(
                indicatrix_net::scene::HdrEnvironment {
                    content_hash: [7; 32],
                    width: 4,
                    height: 2,
                },
            ),
            ..state
        };
        assert!(
            !contribution_allowed(
                &scene,
                pose,
                width,
                height,
                &mismatched_hdr,
                ComputeTarget::Both,
                true
            ),
            "a request environment the local tracer does not itself resolve to must refuse"
        );
    }
}
