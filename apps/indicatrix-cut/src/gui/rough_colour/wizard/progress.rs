//! The progress line of the Fit step: which stage the fit is in, how far the whole run is, and the
//! time left (the app's regression estimator, worded like the batch dialogs).

use crate::gui::progress_eta::{EtaEstimator, batch_eta_label};
use indicatrix_cut_core::rough_plan::colour_fit::solve::FitStage;
use std::time::{Duration, Instant};

/// The share of the whole run that each stage takes, as `(start, end)`. The roughness search
/// re-traces the rig several times, so it is the longest stage when it is on.
#[must_use]
pub fn stage_range(stage: FitStage, roughness_search: bool) -> (f32, f32) {
    // Order: trace, rough, align, fit, select, cross.
    let order: [f32; 6] = if roughness_search {
        [0.10, 0.45, 0.0, 0.25, 0.02, 0.18]
    } else {
        [0.45, 0.0, 0.0, 0.35, 0.02, 0.18]
    };
    let index = match stage {
        FitStage::Tracing => 0,
        FitStage::Roughness => 1,
        FitStage::Alignment => 2,
        FitStage::Fitting => 3,
        FitStage::Selection => 4,
        FitStage::CrossValidation => 5,
        FitStage::Done => return (1.0, 1.0),
    };
    let start: f32 = order[..index].iter().sum();
    (start, start + order[index])
}

/// The finished fraction of the whole run (0 to 1) when stage `stage` is `within` (0 to 1)
/// through.
#[must_use]
pub fn overall_fraction(stage: FitStage, within: f32, roughness_search: bool) -> f32 {
    let (start, end) = stage_range(stage, roughness_search);
    let within = if within.is_finite() {
        within.clamp(0.0, 1.0)
    } else {
        0.0
    };
    (end - start).mul_add(within, start).clamp(0.0, 1.0)
}

/// What the fit is doing, for the progress line.
#[must_use]
pub const fn stage_label(stage: FitStage) -> &'static str {
    match stage {
        FitStage::Tracing => "Tracing light through the stone",
        FitStage::Roughness => "Searching the surface roughness",
        FitStage::Alignment => "Refining the alignment",
        FitStage::Fitting => "Fitting the colours",
        FitStage::Selection => "Comparing the models",
        FitStage::CrossValidation => "Checking with each view held out",
        FitStage::Done => "Done",
    }
}

/// The line under the Start button: `"Fitting the colours - 43 % - about 2 min left"`.
#[must_use]
pub fn progress_line(stage: FitStage, overall: f32, eta: Option<Duration>) -> String {
    let percent = (overall.clamp(0.0, 1.0) * 100.0).round() as u32;
    let mut line = format!("{} - {percent} %", stage_label(stage));
    let tail = if stage == FitStage::Done {
        String::new()
    } else {
        batch_eta_label(percent, 100, eta)
    };
    if !tail.is_empty() {
        line.push_str(" - ");
        line.push_str(&tail);
    }
    line
}

/// The clock of one fit run: feeds the estimator with the overall fraction and words the line.
#[derive(Debug, Default)]
pub struct FitClock {
    estimator: EtaEstimator,
    roughness_search: bool,
}

impl FitClock {
    /// A clock for a run with or without the roughness search.
    #[must_use]
    pub fn new(roughness_search: bool) -> Self {
        Self {
            estimator: EtaEstimator::default(),
            roughness_search,
        }
    }

    /// Records a report and returns the progress line and the overall fraction.
    pub fn report(&mut self, now: Instant, stage: FitStage, within: f32) -> (String, f32) {
        let overall = overall_fraction(stage, within, self.roughness_search);
        self.estimator.observe(now, f64::from(overall));
        let line = progress_line(stage, overall, self.estimator.eta(now));
        (line, overall)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_ranges_tile_the_run() {
        for rough in [false, true] {
            let stages = [
                FitStage::Tracing,
                FitStage::Roughness,
                FitStage::Alignment,
                FitStage::Fitting,
                FitStage::Selection,
                FitStage::CrossValidation,
            ];
            let mut expected_start = 0.0_f32;
            for stage in stages {
                let (start, end) = stage_range(stage, rough);
                assert!((start - expected_start).abs() < 1e-6, "{stage:?}");
                assert!(end >= start);
                expected_start = end;
            }
            assert!((expected_start - 1.0).abs() < 1e-5, "{expected_start}");
            assert_eq!(stage_range(FitStage::Done, rough), (1.0, 1.0));
        }
    }

    #[test]
    fn the_overall_fraction_never_goes_back_across_stages() {
        let mut last = 0.0;
        for (stage, within) in [
            (FitStage::Tracing, 0.0),
            (FitStage::Tracing, 1.0),
            (FitStage::Roughness, 0.5),
            (FitStage::Fitting, 0.2),
            (FitStage::CrossValidation, 1.0),
            (FitStage::Done, 0.0),
        ] {
            let now = overall_fraction(stage, within, true);
            assert!(now >= last - 1e-6, "{stage:?} {within}");
            last = now;
        }
        assert!((last - 1.0).abs() < 1e-5);
        assert_eq!(
            overall_fraction(FitStage::Fitting, f32::NAN, false),
            stage_range(FitStage::Fitting, false).0
        );
        let over = overall_fraction(FitStage::Fitting, 7.0, false);
        assert!((over - stage_range(FitStage::Fitting, false).1).abs() < 1e-6);
    }

    #[test]
    fn the_line_names_the_stage_the_percentage_and_the_time() {
        let line = progress_line(FitStage::Fitting, 0.43, Some(Duration::from_secs(130)));
        assert_eq!(line, "Fitting the colours - 43 % - about 2 min left");
        let line = progress_line(FitStage::Tracing, 0.05, None);
        assert_eq!(
            line,
            "Tracing light through the stone - 5 % - estimating..."
        );
        let line = progress_line(FitStage::Done, 1.0, None);
        assert_eq!(line, "Done - 100 %");
    }

    #[test]
    fn the_clock_estimates_after_enough_history() {
        let mut clock = FitClock::new(false);
        let start = Instant::now();
        let (line, first) = clock.report(start, FitStage::Tracing, 0.0);
        assert!(line.contains("estimating..."));
        assert!(first.abs() < 1e-6);
        let (line, later) = clock.report(start + Duration::from_secs(10), FitStage::Tracing, 0.5);
        assert!(later > first);
        assert!(line.contains("left"), "{line}");
    }
}
