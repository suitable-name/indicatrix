//! Deterministic linear-sRGB environment lookups; no transport samples are drawn.

use std::sync::Arc;

use glam::Vec3;

use crate::{
    optics::{
        raytracer::{
            DEFAULT_HEAD_SHADOW_COSINES, EnvironmentSource, LightingPreset,
            environment::{
                head_shadow_cosines, sample_studio_environment_with_rig_shadow_tray,
                sample_studio_environment_with_rig_spec_power_shadow_tray,
            },
        },
        studio_rig::StudioRig,
    },
    renderer::env_map::EnvironmentMap,
};

use super::tint::{illuminant_rgb, integrate_rgb};

enum Source<'a> {
    Studio {
        preset: LightingPreset,
        exposure: f32,
        rig: Box<StudioRig>,
        shadow: [f32; 2],
        rgb: Vec3,
        backdrop: f32,
    },
    Aset {
        table: Box<[Vec3; 1024]>,
        rgb: Vec3,
        backdrop: f32,
    },
    Hdr(&'a EnvironmentMap),
    OwnedHdr(Arc<EnvironmentMap>),
    #[cfg(test)]
    Constant(Vec3),
}

/// Prepared environment radiance, expressed in linear sRGB.
pub struct WhittedEnv<'a> {
    source: Source<'a>,
}

/// Borrowed prepared data: GPU uploads reuse the CPU tables and rig rather than
/// independently integrating spectra or approximating a directional environment.
#[cfg(feature = "gpu")]
pub enum WhittedGpuEnv<'a> {
    Studio {
        preset: LightingPreset,
        exposure: f32,
        rig: &'a StudioRig,
        shadow: [f32; 2],
        rgb: Vec3,
        backdrop: f32,
    },
    Aset {
        table: &'a [Vec3; 1024],
        rgb: Vec3,
        backdrop: f32,
    },
    Hdr(&'a EnvironmentMap),
    #[cfg(test)]
    Constant(Vec3),
}

impl<'a> WhittedEnv<'a> {
    #[cfg(feature = "gpu")]
    pub(crate) fn gpu_data(&self) -> WhittedGpuEnv<'_> {
        match &self.source {
            Source::Studio {
                preset,
                exposure,
                rig,
                shadow,
                rgb,
                backdrop,
            } => WhittedGpuEnv::Studio {
                preset: *preset,
                exposure: *exposure,
                rig,
                shadow: *shadow,
                rgb: *rgb,
                backdrop: *backdrop,
            },
            Source::Aset {
                table,
                rgb,
                backdrop,
            } => WhittedGpuEnv::Aset {
                table,
                rgb: *rgb,
                backdrop: *backdrop,
            },
            Source::Hdr(map) => WhittedGpuEnv::Hdr(map),
            Source::OwnedHdr(map) => WhittedGpuEnv::Hdr(map),
            #[cfg(test)]
            Source::Constant(rgb) => WhittedGpuEnv::Constant(*rgb),
        }
    }

    pub(super) fn new(environment: EnvironmentSource<'a>) -> Self {
        let source = match environment {
            EnvironmentSource::HdrMap(map) => Source::Hdr(map),
            EnvironmentSource::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                head_shadow_deg,
                ..
            } => {
                let rig = StudioRig::new(light_yaw, light_pitch);
                let rgb = illuminant_rgb(environment);
                if preset == LightingPreset::Aset {
                    let table = Box::new(std::array::from_fn(|row| {
                        #[expect(clippy::cast_precision_loss, reason = "table index is below 1024")]
                        let y = (row as f32 / 1023.0).mul_add(2.0, -1.0);
                        // Operation order mirrors the WGSL twin 00_bindings.wgsl: the square is
                        // its own binding so no clippy version suggests a fused multiply-add.
                        let square = y * y;
                        let dir = Vec3::new((1.0 - square).max(0.0).sqrt(), y, 0.0);
                        // The lit tray (`backdrop`, 0.0 = As Lit) is part of the table, so
                        // the ASET surround and its windows read the same value.
                        integrate_rgb(|wavelength| {
                            sample_studio_environment_with_rig_shadow_tray(
                                dir,
                                wavelength,
                                preset,
                                exposure,
                                &rig,
                                Vec3::ZERO,
                                DEFAULT_HEAD_SHADOW_COSINES,
                                backdrop,
                            )
                        })
                    }));
                    Source::Aset {
                        table,
                        rgb,
                        backdrop,
                    }
                } else {
                    Source::Studio {
                        preset,
                        exposure,
                        rig: Box::new(rig),
                        shadow: head_shadow_cosines(head_shadow_deg),
                        rgb,
                        backdrop,
                    }
                }
            }
        };
        Self { source }
    }

    /// Radiance reaching a leaf in `dir`, with the pixel's direction toward its eye.
    #[must_use]
    pub fn leaf(&self, dir: Vec3, observer: Vec3) -> Vec3 {
        match &self.source {
            Source::Hdr(map) => Vec3::from_array(map.radiance_rgb(dir)),
            Source::OwnedHdr(map) => Vec3::from_array(map.radiance_rgb(dir)),
            Source::Studio {
                preset,
                exposure,
                rig,
                shadow,
                rgb,
                backdrop,
            } => {
                // One arithmetic path for every head-shadow setting and for the lit
                // backdrop tray (`backdrop` = its radiance at exposure 1, 0.0 = As Lit):
                // a primary miss, a window and a reflection all call it, so they agree.
                let geometry = sample_studio_environment_with_rig_spec_power_shadow_tray(
                    dir, *preset, *exposure, rig, observer, 1.0, *shadow, *backdrop,
                );
                *rgb * geometry
            }
            Source::Aset { table, .. } => {
                let position = (dir.y.clamp(-1.0, 1.0) + 1.0) * 511.5;
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "clamped table coordinate is in 0..=1023"
                )]
                let lower = position as usize;
                #[expect(clippy::cast_precision_loss, reason = "table index is below 1024")]
                let fraction = position - lower as f32;
                table[lower].lerp(table[(lower + 1).min(1023)], fraction)
            }
            #[cfg(test)]
            Source::Constant(rgb) => *rgb,
        }
    }

    /// Detaches a prepared environment from its scene for long-lived frame caches.
    /// An HDR map is copied once; no per-tap or per-chunk copy occurs.
    #[must_use]
    pub fn into_owned(self) -> WhittedEnv<'static> {
        self.into_owned_with_hdr(None)
    }

    /// Reuses the scene's existing HDR allocation when it is the exact map this
    /// sampler borrows. A mismatching Arc cannot silently replace the lighting.
    #[must_use]
    pub fn into_owned_with_hdr(self, shared: Option<Arc<EnvironmentMap>>) -> WhittedEnv<'static> {
        let source = match self.source {
            Source::Hdr(map) => Source::OwnedHdr(
                shared
                    .filter(|candidate| std::ptr::eq(map, candidate.as_ref()))
                    .unwrap_or_else(|| Arc::new(map.clone())),
            ),
            Source::OwnedHdr(map) => Source::OwnedHdr(map),
            Source::Studio {
                preset,
                exposure,
                rig,
                shadow,
                rgb,
                backdrop,
            } => Source::Studio {
                preset,
                exposure,
                rig,
                shadow,
                rgb,
                backdrop,
            },
            Source::Aset {
                table,
                rgb,
                backdrop,
            } => Source::Aset {
                table,
                rgb,
                backdrop,
            },
            #[cfg(test)]
            Source::Constant(rgb) => Source::Constant(rgb),
        };
        WhittedEnv { source }
    }

    #[cfg(test)]
    pub(super) const fn for_tests(rgb: Vec3) -> Self {
        Self {
            source: Source::Constant(rgb),
        }
    }
}
