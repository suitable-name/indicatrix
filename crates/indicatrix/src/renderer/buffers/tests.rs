//! CPU-side pins for the WGSL layout invariants documented across this module tree, plus
//! the material/facet-finish encoding functions' mapping behaviour.

use core::mem::offset_of;

use super::{
    CameraUniform, DispersionParams, GpuAbsorptionBand, GpuGemMaterial, GpuTransportParams,
    GpuWavefrontParams, band_shape, encode_facet_finishes, facet_finish,
    material::{empty_bands, encode_bands},
};
use crate::optics::{absorption::AbsorptionBand, raytracer::FacetFinish};

/// `bytemuck::Pod` guards against uninitialized padding bytes but says nothing about
/// WGSL offset rules -- that's what the `offset_of!` assertions in each sibling module
/// are for. This pins the documented sizes as an ordinary `#[test]` too, so a failure
/// prints a named result instead of a compile error buried in this module tree.
#[test]
fn struct_sizes_match_documented_wgsl_layout() {
    assert_eq!(size_of::<CameraUniform>(), 112);
    assert_eq!(size_of::<DispersionParams>(), 96);
    assert_eq!(size_of::<GpuAbsorptionBand>(), 16);
    assert_eq!(size_of::<GpuGemMaterial>(), 576);
    assert_eq!(size_of::<GpuTransportParams>(), 80);
    assert_eq!(size_of::<GpuWavefrontParams>(), 16);
}

/// [`offset_of!`] pinned as ordinary `#[test]`s too (see the comment above), for
/// `GpuAbsorptionBand` and `GpuGemMaterial`: a wrong offset here is exactly the
/// "looks right, isn't" bug class `renderer::gpu::layout_check` exists to catch
/// on the GPU side, but a
/// plain assertion catches the CPU-side half of it for free on every `cargo test`.
#[test]
fn gpu_absorption_band_offsets_match_documented_wgsl_layout() {
    assert_eq!(offset_of!(GpuAbsorptionBand, center_nm), 0);
    assert_eq!(offset_of!(GpuAbsorptionBand, width_nm), 4);
    assert_eq!(offset_of!(GpuAbsorptionBand, peak), 8);
    assert_eq!(offset_of!(GpuAbsorptionBand, shape), 12);
}

#[test]
fn gpu_gem_material_band_array_offsets_match_documented_wgsl_layout() {
    assert_eq!(offset_of!(GpuGemMaterial, o_ray_bands), 116);
    assert_eq!(offset_of!(GpuGemMaterial, e_ray_bands), 244);
    assert_eq!(offset_of!(GpuGemMaterial, scattering_sigma_s), 372);
    assert_eq!(offset_of!(GpuGemMaterial, beta_ray_bands), 392);
    assert_eq!(offset_of!(GpuGemMaterial, absorption_path_scale), 520);
}

/// `material::encode_bands` must map each [`crate::optics::absorption::BandShape`]
/// variant to the matching [`band_shape`] discriminant, and leave every other field
/// untouched -- the bug this guards against is the shape ending up defaulted (always
/// `GAUSSIAN_WAVELENGTH`) regardless of what the CPU-side band actually specified,
/// which would silently make `GaussianEnergy` materials render correctly on the CPU
/// but not on the GPU -- the exact bug this field exists to fix.
#[test]
fn encode_bands_maps_shape_correctly() {
    let bands = vec![
        AbsorptionBand::new(550.0, 20.0, 2.0),
        AbsorptionBand::energy(620.0, 200.0, 1.5),
    ];
    let (encoded, count) = encode_bands(&bands);
    assert_eq!(count, 2);
    assert_eq!(encoded[0].shape, band_shape::GAUSSIAN_WAVELENGTH);
    assert_eq!(encoded[1].shape, band_shape::GAUSSIAN_ENERGY);
    // Untouched slots keep the wavelength-domain default.
    assert_eq!(encoded[2].shape, band_shape::GAUSSIAN_WAVELENGTH);

    let (empty, empty_count) = empty_bands();
    assert_eq!(empty_count, 0);
    assert!(
        empty
            .iter()
            .all(|b| b.shape == band_shape::GAUSSIAN_WAVELENGTH)
    );
}

/// [`encode_facet_finishes`]'s default-fallback semantics must match
/// `trace_spectral_ray_with_finish`'s lookup exactly: an empty slice, or one shorter
/// than `num_planes`, defaults every uncovered index to `facet_finish::POLISHED`.
#[test]
fn encode_facet_finishes_defaults_to_polished() {
    assert_eq!(encode_facet_finishes(&[], 4), vec![0, 0, 0, 0]);

    let finishes = vec![FacetFinish::Frosted, FacetFinish::Polished];
    assert_eq!(
        encode_facet_finishes(&finishes, 4),
        vec![
            facet_finish::FROSTED,
            facet_finish::POLISHED,
            facet_finish::POLISHED,
            facet_finish::POLISHED,
        ]
    );
}
