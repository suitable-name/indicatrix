//! GPU buffer readback for native and wasm32 targets.
//!
//! Every error path unmaps the staging buffer (cancelling a still-pending `map_async`),
//! so the next chunk's readback on the same buffer is valid again.

use super::EwError;

/// Copies the first `count` `f32`s out of the mapped staging buffer and unmaps it.
///
/// The buffer is unmapped on both the success and the error path.
fn read_mapped(buffer: &wgpu::Buffer, count: usize) -> Result<Vec<f32>, EwError> {
    let result = {
        match buffer.slice(..).get_mapped_range() {
            Err(e) => Err(EwError::DeviceLost(format!(
                "mapped buffer range could not be read: {e}"
            ))),
            Ok(data) => {
                let floats = bytemuck::cast_slice::<u8, f32>(&data);
                if floats.len() < count {
                    Err(EwError::DeviceLost(format!(
                        "mapped buffer too small: expected {count} floats, found {}",
                        floats.len()
                    )))
                } else {
                    Ok(floats[..count].to_vec())
                }
            }
        }
    };
    buffer.unmap();
    result
}

/// Reads `count` `f32`s from a mappable staging buffer.
///
/// On native: initiates async mapping and runs a polling loop bounded by `compute::GPU_WAIT_TIMEOUT`.
/// On wasm32: awaits a `futures_channel::oneshot` channel triggered by the browser `mapAsync`
/// callback, raced against the chunk deadline set through `clock::set_readback_deadline_ms`.
///
/// On a timeout the result is [`EwError::ChunkTimeout`]; on every error the staging buffer
/// is unmapped.
///
/// # Errors
///
/// [`EwError::ChunkTimeout`] if the mapping does not complete in time, or
/// [`EwError::DeviceLost`] if polling or mapping the buffer fails.
#[cfg(not(target_arch = "wasm32"))]
pub async fn map_read(
    device: &wgpu::Device,
    _queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    count: usize,
) -> Result<Vec<f32>, EwError> {
    let (tx, rx) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

    let start = std::time::Instant::now();
    loop {
        if let Err(e) = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(crate::renderer::gpu::compute::GPU_WAIT_TIMEOUT),
        }) {
            buffer.unmap();
            return Err(EwError::DeviceLost(format!("device poll failed: {e}")));
        }

        match rx.try_recv() {
            Ok(Ok(())) => break,
            Ok(Err(e)) => {
                buffer.unmap();
                return Err(EwError::DeviceLost(format!("buffer mapping failed: {e}")));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if start.elapsed() >= crate::renderer::gpu::compute::GPU_WAIT_TIMEOUT {
                    buffer.unmap();
                    return Err(EwError::ChunkTimeout);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                buffer.unmap();
                return Err(EwError::DeviceLost(
                    "readback channel disconnected unexpectedly".to_string(),
                ));
            }
        }
    }

    read_mapped(buffer, count)
}

/// Reads `count` `f32`s from a mappable staging buffer on wasm32.
///
/// # Errors
///
/// [`EwError::ChunkTimeout`] if the browser's `mapAsync` callback does not fire before the
/// chunk deadline, or [`EwError::DeviceLost`] if the mapping fails or its callback is dropped.
#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::future_not_send,
    reason = "wgpu web backend buffer mapping futures are !Send; wasm32 is single-threaded"
)]
pub async fn map_read(
    _device: &wgpu::Device,
    _queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    count: usize,
) -> Result<Vec<f32>, EwError> {
    use super::clock::{Raced, race_deadline, readback_deadline_ms};

    let (tx, rx) = futures_channel::oneshot::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

    match race_deadline(rx, readback_deadline_ms()).await {
        Raced::TimedOut => {
            buffer.unmap();
            return Err(EwError::ChunkTimeout);
        }
        Raced::Done(Err(_)) => {
            buffer.unmap();
            return Err(EwError::DeviceLost(
                "buffer-mapping callback was dropped before firing".to_string(),
            ));
        }
        Raced::Done(Ok(Err(e))) => {
            buffer.unmap();
            return Err(EwError::DeviceLost(format!(
                "wgpu buffer mapping failed: {e}"
            )));
        }
        Raced::Done(Ok(Ok(()))) => {}
    }

    read_mapped(buffer, count)
}
