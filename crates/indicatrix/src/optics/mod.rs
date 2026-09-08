pub mod absorption;
pub mod birefringence;
pub mod dispersion;
pub mod materials;
pub mod polarization;
pub mod raytracer;
pub mod studio_rig;

pub use materials::GemMaterial;
pub use raytracer::{
    Camera, EnvironmentSource, HitRecord, LightingPreset, LightingRigParams, Ray,
    intersect_polyhedron, trace_spectral_ray, xyz_to_rgb_in_space, xyz_to_srgb_gamma,
};
pub use studio_rig::StudioRig;
