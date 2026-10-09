//! [`GpuBackend`], the `feature = "gpu"` implementation of the decline-and-fall-back
//! wrapper the parent module's doc comment describes. [`super::stub::GpuBackend`] is the
//! signature-compatible stand-in compiled instead when the feature is off.

use std::{
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use glam::Vec3;

use super::{
    GpuAccumulate, GpuBatchItem, GpuPipelineKind, GpuSceneRef,
    recovery::{COOL_DOWN, RecoveryPolicy},
    turnstile::{CHUNKS_PER_TURN, Turnstile},
};
use crate::renderer::gpu::{
    GpuFrameError, GpuFrameRenderer,
    frame::{
        BatchEvent, BatchState, ChunkCursor, ChunkTurnOutcome, TurnRequest, classify_material,
    },
};

/// The most scratch buffers [`GpuBackend`] keeps between requests. Each is as large as
/// the biggest frame it served (12 bytes a pixel), so the pool is kept small; requests
/// beyond it allocate their own and drop it afterwards.
const MAX_POOLED_SCRATCH: usize = 2;

/// Gpu backend.
pub struct GpuBackend {
    /// `pub(super)`: `super::tests` reaches this private field directly to poison the
    /// mutex from a test thread -- see that module's own comment on why it lives outside
    /// `renderer::gpu::frame`'s hardware test module.
    ///
    /// `Some` exactly when an adapter was acquired at construction. After a loss the
    /// renderer INSIDE the mutex is replaced by [`Self::try_recover`]; the mutex itself
    /// (and so this `Option`) never changes.
    pub(super) renderer: Option<Mutex<GpuFrameRenderer>>,
    /// FIFO admission for [`Self::try_accumulate_cancellable`]'s per-turn renderer
    /// access -- see the parent module's doc comment ("Concurrency: chunk-level
    /// fairness"). Exists even when `renderer` is `None` (a `disabled()` backend never
    /// consults it, since every call declines before reaching the turnstile) purely so
    /// the struct needs no `Option` around it.
    turnstile: Turnstile,
    /// Set the first time a dispatch reports [`GpuFrameError::DeviceLost`] (or finds the
    /// renderer mutex poisoned), and cleared only by a successful [`Self::try_recover`].
    /// While set, every call short-circuits to `Declined` without touching the
    /// renderer: every dispatch against the same device would fail identically.
    /// Both [`Self::try_accumulate_cancellable`] and [`Self::adapter_label`] check this
    /// first. `AtomicBool` rather than `Mutex<bool>`: read and written independently of
    /// the `renderer` mutex, from any thread sharing one `Arc`.
    ///
    /// `pub(super)`: `super::tests` reads this directly to confirm a poisoned mutex sets
    /// it.
    pub(super) lost: AtomicBool,
    /// The human-readable reason [`Self::lost`] was last set `true` for -- either the
    /// `why` text from a [`GpuFrameError::DeviceLost`], or a fixed message for the
    /// poisoned-mutex case. `None` until the first loss; kept (not cleared) after a
    /// recovery so a caller can still log why the previous device went away. Read by
    /// [`Self::last_lost_reason`]. `Mutex`, not an atomic: the payload is a `String`,
    /// and this is set only on the cold "just lost the device" path, so lock contention
    /// is irrelevant.
    last_lost_reason: Mutex<Option<String>>,
    /// Cool-down and attempt budget for [`Self::try_recover`] -- see
    /// [`RecoveryPolicy`]. `pub(super)` so `super::tests` can age a loss without sleeping.
    pub(super) recovery: Mutex<RecoveryPolicy>,
    /// `true` while one thread is inside [`Self::reacquire`], so concurrent callers decline
    /// instead of queueing up behind a slow device acquisition.
    recovering: AtomicBool,
    /// Zeroed accumulation buffers reused across requests: every request traces into one
    /// of these and adds it into the caller's `accum` only when it completes -- see
    /// [`Self::try_accumulate_cancellable`]. A pool rather than one buffer because turns of
    /// concurrent requests interleave, each needing its own partial sums.
    scratch_pool: Mutex<Vec<Vec<Vec3>>>,
    /// Test seam: the 1-based turn number, counted per request, whose outcome is replaced
    /// by an injected [`GpuFrameError::DeviceLost`] (`0` = never). Self-clearing once it
    /// fires.
    #[cfg(test)]
    pub(super) fail_on_turn: std::sync::atomic::AtomicUsize,
}

impl GpuBackend {
    /// Acquires an adapter and compiles the megakernel.
    ///
    /// Do this once, off the frame or request path: both steps are slow enough to
    /// matter. A machine with no usable GPU is an expected outcome, logged at `info`,
    /// after which every call declines.
    #[must_use]
    pub fn acquire() -> Self {
        let renderer = match GpuFrameRenderer::new() {
            Ok(r) => {
                tracing::info!(adapter = r.adapter_label(), "GPU render backend active");
                Some(Mutex::new(r))
            }
            Err(e) => {
                tracing::info!("GPU render backend unavailable, using CPU tracer: {e}");
                None
            }
        };
        Self::with_renderer(renderer)
    }

    /// Never acquires an adapter -- every call declines, regardless of what hardware is
    /// present.
    ///
    /// The runtime opt-out (`indicatrix-worker`'s `--only-cpu` flag), and what a test
    /// constructs instead of [`Self::acquire`] so running with `--features gpu` stays as
    /// deterministic as running without it.
    #[must_use]
    pub const fn disabled() -> Self {
        Self::with_renderer(None)
    }

    /// The one place a backend's fields are initialised.
    const fn with_renderer(renderer: Option<Mutex<GpuFrameRenderer>>) -> Self {
        Self {
            renderer,
            turnstile: Turnstile::new(),
            lost: AtomicBool::new(false),
            last_lost_reason: Mutex::new(None),
            recovery: Mutex::new(RecoveryPolicy::new()),
            recovering: AtomicBool::new(false),
            scratch_pool: Mutex::new(Vec::new()),
            #[cfg(test)]
            fail_on_turn: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Whether this backend currently has no usable GPU device because one was lost --
    /// set by a [`GpuFrameError::DeviceLost`] or a poisoned renderer mutex, cleared when
    /// [`Self::try_recover`] (which the request path calls itself) acquires a fresh
    /// device. Distinct from an ordinary per-call decline (unsupported
    /// material/environment, or [`Self::disabled`]/no adapter at all, none of which set
    /// this): a caller doing self-healing on a `false` return from
    /// [`Self::try_accumulate`] checks this first to decide whether re-acquiring a fresh
    /// backend could possibly help.
    #[must_use]
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }

    /// The reason of the most recent loss, if there has been one -- the `why` text from a
    /// [`GpuFrameError::DeviceLost`], or a fixed message for the poisoned-mutex case.
    /// `None` both before any loss and for a [`Self::disabled`] backend, which never
    /// loses a device it never had. Still returns the last reason after a successful
    /// recovery.
    #[must_use]
    pub fn last_lost_reason(&self) -> Option<String> {
        self.last_lost_reason
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// This backend's adapter and backend label, if one was genuinely acquired.
    ///
    /// `None` whenever [`Self::disabled`] was chosen, acquisition failed, or the device
    /// is currently lost (see [`Self::lost`]) -- a caller must not claim a GPU this
    /// process has no working handle to. Read-only: it never attempts recovery itself, so
    /// a caller about to advertise a capability calls [`Self::try_recover`] first.
    #[must_use]
    pub fn adapter_label(&self) -> Option<String> {
        if self.lost.load(Ordering::Relaxed) {
            return None;
        }
        self.renderer.as_ref().map(|m| {
            m.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .adapter_label()
                .to_string()
        })
    }

    /// Selects which kernel this backend's renderer dispatches
    /// every LATER chunk through -- see [`GpuPipelineKind`]'s own doc comment. A no-op
    /// when this backend never acquired a renderer (see [`Self::disabled`]). The choice
    /// carries over to a renderer re-acquired after a device loss.
    pub fn set_pipeline_kind(&self, kind: GpuPipelineKind) {
        if let Some(mutex) = &self.renderer {
            mutex
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
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

    /// Like [`Self::try_accumulate`], but checks `cancel` before every chunk (see
    /// `GpuFrameRenderer::accumulate_turn` for exactly when) and can stop early,
    /// reporting which of three things happened via [`GpuAccumulate`] rather than a
    /// `bool`: [`GpuAccumulate::Done`], [`GpuAccumulate::Declined`] (`accum` untouched,
    /// fall back to the CPU for the full `spp`), or [`GpuAccumulate::Cancelled`] (`accum`
    /// untouched, nothing left to render).
    ///
    /// `accum` is written ONLY on `Done`. Every turn traces into a zeroed scratch buffer
    /// of the same length that this backend owns and reuses; the scratch is added into
    /// `accum` once the last turn reports done, and discarded otherwise. A device loss,
    /// an uncaptured wgpu error or a cancellation on a LATER turn therefore cannot leave
    /// the samples of earlier turns behind for the caller's CPU fallback to double count.
    /// Each pixel belongs to exactly one chunk, so the final add is one `+=` per pixel --
    /// the same sum a direct accumulation would have produced.
    ///
    /// Drives the renderer in [`CHUNKS_PER_TURN`]-chunk TURNS rather than holding it for
    /// the whole dispatch -- see the parent module's doc comment ("Concurrency:
    /// chunk-level fairness") for why and for the correctness argument (draining before
    /// yielding, per-turn scene re-upload, deterministic sample offsets) that makes this
    /// safe to interleave with other callers sharing the same `GpuBackend`.
    ///
    /// A device reported as lost ([`GpuFrameError::DeviceLost`]) is logged at `warn` (an
    /// ordinary decline is `debug`) and this backend's [`Self::lost`] flag is set so every
    /// later call short-circuits to `Declined` without touching the renderer -- until
    /// [`Self::try_recover`] (called here, at the top of each request, once the cool-down
    /// has passed) brings a fresh device back.
    ///
    /// # Panics
    ///
    /// Panics if `accum.len()` is not `scene.width * scene.height`.
    pub fn try_accumulate_cancellable(
        &self,
        scene: &GpuSceneRef<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        if self.lost.load(Ordering::Acquire) && !self.try_recover() {
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
        // Fixed for the whole request, across however many turns it takes to resume --
        // only the cursor and the scratch buffer change turn by turn. See TurnRequest's
        // own doc comment.
        let request = TurnRequest {
            scene: &gpu_scene,
            pipeline_class,
            sample_offset,
            spp,
            cancel: Some(cancel),
            max_chunks: CHUNKS_PER_TURN,
        };

        let mut scratch = self.take_scratch(accum.len());
        let outcome = self.run_turns(mutex, &request, &mut scratch);
        if outcome == GpuAccumulate::Done {
            for (total, traced) in accum.iter_mut().zip(&scratch) {
                *total += *traced;
            }
        }
        self.return_scratch(scratch);
        outcome
    }

    /// Runs `request` turn by turn, summing into `scratch` (never the caller's buffer --
    /// see [`Self::try_accumulate_cancellable`]), until it completes, is cancelled or
    /// declines. `scratch` holds partial sums after any outcome but `Done`; the caller
    /// discards it.
    fn run_turns(
        &self,
        mutex: &Mutex<GpuFrameRenderer>,
        request: &TurnRequest<'_>,
        scratch: &mut [Vec3],
    ) -> GpuAccumulate {
        let mut cursor = ChunkCursor::default();
        #[cfg(test)]
        let mut turns_started = 0_usize;

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
            if self.lost.load(Ordering::Acquire) {
                return GpuAccumulate::Declined;
            }

            // A poisoned `std::sync::Mutex` (its guard was dropped mid-panic --
            // possible if a wgpu call inside `accumulate_turn` panics despite this
            // module's own `on_uncaptured_error`/poisoning defenses, e.g. a bug in wgpu
            // itself) is never silently recovered via `PoisonError::into_inner`, which
            // would hand the next caller a renderer whose state mid-panic is unknown.
            // Treated the same as `DeviceLost` instead: decline until `try_recover`
            // replaces the renderer wholesale.
            let mut renderer = match mutex.lock() {
                Ok(guard) => guard,
                Err(_poisoned) => {
                    tracing::warn!(
                        "GPU renderer mutex poisoned (a previous turn panicked), disabling the \
                         GPU backend until a fresh device is acquired"
                    );
                    self.mark_lost("GPU renderer mutex poisoned (a previous turn panicked)".into());
                    return GpuAccumulate::Declined;
                }
            };
            let outcome = renderer.accumulate_turn(request, scratch, &mut cursor);
            // Released before `_turn` (below, at end of scope) so a woken waiter's own
            // `mutex.lock()` never has to contend with a guard this turn is done with.
            drop(renderer);
            #[cfg(test)]
            let outcome = {
                turns_started += 1;
                self.injected_failure(turns_started).map_or(outcome, Err)
            };

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
                        "GPU device lost ({why}), disabling the GPU backend until a fresh \
                         device is acquired -- meanwhile every call falls back to the CPU \
                         tracer"
                    );
                    self.mark_lost(why);
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

    /// Traces every item of `items`, keeping the GPU queue fed across picture boundaries,
    /// and calls `on_done(index, outcome)` as soon as each picture finishes -- in input
    /// order, which is also completion order.
    ///
    /// Each item's result is bit-identical to [`Self::try_accumulate_cancellable`] on that
    /// item alone: its samples are summed into a zeroed scratch of its own and added into
    /// `out` only on [`GpuAccumulate::Done`] (a `Declined` or `Cancelled` item leaves its
    /// `out` untouched), with the same absolute sample offsets, the same decline rules per
    /// item (a device loss declines every picture not yet reported; an unsupported
    /// material or environment declines only its own picture) and the same cancel polling
    /// (once per chunk). `cancel` firing reports the first unfinished picture and all
    /// later ones as `Cancelled`; pictures already `Done` stay done. Afterwards the
    /// renderer is idle and reusable.
    ///
    /// The chunk stream, the scene-upload argument and the memory bound are described in
    /// the parent module's "Batches" section. Fairness: the batch holds the renderer
    /// turn while nobody else is queued and yields (draining first) after
    /// [`CHUNKS_PER_TURN`] chunks as soon as another request is waiting, then rejoins the
    /// back of the queue. `on_done` runs while the turn is held, with the next chunk
    /// already queued, so it should be cheap.
    ///
    /// # Panics
    ///
    /// Panics if an item's `out.len()` is not `scene.width * scene.height`.
    pub fn try_accumulate_batch_cancellable(
        &self,
        items: &mut [GpuBatchItem<'_>],
        cancel: &AtomicBool,
        on_done: &mut dyn FnMut(usize, GpuAccumulate),
    ) {
        for item in items.iter() {
            assert_eq!(
                item.out.len(),
                item.scene.width as usize * item.scene.height as usize,
                "batch output buffer must have one entry per pixel"
            );
        }
        let count = items.len();
        let usable = (!self.lost.load(Ordering::Acquire) || self.try_recover())
            .then_some(self.renderer.as_ref())
            .flatten();
        let Some(mutex) = usable else {
            for index in 0..count {
                on_done(index, GpuAccumulate::Declined);
            }
            return;
        };

        let scenes: Vec<crate::renderer::gpu::GpuFrameScene<'_>> = items
            .iter()
            .map(|item| crate::renderer::gpu::GpuFrameScene {
                camera: item.scene.camera,
                width: item.scene.width,
                height: item.scene.height,
                planes: item.scene.planes,
                facet_finishes: item.scene.facet_finishes,
                material: item.scene.material,
                max_bounces: item.scene.max_bounces,
                environment: item.scene.environment,
            })
            .collect();
        let requests: Vec<TurnRequest<'_>> = scenes
            .iter()
            .zip(items.iter())
            .map(|(scene, item)| TurnRequest {
                scene,
                pipeline_class: classify_material(scene.material),
                sample_offset: item.first_sample,
                spp: item.samples,
                cancel: Some(cancel),
                max_chunks: usize::MAX,
            })
            .collect();

        let mut output = BatchOutput {
            items,
            scratches: vec![Vec::new(); count],
            on_done,
            reported: 0,
        };
        self.run_batch(mutex, &requests, &mut output);
        // Normally every picture was reported by now; this only catches a path that ended
        // the stream early, so no `on_done` is ever missing.
        output.finish_rest(self, GpuAccumulate::Declined);
    }

    /// The turn loop behind [`Self::try_accumulate_batch_cancellable`]: takes a ticket,
    /// steps the batch (one chunk per step, renderer lock taken per step so `on_done` never
    /// runs under it), and gives the turn up only when another request is waiting.
    fn run_batch(
        &self,
        mutex: &Mutex<GpuFrameRenderer>,
        requests: &[TurnRequest<'_>],
        output: &mut BatchOutput<'_, '_>,
    ) {
        let mut state = BatchState::new();
        #[cfg(test)]
        let mut steps_started = 0_usize;

        'turns: loop {
            let ticket = self.turnstile.take_ticket();
            let _turn = self.turnstile.wait_for_turn(ticket);

            // Re-checked after waiting, for the same reason as in `run_turns`.
            if self.lost.load(Ordering::Acquire) {
                output.finish_rest(self, GpuAccumulate::Declined);
                return;
            }
            let Some(mut renderer) = self.lock_renderer(mutex) else {
                output.finish_rest(self, GpuAccumulate::Declined);
                return;
            };
            renderer.batch_begin_turn(requests, &mut state);
            drop(renderer);

            let mut chunks_this_turn = 0_usize;
            loop {
                self.provision_scratch(requests, &state, output);
                let Some(mut renderer) = self.lock_renderer(mutex) else {
                    output.finish_rest(self, GpuAccumulate::Declined);
                    return;
                };
                let result = renderer.batch_step(requests, &mut output.scratches, &mut state);
                drop(renderer);
                #[cfg(test)]
                let result = {
                    steps_started += 1;
                    self.injected_failure(steps_started).map_or(result, Err)
                };
                let step = match result {
                    Ok(step) => step,
                    Err(error) => {
                        self.note_error(error);
                        output.finish_rest(self, GpuAccumulate::Declined);
                        return;
                    }
                };
                for event in step.events {
                    output.deliver(self, event);
                }
                if state.is_finished() {
                    return;
                }
                if step.dispatched {
                    chunks_this_turn += 1;
                }
                if chunks_this_turn < CHUNKS_PER_TURN {
                    continue;
                }
                if !self.turnstile.has_waiters_behind(ticket) {
                    // Nobody wants the renderer: keep the queue full, ask again later.
                    chunks_this_turn = 0;
                    continue;
                }
                // Somebody is waiting: drain our one in-flight chunk so the renderer is idle
                // for them, then rejoin the back of the queue.
                let Some(mut renderer) = self.lock_renderer(mutex) else {
                    output.finish_rest(self, GpuAccumulate::Declined);
                    return;
                };
                let drained = renderer.batch_yield(&mut output.scratches, &mut state);
                drop(renderer);
                match drained {
                    Ok(events) => {
                        for event in events {
                            output.deliver(self, event);
                        }
                    }
                    Err(error) => {
                        self.note_error(error);
                        output.finish_rest(self, GpuAccumulate::Declined);
                        return;
                    }
                }
                continue 'turns;
            }
        }
    }

    /// Makes sure the picture the next step will start has its zeroed scratch buffer.
    fn provision_scratch(
        &self,
        requests: &[TurnRequest<'_>],
        state: &BatchState<'_>,
        output: &mut BatchOutput<'_, '_>,
    ) {
        let index = state.next_item();
        let Some(request) = requests.get(index) else {
            return;
        };
        let pixels = request.scene.width as usize * request.scene.height as usize;
        if request.spp > 0 && pixels > 0 && output.scratches[index].len() != pixels {
            output.scratches[index] = self.take_scratch(pixels);
        }
    }

    /// Locks the renderer for one batch step. A poisoned mutex is never silently
    /// recovered (see `run_turns`): it marks the backend lost and yields `None`.
    fn lock_renderer<'m>(
        &self,
        mutex: &'m Mutex<GpuFrameRenderer>,
    ) -> Option<MutexGuard<'m, GpuFrameRenderer>> {
        let Ok(guard) = mutex.lock() else {
            tracing::warn!(
                "GPU renderer mutex poisoned (a previous turn panicked), disabling the GPU \
                 backend until a fresh device is acquired"
            );
            self.mark_lost("GPU renderer mutex poisoned (a previous turn panicked)".into());
            return None;
        };
        Some(guard)
    }

    /// Logs a batch step's failure like `run_turns` does: a lost device marks the backend
    /// lost, anything else is an ordinary decline.
    fn note_error(&self, error: GpuFrameError) {
        match error {
            GpuFrameError::DeviceLost(why) => {
                tracing::warn!(
                    "GPU device lost ({why}), disabling the GPU backend until a fresh \
                     device is acquired -- meanwhile every call falls back to the CPU \
                     tracer"
                );
                self.mark_lost(why);
            }
            other => {
                tracing::debug!("GPU declined this batch, using CPU tracer: {other}");
            }
        }
    }

    /// Records a device loss: remembers `reason`, starts the recovery cool-down, and
    /// only then raises [`Self::lost`], so a thread that sees the flag also finds the
    /// cool-down already running.
    fn mark_lost(&self, reason: String) {
        *self
            .last_lost_reason
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(reason);
        self.policy().record_loss(Instant::now());
        self.lost.store(true, Ordering::Release);
    }

    fn policy(&self) -> MutexGuard<'_, RecoveryPolicy> {
        self.recovery.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Tries to bring a lost GPU back, subject to the recovery policy: at least
    /// [`COOL_DOWN`] since the loss or the previous attempt, and at most six attempts
    /// per hour. Returns `true` when the backend is usable on return -- it was never lost,
    /// another thread already recovered it, or a fresh device was just acquired --
    /// and `false` when it is still lost (no attempt allowed yet, another thread is
    /// mid-attempt, or the attempt failed).
    ///
    /// [`Self::try_accumulate_cancellable`] calls this itself at the top of every request
    /// while lost; a caller that must report the backend's state first (a joined worker
    /// building its `HELLO`) calls it directly. Never blocks behind another recovery: the
    /// losing thread just declines.
    #[must_use]
    pub fn try_recover(&self) -> bool {
        if !self.lost.load(Ordering::Acquire) {
            return true;
        }
        let Some(slot) = &self.renderer else {
            return false;
        };
        if !self.policy().may_attempt(Instant::now()) {
            return false;
        }
        if self.recovering.swap(true, Ordering::AcqRel) {
            return false;
        }
        let recovered = self.reacquire(slot);
        self.recovering.store(false, Ordering::Release);
        recovered
    }

    /// Acquires a fresh renderer and swaps it into `slot`. The slow part (adapter,
    /// device, megakernel compile) runs before any lock is taken; the swap itself then
    /// waits for a turnstile turn like a dispatch would, so it never lands inside
    /// another request's turn. A poisoned mutex is cleared by the swap.
    fn reacquire(&self, slot: &Mutex<GpuFrameRenderer>) -> bool {
        if !self.lost.load(Ordering::Acquire) {
            return true;
        }
        self.policy().record_attempt(Instant::now());
        let reason = self.last_lost_reason().unwrap_or_default();
        tracing::info!("GPU device was lost ({reason}); trying to acquire a fresh one");
        let mut fresh = match GpuFrameRenderer::new() {
            Ok(fresh) => fresh,
            Err(e) => {
                tracing::info!(
                    "GPU re-acquisition failed, staying on the CPU tracer for at least {} s: {e}",
                    COOL_DOWN.as_secs()
                );
                return false;
            }
        };

        let ticket = self.turnstile.take_ticket();
        let _turn = self.turnstile.wait_for_turn(ticket);
        let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
        fresh.set_pipeline_kind(guard.pipeline_kind());
        *guard = fresh;
        drop(guard);
        slot.clear_poison();
        self.policy().record_recovered();
        self.lost.store(false, Ordering::Release);
        let adapter = self
            .adapter_label()
            .unwrap_or_else(|| "unknown".to_string());
        tracing::info!(adapter = adapter.as_str(), "GPU render backend recovered");
        true
    }

    /// A zeroed buffer of `len` pixels from the pool (or a new one).
    fn take_scratch(&self, len: usize) -> Vec<Vec3> {
        let mut buffer = self
            .scratch_pool
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop()
            .unwrap_or_default();
        buffer.clear();
        buffer.resize(len, Vec3::ZERO);
        buffer
    }

    /// Hands `buffer` back for the next request, unless the pool is already full.
    fn return_scratch(&self, buffer: Vec<Vec3>) {
        if buffer.capacity() == 0 {
            return;
        }
        let mut pool = self
            .scratch_pool
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if pool.len() < MAX_POOLED_SCRATCH {
            pool.push(buffer);
        }
    }

    /// The test seam behind [`Self::fail_on_turn`]: an injected loss exactly when `turn`
    /// is the armed turn number, disarming it.
    #[cfg(test)]
    fn injected_failure(&self, turn: usize) -> Option<GpuFrameError> {
        self.fail_on_turn
            .compare_exchange(turn, 0, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
            .then(|| GpuFrameError::DeviceLost("injected failure (test seam)".to_string()))
    }
}

/// The caller-facing half of one batch run: the output buffers, the per-picture scratch
/// buffers and the `on_done` sink, with the bookkeeping that every picture is reported
/// exactly once and in input order.
struct BatchOutput<'b, 'a> {
    items: &'b mut [GpuBatchItem<'a>],
    /// One scratch per picture: empty until the picture starts, taken back when it is
    /// reported.
    scratches: Vec<Vec<Vec3>>,
    on_done: &'b mut dyn FnMut(usize, GpuAccumulate),
    /// Number of pictures already reported (the next one to report has this index).
    reported: usize,
}

impl BatchOutput<'_, '_> {
    /// Reports picture `index`: on `Done` its scratch is added into the caller's buffer
    /// first (the single path's final `+=`), then the scratch goes back to the pool.
    fn report(&mut self, backend: &GpuBackend, index: usize, outcome: GpuAccumulate) {
        debug_assert_eq!(
            index, self.reported,
            "batch outcomes must be reported in input order"
        );
        let scratch = std::mem::take(&mut self.scratches[index]);
        if outcome == GpuAccumulate::Done {
            for (total, traced) in self.items[index].out.iter_mut().zip(&scratch) {
                *total += *traced;
            }
        }
        backend.return_scratch(scratch);
        self.reported = index + 1;
        (self.on_done)(index, outcome);
    }

    /// Reports what the renderer's stepper concluded.
    fn deliver(&mut self, backend: &GpuBackend, event: BatchEvent) {
        match event {
            BatchEvent::Done(index) => self.report(backend, index, GpuAccumulate::Done),
            BatchEvent::Declined(index) => self.report(backend, index, GpuAccumulate::Declined),
            BatchEvent::Cancelled(first) => {
                for index in first..self.items.len() {
                    self.report(backend, index, GpuAccumulate::Cancelled);
                }
            }
        }
    }

    /// Reports every picture not reported yet with `outcome`.
    fn finish_rest(&mut self, backend: &GpuBackend, outcome: GpuAccumulate) {
        for index in self.reported..self.items.len() {
            self.report(backend, index, outcome);
        }
    }
}
