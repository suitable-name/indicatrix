//! The local picture export: a scene snapshot at export size, traced to a sample target
//! and encoded as a PNG with an ICC profile for the chosen color space.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use glam::Vec3;
use indicatrix::color::ColorSpace;
use indicatrix::renderer::env_map::EnvironmentMap;
use indicatrix::{
    render_setup::encode_png_with_icc,
    renderer::tonemap::{tonemap_to_rgba_with_threads, tonemap_wide_gamut_with_threads},
};
use indicatrix_web_core::{
    render::{Accumulator, ChunkAssignment, ChunkOutcome, ChunkPlanner, ChunkRange},
    scene::{CustomMaterialSpec, GeometrySceneSpec, OwnedScene},
};

/// The message of a cancelled export.
pub const CANCELLED: &str = "The export was cancelled.";

/// One traced chunk: its assignment, its sums (`None` when it was aborted) and its time in
/// milliseconds.
type Traced = (ChunkAssignment, Option<Vec<Vec3>>, f64);

/// A picture export that owns everything it needs: the scene snapshot, the size and the
/// sample target. Send it to a thread and call [`Self::run`].
pub struct ExportJob {
    engine: indicatrix::renderer::engine::EngineSpec,
    scene: GeometrySceneSpec,
    /// The traced material's physics recipe (it gives the scene its fluorescence).
    customs: Vec<CustomMaterialSpec>,
    backend: crate::settings::RenderBackend,
    remote: Option<crate::remote::rendering::RemoteConfig>,
    hdr: Option<Arc<EnvironmentMap>>,
    policy: Arc<crate::compute_policy::ComputePolicy>,
    /// The live render's GPU probe: the export reuses it, it never acquires the GPU again.
    probe: super::backend::GpuProbe,
    width: u32,
    height: u32,
    spp: u32,
    color_space: ColorSpace,
    workers: usize,
    /// Measured pixel-samples per millisecond of one thread, if the live render has run.
    rate_per_thread: Option<f64>,
}

impl ExportJob {
    /// A job over `scene` (already built at `width` x `height`).
    #[expect(
        clippy::too_many_arguments,
        reason = "the snapshot of one export: scene, its material recipe, lighting map, sample plan, thread plan, compute budget, backend choice, GPU probe and remote endpoint; a bag struct would only rename them"
    )]
    pub(super) const fn new(
        scene: GeometrySceneSpec,
        customs: Vec<CustomMaterialSpec>,
        engine: indicatrix::renderer::engine::EngineSpec,
        hdr: Option<Arc<EnvironmentMap>>,
        spp: u32,
        color_space: ColorSpace,
        workers: usize,
        rate_per_thread: Option<f64>,
        policy: Arc<crate::compute_policy::ComputePolicy>,
        backend: crate::settings::RenderBackend,
        probe: super::backend::GpuProbe,
        remote: Option<crate::remote::rendering::RemoteConfig>,
    ) -> Self {
        Self {
            engine,
            width: scene.scene.width,
            height: scene.scene.height,
            scene,
            customs,
            hdr,
            backend,
            remote,
            policy,
            probe,
            spp,
            color_space,
            workers,
            rate_per_thread,
        }
    }

    /// The picture's width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// The picture's height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Samples per pixel the export traces.
    #[must_use]
    pub const fn spp(&self) -> u32 {
        self.spp
    }

    /// A guess of how long [`Self::run`] takes, from the live render's measured speed;
    /// `None` when the live render has not traced anything yet.
    #[must_use]
    pub fn estimate_seconds(&self) -> Option<f32> {
        if self.remote.is_some() || self.engine.is_ray() {
            return None;
        }
        let rate = self.rate_per_thread.filter(|rate| *rate > 0.0)?;
        let work = f64::from(self.width) * f64::from(self.height) * f64::from(self.spp);
        let millis = work / (rate * self.workers.max(1) as f64);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the estimate is shown as f32 seconds; the value is far inside the f32 range"
        )]
        let seconds = (millis / 1000.0) as f32;
        Some(seconds)
    }

    /// Whether this export would run on a Remote endpoint (so a server job can be made).
    #[must_use]
    pub const fn is_remote(&self) -> bool {
        self.remote.is_some()
    }

    /// Hands the picture to the endpoint as a durable server job instead of waiting for it:
    /// the server stores the scene, renders it on its own schedule and keeps the PNG, so the
    /// phone may disconnect, sleep or be closed and fetch it later (`remote::jobs`). Blocks
    /// for the upload only.
    ///
    /// # Errors
    ///
    /// [`CANCELLED`] when `cancel` was set; a sentence when the export is not Remote, the
    /// endpoint refuses (no job service, color zones, size), or the transport fails.
    pub fn submit_server_job(
        &self,
        label: &str,
        cancel: &AtomicBool,
    ) -> Result<crate::remote::jobs::JobInfo, String> {
        let remote = self.remote.as_ref().ok_or_else(|| {
            "Server jobs need the Remote backend. Select Remote in the Render view first."
                .to_owned()
        })?;
        let cancelled = || cancel.load(Ordering::Acquire);
        crate::remote::jobs::submit(
            remote.platform.as_ref(),
            &remote.settings,
            &self.scene,
            &self.customs,
            self.engine,
            self.hdr.as_ref(),
            self.spp,
            self.color_space,
            label,
            &self.policy,
            &cancelled,
        )
        .map_err(|error| {
            if cancelled() {
                CANCELLED.to_owned()
            } else {
                String::from(error)
            }
        })
    }

    /// The Remote half of [`Self::run`]: the endpoint renders, the phone tone-maps and
    /// encodes (or, for `DisplayOnly`, only attaches the ICC profile).
    fn run_remote(
        &self,
        remote: &crate::remote::rendering::RemoteConfig,
        progress: &dyn Fn(f32),
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, String> {
        let cancelled = || cancel.load(Ordering::Acquire);
        // A session ended by the caller's cancel reads as the export's own message.
        let message = |error: crate::remote::rendering::RunError| {
            if cancelled() {
                CANCELLED.to_owned()
            } else {
                String::from(error)
            }
        };
        if remote.settings.transfer == crate::settings::RemoteTransfer::DisplayOnly {
            // A coordinator renders, averages and tone-maps the range itself and
            // answers with the finished PNG; the phone only attaches the ICC profile.
            progress(0.0);
            return crate::remote::final_image::run(
                remote.platform.as_ref(),
                &remote.settings,
                &self.scene,
                &self.customs,
                self.engine,
                self.hdr.as_ref(),
                self.spp,
                self.color_space,
                &self.policy,
                &cancelled,
                progress,
            )
            .map_err(message);
        }
        let output = crate::remote::rendering::run(
            remote.platform.as_ref(),
            &remote.settings,
            &self.scene,
            &self.customs,
            self.engine,
            self.hdr.as_ref(),
            self.spp,
            false,
            indicatrix_net::messages::RequestIntent::Batch,
            &self.policy,
            &cancelled,
            &|_| {},
            &|frame| progress((frame.samples as f32 / self.spp.max(1) as f32).min(1.0)),
        )
        .map_err(message)?;
        let _permit = self
            .policy
            .acquire(&cancelled)
            .ok_or_else(|| CANCELLED.to_owned())?;
        let scale = 1.0 / output.samples.max(1) as f32;
        let rgba = if self.color_space == ColorSpace::Srgb {
            tonemap_to_rgba_with_threads(&output.sum, scale, 1)
        } else {
            tonemap_wide_gamut_with_threads(&output.sum, scale, self.color_space, 1)
        };
        if cancelled() {
            return Err(CANCELLED.to_owned());
        }
        encode_png_with_icc(&rgba, self.width, self.height, self.color_space)
    }

    /// Traces the picture and encodes it. Blocks; run it on a thread of its own. It uses
    /// the render thread count and reports the fraction done (0.0 to 1.0) after every pass.
    /// Setting `cancel` ends it within a fraction of a second.
    ///
    /// # Errors
    ///
    /// [`CANCELLED`] when `cancel` was set; otherwise a sentence about what failed (a
    /// thread panicked, the PNG encoder failed).
    pub fn run(&self, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.to_owned());
        }
        if let Some(remote) = &self.remote {
            return self.run_remote(remote, progress, cancel);
        }
        // Material probing and scene allocation belong to this worker, never the sheet
        // callback. The GPU probe is the live render's own, so the export never acquires the
        // adapter and compiles the kernels a second time.
        let manager = super::backend::BackendManager::sharing(&self.probe);
        manager.select(self.backend);
        manager.begin(0);
        let (scene, backend) = {
            let _permit = self
                .policy
                .acquire(&|| cancel.load(Ordering::Relaxed))
                .ok_or_else(|| CANCELLED.to_owned())?;
            let mut scene = self
                .scene
                .build_with_customs(self.hdr.clone(), &self.customs)?;
            scene
                .set_engine(self.engine)
                .map_err(|reason| reason.message().to_owned())?;
            let backend = manager.prepare(&scene, 0)?;
            (scene, backend)
        };
        let partitions = if backend.is_gpu() {
            1
        } else {
            u32::try_from(self.workers).unwrap_or(1).max(1)
        };
        let pixels = self.width.saturating_mul(self.height);
        let mut planner = ChunkPlanner::new(partitions, pixels, self.spp);
        let mut acc = Accumulator::new(0, self.width, self.height, partitions);
        progress(0.0);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(CANCELLED.to_owned());
            }
            // All partitions of one pass, so the pass completes before the next starts.
            let round: Vec<ChunkAssignment> = (0..partitions)
                .filter_map(|partition| planner.next_chunk(partition, acc.completed_passes()))
                .collect();
            if round.is_empty() {
                break;
            }
            for (assignment, sums, millis) in
                self.trace_round(&manager, &backend, &scene, &round, cancel)?
            {
                let sums = sums.ok_or_else(|| {
                    if cancel.load(Ordering::Relaxed) {
                        CANCELLED.to_owned()
                    } else {
                        "The export stopped unexpectedly: the render backend failed.".to_owned()
                    }
                })?;
                planner.record_timing(assignment.first_pixel, assignment.spp, millis);
                let outcome = acc.add_chunk(
                    0,
                    assignment.first_pixel,
                    assignment.stride,
                    assignment.sample_offset,
                    assignment.spp,
                    sums,
                );
                if let ChunkOutcome::Rejected(reason) = outcome {
                    return Err(format!("The export stopped unexpectedly ({reason:?})."));
                }
            }
            progress((acc.sample_count() as f32 / self.spp.max(1) as f32).min(1.0));
        }
        let _permit = self
            .policy
            .acquire(&|| cancel.load(Ordering::Relaxed))
            .ok_or_else(|| CANCELLED.to_owned())?;
        let scale = 1.0 / acc.sample_count().max(1) as f32;
        let rgba = if self.color_space == ColorSpace::Srgb {
            tonemap_to_rgba_with_threads(acc.sum(), scale, 1)
        } else {
            tonemap_wide_gamut_with_threads(acc.sum(), scale, self.color_space, 1)
        };
        if cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.to_owned());
        }
        encode_png_with_icc(&rgba, self.width, self.height, self.color_space)
    }

    /// Traces one chunk per partition of `round` on scoped threads.
    fn trace_round(
        &self,
        manager: &super::backend::BackendManager,
        backend: &super::backend::TraceBackend,
        scene: &OwnedScene,
        round: &[ChunkAssignment],
        cancel: &AtomicBool,
    ) -> Result<Vec<Traced>, String> {
        std::thread::scope(|scope| {
            #[expect(
                clippy::needless_collect,
                reason = "every thread must be spawned before the first join, or the round runs serially"
            )]
            let handles: Vec<_> = round
                .iter()
                .map(|&assignment| {
                    scope.spawn(move || {
                        let range = ChunkRange {
                            first_pixel: assignment.first_pixel,
                            stride: assignment.stride,
                            sample_offset: assignment.sample_offset,
                            spp: assignment.spp,
                        };
                        let started = Instant::now();
                        let cancelled = || cancel.load(Ordering::Relaxed);
                        let mut sums = None;
                        // Set when the trace was stopped by a pause (not by a cancel): only
                        // that retries; a trace that fails on its own ends the export.
                        let paused = AtomicBool::new(false);
                        while !cancelled() {
                            let Some(permit) = self.policy.acquire(&cancelled) else {
                                break;
                            };
                            sums = manager.trace(
                                backend,
                                scene,
                                range,
                                cancel,
                                &|| {
                                    if cancelled() {
                                        return true;
                                    }
                                    let pause = self.policy.paused();
                                    if pause {
                                        paused.store(true, Ordering::Release);
                                    }
                                    pause
                                },
                                0,
                            );
                            drop(permit);
                            // A thermal/background pause retries the same uncommitted chunk.
                            if sums.is_some() || !paused.swap(false, Ordering::AcqRel) {
                                break;
                            }
                        }
                        (assignment, sums, started.elapsed().as_secs_f64() * 1000.0)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| "The export stopped unexpectedly.".to_owned())
                })
                .collect()
        })
    }
}
