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
//! # Invalid values
//!
//! A chunk sum arrives from a lane (possibly a remote, untrusted one). A pixel with a
//! non-finite or negative component is replaced by zero before the chunk is parked or
//! folded -- dropped but still counted, the tracer's own rule -- so one bad value can
//! never poison the epoch's buffer. [`Merger::dropped_pixels`] is the running total, for
//! the caller to log once per request.
//!
//! # Memory
//!
//! A parked chunk holds a full-frame buffer. Parking only happens while an earlier
//! chunk is still in flight; with chunks sized to similar durations that is at most a
//! few chunks per lane -- UNLESS the chunk at the frontier is stalled (a wedged tracer
//! that still heartbeats), in which case every other lane keeps completing
//! chunks that pile up in `parked` with nothing to bound them. [`Self::with_parked_budget`]
//! charges each one against an external budget and refuses (rather than parks) once
//! it is exhausted, so a stalled frontier fails its own chunk's lane instead of
//! growing memory without limit.

use glam::Vec3;
use indicatrix_net::radiance::shuffle::is_valid_sample;
use std::{
    collections::BTreeMap,
    fmt,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

#[cfg(test)]
mod tests;

/// An external byte budget a [`Merger`] charges its parked (full-frame) buffers against.
///
/// Refuses to park a chunk past the budget rather than letting them grow unbounded (see
/// the module doc's "Memory" section: a stalled lane's straggling frontier
/// otherwise parks one full-frame buffer per completed-but-unmergeable chunk, with
/// nothing capping how many pile up). See [`Merger::with_parked_budget`].
pub trait ParkedBudget: fmt::Debug + Send + Sync {
    /// Reserves `bytes` for one newly parked chunk; `false` refuses (nothing charged).
    fn reserve(&self, bytes: u64) -> bool;

    /// Releases `bytes` reserved by an earlier [`Self::reserve`] once its chunk folds
    /// into the merged prefix (or the [`Merger`] is consumed by
    /// [`Merger::into_parts`] or dropped while the chunk was still parked).
    fn release(&self, bytes: u64);
}

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
    /// Parking this chunk would exceed the [`ParkedBudget`] passed to
    /// [`Merger::with_parked_budget`]; refused, not parked, so the count and buffer
    /// stay exactly what they were before this call.
    ParkedBudgetExceeded {
        /// The bytes this one chunk's buffer would have cost.
        bytes: u64,
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
            Self::ParkedBudgetExceeded { bytes } => write!(
                f,
                "parking this {bytes}-byte chunk would exceed the merger's parked-bytes budget"
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
    /// External byte budget every newly parked chunk is charged against, if any -- see
    /// [`Self::with_parked_budget`].
    budget: Option<Arc<dyn ParkedBudget>>,
    /// Pixels zeroed so far for a non-finite or negative component (module doc).
    dropped_pixels: AtomicU64,
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
            budget: None,
            dropped_pixels: AtomicU64::new(0),
        }
    }

    /// Charges every chunk this merger parks (see the module doc's "Memory" section)
    /// against `budget`, refusing (as [`MergeError::ParkedBudgetExceeded`]) instead of
    /// parking once it says no; released again as each chunk folds.
    #[must_use]
    pub fn with_parked_budget(mut self, budget: Arc<dyn ParkedBudget>) -> Self {
        self.budget = Some(budget);
        self
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

    /// The next sample index the fold expects: every sample below it is already folded
    /// into the merged prefix. Advances as the chunk at the frontier arrives.
    #[must_use]
    pub fn frontier(&self) -> u32 {
        self.lock().frontier
    }

    /// How many pixels of added chunks were zeroed for a non-finite or negative
    /// component (module doc, "Invalid values").
    #[must_use]
    pub fn dropped_pixels(&self) -> u64 {
        self.dropped_pixels.load(Ordering::Relaxed)
    }

    fn lock(&self) -> MutexGuard<'_, MergeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Bytes one parked (full-frame) chunk buffer costs.
    const fn chunk_bytes(&self) -> u64 {
        self.pixels as u64 * std::mem::size_of::<Vec3>() as u64
    }

    /// Adds the chunk `[first_sample, first_sample + done)` whose summed radiance is
    /// `sum`, returning the new exact total count. `done == 0` is a no-op (and `sum`
    /// may then be empty).
    ///
    /// # Errors
    ///
    /// [`MergeError`] when `sum` has the wrong length, the chunk overlaps samples
    /// already added, or parking it would exceed [`Self::with_parked_budget`]'s budget;
    /// nothing is merged then.
    pub fn add(&self, first_sample: u32, done: u32, mut sum: Vec<Vec3>) -> Result<u32, MergeError> {
        if done == 0 {
            return Ok(self.total());
        }
        if sum.len() != self.pixels {
            return Err(MergeError::WrongLength {
                expected: self.pixels,
                got: sum.len(),
            });
        }
        let dropped = zero_invalid_pixels(&mut sum);
        let mut state = self.lock();
        if overlaps(&state, first_sample, done) {
            return Err(MergeError::Overlap { first_sample, done });
        }
        // Reserved bytes always equal `parked.len() * chunk_bytes` as an invariant
        // maintained across calls: `before` is what it was reserved for coming in,
        // `after` is what it should be reserved for now that this call's insert and
        // fold have run. A chunk that arrives exactly at the frontier and immediately
        // folds away (relieving a backlog) never needs budget of its own -- only a
        // chunk that stays parked when this call returns does.
        let before = state.parked.len();
        state.parked.insert(first_sample, Parked { done, sum });
        state.parked_count += done;
        advance_frontier(&mut state, self.pixels);
        let after = state.parked.len();
        if let Some(budget) = &self.budget {
            let chunk_bytes = self.chunk_bytes();
            match after.cmp(&before) {
                // `advance_frontier` only ever removes, so a net increase can only be
                // this call's own new entry, alone, staying parked.
                std::cmp::Ordering::Greater if !budget.reserve(chunk_bytes) => {
                    state.parked.remove(&first_sample);
                    state.parked_count -= done;
                    return Err(MergeError::ParkedBudgetExceeded { bytes: chunk_bytes });
                }
                std::cmp::Ordering::Less => {
                    budget.release(chunk_bytes * (before - after) as u64);
                }
                std::cmp::Ordering::Greater | std::cmp::Ordering::Equal => {}
            }
        }
        let total = state.merged_count + state.parked_count;
        drop(state);
        if dropped > 0 {
            self.dropped_pixels.fetch_add(dropped, Ordering::Relaxed);
        }
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
    pub fn into_parts(mut self) -> (Vec<Vec3>, u32) {
        let pixels = self.pixels;
        let parked = self.take_parked();
        let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
        for chunk in parked.into_values() {
            fold(&mut state.merged, pixels, &chunk.sum);
            state.merged_count += chunk.done;
        }
        if state.merged.is_empty() {
            state.merged = vec![Vec3::ZERO; pixels];
        }
        (std::mem::take(&mut state.merged), state.merged_count)
    }

    /// Empties the parked map and releases its budget charge, so the charge is returned
    /// exactly once however the merger ends ([`Self::into_parts`] or [`Drop`]).
    fn take_parked(&mut self) -> BTreeMap<u32, Parked> {
        let chunk_bytes = self.chunk_bytes();
        let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
        let parked = std::mem::take(&mut state.parked);
        state.parked_count = 0;
        if let Some(budget) = &self.budget
            && !parked.is_empty()
        {
            budget.release(chunk_bytes * parked.len() as u64);
        }
        parked
    }
}

impl Drop for Merger {
    /// Returns whatever is still charged for parked chunks, so a merger dropped without
    /// [`Merger::into_parts`] (a cancelled or exhausted run) does not leak its parked
    /// bytes into a shared budget.
    fn drop(&mut self) {
        drop(self.take_parked());
    }
}

/// Zeroes every pixel that fails [`is_valid_sample`] and returns how many there were.
fn zero_invalid_pixels(sum: &mut [Vec3]) -> u64 {
    let mut dropped = 0u64;
    for pixel in sum {
        if !is_valid_sample(*pixel) {
            *pixel = Vec3::ZERO;
            dropped += 1;
        }
    }
    dropped
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
