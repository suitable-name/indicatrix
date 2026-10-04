/// Beer-Lambert pleochroic absorption tensors (isotropic, uniaxial and biaxial band
/// sets) evaluated along a ray's assigned eigenmode.
pub mod absorption;
/// Uniaxial and biaxial birefringence.
///
/// Effective extraordinary index, walk-off, eigen-polarizations, and the biaxial
/// optical indicatrix.
pub mod birefringence;
/// Physically based chromophore absorption modeling, recipe compilation, and inverse solving.
pub mod chromophore;
/// Refractive-index dispersion curves (Sellmeier and Cauchy fits) evaluated by
/// wavelength.
pub mod dispersion;
/// CPU-only fluorescence: emitter data and the in-medium fluorescence vertex sampling.
pub mod fluorescence;
/// [`GemMaterial`] and the built-in gemstone material table.
///
/// Each entry carries dispersion, birefringence, pleochroism and inclusion
/// scattering.
pub mod materials;
/// Stokes-vector / Mueller-matrix polarization state and the Fresnel/TIR matrices
/// that act on it.
pub mod polarization;
/// The spectral Monte-Carlo raytracer.
///
/// Camera rays, intersection, refraction, transport, environment sampling, and
/// color output.
pub mod raytracer;
/// The gemological studio lighting rig.
///
/// Shared by the CPU and GPU environment sampling paths.
pub mod studio_rig;

pub use fluorescence::{EmissionBand, Fluorescence, FluorescentEmitter};
pub use materials::GemMaterial;
pub use raytracer::{
    Camera, EnvironmentSource, HitRecord, LightingPreset, LightingRigParams, Ray,
    intersect_polyhedron, trace_spectral_ray, xyz_to_rgb_in_space, xyz_to_srgb_gamma,
};
pub use studio_rig::StudioRig;
