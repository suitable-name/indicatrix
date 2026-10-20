//! Ray-Traced's independent GPU admission and transactional CPU-fallback facade.
use super::{GpuAccumulate, GpuWorkLimits};
use crate::{
    geometry::tool::ToolPrimitive,
    renderer::{frame_scene::FrameScene, whitted::WhittedPrep},
};
use glam::Vec3;

#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
#[path = "whitted_native.rs"]
mod native;
#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
pub use native::WhittedGpuBackend;

/// Frozen GPU uploads for exactly the scene passed to preparation.
///
/// Later changes to a public CPU preparation cannot alter this snapshot. A
/// recovered/replaced device declines old snapshots, so consumers discard them
/// and prepare the new scene again.
pub struct WhittedGpuPrepared {
    #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
    inner: crate::renderer::gpu::whitted::WhittedGpuScene,
}

/// Browser consumers await the low-level renderer directly; builds without GPU
/// support retain exactly the same native facade signatures and cleanly decline.
#[cfg(any(not(feature = "gpu"), target_arch = "wasm32"))]
pub struct WhittedGpuBackend;

#[cfg(any(not(feature = "gpu"), target_arch = "wasm32"))]
impl WhittedGpuBackend {
    #[must_use]
    pub const fn disabled() -> Self {
        Self
    }
    #[must_use]
    pub const fn acquire() -> Self {
        Self
    }
    #[must_use]
    pub fn acquire_with_diagnostics() -> (Self, Option<String>) {
        (
            Self,
            Some("Ray-Traced device GPU is unavailable in this build".into()),
        )
    }
    #[must_use]
    pub const fn adapter_label(&self) -> Option<String> {
        None
    }
    #[must_use]
    pub const fn is_lost(&self) -> bool {
        false
    }
    #[must_use]
    pub const fn try_recover(&self) -> bool {
        false
    }
    pub const fn set_work_limits(&self, _limits: GpuWorkLimits) {}
    /// A build without GPU support prepares nothing.
    ///
    /// # Errors
    ///
    /// Always: [`GpuAccumulate::Cancelled`] when `cancel` reports it, else
    /// [`GpuAccumulate::Declined`].
    pub fn prepare_cancellable(
        &self,
        _scene: &FrameScene<'_>,
        _tools: &[ToolPrimitive],
        _prep: &WhittedPrep<'_>,
        cancel: &dyn Fn() -> bool,
    ) -> Result<WhittedGpuPrepared, GpuAccumulate> {
        Err(if cancel() {
            GpuAccumulate::Cancelled
        } else {
            GpuAccumulate::Declined
        })
    }
    pub fn try_accumulate_prepared_cancellable(
        &self,
        _prepared: &WhittedGpuPrepared,
        _offset: u32,
        _spp: u32,
        _accum: &mut [Vec3],
        cancel: &dyn Fn() -> bool,
    ) -> GpuAccumulate {
        if cancel() {
            GpuAccumulate::Cancelled
        } else {
            GpuAccumulate::Declined
        }
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "stub twin of the native one-shot facade; signatures must match"
    )]
    pub fn try_accumulate_cancellable(
        &self,
        _scene: &FrameScene<'_>,
        _tools: &[ToolPrimitive],
        _prep: &WhittedPrep<'_>,
        _offset: u32,
        _spp: u32,
        _accum: &mut [Vec3],
        cancel: &dyn Fn() -> bool,
    ) -> GpuAccumulate {
        if cancel() {
            GpuAccumulate::Cancelled
        } else {
            GpuAccumulate::Declined
        }
    }
}

impl WhittedGpuBackend {
    pub fn try_accumulate(
        &self,
        scene: &FrameScene<'_>,
        tools: &[ToolPrimitive],
        prep: &WhittedPrep<'_>,
        offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> bool {
        self.try_accumulate_cancellable(scene, tools, prep, offset, spp, accum, &|| false)
            == GpuAccumulate::Done
    }
}
