//! [`LiveEpoch`]: one settled live-viewport image's shared sample budget and its merged
//! remote contribution.
//!
//! One `LiveEpoch` exists per settle (a new camera pose, scene or size starts a new
//! one; the old one is dropped, never carried over). It owns:
//!
//! - the epoch's [`SampleCursor`] over `[0, target_samples)`: the local tracer claims
//!   its per-frame range from it ([`LiveEpoch::claim_local`]) and the remote lane
//!   claims chunks from it ([`LiveEpoch::claim_remote`]), so every absolute sample
//!   index of the epoch is traced at most once across both backends;
//! - the remote side's merged radiance: one `remote_sum` plus `remote_count` for every
//!   FINISHED chunk, and the in-flight chunk's own [`Accumulator`] (each chunk gets a
//!   fresh one, since `Accumulator::begin_request` zeroes its buffer) -- the live
//!   counterpart of the export's `RemoteProgress`.
//!
//! # Consistent reads
//!
//! The finished-chunk sum, its count and the in-flight chunk slot live under ONE mutex,
//! so a display read ([`LiveEpoch::add_remote_into`]) and a chunk merge
//! ([`LiveEpoch::finish_chunk`]) can never interleave: a reader either sees a chunk
//! in flight (its buffer and its `samples_done`, read under the chunk accumulator's own
//! lock) or already merged (inside `remote_sum`/`remote_count`), never both or neither.
//! Lock order is always epoch mutex, then chunk accumulator; the connection thread
//! applying `FRAME`s only ever takes the chunk accumulator's lock.

use super::SampleCursor;
use glam::Vec3;
use indicatrix_net::client::Accumulator;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The remote chunk currently being traced for this epoch.
struct InFlightChunk {
    start: u32,
    count: u32,
    accumulator: Arc<Mutex<Accumulator>>,
}

/// Everything behind [`LiveEpoch`]'s single mutex -- see the module doc comment.
struct RemoteSums {
    /// Radiance of every finished chunk, summed. Allocated lazily on the first merge,
    /// so an epoch the remote side never contributes to costs no full-frame buffer.
    sum: Vec<Vec3>,
    /// Samples folded into `sum`.
    count: u32,
    in_flight: Option<InFlightChunk>,
}

/// How a finished (or failed) chunk ended, as [`LiveEpoch::finish_chunk`] reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkEnd {
    /// The chunk's first absolute sample index.
    pub start: u32,
    /// How many samples the chunk was assigned.
    pub count: u32,
    /// How many of them the worker actually traced and were merged -- always the
    /// valid PREFIX `[start, start + done)`, never more than `count`.
    pub done: u32,
}

impl ChunkEnd {
    /// The untraced tail `[start + done, start + count)` as `(start, count)`; `count`
    /// is `0` when the chunk finished completely.
    #[must_use]
    pub const fn remainder(self) -> (u32, u32) {
        (self.start + self.done, self.count - self.done)
    }
}

/// See the module doc comment.
pub struct LiveEpoch {
    cursor: SampleCursor,
    width: u32,
    height: u32,
    /// The scene generation this epoch was dispatched for -- an opaque number from the
    /// owner's scene-identity counter (the app's `RenderContext::scene_generation`).
    /// A backend may only contribute to this epoch while tracing that same scene.
    scene_generation: u64,
    remote: Mutex<RemoteSums>,
}

impl LiveEpoch {
    /// A fresh epoch for a `width x height` image whose combined budget is
    /// `target_samples` (the live view's single global sample target).
    #[must_use]
    pub const fn new(width: u32, height: u32, target_samples: u32) -> Self {
        Self {
            cursor: SampleCursor::new(0, target_samples),
            width,
            height,
            scene_generation: 0,
            remote: Mutex::new(RemoteSums {
                sum: Vec::new(),
                count: 0,
                in_flight: None,
            }),
        }
    }

    /// Stamps the scene generation this epoch is for (see [`Self::scene_generation`]).
    #[must_use]
    pub const fn for_scene(mut self, scene_generation: u64) -> Self {
        self.scene_generation = scene_generation;
        self
    }

    /// The scene generation this epoch was dispatched for; `0` unless stamped with
    /// [`Self::for_scene`].
    #[must_use]
    pub const fn scene_generation(&self) -> u64 {
        self.scene_generation
    }

    fn lock(&self) -> MutexGuard<'_, RemoteSums> {
        self.remote.lock().unwrap_or_else(PoisonError::into_inner)
    }

    const fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// The image dimensions this epoch was created for.
    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The local tracer's per-frame claim: up to `want` samples, a failed remote
    /// chunk's returned remainder first -- see [`SampleCursor::claim_local_bounded`].
    #[must_use]
    pub fn claim_local(&self, want: u32) -> Option<(u32, u32)> {
        self.cursor.claim_local_bounded(want)
    }

    /// The remote lane's chunk claim: up to `want` fresh samples from the shared pool
    /// only (never the local retry pile).
    #[must_use]
    pub fn claim_remote(&self, want: u32) -> Option<(u32, u32)> {
        self.cursor.claim(want)
    }

    /// Hands `[start, start + count)` to the local tracer after a remote chunk failed
    /// to trace it -- see [`SampleCursor::return_to_local`].
    pub fn return_to_local(&self, start: u32, count: u32) {
        self.cursor.return_to_local(start, count);
    }

    /// Records `accumulator` as the in-flight chunk covering `[start, start + count)`.
    /// Any chunk still recorded as in flight is dropped WITHOUT being merged (callers
    /// always finish or abandon a chunk before starting the next one).
    pub fn begin_chunk(&self, start: u32, count: u32, accumulator: Arc<Mutex<Accumulator>>) {
        self.lock().in_flight = Some(InFlightChunk {
            start,
            count,
            accumulator,
        });
    }

    /// Ends the in-flight chunk: merges its valid prefix (the accumulator's
    /// `samples_done`, clamped to the chunk's own `count`) into the finished-chunk sum
    /// and returns how it ended. `None` when no chunk was in flight.
    pub fn finish_chunk(&self) -> Option<ChunkEnd> {
        let mut sums = self.lock();
        let chunk = sums.in_flight.take()?;
        let acc = chunk
            .accumulator
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // A mismatched buffer length cannot happen for an accumulator this epoch's own
        // lane built; treated as "nothing traced" so the count stays honest.
        let merged = if acc.buffer().len() == self.pixel_count() {
            acc.samples_done().min(chunk.count)
        } else {
            0
        };
        if merged > 0 {
            if sums.sum.is_empty() {
                sums.sum = vec![Vec3::ZERO; self.pixel_count()];
            }
            for (dst, src) in sums.sum.iter_mut().zip(acc.buffer()) {
                *dst += *src;
            }
            sums.count += merged;
        }
        drop(acc);
        drop(sums);
        Some(ChunkEnd {
            start: chunk.start,
            count: chunk.count,
            done: merged,
        })
    }

    /// Drops the in-flight chunk WITHOUT merging anything (a drag or scene change made
    /// it worthless). Returns whether one was in flight.
    pub fn abandon_chunk(&self) -> bool {
        self.lock().in_flight.take().is_some()
    }

    /// Samples the remote side has contributed so far: every finished chunk plus the
    /// in-flight chunk's streamed progress.
    #[must_use]
    pub fn remote_done(&self) -> u32 {
        let sums = self.lock();
        let live = sums.in_flight.as_ref().map_or(0, |chunk| {
            chunk
                .accumulator
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .samples_done()
        });
        let finished = sums.count;
        drop(sums);
        finished + live
    }

    /// Adds the remote side's current radiance (finished chunks plus the in-flight
    /// chunk) into `dst` and returns EXACTLY the sample count that radiance
    /// represents, read consistently under the same locks. `dst` must be this epoch's
    /// `width * height`; any other length adds nothing and returns `0`, so the returned
    /// count always matches what was actually added.
    pub fn add_remote_into(&self, dst: &mut [Vec3]) -> u32 {
        if dst.len() != self.pixel_count() {
            return 0;
        }
        let sums = self.lock();
        if !sums.sum.is_empty() {
            for (d, s) in dst.iter_mut().zip(&sums.sum) {
                *d += *s;
            }
        }
        let mut count = sums.count;
        if let Some(chunk) = sums.in_flight.as_ref() {
            let acc = chunk
                .accumulator
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if acc.buffer().len() == dst.len() {
                for (d, s) in dst.iter_mut().zip(acc.buffer()) {
                    *d += *s;
                }
                count += acc.samples_done();
            }
            drop(acc);
        }
        drop(sums);
        count
    }

    /// The remote side's current radiance as a fresh `width * height` buffer, plus its
    /// sample count -- what `RemoteOnly` displays (see [`Self::add_remote_into`]).
    #[must_use]
    pub fn remote_snapshot(&self) -> (Vec<Vec3>, u32) {
        let mut buffer = vec![Vec3::ZERO; self.pixel_count()];
        let count = self.add_remote_into(&mut buffer);
        (buffer, count)
    }
}
