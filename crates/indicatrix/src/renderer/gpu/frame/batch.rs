//! The batch stepper: [`GpuFrameRenderer::batch_step`] and friends, which
//! `renderer::gpu_backend::GpuBackend::try_accumulate_batch_cancellable` drives to trace a
//! LIST of pictures back to back without ever letting the GPU queue run dry at a picture
//! boundary.
//!
//! # What "keeping the queue fed" means here
//!
//! The single-request path ([`GpuFrameRenderer::accumulate_turn`]) drains its one pending
//! chunk before every return, so two pictures traced one after the other always leave the
//! GPU idle for the host round trip between them. The stepper instead treats the whole
//! batch as ONE chunk stream: chunk `k` of the stream is dispatched, THEN the previous
//! chunk of the stream is drained -- whichever picture that previous chunk belonged to. The
//! first chunk of picture `i + 1` is therefore queued before the last chunk of picture `i`
//! is read back (the exact overlap [`GpuFrameRenderer::run_turn_chunks`] already has
//! between chunks of ONE picture, extended across the boundary).
//!
//! # Why no second set of scene buffers is needed
//!
//! Picture `i + 1`'s camera/material/planes/finishes are written into the SAME persistent
//! [`FrameSceneBuffers`] while picture `i`'s last chunk may still be executing. That is
//! safe for the reason `GpuFrameRenderer::params_buffers` already documents: `wgpu`
//! orders `queue.write_buffer` with `queue.submit` in submission order, so the write lands
//! strictly after every earlier submission that reads the buffer, and strictly before the
//! next one. A buffer that has to GROW (`FrameSceneBuffers::update`) or an HDR environment
//! that changes is replaced by a new `wgpu::Buffer`; the in-flight chunk's bind group keeps
//! the old one alive until it completes. Nothing in the stepper ever waits for the GPU
//! before uploading the next picture's scene.
//!
//! The per-chunk OUTPUT side (`outputs`/`pixel_outputs`/`staging`/`params_buffers`) is the
//! existing two-slot ring, indexed by a batch-global chunk counter instead of the per-
//! picture `chunk_index`, so consecutive chunks always land in different slots even across
//! a picture boundary. [`GpuFrameRenderer::batch_begin_turn`] sizes the ring ONCE for the
//! largest picture of the batch before anything is in flight, so no `ensure_*` call during
//! the stream can replace a buffer a pending chunk still uses.
//!
//! # Determinism
//!
//! Every value a chunk's output depends on (absolute sample offset, pixel offset, scene)
//! comes from its own picture's [`TurnRequest`]; the ring slot, the chunk size and the
//! order chunks are queued in change only wall-clock behaviour. Each picture is summed
//! into its own zeroed scratch buffer, so a picture's result is bit-identical to running
//! it alone.

#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::sync::atomic::Ordering;

use glam::Vec3;

use super::{
    ChunkCursor, ChunkFrameState, GpuFrameError, GpuPipelineKind, PendingChunk, TurnRequest,
    TurnSetup, dispatch::chunk_pixels_for, renderer::GpuFrameRenderer,
};

/// What one [`GpuFrameRenderer::batch_step`] learned about the pictures, in input order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchEvent {
    /// Every sample of this picture is traced and summed into its scratch buffer.
    Done(usize),
    /// The renderer cannot serve this picture (unsupported material or environment);
    /// nothing of it was summed. Later pictures are unaffected.
    Declined(usize),
    /// `cancel` fired: this picture and every later one are cancelled; nothing of them
    /// was summed. Earlier pictures keep their `Done`.
    Cancelled(usize),
}

/// The result of one [`GpuFrameRenderer::batch_step`].
#[derive(Debug, Default)]
pub struct BatchStep {
    /// Pictures that finished (one way or another) during this step, in input order.
    pub(crate) events: Vec<BatchEvent>,
    /// Whether this step queued a new chunk (the caller counts these against its turn
    /// budget).
    pub(crate) dispatched: bool,
}

/// One chunk queued but not yet read back, with the picture it belongs to.
struct BatchPending {
    chunk: PendingChunk,
    item: usize,
    spp: u32,
    /// The chunk is the picture's last, so draining it completes the picture.
    last_of_item: bool,
}

/// The per-picture setup [`GpuFrameRenderer::prepare_turn`] produced, valid until the
/// next picture starts or the turn is handed to another request.
struct BatchSetup<'a> {
    item: usize,
    state: ChunkFrameState<'a>,
    byte_budget_pixels: usize,
}

/// The caller-owned progress through one batch, carried across turns (and across other
/// requests' turns) exactly like [`ChunkCursor`] is for a single request.
pub struct BatchState<'a> {
    next_item: usize,
    cursor: ChunkCursor,
    /// Batch-global chunk counter: the output-slot ring index.
    global_chunk: usize,
    pending: Option<BatchPending>,
    setup: Option<BatchSetup<'a>>,
    /// The pipeline kind the output ring was sized for (see
    /// [`GpuFrameRenderer::batch_begin_turn`]).
    sized_for: GpuPipelineKind,
    finished: bool,
}

impl BatchState<'_> {
    pub(crate) fn new() -> Self {
        Self {
            next_item: 0,
            cursor: ChunkCursor::default(),
            global_chunk: 0,
            pending: None,
            setup: None,
            sized_for: GpuPipelineKind::default(),
            finished: false,
        }
    }

    /// The picture the next step works on (the batch length once all are dispatched).
    pub(crate) const fn next_item(&self) -> usize {
        self.next_item
    }

    /// Whether every picture has been reported (`Done`, `Declined` or `Cancelled`).
    pub(crate) const fn is_finished(&self) -> bool {
        self.finished
    }
}

impl GpuFrameRenderer {
    /// Starts (or resumes after another request's turn) a batch turn: forgets the cached
    /// per-picture setup, because another request may have overwritten the scene buffers
    /// since, and sizes the output ring for the largest picture of `requests`.
    ///
    /// Must be called with nothing in flight (`state` has no pending chunk) -- true at the
    /// start of every turn, since a yielded turn drains via [`Self::batch_yield`].
    pub(crate) fn batch_begin_turn<'a>(
        &mut self,
        requests: &[TurnRequest<'a>],
        state: &mut BatchState<'a>,
    ) {
        debug_assert!(
            state.pending.is_none(),
            "a turn must start with nothing in flight"
        );
        state.setup = None;
        self.batch_reserve(requests);
        state.sized_for = self.pipeline_kind;
    }

    /// Grows the two-slot output ring to the largest picture of `requests`, using exactly
    /// the sizing [`Self::prepare_turn`] applies per picture, so the per-picture
    /// `ensure_*` calls inside it are no-ops while a chunk is in flight.
    fn batch_reserve(&mut self, requests: &[TurnRequest<'_>]) {
        let mut max_tuples = 0usize;
        let mut max_pixels = 0usize;
        for request in requests {
            let num_pixels = request.scene.width as usize * request.scene.height as usize;
            if request.spp == 0 || num_pixels == 0 || !request.scene.material.gpu_supported() {
                continue;
            }
            let budget = chunk_pixels_for(self.chunk_budget_bytes, request.spp, num_pixels);
            let budget = self.cap_byte_budget_for_wavefront(budget, request.spp);
            max_pixels = max_pixels.max(budget);
            max_tuples = max_tuples.max(budget * request.spp as usize);
        }
        if max_pixels == 0 {
            return;
        }
        self.ensure_capacity(max_tuples);
        self.ensure_pixel_capacity(max_pixels);
        self.ensure_staging_capacity(max_pixels);
        self.ensure_params_buffers();
        self.ensure_reduce_params_buffers();
    }

    /// One step of the batch: queues at most one chunk of the picture at
    /// `state.next_item()` (preparing its scene first when it is a new picture or the
    /// turn was just resumed), then reads back the chunk queued BEFORE it -- possibly the
    /// last chunk of the previous picture. Pictures that finish are reported in `events`,
    /// in input order.
    ///
    /// `scratches[i]` must be a zeroed buffer of picture `i`'s pixel count by the time a
    /// step first works on picture `i` (see `GpuBackend`'s provisioning); a picture's
    /// samples are summed only into its own scratch.
    ///
    /// Cancellation mirrors the single-request path exactly: `cancel` is polled before
    /// each chunk is queued. A picture whose last chunk is already queued when `cancel`
    /// fires still completes (the single path never polls after its last dispatch); the
    /// first picture that has not queued everything is cancelled, along with all later
    /// ones, and its in-flight chunk is waited on but not summed.
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::DeviceLost`] if this renderer is (or just became) poisoned, a
    /// readback failed, or wgpu reported an uncaptured error. The renderer is abandoned
    /// and every picture not yet reported is the caller's to decline. A per-picture
    /// refusal (unsupported material or environment) is a [`BatchEvent::Declined`], not an
    /// error.
    pub(crate) fn batch_step<'a>(
        &mut self,
        requests: &[TurnRequest<'a>],
        scratches: &mut [Vec<Vec3>],
        state: &mut BatchState<'a>,
    ) -> Result<BatchStep, GpuFrameError> {
        if self.poisoned {
            return Err(GpuFrameError::DeviceLost(
                "GPU renderer poisoned by a previous failed chunk readback".to_string(),
            ));
        }
        if self.turn_in_progress {
            self.abandon_in_flight();
            return Err(GpuFrameError::DeviceLost(
                "GPU renderer poisoned: a previous turn unwound without completing".to_string(),
            ));
        }
        self.turn_in_progress = true;
        let result = self.batch_step_body(requests, scratches, state);
        self.turn_in_progress = false;
        result
    }

    /// Drains whatever chunk is still in flight (summing it) so another request can take
    /// the renderer, reporting a picture that this completes. The batch-side twin of the
    /// final drain in [`Self::run_turn_chunks`].
    ///
    /// # Errors
    ///
    /// Same as [`Self::batch_step`].
    pub(crate) fn batch_yield(
        &mut self,
        scratches: &mut [Vec<Vec3>],
        state: &mut BatchState<'_>,
    ) -> Result<Vec<BatchEvent>, GpuFrameError> {
        if self.poisoned {
            return Err(GpuFrameError::DeviceLost(
                "GPU renderer poisoned by a previous failed chunk readback".to_string(),
            ));
        }
        let mut step = BatchStep::default();
        self.drain_batch_pending(state, scratches, true, &mut step)?;
        state.setup = None;
        self.check_uncaptured_error()?;
        Ok(step.events)
    }

    /// The body of [`Self::batch_step`], split out so its poisoning bookkeeping wraps
    /// every early return.
    fn batch_step_body<'a>(
        &mut self,
        requests: &[TurnRequest<'a>],
        scratches: &mut [Vec<Vec3>],
        state: &mut BatchState<'a>,
    ) -> Result<BatchStep, GpuFrameError> {
        let mut step = BatchStep::default();
        if state.finished {
            return Ok(step);
        }

        // Nothing left to queue: read back the tail and close the batch.
        let Some(request) = requests.get(state.next_item) else {
            self.drain_batch_pending(state, scratches, true, &mut step)?;
            state.finished = true;
            self.check_uncaptured_error()?;
            return Ok(step);
        };
        let num_pixels = request.scene.width as usize * request.scene.height as usize;
        let trivial = request.spp == 0 || num_pixels == 0;

        // Pictures that never reach the GPU. Decided BEFORE the cancel poll, like the
        // single path (which returns `Done` for an empty request and `Declined` for an
        // unsupported material before its chunk loop ever polls `cancel`). A previous
        // picture's last chunk is read back first so events stay in input order.
        if trivial || !request.scene.material.gpu_supported() {
            if state.pending.is_some() {
                self.drain_batch_pending(state, scratches, true, &mut step)?;
                self.check_uncaptured_error()?;
                return Ok(step);
            }
            step.events.push(if trivial {
                BatchEvent::Done(state.next_item)
            } else {
                BatchEvent::Declined(state.next_item)
            });
            state.next_item += 1;
            state.cursor = ChunkCursor::default();
            return Ok(step);
        }

        // The single path's per-chunk cancel poll. A pending chunk that is the previous
        // picture's last one belongs to a picture that finished dispatching, so it is
        // kept; a pending chunk of the current picture is waited on and dropped.
        if request
            .cancel
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            let keep = state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.last_of_item);
            self.drain_batch_pending(state, scratches, keep, &mut step)?;
            step.events.push(BatchEvent::Cancelled(state.next_item));
            state.finished = true;
            self.check_uncaptured_error()?;
            return Ok(step);
        }

        // A pipeline-kind change by another thread (it only needs the mutex) could change
        // the sizing `batch_reserve` assumed; re-size with nothing in flight.
        if self.pipeline_kind != state.sized_for {
            self.drain_batch_pending(state, scratches, true, &mut step)?;
            state.setup = None;
            self.batch_reserve(requests);
            state.sized_for = self.pipeline_kind;
        }

        // New picture, or this turn was just resumed: (re)upload its scene.
        if !self.ensure_batch_setup(request, state, scratches, &mut step)? {
            return Ok(step);
        }

        // Queue this picture's next chunk, THEN read back the one queued before it.
        let setup = state
            .setup
            .as_ref()
            .expect("the setup above is populated for this picture");
        let first_pixel = state.cursor.first_pixel;
        let chunk_pixels = Self::next_chunk_pixels(
            self.ns_per_tuple_ema,
            setup.byte_budget_pixels,
            request.spp,
            num_pixels,
        );
        let pixels_this_chunk = chunk_pixels.min(num_pixels - first_pixel);
        let frame_buffers = self
            .scene_buffers
            .as_ref()
            .expect("prepare_turn stored the scene buffers");
        let chunk = self.dispatch_chunk(
            &setup.state,
            frame_buffers,
            first_pixel,
            pixels_this_chunk,
            state.global_chunk,
        );
        let next_first_pixel = first_pixel + pixels_this_chunk;
        let last_of_item = next_first_pixel >= num_pixels;
        let previous = state.pending.replace(BatchPending {
            chunk,
            item: state.next_item,
            spp: request.spp,
            last_of_item,
        });
        step.dispatched = true;
        state.global_chunk += 1;
        if last_of_item {
            state.next_item += 1;
            state.cursor = ChunkCursor::default();
            state.setup = None;
        } else {
            state.cursor.first_pixel = next_first_pixel;
            state.cursor.chunk_index += 1;
        }
        if let Some(previous) = previous {
            self.drain_one(previous, scratches, true, &mut step)?;
        }
        self.check_uncaptured_error()?;
        Ok(step)
    }

    /// Uploads the scene of the picture at `state.next_item` unless that is already done
    /// (a new picture, or a turn that was just resumed).
    ///
    /// `Ok(false)` when the GPU declined the picture: it is reported as `Declined`, the batch
    /// moves on to the next picture, and `step` is complete.
    fn ensure_batch_setup<'a>(
        &mut self,
        request: &TurnRequest<'a>,
        state: &mut BatchState<'a>,
        scratches: &mut [Vec<Vec3>],
        step: &mut BatchStep,
    ) -> Result<bool, GpuFrameError> {
        if state
            .setup
            .as_ref()
            .is_some_and(|setup| setup.item == state.next_item)
        {
            return Ok(true);
        }
        match self.prepare_turn(request) {
            Ok(TurnSetup {
                state: chunk_state,
                frame_buffers,
                byte_budget_pixels,
            }) => {
                self.scene_buffers = Some(frame_buffers);
                state.setup = Some(BatchSetup {
                    item: state.next_item,
                    state: chunk_state,
                    byte_budget_pixels,
                });
                Ok(true)
            }
            Err(GpuFrameError::DeviceLost(why)) => Err(GpuFrameError::DeviceLost(why)),
            Err(error) => {
                tracing::debug!("GPU declined a batch picture, using CPU tracer: {error}");
                self.drain_batch_pending(state, scratches, true, step)?;
                step.events.push(BatchEvent::Declined(state.next_item));
                state.next_item += 1;
                state.cursor = ChunkCursor::default();
                state.setup = None;
                self.check_uncaptured_error()?;
                Ok(false)
            }
        }
    }

    /// Reads back the pending chunk, if any. `sum: false` waits for it but throws its
    /// samples away (the cancelled picture's chunk).
    fn drain_batch_pending(
        &mut self,
        state: &mut BatchState<'_>,
        scratches: &mut [Vec<Vec3>],
        sum: bool,
        step: &mut BatchStep,
    ) -> Result<(), GpuFrameError> {
        state.pending.take().map_or(Ok(()), |pending| {
            self.drain_one(pending, scratches, sum, step)
        })
    }

    /// Reads back one chunk into its own picture's scratch, reporting the picture as
    /// `Done` when that was its last chunk. A failed readback abandons the renderer (its
    /// staging slots are stuck mapped), exactly like the single path.
    fn drain_one(
        &mut self,
        pending: BatchPending,
        scratches: &mut [Vec<Vec3>],
        sum: bool,
        step: &mut BatchStep,
    ) -> Result<(), GpuFrameError> {
        let BatchPending {
            chunk,
            item,
            spp,
            last_of_item,
        } = pending;
        let accum = sum.then(|| scratches[item].as_mut_slice());
        if let Err(error) = self.drain_pending_chunk(chunk, spp, accum) {
            self.abandon_in_flight();
            return Err(error);
        }
        if sum && last_of_item {
            step.events.push(BatchEvent::Done(item));
        }
        Ok(())
    }
}
