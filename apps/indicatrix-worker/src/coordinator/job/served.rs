//! The one `info!` line per request a coordinator serves.

use super::plan::{Ask, Route};
use crate::stream_emit::StreamOutcome;
use indicatrix_net::messages::RenderRequest;
use std::time::Duration;

/// How a request was executed, for its log line.
#[derive(Debug, Clone, Copy)]
pub(super) struct Served {
    /// The route: the own lane served it like a plain worker (`true`), or a job over the
    /// lane pool.
    pub direct: bool,
    /// The job was a whole-image one (one picture on one lane).
    pub whole_image: bool,
    /// How many lanes the request ran on: `1` for a direct request, the job's lanes once
    /// checked out (`0` when it never got any).
    pub lanes: u32,
}

impl Served {
    /// How a request planned as `route` ran, given the lanes its job reported running on
    /// (`job_lanes`, unused for a direct request).
    pub(super) const fn new(route: Route, job_lanes: u32) -> Self {
        match route {
            Route::Direct => Self {
                direct: true,
                whole_image: false,
                lanes: 1,
            },
            Route::Job(plan) => Self {
                direct: false,
                whole_image: plan.whole_image,
                lanes: job_lanes,
            },
        }
    }
}

/// Logs how a request ended: without it a request that SUCCEEDS leaves no trace on the
/// console, so "the console went quiet" cannot tell "stopped being asked" from "stopped
/// answering".
///
/// `served` is how it ran; `elapsed` runs from planning to the end of the stream, viewer
/// wait (FIFO turn or whole-image slot) included.
pub(super) fn log(
    request: &RenderRequest,
    ask: Ask,
    served: Served,
    elapsed: Duration,
    outcome: &StreamOutcome,
) {
    tracing::info!(
        request_id = request.request_id,
        intent = ?ask.intent,
        size = %format_args!("{}x{}", request.scene.width, request.scene.height),
        samples = request.samples,
        route = if served.direct { "direct" } else { "job" },
        whole_image = served.whole_image,
        lanes = served.lanes,
        ?elapsed,
        %outcome,
        "coordinator: served a render request"
    );
}
