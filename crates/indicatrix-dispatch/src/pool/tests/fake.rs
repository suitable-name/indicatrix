//! In-process fake lanes whose output is a deterministic function of `(pixel, sample
//! index)`, so coverage and merged sums are checkable exactly. No network, no GPU.

use crate::{CancelToken, ChunkResult, SampleRange, WorkerLane};
use glam::Vec3;
use indicatrix_net::SceneState;
use std::{
    sync::{
        Arc, Barrier, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
    time::Duration,
};

/// A small real scene: only its `width x height` matters to the fakes.
pub(super) fn scene(width: u32, height: u32) -> SceneState {
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };
    SceneState {
        width,
        height,
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
    }
}

/// Which values a fake traces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Values {
    /// Small integers: every partial sum is exact in `f32`, so a merged buffer equals
    /// the reference sum exactly whatever the chunking.
    Integer,
    /// Fractional values of varying magnitude: summation order changes the rounding.
    Fractional,
}

/// The radiance of sample `sample` at pixel `pixel`.
pub(super) fn value(values: Values, pixel: u32, sample: u32) -> Vec3 {
    match values {
        Values::Integer => Vec3::new(
            ((pixel * 7 + sample) % 13) as f32,
            ((pixel + sample * 3) % 5) as f32,
            (sample % 4) as f32,
        ),
        Values::Fractional => {
            let h = pixel
                .wrapping_mul(0x9e37_79b9)
                .wrapping_add(sample.wrapping_mul(0x85eb_ca6b))
                .rotate_left(13);
            let scale = ((sample % 1000) as f32).mul_add(37.0, 1.0);
            Vec3::new(
                (h % 1_000_003) as f32 / 997.0 * scale,
                (h % 7919) as f32 / 13.0,
                (h >> 20) as f32 * 1e-3,
            )
        }
    }
}

/// The sum a lane computes for one chunk: samples in ascending order, per pixel.
pub(super) fn chunk_sum(values: Values, pixels: usize, range: SampleRange, done: u32) -> Vec<Vec3> {
    let mut sum = vec![Vec3::ZERO; pixels];
    for sample in range.first_sample..range.first_sample + done {
        for (pixel, dst) in sum.iter_mut().enumerate() {
            *dst += value(values, pixel as u32, sample);
        }
    }
    sum
}

/// When a fake fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Failure {
    Never,
    /// Every chunk stops after tracing `after` samples (if it is longer than that).
    Always {
        after: u32,
    },
    /// The first `chunks` chunks stop after `after` samples; later ones succeed.
    First {
        chunks: u32,
        after: u32,
    },
    /// Panics on every chunk.
    Panic,
}

/// See the module doc.
pub(super) struct FakeLane {
    name: String,
    values: Values,
    /// Sleep per traced sample (the lane's speed).
    per_sample: Duration,
    failure: Failure,
    chunks_seen: AtomicU32,
    /// Every sample index this lane reported as done.
    traced: Mutex<Vec<u32>>,
}

impl FakeLane {
    pub(super) fn new(name: &str, values: Values, per_sample: Duration, failure: Failure) -> Self {
        Self {
            name: name.to_owned(),
            values,
            per_sample,
            failure,
            chunks_seen: AtomicU32::new(0),
            traced: Mutex::new(Vec::new()),
        }
    }

    /// The sample indices this lane delivered.
    pub(super) fn traced(&self) -> Vec<u32> {
        self.traced
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// How many samples of `range` this chunk will trace before failing.
    fn limit(&self, chunk_number: u32, range: SampleRange) -> Option<u32> {
        match self.failure {
            Failure::Always { after } => Some(after),
            Failure::First { chunks, after } if chunk_number < chunks => Some(after),
            Failure::Never | Failure::First { .. } | Failure::Panic => None,
        }
        .filter(|&after| after < range.samples)
    }
}

/// A lane whose backing resource dies: it traces `healthy_chunks` chunks completely, then
/// traces only `after` samples of the next one, reports it short, and is
/// [`WorkerLane::lost`] from then on (every later chunk fails outright).
pub(super) struct DyingLane {
    inner: FakeLane,
    healthy_chunks: u32,
    after: u32,
    chunks: AtomicU32,
    dead: AtomicBool,
}

impl DyingLane {
    pub(super) fn new(per_sample: Duration, healthy_chunks: u32, after: u32) -> Self {
        Self {
            inner: FakeLane::new("dying", Values::Integer, per_sample, Failure::Never),
            healthy_chunks,
            after,
            chunks: AtomicU32::new(0),
            dead: AtomicBool::new(false),
        }
    }

    /// The sample indices this lane delivered.
    pub(super) fn traced(&self) -> Vec<u32> {
        self.inner.traced()
    }

    /// How many chunks this lane was handed.
    pub(super) fn chunks(&self) -> u32 {
        self.chunks.load(Ordering::Relaxed)
    }
}

impl WorkerLane for DyingLane {
    fn name(&self) -> &'static str {
        "dying"
    }

    fn render_chunk(
        &self,
        scene: &SceneState,
        range: SampleRange,
        cancel: &CancelToken,
    ) -> ChunkResult {
        let number = self.chunks.fetch_add(1, Ordering::Relaxed);
        if self.dead.load(Ordering::Relaxed) {
            return ChunkResult::failed("connection closed".to_owned());
        }
        if number < self.healthy_chunks {
            return self.inner.render_chunk(scene, range, cancel);
        }
        let head = SampleRange::new(range.first_sample, self.after.min(range.samples));
        let traced = self.inner.render_chunk(scene, head, cancel);
        self.dead.store(true, Ordering::Relaxed);
        ChunkResult::partial(traced.sum, traced.done, "connection closed".to_owned())
    }

    fn lost(&self) -> bool {
        self.dead.load(Ordering::Relaxed)
    }
}

/// A lane that traces instantly (an all-zero sum), reports a fixed `rate` and records the
/// length of every chunk it is handed, so a test sees exactly how the pool sized them.
pub(super) struct SizeRecorder {
    /// The rate every chunk reports, in samples per second.
    rate: f64,
    sizes: Mutex<Vec<u32>>,
    /// Waited at inside the first chunk, so that every lane sharing the barrier has
    /// claimed its first chunk before any of them finishes one.
    rendezvous: Option<Arc<Barrier>>,
}

impl SizeRecorder {
    pub(super) const fn new(rate: f64) -> Self {
        Self {
            rate,
            sizes: Mutex::new(Vec::new()),
            rendezvous: None,
        }
    }

    /// Makes the first chunk wait at `gate` until every other holder of it arrives.
    pub(super) fn meeting_at(mut self, gate: Arc<Barrier>) -> Self {
        self.rendezvous = Some(gate);
        self
    }

    /// The length of every chunk handed to this lane, in the order it received them.
    pub(super) fn sizes(&self) -> Vec<u32> {
        self.sizes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl WorkerLane for SizeRecorder {
    fn name(&self) -> &'static str {
        "size recorder"
    }

    fn render_chunk(
        &self,
        scene: &SceneState,
        range: SampleRange,
        _cancel: &CancelToken,
    ) -> ChunkResult {
        let first = {
            let mut sizes = self.sizes.lock().unwrap_or_else(PoisonError::into_inner);
            sizes.push(range.samples);
            sizes.len() == 1
        };
        if let Some(gate) = self.rendezvous.as_ref().filter(|_| first) {
            gate.wait();
        }
        let pixels = scene.width as usize * scene.height as usize;
        ChunkResult::complete(vec![Vec3::ZERO; pixels], range.samples, Some(self.rate))
    }
}

impl WorkerLane for FakeLane {
    fn name(&self) -> &str {
        &self.name
    }

    fn render_chunk(
        &self,
        scene: &SceneState,
        range: SampleRange,
        cancel: &CancelToken,
    ) -> ChunkResult {
        let chunk_number = self.chunks_seen.fetch_add(1, Ordering::Relaxed);
        assert!(
            self.failure != Failure::Panic,
            "fake lane {} panics",
            self.name
        );
        let limit = self.limit(chunk_number, range);
        let pixels = scene.width as usize * scene.height as usize;
        let mut done = 0;
        for _ in 0..limit.unwrap_or(range.samples) {
            if cancel.is_cancelled() {
                break;
            }
            if !self.per_sample.is_zero() {
                thread::sleep(self.per_sample);
            }
            done += 1;
        }
        let sum = chunk_sum(self.values, pixels, range, done);
        self.traced
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(range.first_sample..range.first_sample + done);
        if done == range.samples {
            ChunkResult::complete(sum, done, None)
        } else if cancel.is_cancelled() {
            ChunkResult::partial(sum, done, "cancelled".to_owned())
        } else {
            ChunkResult::partial(sum, done, format!("{} dropped the connection", self.name))
        }
    }
}
