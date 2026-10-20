//! Spectral-to-RGB absorption coefficients, resolved once per scene.

use glam::Vec3;

use crate::{
    color::{cie1931::cie_1931_cmf, space::ColorSpace},
    optics::{absorption::AbsorptionBand, raytracer::EnvironmentSource},
    render_setup::MODEL_UNIT_FACE_UP_PATH,
};

/// Integrates the same visible CMFs and normalization as the spectral renderer.
pub(super) fn integrate_rgb(mut spectrum: impl FnMut(f32) -> f32) -> Vec3 {
    let mut xyz = Vec3::ZERO;
    for step in 0_u16..=80 {
        let wavelength = f32::from(step).mul_add(5.0, 380.0);
        xyz += Vec3::from_array(cie_1931_cmf(wavelength)) * spectrum(wavelength) * 5.0;
    }
    ColorSpace::Srgb.xyz_to_linear(xyz / 106.856)
}

pub(super) fn illuminant(environment: EnvironmentSource<'_>, wavelength: f32) -> f32 {
    match environment {
        EnvironmentSource::Studio { preset, .. } => preset.spectral_power(wavelength),
        EnvironmentSource::HdrMap(_) => {
            crate::optics::raytracer::environment::d65_relative_spectral_power(wavelength)
        }
    }
}

pub(super) fn illuminant_rgb(environment: EnvironmentSource<'_>) -> Vec3 {
    integrate_rgb(|wavelength| illuminant(environment, wavelength))
}

pub(super) fn alpha(
    bands: &[AbsorptionBand],
    path_scale: f32,
    environment: EnvironmentSource<'_>,
) -> Vec3 {
    if bands.is_empty() || path_scale <= 0.0 {
        return Vec3::ZERO;
    }
    let white = illuminant_rgb(environment);
    let transmitted = integrate_rgb(|wavelength| {
        let coefficient: f32 = bands.iter().map(|band| band.evaluate(wavelength)).sum();
        illuminant(environment, wavelength)
            * (-coefficient.max(0.0) * path_scale * MODEL_UNIT_FACE_UP_PATH).exp()
    });
    let transmission = (transmitted / white).clamp(Vec3::splat(1.0e-4), Vec3::ONE);
    Vec3::new(
        -transmission.x.ln(),
        -transmission.y.ln(),
        -transmission.z.ln(),
    ) / MODEL_UNIT_FACE_UP_PATH
}
