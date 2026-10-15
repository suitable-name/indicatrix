//! The labelled states of the imitation rounds (`DAgger`) and the exploration noise that visits them.
//!
//! Every round rolls the current policy out with a little noise, asks the simulator for the
//! finite-difference labels of a share of the visited states ([`indicatrix_ai::env::TeacherLabel`])
//! and adds them to one growing aggregate, [`LabelledStates`]. The next update trains on the whole
//! aggregate, so the model keeps learning from the states of its earlier, worse selves. The
//! aggregate is a first-in-first-out queue with a capacity: past it the oldest states leave.

use std::collections::VecDeque;

use indicatrix_ai::{
    env::{N_COMPONENTS, TeacherLabel},
    model::Prng,
    training::ImitationBatch,
};

/// One labelled state: the flat observation, the free-tier mask and the label rows the
/// imitation loss reads (see [`indicatrix_ai::training::ImitationBatch`]).
#[derive(Clone, Debug, PartialEq)]
pub struct LabelledSample {
    /// The flat observation, `[global | padded tier tokens]`.
    state: Vec<f32>,
    /// Which real tiers the environment moves; its length is the tier count.
    free: Vec<bool>,
    /// Per real tier, the four component slopes; all NaN for a tier without a label.
    sensitivities: Vec<[f32; N_COMPONENTS]>,
    /// Per real tier, the score slope; NaN for a tier without a label.
    score_slopes: Vec<f32>,
    /// The oracle step's score drop; NaN when the label has none.
    oracle_gain: f32,
}

impl LabelledSample {
    /// The sample of `state` (flat observation) with free-tier mask `free` and `label`.
    ///
    /// The tier count is `free.len()`. A tier the label has no entry for (`None`, or beyond the
    /// label's length) gets NaN rows, and so does a missing oracle gain: NaN means "no label"
    /// for the loss.
    #[must_use]
    pub fn new(state: Vec<f32>, free: Vec<bool>, label: &TeacherLabel) -> Self {
        let tier = |k: usize| label.tiers.get(k).copied().flatten();
        let sensitivities = (0..free.len())
            .map(|k| tier(k).map_or([f32::NAN; N_COMPONENTS], |t| t.components))
            .collect();
        let score_slopes = (0..free.len())
            .map(|k| tier(k).map_or(f32::NAN, |t| t.score))
            .collect();
        Self {
            state,
            free,
            sensitivities,
            score_slopes,
            oracle_gain: label.oracle_gain.unwrap_or(f32::NAN),
        }
    }
}

/// Whether `label` teaches anything: some tier has a finite component or score slope, or the
/// oracle gain is finite. A state whose label has none of these is not worth a place in the
/// aggregate (the loss would skip every entry of it).
#[must_use]
pub fn label_is_informative(label: &TeacherLabel) -> bool {
    label.oracle_gain.is_some_and(f32::is_finite)
        || label
            .tiers
            .iter()
            .flatten()
            .any(|tier| tier.score.is_finite() || tier.components.iter().any(|c| c.is_finite()))
}

/// The aggregate of labelled states, oldest first, holding at most `cap` of them.
///
/// The six queues are parallel (one entry per state), in the layout of
/// [`indicatrix_ai::training::ImitationBatch`].
#[derive(Debug)]
pub struct LabelledStates {
    cap: usize,
    states: VecDeque<Vec<f32>>,
    n_tiers: VecDeque<usize>,
    free_tiers: VecDeque<Vec<bool>>,
    sensitivities: VecDeque<Vec<[f32; N_COMPONENTS]>>,
    score_slopes: VecDeque<Vec<f32>>,
    oracle_gain: VecDeque<f32>,
}

impl LabelledStates {
    /// An empty aggregate that keeps at most `cap` states (at least 1).
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            states: VecDeque::new(),
            n_tiers: VecDeque::new(),
            free_tiers: VecDeque::new(),
            sensitivities: VecDeque::new(),
            score_slopes: VecDeque::new(),
            oracle_gain: VecDeque::new(),
        }
    }

    /// States held now.
    #[must_use]
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// Whether no state is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// Adds `sample` as the newest state; past the capacity the oldest states leave.
    pub fn push(&mut self, sample: LabelledSample) {
        self.n_tiers.push_back(sample.free.len());
        self.states.push_back(sample.state);
        self.free_tiers.push_back(sample.free);
        self.sensitivities.push_back(sample.sensitivities);
        self.score_slopes.push_back(sample.score_slopes);
        self.oracle_gain.push_back(sample.oracle_gain);
        while self.states.len() > self.cap {
            self.states.pop_front();
            self.n_tiers.pop_front();
            self.free_tiers.pop_front();
            self.sensitivities.pop_front();
            self.score_slopes.pop_front();
            self.oracle_gain.pop_front();
        }
    }

    /// Drops every state.
    pub fn clear(&mut self) {
        self.states.clear();
        self.n_tiers.clear();
        self.free_tiers.clear();
        self.sensitivities.clear();
        self.score_slopes.clear();
        self.oracle_gain.clear();
    }

    /// The whole aggregate as a training batch, oldest state first. Nothing is copied: the
    /// queues are made contiguous in place and the batch borrows them.
    pub fn batch(&mut self) -> ImitationBatch<'_> {
        ImitationBatch {
            states: self.states.make_contiguous(),
            n_tiers: self.n_tiers.make_contiguous(),
            free_tiers: self.free_tiers.make_contiguous(),
            sensitivities: self.sensitivities.make_contiguous(),
            score_slopes: self.score_slopes.make_contiguous(),
            oracle_gain: self.oracle_gain.make_contiguous(),
        }
    }
}

/// The raw (pre-squash) action of an exploration step: `mean + std * N(0, 1)` on the free
/// tiers, the mean itself on the others.
///
/// A tier that is not free draws no random number, and with `std <= 0` nothing is drawn at all:
/// the result is the deterministic policy. This is written out instead of going through
/// `GaussianPolicy::sample_raw_masked` because that sampler clamps its log-std into the policy's
/// own range, which would turn a requested standard deviation of 0 into 0.05.
#[must_use]
pub fn explore_raw(mean: &[f32], free: &[bool], std: f32, rng: &mut Prng) -> Vec<f32> {
    mean.iter()
        .enumerate()
        .map(|(k, &m)| {
            if std > 0.0 && free.get(k).copied().unwrap_or(false) {
                std.mul_add(rng.standard_normal(), m)
            } else {
                m
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use indicatrix_ai::env::{GLOBAL_DIM, TIER_DIM, TierSensitivity, flat_len};

    use super::*;

    fn sensitivity(base: f32) -> TierSensitivity {
        TierSensitivity {
            components: [base, base + 1.0, base + 2.0, base + 3.0],
            score: base * 10.0,
            forward_probe: true,
        }
    }

    fn label(tiers: Vec<Option<TierSensitivity>>, gain: Option<f32>) -> TeacherLabel {
        TeacherLabel {
            tiers,
            oracle_gain: gain,
            oracle_scale: gain.map(|_| 1.0),
            evaluations: 0,
        }
    }

    /// A sample of a stone with `n` tiers whose state starts with `marker` and whose oracle
    /// gain is `marker`.
    fn sample(marker: f32, n: usize) -> LabelledSample {
        let mut state = vec![0.0; flat_len(4)];
        state[0] = marker;
        let tiers = (0..n).map(|k| Some(sensitivity(k as f32))).collect();
        LabelledSample::new(state, vec![true; n], &label(tiers, Some(marker)))
    }

    #[test]
    fn a_label_becomes_one_row_per_tier_with_nan_for_what_is_missing() {
        let tiers = vec![Some(sensitivity(1.0)), None, Some(sensitivity(3.0))];
        let s = LabelledSample::new(vec![0.5; 4], vec![true, false, true], &label(tiers, None));
        assert_eq!(s.sensitivities.len(), 3);
        assert_eq!(s.score_slopes.len(), 3);
        assert_eq!(s.sensitivities[0], [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(s.score_slopes[0], 10.0);
        // A tier without a label is all NaN, with a NaN score slope.
        assert!(s.sensitivities[1].iter().all(|v| v.is_nan()));
        assert!(s.score_slopes[1].is_nan());
        assert_eq!(s.sensitivities[2], [3.0, 4.0, 5.0, 6.0]);
        // No oracle gain is NaN as well.
        assert!(s.oracle_gain.is_nan());
        assert_eq!(s.free, [true, false, true]);
    }

    #[test]
    fn a_yield_that_cannot_be_measured_stays_a_nan_entry_of_a_labelled_tier() {
        let mut yieldless = sensitivity(2.0);
        yieldless.components[3] = f32::NAN;
        let s = LabelledSample::new(
            vec![0.0; 2],
            vec![true],
            &label(vec![Some(yieldless)], Some(0.0)),
        );
        assert_eq!(s.sensitivities[0][..3], [2.0, 3.0, 4.0]);
        assert!(s.sensitivities[0][3].is_nan());
        assert_eq!(s.oracle_gain, 0.0);
    }

    #[test]
    fn the_rows_follow_the_free_mask_not_the_labels_length() {
        // A shorter label leaves the extra tiers unlabelled; a longer one is cut.
        let short = LabelledSample::new(
            vec![0.0],
            vec![true; 3],
            &label(vec![Some(sensitivity(1.0))], None),
        );
        assert_eq!(short.sensitivities.len(), 3);
        assert!(short.score_slopes[1].is_nan() && short.score_slopes[2].is_nan());
        let long = LabelledSample::new(
            vec![0.0],
            vec![true; 1],
            &label(vec![Some(sensitivity(1.0)), Some(sensitivity(2.0))], None),
        );
        assert_eq!(long.sensitivities.len(), 1);
    }

    #[test]
    fn a_label_is_informative_with_any_finite_slope_or_gain() {
        assert!(!label_is_informative(&label(vec![None, None], None)));
        assert!(!label_is_informative(&label(Vec::new(), None)));
        assert!(label_is_informative(&label(vec![None], Some(0.0))));
        assert!(label_is_informative(&label(
            vec![Some(sensitivity(1.0))],
            None
        )));
        // A gain that is not a number teaches nothing.
        assert!(!label_is_informative(&label(vec![None], Some(f32::NAN))));
        // Nor does a tier whose every figure is NaN.
        let mut blank = sensitivity(1.0);
        blank.components = [f32::NAN; N_COMPONENTS];
        blank.score = f32::NAN;
        assert!(!label_is_informative(&label(vec![Some(blank)], None)));
        // A finite score slope alone is enough: the direction cosine reads it.
        let mut score_only = blank;
        score_only.score = 2.0;
        assert!(label_is_informative(&label(vec![Some(score_only)], None)));
        // So is a single finite component.
        let mut one = blank;
        one.components[1] = 0.5;
        assert!(label_is_informative(&label(vec![Some(one)], None)));
    }

    #[test]
    fn the_oldest_states_leave_first_once_the_capacity_is_reached() {
        let mut aggregate = LabelledStates::new(3);
        assert!(aggregate.is_empty());
        for marker in 0..5_usize {
            aggregate.push(sample(marker as f32, 2));
            assert_eq!(aggregate.len(), (marker + 1).min(3));
        }
        let batch = aggregate.batch();
        // The states 2, 3 and 4 remain, oldest first, in every parallel queue.
        assert_eq!(batch.oracle_gain, [2.0, 3.0, 4.0]);
        let firsts: Vec<f32> = batch.states.iter().map(|s| s[0]).collect();
        assert_eq!(firsts, [2.0, 3.0, 4.0]);
        assert_eq!(batch.n_tiers, [2, 2, 2]);
        assert_eq!(batch.free_tiers.len(), 3);
        assert_eq!(batch.sensitivities.len(), 3);
        assert_eq!(batch.score_slopes.len(), 3);
    }

    #[test]
    fn a_capacity_of_zero_still_keeps_the_newest_state() {
        let mut aggregate = LabelledStates::new(0);
        aggregate.push(sample(1.0, 1));
        aggregate.push(sample(2.0, 1));
        assert_eq!(aggregate.len(), 1);
        assert_eq!(aggregate.batch().oracle_gain, [2.0]);
    }

    #[test]
    fn the_batch_has_matching_lengths_and_passes_the_trainers_own_validation() {
        let mut aggregate = LabelledStates::new(10);
        for (marker, n) in [(0.0, 1), (1.0, 3), (2.0, 4), (3.0, 2)] {
            aggregate.push(sample(marker, n));
        }
        let batch = aggregate.batch();
        assert_eq!(batch.len(), 4);
        assert!(!batch.is_empty());
        assert_eq!(batch.n_tiers, [1, 3, 4, 2]);
        for i in 0..batch.len() {
            assert_eq!(batch.free_tiers[i].len(), batch.n_tiers[i]);
            assert_eq!(batch.sensitivities[i].len(), batch.n_tiers[i]);
            assert_eq!(batch.score_slopes[i].len(), batch.n_tiers[i]);
        }
        assert_eq!(batch.states[0].len(), flat_len(4));
        batch
            .validate(GLOBAL_DIM, TIER_DIM, 4)
            .expect("the aggregate is a valid imitation batch");
    }

    #[test]
    fn a_wrapped_queue_still_yields_one_contiguous_batch_in_order() {
        // Evict and push repeatedly so the ring buffers wrap around.
        let mut aggregate = LabelledStates::new(4);
        for marker in 0..11 {
            aggregate.push(sample(marker as f32, 2));
            let _ = aggregate.batch();
        }
        assert_eq!(aggregate.batch().oracle_gain, [7.0, 8.0, 9.0, 10.0]);
    }

    #[test]
    fn clearing_empties_every_queue() {
        let mut aggregate = LabelledStates::new(5);
        aggregate.push(sample(1.0, 2));
        aggregate.push(sample(2.0, 2));
        aggregate.clear();
        assert!(aggregate.is_empty());
        let batch = aggregate.batch();
        assert!(batch.is_empty());
        assert!(batch.n_tiers.is_empty() && batch.oracle_gain.is_empty());
        // It can be refilled afterwards.
        aggregate.push(sample(3.0, 1));
        assert_eq!(aggregate.len(), 1);
    }

    #[test]
    fn exploration_without_noise_is_the_deterministic_policy_and_draws_nothing() {
        let mean = [0.5, -1.0, 0.25];
        let mut rng = Prng::new(9);
        let before = rng.clone();
        assert_eq!(explore_raw(&mean, &[true; 3], 0.0, &mut rng), mean);
        assert_eq!(explore_raw(&mean, &[true; 3], -1.0, &mut rng), mean);
        assert_eq!(rng, before, "no random number may be drawn");
    }

    #[test]
    fn exploration_noise_is_reproducible_and_skips_tiers_that_are_not_free() {
        let mean = [0.5, -1.0, 0.25, 1.0];
        let free = [true, false, true, false];
        let a = explore_raw(&mean, &free, 0.3, &mut Prng::new(4));
        let b = explore_raw(&mean, &free, 0.3, &mut Prng::new(4));
        assert_eq!(a, b);
        assert_ne!(a[0], mean[0]);
        assert_ne!(a[2], mean[2]);
        assert_eq!((a[1], a[3]), (mean[1], mean[3]));
        // Tiers that are not free draw nothing: the free tiers see the same numbers as in a
        // stream that never met the others.
        let mut reference = Prng::new(4);
        assert_eq!(a[0], 0.3f32.mul_add(reference.standard_normal(), mean[0]));
        assert_eq!(a[2], 0.3f32.mul_add(reference.standard_normal(), mean[2]));
        // A mask shorter than the mean marks the missing tiers as not free.
        let short = explore_raw(&mean, &[true], 0.3, &mut Prng::new(4));
        assert_eq!(short[0], a[0]);
        assert_eq!(short[1..], mean[1..]);
    }

    #[test]
    fn exploration_noise_has_the_requested_spread() {
        let mut rng = Prng::new(11);
        let n = 20_000;
        let mean = vec![0.0; n];
        let draws = explore_raw(&mean, &vec![true; n], 0.3, &mut rng);
        let average = draws.iter().sum::<f32>() / n as f32;
        let variance = draws.iter().map(|d| (d - average).powi(2)).sum::<f32>() / n as f32;
        assert!(average.abs() < 0.01, "{average}");
        assert!((variance.sqrt() - 0.3).abs() < 0.01, "{}", variance.sqrt());
    }
}
