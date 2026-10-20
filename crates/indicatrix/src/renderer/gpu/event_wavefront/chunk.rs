//! Chunk ray buffers and chunk dispatch execution for the event wavefront renderer.
//!
//! Implements §4.4 option (b): one submission per bounce round with `write_buffer`
//! before each round to update `EwParams`, followed by a final reduction pass and
//! staging buffer readback.
//!
//! The scene buffers live in [`scene_buffers`], the per-kernel bind tables in [`bindings`].

mod bindings;
mod scene_buffers;

use wgpu::BufferUsages;

pub(crate) use bindings::bind_kernel;
#[cfg(test)]
pub(crate) use bindings::kernel_slots;
pub(crate) use scene_buffers::SceneBuffers;

use crate::renderer::{
    buffers::{EwHit, EwParams, EwRayChan, EwRayCore, GpuTransportParams},
    frame_scene::FrameScene,
    gpu::{compute, frame::GpuReduceParams},
};

use super::{
    EwError,
    accumulate::{MAX_WORKGROUPS_PER_DIMENSION, WORKGROUP_SIZE},
    kernels::{EwKernel, needed_kernels},
    pipelines::EwPipelines,
    readback::map_read,
};

#[inline]
fn count_u32(count: usize) -> u32 {
    u32::try_from(count).expect("a chunk's pixel and tuple counts fit u32")
}

/// Debug output buffers hold this many elements: the event renderer always runs with
/// `with_debug_buffers_disabled()`, so `transport_finalize_ray` never writes them (and WGSL
/// bounds-checks any stray write). Sizing them per ray would add 128 B/ray for nothing.
const DEBUG_BUFFER_FLOATS: usize = 8;

/// Chunk ray buffers, reused and grown across chunks.
pub struct ChunkRayBuffers {
    pub ew_params: wgpu::Buffer,
    pub ray_core: wgpu::Buffer,
    pub ray_chan: wgpu::Buffer,
    pub ray_hit: wgpu::Buffer,
    pub out_xyz: wgpu::Buffer,
    pub out_radiance: wgpu::Buffer,
    pub out_lambdas: wgpu::Buffer,
    pub out_path_pdf: wgpu::Buffer,
    pub out_compat: wgpu::Buffer,
    pub pixel_output: wgpu::Buffer,
    pub staging: wgpu::Buffer,
    pub reduce_params: wgpu::Buffer,
    pub capacity_rays: usize,
    pub capacity_pixels: usize,
}

/// The per-ray storage buffers, which grow together.
struct RayStorage {
    ray_core: wgpu::Buffer,
    ray_chan: wgpu::Buffer,
    ray_hit: wgpu::Buffer,
    out_xyz: wgpu::Buffer,
}

fn alloc_ray_storage(device: &wgpu::Device, rays: usize, suffix: &str) -> RayStorage {
    let storage_rw = BufferUsages::STORAGE | BufferUsages::COPY_DST;
    RayStorage {
        ray_core: compute::zeroed_buffer::<EwRayCore>(
            device,
            &format!("ew ray core{suffix}"),
            rays,
            storage_rw,
        ),
        ray_chan: compute::zeroed_buffer::<EwRayChan>(
            device,
            &format!("ew ray chan{suffix}"),
            rays,
            storage_rw,
        ),
        ray_hit: compute::zeroed_buffer::<EwHit>(
            device,
            &format!("ew ray hit{suffix}"),
            rays,
            storage_rw,
        ),
        out_xyz: compute::zeroed_buffer::<f32>(
            device,
            &format!("ew out xyz{suffix}"),
            rays * 3,
            storage_rw | BufferUsages::COPY_SRC,
        ),
    }
}

fn alloc_pixel_buffers(
    device: &wgpu::Device,
    pixels: usize,
    suffix: &str,
) -> (wgpu::Buffer, wgpu::Buffer) {
    let output = compute::zeroed_buffer::<f32>(
        device,
        &format!("ew pixel output{suffix}"),
        pixels * 3,
        BufferUsages::STORAGE | BufferUsages::COPY_SRC,
    );
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(&format!("ew staging buffer{suffix}")),
        size: (pixels * 3 * size_of::<f32>()) as wgpu::BufferAddress,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    (output, staging)
}

impl ChunkRayBuffers {
    /// Allocates chunk ray buffers for `capacity_pixels` pixels and `capacity_rays` rays.
    #[must_use]
    pub fn new(device: &wgpu::Device, capacity_pixels: usize, capacity_rays: usize) -> Self {
        let storage_rw = BufferUsages::STORAGE | BufferUsages::COPY_DST;
        let uniform_usage = BufferUsages::UNIFORM | BufferUsages::COPY_DST;
        let rays = alloc_ray_storage(device, capacity_rays, "");
        let (pixel_output, staging) = alloc_pixel_buffers(device, capacity_pixels, "");

        Self {
            ew_params: compute::zeroed_buffer::<EwParams>(
                device,
                "ew params uniform",
                1,
                uniform_usage,
            ),
            ray_core: rays.ray_core,
            ray_chan: rays.ray_chan,
            ray_hit: rays.ray_hit,
            out_xyz: rays.out_xyz,
            out_radiance: compute::zeroed_buffer::<f32>(
                device,
                "ew out radiance",
                DEBUG_BUFFER_FLOATS,
                storage_rw,
            ),
            out_lambdas: compute::zeroed_buffer::<f32>(
                device,
                "ew out lambdas",
                DEBUG_BUFFER_FLOATS,
                storage_rw,
            ),
            out_path_pdf: compute::zeroed_buffer::<f32>(
                device,
                "ew out path pdf",
                DEBUG_BUFFER_FLOATS,
                storage_rw,
            ),
            out_compat: compute::zeroed_buffer::<u32>(
                device,
                "ew out compat",
                DEBUG_BUFFER_FLOATS,
                storage_rw,
            ),
            pixel_output,
            staging,
            reduce_params: compute::zeroed_buffer::<GpuReduceParams>(
                device,
                "ew reduce params",
                1,
                uniform_usage,
            ),
            capacity_rays,
            capacity_pixels,
        }
    }

    /// Grows chunk ray buffers if the required capacities exceed current allocations.
    pub fn ensure_capacity(
        &mut self,
        device: &wgpu::Device,
        required_pixels: usize,
        required_rays: usize,
    ) {
        if required_rays > self.capacity_rays {
            let rays = alloc_ray_storage(device, required_rays, " (grown)");
            self.ray_core = rays.ray_core;
            self.ray_chan = rays.ray_chan;
            self.ray_hit = rays.ray_hit;
            self.out_xyz = rays.out_xyz;
            self.capacity_rays = required_rays;
        }

        if required_pixels > self.capacity_pixels {
            let (pixel_output, staging) = alloc_pixel_buffers(device, required_pixels, " (grown)");
            self.pixel_output = pixel_output;
            self.staging = staging;
            self.capacity_pixels = required_pixels;
        }
    }
}

/// Records one compute pass dispatching `pipeline` with `bind_group` over `workgroups`.
fn dispatch_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    pipeline: &wgpu::ComputePipeline,
    bind_group: &wgpu::BindGroup,
    workgroups: u32,
) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some(label),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.dispatch_workgroups(workgroups, 1, 1);
}

/// Uploads one dispatch round's [`EwParams`] into the chunk's uniform.
fn write_ew_params(queue: &wgpu::Queue, chunk_buffers: &ChunkRayBuffers, params: &EwParams) {
    queue.write_buffer(&chunk_buffers.ew_params, 0, bytemuck::bytes_of(params));
}

/// Executes one chunk dispatch and reads back the GPU-reduced per-pixel XYZ floats.
#[cfg_attr(
    target_arch = "wasm32",
    expect(
        clippy::future_not_send,
        reason = "wasm is single-threaded: the wgpu handles the future holds are not `Send` there"
    )
)]
#[expect(
    clippy::too_many_arguments,
    reason = "chunk dispatch needs device, queue, pipelines, persistent scene and chunk buffers, and chunk metrics"
)]
pub(crate) async fn execute_chunk(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipelines: &mut EwPipelines,
    scene_buffers: &SceneBuffers,
    chunk_buffers: &ChunkRayBuffers,
    scene: &FrameScene<'_>,
    material_class: u32,
    _pixel_offset: usize,
    pixels_this_chunk: usize,
    spp: u32,
    params: &GpuTransportParams,
) -> Result<Vec<f32>, EwError> {
    let total_rays = pixels_this_chunk * spp as usize;
    if total_rays == 0 {
        return Ok(Vec::new());
    }
    // A single sample slice larger than one dispatch can address (spp above the chunk cap makes
    // `chunk_pixels_for` return one pixel with `spp` rays) or than the buffers hold.
    let max_rays = MAX_WORKGROUPS_PER_DIMENSION * WORKGROUP_SIZE;
    if total_rays > max_rays || total_rays > chunk_buffers.capacity_rays {
        return Err(EwError::InvalidBufferSize {
            expected: max_rays.min(chunk_buffers.capacity_rays),
            actual: total_rays,
        });
    }

    let ray_workgroups = count_u32(total_rays).div_ceil(64);
    let has_scatter = scene.material.scattering_sigma_s > 0.0;

    // 1. Compile every needed pipeline, then build one bind group per kernel. The kernel list is
    //    in dispatch order: generate, extend, [scatter, nee_hg], surface, arms, split, tail,
    //    finalize.
    let kernels = needed_kernels(material_class, scene);
    for &kernel in &kernels {
        pipelines.ensure(device, kernel, material_class);
    }
    let binds = build_binds(
        device,
        pipelines,
        &kernels,
        material_class,
        scene_buffers,
        chunk_buffers,
    );
    let bind_of = |kernel: EwKernel| -> &wgpu::BindGroup {
        &binds
            .iter()
            .find(|(k, _)| *k == kernel)
            .expect("kernel is in the needed list")
            .1
    };
    let ew_params = |bounce: u32, finalize_all: u32| EwParams {
        chunk_rays: count_u32(total_rays),
        bounce,
        finalize_all,
        has_scatter: u32::from(has_scatter),
        pad0: 0,
        pad1: 0,
        pad2: 0,
        pad3: 0,
    };

    // 2. Upload per-chunk transport params and dispatch ew_generate (bounce = 0).
    queue.write_buffer(&scene_buffers.params, 0, bytemuck::bytes_of(params));
    write_ew_params(queue, chunk_buffers, &ew_params(0, 0));
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ew_generate encoder"),
    });
    dispatch_pass(
        &mut encoder,
        "ew_generate pass",
        pipelines.get(EwKernel::Generate, material_class),
        bind_of(EwKernel::Generate),
        ray_workgroups,
    );
    queue.submit(Some(encoder.finish()));

    // 3. Bounce loop: every kernel after Generate, in list order, once per round.
    for bounce in 0..scene.max_bounces {
        write_ew_params(queue, chunk_buffers, &ew_params(bounce, 0));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ew bounce round encoder"),
        });
        for (kernel, bind_group) in binds.iter().skip(1) {
            dispatch_pass(
                &mut encoder,
                kernel.entry_point(),
                pipelines.get(*kernel, material_class),
                bind_group,
                ray_workgroups,
            );
        }
        queue.submit(Some(encoder.finish()));
    }

    // 4. Post-loop: finalize survivors (finalize_all = 1).
    write_ew_params(queue, chunk_buffers, &ew_params(scene.max_bounces, 1));
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ew finalize survivors encoder"),
    });
    dispatch_pass(
        &mut encoder,
        "ew finalize survivors pass",
        pipelines.get(EwKernel::Finalize, material_class),
        bind_of(EwKernel::Finalize),
        ray_workgroups,
    );
    queue.submit(Some(encoder.finish()));

    // 5. Reduce out_xyz -> pixel_output, copy to staging, read back.
    reduce_to_staging(
        device,
        queue,
        pipelines,
        chunk_buffers,
        pixels_this_chunk,
        spp,
    );
    map_read(device, queue, &chunk_buffers.staging, pixels_this_chunk * 3).await
}

/// Reduces `out_xyz` into `pixel_output` and copies the result into the staging buffer.
fn reduce_to_staging(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipelines: &EwPipelines,
    chunk_buffers: &ChunkRayBuffers,
    pixels_this_chunk: usize,
    spp: u32,
) {
    let reduce_workgroups = count_u32(pixels_this_chunk).div_ceil(64);
    queue.write_buffer(
        &chunk_buffers.reduce_params,
        0,
        bytemuck::bytes_of(&GpuReduceParams {
            num_pixels: count_u32(pixels_this_chunk),
            num_samples: spp,
            _pad0: 0,
            _pad1: 0,
        }),
    );

    let bg_reduce = compute::bind_buffers(
        device,
        "ew reduce bind group",
        pipelines.reduce_pipeline(),
        &[
            (0, &chunk_buffers.reduce_params),
            (1, &chunk_buffers.out_xyz),
            (2, &chunk_buffers.pixel_output),
        ],
    );

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ew reduce and copy encoder"),
    });
    dispatch_pass(
        &mut encoder,
        "ew reduce pass",
        pipelines.reduce_pipeline(),
        &bg_reduce,
        reduce_workgroups,
    );

    let copy_bytes = (pixels_this_chunk * 3 * size_of::<f32>()) as wgpu::BufferAddress;
    encoder.copy_buffer_to_buffer(
        &chunk_buffers.pixel_output,
        0,
        &chunk_buffers.staging,
        0,
        copy_bytes,
    );
    queue.submit(Some(encoder.finish()));
}

/// The binding sizes of the two unsized scene arrays, in bytes: `planes` holds the planes and
/// the packed tool records (`packed_len` entries of one `GpuFacetPlane`), `facet_finishes` one
/// `u32` per entry. The shader's `arrayLength(&planes)` is derived from the first, so it must
/// follow the current scene, never a buffer's grown capacity.
pub(super) const fn scene_binding_bytes(
    packed_len: usize,
) -> (wgpu::BufferAddress, wgpu::BufferAddress) {
    (
        (packed_len * size_of::<crate::geometry::GpuFacetPlane>()) as wgpu::BufferAddress,
        (packed_len * size_of::<u32>()) as wgpu::BufferAddress,
    )
}

/// One bind group per kernel of `kernels`, in the same order.
fn build_binds(
    device: &wgpu::Device,
    pipelines: &EwPipelines,
    kernels: &[EwKernel],
    material_class: u32,
    scene_buffers: &SceneBuffers,
    chunk_buffers: &ChunkRayBuffers,
) -> Vec<(EwKernel, wgpu::BindGroup)> {
    let (planes_bytes, finishes_bytes) = scene_binding_bytes(scene_buffers.planes_len);
    kernels
        .iter()
        .map(|&kernel| {
            let bind_group = bind_kernel(
                device,
                pipelines.get(kernel, material_class),
                kernel,
                scene_buffers,
                chunk_buffers,
                planes_bytes,
                finishes_bytes,
            );
            (kernel, bind_group)
        })
        .collect()
}
