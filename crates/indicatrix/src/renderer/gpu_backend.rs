//! The decline-and-fall-back GPU wrapper, shared by every application in this
//! workspace.
//!
//! [`GpuBackend`] wraps [`GpuFrameRenderer`](super::gpu::GpuFrameRenderer) with the one
//! policy every caller needs: try the GPU, fall back to the CPU tracer whenever it
//! declines, and never pretend the GPU ran when it didn't.
//!
//! Lives here rather than duplicated in `apps/indicatrix-cut` and
//! `apps/indicatrix-worker` (as it once was) because the decline reasons below are
//! correctness rules -- a drifted copy would render a plausible-looking wrong image
//! rather than fail.
//!
//! Deliberately NOT `#[cfg(feature = "gpu")]`: [`GpuBackend`] exists in *both*
//! configurations (the real thing with the feature on, a stand-in that always declines
//! with it off), so an application has exactly one call site and no `#[cfg]` of its own.
//!
//! # Declining is normal, and decided per call
//!
//! A decline is not an error: it happens when this build has no `gpu` feature,
//! [`GpuBackend::disabled`] was chosen explicitly, or the machine has no usable adapter.
//! (HDR environment maps and biaxial materials both used to decline; neither does any
//! more -- see finding G6 and `renderer::gpu::frame`'s module doc comment.) Re-made on
//! every call, so a declining scene moves only its own samples to the CPU.
//!
//! # Sample-range additivity
//!
//! [`GpuBackend::try_accumulate`] ADDS into the caller's buffer, exactly as the CPU
//! tracers do. `sample_offset` must be the number of samples already folded into those
//! pixels: both the CPU formula and the GPU shader derive each sample's jitter and RNG
//! from the *absolute* sample index, so reusing an offset redraws identical samples and
//! biases the average instead of extending it. This keeps a GPU worker's samples
//! mergeable with a CPU viewer's.
//!
//! # Concurrency: chunk-level fairness, not per-frame monopoly
//!
//! Acquire once and share. The renderer lives behind a `Mutex` and every method takes
//! `&self`, so a `GpuBackend` can be wrapped in an `Arc` and handed to many threads --
//! `indicatrix-worker`'s `serve` module does exactly this, handing the SAME
//! `Arc<GpuBackend>` to every connection's own thread (see
//! `apps/indicatrix-worker/src/serve/mod.rs::run`).
//!
//! [`GpuBackend::try_accumulate_cancellable`] does NOT hold the renderer lock for a
//! whole request's worth of chunks the way it used to. It instead takes a FAIR, FIFO
//! ticket ([`Turnstile`]) before every [`CHUNKS_PER_TURN`]-chunk "turn" through
//! [`super::gpu::GpuFrameRenderer::accumulate_turn`], releases both the ticket and the
//! renderer lock at the end of that turn, and -- if pixels remain -- rejoins the BACK of
//! the ticket queue for another turn. N concurrent requests therefore interleave
//! chunk-batch by chunk-batch in the order they first asked for GPU time, rather than one
//! request's whole frame running to completion while every other connection's thread
//! blocks behind it. A plain `Mutex` gives no such guarantee -- the OS is free to let one
//! thread relock it repeatedly ahead of others already waiting -- which is the starvation
//! this replaces (concurrency review finding G4, 2026-09-06).
//!
//! ## Why draining before yielding a turn is required
//!
//! `GpuFrameRenderer`'s chunk pipeline is double-buffered and at most ONE chunk deep in
//! flight (see `renderer::gpu::frame`'s own "Overlapped chunk pipeline" doc section).
//! `accumulate_turn` ALWAYS drains that one pending chunk before returning -- whether it
//! reports done, cancelled, or "more work" -- so by the time a turn ends, the renderer's
//! output/staging slots hold no outstanding GPU work. That is what makes handing the SAME
//! renderer to a COMPLETELY DIFFERENT request's next turn safe: there is nothing left in
//! flight for that other request's dispatch to race or corrupt.
//!
//! ## Scene re-upload: the fairness cost
//!
//! `GpuFrameRenderer::scene_buffers` (camera/material/planes/facet-finishes) is ONE
//! persistent set, not one per request -- so when a different request's turn runs
//! between two of THIS request's turns, that request's `accumulate_turn` overwrites
//! those same four buffers with ITS scene. `accumulate_turn` re-verifies/re-uploads them
//! unconditionally at the START of every turn (see that function's own doc comment),
//! which is what makes a resumed request correct regardless of what ran in between. The
//! cost: whenever requests are actually alternating turns, each resumed turn re-uploads a
//! few hundred bytes across four small buffers before dispatching its next chunk --
//! `outputs`/`staging`/`params_buffers` (the large, expensive-to-reallocate per-chunk
//! buffers) are NEVER re-uploaded on a resume, only written into per-chunk exactly as
//! before. A single request with no contention pays this small fixed cost once per
//! [`CHUNKS_PER_TURN`]-chunk turn too (there is no "am I still the only user" fast path)
//! -- the tradeoff this module makes for fairness.
//!
//! ## Chunk sample offsets stay deterministic regardless of interleaving
//!
//! Every sample a chunk dispatches derives its RNG seed and stratified jitter from the
//! ABSOLUTE `(pixel, sample_offset + local_sample)` index alone (see the module doc
//! comment's "Sample-range additivity" section above) -- never from which turn, which
//! chunk index, or which other request ran before or after it. Interleaving two
//! requests' turns on one renderer changes only the WALL-CLOCK ORDER their chunks
//! dispatch in, never `sample_offset`, `pixel_offset`, or any other value a chunk's
//! output depends on -- so a request's accumulated result is bit-identical to running it
//! alone, uncontended, exactly as `run_chunk_equivalence` already proves for the
//! non-interleaved chunk-vs-chunk-count case.
//!
//! ## Desktop viewport and export: no contention to be fair about
//!
//! `apps/indicatrix-cut`'s live viewport (`ViewportGpu`, in
//! `bridge::render_thread::gpu_backend`) and its export worker
//! (`bridge::export_thread::worker::run_export`) each acquire their OWN [`GpuBackend`] --
//! independent adapters, devices, and renderers, never one shared `Arc`. Suspending the
//! viewport's local tracing for the duration of an export (`RenderContext::export_active`)
//! is real, but it is unrelated to this module's fairness model: the two never contend
//! for the same [`Turnstile`] or the same `Mutex<GpuFrameRenderer>` at all, since they
//! are two entirely separate `GpuBackend` instances. The turnstile below only ever
//! arbitrates more than one admission at a time on `indicatrix-worker`, where multiple
//! connection threads genuinely do share one `Arc<GpuBackend>`.

use std::sync::atomic::AtomicBool;

use glam::Vec3;

use crate::{
    geometry::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, FacetFinish},
    },
};

#[cfg(feature = "gpu")]
use super::gpu::GpuFrameError;
#[cfg(feature = "gpu")]
use super::gpu::frame::{ChunkCursor, ChunkTurnOutcome, TurnRequest, classify_material};

/// The scene inputs one GPU accumulation call needs.
///
/// Defined in both build configurations, like [`GpuAccumulate`] below, so callers stay
/// free of `#[cfg]`; the non-`gpu` stand-in just ignores the whole bundle.
#[cfg_attr(not(feature = "gpu"), allow(dead_code))]
pub struct GpuSceneRef<'a> {
    pub camera: &'a Camera,
    pub width: u32,
    pub height: u32,
    pub planes: &'a [GpuFacetPlane],
    /// Per-plane surface finish, indexed in step with `planes`. `&[]` means every facet
    /// is polished -- see `GpuFrameScene::facet_finishes` on how a shorter slice is
    /// padded.
    pub facet_finishes: &'a [FacetFinish],
    pub material: &'a GemMaterial,
    pub max_bounces: u32,
    pub environment: EnvironmentSource<'a>,
}

/// Outcome of [`GpuBackend::try_accumulate_cancellable`].
///
/// Gives a cancelled dispatch its OWN outcome, distinct from "done" and "declined": a
/// decline means fall back to the CPU for every requested sample, while a cancellation
/// means the caller no longer wants this work and usually retries neither.
///
/// Defined in both build configurations (like [`GpuSceneRef`] above).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuAccumulate {
    /// Every requested sample was traced and added into `accum`.
    Done,
    /// The GPU declined this dispatch before tracing anything -- same meaning as
    /// [`GpuBackend::try_accumulate`] returning `false`: `accum` untouched, fall back to
    /// the CPU tracer for the FULL `spp`. See the module doc's "Declining is normal"
    /// section; also covers a permanently lost device (see [`GpuFrameError::DeviceLost`]
    /// and the `lost` field on the `gpu`-gated `GpuBackend`).
    Declined,
    /// `cancel` was observed set before every sample was traced.
    ///
    /// `accum` is GUARANTEED untouched (drain-then-discard, see
    /// `AccumulateOutcome::Cancelled`). Either fall back to the CPU for the full `spp`,
    /// or simply stop, exactly as if this call had never been made.
    Cancelled,
}

/// Finding G5 Part B: which transport kernel a render dispatches every chunk through.
///
/// The megakernel (`spectral_transport.wgsl`'s `transport_main`) or the wavefront
/// pipeline (`wavefront_transport.wgsl`) -- see `renderer::gpu::frame`'s module doc
/// comment ("Wavefront pipeline") for the trade-off and when to prefer which. Both
/// produce bit-identical `out_xyz` for the same scene/samples
/// (`renderer::gpu::transport_check::run_pipeline_equivalence`).
///
/// Defined here (in both build configurations, like [`GpuSceneRef`]/[`GpuAccumulate`]
/// above) rather than in the `gpu`-feature-gated `renderer::gpu::frame`, and re-exported
/// from there as `renderer::gpu::frame::GpuPipelineKind` -- `GpuBackend::set_pipeline_kind`
/// below must accept this type in EITHER build configuration, so the type itself can't
/// live only behind the feature.
///
/// `Default`/`#[default] Megakernel`: [`GpuBackend::set_pipeline_kind`]/
/// [`super::gpu::GpuFrameRenderer::set_pipeline_kind`] are the only way to change it, so
/// no existing caller's behaviour changes until the owner explicitly opts in after
/// measuring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GpuPipelineKind {
    #[default]
    Megakernel,
    Wavefront,
}

/// How many chunks one [`GpuBackend::try_accumulate_cancellable`] "turn" dispatches
/// before releasing the renderer and rejoining the back of the [`Turnstile`] queue.
///
/// `1` would lose the chunk pipeline's overlap entirely: `accumulate_turn`'s dispatch
/// loop only overlaps chunk i's GPU work with chunk i-1's readback (see
/// `renderer::gpu::frame`'s "Overlapped chunk pipeline" doc section), and a turn always
/// drains whatever it dispatches before returning (see this module's doc comment) -- with
/// a one-chunk turn, that "previous" chunk is always ITS OWN, so the CPU would submit and
/// then immediately block on the very dispatch it just queued, exactly the un-pipelined
/// behaviour the double-buffering exists to avoid. `2` keeps that one window of overlap
/// (dispatch chunk 0, dispatch chunk 1 while draining chunk 0, drain chunk 1) inside every
/// turn, while still yielding often enough that a large request can't monopolise the GPU
/// for long -- at [`super::gpu::frame::TARGET_CHUNK_MS`]'s default, at most ~300ms of
/// wall-clock GPU time per turn once the chunk-timing EMA has converged, far less on the
/// first (uncalibrated) turn of a session. See "Scene re-upload: the fairness cost" above
/// for what a larger value would trade against: fewer re-uploads, coarser fairness.
#[cfg(feature = "gpu")]
const CHUNKS_PER_TURN: usize = 2;

/// A FIFO ticket lock.
///
/// [`Self::take_ticket`] hands out ticket numbers in call order; [`Self::wait_for_turn`]
/// blocks the calling thread until its ticket is the one being served, returning an RAII
/// [`TurnstileTurn`] that advances to the next ticket (waking every other waiter) on
/// drop. Unlike a plain `Mutex` -- which makes no ordering promise among blocked waiters,
/// so the OS scheduler can and does let one thread relock it repeatedly ahead of others
/// already queued -- this guarantees admissions happen in the exact order threads asked
/// for one. That is what lets [`GpuBackend::try_accumulate_cancellable`]'s "take a turn,
/// dispatch a few chunks, rejoin the back of the queue" loop alternate FAIRLY among
/// several concurrent requests rather than one starving the rest. Std-only: an
/// `AtomicU64` ticket counter plus a `Mutex`+`Condvar` "now serving" pair, the standard
/// ticket-lock construction -- no new dependency.
#[cfg(feature = "gpu")]
struct Turnstile {
    next_ticket: std::sync::atomic::AtomicU64,
    now_serving: std::sync::Mutex<u64>,
    turn_taken: std::sync::Condvar,
}

#[cfg(feature = "gpu")]
impl Turnstile {
    const fn new() -> Self {
        Self {
            next_ticket: std::sync::atomic::AtomicU64::new(0),
            now_serving: std::sync::Mutex::new(0),
            turn_taken: std::sync::Condvar::new(),
        }
    }

    /// Claims the next ticket, in call order. Cheap and non-blocking: ordering among
    /// callers is decided HERE, at call time, not later when [`Self::wait_for_turn`]
    /// actually blocks -- two threads calling this back-to-back are served in that same
    /// order regardless of how long either later waits.
    fn take_ticket(&self) -> u64 {
        self.next_ticket
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Blocks until `ticket` is being served, then returns a guard whose `Drop` advances
    /// `now_serving` and wakes every waiter -- so a turn is released exactly once, even
    /// if the caller returns early (a `?` on a GPU error, a cancellation), never leaked
    /// and never released twice.
    fn wait_for_turn(&self, ticket: u64) -> TurnstileTurn<'_> {
        let mut serving = self
            .now_serving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *serving != ticket {
            serving = self
                .turn_taken
                .wait(serving)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        drop(serving);
        TurnstileTurn { turnstile: self }
    }
}

/// RAII: holding this value IS holding the [`Turnstile`]'s current turn. `notify_all`
/// (not `notify_one`) on drop -- of the threads waiting on tickets other than the very
/// next one, none can proceed yet, so waking all of them just to have them re-check and
/// go back to sleep is the standard, simple ticket-lock shape; with turns held only for a
/// bounded [`CHUNKS_PER_TURN`]-chunk slice, that wasted wakeup is not worth optimising
/// away with a per-ticket condvar.
#[cfg(feature = "gpu")]
struct TurnstileTurn<'a> {
    turnstile: &'a Turnstile,
}

#[cfg(feature = "gpu")]
impl Drop for TurnstileTurn<'_> {
    fn drop(&mut self) {
        let mut serving = self
            .turnstile
            .now_serving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *serving += 1;
        drop(serving);
        self.turnstile.turn_taken.notify_all();
    }
}

#[cfg(feature = "gpu")]
pub struct GpuBackend {
    renderer: Option<std::sync::Mutex<super::gpu::GpuFrameRenderer>>,
    /// FIFO admission for [`Self::try_accumulate_cancellable`]'s per-turn renderer
    /// access -- see this module's doc comment ("Concurrency: chunk-level fairness").
    /// Exists even when `renderer` is `None` (a `disabled()` backend never consults it,
    /// since every call declines before reaching the turnstile) purely so the struct
    /// needs no `Option` around it.
    turnstile: Turnstile,
    /// Set permanently, once, the first time a dispatch reports
    /// [`GpuFrameError::DeviceLost`] -- the one decline reason that must never be
    /// retried, since every future dispatch against the same device fails identically.
    /// Both [`Self::try_accumulate_cancellable`] and [`Self::adapter_label`] check this
    /// first once true. `AtomicBool` rather than `Mutex<bool>`: read and written
    /// independently of the `renderer` mutex, from any thread sharing one `Arc`.
    lost: AtomicBool,
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
        let renderer = match super::gpu::GpuFrameRenderer::new() {
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
        }
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

    /// Finding G5 Part B: selects which kernel this backend's renderer dispatches
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
    /// is never set, "cancelled" never actually arises here. See the module doc for the
    /// decline reasons.
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
    /// the whole dispatch -- see this module's doc comment ("Concurrency: chunk-level
    /// fairness") for why and for the correctness argument (draining before yielding,
    /// per-turn scene re-upload, deterministic sample offsets) that makes this safe to
    /// interleave with other callers sharing the same `GpuBackend`.
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
        let gpu_scene = super::gpu::GpuFrameScene {
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

            let mut renderer = mutex
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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

/// Stand-in for a build without the `gpu` feature -- see the `gpu`-gated [`GpuBackend`]
/// above for what it stands in for and why it exists at all.
#[cfg(not(feature = "gpu"))]
pub struct GpuBackend;

#[cfg(not(feature = "gpu"))]
impl GpuBackend {
    #[must_use]
    pub const fn acquire() -> Self {
        Self
    }

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
    /// [`GpuBackend::set_pipeline_kind`] above.
    pub const fn set_pipeline_kind(&self, _kind: GpuPipelineKind) {}

    /// Always declines, leaving `accum` untouched.
    ///
    /// Ignores every argument only to stay signature-compatible with the real
    /// `try_accumulate` above -- that identical signature is what keeps callers free of
    /// `#[cfg]`.
    #[allow(
        clippy::unused_self,
        reason = "signature must match the `gpu`-gated GpuBackend::try_accumulate"
    )]
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
    /// [`GpuBackend::try_accumulate_cancellable`] above for what it stands in for.
    /// Ignores `cancel` (and every other argument) for the same reason
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
}

/// [`Turnstile`] is plain `std::sync` -- no GPU, no adapter, no `--features gpu` device
/// needed -- so its fairness property is unit-testable directly, unlike everything else
/// in this module that actually dispatches. Gated on `feature = "gpu"` only because
/// `Turnstile` itself is (see that type's own `#[cfg]`), not because these tests need
/// hardware.
#[cfg(all(test, feature = "gpu"))]
mod tests {
    use super::Turnstile;
    use std::{sync::Mutex, thread};

    /// The fairness property [`Turnstile`] exists for: three threads holding tickets
    /// `t0 < t1 < t2`, asked to wait for their turn in REVERSE ticket order (`t2` and
    /// `t1` call `wait_for_turn` before `t0` does), must still be SERVED in ascending
    /// ticket order -- exactly what lets `GpuBackend::try_accumulate_cancellable`'s
    /// "take a turn, rejoin the back of the queue" loop alternate fairly among several
    /// concurrent requests instead of a plain `Mutex`'s unspecified wakeup order
    /// letting one thread cut back in ahead of others already waiting.
    ///
    /// Deterministic without any `sleep`: `take_ticket` (called by the TEST, not the
    /// worker threads) fixes the serving order up front, and a waiter for ticket `k`
    /// can only proceed once `k` prior turns have each been explicitly dropped -- so the
    /// recorded order reflects ticket order regardless of thread-scheduling timing,
    /// including however early or late each thread actually calls `wait_for_turn`.
    #[test]
    fn turnstile_serves_tickets_in_order_even_when_threads_queue_out_of_order() {
        let turnstile = Turnstile::new();
        let served_order: Mutex<Vec<u64>> = Mutex::new(Vec::new());

        // Tickets are claimed here, on the main thread, in the exact order this test
        // wants served -- take_ticket's own doc comment: ordering is decided at the
        // moment a ticket is taken, not when wait_for_turn later blocks.
        let t0 = turnstile.take_ticket();
        let t1 = turnstile.take_ticket();
        let t2 = turnstile.take_ticket();

        thread::scope(|scope| {
            // Spawned in REVERSE ticket order -- t2's thread calls wait_for_turn before
            // t1's does -- to prove arrival order at the call site has no bearing on
            // service order.
            scope.spawn(|| {
                let _turn = turnstile.wait_for_turn(t2);
                served_order
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(2);
            });
            scope.spawn(|| {
                let _turn = turnstile.wait_for_turn(t1);
                served_order
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(1);
            });
            // The main thread holds ticket 0's turn last and briefly, on purpose: every
            // other waiter is provably still blocked on the condvar right up until this
            // drops, so releasing it is what starts the ascending cascade.
            {
                let _turn0 = turnstile.wait_for_turn(t0);
                served_order
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(0);
            }
        });

        assert_eq!(
            *served_order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![0, 1, 2],
            "tickets must be served in the order they were taken, not the order threads \
             happened to start waiting"
        );
    }

    /// A turn that is dropped without ever completing (mirrors a `?`-propagated GPU
    /// error, or a cancellation, cutting `try_accumulate_cancellable`'s loop short
    /// mid-turn) must still release the turnstile for the next waiter -- the RAII
    /// `Drop` on [`super::TurnstileTurn`] is what this test actually exercises, not any
    /// explicit "release" call site.
    #[test]
    fn turnstile_releases_on_an_early_return_out_of_the_turn_scope() {
        fn takes_a_turn_then_bails_early(turnstile: &Turnstile, ticket: u64) -> Option<()> {
            let _turn = turnstile.wait_for_turn(ticket);
            None // early return -- `_turn` drops here, releasing the turnstile.
        }

        let turnstile = Turnstile::new();
        let t0 = turnstile.take_ticket();
        let t1 = turnstile.take_ticket();

        assert_eq!(takes_a_turn_then_bails_early(&turnstile, t0), None);

        // If the early return above had leaked the turn (never dropped it), this would
        // hang forever waiting for a `now_serving` bump that would never come --
        // `thread::scope` with no spawned threads just runs straight through, so a hang
        // here fails the test the same way any other infinite loop would.
        let _turn1 = turnstile.wait_for_turn(t1);
    }
}
