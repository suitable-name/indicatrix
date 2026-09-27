//! The per-frame accumulate loop: [`GpuFrameRenderer::accumulate`]/
//! [`GpuFrameRenderer::accumulate_cancellable`] and the turn-granular machinery
//! (`accumulate_turn`/`prepare_turn`/`run_turn_chunks`) `renderer::gpu_backend::GpuBackend`
//! drives for fairness across concurrent requests. Per-chunk GPU dispatch itself lives in
//! [`super::dispatch`]; this module is the loop that decides WHEN to dispatch a chunk and
//! sums its result.

use std::sync::atomic::{AtomicBool, Ordering};

use glam::Vec3;

use crate::{
    optics::raytracer::environment::environment_white_balance,
    renderer::buffers::{GpuCameraParams, GpuGemMaterial, encode_facet_finishes},
};

use super::{
    AccumulateOutcome, ChunkCursor, ChunkFrameState, ChunkSlotResources, ChunkTurnOutcome,
    GpuFrameError, GpuFrameScene, GpuPipelineKind, PendingChunk, TurnRequest, TurnSetup,
    bind_groups::wavefront_pixel_cap,
    classify_material,
    dispatch::{chunk_pixels_for, environment_params},
    renderer::GpuFrameRenderer,
    scene_buffers::{FrameSceneBuffers, SceneBufferInputs},
};

impl GpuFrameRenderer {
    /// Traces `spp` samples per pixel, starting at sample index `sample_offset`, and
    /// ADDS each pixel's summed XYZ into `accum`.
    ///
    /// Accumulating rather than overwriting mirrors the CPU path's
    /// `acc_chunk[i] += sample_sum`, so a caller's progressive-accumulation buffer and its
    /// sample counter mean the same thing regardless of backend. `sample_offset` must be
    /// the number of samples already in `accum` for these pixels: the shader derives each
    /// thread's seed and stratified jitter from the absolute sample index, so reusing an
    /// offset would re-draw identical samples and bias the average.
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::UnsupportedEnvironment`] if the scene uses an environment the
    /// megakernel has no `env_mode` for -- currently unreachable (see that variant's own
    /// doc comment; HDR maps render on the GPU).
    /// [`GpuFrameError::DeviceLost`] if the device stops making forward progress
    /// mid-frame -- see that variant's own doc comment; `renderer::gpu_backend::GpuBackend`
    /// is the caller expected to react to it by permanently disabling the GPU for the rest
    /// of the process.
    ///
    /// # Panics
    ///
    /// Panics if `accum.len()` is not `width * height`.
    pub fn accumulate(
        &mut self,
        scene: &GpuFrameScene<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> Result<(), GpuFrameError> {
        // classify_material is the ONE place this decision is made -- see the parent
        // module's doc comment's "Material-class kernel specialisation" section.
        // accumulate_via_pipeline is parameterised on which pipeline to dispatch through,
        // so run_specialisation_equivalence can force a class directly and compare
        // against what this wrapper picks. cancel: None -- this entry point never stops
        // early.
        let pipeline_class = classify_material(scene.material);
        self.accumulate_via_pipeline(scene, pipeline_class, sample_offset, spp, accum, None)?;
        Ok(())
    }

    /// Like [`Self::accumulate`], but checked for cancellation between chunks.
    ///
    /// `cancel` is polled once per loop iteration, between one chunk's dispatch and the
    /// next's (never mid-chunk) -- see [`AccumulateOutcome::Cancelled`]'s doc comment for
    /// the drain-then-discard guarantee this makes about `accum` once it fires. Intended
    /// caller: `renderer::gpu_backend::GpuBackend::try_accumulate_cancellable`, for a long
    /// dispatch whose caller no longer needs the result (e.g. a disconnected client) and
    /// would rather reclaim the GPU/CPU than wait it out.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    pub fn accumulate_cancellable(
        &mut self,
        scene: &GpuFrameScene<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> Result<AccumulateOutcome, GpuFrameError> {
        let pipeline_class = classify_material(scene.material);
        self.accumulate_via_pipeline(
            scene,
            pipeline_class,
            sample_offset,
            spp,
            accum,
            Some(cancel),
        )
    }

    /// The body [`Self::accumulate`]/[`Self::accumulate_cancellable`] delegate to,
    /// parameterised on `pipeline_class` (one of `super::material_class`'s values) rather
    /// than deriving it from `scene.material` -- lets
    /// [`super::equivalence::run_specialisation_equivalence`] force the GENERIC pipeline
    /// for a material [`classify_material`] would otherwise route to a specialised one,
    /// and vice versa, which is what proves the two pipelines agree.
    ///
    /// A thin driver over [`Self::accumulate_turn`]: runs turns of unlimited chunk budget
    /// (`max_chunks: usize::MAX`) back to back, starting from a fresh [`ChunkCursor`],
    /// until the whole request is `Done`/`Cancelled` -- i.e. this function alone never
    /// yields the renderer mid-request, exactly its pre-chunk-fairness behaviour. See
    /// [`Self::accumulate_turn`]'s own doc comment for the turn-granular entry point
    /// `renderer::gpu_backend::GpuBackend` actually drives for fairness across concurrent
    /// requests.
    ///
    /// `cancel`, when `Some`, is [`Self::accumulate_cancellable`]'s cooperative
    /// cancellation flag; `None` (every self-test, and [`Self::accumulate`]) means "never
    /// cancel".
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    pub(super) fn accumulate_via_pipeline(
        &mut self,
        scene: &GpuFrameScene<'_>,
        pipeline_class: u32,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
        cancel: Option<&AtomicBool>,
    ) -> Result<AccumulateOutcome, GpuFrameError> {
        let mut cursor = ChunkCursor::default();
        let request = TurnRequest {
            scene,
            pipeline_class,
            sample_offset,
            spp,
            cancel,
            max_chunks: usize::MAX,
        };
        loop {
            match self.accumulate_turn(&request, accum, &mut cursor)? {
                ChunkTurnOutcome::Done => return Ok(AccumulateOutcome::Done),
                ChunkTurnOutcome::Cancelled => return Ok(AccumulateOutcome::Cancelled),
                // Unreachable with an unlimited turn budget (the turn loop below only
                // stops early on `max_chunks`, never hit here) -- kept rather than
                // `unreachable!()` so this driver stays correct even if that invariant
                // ever changes.
                ChunkTurnOutcome::MoreWork => {}
            }
        }
    }

    /// One fairness "turn" through the chunk loop: dispatches at most
    /// `request.max_chunks` chunks starting from `cursor`'s progress, updates `cursor` in
    /// place, and returns without leaving anything in-flight on the GPU -- see
    /// [`ChunkTurnOutcome::MoreWork`]'s doc comment for exactly what "nothing in flight"
    /// guarantees for a caller that hands this same renderer to a DIFFERENT request's
    /// turn next.
    ///
    /// Every turn -- including a RESUMED one -- re-verifies and re-uploads
    /// `self.scene_buffers` via [`FrameSceneBuffers::ensure`] before dispatching anything
    /// (see [`Self::prepare_turn`]). That upload is cheap (four small buffers) relative
    /// to a chunk's own GPU work, and it is what makes resuming correct: a different
    /// request's turn may have run in between and overwritten those same persistent
    /// buffers with ITS scene, and this call has no other way to know. See
    /// `renderer::gpu_backend`'s module doc comment ("Scene re-upload: the fairness
    /// cost") for the resulting overhead.
    ///
    /// Split into [`Self::prepare_turn`] (setup) and [`Self::run_turn_chunks`] (the
    /// dispatch/drain loop) purely to keep this function under clippy's function-length
    /// limit; [`TurnRequest`] bundles the arguments both this function and
    /// [`Self::accumulate_via_pipeline`] (its `max_chunks: usize::MAX` driver) share, to
    /// stay under the argument-count limit too.
    ///
    /// This is a thin poisoning wrapper around [`Self::accumulate_turn_body`],
    /// which does the actual work -- split out so every early return in the body (there
    /// are several) doesn't also need to remember to clear `self.turn_in_progress`.
    /// Checks `self.poisoned` first (a renderer `abandon_in_flight` already gave up
    /// on declines immediately, without touching the GPU again), then
    /// `self.turn_in_progress`: still `true` here means the PREVIOUS call into this
    /// function never reached its own clearing step below, i.e. it unwound mid-turn,
    /// which can leave a staging buffer mapped exactly like a returned `Err` would -- see
    /// that field's own doc comment.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s, plus [`GpuFrameError::DeviceLost`] if
    /// this renderer is (or just became) poisoned.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    pub(crate) fn accumulate_turn(
        &mut self,
        request: &TurnRequest<'_>,
        accum: &mut [Vec3],
        cursor: &mut ChunkCursor,
    ) -> Result<ChunkTurnOutcome, GpuFrameError> {
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
        let result = self.accumulate_turn_body(request, accum, cursor);
        self.turn_in_progress = false;
        result
    }

    /// The body [`Self::accumulate_turn`] wraps with poisoning checks -- see that
    /// function's own doc comment for why the split exists.
    fn accumulate_turn_body(
        &mut self,
        request: &TurnRequest<'_>,
        accum: &mut [Vec3],
        cursor: &mut ChunkCursor,
    ) -> Result<ChunkTurnOutcome, GpuFrameError> {
        let scene = request.scene;
        let num_pixels = scene.width as usize * scene.height as usize;
        assert_eq!(
            accum.len(),
            num_pixels,
            "accumulation buffer must have one entry per pixel"
        );
        if request.spp == 0 || num_pixels == 0 {
            return Ok(ChunkTurnOutcome::Done);
        }

        // GemMaterial::gpu_supported is the crate's routing predicate; a caller assembling
        // a full scene must consult it before routing to the GPU. Currently unreachable
        // (unconditionally true), but enforced so a future material kind the megakernel
        // cannot handle produces an error here rather than a plausible-looking wrong image.
        if !scene.material.gpu_supported() {
            return Err(GpuFrameError::UnsupportedMaterial(
                scene.material.name.clone(),
            ));
        }

        let TurnSetup {
            state,
            frame_buffers,
            byte_budget_pixels,
        } = self.prepare_turn(request)?;

        // frame_buffers is threaded through by value (not `&mut self.scene_buffers`
        // directly) so `run_turn_chunks` can keep borrowing `self` mutably for
        // `dispatch_chunk`/`drain_pending_chunk` without an overlapping borrow -- see
        // `FrameSceneBuffers::ensure`'s own doc comment. Not put back into
        // `self.scene_buffers` on an `Err` from below (a `?`-propagated `DeviceLost`, or
        // `prepare_turn`'s own possible `UnsupportedEnvironment`): the former means this
        // whole renderer is about to be discarded (see `GpuFrameError::DeviceLost`'s doc
        // comment), and the latter has already taken `self.scene_buffers` out via
        // `FrameSceneBuffers::ensure` before failing -- either way leaving
        // `self.scene_buffers` as `None` there is harmless, just an extra rebuild next
        // call.
        let (frame_buffers, cancelled) = self.run_turn_chunks(
            request,
            &state,
            frame_buffers,
            byte_budget_pixels,
            accum,
            cursor,
        )?;

        // Put the (possibly newly-created) persistent scene buffers back for the next
        // turn -- this request's own resumption, or a completely different request's --
        // to reuse.
        self.scene_buffers = Some(frame_buffers);

        // A wgpu validation/internal error `GpuContext::acquire_async`'s
        // `on_uncaptured_error` handler observed during this turn's dispatches, but that
        // by wgpu's own default would otherwise have panicked the calling thread instead
        // of returning here. `swap` both reads and clears it in one step -- this turn
        // (and this renderer) is about to be abandoned either way, so there is nothing
        // left for a later turn to still need it set.
        if self
            .ctx
            .validation_error_seen
            .swap(false, Ordering::Relaxed)
        {
            self.abandon_in_flight();
            // Folds in the actual wgpu error text if the
            // `on_uncaptured_error` handler captured one -- see
            // `GpuContext::last_uncaptured_error`'s doc comment for why this avoids
            // becoming a dead end (a `tracing::error!` alone, with no subscriber
            // installed in most binaries, would otherwise lose the message). Cleared in
            // the same step so a later, unrelated turn doesn't report a stale message.
            let detail = self
                .ctx
                .last_uncaptured_error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            let message = detail.map_or_else(
                || {
                    "wgpu reported an uncaptured validation/device error during this turn \
                     (no error text was captured)"
                        .to_string()
                },
                |detail| {
                    format!(
                        "wgpu reported an uncaptured validation/device error during this turn: \
                         {detail}"
                    )
                },
            );
            return Err(GpuFrameError::DeviceLost(message));
        }

        if cancelled {
            return Ok(ChunkTurnOutcome::Cancelled);
        }
        if cursor.first_pixel >= num_pixels {
            return Ok(ChunkTurnOutcome::Done);
        }
        Ok(ChunkTurnOutcome::MoreWork)
    }

    /// The setup [`Self::accumulate_turn`] must redo on EVERY turn before it can
    /// dispatch a single chunk -- see that function's own doc comment for why a resumed
    /// turn cannot skip this. Split out purely to keep `accumulate_turn` under clippy's
    /// function-length limit.
    ///
    /// Fallible: every `EnvironmentSource` variant maps to
    /// an `env_mode` (see [`environment_params`]), but [`FrameSceneBuffers::ensure`] can
    /// fail to (re)build the HDR
    /// environment buffers for an oversized map, so a genuine `Err` path remains below.
    /// [`GpuFrameError::UnsupportedMaterial`]
    /// is still checked by [`Self::accumulate_turn_body`] itself, before this is called.
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::UnsupportedEnvironment`] -- see [`FrameSceneBuffers::ensure`]'s
    /// `# Errors`.
    fn prepare_turn<'a>(
        &mut self,
        request: &TurnRequest<'a>,
    ) -> Result<TurnSetup<'a>, GpuFrameError> {
        let scene = request.scene;
        let num_pixels = scene.width as usize * scene.height as usize;

        let (
            env_mode,
            temp_k,
            spot_mult,
            exposure,
            light_yaw,
            light_pitch,
            use_d65,
            studio_model,
            backdrop,
        ) = environment_params(scene.environment);
        // The per-preset adaptation the CPU tracer applies (identity for an HDR map and
        // for the lit D65 models, Planckian for the rest), so a hybrid frame's CPU and
        // GPU tiles agree.
        let white_balance = environment_white_balance(scene.environment);

        let gpu_material = GpuGemMaterial::encode(scene.material);
        let gpu_finishes = encode_facet_finishes(scene.facet_finishes, scene.planes.len());

        self.ensure_specialized_pipeline(request.pipeline_class);

        // byte_budget_pixels is chunk_budget_bytes's hard ceiling in pixels -- output
        // buffers are sized against it ONCE, up front, so every dispatch below fits
        // without growing buffers mid-frame. See next_chunk_pixels for why the actual
        // per-chunk pixel count is computed fresh every iteration instead.
        let byte_budget_pixels = chunk_pixels_for(self.chunk_budget_bytes, request.spp, num_pixels);
        // chunk_pixels_for's own ceiling is sized off FLOATS_PER_TUPLE (this
        // module tree's production XYZ-only output, 12 bytes/tuple) -- it has no notion
        // of GpuPipelineKind::Wavefront's much larger per-tuple footprint (its widest
        // buffer, `ray_stokes`, is 128 bytes/tuple). A no-op for Megakernel; see
        // Self::cap_byte_budget_for_wavefront's own doc comment.
        let byte_budget_pixels =
            self.cap_byte_budget_for_wavefront(byte_budget_pixels, request.spp);
        self.ensure_capacity(byte_budget_pixels * request.spp as usize);
        self.ensure_pixel_capacity(byte_budget_pixels);
        self.ensure_staging_capacity(byte_budget_pixels);
        self.ensure_params_buffers();
        self.ensure_reduce_params_buffers();

        // camera/material/geometry/facet-finishes are PERSISTENT across every
        // `accumulate` call (see FrameSceneBuffers::ensure), not merely uploaded
        // once per call -- camera_params carries the FULL frame's dimensions, not a
        // chunk-local one. Every chunk below reuses these same four buffers via
        // build_chunk_bind_group, writing only GpuTransportParams's persistent per-slot
        // buffer per chunk. See the parent module's doc comment's "Per-frame uploads,
        // persistent staging" section.
        let camera_params = GpuCameraParams {
            origin: scene.camera.origin.to_array(),
            fov_tan: scene.camera.fov_tan,
            forward: scene.camera.forward.to_array(),
            width: scene.width as f32,
            right: scene.camera.right.to_array(),
            height: scene.height as f32,
            up: scene.camera.up.to_array(),
            num_samples: request.spp,
        };
        // Taken out of `self.scene_buffers` and returned as an OWNED value rather than a
        // `&FrameSceneBuffers` borrowed from `self` -- see `FrameSceneBuffers::ensure`'s
        // doc comment for why: it lets `self.dispatch_chunk`/`self.drain_pending_chunk`
        // keep borrowing `self` freely without conflicting with a borrow this would
        // otherwise be holding from `self.scene_buffers`. The caller
        // ([`Self::accumulate_turn`], via [`Self::run_turn_chunks`]) is responsible for
        // putting it back into `self.scene_buffers` once done. Called EVERY turn, resumed
        // or not -- see `accumulate_turn`'s own doc comment for why that's required now
        // that a different request's turn can run in between two of this one's.
        let frame_buffers = FrameSceneBuffers::ensure(
            self.scene_buffers.take(),
            &self.ctx,
            &SceneBufferInputs {
                camera_params: &camera_params,
                material: &gpu_material,
                planes: scene.planes,
                facet_finishes: &gpu_finishes,
                environment: scene.environment,
            },
        )?;

        let state = ChunkFrameState {
            scene,
            pipeline_class: request.pipeline_class,
            sample_offset: request.sample_offset,
            spp: request.spp,
            env_mode,
            temp_k,
            spot_mult,
            exposure,
            light_yaw,
            light_pitch,
            white_balance,
            use_d65,
            studio_model,
            backdrop,
        };

        Ok(TurnSetup {
            state,
            frame_buffers,
            byte_budget_pixels,
        })
    }

    /// Extra ceiling on `byte_budget_pixels` applied only for
    /// [`GpuPipelineKind::Wavefront`] -- a no-op (returns `byte_budget_pixels` unchanged)
    /// for [`GpuPipelineKind::Megakernel`].
    ///
    /// `dispatch_chunk_wavefront`'s `WavefrontRayBuffers` sizes its widest field,
    /// `stokes`, at `tuples * 8 * 16` bytes -- 128 bytes per (pixel, sample) tuple, an
    /// 8-channel field of `[f32; 4]`s. `chunk_pixels_for`'s own byte budget has no notion
    /// of this: it is sized against `FLOATS_PER_TUPLE` (12 bytes/tuple, this module
    /// tree's XYZ-only production output), which every OTHER per-chunk buffer this module
    /// allocates for the megakernel path stays within. Left uncapped, a chunk near
    /// `chunk_pixels_for`'s dispatch-limited ceiling (`MAX_WORKGROUPS_PER_DIMENSION *
    /// WORKGROUP_SIZE`, ~4.19M tuples) would ask `stokes` alone for ~537 MB -- comfortably
    /// past the WebGPU baseline `max_storage_buffer_binding_size` of 128 MiB -- which
    /// `create_buffer` rejects at validation. Without the `on_uncaptured_error` handler
    /// (see `GpuContext::acquire_async`'s doc comment) that failure panics the calling
    /// thread mid-`dispatch_chunk`, straight into a state where the previous chunk's
    /// staging buffer is never unmapped; with the handler installed, this cap is what
    /// avoids relying on it to catch an entirely predictable and preventable size
    /// mismatch in the first place.
    ///
    /// Queried from the device rather than hard-coded: the WebGPU baseline is 128 MiB,
    /// but a real adapter can advertise more (or, rarely, request a smaller one).
    pub(super) fn cap_byte_budget_for_wavefront(
        &self,
        byte_budget_pixels: usize,
        spp: u32,
    ) -> usize {
        if self.pipeline_kind != GpuPipelineKind::Wavefront {
            return byte_budget_pixels;
        }
        let max_binding_bytes = self.ctx.device.limits().max_storage_buffer_binding_size as usize;
        wavefront_pixel_cap(max_binding_bytes, spp, byte_budget_pixels)
    }

    /// The dispatch/drain loop body of [`Self::accumulate_turn`]: dispatches at most
    /// `request.max_chunks` chunks starting from `cursor`'s progress, then drains
    /// whatever chunk the overlapped pipeline may still have in flight before returning
    /// -- see [`ChunkTurnOutcome::MoreWork`]'s doc comment for why that final drain is
    /// required regardless of how this loop ends. Updates `cursor` in place and returns
    /// `frame_buffers` back to the caller (to put back into `self.scene_buffers`)
    /// alongside whether `request.cancel` fired during this turn. Split out of
    /// `accumulate_turn` purely to keep that function under clippy's function-length
    /// limit.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    fn run_turn_chunks(
        &mut self,
        request: &TurnRequest<'_>,
        state: &ChunkFrameState<'_>,
        frame_buffers: FrameSceneBuffers,
        byte_budget_pixels: usize,
        accum: &mut [Vec3],
        cursor: &mut ChunkCursor,
    ) -> Result<(FrameSceneBuffers, bool), GpuFrameError> {
        let num_pixels = state.scene.width as usize * state.scene.height as usize;

        // One chunk's dispatch+copy is submitted, then (if a PREVIOUS chunk is still
        // pending) that older chunk is mapped/read/summed -- so by the time the CPU
        // blocks on chunk i's result, chunk i+1's GPU work is already queued behind it.
        // See the parent module's doc comment's "Overlapped chunk pipeline" section.
        // Resumed from `cursor`'s progress rather than always starting at pixel 0/chunk
        // 0, so a turn picks up exactly where the previous one (on this same request)
        // left off.
        let mut pending: Option<PendingChunk> = None;
        let mut first_pixel = cursor.first_pixel;
        let mut chunk_index = cursor.chunk_index;
        let mut cancelled = false;
        let mut chunks_this_turn = 0usize;
        while first_pixel < num_pixels {
            if request
                .cancel
                .is_some_and(|flag| flag.load(Ordering::Relaxed))
            {
                cancelled = true;
                break;
            }
            if chunks_this_turn >= request.max_chunks {
                // This turn's budget is spent -- stop dispatching new chunks, but a
                // chunk already dispatched this turn (if any) is still drained below
                // exactly like every other exit from this loop, so nothing is ever left
                // pending across a yielded turn.
                break;
            }

            let chunk_pixels = Self::next_chunk_pixels(
                self.ns_per_tuple_ema,
                byte_budget_pixels,
                state.spp,
                num_pixels,
            );
            let pixels_this_chunk = chunk_pixels.min(num_pixels - first_pixel);

            // Dispatch THEN drain, in that order -- so chunk i+1's GPU work is already
            // queued by the time this call blocks (inside `drain_pending_chunk`) on
            // chunk i's readback. See this function's own "R4" note above and the parent
            // module's doc comment's "Overlapped chunk pipeline" section.
            let new_pending = self.dispatch_chunk(
                state,
                &frame_buffers,
                first_pixel,
                pixels_this_chunk,
                chunk_index,
            );
            if let Some(prev) = pending.take() {
                // Chunk i+1's dispatch (above) is already queued on the GPU
                // by the time this blocks on chunk i's readback -- so if THIS drain
                // fails, chunk i+1's own eventual drain would try to map a slot whose
                // sibling never finished, and any later chunk reusing chunk i's slot
                // would submit into a still-mapped buffer. Draining
                // that still-pending chunk i+1 first is not safe either: the device may
                // already have stopped making forward progress, so its own drain could
                // fail (or hang) too. Abandon both slots now instead of trying.
                if let Err(err) = self.drain_pending_chunk(prev, state.spp, Some(&mut *accum)) {
                    self.abandon_in_flight();
                    return Err(err);
                }
            }
            pending = Some(new_pending);

            first_pixel += pixels_this_chunk;
            chunk_index += 1;
            chunks_this_turn += 1;
        }

        // Whatever this turn ends with -- done, cancelled, or its chunk budget merely
        // spent -- the ONE chunk the overlapped pipeline may still have in flight is
        // drained right here, unconditionally, before this function returns control to
        // the caller. This is the invariant `ChunkTurnOutcome::MoreWork`'s doc comment
        // promises: a turn never returns with GPU work still outstanding, which is what
        // lets a completely different request safely take the next turn on this same
        // renderer. `cancelled` still discards the drained chunk's samples
        // (drain-then-discard, see `AccumulateOutcome::Cancelled`'s doc comment);
        // reaching the pixel budget or this turn's chunk budget both keep them --
        // neither is a cancellation, so nothing traced this turn is thrown away.
        if let Some(chunk) = pending.take() {
            let drain_result = if cancelled {
                self.drain_pending_chunk(chunk, state.spp, None)
            } else {
                self.drain_pending_chunk(chunk, state.spp, Some(&mut *accum))
            };
            // Identical reasoning to the mid-loop drain above -- a failure
            // here means this renderer's staging slot is stuck mapped, so the next
            // caller (a resumed turn on THIS request, or a completely different one)
            // must never be allowed to dispatch into it.
            if let Err(err) = drain_result {
                self.abandon_in_flight();
                return Err(err);
            }
        }

        cursor.first_pixel = first_pixel;
        cursor.chunk_index = chunk_index;

        Ok((frame_buffers, cancelled))
    }

    /// Resolves the four per-slot GPU resources [`super::GpuFrameRenderer::dispatch_chunk`]
    /// needs for one chunk (bundled as [`ChunkSlotResources`]), asserting each is large
    /// enough for this dispatch. Split out of `dispatch_chunk` purely to keep that method
    /// under clippy's function-length limit.
    pub(super) fn resolve_chunk_slot_resources(
        &self,
        staging_slot: usize,
        tuples: usize,
        pixels_this_chunk: usize,
    ) -> ChunkSlotResources<'_> {
        let outputs = self.outputs[staging_slot]
            .as_ref()
            .expect("ensure_capacity just populated both slots");
        assert!(
            tuples <= outputs.capacity,
            "dispatch of {tuples} tuples exceeds output capacity {}",
            outputs.capacity
        );

        let pixel_output = self.pixel_outputs[staging_slot]
            .as_ref()
            .expect("ensure_pixel_capacity just populated both slots");
        assert!(
            pixels_this_chunk <= pixel_output.capacity,
            "reduced-pixel dispatch of {pixels_this_chunk} pixels exceeds pixel output \
             capacity {}",
            pixel_output.capacity
        );

        let staging_buffer = &self.staging[staging_slot]
            .as_ref()
            .expect("ensure_staging_capacity just populated both slots")
            .buffer;

        let params_buf = self.params_buffers[staging_slot]
            .as_ref()
            .expect("ensure_params_buffers just populated both slots");

        ChunkSlotResources {
            outputs,
            pixel_output,
            staging_buffer,
            params_buf,
        }
    }
}
