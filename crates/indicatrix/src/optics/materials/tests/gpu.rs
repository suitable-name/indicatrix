//! GPU material encoding tests: [`GpuGemMaterial::encode`] must place each CPU
//! material field in the slot the WGSL side reads it from.

use crate::{
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor, BandShape},
        dispersion::DispersionModel,
        materials::GemMaterial,
    },
    renderer::buffers::{
        GpuGemMaterial, MAX_ABSORPTION_BANDS, band_shape, crystal_system, dispersion_model_type,
        optical_character,
    },
};
use glam::Vec3;

/// A uniaxial material with every encoded field set to a value that is distinct from
/// every other field and from the zero default, so a swapped or dropped slot shows up.
fn probe_material() -> GemMaterial {
    let mut material = GemMaterial::by_name("Zircon").expect("Zircon must be a built-in");
    material.dispersion = DispersionModel::Cauchy {
        a: 1.5,
        b: 0.01,
        c: 0.0,
    };
    material.birefringence_delta = 0.05;
    material.c_axis = Vec3::X;
    material.absorption = AbsorptionTensor::uniaxial(
        vec![
            AbsorptionBand::new(500.0, 10.0, 1.0),
            AbsorptionBand::new(600.0, 20.0, 2.0),
        ],
        vec![
            AbsorptionBand::new(510.0, 11.0, 1.5),
            AbsorptionBand::new(610.0, 21.0, 2.5),
            AbsorptionBand {
                center_nm: 700.0,
                width_nm: 300.0,
                peak: 3.5,
                shape: BandShape::GaussianEnergy,
            },
        ],
    );
    material
}

/// The ordinary and extraordinary band sets land in their own slot arrays with their own
/// counts (2 ordinary, 3 extraordinary), in order, with the shape discriminant carried and
/// every unused slot left at the zero-peak default; the optional third set stays absent.
#[test]
fn encode_places_the_o_and_e_band_sets_in_their_own_slots() {
    let gpu = GpuGemMaterial::encode(&probe_material());

    assert_eq!(gpu.o_ray_band_count, 2);
    assert_eq!(gpu.e_ray_band_count, 3);
    assert_eq!(gpu.is_pleochroic, 1);
    assert_eq!(gpu.has_beta_ray, 0);
    assert_eq!(gpu.beta_ray_band_count, 0);

    let o = &gpu.o_ray_bands;
    assert_eq!(
        (o[0].center_nm, o[0].width_nm, o[0].peak),
        (500.0, 10.0, 1.0)
    );
    assert_eq!(
        (o[1].center_nm, o[1].width_nm, o[1].peak),
        (600.0, 20.0, 2.0)
    );
    assert_eq!(o[0].shape, band_shape::GAUSSIAN_WAVELENGTH);
    assert_eq!(o[2].peak, 0.0, "slot past the count is an empty band");

    let e = &gpu.e_ray_bands;
    assert_eq!(
        (e[0].center_nm, e[0].width_nm, e[0].peak),
        (510.0, 11.0, 1.5)
    );
    assert_eq!(
        (e[1].center_nm, e[1].width_nm, e[1].peak),
        (610.0, 21.0, 2.5)
    );
    assert_eq!(
        (e[2].center_nm, e[2].width_nm, e[2].peak),
        (700.0, 300.0, 3.5)
    );
    assert_eq!(e[2].shape, band_shape::GAUSSIAN_ENERGY);
    assert_eq!(e[3].peak, 0.0, "slot past the count is an empty band");
}

/// The dispersion curve, crystal class, optical character and optic axis land in the
/// discriminant and parameter slots the shader decodes: Cauchy `(a, b, c)` in `param_a`,
/// the axis and birefringence packed as `[x, y, z, delta]`, tetragonal uniaxial-positive.
#[test]
fn encode_places_dispersion_axis_and_class_in_their_slots() {
    let gpu = GpuGemMaterial::encode(&probe_material());

    assert_eq!(gpu.dispersion.model_type, dispersion_model_type::CAUCHY);
    assert_eq!(gpu.dispersion.param_a, [1.5, 0.01, 0.0, 0.0]);
    assert_eq!(
        gpu.dispersion.c_axis_and_birefringence,
        [1.0, 0.0, 0.0, 0.05]
    );
    assert_eq!(gpu.dispersion.is_anisotropic, 1);
    assert_eq!(gpu.dispersion.has_biaxial_delta, 0);
    assert_eq!(gpu.crystal_system, crystal_system::TETRAGONAL);
    assert_eq!(gpu.optical_character, optical_character::UNIAXIAL_POSITIVE);
    assert_eq!(gpu.has_extraordinary_dispersion, 0);
}

/// A band set longer than the GPU capacity is truncated to [`MAX_ABSORPTION_BANDS`]
/// entries and the stored count never exceeds the array length.
#[test]
fn encode_truncates_an_over_long_band_set_to_the_gpu_capacity() {
    let mut material = probe_material();
    let too_many: Vec<AbsorptionBand> = (0..MAX_ABSORPTION_BANDS + 2)
        .map(|i| AbsorptionBand::new((450 + 10 * i) as f32, 5.0, 1.0))
        .collect();
    material.absorption = AbsorptionTensor::isotropic(too_many);

    let gpu = GpuGemMaterial::encode(&material);

    assert_eq!(gpu.o_ray_band_count as usize, MAX_ABSORPTION_BANDS);
    assert_eq!(gpu.e_ray_band_count as usize, MAX_ABSORPTION_BANDS);
    let last = &gpu.o_ray_bands[MAX_ABSORPTION_BANDS - 1];
    assert_eq!(
        last.center_nm,
        (450 + 10 * (MAX_ABSORPTION_BANDS - 1)) as f32
    );
}
