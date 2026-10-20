//! Immutable per-scene preparation for the deterministic ray tracer.

use glam::{Mat3, Vec3};

use crate::{
    color::space::ColorSpace,
    optics::{
        fluorescence::Fluorescence,
        raytracer::{EnvironmentSource, LightingPreset, environment::environment_white_balance},
    },
    renderer::{engine::RayOptions, frame_scene::FrameScene},
};

use super::{WhittedDecline, env::WhittedEnv, frost, tint};

/// A refractive-index chain and the RGB channels its radiance contributes to.
#[derive(Clone, Copy, Debug)]
pub struct Chain {
    /// Scalar refractive index.
    pub n: f32,
    /// RGB channel selector; white for the single-index chain.
    pub mask: Vec3,
}

/// Direction-dependent RGB absorption coefficients, per model unit.
#[derive(Clone, Copy, Debug)]
pub enum AlphaModel {
    /// Isotropic coefficient, also used for the three-mode biaxial mean.
    Isotropic(Vec3),
    /// Ordinary and extraordinary coefficients about the optical axis.
    Uniaxial { o: Vec3, e: Vec3, axis: Vec3 },
}

impl AlphaModel {
    /// Coefficient for the ray's unit propagation direction.
    #[must_use]
    #[expect(
        clippy::suspicious_operation_groupings,
        reason = "false positive: `(o - e) * cosine^2` is the intended uniaxial mix, bit-pinned to the WGSL twin"
    )]
    pub fn at(&self, dir: Vec3) -> Vec3 {
        match *self {
            Self::Isotropic(alpha) => alpha,
            Self::Uniaxial { o, e, axis } => {
                // Plain glam `dot` on purpose: the WGSL twin (`00_bindings.wgsl`) uses a
                // plain `dot` here too, so the twins agree (unlike the fused chain `dot`).
                let cosine = dir.dot(axis);
                (o + e + (o - e) * (cosine * cosine)) * 0.5
            }
        }
    }
}

/// All expensive tint, light and frost preparation, cached once per scene.
pub struct WhittedPrep<'a> {
    /// The fixed one- or three-index chains.
    pub chains: [Chain; 3],
    /// Number of active entries in `chains`.
    pub chain_count: u8,
    /// Beer-Lambert coefficients.
    pub alpha: AlphaModel,
    /// Deterministic environment sampler.
    pub env: WhittedEnv<'a>,
    /// Scale of polished primary reflection only.
    pub glare: f32,
    /// Cosine-mean outside radiance per plane; empty for all polished facets.
    pub frost: Vec<Vec3>,
    /// Inverse of the renderer's linear-sRGB conversion matrix.
    pub rgb_to_xyz: Mat3,
    /// Bradford LMS scale for a Studio Planckian illuminant, otherwise absent.
    pub white_balance: Option<Vec3>,
    /// Maximum chain depth.
    pub max_depth: u32,
    /// Sanitized fixed AA tap count.
    pub taps: u8,
}

impl<'a> WhittedPrep<'a> {
    /// Resolves a scene once. UV illumination and fluorescence require Path-Traced.
    ///
    /// # Errors
    /// Returns the reason the requested scene cannot use this renderer.
    pub fn new(
        scene: &FrameScene<'a>,
        fluorescence: &Fluorescence,
        ray: RayOptions,
    ) -> Result<Self, WhittedDecline> {
        if !fluorescence.is_empty() {
            return Err(WhittedDecline::Fluorescence);
        }
        if scene
            .environment
            .lighting_preset()
            .is_some_and(LightingPreset::is_uv_lamp)
        {
            return Err(WhittedDecline::UvLamp);
        }
        let material = scene.material;
        let indices =
            [615.0, 545.0, 465.0].map(|wavelength| material.dispersion.evaluate(wavelength));
        let three = ray.fire && (indices[0] - indices[2]).abs() >= 1.0e-5;
        let chains = if three {
            std::array::from_fn(|index| Chain {
                n: indices[index],
                mask: [Vec3::X, Vec3::Y, Vec3::Z][index],
            })
        } else {
            [Chain {
                n: material.dispersion.n_d(),
                mask: Vec3::ONE,
            }; 3]
        };
        let absorption = &material.absorption;
        let coefficient =
            |bands: &[_]| tint::alpha(bands, material.absorption_path_scale, scene.environment);
        let o = coefficient(&absorption.o_ray);
        let alpha = absorption.beta_ray.as_ref().map_or_else(
            || {
                if absorption.is_pleochroic {
                    let normalized = material.c_axis.normalize_or_zero();
                    AlphaModel::Uniaxial {
                        o,
                        e: coefficient(&absorption.e_ray),
                        axis: if normalized == Vec3::ZERO {
                            Vec3::Y
                        } else {
                            normalized
                        },
                    }
                } else {
                    AlphaModel::Isotropic(o)
                }
            },
            |beta| {
                AlphaModel::Isotropic(
                    (o + coefficient(&absorption.e_ray) + coefficient(beta)) / 3.0,
                )
            },
        );
        let env = WhittedEnv::new(scene.environment);
        let frost = frost::prepare(scene, &env);
        let white_balance = match scene.environment {
            EnvironmentSource::Studio { preset, .. } if preset.uses_white_balance() => {
                Some(environment_white_balance(scene.environment))
            }
            _ => None,
        };
        Ok(Self {
            chains,
            chain_count: if three { 3 } else { 1 },
            alpha,
            env,
            glare: scene.environment.surface_glare(),
            frost,
            rgb_to_xyz: rgb_to_xyz(),
            white_balance,
            max_depth: scene.max_bounces,
            taps: ray.sanitized().aa_taps,
        })
    }

    /// Makes preparation cacheable beside owned scene data without self-references.
    /// HDR pixels are copied at most once, when this method detaches the sampler.
    #[must_use]
    pub fn into_owned(self) -> WhittedPrep<'static> {
        self.into_owned_with_hdr(None)
    }

    /// Detaches preparation while sharing the scene's existing HDR allocation.
    /// A mismatched map is ignored so an unrelated scene can never replace the light.
    #[must_use]
    pub fn into_owned_with_hdr(
        self,
        shared: Option<std::sync::Arc<crate::renderer::env_map::EnvironmentMap>>,
    ) -> WhittedPrep<'static> {
        WhittedPrep {
            chains: self.chains,
            chain_count: self.chain_count,
            alpha: self.alpha,
            env: self.env.into_owned_with_hdr(shared),
            glare: self.glare,
            frost: self.frost,
            rgb_to_xyz: self.rgb_to_xyz,
            white_balance: self.white_balance,
            max_depth: self.max_depth,
            taps: self.taps,
        }
    }

    #[cfg(test)]
    pub(super) fn for_tests(n: f32, alpha: Vec3, env_rgb: Vec3, max_depth: u32, taps: u8) -> Self {
        Self {
            chains: [Chain { n, mask: Vec3::ONE }; 3],
            chain_count: 1,
            alpha: AlphaModel::Isotropic(alpha),
            env: WhittedEnv::for_tests(env_rgb),
            glare: 1.0,
            frost: Vec::new(),
            rgb_to_xyz: rgb_to_xyz(),
            white_balance: None,
            max_depth,
            taps: RayOptions {
                aa_taps: taps,
                fire: false,
            }
            .sanitized()
            .aa_taps,
        }
    }
}

pub(super) fn rgb_to_xyz() -> Mat3 {
    let rows = ColorSpace::Srgb.xyz_to_rgb_matrix();
    Mat3::from_cols_array_2d(&rows).transpose().inverse()
}
