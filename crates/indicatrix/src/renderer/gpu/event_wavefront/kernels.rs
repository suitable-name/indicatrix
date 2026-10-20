//! Event wavefront kernel enumeration, entry points, generated shader sources,
//! and per-scene kernel selection logic.

use crate::{
    optics::raytracer::{EnvironmentSource, FacetFinish},
    renderer::{frame_scene::FrameScene, gpu::frame::material_class},
};

/// The thirteen discrete event-wavefront compute kernels.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EwKernel {
    /// Ray generation: initializes ray core and channel state from camera.
    Generate,
    /// Scene intersection and environment miss evaluation.
    Extend,
    /// Homogeneous Henyey-Greenstein interior scattering evaluation.
    Scatter,
    /// Henyey-Greenstein next-event estimation at a parked scatter event (HDR environments).
    NeeHg,
    /// Surface hit setup, shading normal, and interaction arm selection.
    Surface,
    /// Frosted facet finish interaction arm.
    ArmFrosted,
    /// Total internal reflection interaction arm.
    ArmTir,
    /// Uniaxial crystal entry interaction arm.
    ArmUniEntry,
    /// Uniaxial crystal internal reflection/exit interaction arm.
    ArmUniInternal,
    /// Generic isotropic and biaxial Fresnel reflection/refraction interaction arm.
    ArmGeneric,
    /// Deferred chromatic exit split resolution.
    SplitResolve,
    /// Surface glare and Russian roulette path termination.
    Tail,
    /// Final spectral integration and XYZ/debug buffer output.
    Finalize,
}

impl EwKernel {
    /// All event wavefront kernels in canonical sequence.
    pub const ALL: [Self; 13] = [
        Self::Generate,
        Self::Extend,
        Self::Scatter,
        Self::NeeHg,
        Self::Surface,
        Self::ArmFrosted,
        Self::ArmTir,
        Self::ArmUniEntry,
        Self::ArmUniInternal,
        Self::ArmGeneric,
        Self::SplitResolve,
        Self::Tail,
        Self::Finalize,
    ];

    /// WGSL entry point function name.
    #[must_use]
    pub const fn entry_point(self) -> &'static str {
        match self {
            Self::Generate => "ew_generate",
            Self::Extend => "ew_extend",
            Self::Scatter => "ew_scatter",
            Self::NeeHg => "ew_nee_hg",
            Self::Surface => "ew_surface",
            Self::ArmFrosted => "ew_arm_frosted",
            Self::ArmTir => "ew_arm_tir",
            Self::ArmUniEntry => "ew_arm_uni_entry",
            Self::ArmUniInternal => "ew_arm_uni_internal",
            Self::ArmGeneric => "ew_arm_generic",
            Self::SplitResolve => "ew_split_resolve",
            Self::Tail => "ew_tail",
            Self::Finalize => "ew_finalize",
        }
    }

    /// Generated WGSL source code for this kernel.
    #[must_use]
    pub const fn source(self) -> &'static str {
        match self {
            Self::Generate => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_generate.generated.wgsl"))
            }
            Self::Extend => include_str!(concat!(env!("OUT_DIR"), "/ew_extend.generated.wgsl")),
            Self::Scatter => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_scatter.generated.wgsl"))
            }
            Self::NeeHg => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_nee_hg.generated.wgsl"))
            }
            Self::Surface => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_surface.generated.wgsl"))
            }
            Self::ArmFrosted => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_arm_frosted.generated.wgsl"))
            }
            Self::ArmTir => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_arm_tir.generated.wgsl"))
            }
            Self::ArmUniEntry => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_arm_uni_entry.generated.wgsl"))
            }
            Self::ArmUniInternal => include_str!(concat!(
                env!("OUT_DIR"),
                "/ew_arm_uni_internal.generated.wgsl"
            )),
            Self::ArmGeneric => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_arm_generic.generated.wgsl"))
            }
            Self::SplitResolve => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_split_resolve.generated.wgsl"))
            }
            Self::Tail => include_str!(concat!(env!("OUT_DIR"), "/ew_tail.generated.wgsl")),
            Self::Finalize => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_finalize.generated.wgsl"))
            }
        }
    }
}

pub use crate::renderer::buffers::KERNEL_SET_HASH;

/// Returns whether `scene` has any frosted facet finish or frosted girdle facets.
#[must_use]
pub fn has_frosted_facets(scene: &FrameScene<'_>) -> bool {
    scene.facet_finishes.contains(&FacetFinish::Frosted)
}

/// The arm kernels for a material class: the pure core of [`needed_arms`].
///
/// Class 0 (GENERIC, runtime-dispatched) can tag a ray with every arm, so it gets all of them;
/// class 1 (isotropic) never reaches the uniaxial arms and class 3 (biaxial) forces
/// `is_biaxial`, which excludes them too.
#[must_use]
pub fn arms_for(class: u32, frosted: bool) -> Vec<EwKernel> {
    let mut arms = Vec::new();
    if frosted {
        arms.push(EwKernel::ArmFrosted);
    }
    arms.push(EwKernel::ArmTir);
    if class == material_class::UNIAXIAL || class == material_class::GENERIC {
        arms.push(EwKernel::ArmUniEntry);
        arms.push(EwKernel::ArmUniInternal);
    }
    arms.push(EwKernel::ArmGeneric);
    arms
}

/// Selects the surface interaction arm kernels needed for `scene` under `material_class`.
#[must_use]
pub fn needed_arms(material_class: u32, scene: &FrameScene<'_>) -> Vec<EwKernel> {
    arms_for(material_class, has_frosted_facets(scene))
}

/// Whether the scatter phase needs the separate HG next-event kernel: scattering inside the
/// stone under an HDR environment map (`env_mode == 2` in the shader).
#[must_use]
pub fn needs_nee_hg(scene: &FrameScene<'_>) -> bool {
    scene.material.scattering_sigma_s > 0.0
        && matches!(scene.environment, EnvironmentSource::HdrMap(_))
}

/// The kernel list for given scene properties: the pure core of [`needed_kernels`].
#[must_use]
pub fn kernels_for(class: u32, frosted: bool, has_scatter: bool, nee_hg: bool) -> Vec<EwKernel> {
    let mut kernels = Vec::with_capacity(EwKernel::ALL.len());
    kernels.push(EwKernel::Generate);
    kernels.push(EwKernel::Extend);
    if has_scatter {
        kernels.push(EwKernel::Scatter);
    }
    if nee_hg {
        kernels.push(EwKernel::NeeHg);
    }
    kernels.push(EwKernel::Surface);
    kernels.extend(arms_for(class, frosted));
    kernels.push(EwKernel::SplitResolve);
    kernels.push(EwKernel::Tail);
    kernels.push(EwKernel::Finalize);
    kernels
}

/// Selects all event wavefront kernels needed to render `scene` under `material_class` per §4.5.
#[must_use]
pub fn needed_kernels(material_class: u32, scene: &FrameScene<'_>) -> Vec<EwKernel> {
    kernels_for(
        material_class,
        has_frosted_facets(scene),
        scene.material.scattering_sigma_s > 0.0,
        needs_nee_hg(scene),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_class_has_every_arm_a_ray_can_be_tagged_with() {
        let arms = arms_for(material_class::GENERIC, true);
        for arm in [
            EwKernel::ArmFrosted,
            EwKernel::ArmTir,
            EwKernel::ArmUniEntry,
            EwKernel::ArmUniInternal,
            EwKernel::ArmGeneric,
        ] {
            assert!(arms.contains(&arm), "class 0 is missing {arm:?}");
        }
    }

    #[test]
    fn isotropic_and_biaxial_skip_the_uniaxial_arms_uniaxial_keeps_them() {
        for class in [material_class::ISOTROPIC, material_class::BIAXIAL] {
            let arms = arms_for(class, false);
            assert!(!arms.contains(&EwKernel::ArmUniEntry));
            assert!(!arms.contains(&EwKernel::ArmUniInternal));
            assert!(!arms.contains(&EwKernel::ArmFrosted));
        }
        let arms = arms_for(material_class::UNIAXIAL, false);
        assert!(arms.contains(&EwKernel::ArmUniEntry) && arms.contains(&EwKernel::ArmUniInternal));
    }

    #[test]
    fn nee_hg_is_scheduled_only_with_scattering_and_hdr() {
        let with = kernels_for(material_class::ISOTROPIC, false, true, true);
        let without = kernels_for(material_class::ISOTROPIC, false, true, false);
        assert!(with.contains(&EwKernel::NeeHg) && with.contains(&EwKernel::Scatter));
        assert!(!without.contains(&EwKernel::NeeHg));
        let scatter = with.iter().position(|k| *k == EwKernel::Scatter);
        let nee = with.iter().position(|k| *k == EwKernel::NeeHg);
        assert!(scatter < nee, "ew_nee_hg runs after ew_scatter");
    }

    #[test]
    fn all_lists_every_variant_once_in_dispatch_order() {
        let mut sorted = EwKernel::ALL;
        sorted.sort();
        assert_eq!(sorted, EwKernel::ALL, "ALL is in declaration order");
        assert_eq!(EwKernel::ALL.len(), 13);
    }
}
