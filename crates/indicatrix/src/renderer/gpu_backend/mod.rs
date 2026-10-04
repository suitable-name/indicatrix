//! The decline-and-fall-back GPU wrapper, shared by every application in this
//! workspace.
//!
//! [`GpuBackend`] wraps [`GpuFrameRenderer`](super::gpu::GpuFrameRenderer) with the one
//! policy every caller needs: try the GPU, fall back to the CPU tracer whenever it
//! declines, and never pretend the GPU ran when it didn't.
//!
//! Lives here, shared by `apps/indicatrix-cut` and `apps/indicatrix-worker`, because the
//! decline reasons below are correctness rules -- a drifted copy would render a
//! plausible-looking wrong image rather than fail.
//!
//! Deliberately NOT `#[cfg(feature = "gpu")]`: [`GpuBackend`] exists in *both*
//! configurations (the real thing with the feature on, a stand-in that always declines
//! with it off), so an application has exactly one call site and no `#[cfg]` of its own.
//!
//! # Declining is normal, and decided per call
//!
//! A decline is not an error: it happens when this build has no `gpu` feature,
//! [`GpuBackend::disabled`] was chosen explicitly, or the machine has no usable adapter.
//! (Which environment/material kinds the GPU can dispatch is its own evolving list --
//! see `renderer::gpu::frame`'s module doc comment for the current picture.) Re-made on
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
//! whole request's worth of chunks. It instead takes a FAIR, FIFO
//! ticket ([`turnstile::Turnstile`]) before every [`turnstile::CHUNKS_PER_TURN`]-chunk
//! "turn" through [`super::gpu::GpuFrameRenderer::accumulate_turn`], releases both the
//! ticket and the renderer lock at the end of that turn, and -- if pixels remain --
//! rejoins the BACK of the ticket queue for another turn. N concurrent requests
//! therefore interleave chunk-batch by chunk-batch in the order they first asked for GPU
//! time, rather than one request's whole frame running to completion while every other
//! connection's thread blocks behind it. A plain `Mutex` gives no such guarantee -- the
//! OS is free to let one thread relock it repeatedly ahead of others already waiting --
//! which is the starvation this design avoids.
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
//! [`turnstile::CHUNKS_PER_TURN`]-chunk turn too (there is no "am I still the only user"
//! fast path) -- the tradeoff this module makes for fairness.
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
//! ## `accum` is written only on `Done`
//!
//! Turns run against a backend-owned scratch buffer (zeroed per request, pooled across
//! requests), and [`GpuBackend::try_accumulate_cancellable`] adds it into the caller's
//! `accum` once the last turn completes. A decline or cancellation after some turns
//! already ran (a device lost mid-request, an uncaptured wgpu error caught at the end of a
//! turn, a cancel between turns) discards the scratch instead, so a caller that falls back
//! to the CPU for the full `spp` never double counts the samples the GPU had finished.
//! Cost: one zero-fill and one add pass over the frame per request.
//!
//! ## Desktop viewport and export: no contention to be fair about
//!
//! `apps/indicatrix-cut`'s live viewport (`ViewportGpu`, in
//! `bridge::render_thread::gpu_backend`) and its export worker
//! (`bridge::export_thread::worker::run_export`) each acquire their OWN [`GpuBackend`] --
//! independent adapters, devices, and renderers, never one shared `Arc`. Suspending the
//! viewport's local tracing for the duration of an export (`RenderContext::export_active`)
//! is real, but it is unrelated to this module's fairness model: the two never contend
//! for the same [`turnstile::Turnstile`] or the same `Mutex<GpuFrameRenderer>` at all,
//! since they are two entirely separate `GpuBackend` instances.
//!
//! The turnstile below is not specific to `indicatrix-worker`: `apps/indicatrix-cut`'s
//! BATCH preview (`gui::batch::preview::wiring`) acquires
//! ONE [`GpuBackend`] for the whole batch job and spawns `local_lane_count()` worker
//! lanes over that SAME `Arc<GpuBackend>`, exactly the multi-admission shape this
//! module's fairness turnstile exists for. `indicatrix-worker`'s `serve` module (handing
//! one `Arc<GpuBackend>` to every connection's own thread) is simply the OTHER caller
//! that shares one backend across threads, not the only one.
//!
//! # Device loss and recovery
//!
//! A [`GpuFrameError::DeviceLost`](super::gpu::GpuFrameError::DeviceLost) (or a renderer
//! mutex poisoned by a panic) is a decline like any other for the request that hits it,
//! and additionally marks the backend lost: [`GpuBackend::is_lost`] turns `true`,
//! [`GpuBackend::adapter_label`] returns `None`, and every call declines without touching
//! the renderer. It is not permanent. Once 30 s have passed since the loss (or since the
//! previous failed attempt), the next request re-acquires an adapter and compiles a fresh
//! renderer; on success the lost flag clears and the GPU serves again, with the pipeline
//! kind carried over. At most 6 attempts start in any hour, so a device that keeps
//! failing is left alone (the policy lives in [`recovery`]). Only a backend that HAD a
//! renderer can recover: one that found no adapter at start-up stays declining, as does
//! [`GpuBackend::disabled`]. A caller that advertises the backend (a worker's `HELLO`)
//! calls [`GpuBackend::try_recover`] first so it reports the current state.
//!
//! # Module layout
//!
//! [`turnstile`] is the FIFO ticket lock ([`turnstile::Turnstile`]/its RAII turn guard)
//! `feature = "gpu"`'s [`backend::GpuBackend`] drives it through; the non-`gpu` build
//! instead compiles [`stub::GpuBackend`], a signature-compatible always-decline stand-in.
//! [`GpuSceneRef`]/[`GpuAccumulate`]/[`GpuPipelineKind`] below are defined in both
//! configurations so a caller needs no `#[cfg]` of its own.

use crate::{
    geometry::{GpuFacetPlane, tool::StoneGeometry},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, FacetFinish, LightingPreset},
    },
};

#[cfg(feature = "gpu")]
mod backend;
#[cfg(feature = "gpu")]
mod recovery;
#[cfg(not(feature = "gpu"))]
mod stub;
#[cfg(all(test, feature = "gpu"))]
mod tests;
#[cfg(feature = "gpu")]
mod turnstile;

#[cfg(feature = "gpu")]
pub use backend::GpuBackend;
#[cfg(not(feature = "gpu"))]
pub use stub::GpuBackend;

/// The scene inputs one GPU accumulation call needs.
///
/// Defined in both build configurations, like [`GpuAccumulate`] below, so callers stay
/// free of `#[cfg]`; the non-`gpu` stand-in just ignores the whole bundle.
#[cfg_attr(not(feature = "gpu"), allow(dead_code))]
pub struct GpuSceneRef<'a> {
    /// Camera the frame is rendered from.
    pub camera: &'a Camera,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Facet planes of the stone.
    pub planes: &'a [GpuFacetPlane],
    /// Per-plane surface finish, indexed in step with `planes`. `&[]` means every facet
    /// is polished -- see `GpuFrameScene::facet_finishes` on how a shorter slice is
    /// padded.
    pub facet_finishes: &'a [FacetFinish],
    /// Gem material.
    pub material: &'a GemMaterial,
    /// Maximum number of internal bounces per path.
    pub max_bounces: u32,
    /// Lighting environment.
    pub environment: EnvironmentSource<'a>,
}

/// Whether a scene may be dispatched to the GPU at all.
///
/// The material must be one the GPU supports, the stone must be convex (no tools), the
/// material must not fluoresce (`fluorescence` empty) and the lighting must not be a UV
/// lamp.
///
/// Fluorescence is single-wavelength CPU transport (`optics::fluorescence`), and the UV
/// lamps' spectra and the excitation paths below 380 nm exist only on the CPU, so such
/// a scene is traced entirely by the CPU, like a concave one. `lighting` is the analytic
/// lighting preset of the scene (`EnvironmentSource::lighting_preset`; for an HDR
/// panorama pass the preset's default, as no lamp is in play).
///
/// The WGSL intersection, NEE probes and edge-rounding know nothing about tools, and
/// changing them needs an adapter to verify (see `docs/gpu.md`). Until that lands a
/// concave stone must take the CPU tracer, which is the shipped behaviour for it; a
/// caller that gets `false` renders the whole frame with
/// [`crate::renderer::cpu_frame::trace_pixels_interleaved_geom`], i.e. a
/// `HybridSplit::cpu_only` split. A pure function of the scene so it is unit-tested
/// without an adapter.
#[must_use]
pub const fn scene_routes_to_gpu(
    material: &GemMaterial,
    geom: StoneGeometry<'_>,
    fluorescence: &Fluorescence,
    lighting: LightingPreset,
) -> bool {
    material.gpu_supported()
        && geom.is_convex()
        && fluorescence.is_empty()
        && !lighting.is_uv_lamp()
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
    /// The GPU did not produce this request -- same meaning as
    /// [`GpuBackend::try_accumulate`] returning `false`: `accum` untouched, fall back to
    /// the CPU tracer for the FULL `spp`. See the module doc's "Declining is normal"
    /// section; also covers a lost device (see `GpuFrameError::DeviceLost`, the `lost`
    /// field on the `gpu`-gated `GpuBackend`, and the module doc's "Device loss and
    /// recovery" section). The guarantee holds however late the decline happens: a
    /// failure on the last turn of a many-turn request leaves `accum` exactly as it was
    /// passed in.
    Declined,
    /// `cancel` was observed set before every sample was traced.
    ///
    /// `accum` is untouched, however many turns had already run. Either fall back to the
    /// CPU for the full `spp`, or simply stop, exactly as if this call had never been
    /// made.
    Cancelled,
}

/// Which transport kernel a render dispatches every chunk through.
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
    /// One monolithic kernel traces each path to completion.
    #[default]
    Megakernel,
    /// Separate kernels advance all paths one bounce at a time.
    Wavefront,
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    use crate::{
        geometry::{cuts::StandardGemCuts, tool::ToolPrimitive},
        optics::{
            absorption::AbsorptionBand,
            fluorescence::{EmissionBand, FluorescentEmitter},
        },
    };
    use glam::Vec3;

    #[test]
    fn scene_routes_to_gpu_is_false_whenever_tools_are_present() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let tools = [ToolPrimitive::ball(Vec3::new(0.0, 0.3, 0.0), 0.1)];
        let material = GemMaterial::diamond();
        assert!(
            material.gpu_supported(),
            "test premise: a supported material"
        );
        let none = Fluorescence::none();
        let daylight = LightingPreset::Daylight;
        assert!(scene_routes_to_gpu(
            &material,
            StoneGeometry::planes_only(&planes),
            none,
            daylight
        ));
        assert!(!scene_routes_to_gpu(
            &material,
            StoneGeometry {
                planes: &planes,
                tools: &tools,
            },
            none,
            daylight
        ));
    }

    #[test]
    fn scene_routes_to_gpu_is_false_for_fluorescence_or_a_uv_lamp() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let stone = StoneGeometry::planes_only(&planes);
        let material = GemMaterial::diamond();
        let emitter = FluorescentEmitter {
            excitation: vec![AbsorptionBand::new(410.0, 20.0, 1.0)],
            emission: vec![EmissionBand::new(694.0, 2.0, 1.0)],
            quantum_yield: 0.9,
        };
        let fluorescent = Fluorescence::new(vec![emitter]);
        // Unchanged for every existing preset with no fluorescence.
        for preset in LightingPreset::ALL {
            assert_eq!(
                scene_routes_to_gpu(&material, stone, Fluorescence::none(), preset),
                !preset.is_uv_lamp(),
                "{preset:?}"
            );
        }
        // A non-empty fluorescence routes to the CPU under any lighting.
        for preset in LightingPreset::ALL {
            assert!(!scene_routes_to_gpu(&material, stone, &fluorescent, preset));
        }
    }
}
