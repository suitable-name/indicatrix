//! The producer side of [`super::run_stream_with`]: whatever fills a streamed request's
//! samples on its own thread while the emitter owns the socket.
//!
//! [`super::run_stream`] (a plain worker, or a coordinator's own lane served directly)
//! produces with the local tracer (`super::tracer::run_tracer`). A coordinator job
//! produces with a lane pool over joined workers and its own lane, handing each finished
//! chunk to [`ProducerSink::add_chunk`] -- chunks that are NOT contiguous with each other,
//! which is why a coordinator `FRAME` carries a set of samples (`first_sample =
//! request.first_sample`, `samples` exact), see `indicatrix_net`'s `FrameHeader` docs.

use super::tracer::{SharedState, TracerJob, run_tracer};
use glam::Vec3;
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_net::messages::{ErrorMsg, error_codes};
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

/// What a streamed request turns into at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// Radiance: `FRAME`/`PREVIEW` per the request's `TransferMode`, or `DISPLAY_FRAME`s
    /// for `TransferMode::DisplayOnly`; then `DONE`.
    Radiance,
    /// A `FinalImageRequest`: `PROGRESS` heartbeats, then one `FINAL_IMAGE` (always a PNG
    /// tone-mapped into this color space with the GUI export's own
    /// `indicatrix::renderer::tonemap::tonemap_accumulation`), then `DONE`.
    FinalImage(indicatrix::color::ColorSpace),
}

/// How a producer ended, for [`ProducerSink::finish`].
#[derive(Debug)]
pub enum ProducerOutcome {
    /// Every requested sample was added. `final_total`, when given, is the producer's own
    /// sum of the whole request (bit-deterministic for a fixed chunk partition) and
    /// replaces the emitter's arrival-order running total for the final output.
    Complete {
        /// The whole request's summed radiance, `width * height` long.
        final_total: Option<Vec<Vec3>>,
        /// v16: samples of a `FinalImageRequest`'s viewer-reserved range this producer
        /// rendered itself because the viewer's contribution didn't arrive in time or
        /// was invalid. `0` for every `RENDER` and for a `FinalImageRequest` with no
        /// reserved share.
        reclaimed_samples: u32,
    },
    /// Stopped because the cancel flag was raised (the emitter then answers
    /// `DONE { cancelled: true }`). Reported as an internal failure if the flag was NOT
    /// raised -- a producer must never stop short silently.
    Cancelled,
    /// The request cannot complete: the emitter sends this as a stream `ERROR` and no
    /// `DONE`.
    Failed(ErrorMsg),
}

/// A producer's handle on one streamed request: add chunks, watch for cancellation,
/// finish. Dropping it without [`Self::finish`] (a panic on the producer thread) ends
/// the stream as a trace panic, so the emitter never waits forever.
pub struct ProducerSink {
    state: Arc<Mutex<SharedState>>,
    cancel: Arc<AtomicBool>,
    progress: mpsc::Sender<()>,
    /// The request's `first_sample`: the anchor every coordinator `FRAME` names.
    anchor: u32,
    pixels: usize,
}

impl ProducerSink {
    pub(super) const fn new(
        state: Arc<Mutex<SharedState>>,
        cancel: Arc<AtomicBool>,
        progress: mpsc::Sender<()>,
        anchor: u32,
        pixels: usize,
    ) -> Self {
        Self {
            state,
            cancel,
            progress,
            anchor,
            pixels,
        }
    }

    /// Whether the emitter has asked the producer to stop (a viewer `CANCEL`, a
    /// pipelined request, a closed connection, or a transport error).
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Folds one finished chunk (`done` samples summed into `sum`, `width * height` long)
    /// into the pending delta and the progress count, and wakes the emitter. A
    /// wrongly-sized `sum` or an empty chunk is ignored.
    pub fn add_chunk(&self, done: u32, sum: &[Vec3]) {
        if done == 0 || sum.len() != self.pixels {
            return;
        }
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        guard.pending_delta.add_set(self.anchor, done, sum);
        guard.samples_done += done;
        drop(guard);
        let _ = self.progress.send(());
    }

    /// Ends production with `outcome` (see [`ProducerOutcome`]) and wakes the emitter.
    pub fn finish(self, outcome: ProducerOutcome) {
        let cancelled = self.is_cancelled();
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match outcome {
            ProducerOutcome::Complete {
                final_total,
                reclaimed_samples,
            } => {
                guard.final_total = final_total.filter(|t| t.len() == self.pixels);
                guard.reclaimed_samples = reclaimed_samples;
            }
            ProducerOutcome::Cancelled if cancelled => {}
            ProducerOutcome::Cancelled => {
                guard.failed = Some(ErrorMsg {
                    code: error_codes::TRACE_PANIC,
                    message: "internal error: the request stopped before it completed".to_string(),
                    // Overwritten with the real request_id by
                    // `stream_emit::emitter::run_stream_loop` on its way out (the one
                    // place that knows it) -- see that function's own `StreamOutcome::Failed`.
                    request_id: None,
                });
            }
            ProducerOutcome::Failed(error) => guard.failed = Some(error),
        }
        guard.finished = true;
        drop(guard);
        let _ = self.progress.send(());
    }

    /// The local tracer as the producer (a plain worker's request, or a coordinator's
    /// own lane served directly): see [`run_tracer`].
    pub(super) fn run_tracer(self, job: &TracerJob, gpu: &GpuBackend) {
        run_tracer(job, gpu, &self.state, &self.cancel, &self.progress);
    }
}

/// The local tracer as a [`super::run_stream_with`] producer for `request`: what
/// [`super::run_stream`] streams, usable with any [`Output`].
pub fn local_tracer(
    request: &indicatrix_net::messages::RenderRequest,
    threads: usize,
    gpu: &Arc<GpuBackend>,
    compute_mode: crate::cli::ComputeMode,
) -> impl FnOnce(ProducerSink) + Send + 'static {
    let job = TracerJob {
        scene: request.scene.clone(),
        first_sample: request.first_sample,
        samples: request.samples,
        threads,
        compute_mode,
    };
    let gpu = Arc::clone(gpu);
    move |sink| sink.run_tracer(&job, &gpu)
}

impl Drop for ProducerSink {
    fn drop(&mut self) {
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !guard.finished {
            guard.finished = true;
            guard.panicked = true;
            drop(guard);
            let _ = self.progress.send(());
        }
    }
}
