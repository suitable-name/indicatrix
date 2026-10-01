//! The one `info!` line per request a coordinator serves.

use super::plan::Ask;
use crate::stream_emit::StreamOutcome;
use indicatrix_net::messages::RenderRequest;
use std::time::Duration;

/// Logs how a request ended: without it a request that SUCCEEDS leaves no trace on the
/// console, so "the console went quiet" cannot tell "stopped being asked" from "stopped
/// answering".
///
/// `direct` is the route (the own lane served it like a plain worker, or a job over the
/// lane pool); `elapsed` runs from planning to the end of the stream, viewer-FIFO wait
/// included.
pub(super) fn log(
    request: &RenderRequest,
    ask: Ask,
    direct: bool,
    elapsed: Duration,
    outcome: &StreamOutcome,
) {
    tracing::info!(
        request_id = request.request_id,
        intent = ?ask.intent,
        size = %format_args!("{}x{}", request.scene.width, request.scene.height),
        samples = request.samples,
        route = if direct { "direct" } else { "job" },
        ?elapsed,
        %outcome,
        "coordinator: served a render request"
    );
}
