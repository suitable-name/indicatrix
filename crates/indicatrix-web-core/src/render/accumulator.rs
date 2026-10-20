//! [`Accumulator`]: the page's full-frame running sum, merged pass by pass.

use std::collections::BTreeMap;

use glam::Vec3;

use super::partition_len;

/// Why [`Accumulator::add_chunk`] refused a chunk of the CURRENT scene. Each one is a
/// host bug or a corrupted message, never an ordinary race (a stale scene is
/// [`ChunkOutcome::Stale`], not a rejection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkRejection {
    /// `stride` is not the accumulator's partition count, or `first_pixel` is not a
    /// partition index.
    BadPartition,
    /// `sums` does not have one entry per pixel of the partition.
    WrongLength {
        /// Pixels the partition owns.
        expected: usize,
        /// Entries received.
        got: usize,
    },
    /// A full pass's `sums` length does not equal the full-frame pixel count.
    LengthMismatch,
    /// The chunk's `spp` differs from another partition's chunk for the same pass.
    SppMismatch,
    /// This partition's chunk for this pass already arrived.
    Duplicate,
    /// The pass starting at this sample offset was already merged.
    AlreadyCommitted,
    /// A zero-sample chunk.
    Empty,
    /// A full pass holds a NaN, an infinite or a negative value. It is refused whole so
    /// one bad pass can never poison the running sum; the caller hands the pass back to
    /// the planner (`ChunkPlanner::return_gpu_pass`).
    InvalidSums,
}

/// What [`Accumulator::add_chunk`] did with a chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkOutcome {
    /// The chunk belongs to an older scene and was dropped.
    Stale,
    /// Refused; see [`ChunkRejection`].
    Rejected(ChunkRejection),
    /// Held until its pass is complete (and every earlier pass merged).
    Pending,
    /// The chunk completed one or more passes, now merged; `passes` is how many.
    Committed {
        /// Passes merged by this call.
        passes: u32,
    },
}

/// One pass still waiting to be merged.
#[derive(Debug)]
enum PendingPass {
    /// A pass split across CPU partitions.
    Partitioned {
        spp: u32,
        parts: Vec<Option<Vec<Vec3>>>,
        received: u32,
    },
    /// A complete full-frame pass (from GPU).
    Full { spp: u32, sums: Vec<[f32; 3]> },
}

/// The full-frame sum buffer and its per-pass bookkeeping.
///
/// A pass `[sample_offset, sample_offset + spp)` is merged only once every partition's
/// chunk for it has arrived AND every earlier pass is merged, so [`Self::sum`] always
/// holds exactly [`Self::sample_count`] samples in every pixel. Chunks for a different
/// `scene_id` are dropped ([`ChunkOutcome::Stale`]).
#[derive(Debug)]
pub struct Accumulator {
    scene_id: u64,
    width: u32,
    height: u32,
    partitions: u32,
    sum: Vec<Vec3>,
    completed_passes: u32,
    samples: u32,
    pending: BTreeMap<u32, PendingPass>,
}

impl Accumulator {
    /// An empty accumulator for scene `scene_id`, a `width x height` frame split into
    /// `partitions` interleaved partitions (clamped to at least 1).
    #[must_use]
    pub fn new(scene_id: u64, width: u32, height: u32, partitions: u32) -> Self {
        Self {
            scene_id,
            width,
            height,
            partitions: partitions.max(1),
            sum: vec![Vec3::ZERO; width as usize * height as usize],
            completed_passes: 0,
            samples: 0,
            pending: BTreeMap::new(),
        }
    }

    /// The scene this buffer accumulates.
    #[must_use]
    pub const fn scene_id(&self) -> u64 {
        self.scene_id
    }

    /// Frame width.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Frame height.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Partition count (the stride every chunk must use).
    #[must_use]
    pub const fn partitions(&self) -> u32 {
        self.partitions
    }

    /// The running per-pixel SUM (not the mean) over every merged pass, row-major.
    #[must_use]
    pub fn sum(&self) -> &[Vec3] {
        &self.sum
    }

    /// Samples per pixel in [`Self::sum`] -- the count to divide by for display.
    #[must_use]
    pub const fn sample_count(&self) -> u32 {
        self.samples
    }

    /// Passes merged so far.
    #[must_use]
    pub const fn completed_passes(&self) -> u32 {
        self.completed_passes
    }

    /// Passes with at least one chunk held but not yet merged.
    #[must_use]
    pub fn pending_passes(&self) -> usize {
        self.pending.len()
    }

    /// The per-pixel mean (`sum / sample_count`), or all zero before the first pass.
    #[must_use]
    pub fn mean(&self) -> Vec<Vec3> {
        if self.samples == 0 {
            return vec![Vec3::ZERO; self.sum.len()];
        }
        let inv = 1.0 / self.samples as f32;
        self.sum.iter().map(|&v| v * inv).collect()
    }

    /// Adds one chunk: partition `first_pixel` of `stride`, samples
    /// `[sample_offset, sample_offset + spp)`, `sums` in partition pixel order (what
    /// [`super::handle_trace_chunk`] returns).
    pub fn add_chunk(
        &mut self,
        scene_id: u64,
        first_pixel: u32,
        stride: u32,
        sample_offset: u32,
        spp: u32,
        sums: Vec<Vec3>,
    ) -> ChunkOutcome {
        if scene_id != self.scene_id {
            return ChunkOutcome::Stale;
        }
        if let Err(rejection) = self.check_chunk(first_pixel, stride, sample_offset, spp, &sums) {
            return ChunkOutcome::Rejected(rejection);
        }
        let partitions = self.partitions as usize;
        let pass = self
            .pending
            .entry(sample_offset)
            .or_insert_with(|| PendingPass::Partitioned {
                spp,
                parts: vec![None; partitions],
                received: 0,
            });
        match pass {
            PendingPass::Partitioned {
                spp: pass_spp,
                parts,
                received,
            } => {
                if *pass_spp != spp {
                    return ChunkOutcome::Rejected(ChunkRejection::SppMismatch);
                }
                let slot = &mut parts[first_pixel as usize];
                if slot.is_some() {
                    return ChunkOutcome::Rejected(ChunkRejection::Duplicate);
                }
                *slot = Some(sums);
                *received += 1;
            }
            PendingPass::Full { .. } => {
                return ChunkOutcome::Rejected(ChunkRejection::Duplicate);
            }
        }
        let merged = self.merge_ready_passes();
        if merged == 0 {
            ChunkOutcome::Pending
        } else {
            ChunkOutcome::Committed { passes: merged }
        }
    }

    /// `true` when `sums` is non-empty and every value is finite and non-negative: the
    /// only full-pass buffers [`Self::add_full_pass`] merges.
    #[must_use]
    pub fn full_pass_sums_valid(sums: &[[f32; 3]]) -> bool {
        !sums.is_empty()
            && sums
                .iter()
                .all(|s| s.iter().all(|v| v.is_finite() && *v >= 0.0))
    }

    /// Adds one complete full-frame pass (from the GPU worker or a single full trace).
    ///
    /// A pass with a non-finite or negative value is refused
    /// ([`ChunkRejection::InvalidSums`]).
    pub fn add_full_pass(
        &mut self,
        scene_id: u64,
        pass_id: u64,
        sample_offset: u32,
        spp: u32,
        sums: &[[f32; 3]],
    ) -> ChunkOutcome {
        // Cheap checks first, so a refused pass does not copy its buffer.
        if scene_id != self.scene_id {
            return ChunkOutcome::Stale;
        }
        if sums.len() != self.sum.len() {
            return ChunkOutcome::Rejected(ChunkRejection::LengthMismatch);
        }
        if !Self::full_pass_sums_valid(sums) {
            return ChunkOutcome::Rejected(ChunkRejection::InvalidSums);
        }
        self.add_full_pass_owned(scene_id, pass_id, sample_offset, spp, sums.to_vec())
    }

    /// Like [`Self::add_full_pass`], taking the buffer by value (no copy).
    pub fn add_full_pass_owned(
        &mut self,
        scene_id: u64,
        _pass_id: u64,
        sample_offset: u32,
        spp: u32,
        sums: Vec<[f32; 3]>,
    ) -> ChunkOutcome {
        if scene_id != self.scene_id {
            return ChunkOutcome::Stale;
        }
        if sums.len() != self.sum.len() {
            return ChunkOutcome::Rejected(ChunkRejection::LengthMismatch);
        }
        if !Self::full_pass_sums_valid(&sums) {
            return ChunkOutcome::Rejected(ChunkRejection::InvalidSums);
        }
        if spp == 0 {
            return ChunkOutcome::Rejected(ChunkRejection::Empty);
        }
        if sample_offset < self.samples {
            return ChunkOutcome::Rejected(ChunkRejection::AlreadyCommitted);
        }
        if self.pending.contains_key(&sample_offset) {
            return ChunkOutcome::Rejected(ChunkRejection::Duplicate);
        }
        self.pending
            .insert(sample_offset, PendingPass::Full { spp, sums });
        let merged = self.merge_ready_passes();
        if merged == 0 {
            ChunkOutcome::Pending
        } else {
            ChunkOutcome::Committed { passes: merged }
        }
    }

    const fn check_chunk(
        &self,
        first_pixel: u32,
        stride: u32,
        sample_offset: u32,
        spp: u32,
        sums: &[Vec3],
    ) -> Result<(), ChunkRejection> {
        if stride != self.partitions || first_pixel >= self.partitions {
            return Err(ChunkRejection::BadPartition);
        }
        if spp == 0 {
            return Err(ChunkRejection::Empty);
        }
        if sample_offset < self.samples {
            return Err(ChunkRejection::AlreadyCommitted);
        }
        let expected = partition_len(self.width * self.height, first_pixel, stride) as usize;
        if sums.len() != expected {
            return Err(ChunkRejection::WrongLength {
                expected,
                got: sums.len(),
            });
        }
        Ok(())
    }

    /// Merges every complete pass that starts exactly where the merged samples end, in
    /// order; returns how many.
    fn merge_ready_passes(&mut self) -> u32 {
        let mut merged = 0;
        loop {
            let ready = match self.pending.get(&self.samples) {
                Some(PendingPass::Partitioned { received, .. }) => *received == self.partitions,
                Some(PendingPass::Full { .. }) => true,
                None => false,
            };
            if !ready {
                return merged;
            }
            let Some(pass) = self.pending.remove(&self.samples) else {
                return merged;
            };
            match pass {
                PendingPass::Partitioned { spp, parts, .. } => {
                    for (first_pixel, part) in parts.into_iter().enumerate() {
                        if let Some(sums) = part {
                            indicatrix::renderer::cpu_frame::scatter_interleaved(
                                &mut self.sum,
                                u32::try_from(first_pixel).expect("a partition index fits u32"),
                                self.partitions,
                                &sums,
                            );
                        }
                    }
                    self.samples += spp;
                }
                PendingPass::Full { spp, sums } => {
                    for (dst, src) in self.sum.iter_mut().zip(sums) {
                        *dst += Vec3::new(src[0], src[1], src[2]);
                    }
                    self.samples += spp;
                }
            }
            self.completed_passes += 1;
            merged += 1;
        }
    }
}
