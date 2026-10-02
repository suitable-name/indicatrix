//! [`WorkerLane`]: one backend that traces sample chunks, and what it returns.

use crate::CancelToken;
use glam::Vec3;
use indicatrix_net::SceneState;

/// A contiguous absolute sample range `[first_sample, first_sample + samples)`, named
/// like the protocol's `RenderRequest` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SampleRange {
    /// The first absolute sample index.
    pub first_sample: u32,
    /// How many samples, starting at `first_sample`.
    pub samples: u32,
}

impl SampleRange {
    /// `[first_sample, first_sample + samples)`.
    #[must_use]
    pub const fn new(first_sample: u32, samples: u32) -> Self {
        Self {
            first_sample,
            samples,
        }
    }

    /// One past the last sample index.
    #[must_use]
    pub const fn end(self) -> u32 {
        self.first_sample + self.samples
    }

    /// Whether the range holds no samples.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.samples == 0
    }

    /// The untraced tail after a valid prefix of `done` samples, `[first_sample +
    /// done, end)`. `done` larger than the range yields an empty tail.
    #[must_use]
    pub const fn after_prefix(self, done: u32) -> Self {
        let done = if done > self.samples {
            self.samples
        } else {
            done
        };
        Self::new(self.first_sample + done, self.samples - done)
    }
}

/// What one [`WorkerLane::render_chunk`] call produced.
///
/// `done` is always a PREFIX of the assigned range: `sum` holds exactly the samples
/// `[first_sample, first_sample + done)`, never a gap and never anything past it. This
/// is the worker protocol's own guarantee (a stream's delivered `FRAME`s cover the
/// request from its start), so a lane over a real connection gets it for free.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkResult {
    /// Per-pixel summed CIE XYZ radiance of the `done` traced samples, `width *
    /// height` long. May be empty when `done == 0`.
    pub sum: Vec<Vec3>,
    /// How many samples of the assigned range were traced: the valid prefix.
    pub done: u32,
    /// The lane's own steady-state throughput measurement for this chunk, in
    /// samples per second, excluding one-time costs such as connection setup, scene
    /// upload and the final frame's encode and upload (for a remote lane: the marginal
    /// rate between its first and its last progress report that advanced the count).
    /// `None` lets the pool fall back to `done / wall time`.
    pub rate: Option<f64>,
    /// Why the chunk ended short, when it did. `None` together with `done` short of
    /// the range is reported as "ended early".
    pub error: Option<String>,
}

impl ChunkResult {
    /// Every sample of the range was traced.
    #[must_use]
    pub const fn complete(sum: Vec<Vec3>, done: u32, rate: Option<f64>) -> Self {
        Self {
            sum,
            done,
            rate,
            error: None,
        }
    }

    /// Only the prefix `done` was traced before `error` ended the chunk.
    #[must_use]
    pub const fn partial(sum: Vec<Vec3>, done: u32, error: String) -> Self {
        Self {
            sum,
            done,
            rate: None,
            error: Some(error),
        }
    }

    /// Nothing was traced.
    #[must_use]
    pub const fn failed(error: String) -> Self {
        Self {
            sum: Vec::new(),
            done: 0,
            rate: None,
            error: Some(error),
        }
    }
}

/// One backend contributing samples to an image: a joined remote worker, a TLS
/// connection to a plain worker, or the local CPU/GPU. The [`crate::LanePool`] does
/// not care which.
///
/// Implementations are shared across the pool's lane threads, hence `Send + Sync`;
/// one lane is only ever asked for one chunk at a time by the pool.
pub trait WorkerLane: Send + Sync {
    /// A short human-readable name for notes ("worker gpu-01", "local CPU").
    fn name(&self) -> &str;

    /// Traces `range` of `scene` at `scene.width x scene.height` and returns the summed
    /// radiance of the traced prefix.
    ///
    /// Must return promptly (with whatever prefix is done) once `cancel` is raised.
    /// Must never trace or report samples outside `range`: a result whose `done`
    /// exceeds `range.samples`, or whose `sum` has the wrong length, is discarded and
    /// counted as a failed chunk.
    fn render_chunk(
        &self,
        scene: &SceneState,
        range: SampleRange,
        cancel: &CancelToken,
    ) -> ChunkResult;

    /// Whether this lane's backing resource is gone for good (a joined worker's
    /// connection closed): the pool then removes the lane from a running epoch instead
    /// of retrying it through its backoff schedule, provided another lane remains. A lane
    /// that is the pool's last keeps going through the ordinary failure path, so a lane
    /// that can replace its resource on its own still gets the chance. Defaults to `false`.
    fn lost(&self) -> bool {
        false
    }
}
