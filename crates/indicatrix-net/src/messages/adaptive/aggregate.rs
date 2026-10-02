//! Folding consecutive small writes into one bandwidth sample.
//!
//! The estimator ignores writes under [`MIN_SAMPLE_BYTES`] because the kernel send buffer
//! swallows them at memory speed. A sender whose frames are all small (a thumbnail
//! preview, a small display frame) would then never learn its link. [`WriteAggregator`]
//! sums the bytes and the blocking time of consecutive small writes and releases one
//! sample once the sum reaches the threshold. It holds no clock: the caller passes the
//! measured `Duration`.

use super::bandwidth::MIN_SAMPLE_BYTES;
use std::time::Duration;

/// One bandwidth sample: `bytes` written in `elapsed` of blocking write time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteSample {
    /// Wire bytes written.
    pub bytes: usize,
    /// Summed duration of the blocking writes.
    pub elapsed: Duration,
    /// Whether several small writes were folded into this sample (as opposed to one
    /// write that reached the threshold alone). An aggregate is weaker evidence, since
    /// each of its parts may have fitted in the send buffer.
    pub aggregated: bool,
}

/// Accumulates small writes until their sum is a usable sample.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteAggregator {
    bytes: usize,
    elapsed: Duration,
    parts: u32,
}

impl WriteAggregator {
    /// An empty aggregator.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bytes: 0,
            elapsed: Duration::ZERO,
            parts: 0,
        }
    }

    /// Adds one write. A write of at least [`MIN_SAMPLE_BYTES`] is a sample by itself and
    /// discards what was pending (the pending writes were not contiguous with it). A
    /// smaller one is held until the pending sum reaches the threshold, which returns the
    /// sum and starts afresh.
    pub fn add(&mut self, bytes: usize, elapsed: Duration) -> Option<WriteSample> {
        if bytes >= MIN_SAMPLE_BYTES {
            *self = Self::new();
            return Some(WriteSample {
                bytes,
                elapsed,
                aggregated: false,
            });
        }
        self.bytes += bytes;
        self.elapsed += elapsed;
        self.parts += 1;
        if self.bytes < MIN_SAMPLE_BYTES {
            return None;
        }
        let sample = WriteSample {
            bytes: self.bytes,
            elapsed: self.elapsed,
            aggregated: self.parts > 1,
        };
        *self = Self::new();
        Some(sample)
    }

    /// Bytes held back, waiting for the threshold.
    #[must_use]
    pub const fn pending_bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn small_writes_accumulate_until_the_threshold_then_release_the_sum() {
        let mut agg = WriteAggregator::new();
        let part = MIN_SAMPLE_BYTES / 4;
        for _ in 0..3 {
            assert_eq!(agg.add(part, 10 * MS), None);
        }
        assert_eq!(agg.pending_bytes(), 3 * part);
        let sample = agg.add(part, 12 * MS).expect("the fourth write reaches it");
        assert_eq!(sample.bytes, 4 * part);
        assert_eq!(sample.elapsed, 42 * MS);
        assert!(sample.aggregated);
        assert_eq!(agg.pending_bytes(), 0);
    }

    #[test]
    fn a_large_write_is_its_own_sample_and_drops_the_pending_ones() {
        let mut agg = WriteAggregator::new();
        assert_eq!(agg.add(1000, MS), None);
        let sample = agg.add(MIN_SAMPLE_BYTES, 5 * MS).unwrap();
        assert_eq!(sample.bytes, MIN_SAMPLE_BYTES);
        assert_eq!(sample.elapsed, 5 * MS);
        assert!(!sample.aggregated);
        assert_eq!(agg.pending_bytes(), 0);
    }

    #[test]
    fn a_single_write_that_tops_up_the_sum_by_itself_is_not_marked_aggregated() {
        let mut agg = WriteAggregator::new();
        let sample = agg.add(MIN_SAMPLE_BYTES, MS).unwrap();
        assert!(!sample.aggregated);
        // The remainder of a sum left over after a release starts from zero.
        assert_eq!(agg.add(10, MS), None);
        assert_eq!(agg.pending_bytes(), 10);
    }

    #[test]
    fn zero_durations_are_summed_like_any_other_and_left_to_the_estimator() {
        let mut agg = WriteAggregator::new();
        let sample = agg
            .add(MIN_SAMPLE_BYTES - 1, Duration::ZERO)
            .or_else(|| agg.add(1, Duration::ZERO))
            .unwrap();
        assert_eq!(sample.elapsed, Duration::ZERO);
    }
}
