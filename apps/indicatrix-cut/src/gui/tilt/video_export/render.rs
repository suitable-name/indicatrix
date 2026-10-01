//! Per-frame rendering for the tilt performance video: shares
//! `bridge::export_thread`'s per-image entry point (`render_image_rgba`) with the
//! still-image export, rather than a private CPU-only tracer. The whole video pauses the
//! live viewport for its own duration (`run`'s own doc comment), exactly as a still-image
//! export does, so it is safe to share the GPU adapter the same way -- the video is a
//! sequence of calls into the SAME core the still export uses for one image, not a
//! second, divergent renderer.
//!
//! [`VideoComputeConfig`] carries the SAME compute configuration a still-image export
//! reads: the app's local CPU/GPU/hybrid setting (`RenderContext::local_compute_target`,
//! a persisted general setting) and the configured remote endpoint
//! (`AppSettings::remote`) with the video section's own "Transfer" choice -- see
//! `mod::build_request`'s own doc comment for where these are read from. There is no
//! per-video "Compute" pill the way the still-image export dialog has one;
//! `compute_target` always mirrors that dialog's own default (`Both`), letting
//! `render_image_rgba`'s existing graceful remote-unavailable fallback decide, frame to
//! frame, whether the remote is actually reachable.
//!
//! # Final picture only
//!
//! With "Transfer: Final picture only" each frame is ONE `FinalImageRequest`: the remote
//! renders and tone-maps the frame, the viewer decodes the PNG to RGBA8, then overlays
//! and writes it exactly like a locally rendered frame. A remote that answers
//! `UNSUPPORTED_REQUEST` is remembered for the rest of the sweep (via the shared
//! `AccumulationCarry`) and every later frame uses full data.

use crate::{
    bridge::export_thread::{
        AccumulationCarry, ExportParams, ExportProgress, RemoteSelection, RenderedImage,
        SceneSnapshot, render_image_rgba,
    },
    settings::LocalComputeTarget,
};
use indicatrix::{color::ColorSpace, renderer::gpu_backend::GpuBackend};
use std::sync::atomic::AtomicBool;

/// The compute configuration for a WHOLE video, read once from the same settings
/// source a still-image export reads (see this module's own doc comment) and reused
/// unchanged for every frame of the sweep.
pub(super) struct VideoComputeConfig {
    /// Always `ComputeTarget::Both` (there is no per-video "Compute" pill, so this
    /// mirrors the still-image export dialog's own default), the configured remote
    /// endpoint snapshotted once for the whole video, and the section's "Transfer"
    /// choice.
    pub(super) remote: RemoteSelection,
    /// `RenderContext::local_compute_target` at the moment the video started -- the
    /// SAME persisted CPU/CPU+GPU/GPU setting the still-image export and the live
    /// viewport both use.
    pub(super) local_compute: LocalComputeTarget,
}

/// One frame's outcome from [`render_frame_rgba`] -- tone-mapped RGBA8 on the
/// `Rendered` path (whichever side tone-mapped it), since a video frame has no PNG/ICC
/// step of its own to defer that to (the overlay draws directly onto the RGBA buffer
/// next, in `run.rs`).
pub(super) enum FrameOutcome {
    /// Tone-mapped RGBA8 bytes, `width * height * 4` long.
    Rendered(Vec<u8>),
    /// `cancel` was observed before this frame finished.
    Cancelled,
    /// The frame's render failed outright (only reachable via `ComputeTarget::RemoteOnly`
    /// with no local fallback -- see `AccumulationOutcome::Failed`'s own doc comment).
    Failed(String),
}

/// Renders one frame at `(cam_yaw, cam_pitch)` through the SAME per-image entry point
/// the still-image export uses (`bridge::export_thread::render_image_rgba`: the local
/// CPU/GPU/hybrid + remote full-data core with `tonemap_accumulation`, or the remote's
/// own final picture) -- no separate video-only path. Everything else -- material,
/// geometry, light, exposure, bounce cap -- comes from `scene`; only the camera pose
/// changes frame to frame.
///
/// `gpu`/`carry` are owned by the WHOLE video (see `run::render_all_frames`), not
/// reacquired/recalibrated per frame -- `config`'s compute settings are likewise fixed
/// for the whole sweep.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of one frame's own render request \
              (scene/pose, dimensions/samples, colour space, the whole-video compute \
              config/GPU backend/calibration carry, cancellation, and progress \
              reporting) -- bundling them into a struct would just move the same \
              count into field access, not reduce it, matching \
              `bridge::export_thread::render_image_rgba`'s own identical shape"
)]
pub(super) fn render_frame_rgba(
    scene: &SceneSnapshot,
    width: u32,
    height: u32,
    samples_per_pixel: u32,
    cam_yaw: f32,
    cam_pitch: f32,
    color_space: ColorSpace,
    config: &VideoComputeConfig,
    gpu: &GpuBackend,
    carry: &mut AccumulationCarry,
    cancel: &AtomicBool,
    report_progress: impl FnMut(ExportProgress),
) -> FrameOutcome {
    let params = ExportParams {
        width,
        height,
        samples_per_pixel: samples_per_pixel.max(1),
        max_bounces: scene.max_bounces,
    };
    match render_image_rgba(
        scene,
        cam_yaw,
        cam_pitch,
        params,
        color_space,
        &config.remote,
        gpu,
        config.local_compute,
        carry,
        cancel,
        report_progress,
    ) {
        RenderedImage::Cancelled => FrameOutcome::Cancelled,
        RenderedImage::Failed(message) => FrameOutcome::Failed(message),
        RenderedImage::Rgba(rgba) => FrameOutcome::Rendered(rgba),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::render_thread::RenderContext;
    use std::sync::Mutex;

    /// A CPU-only, local-only smoke test: one frame at a tiny size/sample count must
    /// produce a full, opaque RGBA buffer of the right length, through the SAME
    /// `render_accumulation` core path the still-image export's CPU-only mode uses.
    /// `run.rs`'s own end-to-end test covers the whole multi-frame pipeline; this
    /// isolates just the render+tonemap step.
    #[test]
    fn render_frame_rgba_produces_a_full_opaque_buffer() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("Diamond resolves");
        let config = VideoComputeConfig {
            remote: RemoteSelection::local_only(),
            local_compute: LocalComputeTarget::Cpu,
        };
        let gpu = GpuBackend::disabled();
        let mut carry = AccumulationCarry::default();
        let cancel = AtomicBool::new(false);
        let outcome = render_frame_rgba(
            &scene,
            8,
            8,
            1,
            0.6,
            0.5,
            ColorSpace::Srgb,
            &config,
            &gpu,
            &mut carry,
            &cancel,
            |_| {},
        );
        let FrameOutcome::Rendered(rgba) = outcome else {
            panic!("expected a rendered frame");
        };
        assert_eq!(rgba.len(), 8 * 8 * 4);
        assert!(
            rgba.chunks(4).all(|px| px[3] == 255),
            "every pixel must be fully opaque"
        );
    }

    #[test]
    fn render_frame_rgba_accepts_wide_gamut_color_spaces_too() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("Diamond resolves");
        let config = VideoComputeConfig {
            remote: RemoteSelection::local_only(),
            local_compute: LocalComputeTarget::Cpu,
        };
        let gpu = GpuBackend::disabled();
        let mut carry = AccumulationCarry::default();
        let cancel = AtomicBool::new(false);
        let outcome = render_frame_rgba(
            &scene,
            4,
            4,
            1,
            0.0,
            1.4,
            ColorSpace::DisplayP3,
            &config,
            &gpu,
            &mut carry,
            &cancel,
            |_| {},
        );
        let FrameOutcome::Rendered(rgba) = outcome else {
            panic!("expected a rendered frame");
        };
        assert_eq!(rgba.len(), 4 * 4 * 4);
    }

    /// `AccumulationCarry` must actually be REUSED across frames, not silently reset --
    /// this is the whole point of threading it through from `run::render_all_frames`
    /// instead of building a fresh one per frame. `ComputeTarget::Both` with NO worker
    /// configured probes remote once, fails immediately (no network call needed -- see
    /// `probe_remote`'s `NoWorkerConfigured` case), and surfaces a "rendering locally
    /// only" note; a caller reusing the SAME carry across frames must see that note
    /// exactly ONCE, not once per frame, proving the probe itself was not repeated.
    #[test]
    fn a_no_worker_configured_note_is_surfaced_once_not_once_per_frame() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("Diamond resolves");
        let config = VideoComputeConfig {
            remote: RemoteSelection {
                compute_target: crate::bridge::export_thread::ComputeTarget::Both,
                ..RemoteSelection::local_only()
            },
            local_compute: LocalComputeTarget::Cpu,
        };
        let gpu = GpuBackend::disabled();
        let mut carry = AccumulationCarry::default();
        let cancel = AtomicBool::new(false);
        let mut notes = Vec::new();
        for (cam_yaw, cam_pitch) in [(0.1f32, 0.1f32), (0.2f32, 0.2f32), (0.3f32, 0.3f32)] {
            let notes = &mut notes;
            let outcome = render_frame_rgba(
                &scene,
                4,
                4,
                1,
                cam_yaw,
                cam_pitch,
                ColorSpace::Srgb,
                &config,
                &gpu,
                &mut carry,
                &cancel,
                |progress| {
                    if let Some(note) = progress.note {
                        notes.push(note);
                    }
                },
            );
            assert!(matches!(outcome, FrameOutcome::Rendered(_)));
        }
        assert_eq!(
            notes.len(),
            1,
            "the no-worker-configured note must surface once for the whole carry, not \
             once per frame: {notes:?}"
        );
    }

    /// "Final picture only" against a remote that cannot deliver (here: no certificate
    /// bundle, so it fails before touching the network) falls back to full data under
    /// `Both` -- every frame still renders -- and is not retried on later frames: the
    /// final-picture note and the full-data "rendering locally" note each appear once.
    #[test]
    fn a_failing_final_picture_falls_back_to_full_data_once_per_sweep() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("Diamond resolves");
        let config = VideoComputeConfig {
            remote: RemoteSelection {
                compute_target: crate::bridge::export_thread::ComputeTarget::Both,
                worker: Some(crate::settings::WorkerSettings {
                    address: "127.0.0.1:1".to_string(),
                    cert_dir: std::env::temp_dir()
                        .join("no-such-final-picture-bundle")
                        .display()
                        .to_string(),
                    ..crate::settings::WorkerSettings::default()
                }),
                transfer: crate::settings::ExportTransfer::FinalPicture,
                contribute_local: false,
            },
            local_compute: LocalComputeTarget::Cpu,
        };
        let gpu = GpuBackend::disabled();
        let mut carry = AccumulationCarry::default();
        let cancel = AtomicBool::new(false);
        let mut notes = Vec::new();
        for cam_yaw in [0.1_f32, 0.2, 0.3] {
            let notes = &mut notes;
            let outcome = render_frame_rgba(
                &scene,
                4,
                4,
                1,
                cam_yaw,
                0.2,
                ColorSpace::Srgb,
                &config,
                &gpu,
                &mut carry,
                &cancel,
                |progress| notes.extend(progress.note),
            );
            assert!(matches!(outcome, FrameOutcome::Rendered(ref rgba) if rgba.len() == 64));
        }
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains("final-picture"), "{notes:?}");
    }
}
