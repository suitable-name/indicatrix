//! Camera/ray generation.
//!
//! The pinhole [`Camera`] model plus the [`Ray`] and [`HitRecord`] primitives every
//! intersection routine in this module tree operates on, and [`FacetFinish`] (a
//! facet's surface finish, looked up per-hit alongside its [`HitRecord::facet_idx`]).

use glam::Vec3;

#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

#[derive(Clone, Copy, Debug)]
pub struct HitRecord {
    pub t: f32,
    pub normal: Vec3,
    pub facet_idx: usize,
}

/// Girdle finish: a facet's surface finish.
///
/// Looked up per-hit via `HitRecord::facet_idx` into a `&[FacetFinish]` slice parallel to
/// `&[GpuFacetPlane]` (not a new field on `GpuFacetPlane` itself, to avoid disturbing that
/// struct's byte layout, its GPU echo test, or the Tier 2 `intersect_polyhedron` kernel
/// that reads it -- none of which need to know about finish at all). See
/// `apply_frosted_bounce`'s doc comment for what `Frosted` actually models, and
/// `trace_spectral_ray_with_finish`'s doc comment for why this is a distinct CPU entry
/// point from `trace_spectral_ray` rather than a new parameter on it.
///
/// The GPU megakernel has a full frosted-finish counterpart, not merely a Tier 2
/// self-test: `renderer::gpu::frame::TransportScene::facet_finishes` accepts a
/// `&[FacetFinish]` slice exactly like this one, `renderer::buffers::encode_facet_finishes`
/// packs it into the same `facet_finishes` GPU buffer `shaders/spectral_transport.wgsl`
/// reads (binding 8), and that kernel routes a `Frosted` hit through a bit-for-bit WGSL
/// translation of `apply_frosted_bounce` (in `shaders/transport_physics.wgsl`) instead of
/// the polished TIR/reflect/refract dispatch -- verified against this real CPU function by
/// `renderer::gpu::transport_check`'s Tier 2 `run_frosted_bounce` self-test. Unlike biaxial
/// materials (`GemMaterial::gpu_supported`), which remain a genuine CPU-only restriction,
/// `FacetFinish::Frosted` is not missing anything on the GPU side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FacetFinish {
    /// A smooth optical interface: the existing Dirac-delta specular Fresnel machinery,
    /// completely unchanged. Every scene rendered before girdle finish was added (and every scene calling
    /// the unmodified `trace_spectral_ray`) is implicitly all-`Polished`.
    #[default]
    Polished,
    /// A ground/lapped ("bruted") surface: diffusely scattering rather than mirror-like.
    Frosted,
}

pub struct Camera {
    pub origin: Vec3,
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub fov_tan: f32,
}

impl Camera {
    #[must_use]
    pub fn new(yaw: f32, pitch: f32, distance: f32, fov_deg: f32) -> Self {
        let cos_p = pitch.cos();
        let sin_p = pitch.sin();
        let cos_y = yaw.cos();
        let sin_y = yaw.sin();

        // 3D camera position orbiting origin (0, 0, 0)
        let origin = Vec3::new(
            distance * cos_p * sin_y,
            distance * sin_p,
            distance * cos_p * cos_y,
        );

        let forward = (-origin).normalize();
        let world_up = if cos_p.abs() < 1e-4 {
            Vec3::new(0.0, 0.0, -1.0)
        } else {
            Vec3::Y
        };
        let right = forward.cross(world_up).normalize();
        let up = right.cross(forward).normalize();
        let fov_tan = (fov_deg.to_radians() * 0.5).tan();

        Self {
            origin,
            forward,
            right,
            up,
            fov_tan,
        }
    }

    #[must_use]
    pub fn generate_ray(
        &self,
        screen_x: f32,
        screen_y: f32,
        width: f32,
        height: f32,
        jitter_x: f32,
        jitter_y: f32,
    ) -> Ray {
        let aspect = width / height;
        let u = ((screen_x + jitter_x) / width - 0.5) * 2.0 * aspect * self.fov_tan;
        let v = (0.5 - (screen_y + jitter_y) / height) * 2.0 * self.fov_tan;
        let dir = (self.forward + self.right * u + self.up * v).normalize();

        Ray {
            origin: self.origin,
            dir,
        }
    }
}
