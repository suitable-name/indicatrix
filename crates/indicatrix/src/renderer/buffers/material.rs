//! Material encoding: [`DispersionParams`] (one dispersion curve) and [`GpuGemMaterial`]
//! (a full `optics::materials::GemMaterial`), plus their `encode` constructors and the
//! discriminant modules ([`dispersion_model_type`]/[`band_shape`]/[`crystal_system`]/
//! [`optical_character`]) both sides must agree on.

use core::mem::offset_of;

use crate::optics::{
    absorption::{AbsorptionBand, BandShape},
    dispersion::DispersionModel,
    materials::{CrystalSystem, GemMaterial, OpticalCharacter},
};

/// One dispersion curve (`optics::dispersion::DispersionModel`), GPU-encoded.
///
/// `model_type` selects the interpretation of `param_a`/`param_b`:
/// - `0` (Sellmeier1 `{b1, c1}`): `param_a[0] = b1`, `param_b[0] = c1`.
/// - `1` (Sellmeier3 `{b: [f32;3], c: [f32;3]}`): `param_a[0..3] = b`, `param_b[0..3] = c`.
/// - `2` (Cauchy `{a, b, c}`): `param_a[0..3] = [a, b, c]`.
///
/// `c_axis_and_birefringence.xyz` is `GemMaterial::c_axis`; `.w` is
/// `GemMaterial::birefringence_delta`. `biaxial_delta_beta_alpha` /
/// `has_biaxial_delta` mirror `GemMaterial::biaxial_delta_beta_alpha: Option<f32>`
/// (`has_biaxial_delta != 0` <=> `Some`).
///
/// # Layout
///
/// The field after `model_type` needs 16-byte alignment (every `param_*`/
/// `c_axis_and_birefringence` field is a `vec4<f32>`), so `_pad_after_model_type`
/// reproduces WGSL's implicit 12-byte padding. `param_a` through
/// `c_axis_and_birefringence` pack back-to-back at 16 bytes each; the trailing three
/// scalars pack tightly, and `_pad_tail` reproduces WGSL's final 4-byte struct-size
/// rounding.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DispersionParams {
    pub model_type: u32,
    _pad_after_model_type: [u32; 3],
    pub param_a: [f32; 4],
    pub param_b: [f32; 4],
    pub param_c: [f32; 4],
    pub c_axis_and_birefringence: [f32; 4],
    pub is_anisotropic: u32,
    pub biaxial_delta_beta_alpha: f32,
    pub has_biaxial_delta: u32,
    _pad_tail: f32,
}

const _: () = {
    assert!(offset_of!(DispersionParams, model_type) == 0);
    assert!(offset_of!(DispersionParams, param_a) == 16);
    assert!(offset_of!(DispersionParams, param_b) == 32);
    assert!(offset_of!(DispersionParams, param_c) == 48);
    assert!(offset_of!(DispersionParams, c_axis_and_birefringence) == 64);
    assert!(offset_of!(DispersionParams, is_anisotropic) == 80);
    assert!(offset_of!(DispersionParams, biaxial_delta_beta_alpha) == 84);
    assert!(offset_of!(DispersionParams, has_biaxial_delta) == 88);
    assert!(offset_of!(DispersionParams, _pad_tail) == 92);
    assert!(size_of::<DispersionParams>() == 96);
};

/// `model_type` discriminants for [`DispersionParams`] -- must match
/// `renderer/shaders/layout_echo.wgsl` and (eventually) any real dispersion-evaluating
/// kernel.
pub mod dispersion_model_type {
    pub const SELLMEIER1: u32 = 0;
    pub const SELLMEIER3: u32 = 1;
    pub const CAUCHY: u32 = 2;
}

/// Hard cap on how many [`GpuAbsorptionBand`]s either eigenmode of a [`GpuGemMaterial`]
/// can carry.
///
/// `GemMaterial::absorption`'s `Vec<AbsorptionBand>` is unbounded on the CPU side, but a
/// GPU encoding needs a fixed-capacity array. Enforced on scene ingest by
/// `apps/indicatrix-worker/src/validate.rs`'s `validate_scene`, so a scene that would
/// silently truncate on the GPU is rejected before it ever gets there.
///
/// 8 is comfortably above every built-in material's real band count (the widest is 3,
/// `legacy_rgb_bands`), while staying small enough that the fixed array costs nothing
/// worth measuring for materials that use far fewer.
pub const MAX_ABSORPTION_BANDS: usize = 8;

/// One Gaussian absorption band (`optics::absorption::AbsorptionBand`), GPU-encoded.
///
/// # Layout
///
/// All four fields are 4-byte-aligned scalars (`shape` a `u32`), so this struct's WGSL
/// alignment is 4 (no vec3/vec4 field to trigger the usual pitfall) and its size is
/// exactly 16 bytes with no padding, matching Rust's natural layout. This is why
/// [`GpuGemMaterial`]'s band arrays are safe as plain `[GpuAbsorptionBand;
/// MAX_ABSORPTION_BANDS]` with no per-element padding: a storage buffer's array-stride
/// rule only requires a multiple of the element's own alignment (4), unlike a *uniform*
/// buffer's array, which would require a multiple of 16 -- [`GpuGemMaterial`] is bound
/// as storage specifically because of this.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuAbsorptionBand {
    pub center_nm: f32,
    pub width_nm: f32,
    pub peak: f32,
    /// Which domain this band is Gaussian in -- see [`band_shape`] for the
    /// discriminants, mirroring `optics::absorption::BandShape`.
    pub shape: u32,
}

const _: () = {
    assert!(offset_of!(GpuAbsorptionBand, center_nm) == 0);
    assert!(offset_of!(GpuAbsorptionBand, width_nm) == 4);
    assert!(offset_of!(GpuAbsorptionBand, peak) == 8);
    assert!(offset_of!(GpuAbsorptionBand, shape) == 12);
    assert!(size_of::<GpuAbsorptionBand>() == 16);
};

/// `optics::absorption::BandShape` discriminants for [`GpuAbsorptionBand::shape`]. Must
/// match `renderer/shaders/transport_physics.wgsl`'s `spectral_absorption`'s own
/// `band.shape == 1u` branch.
pub mod band_shape {
    pub const GAUSSIAN_WAVELENGTH: u32 = 0;
    pub const GAUSSIAN_ENERGY: u32 = 1;
}

/// `crystal_system` discriminants for [`GpuGemMaterial`].
///
/// Must match `renderer/shaders/layout_echo.wgsl`'s and (eventually) any real
/// material-evaluating kernel's own numbering. Order matches
/// `optics::materials::CrystalSystem`'s own declaration order.
pub mod crystal_system {
    pub const CUBIC: u32 = 0;
    pub const TETRAGONAL: u32 = 1;
    pub const HEXAGONAL: u32 = 2;
    pub const TRIGONAL: u32 = 3;
    pub const ORTHORHOMBIC: u32 = 4;
    pub const MONOCLINIC: u32 = 5;
    pub const TRICLINIC: u32 = 6;
}
/// `optical_character` discriminants for [`GpuGemMaterial`].
///
/// Must match `renderer/shaders/layout_echo.wgsl`'s and (eventually) any real
/// material-evaluating kernel's own numbering. Order matches
/// `optics::materials::OpticalCharacter`'s own declaration order.
pub mod optical_character {
    pub const ISOTROPIC: u32 = 0;
    pub const UNIAXIAL_POSITIVE: u32 = 1;
    pub const UNIAXIAL_NEGATIVE: u32 = 2;
    pub const BIAXIAL_POSITIVE: u32 = 3;
    pub const BIAXIAL_NEGATIVE: u32 = 4;
}

/// A full `optics::materials::GemMaterial`, GPU-encoded.
///
/// [`DispersionParams`] plus crystal/optical-character discriminants and both
/// eigenmodes' absorption band sets (flattened to [`MAX_ABSORPTION_BANDS`]-capacity
/// arrays with an explicit count, per this crate's Phase-0 plan).
///
/// # Layout
///
/// `dispersion` is 96 bytes (a multiple of its own 16-byte WGSL alignment), occupying
/// 0..96 with no leading padding. `crystal_system` through `e_ray_band_count` are five
/// 4-byte-aligned `u32`s packing at 96..116. `o_ray_bands`/`e_ray_bands` (alignment 4)
/// need only a 4-byte offset, so 116 already qualifies; each is now
/// `MAX_ABSORPTION_BANDS` (8) * 16 bytes = 128 bytes (see [`GpuAbsorptionBand`]'s own
/// Layout doc comment for why its size grew from 12 to 16), so `o_ray_bands` occupies
/// 116..244 and `e_ray_bands` 244..372. `scattering_sigma_s` through
/// `edge_rounding_radius` pack at 372..384.
///
/// Every field from `has_beta_ray` onward is APPENDED at the end rather than inserted
/// alongside its logical sibling, so every earlier field keeps its offset:
/// `has_beta_ray`/`beta_ray_band_count` (384..392), `beta_ray_bands` (392..520, the same
/// 128-byte band-array size as `o_ray_bands`/`e_ray_bands`), `absorption_path_scale`
/// (520..524), `has_extraordinary_dispersion`/`extraordinary_model_type` (524..532,
/// mirroring `GemMaterial::uniaxial_extraordinary_dispersion: Option<DispersionModel>`).
/// `extraordinary_param_a`/`extraordinary_param_b` encode that model like
/// [`DispersionParams::encode`]'s own `param_a`/`param_b`, and being `vec4<f32>` need
/// 16-byte alignment: `_pad_before_extraordinary_params` (532..544) reproduces WGSL's
/// implicit padding to get there; they then occupy 544..576, already 16-byte aligned,
/// with no trailing pad.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuGemMaterial {
    pub dispersion: DispersionParams,
    pub crystal_system: u32,
    pub optical_character: u32,
    pub is_pleochroic: u32,
    pub o_ray_band_count: u32,
    pub e_ray_band_count: u32,
    pub o_ray_bands: [GpuAbsorptionBand; MAX_ABSORPTION_BANDS],
    pub e_ray_bands: [GpuAbsorptionBand; MAX_ABSORPTION_BANDS],
    /// Inclusion/subsurface scattering: mirrors
    /// `optics::materials::GemMaterial::scattering_sigma_s`/`scattering_g` exactly.
    pub scattering_sigma_s: f32,
    pub scattering_g: f32,
    /// Facet edge rounding: mirrors
    /// `optics::materials::GemMaterial::edge_rounding_radius` exactly.
    pub edge_rounding_radius: f32,
    /// Mirrors `AbsorptionTensor::beta_ray`'s presence (`beta_ray.is_some()`).
    /// `beta_ray_band_count`/`beta_ray_bands` are meaningless when this is 0;
    /// distinguishes "no third band set" from "a third band set with zero bands", same
    /// as `Option<Vec<AbsorptionBand>>` on the CPU side.
    pub has_beta_ray: u32,
    /// The third, `n_beta`, principal direction's absorption bands' count -- see
    /// `has_beta_ray`.
    pub beta_ray_band_count: u32,
    /// The third, `n_beta`, principal direction's absorption bands -- see
    /// `has_beta_ray`. Mirrors `o_ray_bands`/`e_ray_bands`'s fixed-capacity encoding.
    pub beta_ray_bands: [GpuAbsorptionBand; MAX_ABSORPTION_BANDS],
    /// Mirrors `GemMaterial::absorption_path_scale` -- every model-unit length entering
    /// Beer-Lambert absorption or the scattering estimator is multiplied by this before
    /// use, both in the CPU tracer and in `spectral_transport.wgsl`'s mirrored blocks.
    pub absorption_path_scale: f32,
    /// Whether `extraordinary_param_a`/`extraordinary_param_b` carry a genuine
    /// independent extraordinary-ray dispersion curve -- mirrors
    /// `uniaxial_extraordinary_dispersion.is_some()`. Zero means the shader falls back to
    /// the constant-offset approximation `n_o + birefringence_delta` -- see
    /// `spectral_transport.wgsl`'s `extraordinary_index_at`.
    pub has_extraordinary_dispersion: u32,
    /// The extraordinary-ray curve's [`DispersionModel`] variant, using the SAME
    /// [`dispersion_model_type`] discriminants as `dispersion.model_type`. Meaningless
    /// when `has_extraordinary_dispersion == 0`.
    pub extraordinary_model_type: u32,
    /// Explicit padding reproducing the implicit bytes WGSL inserts before
    /// `extraordinary_param_a` (a `vec4<f32>`) -- see this struct's Layout doc comment.
    _pad_before_extraordinary_params: [u32; 3],
    /// The extraordinary-ray curve's `param_a`, encoded like [`DispersionParams::param_a`].
    /// Meaningless when `has_extraordinary_dispersion == 0`.
    pub extraordinary_param_a: [f32; 4],
    /// The extraordinary-ray curve's `param_b`, encoded like [`DispersionParams::param_b`].
    /// Meaningless when `has_extraordinary_dispersion == 0`.
    pub extraordinary_param_b: [f32; 4],
}

const _: () = {
    assert!(offset_of!(GpuGemMaterial, dispersion) == 0);
    assert!(offset_of!(GpuGemMaterial, crystal_system) == 96);
    assert!(offset_of!(GpuGemMaterial, optical_character) == 100);
    assert!(offset_of!(GpuGemMaterial, is_pleochroic) == 104);
    assert!(offset_of!(GpuGemMaterial, o_ray_band_count) == 108);
    assert!(offset_of!(GpuGemMaterial, e_ray_band_count) == 112);
    assert!(offset_of!(GpuGemMaterial, o_ray_bands) == 116);
    assert!(offset_of!(GpuGemMaterial, e_ray_bands) == 244);
    assert!(offset_of!(GpuGemMaterial, scattering_sigma_s) == 372);
    assert!(offset_of!(GpuGemMaterial, scattering_g) == 376);
    assert!(offset_of!(GpuGemMaterial, edge_rounding_radius) == 380);
    assert!(offset_of!(GpuGemMaterial, has_beta_ray) == 384);
    assert!(offset_of!(GpuGemMaterial, beta_ray_band_count) == 388);
    assert!(offset_of!(GpuGemMaterial, beta_ray_bands) == 392);
    assert!(offset_of!(GpuGemMaterial, absorption_path_scale) == 520);
    assert!(offset_of!(GpuGemMaterial, has_extraordinary_dispersion) == 524);
    assert!(offset_of!(GpuGemMaterial, extraordinary_model_type) == 528);
    assert!(offset_of!(GpuGemMaterial, extraordinary_param_a) == 544);
    assert!(offset_of!(GpuGemMaterial, extraordinary_param_b) == 560);
    assert!(size_of::<GpuGemMaterial>() == 576);
};

/// Encodes a CPU `optics::materials::GemMaterial` into a [`GpuGemMaterial`] for upload.
///
/// Never a hand-copied duplicate of the material data: every field read here is the SAME
/// field `optics::raytracer::trace_spectral_ray` itself reads. `material.absorption.beta_ray`
/// (the optional third, trichroic band set) is encoded into
/// `has_beta_ray`/`beta_ray_band_count`/`beta_ray_bands`, consumed by the shader's
/// genuinely biaxial absorption path whenever `dispersion.has_biaxial_delta != 0`.
impl GpuGemMaterial {
    /// # Panics
    ///
    /// Panics if `material` has more than [`MAX_ABSORPTION_BANDS`] bands in either
    /// eigenmode's `Vec<AbsorptionBand>` -- every built-in material has at most 3, so
    /// this is only reachable for a hand-constructed test material; acceptable to
    /// panic in this self-test-only encoder rather than silently truncate a band set.
    #[must_use]
    pub fn encode(material: &GemMaterial) -> Self {
        let crystal_system_val = match material.crystal_system {
            CrystalSystem::Cubic => crystal_system::CUBIC,
            CrystalSystem::Tetragonal => crystal_system::TETRAGONAL,
            CrystalSystem::Hexagonal => crystal_system::HEXAGONAL,
            CrystalSystem::Trigonal => crystal_system::TRIGONAL,
            CrystalSystem::Orthorhombic => crystal_system::ORTHORHOMBIC,
            CrystalSystem::Monoclinic => crystal_system::MONOCLINIC,
            CrystalSystem::Triclinic => crystal_system::TRICLINIC,
        };
        let optical_character_val = match material.optical_character {
            OpticalCharacter::Isotropic => optical_character::ISOTROPIC,
            OpticalCharacter::UniaxialPositive => optical_character::UNIAXIAL_POSITIVE,
            OpticalCharacter::UniaxialNegative => optical_character::UNIAXIAL_NEGATIVE,
            OpticalCharacter::BiaxialPositive => optical_character::BIAXIAL_POSITIVE,
            OpticalCharacter::BiaxialNegative => optical_character::BIAXIAL_NEGATIVE,
        };

        let (o_ray_bands, o_ray_band_count) = encode_bands(&material.absorption.o_ray);
        let (e_ray_bands, e_ray_band_count) = encode_bands(&material.absorption.e_ray);
        let (beta_ray_bands, beta_ray_band_count) = material
            .absorption
            .beta_ray
            .as_deref()
            .map_or_else(empty_bands, encode_bands);

        // None (every built-in except Quartz/Amethyst/Citrine) encodes to
        // has_extraordinary_dispersion == 0 and an all-zero curve the shader never reads.
        let (
            has_extraordinary_dispersion,
            extraordinary_model_type,
            extraordinary_param_a,
            extraordinary_param_b,
        ) = material.uniaxial_extraordinary_dispersion.map_or(
            (0u32, 0u32, [0.0f32; 4], [0.0f32; 4]),
            |e_dispersion| {
                let (model_type, param_a, param_b) = encode_dispersion_model(&e_dispersion);
                (1u32, model_type, param_a, param_b)
            },
        );

        Self {
            dispersion: DispersionParams::encode(material),
            crystal_system: crystal_system_val,
            optical_character: optical_character_val,
            is_pleochroic: u32::from(material.absorption.is_pleochroic),
            o_ray_band_count,
            e_ray_band_count,
            o_ray_bands,
            e_ray_bands,
            scattering_sigma_s: material.scattering_sigma_s,
            scattering_g: material.scattering_g,
            edge_rounding_radius: material.edge_rounding_radius,
            has_beta_ray: u32::from(material.absorption.beta_ray.is_some()),
            beta_ray_band_count,
            beta_ray_bands,
            absorption_path_scale: material.absorption_path_scale,
            has_extraordinary_dispersion,
            extraordinary_model_type,
            _pad_before_extraordinary_params: [0; 3],
            extraordinary_param_a,
            extraordinary_param_b,
        }
    }
}

/// Encodes one `optics::dispersion::DispersionModel` into its `(model_type, param_a,
/// param_b)` GPU representation -- shared by [`DispersionParams::encode`] (the
/// material's primary curve) and [`GpuGemMaterial::encode`] (the optional extraordinary
/// curve), so the two never independently drift on the mapping.
#[must_use]
const fn encode_dispersion_model(model: &DispersionModel) -> (u32, [f32; 4], [f32; 4]) {
    match *model {
        DispersionModel::Sellmeier1 { b1, c1 } => (
            dispersion_model_type::SELLMEIER1,
            [b1, 0.0, 0.0, 0.0],
            [c1, 0.0, 0.0, 0.0],
        ),
        DispersionModel::Sellmeier3 { b, c } => (
            dispersion_model_type::SELLMEIER3,
            [b[0], b[1], b[2], 0.0],
            [c[0], c[1], c[2], 0.0],
        ),
        DispersionModel::Cauchy { a, b, c } => {
            (dispersion_model_type::CAUCHY, [a, b, c, 0.0], [0.0; 4])
        }
    }
}

impl DispersionParams {
    #[must_use]
    fn encode(material: &GemMaterial) -> Self {
        let (model_type, param_a, param_b) = encode_dispersion_model(&material.dispersion);

        let c_axis = material.c_axis;
        Self {
            model_type,
            _pad_after_model_type: [0; 3],
            param_a,
            param_b,
            param_c: [0.0; 4],
            c_axis_and_birefringence: [c_axis.x, c_axis.y, c_axis.z, material.birefringence_delta],
            is_anisotropic: u32::from(
                material.crystal_system != CrystalSystem::Cubic
                    && material.birefringence_delta.abs() > 1e-4,
            ),
            biaxial_delta_beta_alpha: material.biaxial_delta_beta_alpha.unwrap_or(0.0),
            has_biaxial_delta: u32::from(material.biaxial_delta_beta_alpha.is_some()),
            _pad_tail: 0.0,
        }
    }
}

/// The all-zero-bands, zero-count encoding used for `beta_ray_bands` when
/// `material.absorption.beta_ray` is `None` -- mirrors [`encode_bands`]'s default slot
/// values (`width_nm: 1.0`, else `0.0`) so an unused slot is never a degenerate
/// zero-width Gaussian, even though `peak` is always `0.0` regardless.
pub(super) const fn empty_bands() -> ([GpuAbsorptionBand; MAX_ABSORPTION_BANDS], u32) {
    (
        [GpuAbsorptionBand {
            center_nm: 0.0,
            width_nm: 1.0,
            peak: 0.0,
            shape: band_shape::GAUSSIAN_WAVELENGTH,
        }; MAX_ABSORPTION_BANDS],
        0,
    )
}

/// Encodes a `Vec<AbsorptionBand>` into a fixed [`MAX_ABSORPTION_BANDS`]-capacity array
/// plus its real length, panicking (see [`GpuGemMaterial::encode`]'s doc comment) if the
/// source has more bands than the GPU encoding can hold.
pub(super) fn encode_bands(
    bands: &[AbsorptionBand],
) -> ([GpuAbsorptionBand; MAX_ABSORPTION_BANDS], u32) {
    assert!(
        bands.len() <= MAX_ABSORPTION_BANDS,
        "material has {} absorption bands, exceeding MAX_ABSORPTION_BANDS ({MAX_ABSORPTION_BANDS})",
        bands.len()
    );
    let mut out = [GpuAbsorptionBand {
        center_nm: 0.0,
        width_nm: 1.0,
        peak: 0.0,
        shape: band_shape::GAUSSIAN_WAVELENGTH,
    }; MAX_ABSORPTION_BANDS];
    for (slot, band) in out.iter_mut().zip(bands.iter()) {
        *slot = GpuAbsorptionBand {
            center_nm: band.center_nm,
            width_nm: band.width_nm,
            peak: band.peak,
            shape: match band.shape {
                BandShape::GaussianWavelength => band_shape::GAUSSIAN_WAVELENGTH,
                BandShape::GaussianEnergy => band_shape::GAUSSIAN_ENERGY,
            },
        };
    }
    (out, bands.len() as u32)
}
