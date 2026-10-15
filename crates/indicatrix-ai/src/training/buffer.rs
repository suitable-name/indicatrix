//! Experience rollout buffers, reward normalisation and Generalized Advantage Estimation (GAE).

use serde::{Deserialize, Serialize};

use super::TrainError;

/// Magnitude a scaled reward is clipped to before GAE. With the scale from
/// [`ReturnNormalizer::scale`] a typical reward is O(1), so 10 only ever cuts a rare outlier (a
/// step whose reward is ten return-standard-deviations away), and no single step can dominate a
/// batch.
pub const SCALED_REWARD_CLIP: f32 = 10.0;

/// Running scale of the discounted return: the reward normaliser of the usual PPO implementations
/// (baselines `VecNormalize`, SB3 `normalize_reward`), review TQ-15.
///
/// The un-normalised returns of the first two runs sat around -150 (about 40 steps of -4.6 at
/// gamma 0.99), so the critic's squared error (100 to 2200) outweighed the policy loss by two to
/// three orders of magnitude. The scale is `1 / std` of the *discounted return* `ret_w = gamma *
/// ret_w + reward`, accumulated per worker and reset at every episode end; the variance is a
/// Welford running estimate over every tracked step of the run, so it persists across iterations
/// and settles instead of jumping with each rollout. Rewards are only divided by it (nothing is
/// subtracted), which keeps the sign and the ordering of every reward.
///
/// The scale is not part of a checkpoint (review TQ-21): a resumed run re-estimates it, and its
/// first update already uses the estimate of its own first rollout. Until two steps are tracked
/// the scale is 1, which the clip at [`SCALED_REWARD_CLIP`] keeps safe.
#[derive(Debug, Clone, Default)]
pub struct ReturnNormalizer {
    /// Discounted return of each worker's running episode (grown on demand).
    returns: Vec<f64>,
    /// Number of tracked steps.
    count: u64,
    /// Running mean of the tracked discounted returns.
    mean: f64,
    /// Running sum of squared deviations from the mean (Welford).
    m2: f64,
}

impl ReturnNormalizer {
    /// An empty normaliser: scale 1 until two steps were tracked.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one step of worker `worker`: `ret_w = gamma * ret_w + reward`, then updates the
    /// running variance with `ret_w`; `ended` resets `ret_w` to 0 afterwards.
    ///
    /// A non-finite reward or discount is not folded in (it would poison the variance for the
    /// rest of the run; the GAE pass reports the bad reward as an error), but `ended` still
    /// resets the worker.
    pub fn track(&mut self, worker: usize, reward: f32, gamma: f32, ended: bool) {
        if self.returns.len() <= worker {
            self.returns.resize(worker + 1, 0.0);
        }
        let ret = &mut self.returns[worker];
        if reward.is_finite() && gamma.is_finite() {
            *ret = f64::from(gamma).mul_add(*ret, f64::from(reward));
            self.count += 1;
            let delta = *ret - self.mean;
            self.mean += delta / self.count as f64;
            self.m2 += delta * (*ret - self.mean);
        }
        if ended {
            *ret = 0.0;
        }
    }

    /// `1 / sqrt(var + 1e-8)` of the tracked discounted returns, or `1.0` until at least two
    /// steps were tracked. Multiply rewards by it before [`RolloutBuffer::compute_gae_per_worker`].
    #[must_use]
    pub fn scale(&self) -> f32 {
        if self.count < 2 {
            return 1.0;
        }
        let var = (self.m2 / self.count as f64).max(0.0);
        let scale = 1.0 / (var + 1e-8).sqrt();
        if scale.is_finite() { scale as f32 } else { 1.0 }
    }

    /// Forgets everything (used when the task, the model or the worker set changes).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Number of steps folded in so far.
    #[must_use]
    pub const fn samples(&self) -> u64 {
        self.count
    }
}

/// Population variance of `values` (0 for an empty slice).
fn population_variance(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n
}

/// Single trajectory step stored in the experience buffer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrajectoryStep {
    /// Flattened observation vector.
    pub observation: Vec<f32>,
    /// Continuous action deltas applied.
    pub action: Vec<f64>,
    /// Step reward scalar.
    pub reward: f32,
    /// Critic state-value prediction `V(s)`.
    pub value: f32,
    /// Action log probability under sampling policy.
    pub log_prob: f32,
}

/// How the episode of a worker continues after a stored step.
///
/// GAE must bootstrap differently for the three cases: a terminated episode has no future value, a
/// time-limit truncation still has one (`V(s_next)`), and a continuing episode takes the value of the
/// worker's next stored step (or the rollout-tail value for its last step).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum EpisodeEnd {
    /// The episode continues with the worker's next step.
    Continue,
    /// The environment terminated: the successor value is exactly zero.
    Terminated,
    /// The step limit cut the episode: bootstrap from the critic's value of the final state.
    Truncated {
        /// `V(s_next)` of the state reached by the truncating step.
        next_value: f32,
    },
}

/// Rollout buffer collecting trajectories across multiple episodes and environments.
#[derive(Debug, Clone, Default)]
pub struct RolloutBuffer {
    /// List of collected trajectory steps.
    pub steps: Vec<TrajectoryStep>,
    /// Worker (environment slot) that produced each step; parallel to `steps`.
    pub worker_ids: Vec<usize>,
    /// Episode continuation of each step; parallel to `steps`.
    pub ends: Vec<EpisodeEnd>,
    /// Computed Generalized Advantage Estimation (GAE) per step.
    pub advantages: Vec<f32>,
    /// Target discounted returns `R_t` for critic regression.
    pub returns: Vec<f32>,
}

impl RolloutBuffer {
    /// Create a new empty rollout buffer with reserved capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            steps: Vec::with_capacity(capacity),
            worker_ids: Vec::with_capacity(capacity),
            ends: Vec::with_capacity(capacity),
            advantages: Vec::with_capacity(capacity),
            returns: Vec::with_capacity(capacity),
        }
    }

    /// Add a step produced by environment slot `worker_id`, in that worker's own time order.
    ///
    /// Steps of different workers may be pushed interleaved; [`Self::compute_gae_per_worker`]
    /// separates them again.
    pub fn push_worker_step(&mut self, worker_id: usize, step: TrajectoryStep, end: EpisodeEnd) {
        self.steps.push(step);
        self.worker_ids.push(worker_id);
        self.ends.push(end);
    }

    /// Number of stored steps.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether the buffer holds no steps.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Clear all stored steps and computed advantages.
    pub fn clear(&mut self) {
        self.steps.clear();
        self.worker_ids.clear();
        self.ends.clear();
        self.advantages.clear();
        self.returns.clear();
    }

    /// Compute GAE separately for every worker, then standardise the advantages over the whole buffer.
    ///
    /// Each worker's steps are walked backwards in their own order, so a step never bootstraps from a
    /// different worker's value. Per step the stored [`EpisodeEnd`] decides the bootstrap: `Terminated`
    /// uses zero, `Truncated` uses its `next_value`, `Continue` uses the worker's next stored value (or
    /// `last_values[worker]` for the worker's final step). The GAE trace is cut at every episode end.
    ///
    /// `last_values[w]` is the critic's value of worker `w`'s current state after its last stored step;
    /// it is only read when that last step is `Continue`.
    ///
    /// `reward_scale` (from [`ReturnNormalizer::scale`]; pass `1.0` for raw rewards) multiplies every
    /// stored reward before the deltas, and the scaled reward is clipped to `+-`[`SCALED_REWARD_CLIP`].
    /// The critic's values, the bootstrap values and the resulting returns are therefore in scaled
    /// units, and `returns` is what the critic regresses on. The stored `steps[].reward` stay raw.
    ///
    /// # Errors
    ///
    /// [`TrainError::InvalidConfig`] when `reward_scale` is not finite and positive,
    /// [`TrainError::ShapeMismatch`] when `last_values` does not cover every worker id in the buffer,
    /// [`TrainError::NonFinite`] when a stored reward, value or bootstrap value is NaN or infinite.
    pub fn compute_gae_per_worker(
        &mut self,
        last_values: &[f32],
        gamma: f32,
        lambda: f32,
        reward_scale: f32,
    ) -> Result<(), TrainError> {
        if !(reward_scale.is_finite() && reward_scale > 0.0) {
            return Err(TrainError::InvalidConfig(format!(
                "reward_scale must be finite and > 0 (got {reward_scale})"
            )));
        }
        let n = self.steps.len();
        let n_workers = self.worker_ids.iter().copied().max().map_or(0, |m| m + 1);
        if last_values.len() < n_workers {
            return Err(TrainError::ShapeMismatch(format!(
                "last_values has {} entries but the buffer holds steps of {n_workers} workers",
                last_values.len()
            )));
        }
        if self.ends.len() != n || self.worker_ids.len() != n {
            return Err(TrainError::ShapeMismatch(
                "worker_ids / ends are not parallel to steps".into(),
            ));
        }

        let mut per_worker: Vec<Vec<usize>> = vec![Vec::new(); n_workers];
        for (i, &w) in self.worker_ids.iter().enumerate() {
            per_worker[w].push(i);
        }

        self.advantages = vec![0.0; n];
        self.returns = vec![0.0; n];

        for (w, list) in per_worker.iter().enumerate() {
            let mut gae = 0.0f32;
            for (pos, &t) in list.iter().enumerate().rev() {
                let step = &self.steps[t];
                if !step.reward.is_finite() || !step.value.is_finite() {
                    return Err(TrainError::NonFinite(format!(
                        "worker {w} step {pos}: reward {} / value {}",
                        step.reward, step.value
                    )));
                }
                let (next_val, trace) = match self.ends[t] {
                    EpisodeEnd::Terminated => (0.0, 0.0),
                    EpisodeEnd::Truncated { next_value } => (next_value, 0.0),
                    EpisodeEnd::Continue => {
                        let nv = match list.get(pos + 1) {
                            Some(&next) => self.steps[next].value,
                            None => last_values[w],
                        };
                        (nv, 1.0)
                    }
                };
                if !next_val.is_finite() {
                    return Err(TrainError::NonFinite(format!(
                        "worker {w} step {pos}: bootstrap value {next_val}"
                    )));
                }
                let reward =
                    (step.reward * reward_scale).clamp(-SCALED_REWARD_CLIP, SCALED_REWARD_CLIP);
                let delta = gamma.mul_add(next_val, reward) - step.value;
                gae = (gamma * lambda * trace).mul_add(gae, delta);
                self.advantages[t] = gae;
                self.returns[t] = gae + step.value;
            }
        }

        self.standardize_advantages();
        Ok(())
    }

    /// Explained variance of the critic over the buffer, `1 - Var(returns - values) / Var(returns)`
    /// (1 = perfect critic, 0 = no better than predicting the mean return, negative = worse).
    ///
    /// Call after [`Self::compute_gae_per_worker`]; the values are the stored `steps[].value`.
    /// `None` with fewer than two steps, when `returns` is not parallel to the steps, or when the
    /// returns have no variance (the ratio is undefined). The two runs of the review recorded a
    /// value loss of 100 to 2200 and no way to tell whether the critic learned anything; this is
    /// the number that answers it (review TQ-15).
    #[must_use]
    pub fn explained_variance(&self) -> Option<f32> {
        let n = self.steps.len();
        if n < 2 || self.returns.len() != n {
            return None;
        }
        let returns: Vec<f64> = self.returns.iter().map(|&r| f64::from(r)).collect();
        let residual: Vec<f64> = self
            .steps
            .iter()
            .zip(&returns)
            .map(|(step, &r)| r - f64::from(step.value))
            .collect();
        let var_returns = population_variance(&returns);
        (var_returns > 1e-12).then(|| (1.0 - population_variance(&residual) / var_returns) as f32)
    }

    /// Standardise advantages to zero mean and unit variance for training stability.
    fn standardize_advantages(&mut self) {
        let n = self.advantages.len();
        if n > 1 {
            let mean = self.advantages.iter().sum::<f32>() / n as f32;
            let var = self
                .advantages
                .iter()
                .map(|&a| (a - mean).powi(2))
                .sum::<f32>()
                / n as f32;
            let std = (var + 1e-8).sqrt();
            for a in &mut self.advantages {
                *a = (*a - mean) / std;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(reward: f32, value: f32) -> TrajectoryStep {
        TrajectoryStep {
            observation: Vec::new(),
            action: Vec::new(),
            reward,
            value,
            log_prob: 0.0,
        }
    }

    /// Un-normalised GAE of one worker, computed independently by the textbook recursion.
    fn reference(
        steps: &[(f32, f32, EpisodeEnd)],
        last: f32,
        gamma: f32,
        lambda: f32,
    ) -> (Vec<f32>, Vec<f32>) {
        let n = steps.len();
        let mut adv = vec![0.0; n];
        let mut ret = vec![0.0; n];
        let mut gae = 0.0;
        for t in (0..n).rev() {
            let (r, v, end) = steps[t];
            let (nv, trace) = match end {
                EpisodeEnd::Terminated => (0.0, 0.0),
                EpisodeEnd::Truncated { next_value } => (next_value, 0.0),
                EpisodeEnd::Continue => (if t + 1 < n { steps[t + 1].1 } else { last }, 1.0),
            };
            gae = (gamma * lambda * trace).mul_add(gae, gamma.mul_add(nv, r) - v);
            adv[t] = gae;
            ret[t] = gae + v;
        }
        (adv, ret)
    }

    #[test]
    fn interleaved_workers_equal_their_sequential_results() {
        let (gamma, lambda) = (0.9, 0.8);
        // Worker 0: continues, terminates at step 2, then a fresh episode of one step.
        let w0 = [
            (1.0, 0.5, EpisodeEnd::Continue),
            (0.5, 0.4, EpisodeEnd::Continue),
            (2.0, 0.3, EpisodeEnd::Terminated),
            (0.7, 0.2, EpisodeEnd::Continue),
        ];
        // Worker 1: truncated at step 1 (bootstraps from 0.9), then continues.
        let w1 = [
            (-1.0, 0.1, EpisodeEnd::Continue),
            (0.25, 0.6, EpisodeEnd::Truncated { next_value: 0.9 }),
            (0.3, 0.7, EpisodeEnd::Continue),
        ];
        let last = [0.45, 0.35];

        let mut buffer = RolloutBuffer::with_capacity(8);
        // Interleave round-robin: w0[0], w1[0], w0[1], w1[1], ...
        let mut order = Vec::new();
        for i in 0..4 {
            if i < w0.len() {
                order.push((0usize, w0[i]));
            }
            if i < w1.len() {
                order.push((1usize, w1[i]));
            }
        }
        for &(w, (r, v, end)) in &order {
            buffer.push_worker_step(w, step(r, v), end);
        }
        buffer
            .compute_gae_per_worker(&last, gamma, lambda, 1.0)
            .unwrap();

        let (a0, r0) = reference(&w0, last[0], gamma, lambda);
        let (a1, r1) = reference(&w1, last[1], gamma, lambda);

        // Returns are not standardised, so they must match exactly; advantages match up to the shared
        // affine standardisation, so compare the un-standardised quantity via returns - value.
        let mut idx = [0usize, 0usize];
        for (i, &(w, (_, v, _))) in order.iter().enumerate() {
            let k = idx[w];
            idx[w] += 1;
            let (want_ret, want_adv) = if w == 0 {
                (r0[k], a0[k])
            } else {
                (r1[k], a1[k])
            };
            assert!(
                (buffer.returns[i] - want_ret).abs() < 1e-5,
                "return of step {i}"
            );
            assert!(
                (want_adv - (buffer.returns[i] - v)).abs() < 1e-5,
                "advantage of step {i}"
            );
        }

        // Standardised advantages: zero mean, unit variance.
        let n = buffer.advantages.len() as f32;
        let mean = buffer.advantages.iter().sum::<f32>() / n;
        let var = buffer
            .advantages
            .iter()
            .map(|a| (a - mean).powi(2))
            .sum::<f32>()
            / n;
        assert!(mean.abs() < 1e-5);
        assert!((var - 1.0).abs() < 1e-3);
    }

    #[test]
    fn truncated_bootstraps_and_terminated_does_not() {
        let mut a = RolloutBuffer::default();
        a.push_worker_step(
            0,
            step(1.0, 0.0),
            EpisodeEnd::Truncated { next_value: 10.0 },
        );
        a.push_worker_step(0, step(0.0, 0.0), EpisodeEnd::Terminated);
        a.compute_gae_per_worker(&[0.0], 1.0, 1.0, 1.0).unwrap();
        // Truncated step: return = 1 + 1 * 10; terminated step: return = 0.
        assert!((a.returns[0] - 11.0).abs() < 1e-6);
        assert!(a.returns[1].abs() < 1e-6);

        let mut b = RolloutBuffer::default();
        b.push_worker_step(0, step(1.0, 0.0), EpisodeEnd::Terminated);
        b.compute_gae_per_worker(&[10.0], 1.0, 1.0, 1.0).unwrap();
        assert!(
            (b.returns[0] - 1.0).abs() < 1e-6,
            "terminated must ignore the tail value"
        );
    }

    #[test]
    fn single_worker_gae_standardises_advantages() {
        let mut buffer = RolloutBuffer::with_capacity(10);
        for i in 0..5 {
            let end = if i == 4 {
                EpisodeEnd::Terminated
            } else {
                EpisodeEnd::Continue
            };
            buffer.push_worker_step(0, step(1.0, 0.5), end);
        }
        buffer
            .compute_gae_per_worker(&[0.0], 0.99, 0.95, 1.0)
            .unwrap();
        assert_eq!(buffer.advantages.len(), 5);
        assert_eq!(buffer.returns.len(), 5);
        let n = 5.0f32;
        let mean = buffer.advantages.iter().sum::<f32>() / n;
        let var = buffer
            .advantages
            .iter()
            .map(|a| (a - mean).powi(2))
            .sum::<f32>()
            / n;
        assert!(mean.abs() < 1e-5);
        assert!((var - 1.0).abs() < 1e-3);
        // The terminated last step has return = reward.
        assert!((buffer.returns[4] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn missing_tail_values_and_non_finite_inputs_are_errors() {
        let mut buffer = RolloutBuffer::default();
        buffer.push_worker_step(1, step(1.0, 0.0), EpisodeEnd::Continue);
        assert!(matches!(
            buffer.compute_gae_per_worker(&[0.0], 0.99, 0.95, 1.0),
            Err(TrainError::ShapeMismatch(_))
        ));

        let mut buffer = RolloutBuffer::default();
        buffer.push_worker_step(0, step(f32::NAN, 0.0), EpisodeEnd::Continue);
        assert!(matches!(
            buffer.compute_gae_per_worker(&[0.0], 0.99, 0.95, 1.0),
            Err(TrainError::NonFinite(_))
        ));
    }

    #[test]
    fn scaled_rewards_change_returns_by_the_scale_and_are_clipped() {
        // gamma = lambda = 1 and a zero critic: a step's return is the sum of the scaled rewards
        // from there to the episode end.
        let mut buffer = RolloutBuffer::default();
        buffer.push_worker_step(0, step(1.0, 0.0), EpisodeEnd::Continue);
        buffer.push_worker_step(0, step(2.0, 0.0), EpisodeEnd::Terminated);
        buffer
            .compute_gae_per_worker(&[0.0], 1.0, 1.0, 2.0)
            .unwrap();
        // Scaled rewards 2 and 4: returns 6 and 4 (3 and 2 unscaled).
        assert!((buffer.returns[0] - 6.0).abs() < 1e-6);
        assert!((buffer.returns[1] - 4.0).abs() < 1e-6);
        // The stored rewards stay raw.
        assert!((buffer.steps[0].reward - 1.0).abs() < 1e-9);
        assert!((buffer.steps[1].reward - 2.0).abs() < 1e-9);

        // Scale 7: 3 -> 21 clipped to 10, -3 -> -21 clipped to -10, 0.5 -> 3.5 untouched.
        let mut clipped = RolloutBuffer::default();
        clipped.push_worker_step(0, step(3.0, 0.0), EpisodeEnd::Terminated);
        clipped.push_worker_step(1, step(-3.0, 0.0), EpisodeEnd::Terminated);
        clipped.push_worker_step(2, step(0.5, 0.0), EpisodeEnd::Terminated);
        clipped
            .compute_gae_per_worker(&[0.0, 0.0, 0.0], 0.99, 0.95, 7.0)
            .unwrap();
        assert!((clipped.returns[0] - SCALED_REWARD_CLIP).abs() < 1e-6);
        assert!((clipped.returns[1] + SCALED_REWARD_CLIP).abs() < 1e-6);
        assert!((clipped.returns[2] - 3.5).abs() < 1e-6);

        // A scale that is not a positive number is refused instead of silently flipping signs.
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(matches!(
                buffer.compute_gae_per_worker(&[0.0], 1.0, 1.0, bad),
                Err(TrainError::InvalidConfig(_))
            ));
        }
    }

    #[test]
    fn normalizer_scale_is_one_until_two_samples_then_inverse_std() {
        let mut normalizer = ReturnNormalizer::new();
        assert!((normalizer.scale() - 1.0).abs() < 1e-9);
        // Three unit rewards at gamma 0.5 give the discounted returns 1, 3/2 and 7/4: mean 17/12,
        // squared deviations 25/144, 1/144 and 16/144, population variance 7/72.
        normalizer.track(0, 1.0, 0.5, false);
        assert_eq!(normalizer.samples(), 1);
        assert!((normalizer.scale() - 1.0).abs() < 1e-9, "one sample");
        normalizer.track(0, 1.0, 0.5, false);
        normalizer.track(0, 1.0, 0.5, false);
        assert_eq!(normalizer.samples(), 3);
        let want = (72.0f32 / 7.0).sqrt();
        assert!(
            (normalizer.scale() - want).abs() < 1e-3,
            "{}",
            normalizer.scale()
        );

        // An episode end restarts the worker's accumulator and workers do not mix: the tracked
        // returns are 1 (worker 0, ended), 2 (worker 0, fresh episode) and 1 (worker 1, not 2.5).
        // Mean 4/3, squared deviations 1/9, 4/9 and 1/9, population variance 2/9.
        let mut episodes = ReturnNormalizer::new();
        episodes.track(0, 1.0, 0.5, true);
        episodes.track(0, 2.0, 0.5, false);
        episodes.track(1, 1.0, 0.5, false);
        assert!((episodes.scale() - 4.5f32.sqrt()).abs() < 1e-3);

        // A non-finite reward is not folded in, and reset forgets everything.
        episodes.track(0, f32::NAN, 0.5, false);
        assert_eq!(episodes.samples(), 3);
        episodes.reset();
        assert_eq!(episodes.samples(), 0);
        assert!((episodes.scale() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn explained_variance_is_one_for_a_perfect_critic_and_zero_for_the_mean() {
        // Returns 1..4 have population variance 1.25.
        let returns = [1.0f32, 2.0, 3.0, 4.0];
        let with_values = |values: [f32; 4]| {
            let mut buffer = RolloutBuffer::default();
            for value in values {
                buffer.push_worker_step(0, step(0.0, value), EpisodeEnd::Continue);
            }
            buffer.returns = returns.to_vec();
            buffer
        };

        // value == return: the residual is zero.
        let perfect = with_values(returns).explained_variance().unwrap();
        assert!((perfect - 1.0).abs() < 1e-6);
        // value == mean return (2.5): the residual is return - 2.5, the same variance as the returns.
        let mean_critic = with_values([2.5; 4]).explained_variance().unwrap();
        assert!(mean_critic.abs() < 1e-6);
        // value == -return: the residual 2 * return has four times the variance, so 1 - 4 = -3.
        let wrong = with_values([-1.0, -2.0, -3.0, -4.0])
            .explained_variance()
            .unwrap();
        assert!((wrong + 3.0).abs() < 1e-5);

        // Undefined: a single step, and returns without variance.
        let mut single = RolloutBuffer::default();
        single.push_worker_step(0, step(0.0, 1.0), EpisodeEnd::Terminated);
        single.returns = vec![1.0];
        assert!(single.explained_variance().is_none());
        let mut constant = with_values([0.0; 4]);
        constant.returns = vec![5.0; 4];
        assert!(constant.explained_variance().is_none());
    }
}
