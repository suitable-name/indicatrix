//! Stand-in for a build without the `gpu` feature -- see the `gpu`-gated
//! [`super::backend::GpuBackend`] for what this stands in for and why it exists at all.

use std::sync::atomic::AtomicBool;

use glam::Vec3;

use super::{GpuAccumulate, GpuBatchItem, GpuPipelineKind, GpuSceneRef};

/// Gpu backend.
pub struct GpuBackend;

impl GpuBackend {
    /// Creates the backend (a no-op stand-in without the `gpu` feature).
    #[must_use]
    pub const fn acquire() -> Self {
        Self
    }

    /// Creates a backend that never renders on the GPU.
    #[must_use]
    pub const fn disabled() -> Self {
        Self
    }

    /// Always `None`: with no `gpu` feature there is no adapter to name, so a caller
    /// advertising its backend correctly reports CPU.
    #[must_use]
    pub const fn adapter_label(&self) -> Option<String> {
        None
    }

    /// No-op: with no `gpu` feature there is no renderer to select a kernel on. Ignores
    /// `_kind` only to stay signature-compatible with the `gpu`-gated
    /// [`super::backend::GpuBackend::set_pipeline_kind`].
    pub const fn set_pipeline_kind(&self, _kind: GpuPipelineKind) {}

    /// Always `false`: with no `gpu` feature there is no device to ever lose -- see the
    /// `gpu`-gated [`super::backend::GpuBackend::is_lost`].
    #[must_use]
    pub const fn is_lost(&self) -> bool {
        false
    }

    /// Always `true` ("not lost, nothing to recover"): with no `gpu` feature there is no
    /// device to ever lose -- see the `gpu`-gated [`super::backend::GpuBackend::try_recover`].
    #[must_use]
    pub const fn try_recover(&self) -> bool {
        true
    }

    /// Always `None`, for the same reason [`Self::is_lost`] is always `false`.
    #[must_use]
    pub const fn last_lost_reason(&self) -> Option<String> {
        None
    }

    /// Always declines, leaving `accum` untouched.
    ///
    /// Ignores every argument only to stay signature-compatible with the real
    /// `try_accumulate` above -- that identical signature is what keeps callers free of
    /// `#[cfg]`.
    #[allow(
        clippy::unused_self,
        reason = "signature must match the `gpu`-gated GpuBackend::try_accumulate"
    )]
    /// Always reports the GPU as unavailable, matching the `gpu` build's signature.
    pub const fn try_accumulate(
        &self,
        _scene: &GpuSceneRef<'_>,
        _sample_offset: u32,
        _spp: u32,
        _accum: &mut [Vec3],
    ) -> bool {
        false
    }

    /// Always declines, leaving `accum` untouched -- see the `gpu`-gated
    /// [`super::backend::GpuBackend::try_accumulate_cancellable`] for what it stands in
    /// for. Ignores `cancel` (and every other argument) for the same reason
    /// [`Self::try_accumulate`] does.
    pub const fn try_accumulate_cancellable(
        &self,
        _scene: &GpuSceneRef<'_>,
        _sample_offset: u32,
        _spp: u32,
        _accum: &mut [Vec3],
        _cancel: &AtomicBool,
    ) -> GpuAccumulate {
        GpuAccumulate::Declined
    }

    /// Declines every item through `on_done`, leaving every `out` untouched -- see the
    /// `gpu`-gated [`super::backend::GpuBackend::try_accumulate_batch_cancellable`] for
    /// what it stands in for.
    #[allow(
        clippy::unused_self,
        reason = "signature must match the `gpu`-gated GpuBackend::try_accumulate_batch_cancellable"
    )]
    pub fn try_accumulate_batch_cancellable(
        &self,
        items: &mut [GpuBatchItem<'_>],
        _cancel: &AtomicBool,
        on_done: &mut dyn FnMut(usize, GpuAccumulate),
    ) {
        for index in 0..items.len() {
            on_done(index, GpuAccumulate::Declined);
        }
    }
}
