//! The bounded list of best distinct states a run keeps when it was asked for several
//! candidates ([`super::OptimizeOptions::keep_candidates`] above one).
//!
//! A *state* is the vector of every free tier's angle, in the order of the run's free
//! tier list, with its fast-fidelity score (lower is better). Two states are *distinct*
//! when some angle differs by at least the separation (`min_step_deg` unless the
//! request says otherwise); otherwise one of them is redundant and the better one wins.
//!
//! The rule that keeps the list honest: an offered state is dropped when a similar
//! state with an equal or better score is already there, and when it is kept it evicts
//! every similar state with a worse score. So the entries are always pairwise
//! distinct, sorted best first (ties keep their arrival order), and at most
//! `capacity` long. Plain vectors only, so the list is deterministic.

/// A small tolerance so a step of exactly the separation, computed in floating
/// point, still counts as "at least the separation".
const SEPARATION_TOLERANCE_DEG: f64 = 1e-9;

/// The smallest separation a pool accepts; a zero or negative one would make two
/// identical states "distinct".
const MIN_SEPARATION_DEG: f64 = 1e-6;

/// One state in the pool.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct PoolEntry {
    /// Every free tier's angle, in free-tier order.
    pub(super) angles: Vec<f64>,
    /// The state's fast-fidelity score.
    pub(super) score: f32,
}

/// The bounded list of best distinct states. A pool with capacity one or less is
/// disabled and ignores every offer, so the single-result path pays nothing.
#[derive(Debug, Clone)]
pub(super) struct CandidatePool {
    capacity: usize,
    separation_deg: f64,
    entries: Vec<PoolEntry>,
}

/// Whether no angle of `a` and `b` differs by `separation_deg`: the two states are
/// the same candidate for the user's purposes. States of different length are never
/// similar.
fn similar(separation_deg: f64, a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| (x - y).abs() < separation_deg - SEPARATION_TOLERANCE_DEG)
}

impl CandidatePool {
    /// A pool keeping at most `capacity` states at least `separation_deg` apart.
    pub(super) const fn new(capacity: usize, separation_deg: f64) -> Self {
        let separation_deg = if separation_deg.is_finite() {
            separation_deg.max(MIN_SEPARATION_DEG)
        } else {
            MIN_SEPARATION_DEG
        };
        Self {
            capacity,
            separation_deg,
            entries: Vec::new(),
        }
    }

    /// Whether this pool keeps anything at all.
    pub(super) const fn is_enabled(&self) -> bool {
        self.capacity > 1
    }

    /// The kept states, best first (for the tests; a run reads them through
    /// [`Self::rivals`]).
    #[cfg(test)]
    pub(super) fn entries(&self) -> &[PoolEntry] {
        &self.entries
    }

    /// The kept states, best first, consuming the pool: what a multi-start run merges
    /// into one pool by offering them in start order.
    pub(super) fn into_entries(self) -> Vec<PoolEntry> {
        self.entries
    }

    /// Offers one scored state. Ignored when the pool is disabled or the score is not
    /// finite; see the module docs for the keep/evict rule.
    pub(super) fn offer(&mut self, angles: Vec<f64>, score: f32) {
        if !self.is_enabled() || !score.is_finite() {
            return;
        }
        let separation = self.separation_deg;
        if self
            .entries
            .iter()
            .any(|entry| entry.score <= score && similar(separation, &entry.angles, &angles))
        {
            return;
        }
        // Every similar entry left has a worse score than this one: evict them all.
        let mut kept: Vec<PoolEntry> = std::mem::take(&mut self.entries)
            .into_iter()
            .filter(|entry| !similar(separation, &entry.angles, &angles))
            .collect();
        let position = kept.partition_point(|entry| entry.score <= score);
        kept.insert(position, PoolEntry { angles, score });
        kept.truncate(self.capacity);
        self.entries = kept;
    }

    /// The kept states other than `best_angles` itself (and anything similar to it),
    /// best first, at most `capacity - 1` of them: the alternatives to the run's end
    /// point, which the caller already holds as a design.
    pub(super) fn rivals(self, best_angles: &[f64]) -> Vec<PoolEntry> {
        let Self {
            capacity,
            separation_deg,
            entries,
        } = self;
        entries
            .into_iter()
            .filter(|entry| !similar(separation_deg, &entry.angles, best_angles))
            .take(capacity.saturating_sub(1))
            .collect()
    }
}
