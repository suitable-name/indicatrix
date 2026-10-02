//! Denoised `DISPLAY_FRAME`s: a coordinator streams tone-mapped, DENOISED
//! 8-bit frames, with the denoiser's guides from its own primary-ray prepass.
//!
//! # One denoiser thread per `DisplayOnly` request
//!
//! [`DisplayDenoiser::spawn`] starts a thread that first computes the request's guide
//! buffers ([`generate_guide_buffers_cancellable`]) from the request's `SceneState` --
//! pose (`yaw`/`pitch`/`distance`, the viewer's fixed 42° field of view) and planes
//! only, so they are computed ONCE per request and reused for every frame of it -- and
//! then turns each submitted merged sum into a picture with the viewer's own
//! [`denoise_and_tonemap_frame`]. Invariant 5 holds by construction: the denoise runs
//! once, on the merged sum (the emitter's running total, or the job's deterministic
//! merge for the final frame), never per lane.
//!
//! # Never faster than it can denoise
//!
//! At most one denoise is in flight. A cadence tick hands over the current running total
//! only when the previous picture is finished; a tick that finds a denoise still running
//! sends no `DISPLAY_FRAME` (just its `PROGRESS`) and remembers that newer samples are
//! waiting, so the next free tick picks up the newest sum. The emitter thread itself
//! never denoises during a tick, so its `CANCEL`/`PING` polling and heartbeats are never
//! stalled by a multi-hundred-millisecond 4K denoise. Only the final frame is waited for
//! (with `PROGRESS` heartbeats every [`crate::stream_emit::HEARTBEAT_INTERVAL`]).
//!
//! # CPU, never the GPU
//!
//! The guide prepass (`intersect_polyhedron`) and the À-Trous denoiser are both CPU
//! code (`std::thread::scope` loops); neither submits a GPU program, so the
//! one-GPU-program-at-a-time `GpuBackend` FIFO is untouched and a GPU own lane keeps
//! tracing while a frame denoises. The cost is CPU time beside the own lane's CPU tracer
//! threads (the viewer's own live view makes the same trade).
//!
//! # Cancellation
//!
//! Dropping the [`DisplayDenoiser`] (the request ended, was cancelled or superseded)
//! raises its cancel flag and closes its job channel: a guide prepass stops within a row,
//! an idle thread exits at once, and a denoise already running finishes and is dropped
//! (the À-Trous pass has no cancel point). The thread is never joined, so a superseded
//! live-view request never waits for it.
//!
//! If the thread cannot be started or dies, [`DisplayUpdate::Plain`] asks the emitter for
//! the plain tone-mapped running average instead -- a display frame is never withheld.

use glam::Vec3;
use indicatrix::{
    geometry::plane::GpuFacetPlane,
    optics::raytracer::{Camera, DEFAULT_FOV_DEG},
    renderer::{
        denoise::AtrousDenoiser,
        frame_denoise::{DenoiseScratch, FirstHitSnapshot, denoise_and_tonemap_frame},
        guide_pass::generate_guide_buffers_cancellable,
    },
};
use indicatrix_net::SceneState;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError, TryRecvError},
    },
    thread,
    time::Duration,
};

/// A finished display picture: the RGBA8 of a merged sum of `samples` samples.
pub(in crate::stream_emit) struct DisplayPicture {
    /// Samples per pixel in the sum the picture was made from.
    pub(in crate::stream_emit) samples: u32,
    /// `width * height * 4` bytes.
    pub(in crate::stream_emit) rgba: Vec<u8>,
}

/// What one cadence tick should send (see [`DisplayDenoiser::tick`]).
pub(in crate::stream_emit) enum DisplayUpdate {
    /// A denoised picture finished since the last tick.
    Picture(DisplayPicture),
    /// The denoiser is unavailable and new samples landed: send the plain tone-mapped
    /// running average (the pre-denoise behaviour).
    Plain,
    /// Nothing to send this tick.
    Nothing,
}

/// One merged sum handed to the denoise thread.
struct Job {
    samples: u32,
    sum: Vec<Vec3>,
}

/// The pose and geometry a request's guides depend on (nothing else, see the module doc
/// comment).
struct GuideInputs {
    width: u32,
    height: u32,
    yaw: f32,
    pitch: f32,
    distance: f32,
    planes: Vec<GpuFacetPlane>,
}

/// One `DisplayOnly` request's denoise thread and its hand-over state (see the module
/// doc comment).
pub(in crate::stream_emit) struct DisplayDenoiser {
    jobs: mpsc::Sender<Job>,
    pictures: mpsc::Receiver<DisplayPicture>,
    cancel: Arc<AtomicBool>,
    /// A job was submitted and its picture not yet received.
    in_flight: bool,
    /// Samples landed since the last submission.
    dirty: bool,
    /// The thread could not start or has exited.
    gone: bool,
}

impl DisplayDenoiser {
    /// Starts the denoise thread for `scene` (it computes the guides first).
    pub(in crate::stream_emit) fn spawn(scene: &SceneState) -> Self {
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (picture_tx, pictures) = mpsc::channel::<DisplayPicture>();
        let cancel = Arc::new(AtomicBool::new(false));
        let inputs = GuideInputs {
            width: scene.width,
            height: scene.height,
            yaw: scene.yaw,
            pitch: scene.pitch,
            distance: scene.distance,
            planes: scene.planes.clone(),
        };
        let flag = Arc::clone(&cancel);
        let spawned = thread::Builder::new()
            .name("display-denoise".to_string())
            .spawn(move || run(&inputs, &flag, &job_rx, &picture_tx));
        if let Err(e) = &spawned {
            tracing::warn!(
                "display frames go out un-denoised: could not start the denoise thread: {e}"
            );
        }
        Self {
            jobs,
            pictures,
            cancel,
            in_flight: false,
            dirty: false,
            gone: spawned.is_err(),
        }
    }

    /// One cadence tick: takes a finished picture (if any), then -- if samples landed
    /// since the last submission (`fresh` now, or remembered from a busy tick) and no
    /// denoise is in flight -- submits `total` (`samples_done` samples) for the next one.
    pub(in crate::stream_emit) fn tick(
        &mut self,
        fresh: bool,
        samples_done: u32,
        total: &[Vec3],
    ) -> DisplayUpdate {
        self.dirty |= fresh;
        let finished = self.try_picture();
        if self.gone {
            let plain = self.dirty && samples_done > 0;
            self.dirty = false;
            return if plain {
                DisplayUpdate::Plain
            } else {
                DisplayUpdate::Nothing
            };
        }
        if self.dirty && samples_done > 0 && self.submit(samples_done, total) {
            self.dirty = false;
        }
        finished.map_or(DisplayUpdate::Nothing, DisplayUpdate::Picture)
    }

    /// The final picture: drops whatever is still in flight, submits `total` (`samples`
    /// samples) and waits for it, calling `heartbeat` every `heartbeat_every` meanwhile.
    /// `Ok(None)` if the thread is gone (the caller sends the plain tone-map).
    ///
    /// # Errors
    ///
    /// Whatever `heartbeat` returns (a transport failure).
    pub(in crate::stream_emit) fn finish<E>(
        &mut self,
        samples: u32,
        total: &[Vec3],
        heartbeat_every: Duration,
        mut heartbeat: impl FnMut() -> Result<(), E>,
    ) -> Result<Option<Vec<u8>>, E> {
        while self.in_flight {
            if self.wait(heartbeat_every, &mut heartbeat)?.is_none() {
                return Ok(None);
            }
        }
        if samples == 0 || !self.submit(samples, total) {
            return Ok(None);
        }
        Ok(self
            .wait(heartbeat_every, &mut heartbeat)?
            .map(|picture| picture.rgba))
    }

    /// A finished picture, without blocking.
    fn try_picture(&mut self) -> Option<DisplayPicture> {
        if self.gone {
            return None;
        }
        match self.pictures.try_recv() {
            Ok(picture) => {
                self.in_flight = false;
                Some(picture)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.gone = true;
                self.in_flight = false;
                None
            }
        }
    }

    /// Blocks for the in-flight picture, heartbeating every `every`; `None` once the
    /// thread is gone.
    fn wait<E>(
        &mut self,
        every: Duration,
        heartbeat: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<Option<DisplayPicture>, E> {
        loop {
            match self.pictures.recv_timeout(every) {
                Ok(picture) => {
                    self.in_flight = false;
                    return Ok(Some(picture));
                }
                Err(RecvTimeoutError::Timeout) => heartbeat()?,
                Err(RecvTimeoutError::Disconnected) => {
                    self.gone = true;
                    self.in_flight = false;
                    return Ok(None);
                }
            }
        }
    }

    /// Hands a copy of `total` to the thread unless a denoise is in flight; whether it did.
    fn submit(&mut self, samples: u32, total: &[Vec3]) -> bool {
        if self.gone || self.in_flight {
            return false;
        }
        let job = Job {
            samples,
            sum: total.to_vec(),
        };
        if self.jobs.send(job).is_err() {
            self.gone = true;
            return false;
        }
        self.in_flight = true;
        true
    }
}

impl Drop for DisplayDenoiser {
    fn drop(&mut self) {
        // The job sender drops right after this, so an idle thread's `recv` ends too.
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The denoise thread: guides once, then one picture per job until the channel closes
/// or `cancel` is raised.
fn run(
    inputs: &GuideInputs,
    cancel: &AtomicBool,
    jobs: &mpsc::Receiver<Job>,
    pictures: &mpsc::Sender<DisplayPicture>,
) {
    let camera = Camera::new(inputs.yaw, inputs.pitch, inputs.distance, DEFAULT_FOV_DEG);
    let Some(guides) = generate_guide_buffers_cancellable(
        inputs.width,
        inputs.height,
        &camera,
        &inputs.planes,
        cancel,
    ) else {
        return;
    };
    let mut denoiser = AtrousDenoiser::new();
    let (mut avg_color_buf, mut filtered_buf) = (Vec::new(), Vec::new());
    while let Ok(job) = jobs.recv() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let rgba = denoise_and_tonemap_frame(
            FirstHitSnapshot {
                width: inputs.width,
                height: inputs.height,
                current_sample_count: job.samples,
                accum_buffer: &job.sum,
                first_hit_depth: &guides.depth,
                first_hit_normal: &guides.normal,
                first_hit_facet_id: &guides.facet_id,
            },
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg_color_buf,
                filtered_buf: &mut filtered_buf,
            },
        );
        let picture = DisplayPicture {
            samples: job.samples,
            rgba,
        };
        if pictures.send(picture).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
        renderer::guide_pass::generate_guide_buffers,
    };
    use std::convert::Infallible;

    fn scene() -> SceneState {
        SceneState {
            width: 12,
            height: 9,
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
            environment: indicatrix_net::scene::SceneEnvironment::Studio,
            surface_glare: 1.0,
        }
    }

    fn sum(len: usize, scale: f32) -> Vec<Vec3> {
        (0..len)
            .map(|i| Vec3::new((i % 7) as f32, (i % 5) as f32, (i % 3) as f32) * scale)
            .collect()
    }

    /// The viewer's own pipeline on the same scene: `generate_guide_buffers` at the
    /// viewer's fov, then `denoise_and_tonemap_frame`.
    fn reference(scene: &SceneState, samples: u32, total: &[Vec3]) -> Vec<u8> {
        let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG);
        let guides = generate_guide_buffers(scene.width, scene.height, &camera, &scene.planes);
        denoise_and_tonemap_frame(
            FirstHitSnapshot {
                width: scene.width,
                height: scene.height,
                current_sample_count: samples,
                accum_buffer: total,
                first_hit_depth: &guides.depth,
                first_hit_normal: &guides.normal,
                first_hit_facet_id: &guides.facet_id,
            },
            &mut DenoiseScratch {
                denoiser: &mut AtrousDenoiser::new(),
                avg_color_buf: &mut Vec::new(),
                filtered_buf: &mut Vec::new(),
            },
        )
    }

    /// Ticks (without new samples) until a picture arrives, bounded at 5 s.
    fn next_picture(
        denoiser: &mut DisplayDenoiser,
        samples: u32,
        total: &[Vec3],
    ) -> DisplayPicture {
        for _ in 0..500 {
            if let DisplayUpdate::Picture(p) = denoiser.tick(false, samples, total) {
                return p;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("no display picture within 5 s");
    }

    /// At most one denoise in flight: a tick with new samples while one runs submits
    /// nothing but remembers them, and the tick that receives the picture submits the
    /// newest sum. Every picture -- ticks and the final one -- is exactly the viewer's own
    /// pipeline for the sum it was made from.
    #[test]
    fn pictures_match_the_viewers_pipeline_and_never_overlap() {
        let scene = scene();
        let len = (scene.width * scene.height) as usize;
        let (first, second) = (sum(len, 1.0), sum(len, 3.0));
        let mut denoiser = DisplayDenoiser::spawn(&scene);

        assert!(matches!(
            denoiser.tick(true, 2, &first),
            DisplayUpdate::Nothing
        ));
        assert!(denoiser.in_flight, "the first fresh tick submits");
        // Either the first picture is already back (and `second` goes out right away) or
        // the tick is busy and `second` waits for the tick that receives the picture.
        let first_picture = match denoiser.tick(true, 4, &second) {
            DisplayUpdate::Picture(p) => p,
            _ => next_picture(&mut denoiser, 4, &second),
        };
        assert_eq!(first_picture.samples, 2);
        assert_eq!(first_picture.rgba, reference(&scene, 2, &first));
        assert!(denoiser.in_flight, "the remembered newer sum was submitted");
        let second_picture = next_picture(&mut denoiser, 4, &second);
        assert_eq!(second_picture.samples, 4);
        assert_eq!(second_picture.rgba, reference(&scene, 4, &second));

        let final_rgba = denoiser
            .finish(6, &second, Duration::from_millis(50), || {
                Ok::<(), Infallible>(())
            })
            .unwrap()
            .expect("the thread is alive");
        assert_eq!(final_rgba, reference(&scene, 6, &second));
    }

    /// Without new samples a tick submits nothing.
    #[test]
    fn a_tick_without_new_samples_submits_nothing() {
        let scene = scene();
        let total = sum((scene.width * scene.height) as usize, 1.0);
        let mut denoiser = DisplayDenoiser::spawn(&scene);
        assert!(matches!(
            denoiser.tick(false, 3, &total),
            DisplayUpdate::Nothing
        ));
        assert!(!denoiser.in_flight);
        assert!(matches!(
            denoiser.tick(true, 0, &total),
            DisplayUpdate::Nothing
        ));
        assert!(!denoiser.in_flight, "no picture of zero samples");
    }
}
