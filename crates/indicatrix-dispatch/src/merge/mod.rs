//! [`Merger`]: the one merged radiance buffer and exact sample count of an image epoch.
//!
//! # Determinism of the float summation
//!
//! Chunks finish in whatever order the lanes happen to finish them. Adding each chunk
//! into the merged buffer on arrival would make the float rounding of the result
//! depend on thread timing. Instead the merge is a left fold **in ascending
//! `first_sample` order**, starting from zero:
//!
//! ```text
//! merged = ((0 + chunk[a]) + chunk[b]) + chunk[c] + ...   with a < b < c < ...
//! ```
//!
//! A chunk that arrives ahead of the fold's frontier (the next unmerged sample index)
//! is parked in a pending map keyed by its `first_sample`; whenever the chunk starting
//! exactly at the frontier arrives, it and every parked chunk contiguous with it are
//! folded in, in order. Because the samples of one epoch come from one
//! [`crate::SampleCursor`], the finished chunks tile the range exactly, so the frontier
//! always reaches the end and nothing stays parked in a complete run.
//!
//! **What this guarantees:** for a given partition of the range into chunks (and
//! given each chunk's own sum), the merged buffer is bit-identical no matter which lane
//! traced which chunk or in which order they finished.
//!
//! **What it does not:** the partition itself. With a timed [`crate::ChunkPolicy`],
//! chunk sizes come from measured rates, so two runs may cut the range differently and
//! round differently; the results are then statistically equivalent, not
//! bit-identical. [`crate::ChunkPolicy::fixed`] (and no lane failures) makes the
//! partition timing-independent too. Different backends are never bit-identical to
//! each other anyway (GPU `fma` fusion), so a chunk's own sum is only reproducible on
//! the same lane.
//!
//! # Memory
//!
//! A parked chunk holds a full-frame buffer. Parking only happens while an earlier
//! chunk is still in flight; with chunks sized to similar durations that is at most a
//! few chunks per lane.

use glam::Vec3;
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Mutex, MutexGuard, PoisonError},
};

#[cfg(test)]
mod tests;

/// Why [`Merger::add`] refused a chunk. A refused chunk contributes nothing, so the
/// count always matches the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeError {
    /// The chunk's buffer is not `width * height` long.
    WrongLength {
        /// The merger's pixel count.
        expected: usize,
        /// The chunk's buffer length.
        got: usize,
    },
    /// The chunk overlaps samples already merged or parked: a disjointness violation.
    Overlap {
        /// The refused chunk's first sample.
        first_sample: u32,
        /// The refused chunk's traced sample count.
        done: u32,
    },
}

impl fmt::Display for MergeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongLength { expected, got } => write!(
                f,
                "chunk buffer holds {got} pixels, the image has {expected}"
            ),
            Self::Overlap { first_sample, done } => write!(
                f,
                "chunk [{first_sample}, +{done}) overlaps samples already merged"
            ),
        }
    }
}

impl std::error::Error for MergeError {}

/// A chunk that arrived ahead of the fold's frontier.
#[derive(Debug)]
struct Parked {
    done: u32,
    sum: Vec<Vec3>,
}

#[derive(Debug)]
struct MergeState {
    /// Every chunk below `frontier`, folded in order. Allocated on the first fold.
    merged: Vec<Vec3>,
    merged_count: u32,
    /// The next sample index the fold expects.
    frontier: u32,
    /// Chunks at or past `frontier`, keyed by `first_sample`.
    parked: BTreeMap<u32, Parked>,
    parked_count: u32,
}

/// See the module doc. Shared by reference between the pool's lane threads (which
/// [`add`](Self::add)) and any observer thread (which reads
/// [`snapshot_into`](Self::snapshot_into) / [`total`](Self::total)).
#[derive(Debug)]
pub struct Merger {
    pixels: usize,
    first_sample: u32,
    state: Mutex<MergeState>,
}

impl Merger {
    /// An empty merge for a `pixels`-pixel image whose sample range starts at
    /// `first_sample`.
    #[must_use]
    pub const fn new(pixels: usize, first_sample: u32) -> Self {
        Self {
            pixels,
            first_sample,
            state: Mutex::new(MergeState {
                merged: Vec::new(),
                merged_count: 0,
                frontier: first_sample,
                parked: BTreeMap::new(),
                parked_count: 0,
            }),
        }
    }

    /// The image's pixel count.
    #[must_use]
    pub const fn pixel_count(&self) -> usize {
        self.pixels
    }

    /// The first sample index of the range this merger was built for.
    #[must_use]
    pub const fn first_sample(&self) -> u32 {
        self.first_sample
    }

    fn lock(&self) -> MutexGuard<'_, MergeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds the chunk `[first_sample, first_sample + done)` whose summed radiance is
    /// `sum`, returning the new exact total count. `done == 0` is a no-op (and `sum`
    /// may then be empty).
    ///
    /// # Errors
    ///
    /// [`MergeError`] when `sum` has the wrong length or the chunk overlaps samples
    /// already added; nothing is merged then.
    pub fn add(&self, first_sample: u32, done: u32, sum: Vec<Vec3>) -> Result<u32, MergeError> {
        if done == 0 {
            return Ok(self.total());
        }
        if sum.len() != self.pixels {
            return Err(MergeError::WrongLength {
                expected: self.pixels,
                got: sum.len(),
            });
        }
        let mut state = self.lock();
        if overlaps(&state, first_sample, done) {
            return Err(MergeError::Overlap { first_sample, done });
        }
        state.parked.insert(first_sample, Parked { done, sum });
        state.parked_count += done;
        advance_frontier(&mut state, self.pixels);
        let total = state.merged_count + state.parked_count;
        drop(state);
        Ok(total)
    }

    /// The exact number of samples added so far (folded plus parked).
    #[must_use]
    pub fn total(&self) -> u32 {
        let state = self.lock();
        state.merged_count + state.parked_count
    }

    /// Adds everything merged so far into `dst` (folded buffer first, then parked
    /// chunks in `first_sample` order) and returns exactly the sample count that
    /// represents. `dst` must be `pixel_count` long; any other length adds nothing and
    /// returns `0`, so the returned count always matches what was added.
    pub fn snapshot_into(&self, dst: &mut [Vec3]) -> u32 {
        if dst.len() != self.pixels {
            return 0;
        }
        let state = self.lock();
        add_assign(dst, &state.merged);
        for parked in state.parked.values() {
            add_assign(dst, &parked.sum);
        }
        let count = state.merged_count + state.parked_count;
        drop(state);
        count
    }

    /// Consumes the merger: the merged buffer (`pixel_count` long, zeros if nothing
    /// was added) and its exact sample count. Parked chunks left behind by an
    /// incomplete run (cancel, lanes lost) are folded in `first_sample` order, so this
    /// is deterministic too.
    #[must_use]
    pub fn into_parts(self) -> (Vec<Vec3>, u32) {
        let pixels = self.pixels;
        let mut state = self
            .state
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        let parked = std::mem::take(&mut state.parked);
        for chunk in parked.into_values() {
            fold(&mut state.merged, pixels, &chunk.sum);
            state.merged_count += chunk.done;
        }
        if state.merged.is_empty() {
            state.merged = vec![Vec3::ZERO; pixels];
        }
        (state.merged, state.merged_count)
    }
}

/// Whether `[first, first + done)` intersects the folded prefix or any parked chunk.
fn overlaps(state: &MergeState, first: u32, done: u32) -> bool {
    let end = first + done;
    if first < state.frontier {
        return true;
    }
    let before = state.parked.range(..=first).next_back();
    if before.is_some_and(|(&start, chunk)| start + chunk.done > first) {
        return true;
    }
    state
        .parked
        .range(first..)
        .next()
        .is_some_and(|(&start, _)| start < end)
}

/// Folds every parked chunk that starts exactly at the frontier, in order.
fn advance_frontier(state: &mut MergeState, pixels: usize) {
    while let Some(chunk) = state.parked.remove(&state.frontier) {
        fold(&mut state.merged, pixels, &chunk.sum);
        state.frontier += chunk.done;
        state.merged_count += chunk.done;
        state.parked_count -= chunk.done;
    }
}

/// `merged += sum`, allocating `merged` as zeros on first use so every fold is the
/// same `0 + a + b + ...` sequence.
fn fold(merged: &mut Vec<Vec3>, pixels: usize, sum: &[Vec3]) {
    if merged.is_empty() {
        *merged = vec![Vec3::ZERO; pixels];
    }
    add_assign(merged, sum);
}

fn add_assign(dst: &mut [Vec3], src: &[Vec3]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d += *s;
    }
}
