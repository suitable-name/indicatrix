//! The analysis thread's loop: take a job, run the shared solve handler, store the result
//! if the job is still the newest of its kind.

use std::{
    cell::Cell,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
    },
    time::{Duration, Instant},
};

use indicatrix::{
    color::metrics::{MetricsCache, SweepProgress},
    renderer::env_map::EnvironmentMap,
};
use indicatrix_web_core::solve::{
    MetricsParams, SolveHooks, SolveResponse, TiltParams, run_metrics_with_map, run_tilt,
    sweep_fraction, sweep_status,
};

use super::{Kind, MetricsReading, Shared, TILT_POINTS, TiltReading, inputs::HDR_ID};

/// How often a running sweep tells the UI about its progress.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);

/// What a job measures.
pub(super) enum Params {
    /// The five metrics, with the HDR map they are scored under (if any).
    Metrics(Box<MetricsParams>, Option<Arc<EnvironmentMap>>),
    /// The tilt sweep.
    Tilt(Box<TiltParams>),
}

impl Params {
    /// Metrics inputs.
    pub(super) fn metrics(params: MetricsParams, hdr: Option<Arc<EnvironmentMap>>) -> Self {
        Self::Metrics(Box::new(params), hdr)
    }

    /// Tilt inputs.
    pub(super) fn tilt(params: TiltParams) -> Self {
        Self::Tilt(Box::new(params))
    }
}

/// One queued measurement.
pub(super) struct Job {
    pub(super) kind: Kind,
    /// The generation `enqueue` got for it; the job is stale once a newer one exists.
    pub(super) generation: u64,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) params: Params,
}

/// How a job ended.
enum Outcome {
    Metrics(MetricsReading),
    Tilt(TiltReading),
    Cancelled,
    Failed(String),
}

/// The thread body: runs jobs until the channel closes.
pub(super) fn run_loop(shared: &Shared, receiver: &Receiver<Job>) {
    let mut cache: Option<MetricsCache> = None;
    for job in receiver {
        // A newer request (or a cancel) made this one obsolete while it waited.
        if job.cancel.load(Ordering::Relaxed)
            || !shared.generation(job.kind).is_current(job.generation)
        {
            continue;
        }
        let outcome = catch_unwind(AssertUnwindSafe(|| run(shared, &job, &mut cache)))
            .unwrap_or_else(|_| {
                Outcome::Failed("The measurement stopped unexpectedly.".to_owned())
            });
        finish(shared, &job, outcome);
    }
}

/// Runs `job` to its outcome.
fn run(shared: &Shared, job: &Job, cache: &mut Option<MetricsCache>) -> Outcome {
    match &job.params {
        Params::Metrics(params, hdr) => {
            let held = hdr.as_deref().map(|map| (HDR_ID, map));
            match run_metrics_with_map(cache, params, held) {
                SolveResponse::Metrics(data) => Outcome::Metrics(data.into()),
                SolveResponse::AnalysisFailed { message, .. } => Outcome::Failed(message),
                _ => Outcome::Failed("The measurement gave an unexpected answer.".to_owned()),
            }
        }
        Params::Tilt(params) => run_sweep(shared, job, params),
    }
}

/// Runs the tilt sweep, reporting progress now and then and stopping when cancelled.
fn run_sweep(shared: &Shared, job: &Job, params: &TiltParams) -> Outcome {
    let last = Cell::new(None::<Instant>);
    let on_sweep = |progress: SweepProgress| {
        let due = last
            .get()
            .is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL);
        if !due {
            return;
        }
        last.set(Some(Instant::now()));
        let updated = {
            let mut inner = shared.lock();
            match inner.tilt.busy.as_mut() {
                Some(busy) if busy.generation == job.generation => {
                    busy.progress = Some((sweep_fraction(progress), sweep_status(progress)));
                    true
                }
                _ => false,
            }
        };
        if updated {
            shared.notify();
        }
    };
    let hooks = SolveHooks {
        cancel: &job.cancel,
        on_progress: &|_| {},
        on_sweep: &on_sweep,
    };
    match run_tilt(params, &hooks) {
        SolveResponse::TiltCurves(data) => {
            let well_formed = data.axes.len() == 4
                && data.axes.iter().all(|axis| {
                    [&axis.brilliance, &axis.extinction, &axis.windowing]
                        .iter()
                        .all(|curve| curve.len() == TILT_POINTS)
                });
            if well_formed {
                Outcome::Tilt(TiltReading { axes: data.axes })
            } else {
                Outcome::Failed("The sweep returned malformed curves.".to_owned())
            }
        }
        SolveResponse::Cancelled => Outcome::Cancelled,
        SolveResponse::AnalysisFailed { message, .. } => Outcome::Failed(message),
        _ => Outcome::Failed("The sweep gave an unexpected answer.".to_owned()),
    }
}

/// Stores `outcome` when `job` is still the newest of its kind, then tells the UI.
fn finish(shared: &Shared, job: &Job, outcome: Outcome) {
    let current = shared.generation(job.kind).is_current(job.generation);
    if current {
        let mut inner = shared.lock();
        match (job.kind, outcome) {
            (Kind::Metrics, Outcome::Metrics(reading)) => {
                if let Some(busy) = inner.metrics.busy.take() {
                    inner.metrics.result = Some((reading, busy.key));
                }
            }
            (Kind::Tilt, Outcome::Tilt(reading)) => {
                if let Some(busy) = inner.tilt.busy.take() {
                    inner.tilt.result = Some((Arc::new(reading), busy.key));
                }
            }
            (Kind::Metrics, Outcome::Failed(message)) => {
                inner.metrics.busy = None;
                inner.metrics.error = Some(message);
            }
            (Kind::Tilt, Outcome::Failed(message)) => {
                inner.tilt.busy = None;
                inner.tilt.error = Some(message);
            }
            // A cancelled job whose request is still the newest (nothing replaced it):
            // just stop being busy. Mismatched kinds cannot happen.
            (Kind::Metrics, _) => inner.metrics.busy = None,
            (Kind::Tilt, _) => inner.tilt.busy = None,
        }
    }
    shared.notify();
}
