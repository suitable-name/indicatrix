//! The gemological-metrics worker: evaluates the optical metrics and the angular profile
//! off the render thread, so a light, cut or material change never stalls tracing.
//!
//! One evaluation is an 18x18 grid of multi-bounce traces (see
//! `indicatrix::color::metrics`), and a change of light, facets or material needs twenty
//! of them (the pose plus the 19-point profile). Run inline, that held the render loop for
//! seconds. Here the loop only *requests* an evaluation when its inputs differ from the
//! last request, and keeps using the last published result until a newer one lands.
//!
//! # Scheduling
//!
//! Requests are debounced: the worker starts once [`DEBOUNCE`] has passed without a newer
//! request, so an orbit or a slider drag costs one evaluation after the gesture settles,
//! not one per intermediate pose. [`MAX_DEFER`] bounds that wait, so a gesture that never
//! settles still refreshes the numbers about twice a second. The very first request runs
//! immediately. The thread is an ordinary OS thread, not a rayon task, and does not touch
//! the trace thread pool.

use super::{
    display_thread::FrameMetricsSnapshot,
    frame_helpers::push_metrics_to_ui,
    hash_planes,
    metrics::{MetricsCache, compute_or_reuse_metrics},
};
use indicatrix::{
    color::metrics::GemOpticalMetrics,
    geometry::plane::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        raytracer::{EnvironmentSource, LightingPreset},
    },
    renderer::env_map::EnvironmentMap,
};
use slint::{ComponentHandle, Weak};
use std::{
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    thread,
    time::{Duration, Instant},
};

/// Quiet time after the latest request before the worker starts evaluating it.
pub(super) const DEBOUNCE: Duration = Duration::from_millis(100);

/// Longest a pending request may wait while newer requests keep replacing it.
pub(super) const MAX_DEFER: Duration = Duration::from_millis(500);

/// One finished evaluation: the pose metrics and the three 19-point profile graphs.
#[derive(Clone, Copy)]
pub(super) struct EvaluatedMetrics {
    pub(super) metrics: GemOpticalMetrics,
    pub(super) graph_brilliance: [f32; 19],
    pub(super) graph_extinction: [f32; 19],
    pub(super) graph_windowing: [f32; 19],
}

impl EvaluatedMetrics {
    /// The display-cycle payload for these values at the current camera pitch.
    pub(super) const fn snapshot(self, cam_pitch_deg: f32) -> FrameMetricsSnapshot {
        FrameMetricsSnapshot {
            metrics: self.metrics,
            graph_brilliance: self.graph_brilliance,
            graph_extinction: self.graph_extinction,
            graph_windowing: self.graph_windowing,
            cam_pitch_deg,
        }
    }
}

/// The values shown before the first evaluation lands, equal to the UI's own initial
/// state.
const NOT_YET_EVALUATED: EvaluatedMetrics = EvaluatedMetrics {
    metrics: GemOpticalMetrics {
        brilliance_pct: 0.0,
        fire_index: 0.0,
        scintillation_pct: 0.0,
        windowing_pct: 0.0,
        extinction_pct: 0.0,
    },
    graph_brilliance: [0.0; 19],
    graph_extinction: [0.0; 19],
    graph_windowing: [0.0; 19],
};

/// Everything one evaluation reads.
struct Request {
    planes: Arc<Vec<GpuFacetPlane>>,
    material: GemMaterial,
    /// `[yaw, pitch, light_yaw, light_pitch]`.
    pose: [f32; 4],
    /// The selected lighting preset: the metrics are scored under the radiance the viewport
    /// is lit with.
    lighting_preset: LightingPreset,
    /// The loaded HDR panorama, which replaces the preset's rig as the lighting.
    env_map: Option<Arc<EnvironmentMap>>,
}

impl Request {
    /// The environment this request is scored under: the loaded panorama if any, else the
    /// preset's rig at the request's light pose.
    fn environment(&self) -> EnvironmentSource<'_> {
        let [_, _, light_yaw, light_pitch] = self.pose;
        self.env_map.as_deref().map_or_else(
            || self.lighting_preset.studio(1.0, light_yaw, light_pitch),
            EnvironmentSource::HdrMap,
        )
    }
}

/// Whether two optional panoramas are the same loaded map (or both absent).
fn same_env_map(a: Option<&Arc<EnvironmentMap>>, b: Option<&Arc<EnvironmentMap>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

/// A request waiting out its debounce.
struct Pending {
    request: Request,
    first_queued: Instant,
    last_changed: Instant,
}

impl Pending {
    /// When the worker may start: [`DEBOUNCE`] after the last change, but never later
    /// than [`MAX_DEFER`] after the request first queued, and at once while nothing has
    /// been published yet.
    fn due_at(&self, have_result: bool) -> Instant {
        if have_result {
            (self.last_changed + DEBOUNCE).min(self.first_queued + MAX_DEFER)
        } else {
            self.first_queued
        }
    }
}

#[derive(Default)]
struct State {
    pending: Option<Pending>,
    published: Option<EvaluatedMetrics>,
    /// Bumped by every publication; `0` until the first.
    version: u64,
    closed: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the pending request, keeping the time the first one queued.
    fn submit(&self, request: Request) {
        let now = Instant::now();
        let mut state = self.lock();
        let first_queued = state
            .pending
            .as_ref()
            .map_or(now, |pending| pending.first_queued);
        state.pending = Some(Pending {
            request,
            first_queued,
            last_changed: now,
        });
        drop(state);
        self.wake.notify_one();
    }

    /// Blocks until a request is due and takes it; `None` once the handle is gone.
    fn next_due(&self) -> Option<Request> {
        let mut state = self.lock();
        loop {
            if state.closed {
                return None;
            }
            let have_result = state.published.is_some();
            let due = state
                .pending
                .as_ref()
                .map(|pending| pending.due_at(have_result));
            state = match due {
                None => self
                    .wake
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
                Some(due) => {
                    let now = Instant::now();
                    if now >= due {
                        return state.pending.take().map(|pending| pending.request);
                    }
                    self.wake
                        .wait_timeout(state, due - now)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
    }

    fn publish(&self, evaluated: EvaluatedMetrics) {
        let mut state = self.lock();
        state.published = Some(evaluated);
        state.version += 1;
    }
}

/// The worker thread's body: evaluate what is due, publish, repeat until closed.
fn run(shared: &Shared) {
    let mut cache: Option<MetricsCache> = None;
    while let Some(request) = shared.next_due() {
        let [yaw, pitch, ..] = request.pose;
        let (metrics, graph_brilliance, graph_extinction, graph_windowing) =
            compute_or_reuse_metrics(
                &mut cache,
                &request.planes,
                &request.material,
                yaw,
                pitch,
                request.environment(),
            );
        shared.publish(EvaluatedMetrics {
            metrics,
            graph_brilliance,
            graph_extinction,
            graph_windowing,
        });
    }
}

/// What the last queued request was built from, to tell whether the next inputs differ.
struct Submitted {
    planes_hash: u64,
    material: GemMaterial,
    pose: [f32; 4],
    lighting_preset: LightingPreset,
    env_map: Option<Arc<EnvironmentMap>>,
}

/// The render thread's handle on the metrics worker.
pub(super) struct MetricsWorker {
    shared: Arc<Shared>,
    submitted: Option<Submitted>,
}

impl MetricsWorker {
    /// Starts the worker thread. If the OS refuses the thread, no metrics are ever
    /// published and the HUD keeps its initial values; rendering is unaffected.
    pub(super) fn spawn() -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("indicatrix-metrics".to_string())
            .spawn(move || run(&worker_shared));
        if let Err(error) = spawned {
            tracing::warn!(%error, "could not start the metrics worker; HUD metrics stay at their defaults");
        }
        Self {
            shared,
            submitted: None,
        }
    }

    /// Queues an evaluation of these inputs unless they equal the last ones queued.
    ///
    /// `lighting_preset` and `env_map` are the lighting the viewport renders with (the
    /// panorama, when one is loaded, replaces the preset's rig).
    ///
    /// Costs one hash of the facet planes and a material comparison on the calling
    /// thread; the material is cloned only when it actually changed.
    pub(super) fn request(
        &mut self,
        planes: &Arc<Vec<GpuFacetPlane>>,
        material: &GemMaterial,
        pose: [f32; 4],
        lighting_preset: LightingPreset,
        env_map: Option<&Arc<EnvironmentMap>>,
    ) {
        let planes_hash = hash_planes(planes);
        if self.submitted.as_ref().is_some_and(|last| {
            last.planes_hash == planes_hash
                && last.pose == pose
                && last.material == *material
                && last.lighting_preset == lighting_preset
                && same_env_map(last.env_map.as_ref(), env_map)
        }) {
            return;
        }
        let last = self.submitted.get_or_insert_with(|| Submitted {
            planes_hash,
            material: material.clone(),
            pose,
            lighting_preset,
            env_map: env_map.cloned(),
        });
        last.planes_hash = planes_hash;
        last.pose = pose;
        last.lighting_preset = lighting_preset;
        last.env_map = env_map.cloned();
        if last.material != *material {
            last.material.clone_from(material);
        }
        self.shared.submit(Request {
            planes: Arc::clone(planes),
            material: material.clone(),
            pose,
            lighting_preset,
            env_map: env_map.cloned(),
        });
    }

    /// The newest published values and their version (`0` while still the defaults).
    ///
    /// Possibly for inputs older than the last [`Self::request`]: the loop keeps showing
    /// these until the debounced evaluation replaces them.
    pub(super) fn latest(&self) -> (u64, EvaluatedMetrics) {
        let state = self.shared.lock();
        (state.version, state.published.unwrap_or(NOT_YET_EVALUATED))
    }
}

impl Drop for MetricsWorker {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.wake.notify_all();
    }
}

/// Pushes the newest published metrics to the UI without an image, if `pushed_version` has
/// not seen them yet. This is what updates the HUD once the picture has converged and no
/// display cycle is left to carry them.
///
/// `ui_idle` must be `false` while a display cycle is in flight: that cycle carries its
/// own copy of the metrics, and pushing around it could let the older copy land last.
pub(super) fn push_if_fresh<T, M>(
    worker: &MetricsWorker,
    pushed_version: &mut u64,
    ui_idle: bool,
    ui_weak: &Weak<T>,
    update_metrics: &M,
    cam_pitch_deg: f32,
) where
    T: ComponentHandle + 'static,
    M: Fn(&T, f32, f32, f32, f32, f32, [f32; 19], [f32; 19], [f32; 19], f32)
        + Send
        + 'static
        + Clone,
{
    let (version, evaluated) = worker.latest();
    if ui_idle && version > *pushed_version {
        *pushed_version = version;
        push_metrics_to_ui(ui_weak, update_metrics, evaluated.snapshot(cam_pitch_deg));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(first_queued: Instant, last_changed: Instant) -> Pending {
        Pending {
            request: Request {
                planes: Arc::new(Vec::new()),
                material: GemMaterial::diamond(),
                pose: [0.0; 4],
                lighting_preset: LightingPreset::RingLights,
                env_map: None,
            },
            first_queued,
            last_changed,
        }
    }

    #[test]
    fn a_settled_request_starts_after_the_debounce() {
        let t0 = Instant::now();
        assert_eq!(pending(t0, t0).due_at(true), t0 + DEBOUNCE);
    }

    #[test]
    fn a_request_that_keeps_changing_starts_no_later_than_the_max_defer() {
        let t0 = Instant::now();
        let last_changed = t0 + MAX_DEFER;
        assert_eq!(pending(t0, last_changed).due_at(true), t0 + MAX_DEFER);
    }

    #[test]
    fn the_first_request_starts_immediately() {
        let t0 = Instant::now();
        assert_eq!(pending(t0, t0 + DEBOUNCE).due_at(false), t0);
    }

    #[test]
    fn an_unchanged_request_is_not_queued_twice() {
        let mut worker = MetricsWorker {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                wake: Condvar::new(),
            }),
            submitted: None,
        };
        let planes = Arc::new(Vec::new());
        let material = GemMaterial::diamond();
        let pose = [0.1, 0.2, 0.3, 0.4];
        let preset = LightingPreset::RingLights;

        worker.request(&planes, &material, pose, preset, None);
        let first = worker
            .shared
            .lock()
            .pending
            .as_ref()
            .map(|pending| pending.last_changed);
        assert!(first.is_some(), "the first request must queue");

        worker.request(&planes, &material, pose, preset, None);
        let second = worker
            .shared
            .lock()
            .pending
            .as_ref()
            .map(|pending| pending.last_changed);
        assert_eq!(first, second, "identical inputs must not re-queue");

        thread::sleep(Duration::from_millis(2));
        worker.request(&planes, &material, [0.1, 0.2, 0.3, 0.5], preset, None);
        let third = worker
            .shared
            .lock()
            .pending
            .as_ref()
            .map(|pending| pending.last_changed);
        assert_ne!(second, third, "a changed light pose must re-queue");

        thread::sleep(Duration::from_millis(2));
        worker.request(
            &planes,
            &material,
            [0.1, 0.2, 0.3, 0.5],
            LightingPreset::LightTent,
            None,
        );
        let fourth = worker
            .shared
            .lock()
            .pending
            .as_ref()
            .map(|pending| pending.last_changed);
        assert_ne!(third, fourth, "a changed lighting preset must re-queue");

        thread::sleep(Duration::from_millis(2));
        let map = Arc::new(EnvironmentMap::uniform(4, 2, [1.0, 1.0, 1.0]));
        worker.request(
            &planes,
            &material,
            [0.1, 0.2, 0.3, 0.5],
            LightingPreset::LightTent,
            Some(&map),
        );
        let fifth = worker
            .shared
            .lock()
            .pending
            .as_ref()
            .map(|pending| pending.last_changed);
        assert_ne!(fourth, fifth, "a loaded HDR map must re-queue");
    }
}
