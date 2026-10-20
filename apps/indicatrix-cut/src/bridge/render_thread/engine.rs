//! Image-engine dispatch. The spectral path remains the existing implementation.
use super::gpu_backend::{BackendFrame, FrameOutputs, HybridPacing, ViewportGpu};
use crate::settings::model::LocalComputeTarget;
use indicatrix::{
    optics::{fluorescence::Fluorescence, raytracer::LightingPreset},
    renderer::{
        engine::EngineSpec,
        frame_scene::FrameScene,
        gpu_backend::{
            GpuAccumulate,
            whitted::{WhittedGpuBackend, WhittedGpuPrepared},
        },
        whitted::{WhittedDecline, WhittedPrep},
    },
};

/// Cached by content generation and effective preview dimensions, never by chunk.
#[derive(Default)]
pub(super) struct EngineAccumulator {
    prep: Option<PreparedRay>,
    gpu: Option<WhittedGpuBackend>,
}

struct PreparedRay {
    key: (u64, u32, u32),
    prep: WhittedPrep<'static>,
    planes: indicatrix::simd::PlanesSoA32,
    gpu: Option<WhittedGpuPrepared>,
    /// The upload was refused for this key; the CPU route serves it.
    gpu_declined: bool,
    /// First-hit guides depend only on pose and geometry, fixed for the key.
    guides: Option<RayGuides>,
}

/// Owned first-hit depth, normal and facet id for one prepared key.
struct RayGuides {
    depth: Vec<f32>,
    normal: Vec<glam::Vec3>,
    facet_id: Vec<i32>,
}

impl RayGuides {
    /// Ray-Traced needs no denoising. First-hit buffers still serve picking and
    /// depth diagnostics, so a GPU frame gets them from the shared CSG kernel.
    fn compute(frame: &BackendFrame<'_>, planes: &indicatrix::simd::PlanesSoA32) -> Self {
        use indicatrix::renderer::whitted::whitted_primary_hit;
        let scene = frame_scene(frame);
        let width = frame.width as usize;
        let count = width * frame.height as usize;
        let mut guides = Self {
            depth: Vec::with_capacity(count),
            normal: Vec::with_capacity(count),
            facet_id: Vec::with_capacity(count),
        };
        for pixel in 0..count {
            let ray = frame.camera.generate_ray(
                (pixel % width) as f32,
                (pixel / width) as f32,
                frame.width as f32,
                frame.height as f32,
                0.0,
                0.0,
            );
            let hit = whitted_primary_hit(&scene, frame.tools, planes, ray);
            guides.depth.push(hit.map_or(1.0e6, |value| value.t));
            guides
                .normal
                .push(hit.map_or(glam::Vec3::ZERO, |value| value.normal));
            guides.facet_id.push(hit.map_or(-1, |value| {
                i32::try_from(value.facet_idx).expect("facet index fits i32")
            }));
        }
        guides
    }

    fn copy_into(&self, outputs: &mut FrameOutputs<'_>) {
        let count = outputs.depth.len().min(self.depth.len());
        outputs.depth[..count].copy_from_slice(&self.depth[..count]);
        outputs.normal[..count].copy_from_slice(&self.normal[..count]);
        outputs.facet_id[..count].copy_from_slice(&self.facet_id[..count]);
    }
}

/// Protocol 28 workers and coordinators carry both image engines explicitly, so
/// this is always `true`. It is kept as the documented switch for the plan's D7
/// decision (remote Ray-Traced waits for the owner's performance check); the
/// call sites in `context::frame`, the preview wiring and the job conversion
/// consult it so the switch stays a one-line change.
#[must_use]
pub const fn remote_allowed(_engine: EngineSpec) -> bool {
    true
}

/// Refusals feed the existing banner before the frame claims a sample range.
pub(super) fn decline_message(
    engine: EngineSpec,
    preset: Option<LightingPreset>,
    fluorescence: Option<&Fluorescence>,
) -> Option<&'static str> {
    if !engine.is_ray() {
        None
    } else if fluorescence.is_some_and(|value| !value.is_empty()) {
        Some(WhittedDecline::Fluorescence.message())
    } else if preset.is_some_and(indicatrix::optics::LightingPreset::is_uv_lamp) {
        Some(WhittedDecline::UvLamp.message())
    } else {
        None
    }
}

pub(super) const fn frame_scene<'a>(frame: &BackendFrame<'a>) -> FrameScene<'a> {
    FrameScene {
        camera: frame.camera,
        width: frame.width,
        height: frame.height,
        planes: frame.planes,
        facet_finishes: frame.facet_finishes,
        material: frame.material,
        max_bounces: frame.max_bounces,
        environment: frame.environment,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "adapts the existing frame dispatcher without changing its spectral path"
)]
pub(super) fn accumulate_engine_samples(
    cache: &mut EngineAccumulator,
    engine: EngineSpec,
    generation: u64,
    shared_hdr: Option<&std::sync::Arc<indicatrix::renderer::env_map::EnvironmentMap>>,
    backend: &mut ViewportGpu,
    frame: &BackendFrame<'_>,
    outputs: &mut FrameOutputs<'_>,
    hybrid: &mut HybridPacing,
    compute: LocalComputeTarget,
) -> Result<(), WhittedDecline> {
    if !engine.is_ray() {
        cache.prep = None;
        super::gpu_backend::accumulate_frame_samples(backend, frame, outputs, hybrid, compute);
        return Ok(());
    }
    let key = (generation, frame.width, frame.height);
    if cache.prep.as_ref().is_none_or(|cached| cached.key != key) {
        let prep = WhittedPrep::new(&frame_scene(frame), frame.fluorescence, engine.ray)?
            .into_owned_with_hdr(shared_hdr.cloned());
        cache.prep = Some(PreparedRay {
            key,
            prep,
            planes: indicatrix::optics::raytracer::build_plane_soa(frame.planes),
            gpu: None,
            gpu_declined: false,
            guides: None,
        });
    }
    if let Some(prepared) = &mut cache.prep {
        if matches!(
            compute,
            LocalComputeTarget::Gpu | LocalComputeTarget::CpuGpu
        ) {
            let gpu = cache.gpu.get_or_insert_with(|| {
                #[cfg(feature = "ui-harness")]
                if crate::harness_support::viewport_rendering_disabled() {
                    return WhittedGpuBackend::disabled();
                }
                WhittedGpuBackend::acquire()
            });
            // A decline (no adapter, HDR over budget, too many tools) belongs to the
            // key, so it is remembered instead of repacking every frame.
            if prepared.gpu.is_none() && !prepared.gpu_declined {
                match gpu.prepare_cancellable(
                    &frame_scene(frame),
                    frame.tools,
                    &prepared.prep,
                    &|| false,
                ) {
                    Ok(upload) => prepared.gpu = Some(upload),
                    Err(_) => prepared.gpu_declined = !gpu.is_lost(),
                }
            }
            if prepared.gpu.as_ref().is_some_and(|upload| {
                gpu.try_accumulate_prepared_cancellable(
                    upload,
                    frame.sample_offset,
                    frame.spp,
                    outputs.accum,
                    &|| false,
                ) == GpuAccumulate::Done
            }) {
                let guides = prepared
                    .guides
                    .get_or_insert_with(|| RayGuides::compute(frame, &prepared.planes));
                guides.copy_into(outputs);
                return Ok(());
            }
            if prepared.gpu.is_some() {
                // Device replacement invalidates upload ownership. Reprepare next
                // call; the current call falls back transactionally to the same
                // CPU taps.
                prepared.gpu = None;
                prepared.gpu_declined = false;
            }
        }
        super::whitted_scanline::render_frame(frame, &prepared.prep, &prepared.planes, outputs);
    }
    Ok(())
}
