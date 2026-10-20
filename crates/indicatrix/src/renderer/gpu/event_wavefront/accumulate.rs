//! Accumulation loop and dynamic chunk sizing for [`EventWavefrontRenderer`].
//!
//! Maintains the sum contract of `GpuFrameRenderer::accumulate`: adds the chunk's
//! accumulated XYZ values directly into the caller's `accum` slice (`accum[px] +=`).

use std::sync::atomic::Ordering;

use glam::Vec3;

use crate::{
    geometry::{
        GpuFacetPlane,
        tool::{GPU_MAX_TOOL_PRIMITIVES, ToolPrimitive, pack_planes_and_tools},
    },
    optics::raytracer::{environment::environment_white_balance, head_shadow_cosines},
    render_setup::hash_planes,
    renderer::{
        buffers::{
            EwHit, EwParams, GpuCameraParams, GpuGemMaterial, GpuTransportParams,
            encode_facet_finishes,
        },
        frame_scene::FrameScene,
        gpu::{
            GpuContext,
            event_wavefront::{
                EwError,
                chunk::{
                    ChunkRayBuffers, SceneBuffers, bind_kernel, execute_chunk, scene_binding_bytes,
                },
                clock,
                kernels::EwKernel,
                pipelines::EwPipelines,
            },
            frame::{classify_material, environment_params},
        },
    },
};

#[inline]
fn count_u32(count: usize) -> u32 {
    u32::try_from(count).expect("a chunk's pixel and tuple counts fit u32")
}

/// Target wall-clock duration for one dispatch-to-drain cycle, in milliseconds.
#[cfg(target_arch = "wasm32")]
pub const TARGET_CHUNK_MS: f64 = 30.0;
#[cfg(not(target_arch = "wasm32"))]
pub const TARGET_CHUNK_MS: f64 = 150.0;

/// Default output-buffer budget for a single dispatch, in bytes.
pub const CHUNK_BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// Hard floor on a time-budgeted chunk: 64 KiB worth of tuples.
pub const MIN_CHUNK_BYTES: usize = 64 * 1024;

/// Tuples the very first dispatch uses before an EMA measurement exists.
pub const FIRST_DISPATCH_MAX_TUPLES: usize = 1_000_000;

/// Smoothing factor for exponential moving average of chunk duration.
pub const CHUNK_TIMING_EMA_ALPHA: f64 = 0.3;

/// Memory footprint per ray across core, chan, hit, and `out_xyz` buffers (§4.4: 668 bytes).
pub const BYTES_PER_RAY: usize = 128 + 288 + size_of::<EwHit>() + 12;

/// Maximum workgroups per compute dimension supported on WebGPU baseline.
pub const MAX_WORKGROUPS_PER_DIMENSION: usize = 65535;

/// Workgroup thread size.
pub const WORKGROUP_SIZE: usize = 64;

/// Computes the pixel count for a single chunk within a given byte budget and device limits.
#[must_use]
pub fn chunk_pixels_for(
    budget_bytes: usize,
    spp: u32,
    num_pixels: usize,
    max_storage_binding: u64,
) -> usize {
    let spp_usize = (spp as usize).max(1);
    let max_rays_binding = (max_storage_binding / 288) as usize;
    let max_rays_budget = budget_bytes / BYTES_PER_RAY;
    let max_workgroup_rays = MAX_WORKGROUPS_PER_DIMENSION * WORKGROUP_SIZE;
    let tuples_cap = max_rays_binding
        .min(max_rays_budget)
        .min(max_workgroup_rays);
    (tuples_cap / spp_usize).max(1).min(num_pixels)
}

/// Sizing for the next chunk from EMA timing and budget constraints.
#[must_use]
pub fn next_chunk_pixels(
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
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "target_tuples >= 1.0; truncated to non-negative usize"
    )]
    let target_pixels = ((target_tuples / spp_usize as f64).floor() as usize).max(1);
    let min_pixels = 1.min(byte_budget_pixels).min(num_pixels);
    target_pixels
        .clamp(min_pixels, byte_budget_pixels)
        .min(num_pixels)
}

/// Transport parameters of the chunk of `pixels_this_chunk` pixels starting at `pixel_offset`,
/// for a stone with `tool_count` concave-tool records packed after its planes (`0`: planar).
fn chunk_transport_params(
    scene: &FrameScene<'_>,
    tool_count: u32,
    sample_offset: u32,
    pixels_this_chunk: usize,
    pixel_offset: usize,
) -> GpuTransportParams {
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
    let white_balance = environment_white_balance(scene.environment);
    GpuTransportParams::new(
        count_u32(pixels_this_chunk),
        scene.max_bounces,
        sample_offset,
        env_mode,
        0.0,
        temp_k,
        spot_mult,
        exposure,
        light_yaw,
        light_pitch,
        white_balance.to_array(),
    )
    .with_pixel_offset(count_u32(pixel_offset))
    .with_debug_buffers_disabled()
    .with_studio_use_d65(use_d65)
    .with_studio_model(studio_model)
    .with_tent_of(scene.environment)
    .with_backdrop(backdrop)
    .with_surface_glare(scene.environment.surface_glare())
    .with_head_shadow(head_shadow_cosines(scene.environment.head_shadow_deg()))
    .with_tool_count(tool_count)
}

/// The tool count of a stone as the shader's `num_tools`.
fn tool_count_u32(tools: &[ToolPrimitive]) -> u32 {
    u32::try_from(tools.len()).expect("a stone's tool count is bounded by GPU_MAX_TOOL_PRIMITIVES")
}

/// Refuses a stone with more tools than the kernels trace (the same limit the megakernel route
/// declines at, `renderer::gpu_backend::scene_routes_to_gpu_with_tools`).
///
/// # Errors
///
/// [`EwError::TooManyTools`] beyond [`GPU_MAX_TOOL_PRIMITIVES`].
pub(super) const fn check_tool_limit(tools: &[ToolPrimitive]) -> Result<(), EwError> {
    if tools.len() > GPU_MAX_TOOL_PRIMITIVES {
        return Err(EwError::TooManyTools {
            tools: tools.len(),
            limit: GPU_MAX_TOOL_PRIMITIVES,
        });
    }
    Ok(())
}

/// Native and WebGPU event wavefront gemstone renderer.
pub struct EventWavefrontRenderer {
    ctx: GpuContext,
    pipelines: EwPipelines,
    scene_buffers: Option<SceneBuffers>,
    chunk_buffers: Option<ChunkRayBuffers>,
    ns_per_tuple_ema: Option<f64>,
    last_scene_key: Option<(u32, u32, u64, u32, bool)>,
    chunk_budget_bytes: usize,
}

impl EventWavefrontRenderer {
    /// Constructs a new [`EventWavefrontRenderer`] with the given [`GpuContext`].
    #[must_use]
    pub fn new(ctx: GpuContext) -> Self {
        let pipelines = EwPipelines::new(&ctx.device);
        Self {
            ctx,
            pipelines,
            scene_buffers: None,
            chunk_buffers: None,
            ns_per_tuple_ema: None,
            last_scene_key: None,
            chunk_budget_bytes: CHUNK_BUDGET_BYTES,
        }
    }

    /// Overrides the per-dispatch output-buffer budget (default [`CHUNK_BUDGET_BYTES`]).
    pub const fn set_chunk_budget_bytes(&mut self, bytes: usize) {
        self.chunk_budget_bytes = bytes;
    }

    /// Returns the current chunk budget in bytes.
    #[must_use]
    pub const fn chunk_budget_bytes(&self) -> usize {
        self.chunk_budget_bytes
    }

    /// Accessor to the underlying [`GpuContext`].
    #[must_use]
    pub const fn context(&self) -> &GpuContext {
        &self.ctx
    }

    /// Running exponential moving average of nanoseconds per ray tuple.
    #[must_use]
    pub const fn ns_per_tuple_ema(&self) -> Option<f64> {
        self.ns_per_tuple_ema
    }

    /// Returns an iterator over all compiled pipeline variants `(kernel, material_class)`.
    pub fn compiled_pipelines(&self) -> impl Iterator<Item = (EwKernel, u32)> + '_ {
        self.pipelines.compiled()
    }

    /// Compiles or retrieves the compute pipeline for `(kernel, material_class)`.
    pub fn ensure_pipeline(&mut self, kernel: EwKernel, class: u32) -> &wgpu::ComputePipeline {
        self.pipelines.ensure(&self.ctx.device, kernel, class)
    }

    /// Encodes and submits a test dispatch of 1 workgroup with `chunk_rays = 0`.
    ///
    /// This triggers GPU driver shader compilation and pipeline validation without
    /// reading or writing any ray data.
    ///
    /// Returns `Ok(())` right after `submit`, without waiting for the GPU: a shader or
    /// pipeline failure surfaces only through wgpu error scopes (or the uncaptured-error
    /// flag), so callers must wrap this call in `push_error_scope`/`pop_error_scope` as
    /// the web worker's staged `GpuCompile` does.
    ///
    /// # Errors
    ///
    /// Returns an [`EwError`] when the dummy scene buffers cannot be created.
    ///
    /// # Panics
    ///
    /// Never in practice: the scene and chunk buffers it expects are created just above.
    pub fn test_dispatch_kernel(&mut self, kernel: EwKernel, class: u32) -> Result<(), EwError> {
        let dummy_camera: GpuCameraParams = bytemuck::Zeroable::zeroed();
        let dummy_params: GpuTransportParams = bytemuck::Zeroable::zeroed();
        let dummy_material: GpuGemMaterial = bytemuck::Zeroable::zeroed();
        let dummy_planes: [GpuFacetPlane; 1] = [bytemuck::Zeroable::zeroed()];
        let dummy_finishes = [0u32];

        if self.scene_buffers.is_none() {
            self.scene_buffers = Some(SceneBuffers::new(
                &self.ctx.device,
                &dummy_camera,
                &dummy_params,
                &dummy_material,
                &dummy_planes,
                &dummy_finishes,
                crate::optics::raytracer::LightingPreset::LightTent.studio(1.0, 0.0, 0.0),
            )?);
        }

        if self.chunk_buffers.is_none() {
            self.chunk_buffers = Some(ChunkRayBuffers::new(&self.ctx.device, 1, 64));
        }

        let chunk_buffers = self
            .chunk_buffers
            .as_mut()
            .expect("chunk buffers initialized");
        let ew_params = EwParams {
            chunk_rays: 0,
            bounce: 0,
            finalize_all: 1,
            has_scatter: 0,
            pad0: 0,
            pad1: 0,
            pad2: 0,
            pad3: 0,
        };
        self.ctx
            .queue
            .write_buffer(&chunk_buffers.ew_params, 0, bytemuck::bytes_of(&ew_params));

        self.pipelines.ensure(&self.ctx.device, kernel, class);
        let pipeline = self.pipelines.get(kernel, class);

        let scene_buffers = self
            .scene_buffers
            .as_ref()
            .expect("scene buffers initialized");
        let (planes_bytes, finishes_bytes) = scene_binding_bytes(dummy_planes.len());

        let bg = bind_kernel(
            &self.ctx.device,
            pipeline,
            kernel,
            scene_buffers,
            chunk_buffers,
            planes_bytes,
            finishes_bytes,
        );

        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ew test dispatch"),
            });
        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ew test compute pass"),
                timestamp_writes: None,
            });
            compute_pass.set_pipeline(pipeline);
            compute_pass.set_bind_group(0, &bg, &[]);
            compute_pass.dispatch_workgroups(1, 1, 1);
        }
        self.ctx.queue.submit(Some(encoder.finish()));
        Ok(())
    }

    /// Accumulates `spp` samples of `scene` into `accum` using the material's classified pipeline.
    ///
    /// Sums directly into `accum[pixel_index] += Vec3::new(X, Y, Z)` per the sum contract.
    ///
    /// # Errors
    ///
    /// See [`Self::accumulate_via_class`].
    #[allow(
        clippy::future_not_send,
        reason = "wgpu web backend buffer mapping futures are !Send; wasm32 is single-threaded"
    )]
    pub async fn accumulate(
        &mut self,
        scene: &FrameScene<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> Result<(), EwError> {
        self.accumulate_with_tools(scene, &[], sample_offset, spp, accum)
            .await
    }

    /// [`Self::accumulate`] for a concave stone: the stone is `scene.planes` minus `tools`.
    ///
    /// The tool records ride in the `planes` storage binding after the planes (see
    /// `geometry::tool::pack_planes_and_tools`; the count goes to the shader in
    /// `GpuTransportParams::num_tools`), so no kernel binds anything it did not before and the
    /// storage-buffer budget is unchanged. `scene.facet_finishes` is indexed by facet id, tool
    /// `k` being `scene.planes.len() + k`. Empty `tools` is exactly [`Self::accumulate`]: the
    /// planar upload is byte-identical.
    ///
    /// # Errors
    ///
    /// [`EwError::TooManyTools`] beyond `GPU_MAX_TOOL_PRIMITIVES` tools (route first with the
    /// same limit); otherwise see [`Self::accumulate_via_class`].
    #[allow(
        clippy::future_not_send,
        reason = "wgpu web backend buffer mapping futures are !Send; wasm32 is single-threaded"
    )]
    pub async fn accumulate_with_tools(
        &mut self,
        scene: &FrameScene<'_>,
        tools: &[ToolPrimitive],
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> Result<(), EwError> {
        let class = classify_material(scene.material);
        self.accumulate_via_class_with_tools(scene, tools, class, sample_offset, spp, accum)
            .await
    }

    /// Accumulates `spp` samples of `scene` into `accum` forcing a specific `material_class`.
    ///
    /// # Errors
    ///
    /// [`EwError::InvalidBufferSize`] if `accum` does not hold one entry per pixel,
    /// [`EwError::UnsupportedMaterial`] if the material has no GPU support, or any GPU,
    /// readback or device-loss error raised while running a chunk.
    #[allow(
        clippy::future_not_send,
        reason = "wgpu web backend buffer mapping futures are !Send; wasm32 is single-threaded"
    )]
    pub async fn accumulate_via_class(
        &mut self,
        scene: &FrameScene<'_>,
        material_class: u32,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> Result<(), EwError> {
        self.accumulate_via_class_with_tools(scene, &[], material_class, sample_offset, spp, accum)
            .await
    }

    /// [`Self::accumulate_via_class`] for a concave stone; see [`Self::accumulate_with_tools`].
    ///
    /// # Errors
    ///
    /// As [`Self::accumulate_via_class`], and [`EwError::TooManyTools`] beyond
    /// `GPU_MAX_TOOL_PRIMITIVES` tools.
    #[allow(
        clippy::future_not_send,
        reason = "wgpu web backend buffer mapping futures are !Send; wasm32 is single-threaded"
    )]
    pub async fn accumulate_via_class_with_tools(
        &mut self,
        scene: &FrameScene<'_>,
        tools: &[ToolPrimitive],
        material_class: u32,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> Result<(), EwError> {
        let num_pixels = (scene.width * scene.height) as usize;
        if accum.len() != num_pixels {
            return Err(EwError::InvalidBufferSize {
                expected: num_pixels,
                actual: accum.len(),
            });
        }
        if spp == 0 || num_pixels == 0 {
            return Ok(());
        }

        if !scene.material.gpu_supported() {
            return Err(EwError::UnsupportedMaterial(scene.material.name.clone()));
        }
        check_tool_limit(tools)?;

        // The planes binding as the shader sees it: the planes, then the packed tool records
        // (borrowed unchanged for a convex stone). Hashing it keys the EMA on the tools too.
        let packed = pack_planes_and_tools(scene.planes, tools);

        // The EMA is a per-scene cost estimate; a different design, resolution, material
        // class or scattering switch makes the old one size the first chunk wrongly.
        let scene_key = (
            scene.width,
            scene.height,
            hash_planes(&packed),
            material_class,
            scene.material.scattering_sigma_s > 0.0,
        );
        if self.last_scene_key != Some(scene_key) {
            self.ns_per_tuple_ema = None;
            self.last_scene_key = Some(scene_key);
        }

        // Drop an error flag left over from earlier, unrelated work (compile probes,
        // a previous failed call) so it is not blamed on this pass.
        self.ctx
            .validation_error_seen
            .store(false, Ordering::Relaxed);
        if let Ok(mut last) = self.ctx.last_uncaptured_error.lock() {
            *last = None;
        }

        let result = self
            .accumulate_prepared(
                scene,
                &packed,
                tool_count_u32(tools),
                material_class,
                sample_offset,
                spp,
                accum,
            )
            .await;
        if result.is_err() {
            // A failed or timed-out chunk can leave the staging buffer mapped-pending;
            // rebuild the per-chunk buffers on the next call.
            self.chunk_buffers = None;
        }
        result
    }

    /// Turns an uncaptured wgpu validation/internal error seen since the last check into
    /// [`EwError::DeviceLost`], so a bad bind group or oversize buffer can never yield a
    /// silent black or stale frame (mirrors `GpuFrameRenderer::check_uncaptured_error`).
    pub(super) fn check_uncaptured_error(&self) -> Result<(), EwError> {
        if !self
            .ctx
            .validation_error_seen
            .swap(false, Ordering::Relaxed)
        {
            return Ok(());
        }
        let detail = self
            .ctx
            .last_uncaptured_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        Err(EwError::DeviceLost(format!(
            "wgpu reported an uncaptured validation/device error during this pass: {}",
            detail.unwrap_or_else(|| "(no error text was captured)".to_string())
        )))
    }

    /// Encodes the camera, material, planes and environment of `scene` into the scene
    /// buffers, creating them on first use. `packed` is the planes binding: `scene.planes`
    /// followed by the packed tool records (`pack_planes_and_tools`).
    fn upload_scene(
        &mut self,
        scene: &FrameScene<'_>,
        packed: &[GpuFacetPlane],
        spp: u32,
    ) -> Result<(), EwError> {
        let camera_params = GpuCameraParams {
            origin: scene.camera.origin.to_array(),
            fov_tan: scene.camera.fov_tan,
            forward: scene.camera.forward.to_array(),
            width: scene.width as f32,
            right: scene.camera.right.to_array(),
            height: scene.height as f32,
            up: scene.camera.up.to_array(),
            num_samples: spp,
        };
        let material_params = GpuGemMaterial::encode(scene.material);
        // One finish entry per packed `planes` entry, not per facet: the two bindings are sized
        // alike (`scene_binding_bytes`), and a facet id (planes, then tools) only ever indexes
        // the first `planes + tools` of them -- the rest are never read.
        let facet_finishes = encode_facet_finishes(scene.facet_finishes, packed.len());
        let dummy_transport_params =
            GpuTransportParams::new(0, 0, 0, 0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, [0.0; 3]);

        if let Some(ref mut sb) = self.scene_buffers {
            sb.update(
                &self.ctx.device,
                &self.ctx.queue,
                &camera_params,
                &dummy_transport_params,
                &material_params,
                packed,
                &facet_finishes,
                scene.environment,
            )?;
        } else {
            self.scene_buffers = Some(SceneBuffers::new(
                &self.ctx.device,
                &camera_params,
                &dummy_transport_params,
                &material_params,
                packed,
                &facet_finishes,
                scene.environment,
            )?);
        }
        Ok(())
    }

    /// Makes sure the per-chunk ray buffers can hold `pixels` pixels and `rays` ray tuples.
    fn ensure_chunk_buffers(&mut self, pixels: usize, rays: usize) {
        if let Some(ref mut cb) = self.chunk_buffers {
            cb.ensure_capacity(&self.ctx.device, pixels, rays);
        } else {
            self.chunk_buffers = Some(ChunkRayBuffers::new(&self.ctx.device, pixels, rays));
        }
    }

    #[allow(
        clippy::future_not_send,
        reason = "wgpu web backend buffer mapping futures are !Send; wasm32 is single-threaded"
    )]
    #[expect(
        clippy::too_many_arguments,
        reason = "the packed planes binding and its tool count ride beside the scene so the \
                  planar callers keep the scene struct they have"
    )]
    async fn accumulate_prepared(
        &mut self,
        scene: &FrameScene<'_>,
        packed: &[GpuFacetPlane],
        tool_count: u32,
        material_class: u32,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> Result<(), EwError> {
        let num_pixels = accum.len();
        self.upload_scene(scene, packed, spp)?;

        let max_storage_binding = self.ctx.device.limits().max_storage_buffer_binding_size;
        let byte_budget_pixels = chunk_pixels_for(
            self.chunk_budget_bytes,
            spp,
            num_pixels,
            max_storage_binding,
        );

        let mut pixel_offset = 0usize;
        while pixel_offset < num_pixels {
            let pixels_remaining = num_pixels - pixel_offset;
            let pixels_this_chunk = next_chunk_pixels(
                self.ns_per_tuple_ema,
                byte_budget_pixels,
                spp,
                pixels_remaining,
            )
            .min(pixels_remaining);
            let chunk_rays = pixels_this_chunk * spp as usize;

            self.ensure_chunk_buffers(pixels_this_chunk, chunk_rays);

            let chunk_params = chunk_transport_params(
                scene,
                tool_count,
                sample_offset,
                pixels_this_chunk,
                pixel_offset,
            );

            clock::set_readback_deadline_ms(clock::chunk_deadline_ms(
                self.ns_per_tuple_ema,
                chunk_rays,
            ));
            let start = clock::Instant::now();

            let result = {
                let scene_buffers = self
                    .scene_buffers
                    .as_ref()
                    .expect("scene buffers initialized");
                let chunk_buffers = self
                    .chunk_buffers
                    .as_ref()
                    .expect("chunk buffers initialized");

                execute_chunk(
                    &self.ctx.device,
                    &self.ctx.queue,
                    &mut self.pipelines,
                    scene_buffers,
                    chunk_buffers,
                    scene,
                    material_class,
                    pixel_offset,
                    pixels_this_chunk,
                    spp,
                    &chunk_params,
                )
                .await?
            };

            // A validation error never fails the dispatch call itself: the pass is skipped
            // and the readback returns zeros or the previous chunk. Never accept that.
            self.check_uncaptured_error()?;

            let ns_per_tuple = start.elapsed().as_nanos() as f64 / chunk_rays as f64;
            self.ns_per_tuple_ema = Some(clock::blend_ema(self.ns_per_tuple_ema, ns_per_tuple));

            for (i, triplet) in result.as_chunks::<3>().0.iter().enumerate() {
                accum[pixel_offset + i] += Vec3::new(triplet[0], triplet[1], triplet[2]);
            }

            pixel_offset += pixels_this_chunk;
        }

        Ok(())
    }
}
