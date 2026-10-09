//! Surface classes and the scattering of a ray at a surface: Fresnel split, specular rule for
//! polished triangles, GGX microfacet sampling for frosted ones.
//!
//! The microfacet formulas are those of B. Walter, S. R. Marschner, H. Li and K. E. Torrance,
//! "Microfacet Models for Refraction through Rough Surfaces", EGSR 2007: the normal `m` is
//! sampled with density `D(m) (m . n)` (eq. 35 and 36 with the GGX `D`), the outgoing ray follows
//! from the Fresnel-chosen reflection or refraction about `m`, and the importance weight of
//! either branch is `G(i, o, m) |i . m| / (|i . n| |m . n|)` (eq. 41 for reflection, and the same
//! expression for transmission, which is the ratio of eq. 21 and eq. 24). `G` is the Smith
//! shadowing of GGX (eq. 34), the product of the two `G1`. The Fresnel factor comes from
//! `indicatrix::optics::fresnel_dielectric` and enters through the roulette weight.
//!
//! The model is single-scattering: the energy a microfacet surface loses to shadowing and masking
//! is not returned by further microfacet bounces. For the roughness of a ground rough
//! (`alpha` up to about 0.3) this is a few percent at oblique angles and below one percent near
//! the normal; it is part of the model, not a bias of the estimator.

use std::f64::consts::TAU;

use glam::DVec3;
use indicatrix::optics::fresnel_dielectric;

use super::rng::Rng;
use crate::rough_plan::locate::{reflect, refract};

/// How one triangle of the mesh scatters light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SurfaceClass {
    /// A polished window: ideal specular reflection and refraction.
    Polished,
    /// A frosted or uneven skin: GGX microfacets with this `alpha` (the GGX width parameter, the
    /// root-mean-square slope scale; 0.05 is a lightly ground surface, 0.3 a rough one).
    Frosted {
        /// The GGX `alpha`.
        roughness: f32,
    },
}

/// The lowest `alpha` treated as microfacet; below it a surface is polished.
pub const MIN_ROUGHNESS: f32 = 1e-3;

/// The surface class of every triangle: a default and a sorted list of overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceMap {
    default: SurfaceClass,
    overrides: Vec<(u32, SurfaceClass)>,
}

impl SurfaceMap {
    /// Every triangle in `class`.
    #[must_use]
    pub const fn uniform(class: SurfaceClass) -> Self {
        Self {
            default: class,
            overrides: Vec::new(),
        }
    }

    /// Every triangle polished.
    #[must_use]
    pub const fn polished() -> Self {
        Self::uniform(SurfaceClass::Polished)
    }

    /// Every triangle frosted with this GGX `alpha`.
    #[must_use]
    pub const fn frosted(roughness: f32) -> Self {
        Self::uniform(SurfaceClass::Frosted { roughness })
    }

    /// This map with triangle `triangle` set to `class` (replacing an earlier override).
    #[must_use]
    pub fn with_override(mut self, triangle: u32, class: SurfaceClass) -> Self {
        match self.overrides.binary_search_by_key(&triangle, |&(t, _)| t) {
            Ok(i) => self.overrides[i].1 = class,
            Err(i) => self.overrides.insert(i, (triangle, class)),
        }
        self
    }

    /// The class of triangle `triangle`.
    #[must_use]
    pub fn class_of(&self, triangle: u32) -> SurfaceClass {
        self.overrides
            .binary_search_by_key(&triangle, |&(t, _)| t)
            .map_or(self.default, |i| self.overrides[i].1)
    }

    /// The class of triangles without an override.
    #[must_use]
    pub const fn default_class(&self) -> SurfaceClass {
        self.default
    }

    /// The overrides, ascending by triangle.
    #[must_use]
    pub fn overrides(&self) -> &[(u32, SurfaceClass)] {
        &self.overrides
    }
}

/// What a ray does at a surface.
#[derive(Debug, Clone, Copy)]
pub enum Scatter {
    /// The ray continues in `dir`; its weight is multiplied by `factor`. `transmitted` tells
    /// refraction from reflection.
    Out {
        /// The new unit direction.
        dir: DVec3,
        /// The weight factor (roulette weight times the microfacet importance weight).
        factor: f64,
        /// Whether the ray crossed the surface.
        transmitted: bool,
    },
    /// The sample is invalid (a microfacet seen from behind, an outgoing ray on the wrong side
    /// of the surface, or a numerically degenerate geometry): its weight is lost.
    Invalid,
}

/// The reflect-or-transmit decision for a Fresnel reflectance `f`: `(reflect, weight factor)`.
///
/// Russian roulette with probability `f` clamped to `[0.05, 0.95]` and the unbiased weights
/// `f / p` and `(1 - f) / (1 - p)`; total internal reflection (`f = 1`) and no reflection
/// (`f = 0`) take no random number.
fn fresnel_branch(f: f64, rng: &mut Rng) -> (bool, f64) {
    if f >= 1.0 - 1e-12 {
        return (true, 1.0);
    }
    if f <= 1e-12 {
        return (false, 1.0);
    }
    let p = f.clamp(0.05, 0.95);
    if rng.next_f64() < p {
        (true, f / p)
    } else {
        (false, (1.0 - f) / (1.0 - p))
    }
}

/// Smith `G1` of GGX for direction `v`, with `m` the microfacet normal and `n` the geometric
/// normal (both facing the incident side): zero when the microfacet is seen from behind
/// (`chi`), else `2 / (1 + sqrt(1 + alpha^2 tan^2 theta_v))`.
fn smith_g1(v: DVec3, m: DVec3, n: DVec3, alpha: f64) -> f64 {
    let c = v.dot(n);
    if v.dot(m) * c <= 0.0 {
        return 0.0;
    }
    let c2 = c * c;
    if c2 < 1e-18 {
        return 0.0;
    }
    let tan2 = (1.0 - c2).max(0.0) / c2;
    2.0 / (1.0 + (alpha * alpha).mul_add(tan2, 1.0).sqrt())
}

/// Scatters the unit direction `dir` at a surface with the unit normal `facing`, which faces the
/// side the ray comes from (`dir . facing < 0`), going from index `n_from` to `n_to`.
#[must_use]
pub fn scatter(
    class: SurfaceClass,
    dir: DVec3,
    facing: DVec3,
    n_from: f64,
    n_to: f64,
    rng: &mut Rng,
) -> Scatter {
    match class {
        SurfaceClass::Frosted { roughness } if roughness >= MIN_ROUGHNESS => scatter_frosted(
            f64::from(roughness.min(1.0)),
            dir,
            facing,
            n_from,
            n_to,
            rng,
        ),
        _ => scatter_polished(dir, facing, n_from, n_to, rng),
    }
}

fn scatter_polished(dir: DVec3, facing: DVec3, n_from: f64, n_to: f64, rng: &mut Rng) -> Scatter {
    let cos_i = -dir.dot(facing);
    let f = fresnel_dielectric(cos_i, n_from, n_to);
    let (reflected, factor) = fresnel_branch(f, rng);
    if reflected {
        return Scatter::Out {
            dir: reflect(dir, facing),
            factor,
            transmitted: false,
        };
    }
    refract(dir, facing, n_from, n_to).map_or(Scatter::Invalid, |out| Scatter::Out {
        dir: out,
        factor,
        transmitted: true,
    })
}

fn scatter_frosted(
    alpha: f64,
    dir: DVec3,
    facing: DVec3,
    n_from: f64,
    n_to: f64,
    rng: &mut Rng,
) -> Scatter {
    // Sample the microfacet normal (eq. 35 and 36): tan^2 theta = alpha^2 xi1 / (1 - xi1).
    let xi1 = rng.next_f64();
    let xi2 = rng.next_f64();
    let tan2 = alpha * alpha * xi1 / (1.0 - xi1).max(1e-12);
    let cos_m = 1.0 / (1.0 + tan2).sqrt();
    let sin_m = cos_m.mul_add(-cos_m, 1.0).max(0.0).sqrt();
    let phi = TAU * xi2;
    let (t1, t2) = facing.any_orthonormal_pair();
    let m = (t1 * (sin_m * phi.cos()) + t2 * (sin_m * phi.sin()) + facing * cos_m).normalize();

    let i = -dir;
    let i_m = i.dot(m);
    let i_n = i.dot(facing);
    if i_m <= 0.0 || i_n < 1e-9 {
        return Scatter::Invalid;
    }
    let f = fresnel_dielectric(i_m, n_from, n_to);
    let (reflected, branch_factor) = fresnel_branch(f, rng);
    let (out, transmitted) = if reflected {
        (reflect(dir, m), false)
    } else {
        match refract(dir, m, n_from, n_to) {
            Some(out) => (out, true),
            None => return Scatter::Invalid,
        }
    };
    let o_n = out.dot(facing);
    let side_ok = if transmitted { o_n < 0.0 } else { o_n > 0.0 };
    if !side_ok {
        return Scatter::Invalid;
    }
    let g = smith_g1(i, m, facing, alpha) * smith_g1(out, m, facing, alpha);
    if g <= 0.0 {
        return Scatter::Invalid;
    }
    let m_n = m.dot(facing);
    let importance = g * i_m / (i_n * m_n);
    Scatter::Out {
        dir: out,
        factor: branch_factor * importance,
        transmitted,
    }
}
