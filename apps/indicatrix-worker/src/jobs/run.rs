//! The job executor: one thread renders the queued durable jobs one at a time on this
//! server's own render lane (`serve --render`).
//!
//! A job is traced in slices of a few million pixel-samples, summing the slices (sample
//! ranges are disjoint and additive, so the picture is the same as a one-shot trace of the
//! whole range). Between slices the executor reports progress and looks at the cancel flag,
//! so a cancelled job stops within one slice. The finished sum is tone-mapped and encoded
//! exactly as the desktop's own still export does it (`tonemap_accumulation`, then
//! `encode_png_with_icc` for the job's color space), so a server-rendered job and a local
//! export of the same scene are the same picture.
//!
//! The executor never trusts the world to have stayed as the submission left it: the job
//! file is re-read and re-validated, and an HDR map that left the asset cache since the
//! submission fails the job with a sentence instead of lighting it with the studio rig.

use super::store::{JobStore, unix_now};
use crate::{
    assets::{self, AssetCache},
    render_core,
};
use glam::Vec3;
use indicatrix::{
    color::ColorSpace,
    render_setup::encode_png_with_icc,
    renderer::{gpu_backend::GpuBackend, tonemap::tonemap_accumulation},
};
use indicatrix_net::scene::HdrEnvironment;
use indicatrix_render_jobs::{JobColorSpace, JobKind, RenderJobFile};
use std::{
    panic::AssertUnwindSafe,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// How long the executor waits for work before it does its housekeeping.
const HOUSEKEEPING: Duration = Duration::from_secs(30);
/// Pixel-samples per slice: a few seconds of work on a slow machine.
const SLICE_PIXEL_SAMPLES: u64 = 4_000_000;
/// The most samples per pixel in one slice.
const MAX_SLICE_SAMPLES: u64 = 64;

/// The render lane the executor traces on.
#[derive(Clone)]
pub struct RenderLane {
    /// The process's GPU backend (disabled when the server renders on the CPU only).
    pub gpu: Arc<GpuBackend>,
    /// CPU tracer threads (`0` = all cores).
    pub threads: usize,
    /// Small-kernel GPU intent, independent of spectral adapter admission.
    pub prefer_gpu: bool,
}

/// How one job ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Finished: the PNG.
    Done(Vec<u8>),
    /// The owner cancelled it.
    Cancelled,
    /// It failed; the sentence says why.
    Failed(String),
}

/// Samples per pixel traced per slice for a picture of `width` x `height`.
fn slice_samples(width: u32, height: u32) -> u32 {
    let pixels = (u64::from(width) * u64::from(height)).max(1);
    u32::try_from((SLICE_PIXEL_SAMPLES / pixels).clamp(1, MAX_SLICE_SAMPLES)).unwrap_or(1)
}

const fn color_space(wire: JobColorSpace) -> ColorSpace {
    match wire {
        JobColorSpace::Srgb => ColorSpace::Srgb,
        JobColorSpace::DisplayP3 => ColorSpace::DisplayP3,
        JobColorSpace::Rec2020 => ColorSpace::Rec2020,
    }
}

/// The decoded map a job's scene names, from the registry or decoded from the asset cache.
fn resolve_hdr(
    cache: Option<&AssetCache>,
    environment: &HdrEnvironment,
) -> Result<Arc<indicatrix::renderer::env_map::EnvironmentMap>, String> {
    if let Some(map) = assets::lookup(&environment.content_hash) {
        return Ok(map);
    }
    let bytes = cache
        .and_then(|cache| cache.get(&environment.content_hash))
        .ok_or_else(|| {
            "The HDR map this job was lit with is no longer on the server; submit the job again."
                .to_owned()
        })?;
    assets::decode_and_register(environment, &bytes)
}

/// Renders `job` to a PNG. `progress` receives the samples per pixel traced so far.
pub fn render_job(
    job: &RenderJobFile,
    lane: &RenderLane,
    cache: Option<&AssetCache>,
    cancel: &AtomicBool,
    progress: &dyn Fn(u32),
) -> Outcome {
    let JobKind::Still(still) = &job.kind else {
        return Outcome::Failed("This server renders still pictures only.".to_owned());
    };
    let mut scene = job.scene.clone();
    scene.engine = job.engine;
    if scene.engine.is_ray()
        && let Err(note) = crate::validate::validate_request(&scene, 0, still.samples_per_pixel)
    {
        return Outcome::Failed(note);
    }
    let scene = &scene;
    // Held for the whole render: the tracer finds the decoded map in the registry.
    let _pin = match scene.hdr() {
        None => None,
        Some(environment) => match resolve_hdr(cache, environment) {
            Ok(map) => Some(map),
            Err(note) => return Outcome::Failed(note),
        },
    };
    let (width, height) = (scene.width, scene.height);
    let mut sum = vec![Vec3::ZERO; width as usize * height as usize];
    let step = slice_samples(width, height);
    let mut done = 0_u32;
    while done < still.samples_per_pixel {
        if cancel.load(Ordering::Acquire) {
            return Outcome::Cancelled;
        }
        let count = step.min(still.samples_per_pixel - done);
        let traced = std::panic::catch_unwind(AssertUnwindSafe(|| {
            if scene.engine.is_ray() {
                let mut out = vec![Vec3::ZERO; width as usize * height as usize];
                return render_core::whitted::trace(
                    scene,
                    done,
                    count,
                    lane.threads,
                    lane.prefer_gpu,
                    cancel,
                    &mut out,
                )
                .then_some(out);
            }
            render_core::trace_samples_with_gpu_cancellable(
                &lane.gpu,
                scene,
                done,
                count,
                lane.threads,
                cancel,
            )
        }));
        match traced {
            Err(_) => {
                return Outcome::Failed("Internal error while tracing this job.".to_owned());
            }
            Ok(None) if !scene.engine.is_ray() || cancel.load(Ordering::Acquire) => {
                return Outcome::Cancelled;
            }
            Ok(None) => {
                return Outcome::Failed(
                    "The renderer could not trace this job's complete sample range.".to_owned(),
                );
            }
            Ok(Some(part)) => {
                for (total, piece) in sum.iter_mut().zip(&part) {
                    *total += *piece;
                }
            }
        }
        done += count;
        progress(done);
    }
    let space = color_space(still.color_space);
    let rgba = tonemap_accumulation(width, height, still.samples_per_pixel, &sum, space);
    match encode_png_with_icc(&rgba, width, height, space) {
        Ok(png) => Outcome::Done(png),
        Err(error) => Outcome::Failed(format!("The picture could not be encoded: {error}")),
    }
}

/// Renders one claimed job and records how it ended.
fn run_one(
    store: &JobStore,
    lane: &RenderLane,
    cache: Option<&AssetCache>,
    id: &indicatrix_net::messages::JobId,
    cancel: &AtomicBool,
) {
    let job = match store.load_job(id) {
        Ok(job) => job,
        Err(note) => {
            store.finish_failed(id, &note, unix_now());
            return;
        }
    };
    let outcome = render_job(&job, lane, cache, cancel, &|done| {
        store.set_progress(id, done);
    });
    let now = unix_now();
    match outcome {
        Outcome::Done(png) => {
            if let Err(error) = store.finish_done(id, &png, now) {
                store.finish_failed(id, &format!("{error:?}"), now);
            }
        }
        Outcome::Cancelled => store.finish_cancelled(id, now),
        Outcome::Failed(note) => store.finish_failed(id, &note, now),
    }
}

/// Runs the executor until `stop` is raised. Blocks; the service runs it on a thread.
pub fn run(store: &JobStore, lane: &RenderLane, cache: Option<&AssetCache>, stop: &AtomicBool) {
    while !stop.load(Ordering::Acquire) {
        if let Some(claim) = store.claim_next(stop, HOUSEKEEPING) {
            run_one(store, lane, cache, &claim.id, &claim.cancel);
        } else {
            let removed = store.purge_expired(unix_now());
            if removed > 0 {
                tracing::info!("jobs: deleted {removed} expired job(s)");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };
    use indicatrix_net::{SceneState, scene::SceneEnvironment};
    use indicatrix_render_jobs::{ComputeChoice, StillJob, TransferChoice};

    fn scene() -> SceneState {
        SceneState {
            engine: indicatrix::renderer::engine::EngineSpec::default(),
            width: 16,
            height: 16,
            yaw: 0.4,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
            backdrop: 0.0,
            environment: SceneEnvironment::Studio,
            surface_glare: 1.0,
            tools: Vec::new(),
            fluorescence: indicatrix::optics::fluorescence::Fluorescence::default(),
            head_shadow_deg: 16.0,
            frosted_planes: Vec::new(),
        }
    }

    fn job(samples: u32, space: JobColorSpace) -> RenderJobFile {
        RenderJobFile {
            engine: indicatrix::renderer::engine::EngineSpec::default(),
            format: 1,
            token: "00".repeat(16),
            label: "test".to_owned(),
            created_at: 0,
            design: indicatrix_render_jobs::job::DesignInfo::default(),
            scene: scene(),
            scene_backdrop_model: indicatrix_render_jobs::job::BACKDROP_MODEL_TRAY,
            hdr: None,
            compute: indicatrix_render_jobs::job::JobCompute {
                target: ComputeChoice::Local,
                transfer: TransferChoice::FinalPicture,
                contribute_local: false,
            },
            output: "x.png".to_owned(),
            kind: JobKind::Still(StillJob {
                samples_per_pixel: samples,
                color_space: space,
                preset_label: String::new(),
            }),
            optical_diagnostics: None,
        }
    }

    fn lane() -> RenderLane {
        RenderLane {
            prefer_gpu: false,
            gpu: Arc::new(GpuBackend::disabled()),
            threads: 2,
        }
    }

    #[test]
    fn a_job_renders_the_same_png_as_a_one_shot_trace() {
        let job = job(5, JobColorSpace::Srgb);
        let cancel = AtomicBool::new(false);
        let steps = std::sync::Mutex::new(Vec::new());
        let Outcome::Done(png) = render_job(&job, &lane(), None, &cancel, &|done| {
            steps.lock().unwrap().push(done);
        }) else {
            panic!("the job renders");
        };
        // 16 x 16 pixels: one slice covers every sample.
        assert_eq!(steps.into_inner().unwrap(), vec![5]);
        // The PNG is the tone-mapped one-shot trace of the same range.
        let sums =
            render_core::trace_samples_with_gpu(&GpuBackend::disabled(), &job.scene, 0, 5, 2);
        let rgba = tonemap_accumulation(16, 16, 5, &sums, ColorSpace::Srgb);
        assert_eq!(
            png,
            encode_png_with_icc(&rgba, 16, 16, ColorSpace::Srgb).unwrap()
        );
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (16, 16));
    }

    #[test]
    fn a_ray_job_uses_the_authoritative_job_engine_instead_of_the_scene_copy() {
        let mut job = job(4, JobColorSpace::Srgb);
        job.engine.engine = indicatrix::renderer::engine::RenderEngine::RayTraced;
        job.engine.ray.aa_taps = 4;
        job.format = 2;
        // The scene retains its legacy Path-Traced default on purpose.
        let cancel = AtomicBool::new(false);
        let Outcome::Done(png) = render_job(&job, &lane(), None, &cancel, &|_| {}) else {
            panic!("the Ray-Traced job renders");
        };
        let mut scene = job.scene.clone();
        scene.engine = job.engine;
        let sums = render_core::trace_samples(&scene, 0, 4, 1);
        let rgba = tonemap_accumulation(16, 16, 4, &sums, ColorSpace::Srgb);
        assert_eq!(
            png,
            encode_png_with_icc(&rgba, 16, 16, ColorSpace::Srgb).unwrap()
        );
    }

    #[test]
    fn a_cancelled_job_stops_before_tracing() {
        let cancel = AtomicBool::new(true);
        assert_eq!(
            render_job(
                &job(4, JobColorSpace::Srgb),
                &lane(),
                None,
                &cancel,
                &|_| {}
            ),
            Outcome::Cancelled
        );
    }

    #[test]
    fn a_job_whose_map_left_the_cache_fails_with_a_sentence_never_a_studio_picture() {
        let mut job = job(2, JobColorSpace::Srgb);
        job.scene.environment = SceneEnvironment::Hdr(HdrEnvironment {
            content_hash: [0xCC; 32],
            width: 8,
            height: 4,
        });
        let cancel = AtomicBool::new(false);
        let Outcome::Failed(note) = render_job(&job, &lane(), None, &cancel, &|_| {}) else {
            panic!("a missing map fails the job");
        };
        assert!(note.contains("no longer on the server"), "{note}");
    }

    #[test]
    fn slices_cover_small_pictures_in_one_step_and_large_ones_in_single_samples() {
        assert_eq!(slice_samples(16, 16), 64);
        assert_eq!(slice_samples(2000, 2000), 1);
        assert_eq!(slice_samples(1000, 1000), 4);
        assert_eq!(slice_samples(4096, 4096), 1);
    }
}
