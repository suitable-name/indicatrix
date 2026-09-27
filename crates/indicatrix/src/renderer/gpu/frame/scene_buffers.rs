//! The persistent scene/output GPU buffers [`super::GpuFrameRenderer`] holds across
//! calls: [`TransportOutputs`] (the megakernel's four output buffers),
//! [`FrameSceneBuffers`] (the four persistent scene-input buffers plus the HDR
//! environment), and [`WavefrontRayBuffers`] (one chunk's wavefront ray-state
//! struct-of-arrays). See the parent module's doc comment, "Per-frame uploads,
//! persistent staging" and "Wavefront pipeline" sections.

use wgpu::BufferUsages;

use crate::{
    geometry::GpuFacetPlane,
    optics::raytracer::EnvironmentSource,
    renderer::{
        buffers::{GpuCameraParams, GpuGemMaterial},
        env_map_gpu::HdrEnvGpuData,
        gpu::{GpuContext, compute},
    },
};

use super::{DEBUG_BUFFER_FLOATS, GpuFrameError};

/// The four `transport_main` output buffers, sized for `capacity` (pixel, sample)
/// tuples.
///
/// Held across dispatches by [`super::GpuFrameRenderer`] and reallocated only when a frame
/// needs more capacity than the last one did -- the buffers are large (see the parent
/// module's doc comment) and a progressive render re-dispatches the same geometry many
/// times per second, so recreating them every frame would dominate the frame's own cost.
pub struct TransportOutputs {
    xyz: wgpu::Buffer,
    radiance: wgpu::Buffer,
    lambdas: wgpu::Buffer,
    path_pdf: wgpu::Buffer,
    /// `out_compat`'s backing buffer -- see that binding's own doc comment in
    /// `spectral_transport.wgsl`. Same `write_debug_buffers`-gated, self-test-only
    /// contract as `radiance`/`lambdas`/`path_pdf` above.
    compat: wgpu::Buffer,
    /// Accessed directly (not via an accessor) by [`super::renderer`]'s capacity
    /// bookkeeping and [`super::accumulate`]'s per-chunk resource resolution -- both
    /// siblings of this module.
    pub(super) capacity: usize,
}

impl TransportOutputs {
    pub(crate) fn new(device: &wgpu::Device, capacity: usize) -> Self {
        let usage = BufferUsages::STORAGE | BufferUsages::COPY_SRC;
        Self {
            xyz: compute::zeroed_buffer::<f32>(device, "transport out xyz", capacity * 3, usage),
            radiance: compute::zeroed_buffer::<f32>(
                device,
                "transport out radiance",
                capacity * 8,
                usage,
            ),
            lambdas: compute::zeroed_buffer::<f32>(
                device,
                "transport out lambdas",
                capacity * 8,
                usage,
            ),
            path_pdf: compute::zeroed_buffer::<f32>(
                device,
                "transport out path_pdf",
                capacity * 8,
                usage,
            ),
            compat: compute::zeroed_buffer::<u32>(
                device,
                "transport out compat",
                capacity * 8,
                usage,
            ),
            capacity,
        }
    }

    /// Like [`Self::new`], but for `renderer::gpu::frame`'s production dispatch only --
    /// `xyz` is sized for `capacity` tuples, while the three debug buffers are fixed at
    /// [`DEBUG_BUFFER_FLOATS`] regardless of `capacity`, since a production dispatch's
    /// `write_debug_buffers` is always off and the shader never writes them.
    pub(crate) fn new_production(device: &wgpu::Device, capacity: usize) -> Self {
        let usage = BufferUsages::STORAGE | BufferUsages::COPY_SRC;
        Self {
            xyz: compute::zeroed_buffer::<f32>(
                device,
                "transport out xyz (production)",
                capacity * 3,
                usage,
            ),
            radiance: compute::zeroed_buffer::<f32>(
                device,
                "transport out radiance (production, write_debug_buffers=0, unused)",
                DEBUG_BUFFER_FLOATS,
                usage,
            ),
            lambdas: compute::zeroed_buffer::<f32>(
                device,
                "transport out lambdas (production, write_debug_buffers=0, unused)",
                DEBUG_BUFFER_FLOATS,
                usage,
            ),
            path_pdf: compute::zeroed_buffer::<f32>(
                device,
                "transport out path_pdf (production, write_debug_buffers=0, unused)",
                DEBUG_BUFFER_FLOATS,
                usage,
            ),
            compat: compute::zeroed_buffer::<u32>(
                device,
                "transport out compat (production, write_debug_buffers=0, unused)",
                DEBUG_BUFFER_FLOATS,
                usage,
            ),
            capacity,
        }
    }

    pub(crate) const fn xyz(&self) -> &wgpu::Buffer {
        &self.xyz
    }

    pub(crate) const fn radiance(&self) -> &wgpu::Buffer {
        &self.radiance
    }

    pub(crate) const fn lambdas(&self) -> &wgpu::Buffer {
        &self.lambdas
    }

    pub(crate) const fn path_pdf(&self) -> &wgpu::Buffer {
        &self.path_pdf
    }

    pub(crate) const fn compat(&self) -> &wgpu::Buffer {
        &self.compat
    }
}

/// The four scene-input buffers (camera, material, facet geometry, facet finishes),
/// PERSISTENT across every [`super::GpuFrameRenderer::accumulate`]/[`super::GpuFrameRenderer::accumulate_cancellable`]
/// call, not merely within one -- see [`Self::ensure`]. Held as
/// [`super::GpuFrameRenderer::scene_buffers`] and updated in place via `queue.write_buffer`
/// (camera/material every call; `planes`/`facet_finishes` too, UNLESS the new scene needs
/// more capacity than the buffer already has, in which case it is recreated -- grown,
/// never shrunk, mirroring [`super::GpuFrameRenderer::ensure_capacity`]'s policy for `outputs`).
/// Avoids the four-fresh-buffers-per-call cost a naive implementation would pay: a
/// desktop viewport calling `accumulate` every ~16ms would otherwise recreate all four
/// wgpu buffers that often. None of these four differ between CHUNKS of one frame either
/// -- only `GpuTransportParams` does, itself also a persistent per-slot buffer (see
/// [`super::GpuFrameRenderer::params_buffers`]) rather than a fresh upload in
/// [`super::bind_groups::build_chunk_bind_group`]. See the parent module's doc comment's
/// "Per-frame uploads, persistent staging" section.
pub struct FrameSceneBuffers {
    pub(super) camera: wgpu::Buffer,
    pub(super) material: wgpu::Buffer,
    pub(super) planes: wgpu::Buffer,
    /// `planes`'s allocated capacity, in [`GpuFacetPlane`]s -- may exceed the CURRENT
    /// scene's live plane count when a smaller scene follows a larger one. The live
    /// count is never stored here (it is passed fresh into
    /// [`super::bind_groups::build_chunk_bind_group`] from `state.scene.planes.len()`),
    /// only the buffer's own high-water capacity.
    planes_capacity: usize,
    pub(super) facet_finishes: wgpu::Buffer,
    facet_finishes_capacity: usize,
    /// Bindings 10-14's backing data -- see [`HdrEnvGpuData`]'s doc comment.
    /// Unlike `camera`/`material`/`planes`/`facet_finishes` above, this is REBUILT (never
    /// `queue.write_buffer`d in place) whenever it changes, since a texel buffer's very
    /// SIZE depends on the map's resolution -- there is no fixed-capacity "grow, never
    /// shrink" policy to apply here, just "rebuild on identity change" (see
    /// [`Self::update`]).
    pub(super) hdr_env: HdrEnvGpuData,
    /// The `EnvironmentMap` [`Self::hdr_env`] was built from, identified by its `&self`
    /// pointer address (a scene's loaded HDR map is not `Clone`/`PartialEq`, and a caller
    /// that keeps rendering the same loaded map passes the same `&EnvironmentMap` every
    /// call -- see `EnvironmentSource::HdrMap`'s doc comment). `None` means "currently
    /// bound to [`HdrEnvGpuData::dummy`]", distinct from any real map's address, which
    /// [`Self::update`] uses to detect "no longer HDR" and rebuild back to the dummy.
    hdr_env_identity: Option<usize>,
}

/// Builds a fresh [`HdrEnvGpuData`] for `environment`: the real upload for `HdrMap`, or
/// [`HdrEnvGpuData::dummy`] for `Studio` -- see that type's own doc comment for why a
/// non-HDR scene still needs something bound at bindings 10-14. Returns the identity
/// [`FrameSceneBuffers::hdr_env_identity`] should record alongside it.
///
/// The native production call site that enforces
/// [`HdrEnvGpuData::fits_storage_binding`], declining BEFORE calling the (infallible)
/// [`HdrEnvGpuData::upload`] rather than having `upload` itself return a `Result` --
/// `upload` has several other callers across this crate's self-tests, outside
/// `renderer::gpu::frame`, that always pass small fixture maps and were left untouched
/// rather than every one of them needing a new `.expect(..)`/`?`. This function (reached
/// only from [`FrameSceneBuffers::new`]/[`FrameSceneBuffers::update`], both driven by
/// [`super::GpuFrameRenderer::prepare_turn`]) and wasm32's `accumulate_async` (which
/// repeats the same `fits_storage_binding` check inline -- it never had
/// `FrameSceneBuffers`'s cross-call persistence to share this helper through) are the
/// only two places a caller-controlled resolution reaches [`HdrEnvGpuData::upload`], so
/// they are the only two places that need to decline gracefully instead of assuming a
/// test fixture's size.
///
/// # Errors
///
/// [`GpuFrameError::UnsupportedEnvironment`] if `map`'s texel buffer would exceed the
/// device's storage-binding limit -- see [`HdrEnvGpuData::fits_storage_binding`]'s own
/// doc comment. [`HdrEnvGpuData::dummy`]'s `1x1` map is always well within any real
/// device's limit, so the `Studio` arm never fails.
pub fn build_hdr_env(
    device: &wgpu::Device,
    environment: EnvironmentSource<'_>,
) -> Result<(HdrEnvGpuData, Option<usize>), GpuFrameError> {
    match environment {
        EnvironmentSource::HdrMap(map) => {
            if !HdrEnvGpuData::fits_storage_binding(device, map) {
                return Err(GpuFrameError::UnsupportedEnvironment);
            }
            let identity = std::ptr::from_ref(map) as usize;
            Ok((HdrEnvGpuData::upload(device, map), Some(identity)))
        }
        EnvironmentSource::Studio { .. } => Ok((HdrEnvGpuData::dummy(device), None)),
    }
}

/// Bundles the per-call scene inputs [`FrameSceneBuffers::new`]/[`FrameSceneBuffers::update`]/
/// [`FrameSceneBuffers::ensure`] need, keeping their argument counts within clippy's
/// `too_many_arguments` limit -- the same reason [`super::ChunkFrameState`]/
/// [`super::bind_groups::TransportDispatchArgs`]/[`super::TurnSetup`] exist elsewhere in
/// this module tree.
#[derive(Clone, Copy)]
pub struct SceneBufferInputs<'a> {
    pub(crate) camera_params: &'a GpuCameraParams,
    pub(crate) material: &'a GpuGemMaterial,
    pub(crate) planes: &'a [GpuFacetPlane],
    pub(crate) facet_finishes: &'a [u32],
    pub(crate) environment: EnvironmentSource<'a>,
}

impl FrameSceneBuffers {
    /// Creates all four persistent scene buffers plus the HDR environment buffers for the
    /// very first time, sized exactly to the live data. `COPY_DST` is added to every
    /// scene-buffer usage (unlike the self-test-only [`super::bind_groups::build_bind_group`]'s
    /// one-shot uploads) since [`Self::update`] writes into these same buffers on every
    /// later call -- `hdr_env` has no such in-place-write path (see its own field doc
    /// comment).
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::UnsupportedEnvironment`] if `inputs.environment` is
    /// an HDR map [`build_hdr_env`] declines -- see that function's own `# Errors`.
    fn new(device: &wgpu::Device, inputs: &SceneBufferInputs<'_>) -> Result<Self, GpuFrameError> {
        let (hdr_env, hdr_env_identity) = build_hdr_env(device, inputs.environment)?;
        Ok(Self {
            camera: compute::upload(
                device,
                "transport camera (persistent)",
                std::slice::from_ref(inputs.camera_params),
                BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            ),
            material: compute::upload(
                device,
                "transport material (persistent)",
                std::slice::from_ref(inputs.material),
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            ),
            planes: compute::upload(
                device,
                "transport planes (persistent)",
                inputs.planes,
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            ),
            planes_capacity: inputs.planes.len(),
            facet_finishes: compute::upload(
                device,
                "transport facet finishes (persistent)",
                inputs.facet_finishes,
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            ),
            facet_finishes_capacity: inputs.facet_finishes.len(),
            hdr_env,
            hdr_env_identity,
        })
    }

    /// Updates all four persistent scene buffers in place for a new `accumulate` call, in
    /// lieu of recreating them -- see this struct's own doc comment. `hdr_env` is instead
    /// rebuilt from scratch whenever `inputs.environment`'s identity has changed since the
    /// last call (a different `EnvironmentMap`, or a switch to/from `HdrMap` entirely) --
    /// see [`build_hdr_env`].
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::UnsupportedEnvironment`] if the identity change above
    /// requires rebuilding `hdr_env` and [`build_hdr_env`] declines -- see that
    /// function's own `# Errors`. `self.camera`/`self.material`/`self.planes`/
    /// `self.facet_finishes` are already written by the time this can happen (they come
    /// first, above), so they stay valid regardless of whether this returns `Err`.
    fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        inputs: &SceneBufferInputs<'_>,
    ) -> Result<(), GpuFrameError> {
        let SceneBufferInputs {
            camera_params,
            material,
            planes,
            facet_finishes,
            environment,
        } = *inputs;

        // Fixed-size every call -- always a write, never a resize.
        queue.write_buffer(&self.camera, 0, bytemuck::bytes_of(camera_params));
        queue.write_buffer(&self.material, 0, bytemuck::bytes_of(material));

        if planes.len() > self.planes_capacity {
            self.planes = compute::upload(
                device,
                "transport planes (persistent, grown)",
                planes,
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            );
            self.planes_capacity = planes.len();
        } else {
            queue.write_buffer(&self.planes, 0, bytemuck::cast_slice(planes));
        }

        if facet_finishes.len() > self.facet_finishes_capacity {
            self.facet_finishes = compute::upload(
                device,
                "transport facet finishes (persistent, grown)",
                facet_finishes,
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            );
            self.facet_finishes_capacity = facet_finishes.len();
        } else {
            queue.write_buffer(
                &self.facet_finishes,
                0,
                bytemuck::cast_slice(facet_finishes),
            );
        }

        let new_identity = match environment {
            EnvironmentSource::HdrMap(map) => Some(std::ptr::from_ref(map) as usize),
            EnvironmentSource::Studio { .. } => None,
        };
        if new_identity != self.hdr_env_identity {
            let (hdr_env, hdr_env_identity) = build_hdr_env(device, environment)?;
            self.hdr_env = hdr_env;
            self.hdr_env_identity = hdr_env_identity;
        }
        Ok(())
    }

    /// Create-or-update entry point every caller in this module uses: `existing` comes
    /// from `self.scene_buffers.take()`, `None` only before this renderer's very first
    /// `accumulate` call. Taking `self.scene_buffers` out and returning an OWNED `Self`
    /// (rather than binding a `&FrameSceneBuffers` borrowed from `self`) deliberately
    /// keeps the returned value independent of `self` for the rest of the caller's
    /// dispatch loop, which still needs `self.dispatch_chunk`/`self.drain_pending_chunk`
    /// (`&self`/`&mut self`) alongside it -- the caller is responsible for putting it
    /// back via `self.scene_buffers = Some(..)` once done.
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::UnsupportedEnvironment`] if `inputs.environment` is
    /// an HDR map whose texel buffer would exceed the device's storage-binding limit --
    /// see [`build_hdr_env`]'s own `# Errors`. On the `existing.update` branch,
    /// an `Err` here still leaves every OTHER buffer (`camera`/`material`/`planes`/
    /// `facet_finishes`) freshly written -- only the HDR rebuild itself is abandoned, so
    /// `existing` is dropped along with this call rather than partially reused.
    pub(crate) fn ensure(
        existing: Option<Self>,
        ctx: &GpuContext,
        inputs: &SceneBufferInputs<'_>,
    ) -> Result<Self, GpuFrameError> {
        existing.map_or_else(
            || Self::new(&ctx.device, inputs),
            |mut existing| {
                existing.update(&ctx.device, &ctx.queue, inputs)?;
                Ok(existing)
            },
        )
    }
}

/// One chunk's wavefront ray-state struct-of-arrays buffers -- mirrors
/// `wavefront_transport.wgsl`'s bindings 16-33 exactly (binding 15,
/// `GpuWavefrontParams`, is uploaded separately since it's rewritten every bounce
/// round, unlike these, which are allocated once per chunk -- see
/// [`super::GpuFrameRenderer::dispatch_chunk_wavefront`]). Every per-ray array is flat,
/// sized to `tuples` rays (an 8-channel field to `tuples * 8`); `block_alive_count`/
/// `block_offset` are sized to `tuples.div_ceil(64)` workgroups. See
/// `wavefront_transport.wgsl`'s own module doc comment for what each binding holds.
pub struct WavefrontRayBuffers {
    pub(super) origin: wgpu::Buffer,
    pub(super) dir: wgpu::Buffer,
    pub(super) k: wgpu::Buffer,
    pub(super) prev_plane_normal: wgpu::Buffer,
    pub(super) flags: wgpu::Buffer,
    pub(super) stokes: wgpu::Buffer,
    pub(super) radiance: wgpu::Buffer,
    pub(super) path_pdf: wgpu::Buffer,
    pub(super) split_radiance: wgpu::Buffer,
    pub(super) compat: wgpu::Buffer,
    pub(super) pending_light_mis: wgpu::Buffer,
    /// `ray_pending_light_mis_dir` (binding 34) -- see that WGSL binding's own doc
    /// comment (`wavefront_transport.wgsl`) for what it holds (the light-MIS interior
    /// pre-refraction direction, carried alongside `pending_light_mis` itself).
    /// Allocated/reset exactly like `pending_light_mis`, just `[f32; 4]` instead of
    /// `f32` (`.w` unused, matching `origin`/`dir`/`k`/`prev_plane_normal` above).
    pub(super) pending_light_mis_dir: wgpu::Buffer,
    pub(super) lambdas: wgpu::Buffer,
    pub(super) seed: wgpu::Buffer,
    /// `active_ray_indices` (binding 29): the CURRENT bounce round's live ray indices.
    pub(super) active_in: wgpu::Buffer,
    /// `active_ray_indices_next` (binding 30): `wavefront_compact_scatter`'s
    /// destination, copied back into `active_in` before the next round -- see
    /// `dispatch_chunk_wavefront`'s own comment on why a copy, not a bind-group swap.
    pub(super) active_out: wgpu::Buffer,
    /// `compact_local_offset` (binding 31).
    pub(super) local_offset: wgpu::Buffer,
    /// `compact_block_alive_count` (binding 32) -- read back to the host every round.
    pub(super) block_alive_count: wgpu::Buffer,
    /// `compact_block_offset` (binding 33) -- written by the host every round, from the
    /// CPU-side exclusive prefix sum of `block_alive_count`'s readback.
    pub(super) block_offset: wgpu::Buffer,
}

impl WavefrontRayBuffers {
    pub(crate) fn new(device: &wgpu::Device, tuples: usize) -> Self {
        let storage_rw = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        // `block_alive_count` additionally needs `COPY_SRC`: the host reads it back
        // every bounce round (see `dispatch_chunk_wavefront`).
        let storage_rw_src = storage_rw | wgpu::BufferUsages::COPY_SRC;

        let workgroups = tuples.div_ceil(super::WORKGROUP_SIZE);
        Self {
            origin: compute::zeroed_buffer::<[f32; 4]>(device, "wf ray_origin", tuples, storage_rw),
            dir: compute::zeroed_buffer::<[f32; 4]>(device, "wf ray_dir", tuples, storage_rw),
            k: compute::zeroed_buffer::<[f32; 4]>(device, "wf ray_k", tuples, storage_rw),
            prev_plane_normal: compute::zeroed_buffer::<[f32; 4]>(
                device,
                "wf ray_prev_plane_normal",
                tuples,
                storage_rw,
            ),
            flags: compute::zeroed_buffer::<u32>(device, "wf ray_flags", tuples, storage_rw),
            stokes: compute::zeroed_buffer::<[f32; 4]>(
                device,
                "wf ray_stokes",
                tuples * 8,
                storage_rw,
            ),
            radiance: compute::zeroed_buffer::<f32>(
                device,
                "wf ray_radiance",
                tuples * 8,
                storage_rw,
            ),
            path_pdf: compute::zeroed_buffer::<f32>(
                device,
                "wf ray_path_pdf",
                tuples * 8,
                storage_rw,
            ),
            split_radiance: compute::zeroed_buffer::<f32>(
                device,
                "wf ray_split_radiance",
                tuples * 8,
                storage_rw,
            ),
            compat: compute::zeroed_buffer::<u32>(device, "wf ray_compat", tuples * 8, storage_rw),
            pending_light_mis: compute::zeroed_buffer::<f32>(
                device,
                "wf ray_pending_light_mis",
                tuples,
                storage_rw,
            ),
            pending_light_mis_dir: compute::zeroed_buffer::<[f32; 4]>(
                device,
                "wf ray_pending_light_mis_dir",
                tuples,
                storage_rw,
            ),
            lambdas: compute::zeroed_buffer::<f32>(
                device,
                "wf ray_lambdas",
                tuples * 8,
                storage_rw,
            ),
            seed: compute::zeroed_buffer::<u32>(device, "wf ray_seed", tuples, storage_rw),
            active_in: compute::zeroed_buffer::<u32>(
                device,
                "wf active_ray_indices",
                tuples,
                storage_rw,
            ),
            // Needs `COPY_SRC` -- after
            // `wavefront_compact_scatter` fills it, `dispatch_chunk_wavefront`'s
            // "wavefront active-list copy encoder" copies its content BACK into
            // `active_in` for the next bounce round (see that copy's own comment); a
            // buffer can only be a `copy_buffer_to_buffer` source with `COPY_SRC` set,
            // which plain `storage_rw` (used by every other read/write ray-state buffer
            // here, none of which is ever a copy source) does not include.
            active_out: compute::zeroed_buffer::<u32>(
                device,
                "wf active_ray_indices_next",
                tuples,
                storage_rw_src,
            ),
            local_offset: compute::zeroed_buffer::<u32>(
                device,
                "wf compact_local_offset",
                tuples,
                storage_rw,
            ),
            block_alive_count: compute::zeroed_buffer::<u32>(
                device,
                "wf compact_block_alive_count",
                workgroups.max(1),
                storage_rw_src,
            ),
            block_offset: compute::zeroed_buffer::<u32>(
                device,
                "wf compact_block_offset",
                workgroups.max(1),
                storage_rw,
            ),
        }
    }
}
