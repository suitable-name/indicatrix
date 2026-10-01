//! wasm32-only async counterparts to [`GpuFrameRenderer::new`]/[`GpuFrameRenderer::accumulate`].
//!
//! A separate `impl` block, not `#[cfg]` branches inside the existing methods: a
//! browser's main thread must never block, so there is no OS thread for
//! `pollster::block_on` to park and no `Device::poll(Maintain::Wait)` to synchronously
//! drive a `Buffer::map_async` callback the way native's `compute::begin_map_read`/
//! `finish_map_read` do. Both methods below are genuinely `async fn`s that `.await`
//! `wgpu`'s own futures -- a different control-flow shape that would make non-wasm32
//! callers pay `async`/`.await` for no reason if forced into the same signatures.
//!
//! Also drops the double-buffered chunk overlap: `accumulate`'s "Overlapped chunk
//! pipeline" exists to keep the GPU busy while the CPU blocks mapping the PREVIOUS
//! chunk's readback. On wasm32 nothing blocks in the first place -- awaiting
//! [`map_read_async`] simply yields to the browser's microtask queue at zero CPU cost --
//! so there is no idle window to hide by queuing chunk i+1 early.
//! [`GpuFrameRenderer::accumulate_async`] dispatches, copies, and awaits each chunk in
//! turn; still chunked, just not pipelined two deep.

use glam::Vec3;
use wgpu::BufferUsages;

use crate::{
    optics::raytracer::{EnvironmentSource, environment::environment_white_balance},
    renderer::{
        buffers::{GpuCameraParams, GpuGemMaterial, GpuTransportParams, encode_facet_finishes},
        env_map_gpu::HdrEnvGpuData,
        gpu::{GpuContext, MEGAKERNEL_STORAGE_BUFFERS, compute},
    },
};

use super::{
    CHUNK_BUDGET_BYTES, GpuFrameError, GpuFrameScene, GpuPipelineKind, REDUCE_SHADER_SRC,
    WORKGROUP_SIZE,
    bind_groups::{TransportDispatchArgs, build_bind_group},
    classify_material,
    dispatch::{EnvironmentParams, chunk_pixels_for, environment_params},
    readback::GpuReduceParams,
    renderer::GpuFrameRenderer,
};

// clippy::future_not_send: every async fn below touches wgpu's web backend types, which
// hold browser-side Rc handles and are therefore !Send -- see
// GpuContext::acquire_async's identical allow. This impl block only exists on wasm32,
// where nothing is ever sent across threads, so the lint is meaningless here regardless.
#[allow(
    clippy::future_not_send,
    reason = "wasm32-unknown-unknown has no second thread to send a future to; see the \
              module-level comment above this impl block"
)]
impl GpuFrameRenderer {
    /// Async, wasm32-only counterpart to [`Self::new`]: acquires a GPU device and
    /// compiles `reduce_xyz_main` without blocking the browser's main thread; the
    /// `transport_main` kernels compile lazily per material class, as in [`Self::new`].
    ///
    /// Awaits [`GpuContext::acquire_async`] directly rather than going through
    /// [`GpuContext::acquire`]'s `pollster::block_on` wrapper -- see this `impl` block's
    /// doc comment. Pipeline compilation itself is synchronous on every target, so
    /// nothing else here needs `.await`.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::new`]'s -- most commonly, the browser has no WebGPU
    /// support or no compatible adapter (Safari without the WebGPU flag, or a machine
    /// whose driver stack the browser declines to expose).
    pub async fn new_async() -> Result<Self, GpuFrameError> {
        let ctx = GpuContext::acquire_async()
            .await
            .map_err(GpuFrameError::Acquire)?;
        let available = ctx.device.limits().max_storage_buffers_per_shader_stage;
        if available < MEGAKERNEL_STORAGE_BUFFERS {
            return Err(GpuFrameError::DeviceLimits {
                needed: MEGAKERNEL_STORAGE_BUFFERS,
                available,
            });
        }
        let info = ctx.adapter.get_info();
        let adapter_label = format!("{} ({:?})", info.name, info.backend);
        let pipeline_cache = compute::create_pipeline_cache(&ctx.device);
        let reduce_pipeline = compute::create_compute_pipeline_cached(
            &ctx.device,
            pipeline_cache.as_ref(),
            "reduce_xyz_main",
            REDUCE_SHADER_SRC,
            "reduce_xyz_main",
            &[],
        );
        Ok(Self {
            ctx,
            pipeline_generic: std::sync::OnceLock::new(),
            pipeline_cache,
            pipeline_isotropic: None,
            pipeline_uniaxial: None,
            pipeline_biaxial: None,
            reduce_pipeline,
            adapter_label,
            outputs: [None, None],
            pixel_outputs: [None, None],
            staging: [None, None],
            params_buffers: [None, None],
            reduce_params_buffers: [None, None],
            scene_buffers: None,
            xyz_scratch: Vec::new(),
            chunk_budget_bytes: CHUNK_BUDGET_BYTES,
            ns_per_tuple_ema: None,
            pipeline_kind: GpuPipelineKind::default(),
            wavefront_pipelines: None,
            poisoned: false,
            turn_in_progress: false,
        })
    }

    /// Async, wasm32-only counterpart to [`Self::accumulate`] -- same inputs, same
    /// summed-XYZ-into-`accum` contract, same decline conditions
    /// ([`GpuFrameError::UnsupportedMaterial`]/[`GpuFrameError::UnsupportedEnvironment`]),
    /// but chunk-at-a-time rather than two-deep pipelined -- see this `impl` block's own
    /// doc comment for why.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    pub async fn accumulate_async(
        &mut self,
        scene: &GpuFrameScene<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> Result<(), GpuFrameError> {
        let pipeline_class = classify_material(scene.material);
        let num_pixels = scene.width as usize * scene.height as usize;
        assert_eq!(
            accum.len(),
            num_pixels,
            "accumulation buffer must have one entry per pixel"
        );
        if spp == 0 || num_pixels == 0 {
            return Ok(());
        }

        // See Self::accumulate_via_pipeline's identical check: unreachable today, kept as
        // defensive future-proofing.
        if !scene.material.gpu_supported() {
            return Err(GpuFrameError::UnsupportedMaterial(
                scene.material.name.clone(),
            ));
        }

        let call = AsyncCallInputs::new(self, scene, pipeline_class, spp, sample_offset)?;

        self.ensure_specialized_pipeline(pipeline_class);

        let chunk_pixels = chunk_pixels_for(self.chunk_budget_bytes, spp, num_pixels);
        self.ensure_capacity(chunk_pixels * spp as usize);
        self.ensure_pixel_capacity(chunk_pixels);

        let mut first_pixel = 0usize;
        let mut chunk_index = 0usize;
        while first_pixel < num_pixels {
            let pixels_this_chunk = chunk_pixels.min(num_pixels - first_pixel);
            let staging =
                self.submit_chunk_async(&call, chunk_index, first_pixel, pixels_this_chunk);

            // Awaits the browser's own resolution of map_async -- see map_read_async's
            // doc comment for why this needs no Device::poll call, unlike native's
            // finish_map_read. Returns GpuFrameError::DeviceLost on a mapping failure,
            // propagated here with `?`.
            let xyz: Vec<f32> = map_read_async(&staging, pixels_this_chunk * 3).await?;

            // xyz already holds reduce_xyz_main's GPU-summed per-pixel triples -- no
            // CPU-side per-sample summation loop needed any more.
            for local_pixel in 0..pixels_this_chunk {
                let base = local_pixel * 3;
                accum[first_pixel + local_pixel] +=
                    Vec3::new(xyz[base], xyz[base + 1], xyz[base + 2]);
            }

            first_pixel += pixels_this_chunk;
            chunk_index += 1;
        }

        Ok(())
    }

    /// Records one chunk of [`Self::accumulate_async`]: the transport dispatch, the
    /// GPU-side per-pixel XYZ reduction (mirroring native's `dispatch_chunk` -- see that
    /// function's "Why a second dispatch" doc comment), and the copy of the reduced
    /// triples into a fresh staging buffer, which is returned for [`map_read_async`] to
    /// await. Nothing here blocks.
    ///
    /// Native's pipelined path builds its bind group via `build_chunk_bind_group`
    /// instead (see the parent module's doc comment's "Per-frame uploads, persistent
    /// staging" section), but this wasm32 path re-uploads every buffer every chunk via
    /// `build_bind_group`, and re-creates the tiny reduce-params buffer fresh every
    /// chunk: it never had the overlapped double-buffering the native path uses, so
    /// there is no per-frame state to hoist these uploads out of here.
    fn submit_chunk_async(
        &self,
        call: &AsyncCallInputs<'_, '_>,
        chunk_index: usize,
        first_pixel: usize,
        pixels_this_chunk: usize,
    ) -> wgpu::Buffer {
        let spp = call.spp;
        let tuples = pixels_this_chunk * spp as usize;
        let (camera_params, params) = call.chunk_params(first_pixel, pixels_this_chunk);

        let outputs = self.outputs[chunk_index % 2]
            .as_ref()
            .expect("ensure_capacity just populated both slots");

        let bind_args = TransportDispatchArgs {
            ctx: &self.ctx,
            pipeline: self.pipeline_for_class(call.pipeline_class),
            camera_params: &camera_params,
            params: &params,
            material: &call.gpu_material,
            planes: call.scene.planes,
            facet_finishes: &call.gpu_finishes,
            outputs,
            hdr_env: &call.hdr_env,
        };

        let bind_group = build_bind_group(&bind_args);
        let workgroups = (tuples as u32).div_ceil(WORKGROUP_SIZE as u32);
        let _ = compute::dispatch(
            &self.ctx.device,
            &self.ctx.queue,
            bind_args.pipeline,
            &bind_group,
            (workgroups, 1, 1),
        );

        let pixel_output = self.pixel_outputs[chunk_index % 2]
            .as_ref()
            .expect("ensure_pixel_capacity just populated both slots");
        let reduce_params = GpuReduceParams {
            num_pixels: pixels_this_chunk as u32,
            num_samples: spp,
            _pad0: 0,
            _pad1: 0,
        };
        let reduce_params_buf = compute::upload(
            &self.ctx.device,
            "reduce xyz params (wasm32 async)",
            std::slice::from_ref(&reduce_params),
            BufferUsages::UNIFORM,
        );
        let reduce_bind_group = compute::bind_buffers(
            &self.ctx.device,
            "reduce xyz bind group (wasm32 async)",
            &self.reduce_pipeline,
            &[
                (0, &reduce_params_buf),
                (1, outputs.xyz()),
                (2, &pixel_output.buffer),
            ],
        );
        let reduce_workgroups = (pixels_this_chunk as u32).div_ceil(WORKGROUP_SIZE as u32);
        let _ = compute::dispatch(
            &self.ctx.device,
            &self.ctx.queue,
            &self.reduce_pipeline,
            &reduce_bind_group,
            (reduce_workgroups, 1, 1),
        );

        let (staging, _copy_index) = compute::copy_to_staging::<f32>(
            &self.ctx.device,
            &self.ctx.queue,
            &pixel_output.buffer,
            pixels_this_chunk * 3,
            "transport out pixel xyz staging (wasm32 async)",
        );
        staging
    }
}

/// Everything one [`GpuFrameRenderer::accumulate_async`] call encodes ONCE and every
/// chunk of that call then reads: the environment/white-balance parameters, the encoded
/// material and facet finishes, and the HDR environment upload. Built once per call
/// rather than once per chunk -- this path never had `FrameSceneBuffers`'s cross-call
/// persistence (see this module's own doc comment), so there is no identity cache to
/// consult, only "per call, not per chunk".
struct AsyncCallInputs<'s, 'a> {
    scene: &'a GpuFrameScene<'s>,
    pipeline_class: u32,
    spp: u32,
    sample_offset: u32,
    env: EnvironmentParams,
    white_balance: Vec3,
    gpu_material: GpuGemMaterial,
    gpu_finishes: Vec<u32>,
    hdr_env: HdrEnvGpuData,
}

impl<'s, 'a> AsyncCallInputs<'s, 'a> {
    /// Encodes the per-call inputs. Declines
    /// [`GpuFrameError::UnsupportedEnvironment`] for an oversized HDR map -- mirrors
    /// `build_hdr_env`'s identical check (this path has no `FrameSceneBuffers` to share
    /// that helper through, so the check is repeated here directly).
    fn new(
        renderer: &GpuFrameRenderer,
        scene: &'a GpuFrameScene<'s>,
        pipeline_class: u32,
        spp: u32,
        sample_offset: u32,
    ) -> Result<Self, GpuFrameError> {
        let hdr_env = match scene.environment {
            EnvironmentSource::HdrMap(map) => {
                if !HdrEnvGpuData::fits_storage_binding(&renderer.ctx.device, map) {
                    return Err(GpuFrameError::UnsupportedEnvironment);
                }
                HdrEnvGpuData::upload(&renderer.ctx.device, map)
            }
            EnvironmentSource::Studio { .. } => HdrEnvGpuData::dummy(&renderer.ctx.device),
        };
        Ok(Self {
            scene,
            pipeline_class,
            spp,
            sample_offset,
            env: environment_params(scene.environment),
            // See `GpuFrameRenderer::prepare_turn`.
            white_balance: environment_white_balance(scene.environment),
            gpu_material: GpuGemMaterial::encode(scene.material),
            gpu_finishes: encode_facet_finishes(scene.facet_finishes, scene.planes.len()),
            hdr_env,
        })
    }

    /// The camera and transport uniforms for the chunk starting at `first_pixel`.
    const fn chunk_params(
        &self,
        first_pixel: usize,
        pixels_this_chunk: usize,
    ) -> (GpuCameraParams, GpuTransportParams) {
        let scene = self.scene;
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
        ) = self.env;
        let camera_params = GpuCameraParams {
            origin: scene.camera.origin.to_array(),
            fov_tan: scene.camera.fov_tan,
            forward: scene.camera.forward.to_array(),
            width: scene.width as f32,
            right: scene.camera.right.to_array(),
            height: scene.height as f32,
            up: scene.camera.up.to_array(),
            num_samples: self.spp,
        };
        let params = GpuTransportParams::new(
            pixels_this_chunk as u32,
            scene.max_bounces,
            self.sample_offset,
            env_mode,
            0.0,
            temp_k,
            spot_mult,
            exposure,
            light_yaw,
            light_pitch,
            self.white_balance.to_array(),
        )
        .with_pixel_offset(first_pixel as u32)
        .with_debug_buffers_disabled()
        .with_studio_use_d65(use_d65)
        .with_studio_model(studio_model)
        .with_backdrop(backdrop);
        (camera_params, params)
    }
}

/// Awaits `staging`'s `Buffer::map_async` callback and reads back `count` `T`s, wasm32
/// only.
///
/// Native's `compute::finish_map_read` drives the identical callback to completion via
/// `Device::poll(Maintain::Wait)`, blocking the calling OS thread. `wgpu`'s WebGPU
/// backend needs no such call: a browser resolves `GPUBuffer.mapAsync`'s promise on its
/// own microtask queue as soon as ready -- the callback fires whenever this function
/// `.await`s the channel below, which is why this can be a genuine `async fn` instead of
/// needing a `Device::poll` equivalent.
///
/// # Errors
///
/// Returns [`GpuFrameError::DeviceLost`] on a dropped callback, a failed mapping, or an
/// unreadable mapped range, for [`GpuFrameRenderer::accumulate_async`] to propagate with
/// `?`. There is no bounded-wait timeout here the way `compute::GPU_WAIT_TIMEOUT` bounds
/// the native path: nothing here blocks an OS thread, so there is no thread to free by
/// timing out; a browser tab that never resolves `mapAsync` leaves this `Future` pending.
#[allow(
    clippy::future_not_send,
    reason = "wgpu's web backend's buffer-mapping state is inherently !Send (browser-side \
              Rc handles); wasm32-unknown-unknown has no second thread to send this future \
              to regardless -- see GpuContext::acquire_async's identical allow"
)]
async fn map_read_async<T: bytemuck::Pod>(
    staging: &wgpu::Buffer,
    count: usize,
) -> Result<Vec<T>, GpuFrameError> {
    let (tx, rx) = futures_channel::oneshot::channel();
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            // The receiver can only be dropped by this function returning early, which it
            // never does before this send -- a failed send would mean the callback
            // outlived its own awaiting task, not a race this single-threaded target can
            // produce.
            let _ = tx.send(result);
        });
    rx.await
        .map_err(|_| {
            GpuFrameError::DeviceLost(
                "wgpu buffer-mapping callback was dropped before it fired".to_string(),
            )
        })?
        .map_err(|e| GpuFrameError::DeviceLost(format!("wgpu buffer mapping failed: {e}")))?;
    // Mirrors compute::finish_map_read: staging was sized by copy_to_staging::<T> for
    // precisely `count` T's, so the mapped range's length already equals `count`; `count`
    // is taken as a parameter so the assertion below can catch a size mismatch.
    let out = {
        let data = staging.slice(..).get_mapped_range().map_err(|e| {
            GpuFrameError::DeviceLost(format!("mapped buffer range could not be read: {e}"))
        })?;
        bytemuck::cast_slice::<u8, T>(&data).to_vec()
    };
    debug_assert_eq!(
        out.len(),
        count,
        "staging buffer length must match copy_to_staging's count"
    );
    staging.unmap();
    Ok(out)
}
