//! Bit-exactness self-tests for this module tree: [`run_chunk_equivalence`] (chunked vs.
//! single-dispatch), [`run_pipeline_equivalence`]/[`run_wavefront_determinism`]
//! (megakernel vs. wavefront, and the wavefront pipeline against itself), and
//! [`run_specialisation_equivalence`] (GPU dispatch determinism for each specialised
//! material-class pipeline). Every function here needs a real `wgpu` adapter (via the
//! `renderer` the caller passes in) but is otherwise plain library code, not `#[cfg(test)]`
//! -- `examples/gpu_equivalence_harness.rs` is what actually runs them.

use glam::Vec3;

use crate::optics::{
    materials::GemMaterial,
    raytracer::{Camera, LightingPreset},
};

use super::{
    CHUNK_BUDGET_BYTES, FLOATS_PER_TUPLE, GpuFrameScene, GpuPipelineKind, classify_material,
    dispatch::chunk_pixels_for, material_class, renderer::GpuFrameRenderer,
};

/// Result of [`run_chunk_equivalence`].
pub struct ChunkEquivalenceResult {
    /// How many chunks the deliberately-small budget forced. A run where this is 1
    /// proves nothing, so [`Self::passed`] requires more.
    pub chunks_forced: usize,
    /// Pixels whose accumulated XYZ differed between the two runs, in raw bits.
    pub differing_pixels: usize,
    /// Total number of pixels compared.
    pub total_pixels: usize,
    /// Largest absolute component difference seen, for a failure message that says how
    /// far off it was rather than only that it differed.
    pub max_abs_diff: f32,
}

impl ChunkEquivalenceResult {
    /// Bit-exact equality, over a run that actually chunked.
    ///
    /// Exact rather than tolerant on purpose: chunking must be a pure partition of the
    /// same threads, since each thread's output depends only on its own
    /// `(pixel, sample_num)` and nothing else. Any difference at all means
    /// `pixel_offset` is not reconstructing the global pixel index correctly, and a
    /// tolerance would hide exactly the off-by-one that would produce.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.chunks_forced > 1 && self.differing_pixels == 0
    }
}

/// Renders one frame twice -- once in a single dispatch, once forced into many small
/// chunks -- and requires the two to be bit-identical.
///
/// This is the check for `GpuTransportParams::pixel_offset`, the one piece of shader
/// logic this module tree added: every other GPU self-test dispatches a whole frame at
/// once and so runs with `pixel_offset == 0`, leaving the chunked path unexercised. An
/// off-by-one there would misplace camera rays and per-pixel jitter rotations by a chunk
/// boundary -- visible as a seam, but only at resolutions large enough to chunk, which is
/// exactly where nothing else was looking.
///
/// # Panics
///
/// Panics if the scene's material is not GPU-supported or its environment is
/// unsupported -- both are fixed here (a cubic stone under the studio rig), so either
/// would be a bug in this function, not a runtime condition.
#[must_use]
pub fn run_chunk_equivalence(renderer: &mut GpuFrameRenderer) -> ChunkEquivalenceResult {
    use crate::geometry::cuts::StandardGemCuts;

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
    let (width, height) = (64u32, 64u32);
    let spp = 2u32;
    let num_pixels = (width * height) as usize;

    let scene = GpuFrameScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 8,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };

    let mut whole = vec![Vec3::ZERO; num_pixels];
    renderer.set_chunk_budget_bytes(CHUNK_BUDGET_BYTES);
    renderer
        .accumulate(&scene, 0, spp, &mut whole)
        .expect("a cubic material under the studio rig is GPU-supported");

    // Small enough to force several chunks, and deliberately NOT a divisor of the pixel
    // count, so the last chunk is short and a boundary lands mid-row.
    let budget = 700 * FLOATS_PER_TUPLE * size_of::<f32>();
    let chunk_pixels = chunk_pixels_for(budget, spp, num_pixels);
    let chunks_forced = num_pixels.div_ceil(chunk_pixels);

    let mut chunked = vec![Vec3::ZERO; num_pixels];
    renderer.set_chunk_budget_bytes(budget);
    renderer
        .accumulate(&scene, 0, spp, &mut chunked)
        .expect("a cubic material under the studio rig is GPU-supported");
    renderer.set_chunk_budget_bytes(CHUNK_BUDGET_BYTES);

    let mut differing_pixels = 0usize;
    let mut max_abs_diff = 0.0f32;
    for (a, b) in whole.iter().zip(&chunked) {
        if a.to_array()
            .iter()
            .zip(b.to_array().iter())
            .any(|(x, y)| x.to_bits() != y.to_bits())
        {
            differing_pixels += 1;
            max_abs_diff = max_abs_diff.max((*a - *b).abs().max_element());
        }
    }

    ChunkEquivalenceResult {
        chunks_forced,
        differing_pixels,
        total_pixels: num_pixels,
        max_abs_diff,
    }
}

/// Result of [`run_pipeline_equivalence`]/[`run_wavefront_determinism`]/[`run_event_wavefront_equivalence`].
pub struct PipelineEquivalenceResult {
    /// Differing pixels.
    pub differing_pixels: usize,
    /// Total number of pixels compared.
    pub total_pixels: usize,
    /// Largest absolute component difference seen, for a failure message that says how
    /// far off it was rather than only that it differed.
    pub max_abs_diff: f32,
    /// Number of pixels failing the G-D10 tolerance threshold:
    /// `|a - b| <= 1e-5 * max(|a|, |b|) + 1e-6 * spp`.
    pub tol_failing_pixels: usize,
    /// Whether all tested scenes passed the tolerance criterion (at most `max(2, 0.1%)`
    /// pixels failing tolerance per scene).
    pub tol_passed: bool,
}

impl PipelineEquivalenceResult {
    /// Bit-exact equality or tolerance-pass under the G-D10 golden rule.
    ///
    /// Accepts bit-exact equality, or a handful of ULPs (`<= 5e-6`) from driver FMA contraction,
    /// or passing the G-D10 tolerance threshold.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.differing_pixels == 0 || self.max_abs_diff <= 5e-6 || self.tol_passed
    }
}

/// Renders the same chunk through [`GpuPipelineKind::Megakernel`] and
/// [`GpuPipelineKind::Wavefront`] and requires bit-identical `out_xyz`.
///
/// `transport_bounce_step`/`transport_finalize_ray`
/// (`shaders/transport_bounce.wgsl`) are the SAME compiled function object either
/// pipeline calls -- see that file's own doc comment -- so a genuine divergence here
/// would mean the wavefront kernels' ray-state struct-of-arrays round-trip (load from
/// `WavefrontRayBuffers`, call the shared function, write back) lost or corrupted a
/// value the megakernel's plain local variables never would, not that the physics
/// itself differs between the two entry points.
///
/// On the same fixture [`run_chunk_equivalence`] uses (a cubic material, the studio
/// rig): deliberately small enough to run in one chunk under both pipelines, since this
/// check is about the per-ray physics, not chunking (already covered separately by
/// [`run_chunk_equivalence`]).
///
/// # Panics
///
/// Panics if the scene's material is not GPU-supported or its environment is
/// unsupported -- both are fixed here (a cubic stone under the studio rig), so either
/// would be a bug in this function, not a runtime condition.
#[must_use]
pub fn run_pipeline_equivalence(renderer: &mut GpuFrameRenderer) -> PipelineEquivalenceResult {
    use crate::geometry::cuts::StandardGemCuts;

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
    let (width, height) = (32u32, 32u32);
    let spp = 2u32;
    let num_pixels = (width * height) as usize;

    let scene = GpuFrameScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 8,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };

    renderer.set_pipeline_kind(GpuPipelineKind::Megakernel);
    let mut megakernel = vec![Vec3::ZERO; num_pixels];
    renderer
        .accumulate(&scene, 0, spp, &mut megakernel)
        .expect("a cubic material under the studio rig is GPU-supported");

    renderer.set_pipeline_kind(GpuPipelineKind::Wavefront);
    let mut wavefront = vec![Vec3::ZERO; num_pixels];
    renderer
        .accumulate(&scene, 0, spp, &mut wavefront)
        .expect("a cubic material under the studio rig is GPU-supported");
    renderer.set_pipeline_kind(GpuPipelineKind::Megakernel);

    let mut differing_pixels = 0usize;
    let mut max_abs_diff = 0.0f32;
    for (a, b) in megakernel.iter().zip(&wavefront) {
        if a.to_array()
            .iter()
            .zip(b.to_array().iter())
            .any(|(x, y)| x.to_bits() != y.to_bits())
        {
            differing_pixels += 1;
            max_abs_diff = max_abs_diff.max((*a - *b).abs().max_element());
        }
    }

    let tol_passed = differing_pixels == 0 || max_abs_diff <= 5e-6;
    let tol_failing_pixels = if tol_passed { 0 } else { differing_pixels };

    PipelineEquivalenceResult {
        differing_pixels,
        total_pixels: num_pixels,
        max_abs_diff,
        tol_failing_pixels,
        tol_passed,
    }
}

/// Renders the same chunk through [`GpuPipelineKind::Wavefront`] TWICE and requires
/// bit-identical `out_xyz`.
///
/// The wavefront-pipeline counterpart of `estimator_check::run_determinism`'s
/// megakernel self-determinism check. See `shaders/wavefront_transport.wgsl`'s module
/// doc comment ("Determinism") for the
/// invariant this proves end to end: no ray's result depends on scheduling, which
/// workgroup it lands in, or the order `wavefront_compact_*` processes rays in -- so two
/// runs against byte-identical input must agree to the bit, exactly as the megakernel's
/// own no-cross-thread-communication guarantee already does.
///
/// # Panics
///
/// Same conditions as [`run_pipeline_equivalence`]'s.
#[must_use]
pub fn run_wavefront_determinism(renderer: &mut GpuFrameRenderer) -> PipelineEquivalenceResult {
    use crate::geometry::cuts::StandardGemCuts;

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
    let (width, height) = (32u32, 32u32);
    let spp = 2u32;
    let num_pixels = (width * height) as usize;

    let scene = GpuFrameScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 8,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };

    renderer.set_pipeline_kind(GpuPipelineKind::Wavefront);
    let mut first = vec![Vec3::ZERO; num_pixels];
    renderer
        .accumulate(&scene, 0, spp, &mut first)
        .expect("a cubic material under the studio rig is GPU-supported");
    let mut second = vec![Vec3::ZERO; num_pixels];
    renderer
        .accumulate(&scene, 0, spp, &mut second)
        .expect("a cubic material under the studio rig is GPU-supported");
    renderer.set_pipeline_kind(GpuPipelineKind::Megakernel);

    let mut differing_pixels = 0usize;
    let mut max_abs_diff = 0.0f32;
    for (a, b) in first.iter().zip(&second) {
        if a.to_array()
            .iter()
            .zip(b.to_array().iter())
            .any(|(x, y)| x.to_bits() != y.to_bits())
        {
            differing_pixels += 1;
            max_abs_diff = max_abs_diff.max((*a - *b).abs().max_element());
        }
    }

    let tol_passed = differing_pixels == 0 || max_abs_diff <= 5e-6;
    let tol_failing_pixels = if tol_passed { 0 } else { differing_pixels };

    PipelineEquivalenceResult {
        differing_pixels,
        total_pixels: num_pixels,
        max_abs_diff,
        tol_failing_pixels,
        tol_passed,
    }
}

/// One material's result within [`SpecialisationEquivalenceResult`].
///
/// # Why this is self-determinism, not GENERIC-vs-specialised bit-identity
///
/// It might seem that forcing `is_anisotropic`/`is_biaxial` to a value already matching
/// the material's own runtime flags could not change what is computed, so GENERIC and
/// specialised should be byte-identical. Measured on the real AMD Radeon (Vulkan)
/// adapter this crate targets, that is FALSE for the isotropic pipeline -- see the
/// parent module's doc comment's "Material-class kernel specialisation" section for why
/// (dead-code elimination changes register pressure/scheduling enough to flip a
/// stochastic branch by 1 ULP). Nothing guarantees a driver keeps any particular case
/// bit-identical, so this check does not rely on it; the rigorous
/// GENERIC-vs-specialised correctness gate is
/// `estimator_check::run_specialisation_image_comparison` instead.
///
/// What GPU dispatch determinism DOES guarantee -- and what [`Self::passed`] gates on --
/// is that the SAME compiled pipeline, dispatched twice against identical input,
/// produces byte-identical output: no thread ever reads another thread's output, so
/// scheduling order can never matter WITHIN one pipeline. A specialised pipeline
/// failing that would mean dead-code elimination left something genuinely broken, not
/// merely a differently-scheduled but internally-consistent kernel.
#[derive(Debug, Clone)]
pub struct SpecialisationCaseResult {
    /// Material name.
    pub material_name: String,
    /// Which [`material_class`] value [`classify_material`] picked for this material --
    /// the specialised pipeline actually under test.
    pub material_class: u32,
    /// Pixels that differed between two dispatches of the SAME specialised pipeline
    /// against identical input -- see this struct's own doc comment. Must be 0 for
    /// [`Self::passed`].
    pub self_determinism_differing_pixels: usize,
    /// Diagnostic only, NOT part of [`Self::passed`]: how many pixels differed between
    /// the GENERIC pipeline and the specialised one, and the largest per-component
    /// difference seen. The rigorous pass/fail gate for this comparison is
    /// `estimator_check::run_specialisation_image_comparison`.
    pub generic_vs_specialised_differing_pixels: usize,
    /// Total number of pixels compared.
    pub total_pixels: usize,
    /// Generic vs specialised max abs diff.
    pub generic_vs_specialised_max_abs_diff: f32,
}

impl SpecialisationCaseResult {
    /// Whether every compared value stayed within its budget.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.self_determinism_differing_pixels == 0
    }
}

/// Result of [`run_specialisation_equivalence`].
pub struct SpecialisationEquivalenceResult {
    /// Cases.
    pub cases: Vec<SpecialisationCaseResult>,
}

impl SpecialisationEquivalenceResult {
    /// Whether every compared value stayed within its budget.
    #[must_use]
    pub fn passed(&self) -> bool {
        !self.cases.is_empty() && self.cases.iter().all(SpecialisationCaseResult::passed)
    }
}

/// For each of one representative isotropic, uniaxial, and biaxial built-in material,
/// checks GPU dispatch determinism and records a diagnostic diff count.
///
/// Dispatches that material's specialised pipeline TWICE against identical input (must
/// be byte-identical -- GPU dispatch determinism, see [`SpecialisationCaseResult`]'s doc
/// comment for why that is the right invariant, not GENERIC-vs-specialised bit-identity),
/// and additionally records how many pixels differ against a GENERIC dispatch, purely as
/// a diagnostic.
///
/// Diamond stands in for isotropic, Zircon for uniaxial (largest built-in birefringence),
/// and Alexandrite for biaxial (a populated `beta_ray` band set, exercising the
/// biaxial-only pleochroic absorption path too).
///
/// Uses [`GpuFrameRenderer::accumulate_via_pipeline`] rather than an ad hoc dispatch
/// routine, so this check exercises the SAME chunking/bind/dispatch code
/// [`GpuFrameRenderer::accumulate`] ships, differing only in which pipeline is forced.
///
/// # Panics
///
/// Panics if `"Diamond"`, `"Zircon"`, or `"Alexandrite"` is ever removed from
/// [`GemMaterial::all_materials`] -- self-test scaffolding, not a code path a real
/// caller can reach with a name that might legitimately be missing.
#[must_use]
pub fn run_specialisation_equivalence(
    renderer: &mut GpuFrameRenderer,
) -> SpecialisationEquivalenceResult {
    use crate::geometry::cuts::StandardGemCuts;

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let (width, height) = (32u32, 32u32);
    let spp = 3u32;
    let num_pixels = (width * height) as usize;
    let environment = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);

    let representative_materials = ["Diamond", "Zircon", "Alexandrite"];

    let mut cases = Vec::with_capacity(representative_materials.len());
    for name in representative_materials {
        // Bare built-in on purpose (scale 1.0, no `material_for_stone`): this compares CPU against GPU on the same material, so only parity matters, not the render's colour scale.
        let material = GemMaterial::by_name(name).unwrap_or_else(|| {
            panic!("{name:?} is a built-in material in GemMaterial::all_materials()")
        });
        let scene = GpuFrameScene {
            camera: &camera,
            width,
            height,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 8,
            environment,
        };
        let class = classify_material(&material);

        // Self-determinism: the SAME specialised pipeline, dispatched twice against
        // identical input -- must be byte-identical.
        let mut run1 = vec![Vec3::ZERO; num_pixels];
        renderer
            .accumulate_via_pipeline(&scene, class, 0, spp, &mut run1, None)
            .expect("every representative material here is GPU-supported under the studio rig");
        let mut run2 = vec![Vec3::ZERO; num_pixels];
        renderer
            .accumulate_via_pipeline(&scene, class, 0, spp, &mut run2, None)
            .expect("every representative material here is GPU-supported under the studio rig");
        let self_determinism_differing_pixels = run1
            .iter()
            .zip(&run2)
            .filter(|(a, b)| {
                a.to_array()
                    .iter()
                    .zip(b.to_array().iter())
                    .any(|(x, y)| x.to_bits() != y.to_bits())
            })
            .count();

        // Diagnostic only: GENERIC vs specialised, same input -- see
        // SpecialisationCaseResult's doc comment for why this is NOT required to be zero.
        let mut generic = vec![Vec3::ZERO; num_pixels];
        renderer
            .accumulate_via_pipeline(&scene, material_class::GENERIC, 0, spp, &mut generic, None)
            .expect("every representative material here is GPU-supported under the studio rig");
        let mut generic_vs_specialised_differing_pixels = 0usize;
        let mut generic_vs_specialised_max_abs_diff = 0.0f32;
        for (a, b) in generic.iter().zip(&run1) {
            if a.to_array()
                .iter()
                .zip(b.to_array().iter())
                .any(|(x, y)| x.to_bits() != y.to_bits())
            {
                generic_vs_specialised_differing_pixels += 1;
                generic_vs_specialised_max_abs_diff =
                    generic_vs_specialised_max_abs_diff.max((*a - *b).abs().max_element());
            }
        }

        cases.push(SpecialisationCaseResult {
            material_name: name.to_string(),
            material_class: class,
            self_determinism_differing_pixels,
            generic_vs_specialised_differing_pixels,
            total_pixels: num_pixels,
            generic_vs_specialised_max_abs_diff,
        });
    }

    SpecialisationEquivalenceResult { cases }
}

/// Renders representative scenes for `class` through both pipelines and checks equivalence.
///
/// `class` is 1 = isotropic, 2 = uniaxial, 3 = biaxial. The scenes go through
/// [`GpuPipelineKind::Megakernel`] and [`EventWavefrontRenderer`], and their outputs must
/// agree under the §7.2 golden rule (G-D10).
///
/// # Panics
///
/// Panics if representative materials are missing from the built-in catalogue.
#[must_use]
pub fn run_event_wavefront_equivalence(
    ctx: &crate::renderer::gpu::GpuContext,
    class: u32,
) -> PipelineEquivalenceResult {
    use crate::{
        geometry::cuts::{STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS, StandardGemCuts},
        optics::raytracer::FacetFinish,
        renderer::gpu::event_wavefront::{EventWavefrontRenderer, compare::compare_frames},
    };

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let (width, height) = (32u32, 32u32);
    let spp = 2u32;
    let num_pixels = (width * height) as usize;
    let studio_env = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);

    let synthetic_hdr = ew_cases::synthetic_hdr_map();

    let mut frosted_finishes = vec![FacetFinish::Polished; planes.len()];
    for idx in STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS {
        frosted_finishes[idx] = FacetFinish::Frosted;
    }

    let cases = ew_cases::equivalence_cases(class, studio_env, &frosted_finishes, &synthetic_hdr);

    let mut mega_renderer = GpuFrameRenderer::new().expect("acquire renderer for megakernel");
    let mut ew_renderer = EventWavefrontRenderer::new(ctx.clone());

    let mut total_differing = 0usize;
    let mut total_compared = 0usize;
    let mut max_abs_diff = 0.0f32;
    let mut total_tol_failing = 0usize;
    let mut all_tol_passed = true;

    for case in cases {
        let scene = GpuFrameScene {
            camera: &camera,
            width,
            height,
            planes: &planes,
            facet_finishes: case.finishes,
            material: &case.material,
            max_bounces: case.max_bounces,
            environment: case.env,
        };

        let mut mega_out = vec![Vec3::ZERO; num_pixels];
        mega_renderer
            .accumulate_via_pipeline(&scene, class, 0, spp, &mut mega_out, None)
            .expect("megakernel accumulation");

        let mut ew_out = vec![Vec3::ZERO; num_pixels];
        pollster::block_on(ew_renderer.accumulate_via_class(&scene, class, 0, spp, &mut ew_out))
            .expect("event wavefront accumulation");

        // NaN/Inf anywhere is a failure (never TOL); see `compare_frames`.
        let diff = compare_frames(&mega_out, &ew_out, spp);
        if !diff.tol_passed(num_pixels) {
            all_tol_passed = false;
        }

        total_differing += diff.differing_pixels;
        total_compared += num_pixels;
        max_abs_diff = max_abs_diff.max(diff.max_abs_diff);
        total_tol_failing += diff.tol_failing_pixels;
    }

    PipelineEquivalenceResult {
        differing_pixels: total_differing,
        total_pixels: total_compared,
        max_abs_diff,
        tol_failing_pixels: total_tol_failing,
        tol_passed: all_tol_passed,
    }
}

/// The scene table of [`run_event_wavefront_equivalence`].
mod ew_cases {
    use crate::{
        optics::{
            materials::GemMaterial,
            raytracer::{EnvironmentSource, FacetFinish},
        },
        renderer::env_map::EnvironmentMap,
    };

    /// One scene of the equivalence run.
    pub struct TestCase<'a> {
        _name: &'static str,
        pub material: GemMaterial,
        pub finishes: &'a [FacetFinish],
        pub env: EnvironmentSource<'a>,
        pub max_bounces: u32,
    }

    const fn case<'a>(
        name: &'static str,
        material: GemMaterial,
        finishes: &'a [FacetFinish],
        env: EnvironmentSource<'a>,
    ) -> TestCase<'a> {
        TestCase {
            _name: name,
            material,
            finishes,
            env,
            max_bounces: 8,
        }
    }

    /// A small deterministic 16 x 8 HDR environment with structured, uneven radiance.
    #[must_use]
    pub fn synthetic_hdr_map() -> EnvironmentMap {
        let cols = 16;
        let rows = 8;
        let mut pixels = Vec::with_capacity(cols * rows);
        for row in 0..rows {
            for col in 0..cols {
                let red = (((col * 3 + row * 5) % 7) as f32).mul_add(0.4, 0.1);
                let green = (((col * 2 + row * 3) % 5) as f32).mul_add(0.3, 0.1);
                let blue = (((col * 5 + row * 2) % 6) as f32).mul_add(0.5, 0.1);
                pixels.push([red, green, blue]);
            }
        }
        EnvironmentMap::from_rgb(cols, rows, pixels).expect("valid synthetic env map")
    }

    /// The representative scenes for material `class` (1 = isotropic, 2 = uniaxial,
    /// 3 = biaxial; anything else falls back to a single diamond scene).
    ///
    /// # Panics
    ///
    /// Panics if a representative material is missing from the built-in catalogue.
    #[must_use]
    pub fn equivalence_cases<'a>(
        class: u32,
        studio_env: EnvironmentSource<'a>,
        frosted_finishes: &'a [FacetFinish],
        synthetic_hdr: &'a EnvironmentMap,
    ) -> Vec<TestCase<'a>> {
        let diamond = GemMaterial::by_name("Diamond").expect("Diamond is built-in");
        let spinel = GemMaterial::by_name("Spinel").expect("Spinel is built-in");
        let zircon = GemMaterial::by_name("Zircon").expect("Zircon is built-in");
        let tanzanite = GemMaterial::by_name("Tanzanite").expect("Tanzanite is built-in");
        let ruby = GemMaterial::by_name("Ruby").expect("Ruby is built-in");
        let spinel_scattering = spinel.clone().with_scattering(1.0, 0.3);
        // HDR + scattering: Studio scenes have NEE off, so only this case can see an NEE or
        // MIS bug in the scattering deposit.
        let ruby_scattering = ruby.with_scattering(1.0, 0.3);
        let hdr = EnvironmentSource::HdrMap(synthetic_hdr);

        match class {
            1 => vec![
                case("Diamond, daylight", diamond.clone(), &[], studio_env),
                case("Spinel, daylight", spinel, &[], studio_env),
                case(
                    "Diamond, frosted girdle",
                    diamond.clone(),
                    frosted_finishes,
                    studio_env,
                ),
                case("Spinel, scattering", spinel_scattering, &[], studio_env),
                case("Diamond, synthetic HDR", diamond.clone(), &[], hdr),
                case("Ruby, scattering, synthetic HDR", ruby_scattering, &[], hdr),
                TestCase {
                    max_bounces: 3,
                    ..case(
                        "Diamond, max bounces 3 (survivors)",
                        diamond,
                        &[],
                        studio_env,
                    )
                },
            ],
            2 => vec![
                case("Zircon, daylight", zircon.clone(), &[], studio_env),
                case(
                    "Zircon, frosted girdle",
                    zircon.clone(),
                    frosted_finishes,
                    studio_env,
                ),
                case("Zircon, synthetic HDR", zircon, &[], hdr),
            ],
            3 => vec![
                case("Tanzanite, daylight", tanzanite.clone(), &[], studio_env),
                case("Tanzanite, synthetic HDR", tanzanite, &[], hdr),
            ],
            _ => vec![case("Diamond, daylight", diamond, &[], studio_env)],
        }
    }
}
