//! A rolling, regression-based time-remaining estimator for a long-running progress
//! bar (a still-image export job or a tilt-video frame sweep), plus
//! [`format_eta`] to turn its result into the short string those bars show next to
//! their percentage.
//!
//! # Why regression over a window, not "seconds since last tick / delta fraction"
//!
//! The wire progress a running job reports is step-like, not smooth: a fan-out
//! export job's own progress advances in coordinator-sized chunks (each merge can be
//! tens of seconds apart), and a tilt video only reports once per completed frame.
//! An estimator built from the single most recent pair of observations would swing
//! wildly between updates -- near-zero right after a chunk lands (the "instantaneous
//! rate" looks huge), then rising sharply as the next chunk takes its full time to
//! arrive. Fitting a least-squares line across a whole rolling window of
//! `(time, fraction_done)` observations instead smooths over that noise: a handful of
//! irregular steps still describe a fairly steady average rate, which is what a
//! cutter actually wants from a "how much longer" readout.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Observations older than this are dropped once the window also has more than
/// [`MAX_OBSERVATIONS`] entries -- see [`EtaEstimator::observe`]'s own doc comment.
const MIN_WINDOW: Duration = Duration::from_secs(30);

/// Hard cap on how many observations [`EtaEstimator`] keeps, once the 30-second
/// window (`MIN_WINDOW`) is already satisfied -- bounds the regression's own cost and
/// stops a very long, very chatty run from growing this unboundedly.
const MAX_OBSERVATIONS: usize = 64;

/// [`EtaEstimator::eta`] refuses to guess from less than this much observed history
/// -- a five-second span is enough for even a single coordinator chunk landing twice
/// to give a slope that means something, whereas two observations a fraction of a
/// second apart (e.g. two ticks of the same batch) would extrapolate a noise-sized
/// slope into a wildly wrong estimate.
const MIN_SPAN_FOR_ESTIMATE: Duration = Duration::from_secs(5);

/// A rolling window of `(time, fraction_done)` observations for one running job,
/// used to fit a least-squares line and read off "how much longer until
/// `fraction_done` reaches 1.0". See this module's own doc comment for why a
/// regression over a window, rather than a single instantaneous rate.
///
/// Feed it with [`observe`](Self::observe) on every progress tick, read
/// [`eta`](Self::eta) whenever the UI needs a fresh estimate, and
/// [`reset`](Self::reset) it when a new, unrelated run starts (a fresh export job or
/// video sweep) -- carrying a previous run's observations into a new one would fit a
/// line across two unrelated rates.
#[derive(Debug, Default, Clone)]
pub struct EtaEstimator {
    observations: VecDeque<(Instant, f64)>,
}

impl EtaEstimator {
    /// Records one `(now, fraction_done)` observation, trimming the window
    /// afterward.
    ///
    /// The window keeps AT LEAST the last [`MIN_WINDOW`] (30s) of history
    /// regardless of how many observations that takes, and separately caps at
    /// [`MAX_OBSERVATIONS`] (64) once that 30-second requirement is already met --
    /// so a job reporting many ticks per second still bounds this struct's memory,
    /// while a job reporting only a handful of ticks total (a fan-out export's
    /// coordinator-chunk cadence) still keeps enough history to fit a meaningful
    /// line.
    pub fn observe(&mut self, now: Instant, fraction_done: f64) {
        self.observations
            .push_back((now, fraction_done.clamp(0.0, 1.0)));
        while self.observations.len() > MAX_OBSERVATIONS {
            let Some(&(oldest, _)) = self.observations.front() else {
                break;
            };
            if now.saturating_duration_since(oldest) <= MIN_WINDOW {
                // Still inside the required 30-second window -- keep it even
                // though the count is already over the cap.
                break;
            }
            self.observations.pop_front();
        }
    }

    /// Fits a least-squares line across the current window and reads off the
    /// remaining time until it reaches `fraction_done == 1.0`, evaluated at `now`
    /// (not necessarily the time of the last observation -- the caller may ask for
    /// an estimate between ticks).
    ///
    /// Returns `None` when there isn't enough history to trust a fit yet (fewer
    /// than two observations, or a span under [`MIN_SPAN_FOR_ESTIMATE`]), when the
    /// window's own timestamps are all identical (a zero-variance fit has no
    /// slope to read), or when the fitted slope is zero or negative -- a stalled
    /// or regressing job has no forward rate to project from, so no number is
    /// better than a wrong one.
    #[must_use]
    pub fn eta(&self, now: Instant) -> Option<Duration> {
        let &(first_time, _) = self.observations.front()?;
        let &(last_time, _) = self.observations.back()?;
        let span = last_time.duration_since(first_time);
        if self.observations.len() < 2 || span < MIN_SPAN_FOR_ESTIMATE {
            return None;
        }

        let xs_ys: Vec<(f64, f64)> = self
            .observations
            .iter()
            .map(|&(t, f)| (t.duration_since(first_time).as_secs_f64(), f))
            .collect();
        let count = xs_ys.len() as f64;
        let mean_x = xs_ys.iter().map(|&(x, _)| x).sum::<f64>() / count;
        let mean_y = xs_ys.iter().map(|&(_, y)| y).sum::<f64>() / count;

        let mut covariance = 0.0;
        let mut variance = 0.0;
        for &(x, y) in &xs_ys {
            let dx = x - mean_x;
            covariance = dx.mul_add(y - mean_y, covariance);
            variance = dx.mul_add(dx, variance);
        }
        if variance <= f64::EPSILON {
            return None;
        }
        let slope = covariance / variance;
        if slope <= 0.0 {
            return None;
        }

        let intercept = slope.mul_add(-mean_x, mean_y);
        let now_x = now.duration_since(first_time).as_secs_f64();
        let fitted_now = slope.mul_add(now_x, intercept).max(0.0);
        let remaining_fraction = (1.0 - fitted_now).max(0.0);
        let remaining_secs = remaining_fraction / slope;
        if !remaining_secs.is_finite() {
            return None;
        }
        Some(Duration::from_secs_f64(remaining_secs))
    }

    /// Clears every recorded observation -- called when a new, unrelated run starts
    /// (a fresh export job or video sweep begins, or the previous one finished,
    /// failed, or was cancelled), so its history never leaks into the next run's own
    /// estimate. See this struct's own doc comment.
    pub fn reset(&mut self) {
        self.observations.clear();
    }
}

/// Formats an [`EtaEstimator::eta`] result as the short string a progress bar shows
/// next to its percentage: `""` for `None` (hides the estimate entirely), otherwise
/// `"about 5 s left"` / `"about 2 min left"` / `"about 1 h 20 min left"`, rounded to
/// the coarsest unit that still reads as informative -- never a sub-second reading,
/// and never a bare `"0 s"` for a durations that rounds down to nothing.
#[must_use]
pub fn format_eta(eta: Option<Duration>) -> String {
    let Some(remaining) = eta else {
        return String::new();
    };
    let total_secs = remaining.as_secs_f64().round().max(1.0) as u64;

    if total_secs < 60 {
        format!("about {total_secs} s left")
    } else if total_secs < 3600 {
        let minutes = (total_secs as f64 / 60.0).round() as u64;
        format!("about {minutes} min left")
    } else {
        let hours = total_secs / 3600;
        let minutes = (total_secs % 3600) / 60;
        if minutes == 0 {
            format!("about {hours} h left")
        } else {
            format!("about {hours} h {minutes} min left")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EtaEstimator, format_eta};
    use std::time::{Duration, Instant};

    /// Perfectly linear progress (a constant rate the whole way) must produce an
    /// exact ETA: a least-squares fit across points that already lie exactly on one
    /// line reproduces that line's own slope with no error.
    #[test]
    fn linear_progress_gives_exact_eta() {
        let start = Instant::now();
        let mut eta = EtaEstimator::default();
        // 0.05 fraction/second -- reaches 1.0 at t = 20s.
        for t in 0..=9u64 {
            eta.observe(start + Duration::from_secs(t), 0.05 * t as f64);
        }
        let now = start + Duration::from_secs(9);
        let remaining = eta.eta(now).expect("enough history for an estimate");
        // At t=9 the line is at fraction 0.45; 0.55 left at 0.05/s is 11s.
        assert!(
            (remaining.as_secs_f64() - 11.0).abs() < 1e-6,
            "expected ~11s, got {remaining:?}"
        );
    }

    /// A coordinator-style step cadence (a jump every 20s, matching this task's own
    /// "up to ~22s per step" note) must still land within 25% of the true remaining
    /// time once a handful of steps have been observed -- the whole reason this
    /// estimator fits a line across the window instead of reading the single most
    /// recent step's own instantaneous rate.
    #[test]
    fn step_like_progress_is_within_a_quarter_of_the_truth_after_three_steps() {
        let start = Instant::now();
        let mut eta = EtaEstimator::default();
        // 5 steps of 20s each reach fraction 1.0 at t=100s; a true rate of 0.01/s.
        eta.observe(start, 0.0);
        eta.observe(start + Duration::from_secs(20), 0.2);
        eta.observe(start + Duration::from_secs(40), 0.4);
        eta.observe(start + Duration::from_secs(60), 0.6);
        let now = start + Duration::from_secs(60);
        let remaining = eta.eta(now).expect("enough history after three steps");
        let truth_secs = 40.0; // 100s total - 60s elapsed
        let error = (remaining.as_secs_f64() - truth_secs).abs() / truth_secs;
        assert!(
            error <= 0.25,
            "expected within 25% of {truth_secs}s, got {remaining:?}"
        );
    }

    /// Progress that never advances (the same fraction reported over and over) must
    /// never produce an estimate -- a zero slope has no forward rate to project a
    /// completion time from.
    #[test]
    fn stalled_progress_returns_none() {
        let start = Instant::now();
        let mut eta = EtaEstimator::default();
        for t in 0..=6u64 {
            eta.observe(start + Duration::from_secs(t), 0.3);
        }
        assert!(eta.eta(start + Duration::from_secs(6)).is_none());
    }

    /// [`EtaEstimator::reset`] must drop every prior observation -- confirmed by
    /// re-feeding only a single, too-recent observation afterward and checking the
    /// estimate is still refused for lack of history, exactly as a brand-new
    /// estimator would behave.
    #[test]
    fn reset_clears_prior_history() {
        let start = Instant::now();
        let mut eta = EtaEstimator::default();
        for t in 0..=9u64 {
            eta.observe(start + Duration::from_secs(t), 0.05 * t as f64);
        }
        assert!(eta.eta(start + Duration::from_secs(9)).is_some());

        eta.reset();
        eta.observe(start + Duration::from_secs(9), 0.45);
        assert!(
            eta.eta(start + Duration::from_secs(9)).is_none(),
            "a single observation right after reset must not yet produce an estimate"
        );
    }

    #[test]
    fn format_eta_renders_each_bucket() {
        assert_eq!(format_eta(None), "");
        assert_eq!(format_eta(Some(Duration::from_secs(5))), "about 5 s left");
        assert_eq!(
            format_eta(Some(Duration::from_secs(120))),
            "about 2 min left"
        );
        assert_eq!(
            format_eta(Some(Duration::from_hours(1) + Duration::from_mins(20))),
            "about 1 h 20 min left"
        );
        assert_eq!(
            format_eta(Some(Duration::from_millis(400))),
            "about 1 s left"
        );
    }
}
