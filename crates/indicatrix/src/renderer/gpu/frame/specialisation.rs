//! Material-class kernel specialisation: which of the three specialised
//! `transport_main` pipelines (isotropic/uniaxial/biaxial) a material's real render
//! should dispatch through, and the lazy per-class pipeline cache on
//! [`super::GpuFrameRenderer`]. See the parent module's doc comment's "Material-class
//! kernel specialisation" section for the full rationale.

use crate::optics::materials::{CrystalSystem, GemMaterial};

use super::{SHADER_SRC, renderer::GpuFrameRenderer};
use crate::renderer::gpu::compute;

/// The values `spectral_transport.wgsl`'s `MATERIAL_CLASS` pipeline-overridable
/// constant accepts -- see that override's own doc comment. `pub(crate)` rather than
/// private: `estimator_check::dispatch_transport_for_class` (Tier 3 statistical image
/// comparisons) also needs these to dispatch through the same specialised pipelines
/// [`GpuFrameRenderer::accumulate`] does -- see the parent module's doc comment's
/// "Material-class kernel specialisation" section.
pub mod material_class {
    /// Every class, runtime-dispatched inside the kernel -- the override's own declared
    /// default, so a dispatch that never sets it (every self-test) is unaffected.
    pub const GENERIC: u32 = 0;
    pub const ISOTROPIC: u32 = 1;
    pub const UNIAXIAL: u32 = 2;
    pub const BIAXIAL: u32 = 3;
}

/// Which of [`material_class`]'s values a real render of `material` should dispatch
/// through.
///
/// MIRRORS `renderer::buffers::GpuGemMaterial::encode`'s own `is_anisotropic`/
/// `has_biaxial_delta` derivation exactly -- never a second, independently-maintained
/// definition: biaxial takes priority (`biaxial_delta_beta_alpha.is_some()`), then
/// uniaxial (`crystal_system != Cubic && |birefringence_delta| > 1e-4`, matching the
/// kernel's own `is_anisotropic`), else isotropic. If `encode`'s formula ever changes,
/// this must change with it, or [`GpuFrameRenderer::accumulate`] would pick a specialised
/// pipeline that forces off state the material's own encoded flags say it needs --
/// silently wrong output, not a crash. `estimator_check::run_specialisation_image_comparison`
/// (statistical, not bit-exact) is the check that would catch such a drift.
#[must_use]
pub fn classify_material(material: &GemMaterial) -> u32 {
    if material.biaxial_delta_beta_alpha.is_some() {
        material_class::BIAXIAL
    } else if material.crystal_system != CrystalSystem::Cubic
        && material.birefringence_delta.abs() > 1e-4
    {
        material_class::UNIAXIAL
    } else {
        material_class::ISOTROPIC
    }
}

impl GpuFrameRenderer {
    /// Builds and caches the specialised pipeline for `class`, if not already built --
    /// see the parent module's doc comment's "Material-class kernel specialisation"
    /// section for why this is lazy. A no-op for [`material_class::GENERIC`], built
    /// eagerly in [`GpuFrameRenderer::new`].
    pub(super) fn ensure_specialized_pipeline(&mut self, class: u32) {
        // The `_` arm below treats every out-of-range class exactly like
        // GENERIC (a silent no-op) -- correct for GENERIC itself, but it would just as
        // quietly swallow a caller bug that passes a bogus class, rather than that bug
        // showing up as a panic here instead of much later in `pipeline_for_class`'s own
        // `expect`.
        debug_assert!(
            class <= material_class::BIAXIAL,
            "ensure_specialized_pipeline: class out of range (must be <= material_class::BIAXIAL)"
        );
        let (slot, label) = match class {
            material_class::ISOTROPIC => (
                &mut self.pipeline_isotropic,
                "transport_main (MATERIAL_CLASS=isotropic)",
            ),
            material_class::UNIAXIAL => (
                &mut self.pipeline_uniaxial,
                "transport_main (MATERIAL_CLASS=uniaxial)",
            ),
            material_class::BIAXIAL => (
                &mut self.pipeline_biaxial,
                "transport_main (MATERIAL_CLASS=biaxial)",
            ),
            // GENERIC (and any other value -- none other is caller-reachable) has nothing
            // to build: Self::new already compiled self.pipeline.
            _ => return,
        };
        if slot.is_none() {
            *slot = Some(compute::create_compute_pipeline_with_constants(
                &self.ctx.device,
                label,
                SHADER_SRC,
                "transport_main",
                &[("MATERIAL_CLASS", f64::from(class))],
            ));
        }
    }

    /// Returns the already-built pipeline for `class` -- [`Self::ensure_specialized_pipeline`]
    /// must have been called for this exact `class` first (every caller in this module
    /// tree does so immediately before dispatching).
    ///
    /// # Panics
    ///
    /// Panics if `class` is a specialised value whose pipeline was never built via
    /// [`Self::ensure_specialized_pipeline`] -- a bug in this module's own call
    /// ordering, never a condition a caller outside it can trigger.
    pub(super) const fn pipeline_for_class(&self, class: u32) -> &wgpu::ComputePipeline {
        // Same reasoning as `ensure_specialized_pipeline`'s identical
        // assertion -- the `_` arm below silently routes any out-of-range class onto the
        // generic pipeline instead of the `expect` below ever firing.
        debug_assert!(
            class <= material_class::BIAXIAL,
            "pipeline_for_class: class out of range (must be <= material_class::BIAXIAL)"
        );
        match class {
            material_class::ISOTROPIC => self.pipeline_isotropic.as_ref(),
            material_class::UNIAXIAL => self.pipeline_uniaxial.as_ref(),
            material_class::BIAXIAL => self.pipeline_biaxial.as_ref(),
            _ => Some(&self.pipeline),
        }
        .expect("ensure_specialized_pipeline must be called for this class before dispatching")
    }
}
