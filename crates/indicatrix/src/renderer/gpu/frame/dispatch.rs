//! Per-chunk GPU dispatch: [`encode_and_dispatch`] (the self-test blocking path shared
//! with `estimator_check::dispatch_transport`), [`GpuFrameRenderer::dispatch_chunk`]'s
//! pipelined production dispatch through either kernel shape
//! ([`GpuFrameRenderer::dispatch_chunk_wavefront`]/[`GpuFrameRenderer::run_wavefront_bounce_rounds`]
//! for [`super::GpuPipelineKind::Wavefront`]), and the pure chunk-sizing arithmetic
//! ([`chunk_pixels_for`]/[`GpuFrameRenderer::next_chunk_pixels`]/[`environment_params`]).

use std::time::Instant;

use crate::{
    optics::raytracer::{EnvironmentSource, illuminant_temperature_k},
    renderer::{
        buffers::{GpuTransportParams, GpuWavefrontParams, transport_env_mode},
        gpu::compute,
    },
};

use super::{
    ChunkFrameState, ChunkSlotResources, FIRST_DISPATCH_MAX_TUPLES, FLOATS_PER_TUPLE,
    GpuPipelineKind, MAX_WORKGROUPS_PER_DIMENSION, MIN_CHUNK_BYTES, PendingChunk, TARGET_CHUNK_MS,
    WORKGROUP_SIZE,
    bind_groups::{
        TransportDispatchArgs, WavefrontRoundBindGroups, build_bind_group, build_chunk_bind_group,
        build_wavefront_round_bind_groups, exclusive_prefix_sum, wavefront_generate_bindings,
    },
    readback::copy_to_existing_staging,
    renderer::GpuFrameRenderer,
    scene_buffers::{FrameSceneBuffers, TransportOutputs, WavefrontRayBuffers},
};

/// Uploads the scene inputs, binds all eight buffers, and dispatches `transport_main`
/// over `total_tuples` threads -- then BLOCKS until it finishes.
///
/// The single blocking dispatch routine for the megakernel: `estimator_check::dispatch_transport`
/// and every other self-test in `renderer::gpu` go through here, so the Tier 2/Tier 3
/// equivalence checks verify the exact binding code the renderer ships.
/// [`GpuFrameRenderer::dispatch_chunk`]'s pipelined production dispatch uses
/// [`build_chunk_bind_group`] instead -- see the parent module's doc comment's "Per-frame
/// uploads, persistent staging" section.
pub fn encode_and_dispatch(args: &TransportDispatchArgs<'_>, total_tuples: usize) {
    assert!(
        total_tuples <= args.outputs.capacity,
        "dispatch of {total_tuples} tuples exceeds output capacity {}",
        args.outputs.capacity
    );
    let bind_group = build_bind_group(args);
    let workgroups = (total_tuples as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &args.ctx.device,
        &args.ctx.queue,
        args.pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
}

/// Builds one chunk's `GpuTransportParams`, split out of
/// [`GpuFrameRenderer::dispatch_chunk`] purely to keep that method under clippy's
/// function-length limit. `params` is written into this chunk's slot's persistent
/// uniform buffer by the caller instead of being uploaded fresh here.
const fn build_chunk_params(
    state: &ChunkFrameState<'_>,
    first_pixel: usize,
    pixels_this_chunk: usize,
) -> GpuTransportParams {
    GpuTransportParams::new(
        pixels_this_chunk as u32,
        state.scene.max_bounces,
        state.sample_offset,
        state.env_mode,
        0.0,
        state.temp_k,
        state.spot_mult,
        state.exposure,
        state.light_yaw,
        state.light_pitch,
        state.white_balance.to_array(),
    )
    .with_pixel_offset(first_pixel as u32)
    .with_debug_buffers_disabled()
    .with_studio_use_d65(state.use_d65)
    .with_studio_model(state.studio_model)
    .with_backdrop(state.backdrop)
    .with_surface_glare(state.scene.environment.surface_glare())
}

impl GpuFrameRenderer {
    /// Builds and submits ONE chunk's transport dispatch, its `reduce_xyz_main`
    /// GPU-side sample-sum dispatch, and the non-blocking readback copy of the REDUCED
    /// per-pixel result, returning the resulting [`PendingChunk`] for a later
    /// [`super::GpuFrameRenderer::drain_pending_chunk`] to wait on.
    ///
    /// # Why a second dispatch
    ///
    /// `transport_main` writes one XYZ triple per (pixel, sample) THREAD into
    /// `outputs.xyz()` -- `pixels_this_chunk * spp` triples. Copying and mapping all of
    /// that back to sum it on the CPU would scale readback with sample
    /// count: at 1080p x 8 spp, ~200MB per progressive pass. `reduce_xyz_main` instead
    /// sums each pixel's `spp` consecutive triples ON the GPU, one thread per PIXEL, into
    /// `self.pixel_outputs[staging_slot]` -- `pixels_this_chunk` triples, independent of
    /// `spp`. Only THAT smaller buffer is copied to staging below; `outputs.xyz()` itself
    /// never leaves the GPU. See `reduce_xyz.wgsl`'s header comment for the determinism
    /// argument (fixed ascending summation order, matching the order the CPU sums them
    /// in).
    ///
    /// Factored out of [`super::GpuFrameRenderer::accumulate_via_pipeline`]'s loop body to
    /// keep that function under clippy's function-length limit; `state` bundles the
    /// per-frame-constant values the loop would otherwise close over directly (see
    /// [`ChunkFrameState`]).
    pub(super) fn dispatch_chunk(
        &self,
        state: &ChunkFrameState<'_>,
        frame_buffers: &FrameSceneBuffers,
        first_pixel: usize,
        pixels_this_chunk: usize,
        chunk_index: usize,
    ) -> PendingChunk {
        let tuples = pixels_this_chunk * state.spp as usize;
        let planes_len = state.scene.planes.len();

        // camera_params/material/planes/facet_finishes are NOT rebuilt or re-uploaded
        // here -- frame_buffers already holds them, persistent across every accumulate
        // call (see FrameSceneBuffers). params is written into this slot's persistent
        // uniform buffer below instead of uploaded fresh.
        let params = build_chunk_params(state, first_pixel, pixels_this_chunk);

        let staging_slot = chunk_index % 2;
        let ChunkSlotResources {
            outputs,
            pixel_output,
            staging_buffer,
            params_buf,
        } = self.resolve_chunk_slot_resources(staging_slot, tuples, pixels_this_chunk);

        // Written into this slot's PERSISTENT uniform buffer rather than uploaded fresh
        // -- see GpuFrameRenderer::params_buffers' doc comment for why reusing a slot
        // two chunks later is safe (queue-ordering: this write is submitted strictly
        // after the earlier chunk that read the same slot).
        self.ctx
            .queue
            .write_buffer(params_buf, 0, bytemuck::bytes_of(&params));

        let submitted_at = Instant::now();
        // Which kernel(s) populate `outputs.xyz()` for this chunk --
        // see `GpuPipelineKind`'s own doc comment. Either way, everything from here on
        // (`dispatch_reduce`, the readback copy, `PendingChunk`) is UNCHANGED: both
        // pipelines write into the exact same `outputs.xyz()` binding, at the exact
        // same per-(pixel,sample) index, so the shared tail below needs no branch of
        // its own.
        match self.pipeline_kind {
            GpuPipelineKind::Megakernel => {
                let pipeline = self.pipeline_for_class(state.pipeline_class);
                let bind_group = build_chunk_bind_group(
                    &self.ctx,
                    pipeline,
                    frame_buffers,
                    params_buf,
                    planes_len,
                    outputs,
                );
                let workgroups = (tuples as u32).div_ceil(WORKGROUP_SIZE as u32);
                // The compute dispatch's own submission index is unused: waiting for
                // the readback copy's index (submitted after this one, on the same
                // queue) is sufficient -- see `compute::finish_map_read`'s doc comment.
                let _ = compute::dispatch(
                    &self.ctx.device,
                    &self.ctx.queue,
                    pipeline,
                    &bind_group,
                    (workgroups, 1, 1),
                );
            }
            GpuPipelineKind::Wavefront => {
                self.dispatch_chunk_wavefront(
                    state,
                    frame_buffers,
                    params_buf,
                    planes_len,
                    outputs,
                    tuples,
                );
            }
        }

        // GPU-side sample reduction -- see this function's own doc comment ("Why a
        // second dispatch"). Reads outputs.xyz() (just written by the dispatch above;
        // safe without an explicit barrier because wgpu executes queue submissions in
        // order, and this dispatch is submitted strictly after it on the same queue --
        // the exact guarantee the readback copy right below already relies on) and
        // writes pixel_output.buffer. Split into its own method to keep this one under
        // clippy's function-length limit.
        self.dispatch_reduce(
            state.spp,
            staging_slot,
            pixels_this_chunk,
            outputs,
            pixel_output,
        );

        let copy_index = copy_to_existing_staging::<f32>(
            &self.ctx.device,
            &self.ctx.queue,
            &pixel_output.buffer,
            staging_buffer,
            pixels_this_chunk * 3,
        );
        let rx = compute::begin_map_read(staging_buffer);

        PendingChunk {
            staging_slot,
            copy_index,
            rx,
            first_pixel,
            pixels_this_chunk,
            submitted_at,
        }
    }

    /// Runs one chunk through the wavefront pipeline instead of the
    /// megakernel -- `wavefront_generate`, then `wavefront_bounce`/`wavefront_compact_*`
    /// once per bounce round until either every ray has died or
    /// `state.scene.max_bounces` rounds have run, then `wavefront_finalize_survivors`
    /// for whatever remains. Populates `outputs.xyz()` (and, when `params` requested
    /// them, the debug buffers) exactly as the megakernel would have for this same
    /// chunk -- see `dispatch_chunk`'s own comment at its call site.
    ///
    /// Allocates a fresh set of ray-state buffers EVERY call, sized exactly to `tuples`
    /// -- unlike `outputs`/`staging`/etc., which are grown-never-shrunk persistent
    /// buffers reused across chunks. A documented simplification for this first cut
    /// (see the parent module's doc comment's "Wavefront pipeline" section): the
    /// megakernel path's overlapped, persistent-buffer machinery is substantial
    /// (`FrameSceneBuffers`/`TransportOutputs`/`StagingSlot` growth policies,
    /// double-buffering), and mirroring all of it for a pipeline that defaults OFF
    /// until measured was judged not worth the risk here -- revisit if
    /// `GpuPipelineKind::Wavefront` sees real use and per-chunk allocation shows up in
    /// profiling.
    ///
    /// The compaction round-trip (`wavefront_compact_scan`'s block counts read back,
    /// CPU exclusive-prefix-summed, `compact_block_offset` re-uploaded) is a genuine
    /// synchronous stall per bounce round -- see `wavefront_compact_scan`'s own doc
    /// comment in `wavefront_transport.wgsl` for why an order-preserving compaction was
    /// chosen anyway. This is the other half of why this pipeline stays opt-in: it
    /// cannot currently participate in `dispatch_chunk`'s overlapped-chunk pipeline the
    /// way the megakernel path does.
    fn dispatch_chunk_wavefront(
        &self,
        state: &ChunkFrameState<'_>,
        frame_buffers: &FrameSceneBuffers,
        params_buf: &wgpu::Buffer,
        planes_len: usize,
        outputs: &TransportOutputs,
        tuples: usize,
    ) {
        let pipelines = self
            .wavefront_pipelines
            .as_ref()
            .expect("set_pipeline_kind(Wavefront) must be called before dispatching it");
        let device = &self.ctx.device;
        let queue = &self.ctx.queue;

        let rays = WavefrontRayBuffers::new(device, tuples);
        let wf_params_buf = compute::upload(
            device,
            "wavefront params",
            &[GpuWavefrontParams {
                chunk_rays: tuples as u32,
                active_count: tuples as u32,
                bounce: 0,
                workgroup_count: (tuples as u32).div_ceil(WORKGROUP_SIZE as u32),
            }],
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );

        // 1. wavefront_generate -- see `wavefront_transport.wgsl`'s header comment,
        // kernel sequence step 1. Needs only `camera`/`params` from the shared scene
        // bindings (camera-ray generation) plus its own ray-state/active-list buffers.
        let generate_bind_group = compute::bind_buffers(
            device,
            "wavefront generate bind group",
            &pipelines.generate,
            &wavefront_generate_bindings(&frame_buffers.camera, params_buf, &wf_params_buf, &rays),
        );
        let generate_workgroups = (tuples as u32).div_ceil(WORKGROUP_SIZE as u32);
        let _ = compute::dispatch(
            device,
            queue,
            &pipelines.generate,
            &generate_bind_group,
            (generate_workgroups, 1, 1),
        );

        let bind_groups = build_wavefront_round_bind_groups(
            device,
            pipelines,
            frame_buffers,
            params_buf,
            planes_len,
            outputs,
            &wf_params_buf,
            &rays,
        );

        // 2-4. Bounce rounds: dispatch, order-preserving compact, repeat -- see
        // `wavefront_transport.wgsl`'s header comment, kernel sequence steps 2-4.
        let (active_count, bounce) = self.run_wavefront_bounce_rounds(
            state.scene.max_bounces,
            tuples as u32,
            &rays,
            &wf_params_buf,
            &bind_groups,
        );

        // 5. wavefront_finalize_survivors -- see `wavefront_transport.wgsl`'s header
        // comment, kernel sequence step 5. Only reached when the bounce budget ran out
        // with rays still alive, not when every ray already died (those are finalized
        // inside `wavefront_bounce` itself, above).
        if active_count > 0 {
            queue.write_buffer(
                &wf_params_buf,
                0,
                bytemuck::bytes_of(&GpuWavefrontParams {
                    chunk_rays: tuples as u32,
                    active_count,
                    bounce,
                    workgroup_count: active_count.div_ceil(WORKGROUP_SIZE as u32),
                }),
            );
            let finalize_workgroups = active_count.div_ceil(WORKGROUP_SIZE as u32);
            let _ = compute::dispatch(
                device,
                queue,
                &pipelines.finalize_survivors,
                &bind_groups.finalize,
                (finalize_workgroups, 1, 1),
            );
        }
    }

    /// Runs [`WavefrontRayBuffers::active_in`](super::scene_buffers::WavefrontRayBuffers)'s
    /// bounce/compact loop -- steps 2-4 of `wavefront_transport.wgsl`'s kernel sequence --
    /// until either no ray remains alive or `max_bounces` rounds have run. Returns the
    /// final `(active_count, bounce)` for [`Self::dispatch_chunk_wavefront`]'s own step
    /// 5. Split out purely to keep that function under clippy's function-length limit.
    fn run_wavefront_bounce_rounds(
        &self,
        max_bounces: u32,
        chunk_rays: u32,
        rays: &WavefrontRayBuffers,
        wf_params_buf: &wgpu::Buffer,
        bind_groups: &WavefrontRoundBindGroups,
    ) -> (u32, u32) {
        let device = &self.ctx.device;
        let queue = &self.ctx.queue;
        let pipelines = self
            .wavefront_pipelines
            .as_ref()
            .expect("set_pipeline_kind(Wavefront) must be called before dispatching it");

        let mut active_count = chunk_rays;
        let mut bounce = 0u32;
        while active_count > 0 && bounce < max_bounces {
            let workgroup_count = active_count.div_ceil(WORKGROUP_SIZE as u32);
            queue.write_buffer(
                wf_params_buf,
                0,
                bytemuck::bytes_of(&GpuWavefrontParams {
                    chunk_rays,
                    active_count,
                    bounce,
                    workgroup_count,
                }),
            );

            compute::dispatch_and_wait(
                device,
                queue,
                &pipelines.bounce,
                &bind_groups.bounce,
                (workgroup_count, 1, 1),
            );
            compute::dispatch_and_wait(
                device,
                queue,
                &pipelines.compact_scan,
                &bind_groups.compact_scan,
                (workgroup_count, 1, 1),
            );

            // Deterministic CPU-side exclusive prefix sum over this round's per-workgroup
            // alive counts -- see `wavefront_compact_scan`'s own doc comment
            // (`wavefront_transport.wgsl`) for why this stays a synchronous host
            // round-trip rather than a GPU-side atomic counter.
            let block_counts: Vec<u32> = compute::readback(
                device,
                queue,
                &rays.block_alive_count,
                workgroup_count as usize,
            );
            let (block_offsets, new_active_count) = exclusive_prefix_sum(&block_counts);
            queue.write_buffer(&rays.block_offset, 0, bytemuck::cast_slice(&block_offsets));

            compute::dispatch_and_wait(
                device,
                queue,
                &pipelines.compact_scatter,
                &bind_groups.compact_scatter,
                (workgroup_count, 1, 1),
            );

            // `active_ray_indices_next` becomes the next round's `active_ray_indices` --
            // a plain buffer-to-buffer copy rather than swapping which buffer each bind
            // group references (bind groups are immutable once created in `wgpu`), so
            // every bind group built above stays valid for every round.
            if new_active_count > 0 {
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("wavefront active-list copy encoder"),
                });
                encoder.copy_buffer_to_buffer(
                    &rays.active_out,
                    0,
                    &rays.active_in,
                    0,
                    u64::from(new_active_count) * size_of::<u32>() as u64,
                );
                queue.submit(Some(encoder.finish()));
            }

            active_count = new_active_count;
            bounce += 1;
        }
        (active_count, bounce)
    }

    /// Picks how many pixels the NEXT chunk covers.
    ///
    /// `byte_budget_pixels` (from [`chunk_pixels_for`] against `self.chunk_budget_bytes`)
    /// is always the hard upper bound -- time-budgeted sizing only ever shrinks a chunk
    /// below it to approach [`TARGET_CHUNK_MS`], never raises it, so `chunk_budget_bytes`
    /// keeps meaning a byte-budget ceiling on one dispatch's output buffers.
    ///
    /// `ema_ns_per_tuple` being `None` (no chunk has drained yet) uses
    /// [`FIRST_DISPATCH_MAX_TUPLES`] instead of the full byte budget, so a cold
    /// integrated GPU's first dispatch(es) cannot alone trip Windows' TDR watchdog. Once
    /// a measurement exists, the target pixel count is clamped between
    /// [`MIN_CHUNK_BYTES`] worth of tuples (floor) and `byte_budget_pixels` (ceiling).
    pub(super) fn next_chunk_pixels(
        ema_ns_per_tuple: Option<f64>,
        byte_budget_pixels: usize,
        spp: u32,
        num_pixels: usize,
    ) -> usize {
        let spp_usize = (spp as usize).max(1);
        let Some(ns_per_tuple) = ema_ns_per_tuple.filter(|v| *v > 0.0) else {
            let first_dispatch_pixels = (FIRST_DISPATCH_MAX_TUPLES / spp_usize).max(1);
            return byte_budget_pixels
                .min(first_dispatch_pixels)
                .min(num_pixels)
                .max(1);
        };
        let target_tuples = (TARGET_CHUNK_MS * 1.0e6 / ns_per_tuple).max(1.0);
        let target_pixels = ((target_tuples / spp_usize as f64).floor() as usize).max(1);
        let min_pixels = chunk_pixels_for(MIN_CHUNK_BYTES, spp, num_pixels).min(byte_budget_pixels);
        target_pixels.clamp(min_pixels, byte_budget_pixels)
    }
}

/// How many pixels one dispatch covers, given a byte budget for its output buffers.
///
/// A pixel's `spp` samples always stay in one dispatch (see the parent module's doc
/// comment), so the budget is divided by `spp` first. Always at least 1 -- a single
/// pixel over budget is still dispatched rather than looping forever on a zero-width
/// chunk -- and never more than the frame has. Also never large enough to need more than
/// [`MAX_WORKGROUPS_PER_DIMENSION`] workgroups -- see that constant's doc comment.
pub fn chunk_pixels_for(budget_bytes: usize, spp: u32, num_pixels: usize) -> usize {
    let budget_tuples = budget_bytes / (FLOATS_PER_TUPLE * size_of::<f32>());
    let dispatch_limited_tuples = MAX_WORKGROUPS_PER_DIMENSION * WORKGROUP_SIZE;
    let tuples_per_chunk = budget_tuples.min(dispatch_limited_tuples);
    (tuples_per_chunk / spp as usize).max(1).min(num_pixels)
}

/// [`environment_params`]'s return value: `(env_mode, temp_k, spot_mult, exposure,
/// light_yaw, light_pitch, use_d65, studio_model, backdrop)`. `use_d65` mirrors
/// `optics::raytracer::environment::sample_studio_environment_with_rig`'s own
/// `preset.uses_d65()` check -- see
/// `renderer::buffers::GpuTransportParams::studio_use_d65`'s doc comment.
pub type EnvironmentParams = (u32, f32, f32, f32, f32, f32, bool, u32, f32);

/// Maps an [`EnvironmentSource`] onto the megakernel's `env_mode` and its studio-rig
/// parameters -- see [`EnvironmentParams`] for the returned tuple's field meanings.
///
/// `HdrMap`'s studio-rig fields (`temp_k`/`spot_mult`/`exposure`/`light_yaw`/`light_pitch`/
/// `use_d65`/`studio_model`/`backdrop`) are unused by the shader's `env_mode == transport_env_mode::HDR_MAP` branch
/// (see `sample_environment_with_rig` in `spectral_transport.wgsl`) -- zeroed here rather
/// than left to whatever a caller might otherwise pass, so a stray read of one of them
/// during future maintenance can't silently pick up a stale studio value.
///
/// Infallible: every [`EnvironmentSource`] variant has an
/// `env_mode` -- returns [`EnvironmentParams`] directly rather than wrapping it in a
/// `Result` that could never be `Err`, per clippy's `unnecessary_wraps`.
/// [`super::GpuFrameError::UnsupportedEnvironment`] is still enforced elsewhere (kept as
/// defensive future-proofing; see that variant's own doc comment), just not by this
/// function any more.
pub const fn environment_params(environment: EnvironmentSource<'_>) -> EnvironmentParams {
    match environment {
        EnvironmentSource::Studio {
            preset,
            exposure,
            light_yaw,
            light_pitch,
            backdrop,
            ..
        } => (
            transport_env_mode::STUDIO_RIG,
            illuminant_temperature_k(preset),
            preset.params().spot_mult,
            exposure,
            light_yaw,
            light_pitch,
            preset.uses_d65(),
            preset.model().gpu_id(),
            backdrop,
        ),
        EnvironmentSource::HdrMap(_) => (
            transport_env_mode::HDR_MAP,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            false,
            0,
            0.0,
        ),
    }
}
