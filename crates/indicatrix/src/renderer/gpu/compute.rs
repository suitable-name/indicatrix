//! Minimal compute-pipeline harness shared by every `gpu`-feature self-test.
//!
//! Shader-module/pipeline creation, POD buffer upload, and blocking storage-buffer
//! readback, used across `renderer::gpu`. Generic `wgpu` plumbing, not specific to
//! `indicatrix`'s physics -- no render passes, no textures, no async scheduling beyond
//! a single blocking [`wgpu::Device::poll`] per readback.

use std::time::Duration;

use wgpu::util::DeviceExt;

/// Bound on every production-path GPU wait: [`finish_map_read`]'s `Device::poll` AND the
/// paired `mpsc::Receiver::recv_timeout` for the buffer-mapping callback (also used by
/// [`readback`]/[`dispatch_and_wait`]).
///
/// Without a bound a hung driver blocks forever; 4s sits well above the ~150ms per-chunk
/// target (`TARGET_CHUNK_MS`) even on a slow integrated GPU, while still surfacing a
/// wedged driver as `GpuFrameError::DeviceLost`.
pub const GPU_WAIT_TIMEOUT: Duration = Duration::from_secs(4);

/// Why a bounded, non-panicking production-path GPU wait ([`finish_map_read`]) failed to
/// produce a result.
///
/// Every variant means the device/driver can't be trusted to make forward progress; the
/// right response (see `GpuBackend`'s `lost` flag) is to stop using this
/// `GpuFrameRenderer`, never `.expect()`/panic.
#[derive(Debug)]
pub enum ComputeWaitError {
    /// `Device::poll` reported an error -- usually [`wgpu::PollError::Timeout`] but also
    /// a stale submission index.
    Poll(wgpu::PollError),
    /// The mapping callback did not resolve within [`GPU_WAIT_TIMEOUT`].
    MapTimeout,
    /// The buffer-mapping callback itself reported failure.
    Map(wgpu::BufferAsyncError),
    /// Mapping succeeded, but the mapped range could not be read.
    MapRange(wgpu::MapRangeError),
}

impl std::fmt::Display for ComputeWaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Poll(e) => write!(f, "device poll failed or timed out: {e}"),
            Self::MapTimeout => write!(
                f,
                "buffer-mapping callback did not fire within {GPU_WAIT_TIMEOUT:?}"
            ),
            Self::Map(e) => write!(f, "buffer mapping failed: {e}"),
            Self::MapRange(e) => write!(f, "mapped buffer range could not be read: {e}"),
        }
    }
}

impl std::error::Error for ComputeWaitError {}

/// Creates a compute pipeline from inline WGSL source with a single entry point.
///
/// Lets `wgpu` infer the bind group layout from the shader (`layout: None`) -- every
/// self-test kernel binds a small, fixed set of buffers, so hand-declaring a
/// [`wgpu::BindGroupLayout`] has no benefit. Uses default compilation options, so any
/// `override` declarations keep their defaults -- see
/// [`create_compute_pipeline_with_constants`] to set them.
///
/// # Panics
///
/// Panics if `wgsl_source` fails to parse/validate, or `entry_point` doesn't name a
/// `@compute` entry point -- a bad shader here is a bug in this crate, not runtime input.
#[must_use]
pub fn create_compute_pipeline(
    device: &wgpu::Device,
    label: &str,
    wgsl_source: &str,
    entry_point: &str,
) -> wgpu::ComputePipeline {
    create_compute_pipeline_with_constants(device, label, wgsl_source, entry_point, &[])
}

/// Like [`create_compute_pipeline`], but also sets `constants` on
/// [`wgpu::PipelineCompilationOptions`] -- WGSL `override` name/value pairs resolved at
/// pipeline creation.
///
/// `GpuFrameRenderer`'s material-class-specialised pipelines are the
/// only caller with non-empty `constants`: fixing `MATERIAL_CLASS` per pipeline lets
/// naga/the driver dead-code-eliminate other material classes' per-ray state.
///
/// # Panics
///
/// Same as [`create_compute_pipeline`], plus if `constants` names an identifier with no
/// matching `override`, or a value that doesn't fit its declared type.
#[must_use]
pub fn create_compute_pipeline_with_constants(
    device: &wgpu::Device,
    label: &str,
    wgsl_source: &str,
    entry_point: &str,
    constants: &[(&str, f64)],
) -> wgpu::ComputePipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(wgsl_source.into()),
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: None,
        module: &shader,
        entry_point: Some(entry_point),
        compilation_options: wgpu::PipelineCompilationOptions {
            constants,
            zero_initialize_workgroup_memory: true,
        },
        cache: None,
    })
}

/// Uploads `data` into a new buffer with `usage` via
/// [`wgpu::util::DeviceExt::create_buffer_init`].
#[must_use]
pub fn upload<T: bytemuck::Pod>(
    device: &wgpu::Device,
    label: &str,
    data: &[T],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(data),
        usage,
    })
}

/// Creates a new, zero-initialized buffer sized for `count` `T`s with `usage`.
///
/// Used for self-test output buffers: starting from known zeros makes a byte the
/// kernel fails to write (e.g. a struct-layout mismatch clobbering padding)
/// distinguishable from allocator garbage.
#[must_use]
pub fn zeroed_buffer<T: bytemuck::Pod>(
    device: &wgpu::Device,
    label: &str,
    count: usize,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let zeros = vec![T::zeroed(); count];
    upload(device, label, &zeros, usage)
}

/// Blocking readback of `count` `T`s from a `COPY_SRC` buffer.
///
/// Copies into a fresh `MAP_READ` staging buffer, submits, blocks (bounded by
/// [`GPU_WAIT_TIMEOUT`]) on the poll, and returns the mapped bytes reinterpreted as `T`.
/// Self-test-only: kept panicking (unlike [`finish_map_read`]) since converting every
/// caller to `Result` has no production benefit, but the wait is still bounded.
///
/// # Panics
///
/// Panics if the device poll times out or errors, or the buffer never finishes mapping.
#[must_use]
pub fn readback<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    source: &wgpu::Buffer,
    count: usize,
) -> Vec<T> {
    let byte_len = (count * size_of::<T>()) as wgpu::BufferAddress;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("indicatrix gpu self-test readback staging buffer"),
        size: byte_len,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("indicatrix gpu self-test readback encoder"),
    });
    encoder.copy_buffer_to_buffer(source, 0, &staging, 0, byte_len);
    queue.submit(Some(encoder.finish()));

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(GPU_WAIT_TIMEOUT),
        })
        .expect("device poll failed or timed out during readback");
    rx.recv_timeout(GPU_WAIT_TIMEOUT)
        .expect("map_async callback did not fire within GPU_WAIT_TIMEOUT")
        .expect("failed to map readback staging buffer");

    let out = {
        let data = slice
            .get_mapped_range()
            .expect("staging buffer should be mapped at this point");
        bytemuck::cast_slice::<u8, T>(&data).to_vec()
    };
    staging.unmap();
    out
}

/// Dispatches `pipeline` once with a single bind group (group 0), then blocks until the
/// GPU finishes (bounded by [`GPU_WAIT_TIMEOUT`]).
///
/// # Panics
///
/// Panics if the device poll times out or reports an error -- see [`readback`].
pub fn dispatch_and_wait(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    bind_group: &wgpu::BindGroup,
    workgroups: (u32, u32, u32),
) {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("indicatrix gpu self-test dispatch encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("indicatrix gpu self-test compute pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.dispatch_workgroups(workgroups.0, workgroups.1, workgroups.2);
    }
    queue.submit(Some(encoder.finish()));
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(GPU_WAIT_TIMEOUT),
        })
        .expect("device poll failed or timed out during dispatch");
}

/// The non-blocking counterpart to [`dispatch_and_wait`]: submits `pipeline` once but
/// does not poll or wait.
///
/// `GpuFrameRenderer::accumulate`'s chunked dispatch pairs this with
/// [`copy_to_staging`]/[`begin_map_read`]/[`finish_map_read`] to queue a chunk's work
/// without stalling the CPU before the next chunk. Self-tests use [`dispatch_and_wait`].
#[must_use]
pub fn dispatch(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    bind_group: &wgpu::BindGroup,
    workgroups: (u32, u32, u32),
) -> wgpu::SubmissionIndex {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("indicatrix gpu pipelined dispatch encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("indicatrix gpu pipelined compute pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.dispatch_workgroups(workgroups.0, workgroups.1, workgroups.2);
    }
    queue.submit(Some(encoder.finish()))
}

/// Submits a copy of `count` `T`s from `source` (must carry `COPY_SRC`) into a fresh
/// `MAP_READ` staging buffer, without blocking or mapping.
///
/// Pairs with [`begin_map_read`]/[`finish_map_read`] so the eventual blocking wait
/// overlaps later queued GPU work rather than idling.
#[must_use]
pub fn copy_to_staging<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    source: &wgpu::Buffer,
    count: usize,
    label: &str,
) -> (wgpu::Buffer, wgpu::SubmissionIndex) {
    let byte_len = (count * size_of::<T>()) as wgpu::BufferAddress;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: byte_len,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("indicatrix gpu pipelined readback copy encoder"),
    });
    encoder.copy_buffer_to_buffer(source, 0, &staging, 0, byte_len);
    let index = queue.submit(Some(encoder.finish()));
    (staging, index)
}

/// Begins the async map for a staging buffer created by [`copy_to_staging`]. Returns a
/// receiver that resolves once [`finish_map_read`] drives the device's callbacks.
///
/// Splitting "begin" from "block until done" lets a caller queue several chunks before
/// blocking on any one.
#[must_use]
pub fn begin_map_read(
    staging: &wgpu::Buffer,
) -> std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>> {
    let (tx, rx) = std::sync::mpsc::channel();
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    rx
}

/// Blocks (bounded by [`GPU_WAIT_TIMEOUT`]) until `staging`'s producing submission
/// (`copy_index`, from [`copy_to_staging`]) completes and `rx` (from [`begin_map_read`])
/// resolves, then reads it back and unmaps it.
///
/// The read-back length is whatever `count` [`copy_to_staging`] sized `staging` for.
/// Waiting on `copy_index` specifically (rather than "most recent submission") avoids
/// also waiting for a later chunk already queued. This is the ONE production-path wait
/// (`GpuFrameRenderer::accumulate`'s chunk pipeline is its only caller); every failure
/// returns [`ComputeWaitError`] rather than panicking, for the caller to turn into
/// `GpuFrameError::DeviceLost`.
///
/// # Errors
///
/// [`ComputeWaitError`] if the poll times out or errors, the mapping callback doesn't
/// resolve in time, the callback reports failure, or the mapped range can't be read.
pub fn finish_map_read<T: bytemuck::Pod>(
    device: &wgpu::Device,
    staging: &wgpu::Buffer,
    copy_index: wgpu::SubmissionIndex,
    rx: &std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
) -> Result<Vec<T>, ComputeWaitError> {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(copy_index),
            timeout: Some(GPU_WAIT_TIMEOUT),
        })
        .map_err(ComputeWaitError::Poll)?;
    rx.recv_timeout(GPU_WAIT_TIMEOUT)
        .map_err(|_| ComputeWaitError::MapTimeout)?
        .map_err(ComputeWaitError::Map)?;

    let out = {
        let data = staging
            .slice(..)
            .get_mapped_range()
            .map_err(ComputeWaitError::MapRange)?;
        bytemuck::cast_slice::<u8, T>(&data).to_vec()
    };
    staging.unmap();
    Ok(out)
}

/// Builds a single-group bind group from `(binding, buffer)` pairs, using `pipeline`'s
/// auto-inferred layout for group 0 (see [`create_compute_pipeline`]'s `layout: None`).
#[must_use]
pub fn bind_buffers(
    device: &wgpu::Device,
    label: &str,
    pipeline: &wgpu::ComputePipeline,
    buffers: &[(u32, &wgpu::Buffer)],
) -> wgpu::BindGroup {
    let layout = pipeline.get_bind_group_layout(0);
    let entries: Vec<wgpu::BindGroupEntry<'_>> = buffers
        .iter()
        .map(|(binding, buffer)| wgpu::BindGroupEntry {
            binding: *binding,
            resource: buffer.as_entire_binding(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &layout,
        entries: &entries,
    })
}

/// Like [`bind_buffers`], but each entry may bind an explicit BYTE size rather than the
/// buffer's whole allocation.
///
/// `renderer::gpu::frame`'s persistent scene buffers (`planes`/`facet_finishes`) are
/// grown-never-shrunk across frames -- a buffer can be LARGER than the live scene
/// currently bound to it. Binding the whole (possibly stale, larger) allocation via
/// [`bind_buffers`]'s `as_entire_binding` would make the shader's `arrayLength(&planes)`
/// see the buffer's full capacity rather than the live element count. Passing
/// `Some(bytes)` here binds exactly the first `bytes` bytes instead; `None` binds the
/// whole buffer, identical to [`bind_buffers`].
///
/// `Some(0)` degenerates to `None` (binding the whole buffer) rather than a genuine
/// empty range -- `wgpu::BufferSize` cannot represent zero, and in practice this crate
/// never dispatches a zero-facet scene, so the fallback is unreachable in normal use.
#[must_use]
pub fn bind_buffers_sized(
    device: &wgpu::Device,
    label: &str,
    pipeline: &wgpu::ComputePipeline,
    buffers: &[(u32, &wgpu::Buffer, Option<wgpu::BufferAddress>)],
) -> wgpu::BindGroup {
    let layout = pipeline.get_bind_group_layout(0);
    let entries: Vec<wgpu::BindGroupEntry<'_>> = buffers
        .iter()
        .map(|(binding, buffer, size)| wgpu::BindGroupEntry {
            binding: *binding,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: size.and_then(wgpu::BufferSize::new),
            }),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &layout,
        entries: &entries,
    })
}

/// Like [`finish_map_read`], but copies into a caller-provided, reusable `Vec<T>`
/// instead of allocating a fresh one on every call.
///
/// `GpuFrameRenderer::drain_pending_chunk` holds one scratch `Vec<f32>` across every
/// chunk of every frame and passes it here, rather than paying a heap allocation per
/// chunk the way the plain [`finish_map_read`] does. [`finish_map_read`] itself is kept
/// unchanged (and is still what every self-test uses): converting every caller to a
/// reusable buffer has no benefit for a one-shot self-test dispatch.
///
/// # Errors
///
/// Same conditions as [`finish_map_read`]'s.
pub fn finish_map_read_into<T: bytemuck::Pod>(
    device: &wgpu::Device,
    staging: &wgpu::Buffer,
    copy_index: wgpu::SubmissionIndex,
    rx: &std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    out: &mut Vec<T>,
) -> Result<(), ComputeWaitError> {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(copy_index),
            timeout: Some(GPU_WAIT_TIMEOUT),
        })
        .map_err(ComputeWaitError::Poll)?;
    rx.recv_timeout(GPU_WAIT_TIMEOUT)
        .map_err(|_| ComputeWaitError::MapTimeout)?
        .map_err(ComputeWaitError::Map)?;

    {
        let data = staging
            .slice(..)
            .get_mapped_range()
            .map_err(ComputeWaitError::MapRange)?;
        let src: &[T] = bytemuck::cast_slice(&data);
        out.clear();
        out.extend_from_slice(src);
    }
    staging.unmap();
    Ok(())
}
