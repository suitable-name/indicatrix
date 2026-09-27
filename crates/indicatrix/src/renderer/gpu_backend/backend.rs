//! [`GpuBackend`], the `feature = "gpu"` implementation of the decline-and-fall-back
//! wrapper the parent module's doc comment describes. [`super::stub::GpuBackend`] is the
//! signature-compatible stand-in compiled instead when the feature is off.

use std::sync::atomic::AtomicBool;

use glam::Vec3;

use super::{
    GpuAccumulate, GpuPipelineKind, GpuSceneRef,
    turnstile::{CHUNKS_PER_TURN, Turnstile},
};
use crate::renderer::gpu::{
    GpuFrameError,
    frame::{ChunkCursor, ChunkTurnOutcome, TurnRequest, classify_material},
};

pub struct GpuBackend {
    /// `pub(super)`: `super::tests` reaches this private field directly to poison the
    /// mutex from a test thread -- see that module's own comment on why it lives outside
    /// `renderer::gpu::frame`'s hardware test module.
    pub(super) renderer: Option<std::sync::Mutex<crate::renderer::gpu::GpuFrameRenderer>>,
    /// FIFO admission for [`Self::try_accumulate_cancellable`]'s per-turn renderer
    /// access -- see the parent module's doc comment ("Concurrency: chunk-level
    /// fairness"). Exists even when `renderer` is `None` (a `disabled()` backend never
    /// consults it, since every call declines before reaching the turnstile) purely so
    /// the struct needs no `Option` around it.
    turnstile: Turnstile,
    /// Set permanently, once, the first time a dispatch reports
    /// [`GpuFrameError::DeviceLost`] -- the one decline reason that must never be
    /// retried, since every future dispatch against the same device fails identically.
    /// Both [`Self::try_accumulate_cancellable`] and [`Self::adapter_label`] check this
    /// first once true. `AtomicBool` rather than `Mutex<bool>`: read and written
    /// independently of the `renderer` mutex, from any thread sharing one `Arc`.
    ///
    /// `pub(super)`: `super::tests` reads this directly to confirm a poisoned mutex sets
    /// it.
    pub(super) lost: AtomicBool,
    /// The human-readable reason [`Self::lost`] was last set `true` for -- either the
    /// `why` text from a [`GpuFrameError::DeviceLost`], or a fixed message for the
    /// poisoned-mutex case. `None` until the first loss. Read by [`Self::last_lost_reason`]
    /// so a caller doing its OWN self-healing (drop this backend, construct a fresh one
    /// via [`Self::acquire`]) can log or display WHY, not just that it happened -- see
    /// `apps::indicatrix_cut::bridge::render_thread::gpu_backend::ViewportGpu` for that
    /// caller. `Mutex`, not an atomic: the payload is a `String`, and this is set only on
    /// the cold "just lost the device" path, so lock contention is irrelevant.
    last_lost_reason: std::sync::Mutex<Option<String>>,
}

impl GpuBackend {
    /// Acquires an adapter and compiles the megakernel.
    ///
    /// Do this once, off the frame or request path: both steps are slow enough to
    /// matter. A machine with no usable GPU is an expected outcome, logged at `info`,
    /// after which every call declines.
    #[must_use]
    pub fn acquire() -> Self {
        let renderer = match crate::renderer::gpu::GpuFrameRenderer::new() {
            Ok(r) => {
                tracing::info!(adapter = r.adapter_label(), "GPU render backend active");
                Some(std::sync::Mutex::new(r))
            }
            Err(e) => {
                tracing::info!("GPU render backend unavailable, using CPU tracer: {e}");
                None
            }
        };
        Self {
            renderer,
            turnstile: Turnstile::new(),
            lost: AtomicBool::new(false),
            last_lost_reason: std::sync::Mutex::new(None),
        }
    }

    /// Never acquires an adapter -- every call declines, regardless of what hardware is
    /// present.
    ///
    /// The runtime opt-out (`indicatrix-worker`'s `--only-cpu` flag), and what a test
    /// constructs instead of [`Self::acquire`] so running with `--features gpu` stays as
    /// deterministic as running without it.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            renderer: None,
            turnstile: Turnstile::new(),
            lost: AtomicBool::new(false),
            last_lost_reason: std::sync::Mutex::new(None),
        }
    }

    /// Whether this backend has permanently given up on its GPU device -- set once by a
    /// [`GpuFrameError::DeviceLost`] or a poisoned renderer mutex, never cleared (see
    /// [`Self::lost`]'s own doc comment). Distinct from an ordinary per-call decline
    /// (unsupported material/environment, or [`Self::disabled`]/no adapter at all, none
    /// of which set this): a caller doing self-healing on a `false` return from
    /// [`Self::try_accumulate`] checks this first to decide whether re-acquiring a fresh
    /// backend could possibly help.
    #[must_use]
    pub fn is_lost(&self) -> bool {
        self.lost.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The reason [`Self::is_lost`] became `true`, if it has -- the `why` text from a
    /// [`GpuFrameError::DeviceLost`], or a fixed message for the poisoned-mutex case.
    /// `None` both before any loss and for a [`Self::disabled`] backend, which never
    /// loses a device it never had.
    #[must_use]
    pub fn last_lost_reason(&self) -> Option<String> {
        self.last_lost_reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// This backend's adapter and backend label, if one was genuinely acquired.
    ///
    /// `None` whenever [`Self::disabled`] was chosen, acquisition failed, or the device
    /// was later found to be lost (see [`Self::lost`]) -- a caller must not keep
    /// claiming a GPU this process already gave up on.
    #[must_use]
    pub fn adapter_label(&self) -> Option<String> {
        if self.lost.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        self.renderer.as_ref().map(|m| {
            m.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .adapter_label()
                .to_string()
        })
    }

    /// Selects which kernel this backend's renderer dispatches
    /// every LATER chunk through -- see [`GpuPipelineKind`]'s own doc comment. A no-op
    /// when this backend never acquired a renderer (see [`Self::disabled`]).
    pub fn set_pipeline_kind(&self, kind: GpuPipelineKind) {
        if let Some(mutex) = &self.renderer {
            mutex
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_pipeline_kind(kind);
        }
    }

    /// Traces `spp` samples per pixel starting at `sample_offset` and ADDS each pixel's
    /// summed XYZ into `accum`.
    ///
    /// A thin wrapper over [`Self::try_accumulate_cancellable`] with a cancellation flag
    /// that is never set, for callers with no cancellation concept of their own. Returns
    /// `false` without touching `accum` for either non-`Done` outcome -- since the flag
    /// is never set, "cancelled" never actually arises here. See the parent module's doc
    /// comment for the decline reasons.
    pub fn try_accumulate(
        &self,
        scene: &GpuSceneRef<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> bool {
        let never_cancel = AtomicBool::new(false);
        matches!(
            self.try_accumulate_cancellable(scene, sample_offset, spp, accum, &never_cancel),
            GpuAccumulate::Done
        )
    }

    /// Like [`Self::try_accumulate`], but checks `cancel` between chunks (see
    /// `GpuFrameRenderer::accumulate_turn` for exactly when) and can stop early,
    /// reporting which of three things happened via [`GpuAccumulate`] rather than a
    /// `bool`: [`GpuAccumulate::Done`], [`GpuAccumulate::Declined`] (`accum` untouched,
    /// fall back to the CPU for the full `spp`), or [`GpuAccumulate::Cancelled`] (`accum`
    /// GUARANTEED untouched, nothing left to render).
    ///
    /// Drives the renderer in [`CHUNKS_PER_TURN`]-chunk TURNS rather than holding it for
    /// the whole dispatch -- see the parent module's doc comment ("Concurrency:
    /// chunk-level fairness") for why and for the correctness argument (draining before
    /// yielding, per-turn scene re-upload, deterministic sample offsets) that makes this
    /// safe to interleave with other callers sharing the same `GpuBackend`.
    ///
    /// A device reported as permanently lost ([`GpuFrameError::DeviceLost`]) is logged at
    /// `warn` (an ordinary decline is `debug`) and this backend's [`Self::lost`] flag is
    /// set so every later call short-circuits to `Declined` without touching the
    /// renderer again.
    pub fn try_accumulate_cancellable(
        &self,
        scene: &GpuSceneRef<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        if self.lost.load(std::sync::atomic::Ordering::Relaxed) {
            return GpuAccumulate::Declined;
        }
        let Some(mutex) = &self.renderer else {
            return GpuAccumulate::Declined;
        };
        let gpu_scene = crate::renderer::gpu::GpuFrameScene {
            camera: scene.camera,
            width: scene.width,
            height: scene.height,
            planes: scene.planes,
            facet_finishes: scene.facet_finishes,
            material: scene.material,
            max_bounces: scene.max_bounces,
            environment: scene.environment,
        };
        // classify_material MIRRORS what GpuFrameRenderer::accumulate_cancellable would
        // derive internally -- see that function's own doc comment on why this is the
        // one place that decision is made. Computed once, outside the turn loop: the
        // scene (and so its material class) never changes across this one request's
        // turns.
        let pipeline_class = classify_material(gpu_scene.material);
        let mut cursor = ChunkCursor::default();
        // Fixed for the whole request, across however many turns it takes to resume --
        // only `cursor` (above) and `accum` change turn by turn. See TurnRequest's own
        // doc comment.
        let request = TurnRequest {
            scene: &gpu_scene,
            pipeline_class,
            sample_offset,
            spp,
            cancel: Some(cancel),
            max_chunks: CHUNKS_PER_TURN,
        };

        loop {
            // Admission is FIFO across every caller sharing this GpuBackend: a ticket is
            // taken here, at the moment this turn is ready to start, so a request that
            // rejoins the queue after finishing a turn goes to the BACK of it -- see
            // Turnstile's own doc comment.
            let ticket = self.turnstile.take_ticket();
            let _turn = self.turnstile.wait_for_turn(ticket);

            // Re-checked AFTER waiting, not just once before the loop above --
            // this call may have blocked in `wait_for_turn` behind an admission whose OWN
            // turn just discovered `DeviceLost` and set `self.lost` (the `Err(DeviceLost)`
            // arm below), in which case dispatching into this renderer now would let an
            // already-queued caller submit into a poisoned renderer -- checked here,
            // before `mutex.lock()`, so this waiter declines instead.
            if self.lost.load(std::sync::atomic::Ordering::Relaxed) {
                return GpuAccumulate::Declined;
            }

            // A poisoned `std::sync::Mutex` (its guard was dropped mid-panic --
            // possible if a wgpu call inside `accumulate_turn` panics despite this
            // module's own `on_uncaptured_error`/poisoning defenses, e.g. a bug in wgpu
            // itself) is never silently recovered via `PoisonError::into_inner`, which
            // would hand the next caller a renderer whose state mid-panic is unknown.
            // Treated the same as `DeviceLost` instead: permanently decline rather than
            // guess that whatever the panicking call left behind is still safe to
            // dispatch into.
            let mut renderer = match mutex.lock() {
                Ok(guard) => guard,
                Err(_poisoned) => {
                    tracing::warn!(
                        "GPU renderer mutex poisoned (a previous turn panicked), permanently \
                         disabling the GPU backend for the rest of this process"
                    );
                    *self
                        .last_lost_reason
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some("GPU renderer mutex poisoned (a previous turn panicked)".to_string());
                    self.lost.store(true, std::sync::atomic::Ordering::Relaxed);
                    return GpuAccumulate::Declined;
                }
            };
            let outcome = renderer.accumulate_turn(&request, accum, &mut cursor);
            // Released before `_turn` (below, at end of scope) so a woken waiter's own
            // `mutex.lock()` never has to contend with a guard this turn is done with.
            drop(renderer);

            match outcome {
                Ok(ChunkTurnOutcome::Done) => return GpuAccumulate::Done,
                Ok(ChunkTurnOutcome::Cancelled) => return GpuAccumulate::Cancelled,
                // This turn's chunk budget is spent but pixels remain -- `_turn` drops
                // at the bottom of this loop body, releasing the turnstile and letting
                // any other waiter in FIFO order (including this same request rejoining
                // at the back) take the next admission.
                Ok(ChunkTurnOutcome::MoreWork) => {}
                Err(GpuFrameError::DeviceLost(why)) => {
                    tracing::warn!(
                        "GPU device lost ({why}), permanently disabling the GPU backend for \
                         the rest of this process -- every later call falls back to the CPU \
                         tracer"
                    );
                    *self
                        .last_lost_reason
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(why);
                    self.lost.store(true, std::sync::atomic::Ordering::Relaxed);
                    return GpuAccumulate::Declined;
                }
                Err(e) => {
                    // Not worth `warn`: an unsupported material or environment is a
                    // documented limit of the megakernel, not a malfunction, and the CPU
                    // path produces the image either way.
                    tracing::debug!("GPU declined this dispatch, using CPU tracer: {e}");
                    return GpuAccumulate::Declined;
                }
            }
        }
    }
}
