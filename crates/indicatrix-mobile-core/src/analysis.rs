//! Metrics and tilt curves on their own thread.
//!
//! The Checks tab can measure the stone (five optical metrics: brilliance, fire,
//! scintillation, windowing, extinction) and sweep it through every tilt (four axes of 181
//! points). Both are the shared `indicatrix_web_core::solve` handlers (`run_metrics_with_map`
//! and `run_tilt`), the same code the desktop and the web app run, on **one lazily started
//! worker thread**, so they never compete with the design solve or the render pool for the
//! UI thread.
//!
//! # Requests, supersession and staleness
//!
//! - [`request_metrics`] and [`request_tilt`] build the scene from the app state
//!   (the current solve, the material, the lighting and the chosen HDR map), queue a job and
//!   return at once. A newer request of the same kind supersedes the older one (the
//!   older tilt sweep stops at its next point; an older metrics job finishes but its result
//!   is dropped). The two kinds do not cancel each other: they run one after the other.
//! - [`supersede_stale`] cancels a running job whose inputs no longer match the app (the
//!   design or the lighting changed meanwhile); the front end calls it on every refresh.
//! - A finished result is stored with the inputs it was computed for. The views
//!   ([`metrics_view`], [`tilt_view`]) compare that key with the app as it stands and say
//!   `Stale` when they differ, so old numbers stay visible, marked, until recomputed.
//!
//! # Scoring
//!
//! The metrics are scored near face-up (the renderer's default pose) under the chosen lighting:
//! the HDR map when it is the lighting, otherwise the preset. The tilt sweep is always
//! scored under the preset's rig, as on the desktop.
//!
//! # Notifying the UI
//!
//! The worker finishes on its own thread. [`AnalysisState::set_notifier`] registers a
//! callback it calls (throttled while a sweep reports progress); the front end uses it to
//! hop back to the UI thread and push the new views.

mod inputs;
mod view;
mod worker;

#[cfg(test)]
mod tests;

use std::{
    fmt,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::{Sender, channel},
    },
    thread::JoinHandle,
};

use indicatrix_web_core::solve::{MetricsParams, MetricsResultData, ScoredUnder, TiltParams};

use crate::{app_state::AppState, tasks::Generation};

pub use indicatrix_web_core::solve::TiltAxisData;
pub use view::{
    AnalysisStatus, MetricRow, MetricsView, TILT_POINTS, TiltView, metric_rows, tilt_at,
};

use inputs::{Inputs, Key, scene_inputs};
use worker::Job;

/// Which measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The five optical metrics.
    Metrics,
    /// The four-axis tilt sweep.
    Tilt,
}

/// A callback the worker thread calls when results or progress change.
pub type Notifier = Arc<dyn Fn() + Send + Sync>;

/// The five metrics of a finished measurement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetricsReading {
    /// Percentage of incident rays visibly returned to the observer.
    pub brilliance_pct: f32,
    /// The display-scaled fire index.
    pub fire_index: f32,
    /// Combined spatial and temporal light-return contrast, 0 to 100.
    pub scintillation_pct: f32,
    /// Percentage of incident rays that leaked out through the pavilion.
    pub windowing_pct: f32,
    /// Percentage of incident rays trapped, absorbed or not visibly returned.
    pub extinction_pct: f32,
    /// Whether the numbers were scored under the HDR map (else under the lighting preset).
    pub under_hdr: bool,
}

impl From<MetricsResultData> for MetricsReading {
    fn from(data: MetricsResultData) -> Self {
        Self {
            brilliance_pct: data.brilliance_pct,
            fire_index: data.fire_index,
            scintillation_pct: data.scintillation_pct,
            windowing_pct: data.windowing_pct,
            extinction_pct: data.extinction_pct,
            under_hdr: matches!(data.scored_under, ScoredUnder::HdrMap(_)),
        }
    }
}

/// A finished tilt sweep: one entry per axis, in the order of
/// [`TiltReading::axis_names`], each with three [`TILT_POINTS`]-point curves.
#[derive(Debug, Clone, PartialEq)]
pub struct TiltReading {
    /// The four axes (0, 45, 90 and 135 degrees of azimuth).
    pub axes: Vec<TiltAxisData>,
}

/// A queued or running job, as the state remembers it.
pub(crate) struct Busy {
    pub(crate) generation: u64,
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) key: Key,
    /// A running sweep's completion and stage line.
    pub(crate) progress: Option<(f32, String)>,
}

/// One measurement kind's slots.
pub(crate) struct Slots<T> {
    /// The newest result with the inputs it was computed for.
    pub(crate) result: Option<(T, Key)>,
    /// Why the newest job failed (cleared by the next request).
    pub(crate) error: Option<String>,
    /// The job in flight.
    pub(crate) busy: Option<Busy>,
}

impl<T> Default for Slots<T> {
    fn default() -> Self {
        Self {
            result: None,
            error: None,
            busy: None,
        }
    }
}

/// Everything the worker and the UI thread share.
#[derive(Default)]
pub(crate) struct Inner {
    pub(crate) metrics: Slots<MetricsReading>,
    pub(crate) tilt: Slots<Arc<TiltReading>>,
}

/// The shared half of [`AnalysisState`].
#[derive(Default)]
pub(crate) struct Shared {
    pub(crate) inner: Mutex<Inner>,
    pub(crate) metrics_generation: Generation,
    pub(crate) tilt_generation: Generation,
    notifier: Mutex<Option<Notifier>>,
}

impl Shared {
    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) const fn generation(&self, kind: Kind) -> &Generation {
        match kind {
            Kind::Metrics => &self.metrics_generation,
            Kind::Tilt => &self.tilt_generation,
        }
    }

    /// Calls the notifier, if one is registered. Never call with the inner lock held.
    pub(crate) fn notify(&self) {
        let notifier = self
            .notifier
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(notifier) = notifier {
            notifier();
        }
    }

    /// Cancels the job of `kind` in flight (if any) and forgets it, so its result is
    /// dropped when it arrives.
    fn cancel_kind(&self, kind: Kind) -> bool {
        let _ = self.generation(kind).advance();
        let busy = {
            let mut inner = self.lock();
            match kind {
                Kind::Metrics => inner.metrics.busy.take(),
                Kind::Tilt => inner.tilt.busy.take(),
            }
        };
        busy.is_some_and(|busy| {
            busy.cancel.store(true, Ordering::Relaxed);
            true
        })
    }
}

/// The worker thread and the way to feed it.
struct WorkerHandle {
    sender: Sender<Job>,
    handle: JoinHandle<()>,
}

/// The last metrics and tilt results and the analysis thread. See the module docs.
pub struct AnalysisState {
    shared: Arc<Shared>,
    worker: Option<WorkerHandle>,
    closed: bool,
}

impl Default for AnalysisState {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared::default()),
            worker: None,
            closed: false,
        }
    }
}

impl fmt::Debug for AnalysisState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnalysisState")
            .field("busy", &self.busy())
            .field("worker_started", &self.worker.is_some())
            .finish_non_exhaustive()
    }
}

impl AnalysisState {
    /// Registers the callback the worker calls when a result or progress changes (from
    /// the worker thread: hop to the UI thread inside it). `None` removes it.
    pub fn set_notifier(&self, notifier: Option<Notifier>) {
        *self
            .shared
            .notifier
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = notifier;
    }

    /// The newest metrics, whether or not they still describe the design. See
    /// [`metrics_view`] for the status.
    #[must_use]
    pub fn latest_metrics(&self) -> Option<MetricsReading> {
        self.shared
            .lock()
            .metrics
            .result
            .as_ref()
            .map(|(reading, _)| *reading)
    }

    /// The newest tilt curves, whether or not they still describe the design.
    #[must_use]
    pub fn latest_tilt(&self) -> Option<Arc<TiltReading>> {
        self.shared
            .lock()
            .tilt
            .result
            .as_ref()
            .map(|(reading, _)| Arc::clone(reading))
    }

    /// Whether a metrics job or a tilt sweep is queued or running.
    #[must_use]
    pub fn busy(&self) -> bool {
        let inner = self.shared.lock();
        inner.metrics.busy.is_some() || inner.tilt.busy.is_some()
    }

    /// Whether a metrics job is queued or running.
    #[must_use]
    pub fn metrics_busy(&self) -> bool {
        self.shared.lock().metrics.busy.is_some()
    }

    /// Whether a tilt sweep is queued or running.
    #[must_use]
    pub fn tilt_busy(&self) -> bool {
        self.shared.lock().tilt.busy.is_some()
    }

    /// Cancels everything queued or running; results that arrive later are dropped.
    pub fn cancel_all(&self) {
        let _ = self.shared.cancel_kind(Kind::Metrics);
        let _ = self.shared.cancel_kind(Kind::Tilt);
    }

    /// Stops the tilt sweep (the curves already on screen stay). The next refresh shows it
    /// idle.
    pub fn cancel_tilt(&self) {
        if self.shared.cancel_kind(Kind::Tilt) {
            self.shared.notify();
        }
    }

    /// Cancels, ends the worker thread and waits briefly for it. Called by
    /// `AppState::shutdown`; later requests are refused.
    pub fn shutdown(&mut self) {
        self.closed = true;
        self.cancel_all();
        if let Some(worker) = self.worker.take() {
            // Closing the channel ends the thread once the job it runs (if any) is done.
            drop(worker.sender);
            crate::tasks::join_briefly(worker.handle, "indicatrix-analysis");
        }
    }

    /// Starts the worker thread on first use.
    fn sender(&mut self) -> Result<&Sender<Job>, String> {
        if self.closed {
            return Err("The app is closing.".to_owned());
        }
        if self.worker.is_none() {
            let (sender, receiver) = channel::<Job>();
            let shared = Arc::clone(&self.shared);
            let handle = std::thread::Builder::new()
                .name("indicatrix-analysis".to_owned())
                .spawn(move || worker::run_loop(&shared, &receiver))
                .map_err(|error| format!("The analysis thread could not start: {error}."))?;
            self.worker = Some(WorkerHandle { sender, handle });
        }
        self.worker
            .as_ref()
            .map(|worker| &worker.sender)
            .ok_or_else(|| "The analysis thread is not running.".to_owned())
    }

    /// Queues `params` as the newest job of `kind`, superseding the one in flight.
    fn enqueue(
        &mut self,
        kind: Kind,
        params: worker::Params,
        key: Key,
        tools: Vec<indicatrix::geometry::tool::ToolPrimitive>,
        policy: Arc<crate::compute_policy::ComputePolicy>,
    ) -> Result<(), String> {
        let shared = Arc::clone(&self.shared);
        let sender = self.sender()?.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let generation = {
            let generation = shared.generation(kind).advance();
            let mut inner = shared.lock();
            let busy = Busy {
                generation,
                cancel: Arc::clone(&cancel),
                key,
                progress: None,
            };
            let previous = match kind {
                Kind::Metrics => {
                    inner.metrics.error = None;
                    inner.metrics.busy.replace(busy)
                }
                Kind::Tilt => {
                    inner.tilt.error = None;
                    inner.tilt.busy.replace(busy)
                }
            };
            if let Some(previous) = previous {
                previous.cancel.store(true, Ordering::Relaxed);
            }
            generation
        };
        let job = Job {
            tools,
            policy,
            kind,
            generation,
            cancel,
            params,
        };
        sender.send(job).map_err(|_| {
            // The thread died: nothing will ever answer, so do not stay busy.
            let _ = shared.cancel_kind(kind);
            "The analysis thread stopped. Restart the app to measure.".to_owned()
        })?;
        shared.notify();
        Ok(())
    }
}

impl Drop for AnalysisState {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Queues a measurement of the five metrics for the design, material and lighting as they
/// are now.
///
/// # Errors
///
/// A sentence when there is nothing to measure (no current solve, no facets) or the worker
/// thread cannot run.
pub fn request_metrics(state: &mut AppState) -> Result<(), String> {
    let Inputs { spec, hdr, tools } = scene_inputs(state)?;
    let key = Key::new(state, Kind::Metrics)
        .ok_or_else(|| "Solve the design to measure it.".to_owned())?;
    let params = MetricsParams::from_scene(&spec);
    let policy = state.render.compute_policy();
    state.analysis.enqueue(
        Kind::Metrics,
        worker::Params::metrics(params, hdr),
        key,
        tools,
        policy,
    )
}

/// Queues the tilt sweep for the design, material and lighting as they are now.
///
/// # Errors
///
/// As [`request_metrics`].
pub fn request_tilt(state: &mut AppState) -> Result<(), String> {
    let Inputs { spec, tools, .. } = scene_inputs(state)?;
    let key =
        Key::new(state, Kind::Tilt).ok_or_else(|| "Solve the design to measure it.".to_owned())?;
    let params = TiltParams::from_scene(&spec);
    let policy = state.render.compute_policy();
    state
        .analysis
        .enqueue(Kind::Tilt, worker::Params::tilt(params), key, tools, policy)
}

/// Cancels a running job whose inputs no longer match the app (the design or the lighting
/// changed while it ran). Cheap when nothing runs. Returns whether anything was cancelled.
pub fn supersede_stale(state: &AppState) -> bool {
    let mut cancelled = false;
    for kind in [Kind::Metrics, Kind::Tilt] {
        let running = {
            let inner = state.analysis.shared.lock();
            match kind {
                Kind::Metrics => inner.metrics.busy.as_ref().map(|busy| busy.key.clone()),
                Kind::Tilt => inner.tilt.busy.as_ref().map(|busy| busy.key.clone()),
            }
        };
        if let Some(running) = running
            && Key::new(state, kind).as_ref() != Some(&running)
        {
            cancelled |= state.analysis.shared.cancel_kind(kind);
        }
    }
    cancelled
}

/// The Metrics section for the app as it stands.
#[must_use]
pub fn metrics_view(state: &AppState) -> MetricsView {
    view::metrics_view(state)
}

/// The Tilt Curves page for the app as it stands.
#[must_use]
pub fn tilt_view(state: &AppState) -> TiltView {
    view::tilt_view(state)
}
