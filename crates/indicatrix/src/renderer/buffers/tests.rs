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
    #[cfg(not(feature = "zoning"))]
    assert_eq!(size_of::<GpuGemMaterial>(), 576);
    // With the zone table appended (see `renderer::buffers::zoning`).
    #[cfg(feature = "zoning")]
    assert_eq!(size_of::<GpuGemMaterial>(), 576 + 2128);
    assert_eq!(size_of::<GpuTransportParams>(), 112);
    assert_eq!(size_of::<GpuWavefrontParams>(), 16);
}

/// The transport uniform's glare slot sits at offset 72, defaults to the neutral `1.0`
/// and clamps what a caller sets.
#[test]
fn transport_params_surface_glare_slot_defaults_to_one_and_clamps() {
    assert_eq!(offset_of!(GpuTransportParams, surface_glare), 72);
    let base = GpuTransportParams::new(1, 1, 0, 1, 0.0, 6500.0, 1.0, 1.0, 0.0, 0.0, [1.0; 3]);
    assert_eq!(base.surface_glare.to_bits(), 1.0f32.to_bits());
    assert_eq!(
        base.with_surface_glare(0.4).surface_glare.to_bits(),
        0.4f32.to_bits()
    );
    assert_eq!(
        base.with_surface_glare(-1.0).surface_glare.to_bits(),
        0.0f32.to_bits()
    );
    assert_eq!(
        base.with_surface_glare(9.0).surface_glare.to_bits(),
        1.0f32.to_bits()
    );
}

/// The head-shadow cone slots sit at 76 / 80 and default to the 16 degree literals.
#[test]
fn transport_params_head_shadow_slots_default_to_the_16_degree_cone() {
    assert_eq!(offset_of!(GpuTransportParams, head_shadow_outer_cos), 76);
    assert_eq!(offset_of!(GpuTransportParams, head_shadow_inner_cos), 80);
    let base = GpuTransportParams::new(1, 1, 0, 1, 0.0, 6500.0, 1.0, 1.0, 0.0, 0.0, [1.0; 3]);
    assert_eq!(
        base.head_shadow_outer_cos.to_bits(),
        0.951_056_5f32.to_bits()
    );
    assert_eq!(
        base.head_shadow_inner_cos.to_bits(),
        0.970_295_7f32.to_bits()
    );
    let wide = base.with_head_shadow([0.5, 0.75]);
    assert_eq!(wide.head_shadow_outer_cos.to_bits(), 0.5f32.to_bits());
    assert_eq!(wide.head_shadow_inner_cos.to_bits(), 0.75f32.to_bits());
}

/// The four light-tent slots sit at 96..112 and default to the light tent's own values
/// (the identity of the tent formula); `with_tent` / `with_tent_of` carry a preset's.
#[test]
fn transport_params_tent_slots_default_to_the_light_tent() {
    use crate::optics::raytracer::{LightingPreset, TentParams};

    assert_eq!(offset_of!(GpuTransportParams, tent_walls), 96);
    assert_eq!(offset_of!(GpuTransportParams, tent_cards), 100);
    assert_eq!(offset_of!(GpuTransportParams, tent_spark), 104);
    assert_eq!(offset_of!(GpuTransportParams, tent_ground), 108);
    let base = GpuTransportParams::new(1, 1, 0, 1, 0.0, 6500.0, 1.0, 1.0, 0.0, 0.0, [1.0; 3]);
    assert_eq!(base.tent_walls.to_bits(), 1.0f32.to_bits());
    assert_eq!(base.tent_cards.to_bits(), 1.0f32.to_bits());
    assert_eq!(base.tent_spark.to_bits(), 1.0f32.to_bits());
    assert_eq!(base.tent_ground.to_bits(), 0.02f32.to_bits());
    assert_eq!(offset_of!(GpuTransportParams, tent_flat), 84);
    assert_eq!(base.tent_flat.to_bits(), 0.0f32.to_bits());
    let tray = base.with_tent_of(LightingPreset::WhiteTray.studio(1.0, 0.0, 0.0));
    let expected = LightingPreset::WhiteTray.params().tent;
    assert_eq!(tray.tent_walls.to_bits(), expected.walls.to_bits());
    assert_eq!(tray.tent_ground.to_bits(), expected.ground.to_bits());
    assert_eq!(tray.tent_flat.to_bits(), expected.flat.to_bits());
    // The light tent's own preset keeps the identity.
    let tent = base.with_tent_of(LightingPreset::LightTent.studio(1.0, 0.0, 0.0));
    assert_eq!(
        [
            tent.tent_walls,
            tent.tent_cards,
            tent.tent_spark,
            tent.tent_ground
        ],
        TentParams::DEFAULT.to_array()
    );
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
