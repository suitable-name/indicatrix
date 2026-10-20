use super::{WhittedGpuError, WhittedGpuRenderer, WhittedGpuScene};
use crate::renderer::gpu::{compute, event_wavefront::readback};
use glam::Vec3;

impl WhittedGpuRenderer {
    /// Blocking form of [`Self::trace_chunk_async`].
    ///
    /// # Errors
    ///
    /// See [`Self::trace_chunk_async`].
    #[cfg(not(target_arch = "wasm32"))]
    pub fn trace_chunk(
        &mut self,
        scene: &WhittedGpuScene,
        first_pixel: u32,
        pixels: u32,
        sample_offset: u32,
        spp: u32,
    ) -> Result<Vec<Vec3>, WhittedGpuError> {
        pollster::block_on(self.trace_chunk_async(scene, first_pixel, pixels, sample_offset, spp))
    }

    /// Returns detached sums, never mutating the caller's accumulation before
    /// cancellation/device-health checks and the whole request commit succeed.
    ///
    /// # Errors
    ///
    /// Returns [`WhittedGpuError::Declined`] when the scene belongs to a replaced
    /// device or the pixel/tap range lies outside it, and
    /// [`WhittedGpuError::DeviceLost`] when the device is unhealthy, the readback
    /// fails, or the result is incomplete or non-finite.
    #[cfg_attr(
        target_arch = "wasm32",
        allow(
            clippy::future_not_send,
            reason = "wgpu's web backend futures are inherently !Send (browser-side Rc \
                      handles); wasm32-unknown-unknown has no second thread regardless"
        )
    )]
    pub async fn trace_chunk_async(
        &mut self,
        scene: &WhittedGpuScene,
        first_pixel: u32,
        pixels: u32,
        sample_offset: u32,
        spp: u32,
    ) -> Result<Vec<Vec3>, WhittedGpuError> {
        self.healthy()?;
        if scene.device_id != self.id {
            return Err(WhittedGpuError::Declined(
                "Ray-Traced uploads belong to a replaced GPU device".into(),
            ));
        }
        if pixels == 0
            || pixels > super::CHUNK_PIXELS
            || first_pixel
                .checked_add(pixels)
                .is_none_or(|end| end > scene.pixels)
            || spp == 0
            || sample_offset
                .checked_add(spp)
                .is_none_or(|end| end > scene.taps)
        {
            return Err(WhittedGpuError::Declined(
                "Ray-Traced pixel/tap range is outside the prepared scene".into(),
            ));
        }
        let mut params = scene.params;
        params.range = [first_pixel, pixels, sample_offset, spp];
        let uniform = compute::upload(
            &self.ctx.device,
            "Ray chunk params",
            &[params],
            wgpu::BufferUsages::UNIFORM,
        );
        let bytes = u64::from(pixels) * 12;
        let output = self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ray XYZ output"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let binding = compute::bind_buffers(
            &self.ctx.device,
            "Ray bindings",
            &self.pipeline,
            &[
                (0, &scene.camera),
                (1, &uniform),
                (2, &scene.planes),
                (3, &scene.environment),
                (4, &scene.dimensions),
                (5, &scene.frost),
                (6, &output),
            ],
        );
        let _submission = compute::dispatch(
            &self.ctx.device,
            &self.ctx.queue,
            &self.pipeline,
            &binding,
            (pixels.div_ceil(64), 1, 1),
        );
        let (staging, _copy) = compute::copy_to_staging::<f32>(
            &self.ctx.device,
            &self.ctx.queue,
            &output,
            pixels as usize * 3,
            "Ray readback",
        );
        let floats = readback::map_read(
            &self.ctx.device,
            &self.ctx.queue,
            &staging,
            pixels as usize * 3,
        )
        .await
        .map_err(|error| WhittedGpuError::DeviceLost(error.to_string()))?;
        self.healthy()?;
        let result: Vec<Vec3> = floats
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| Vec3::new(p[0], p[1], p[2]))
            .collect();
        if result.len() != pixels as usize || result.iter().any(|p| !p.is_finite()) {
            return Err(WhittedGpuError::DeviceLost(
                "Ray-Traced GPU returned a non-finite or incomplete frame".into(),
            ));
        }
        Ok(result)
    }
}
