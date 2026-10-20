use super::{SHADER_SOURCE, WhittedGpuError, context};
use crate::renderer::gpu::{GpuContext, compute};

pub struct WhittedGpuRenderer {
    pub(super) id: u64,
    pub(super) ctx: GpuContext,
    pub(super) pipeline: wgpu::ComputePipeline,
    label: String,
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl WhittedGpuRenderer {
    /// Blocking form of [`Self::new_async`].
    ///
    /// # Errors
    ///
    /// See [`Self::new_async`].
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Result<Self, WhittedGpuError> {
        pollster::block_on(Self::new_async())
    }

    /// Genuine asynchronous adapter acquisition for browser clients.
    ///
    /// # Errors
    ///
    /// Returns [`WhittedGpuError::Declined`] when no suitable device can be
    /// acquired or the shader fails validation.
    #[cfg_attr(
        target_arch = "wasm32",
        allow(
            clippy::future_not_send,
            reason = "wgpu's web backend futures are inherently !Send (browser-side Rc \
                      handles); wasm32-unknown-unknown has no second thread regardless"
        )
    )]
    pub async fn new_async() -> Result<Self, WhittedGpuError> {
        let ctx = context::acquire()
            .await
            .map_err(|error| WhittedGpuError::Declined(error.to_string()))?;
        Self::from_context_async(ctx).await
    }

    /// Build the pipeline on an already acquired context.
    ///
    /// # Errors
    ///
    /// Returns [`WhittedGpuError::Declined`] when the device offers fewer than four
    /// storage buffers or the shader fails validation.
    #[cfg_attr(
        target_arch = "wasm32",
        allow(
            clippy::future_not_send,
            reason = "wgpu's web backend futures are inherently !Send (browser-side Rc \
                      handles); wasm32-unknown-unknown has no second thread regardless"
        )
    )]
    pub async fn from_context_async(ctx: GpuContext) -> Result<Self, WhittedGpuError> {
        if ctx.device.limits().max_storage_buffers_per_shader_stage < super::STORAGE_BUFFERS {
            return Err(WhittedGpuError::Declined(
                "Ray-Traced needs four storage buffers".into(),
            ));
        }
        let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pipeline = compute::create_compute_pipeline(
            &ctx.device,
            "Ray-Traced",
            SHADER_SOURCE,
            "whitted_main",
        );
        if let Some(error) = scope.pop().await {
            return Err(WhittedGpuError::Declined(format!(
                "Ray-Traced shader: {error}"
            )));
        }
        let info = ctx.adapter.get_info();
        let label = format!("{} ({:?}; Ray-Traced)", info.name, info.backend);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Self {
            id,
            ctx,
            pipeline,
            label,
        })
    }

    #[must_use]
    pub fn adapter_label(&self) -> &str {
        &self.label
    }

    pub(super) fn healthy(&self) -> Result<(), WhittedGpuError> {
        use std::sync::atomic::Ordering;
        if self.ctx.validation_error_seen.load(Ordering::Relaxed) {
            let text = self
                .ctx
                .last_uncaptured_error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .unwrap_or_else(|| "Ray-Traced GPU device is unavailable".into());
            Err(WhittedGpuError::DeviceLost(text))
        } else {
            Ok(())
        }
    }
}
