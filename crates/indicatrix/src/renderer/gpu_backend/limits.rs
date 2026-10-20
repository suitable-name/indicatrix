//! Optional bounded execution for mobile consumers. Default desktop behavior stays intact.
use super::{GpuBackend, GpuSceneRef};

/// Hard request and dispatch ceilings. These bound work allocation, never final quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuWorkLimits {
    pub max_frame_pixels: u64,
    pub max_samples_per_call: u32,
    pub chunk_budget_bytes: usize,
    pub chunks_per_turn: usize,
}

impl GpuWorkLimits {
    /// Small deterministic kernel: four storage buffers, at most sixteen AA taps.
    pub const WHITTED: Self = Self {
        max_frame_pixels: 1024 * 1024,
        max_samples_per_call: 16,
        chunk_budget_bytes: 4 * 1024 * 1024,
        chunks_per_turn: 1,
    };
    /// Conservative first-device integration; larger final images take exact CPU fallback.
    pub const MOBILE: Self = Self {
        max_frame_pixels: 512 * 512,
        max_samples_per_call: 1,
        chunk_budget_bytes: 1024 * 1024,
        chunks_per_turn: 1,
    };

    #[must_use]
    pub fn decline_reason(self, scene: &GpuSceneRef<'_>, samples: u32) -> Option<&'static str> {
        let pixels = u64::from(scene.width) * u64::from(scene.height);
        if pixels == 0 || pixels > self.max_frame_pixels {
            Some("Frame exceeds the configured mobile GPU memory budget")
        } else if samples == 0 || samples > self.max_samples_per_call {
            Some("Sample batch exceeds the configured mobile GPU dispatch budget")
        } else {
            None
        }
    }
}

#[cfg(feature = "gpu")]
impl GpuBackend {
    /// Acquires an adapter and compiles the megakernel.
    ///
    /// Do this once, off the frame or request path: both steps are slow enough to
    /// matter. A machine with no usable GPU is an expected outcome, logged at `info`,
    /// after which every call declines.
    #[must_use]
    pub fn acquire() -> Self {
        let (backend, decline) = Self::acquire_with_diagnostics();
        if let Some(reason) = decline {
            tracing::info!(%reason, "GPU render backend unavailable, using CPU tracer");
        } else {
            tracing::info!(adapter = ?backend.adapter_label(), "GPU render backend active");
        }
        backend
    }

    /// Probe and shader validation are expensive; call on a preparation worker.
    /// A declined probe retains the exact upstream error for the UI fallback explanation.
    #[must_use]
    pub fn acquire_with_diagnostics() -> (Self, Option<String>) {
        match crate::renderer::gpu::GpuFrameRenderer::new() {
            Ok(renderer) => (
                Self::with_renderer(Some(std::sync::Mutex::new(renderer))),
                None,
            ),
            Err(error) => (Self::disabled(), Some(error.to_string())),
        }
    }

    /// Configures this backend before handing it to execution workers. Batching is
    /// deliberately declined in bounded mode; callers submit single small requests.
    pub fn set_work_limits(&self, limits: GpuWorkLimits) {
        let bounded = GpuWorkLimits {
            max_frame_pixels: limits.max_frame_pixels.max(1),
            max_samples_per_call: limits.max_samples_per_call.max(1),
            chunk_budget_bytes: limits.chunk_budget_bytes.max(12),
            chunks_per_turn: limits.chunks_per_turn.max(1),
        };
        *self
            .limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(bounded);
    }

    pub(super) fn configured_limits(&self) -> Option<GpuWorkLimits> {
        *self
            .limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn work_admits(&self, scene: &GpuSceneRef<'_>, samples: u32) -> bool {
        self.configured_limits()
            .is_none_or(|limits| limits.decline_reason(scene, samples).is_none())
    }
}

#[cfg(not(feature = "gpu"))]
impl GpuBackend {
    #[must_use]
    pub fn acquire_with_diagnostics() -> (Self, Option<String>) {
        (
            Self::disabled(),
            Some("This build does not include device GPU tracing".into()),
        )
    }
    pub const fn set_work_limits(&self, _limits: GpuWorkLimits) {}
}
