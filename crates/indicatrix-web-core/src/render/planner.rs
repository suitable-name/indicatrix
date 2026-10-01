//! [`ChunkPlanner`]: which sample range each render Worker traces next, sized from
//! measured chunk times.

use super::partition_len;

/// The chunk duration the planner aims for, in milliseconds.
///
/// The middle of a 150-250 ms window: long enough that the per-message overhead is
/// small, short enough that a scene change takes effect within a quarter second.
pub const TARGET_CHUNK_MS: f64 = 200.0;

/// Upper bound on one pass's samples per pixel, however fast the Workers are, so a
/// mis-measured rate can never schedule a multi-second chunk.
pub const MAX_CHUNK_SPP: u32 = 64;

/// How far (in passes) a Worker may run ahead of the last merged pass. Bounds the
/// chunks the accumulator holds while a slower Worker catches up.
pub const LOOKAHEAD_PASSES: u32 = 2;

/// A pass may grow to at most this many times the previous pass's samples, so one
/// noisy fast measurement cannot jump straight past the time window.
const MAX_GROWTH: u32 = 4;

/// Weight of a new measurement in the per-Worker moving average.
const RATE_SMOOTHING: f64 = 0.5;

/// Lowest live-view target, in samples per pixel.
pub const LIVE_MIN_SPP: u32 = 64;
/// Highest live-view target.
pub const LIVE_MAX_SPP: u32 = 1024;
/// Default live-view target.
pub const DEFAULT_LIVE_SPP: u32 = 256;
/// Lowest export target.
pub const EXPORT_MIN_SPP: u32 = 16;
/// Highest export target.
pub const EXPORT_MAX_SPP: u32 = 4096;

/// Clamps a live-view target to `[LIVE_MIN_SPP, LIVE_MAX_SPP]`.
#[must_use]
pub fn clamp_live_spp(spp: u32) -> u32 {
    spp.clamp(LIVE_MIN_SPP, LIVE_MAX_SPP)
}

/// Clamps an export target to `[EXPORT_MIN_SPP, EXPORT_MAX_SPP]`.
#[must_use]
pub fn clamp_export_spp(spp: u32) -> u32 {
    spp.clamp(EXPORT_MIN_SPP, EXPORT_MAX_SPP)
}

/// One chunk to send: `ToWorker::TraceChunk`'s fields plus the pass it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkAssignment {
    /// Index of the pass (0 = first).
    pub pass_index: u32,
    /// The partition, which is also the render Worker's index.
    pub first_pixel: u32,
    /// The partition count.
    pub stride: u32,
    /// First sample index.
    pub sample_offset: u32,
    /// Samples per pixel.
    pub spp: u32,
}

/// Plans passes for one scene: their sample ranges, and which one each partition's
/// Worker traces next.
///
/// # Sizing
///
/// Every Worker's chunk times are measured separately (a moving average of
/// milliseconds per pixel-sample). A new pass gets the samples that make the SLOWEST
/// measured Worker's chunk take about [`TARGET_CHUNK_MS`], capped at
/// [`MAX_CHUNK_SPP`] and at [`MAX_GROWTH`] times the previous pass. The first pass of
/// every scene is 1 spp, so a new scene shows at once. Faster Workers are not held back: they may
/// start the next passes (up to [`LOOKAHEAD_PASSES`] ahead of the merged ones), so the
/// pool stays busy while a slow Worker finishes.
///
/// The last pass is cut so the planned total lands exactly on the target.
#[derive(Debug, Clone)]
pub struct ChunkPlanner {
    partitions: u32,
    pixel_count: u32,
    target_spp: u32,
    /// `(sample_offset, spp)` of every pass defined so far.
    passes: Vec<(u32, u32)>,
    /// Per partition: the next pass index it will be given.
    next_pass: Vec<u32>,
    /// Per partition: measured milliseconds per pixel-sample.
    rates: Vec<Option<f64>>,
}

impl ChunkPlanner {
    /// A planner for a `pixel_count`-pixel frame split into `partitions` partitions,
    /// aiming for `target_spp` samples per pixel.
    #[must_use]
    pub fn new(partitions: u32, pixel_count: u32, target_spp: u32) -> Self {
        let partitions = partitions.max(1);
        Self {
            partitions,
            pixel_count,
            target_spp,
            passes: Vec::new(),
            next_pass: vec![0; partitions as usize],
            rates: vec![None; partitions as usize],
        }
    }

    /// Like [`Self::new`], keeping `previous`'s measured rates when it had the same
    /// partition count (a new scene on the same Workers). The first pass is still
    /// 1 spp, so the new scene shows up at once; the second is sized from the old rates
    /// instead of from that one sample.
    #[must_use]
    pub fn with_rates_from(
        previous: &Self,
        partitions: u32,
        pixel_count: u32,
        target_spp: u32,
    ) -> Self {
        let mut planner = Self::new(partitions, pixel_count, target_spp);
        if planner.partitions == previous.partitions && previous.pixel_count > 0 {
            planner.rates.clone_from(&previous.rates);
        }
        planner
    }

    /// The partition count.
    #[must_use]
    pub const fn partitions(&self) -> u32 {
        self.partitions
    }

    /// The target samples per pixel.
    #[must_use]
    pub const fn target_spp(&self) -> u32 {
        self.target_spp
    }

    /// Changes the target. Raising it lets planning continue; lowering it stops new
    /// passes once the planned total reaches it (already planned passes still finish).
    pub const fn set_target_spp(&mut self, target_spp: u32) {
        self.target_spp = target_spp;
    }

    /// Samples per pixel of every pass defined so far.
    #[must_use]
    pub fn planned_spp(&self) -> u32 {
        self.passes.iter().map(|&(_, spp)| spp).sum()
    }

    /// Passes defined so far.
    #[must_use]
    pub const fn planned_passes(&self) -> u32 {
        self.passes.len() as u32
    }

    /// `true` once every planned pass is merged and nothing more will be planned.
    #[must_use]
    pub fn is_done(&self, completed_passes: u32) -> bool {
        self.planned_spp() >= self.target_spp && completed_passes >= self.planned_passes()
    }

    /// The next chunk for `partition`'s Worker, or `None` when it must wait (it is
    /// [`LOOKAHEAD_PASSES`] ahead of `completed_passes`) or there is nothing left to plan.
    pub fn next_chunk(&mut self, partition: u32, completed_passes: u32) -> Option<ChunkAssignment> {
        let slot = partition as usize;
        let pass_index = *self.next_pass.get(slot)?;
        if pass_index >= completed_passes + LOOKAHEAD_PASSES {
            return None;
        }
        if pass_index as usize >= self.passes.len() {
            let remaining = self.target_spp.saturating_sub(self.planned_spp());
            if remaining == 0 {
                return None;
            }
            let offset = self.planned_spp();
            let spp = self.next_pass_spp().min(remaining);
            self.passes.push((offset, spp));
        }
        let (sample_offset, spp) = self.passes[pass_index as usize];
        self.next_pass[slot] += 1;
        Some(ChunkAssignment {
            pass_index,
            first_pixel: partition,
            stride: self.partitions,
            sample_offset,
            spp,
        })
    }

    /// Takes `assignment` back: its chunk could not be delivered, so `partition` is
    /// handed the same pass again by its next [`Self::next_chunk`]. Does nothing unless
    /// `assignment` is the partition's most recent one.
    pub fn unassign(&mut self, assignment: &ChunkAssignment) {
        if let Some(next) = self.next_pass.get_mut(assignment.first_pixel as usize)
            && *next == assignment.pass_index + 1
        {
            *next = assignment.pass_index;
        }
    }

    /// Records that `partition`'s Worker traced `spp` samples over its partition in
    /// `elapsed_ms`.
    pub fn record_timing(&mut self, partition: u32, spp: u32, elapsed_ms: f64) {
        let pixels = partition_len(self.pixel_count, partition, self.partitions);
        let work = f64::from(pixels) * f64::from(spp);
        let Some(rate) = self.rates.get_mut(partition as usize) else {
            return;
        };
        if work <= 0.0 || !elapsed_ms.is_finite() || elapsed_ms < 0.0 {
            return;
        }
        let sample = elapsed_ms / work;
        *rate = Some(rate.map_or(sample, |old| RATE_SMOOTHING.mul_add(sample - old, old)));
    }

    /// The measured rate of `partition`'s Worker, milliseconds per pixel-sample.
    #[must_use]
    pub fn rate(&self, partition: u32) -> Option<f64> {
        self.rates.get(partition as usize).copied().flatten()
    }

    /// Samples for the next new pass -- see the type's "Sizing" section.
    fn next_pass_spp(&self) -> u32 {
        // The first pass of every scene is one sample, so a new scene shows at once.
        if self.passes.is_empty() {
            return 1;
        }
        let slowest = self
            .rates
            .iter()
            .flatten()
            .copied()
            .fold(None, |acc: Option<f64>, r| {
                Some(acc.map_or(r, |a| a.max(r)))
            });
        let Some(slowest) = slowest else {
            return 1;
        };
        let pixels = self.pixel_count.div_ceil(self.partitions);
        let ms_per_spp = slowest * f64::from(pixels);
        let sized = if ms_per_spp > 0.0 {
            (TARGET_CHUNK_MS / ms_per_spp)
                .floor()
                .clamp(1.0, f64::from(MAX_CHUNK_SPP)) as u32
        } else {
            MAX_CHUNK_SPP
        };
        let growth_cap = self
            .passes
            .last()
            .map_or(MAX_CHUNK_SPP, |&(_, spp)| spp.saturating_mul(MAX_GROWTH));
        sized.min(growth_cap).max(1)
    }
}
