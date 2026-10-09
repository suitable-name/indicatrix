//! The zoned stones the GPU checks render (`zoning` feature only).
//!
//! One fixture list shared by the Tier 3 statistical image comparisons
//! (`estimator_check::run_image_comparison_zoned`), the GPU harness and the `gpu_backend`
//! tests, so each of them exercises the same shapes.
//!
//! Every stone is a colourless diamond whose absorption is replaced by zones, sized to
//! [`ZONED_PATH_SCALE`] millimetres per model unit (a round brilliant of model girdle radius
//! about 1 is then about 6 mm wide). Zone geometry is in stone millimetres, origin at the model
//! origin, the stone's axis along +Y.

use glam::DVec3;

use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    materials::GemMaterial,
    zoning::{Zone, ZoneAbsorption, ZoneFrame, ZoneShape, ZonedAbsorption},
};

/// Millimetres per model unit of every fixture below.
pub const ZONED_PATH_SCALE: f32 = 3.0;

/// One zoned test stone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZonedImageCase {
    /// Two halves split by the plane `x = 0`: absorbs red / absorbs blue. Exercises the half space.
    Bicolour,
    /// Watermelon: a trigonal prism tube (axis +Y) inside a base zone, capped by a half space.
    /// Exercises the prism and a second override.
    Watermelon,
    /// Two sectors about +Y, one narrow and one wider than pi. Exercises both sector forms.
    Sectors,
    /// A slab and a cylinder tube. Exercises the slab and the cylinder.
    SlabAndTube,
    /// [`Self::Bicolour`] with a 1.2 mm smoothstep boundary. Exercises the soft kernel.
    SoftBicolour,
    /// The prism-tube stone with a 0.8 mm smoothstep boundary. Exercises the soft kernel on a
    /// curved/kinked depth.
    SoftWatermelon,
}

impl ZonedImageCase {
    /// Every case, in report order.
    pub const ALL: [Self; 6] = [
        Self::Bicolour,
        Self::Watermelon,
        Self::Sectors,
        Self::SlabAndTube,
        Self::SoftBicolour,
        Self::SoftWatermelon,
    ];

    /// A short label for reports.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bicolour => "zoned bicolour (half space)",
            Self::Watermelon => "zoned watermelon (prism tube + cap)",
            Self::Sectors => "zoned sectors (narrow + wide)",
            Self::SlabAndTube => "zoned slab + cylinder tube",
            Self::SoftBicolour => "zoned bicolour, soft 1.2 mm boundary",
            Self::SoftWatermelon => "zoned watermelon, soft 0.8 mm boundary",
        }
    }

    /// The zoning of this case.
    #[must_use]
    pub fn zoning(self) -> ZonedAbsorption {
        match self {
            Self::Bicolour => bicolour(0.0),
            Self::SoftBicolour => bicolour(1.2),
            Self::Watermelon => watermelon(0.0),
            Self::SoftWatermelon => watermelon(0.8),
            Self::Sectors => sectors(),
            Self::SlabAndTube => slab_and_tube(),
        }
    }

    /// The stone: a colourless diamond with this case's zones and [`ZONED_PATH_SCALE`].
    #[must_use]
    pub fn material(self) -> GemMaterial {
        GemMaterial::diamond()
            .with_zoning(self.zoning())
            .with_absorption_path_scale(ZONED_PATH_SCALE)
    }
}

fn band(center_nm: f32, width_nm: f32, peak_per_mm: f32) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
        center_nm,
        width_nm,
        peak_per_mm,
    )]))
}

fn clear() -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(Vec::new()))
}

fn red_absorbing() -> ZoneAbsorption {
    band(620.0, 50.0, 0.25)
}

fn blue_absorbing() -> ZoneAbsorption {
    band(450.0, 40.0, 0.30)
}

fn green_absorbing() -> ZoneAbsorption {
    band(540.0, 45.0, 0.30)
}

const fn zoned(base: ZoneAbsorption, zones: Vec<Zone>, softness_mm: f32) -> ZonedAbsorption {
    ZonedAbsorption {
        frame: ZoneFrame::IDENTITY,
        base,
        zones,
        boundary_softness_mm: softness_mm,
    }
}

fn bicolour(softness_mm: f32) -> ZonedAbsorption {
    zoned(
        red_absorbing(),
        vec![Zone {
            shape: ZoneShape::HalfSpace {
                normal: DVec3::X,
                offset: 0.0,
            },
            absorption: blue_absorbing(),
        }],
        softness_mm,
    )
}

fn watermelon(softness_mm: f32) -> ZonedAbsorption {
    zoned(
        green_absorbing(),
        vec![
            Zone {
                shape: ZoneShape::CoaxialPrism {
                    axis_point: DVec3::ZERO,
                    axis_dir: DVec3::Y,
                    n_sides: 3,
                    r_in: 0.0,
                    r_out: 1.8,
                    phase: 0.4,
                },
                absorption: red_absorbing(),
            },
            Zone {
                shape: ZoneShape::HalfSpace {
                    normal: DVec3::Y,
                    offset: 0.8,
                },
                absorption: blue_absorbing(),
            },
        ],
        softness_mm,
    )
}

fn sectors() -> ZonedAbsorption {
    zoned(
        clear(),
        vec![
            Zone {
                shape: ZoneShape::Sector {
                    axis_point: DVec3::ZERO,
                    axis_dir: DVec3::Y,
                    angle_from: 0.3,
                    angle_to: 1.9,
                },
                absorption: red_absorbing(),
            },
            Zone {
                shape: ZoneShape::Sector {
                    axis_point: DVec3::ZERO,
                    axis_dir: DVec3::Y,
                    angle_from: 2.4,
                    angle_to: 5.9,
                },
                absorption: blue_absorbing(),
            },
        ],
        0.0,
    )
}

fn slab_and_tube() -> ZonedAbsorption {
    zoned(
        clear(),
        vec![
            Zone {
                shape: ZoneShape::Slab {
                    normal: DVec3::Z,
                    offset_min: -1.2,
                    offset_max: 1.2,
                },
                absorption: green_absorbing(),
            },
            Zone {
                shape: ZoneShape::CoaxialCylinder {
                    axis_point: DVec3::ZERO,
                    axis_dir: DVec3::Y,
                    r_in: 0.6,
                    r_out: 1.6,
                },
                absorption: red_absorbing(),
            },
        ],
        0.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fixture_is_valid_and_gpu_renderable() {
        for case in ZonedImageCase::ALL {
            let material = case.material();
            let zoning = material.zoning.as_ref().expect("zoned");
            assert_eq!(zoning.validate(), Ok(()), "{}", case.label());
            assert!(material.gpu_supported(), "{}", case.label());
        }
    }
}
