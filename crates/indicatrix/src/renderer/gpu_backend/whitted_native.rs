use super::{GpuAccumulate, GpuWorkLimits, WhittedGpuPrepared};
use crate::renderer::gpu_backend::{recovery::RecoveryPolicy, turnstile::Turnstile};
use crate::{
    geometry::tool::ToolPrimitive,
    renderer::{
        frame_scene::FrameScene,
        gpu::whitted::{CHUNK_PIXELS, WhittedGpuError, WhittedGpuRenderer},
        whitted::WhittedPrep,
    },
};
use glam::Vec3;
use std::{sync::Mutex, time::Instant};

pub struct WhittedGpuBackend {
    renderer: Mutex<Option<WhittedGpuRenderer>>,
    recovery: Mutex<RecoveryPolicy>,
    turnstile: Turnstile,
    recoverable: bool,
    limits: Mutex<Option<GpuWorkLimits>>,
}

impl WhittedGpuBackend {
    const fn with_renderer(renderer: Option<WhittedGpuRenderer>) -> Self {
        let recoverable = renderer.is_some();
        Self {
            renderer: Mutex::new(renderer),
            recovery: Mutex::new(RecoveryPolicy::new()),
            turnstile: Turnstile::new(),
            recoverable,
            limits: Mutex::new(None),
        }
    }
    #[must_use]
    pub const fn disabled() -> Self {
        Self::with_renderer(None)
    }
    #[must_use]
    pub fn acquire() -> Self {
        Self::acquire_with_diagnostics().0
    }
    #[must_use]
    pub fn acquire_with_diagnostics() -> (Self, Option<String>) {
        match WhittedGpuRenderer::new() {
            Ok(renderer) => (Self::with_renderer(Some(renderer)), None),
            Err(error) => (Self::disabled(), Some(error.to_string())),
        }
    }
    pub fn adapter_label(&self) -> Option<String> {
        self.renderer
            .lock()
            .ok()?
            .as_ref()
            .map(|renderer| renderer.adapter_label().to_owned())
    }
    pub fn is_lost(&self) -> bool {
        self.recoverable && self.adapter_label().is_none()
    }
    pub fn set_work_limits(&self, limits: GpuWorkLimits) {
        *self
            .limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(limits);
    }
    pub fn try_recover(&self) -> bool {
        // A panic while owning GPU state makes its invariants unknown. A fresh
        // backend may be acquired, but this instance never reuses poisoned state.
        if self.renderer.is_poisoned() || !self.is_lost() {
            return false;
        }
        {
            let mut policy = self
                .recovery
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let now = Instant::now();
            if !policy.may_attempt(now) {
                return false;
            }
            policy.record_attempt(now);
        }
        let ticket = self.turnstile.take_ticket();
        let _turn = self.turnstile.wait_for_turn(ticket);
        let Ok(mut current) = self.renderer.lock() else {
            return false;
        };
        if current.is_some() {
            return true;
        }
        let Ok(renderer) = WhittedGpuRenderer::new() else {
            return false;
        };
        *current = Some(renderer);
        self.recovery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record_recovered();
        true
    }

    fn record_error(&self, current: &mut Option<WhittedGpuRenderer>, error: &WhittedGpuError) {
        if matches!(error, WhittedGpuError::DeviceLost(_)) {
            *current = None;
            self.recovery
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .record_loss(Instant::now());
        }
        tracing::info!(%error, "Ray-Traced GPU request declined");
    }

    fn request_admits(
        &self,
        pixels: u64,
        taps: u32,
        offset: u32,
        spp: u32,
        accum_len: usize,
    ) -> bool {
        !self.renderer.is_poisoned()
            && pixels != 0
            && u32::try_from(pixels).is_ok()
            && u64::try_from(accum_len).is_ok_and(|len| len == pixels)
            && spp != 0
            && offset.checked_add(spp).is_some_and(|end| end <= taps)
            && self.configured_limits().is_none_or(|limits| {
                pixels <= limits.max_frame_pixels && spp <= limits.max_samples_per_call
            })
    }

    fn configured_limits(&self) -> Option<GpuWorkLimits> {
        *self
            .limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Upload a frozen scene once, then reuse it for subsequent absolute tap ranges.
    /// Preparation is cancellable around its FIFO turn; no caller accumulator changes.
    ///
    /// # Errors
    ///
    /// Returns [`GpuAccumulate::Cancelled`] when `cancel` fires, and
    /// [`GpuAccumulate::Declined`] when the frame is empty or over the work limits,
    /// no healthy renderer is available, or the upload is declined.
    pub fn prepare_cancellable(
        &self,
        scene: &FrameScene<'_>,
        tools: &[ToolPrimitive],
        prep: &WhittedPrep<'_>,
        cancel: &dyn Fn() -> bool,
    ) -> Result<WhittedGpuPrepared, GpuAccumulate> {
        if cancel() {
            return Err(GpuAccumulate::Cancelled);
        }
        let pixels = u64::from(scene.width) * u64::from(scene.height);
        if pixels == 0
            || pixels > u64::from(u32::MAX)
            || self
                .configured_limits()
                .is_some_and(|limits| pixels > limits.max_frame_pixels)
        {
            return Err(GpuAccumulate::Declined);
        }
        let _ = self.try_recover();
        let ticket = self.turnstile.take_ticket();
        let _turn = self.turnstile.wait_for_turn(ticket);
        if cancel() {
            return Err(GpuAccumulate::Cancelled);
        }
        let Ok(mut current) = self.renderer.lock() else {
            return Err(GpuAccumulate::Declined);
        };
        let Some(renderer) = current.as_ref() else {
            return Err(GpuAccumulate::Declined);
        };
        let uploaded = match renderer.prepare(scene, tools, prep) {
            Ok(inner) => WhittedGpuPrepared { inner },
            Err(error) => {
                self.record_error(&mut current, &error);
                return Err(GpuAccumulate::Declined);
            }
        };
        if cancel() {
            Err(GpuAccumulate::Cancelled)
        } else {
            Ok(uploaded)
        }
    }

    /// Each chunk takes a FIFO turn. A request owns its frozen uploads, so another
    /// request can interleave safely. Commit only after all chunks and cancellation
    /// checks succeed. Limits and accumulator shape are checked on every call.
    ///
    /// # Panics
    ///
    /// Does not panic in practice: every integer conversion it unwraps is a `u32`
    /// value widened to `usize` or a chunk size clamped to `CHUNK_PIXELS`.
    pub fn try_accumulate_prepared_cancellable(
        &self,
        prepared: &WhittedGpuPrepared,
        offset: u32,
        spp: u32,
        accum: &mut [Vec3],
        cancel: &dyn Fn() -> bool,
    ) -> GpuAccumulate {
        if cancel() {
            return GpuAccumulate::Cancelled;
        }
        let pixels = u64::from(prepared.inner.pixels);
        if !self.request_admits(pixels, prepared.inner.taps, offset, spp, accum.len()) {
            return GpuAccumulate::Declined;
        }
        let _ = self.try_recover();
        let chunk = self.configured_limits().map_or(CHUNK_PIXELS, |value| {
            let budget = (value.chunk_budget_bytes / 12).clamp(
                1,
                usize::try_from(CHUNK_PIXELS).expect("chunk size fits usize"),
            );
            u32::try_from(budget).expect("clamped to CHUNK_PIXELS, which is u32")
        });
        // `pixels` came from a `u32` field, so both conversions are exact.
        let total = prepared.inner.pixels;
        let mut sums = vec![Vec3::ZERO; usize::try_from(total).expect("u32 fits usize")];
        let mut first = 0u32;
        while first < total {
            let ticket = self.turnstile.take_ticket();
            let _turn = self.turnstile.wait_for_turn(ticket);
            if cancel() {
                return GpuAccumulate::Cancelled;
            }
            let Ok(mut current) = self.renderer.lock() else {
                return GpuAccumulate::Declined;
            };
            let Some(renderer) = current.as_mut() else {
                return GpuAccumulate::Declined;
            };
            let count = chunk.min(total - first);
            let range = usize::try_from(first).expect("u32 fits usize")
                ..usize::try_from(first + count).expect("u32 fits usize");
            match renderer.trace_chunk(&prepared.inner, first, count, offset, spp) {
                Ok(part) => sums[range].copy_from_slice(&part),
                Err(error) => {
                    self.record_error(&mut current, &error);
                    return GpuAccumulate::Declined;
                }
            }
            first += count;
        }
        if cancel() {
            return GpuAccumulate::Cancelled;
        }
        for (dst, value) in accum.iter_mut().zip(sums) {
            *dst += value;
        }
        GpuAccumulate::Done
    }

    /// Convenience for one-shot consumers. Repeated consumers should retain the
    /// prepared snapshot and call `try_accumulate_prepared_cancellable` instead.
    #[expect(
        clippy::too_many_arguments,
        reason = "one-shot facade mirroring the prepared path's arguments plus the scene triple"
    )]
    pub fn try_accumulate_cancellable(
        &self,
        scene: &FrameScene<'_>,
        tools: &[ToolPrimitive],
        prep: &WhittedPrep<'_>,
        offset: u32,
        spp: u32,
        accum: &mut [Vec3],
        cancel: &dyn Fn() -> bool,
    ) -> GpuAccumulate {
        if cancel() {
            return GpuAccumulate::Cancelled;
        }
        let taps = crate::renderer::engine::RayOptions {
            aa_taps: prep.taps,
            fire: false,
        }
        .sanitized()
        .aa_taps;
        let pixels = u64::from(scene.width) * u64::from(scene.height);
        if !self.request_admits(pixels, u32::from(taps), offset, spp, accum.len()) {
            return GpuAccumulate::Declined;
        }
        match self.prepare_cancellable(scene, tools, prep, cancel) {
            Ok(prepared) => {
                self.try_accumulate_prepared_cancellable(&prepared, offset, spp, accum, cancel)
            }
            Err(outcome) => outcome,
        }
    }
}

#[cfg(test)]
#[path = "whitted_native_tests.rs"]
mod tests;
