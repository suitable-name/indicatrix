//! Coordinator mode (`worker` builds only).
//!
//! - The worker-facing half: the worker port (separate from the viewer port)
//!   `indicatrix-worker join` dials ([`spawn_worker_listener`]), the [`Registry`] of
//!   joined worker connections with its liveness thread, and what the coordinator
//!   advertises to viewers ([`viewer_render_capability`]).
//! - Request execution: [`job`] fans a viewer's `RenderRequest` or
//!   `FinalImageRequest` out over [`Registry::checkout`]ed connections ([`lanes`]) and
//!   the own lane with `indicatrix-dispatch`'s `LanePool`; [`tilt`] routes
//!   `TiltCurvesRequest`s; [`viewer`] sends `CAPABILITY_CHANGED` between requests.

mod job;
mod lanes;
mod listener;
mod liveness;
mod registry;
#[cfg(test)]
mod tests;
mod tilt;
mod viewer;

pub use job::{
    Coordinator, InteractivePin, JobConfig, LaneKey, OwnLaneSetup, RateBook, ViewerSession,
    job_bytes, serve_final_image, serve_render,
};
pub use lanes::{JobLanes, JoinedWorkerLane, LaneNeed, LaneTimeouts, OwnLane};
pub use listener::{WorkerListenerConfig, spawn_worker_listener};
pub use registry::{Capacity, LivenessConfig, Registry, WorkerConn, WorkerHandle, WorkerInfo};
pub use tilt::serve_tilt;
pub use viewer::{CapabilityWatch, read_message_watching};

use indicatrix_net::messages::{Backend, RenderCapability};

/// What a coordinator's viewer-facing `WELCOME.render` says, from
/// its own lane's capability (`Some` iff `serve --render`) and the joined workers'
/// [`Capacity`] (`None` without a worker port):
///
/// - no own lane, no joined worker: `None` -- the GUI sees a library-only remote and
///   renders locally;
/// - own lane, no joined worker: the own lane's capability unchanged (`Cpu`/`Gpu`), so
///   `serve --render` looks exactly like the plain worker it replaces;
/// - at least one joined worker: `Backend::Coordinator { workers, threads, gpus }`,
///   with `threads`/`gpus` summed over the own lane (if any) and every joined worker,
///   `max_pixels` the largest any lane accepts (a lane whose cap a request exceeds just
///   gets no share of it), and the own lane's cadence floor (else the workers').
///
/// `hdr` follows `crate::assets::coordinator_advertises_hdr`: the coordinator
/// holds an asset cache (`holds_assets`) and some lane renders HDR.
#[must_use]
pub fn viewer_render_capability(
    own: Option<&RenderCapability>,
    workers: Option<Capacity>,
    holds_assets: bool,
) -> Option<RenderCapability> {
    let workers = workers.filter(|c| c.workers > 0);
    let Some(workers) = workers else {
        return own.cloned();
    };
    let (own_threads, own_gpus) = match own.map(|o| &o.backend) {
        Some(Backend::Cpu { threads }) => (*threads, 0),
        Some(Backend::Gpu { .. }) => (0, 1),
        Some(Backend::Coordinator { threads, gpus, .. }) => (*threads, *gpus),
        None => (0, 0),
    };
    Some(RenderCapability {
        backend: Backend::Coordinator {
            workers: workers.workers,
            threads: workers.threads.saturating_add(own_threads),
            gpus: workers.gpus.saturating_add(own_gpus),
        },
        max_pixels: own.map_or(workers.max_pixels, |o| o.max_pixels.max(workers.max_pixels)),
        min_cadence_ms: own.map_or(crate::stream_emit::MIN_CADENCE_FLOOR_MS, |o| {
            o.min_cadence_ms
        }),
        // The rule lives in `crate::assets::policy`, next to the request-side one.
        hdr: crate::assets::coordinator_advertises_hdr(own, workers, holds_assets),
    })
}
