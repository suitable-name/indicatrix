//! Gemstone optical material data: [`GemMaterial`] (dispersion, birefringence,
//! pleochroic absorption, inclusion scattering) and the built-in material table.
//!
//! Split from a single `materials.rs` by responsibility: this file holds the type
//! definitions and [`GemMaterial::all_materials`]'s aggregation; each
//! `built_in_materials_*`/`built_in_material_*` builder function that actually
//! constructs the built-in table lives in its own file, named for the species it
//! covers; [`custom`] holds the builder-style constructors
//! ([`GemMaterial::new_custom`], `with_*`), [`body_color`] holds the body-color
//! preset table [`GemMaterial::with_body_color`] is fed from, [`lookup`] holds [`GemMaterial::by_name`]
//! and the convenience accessors, and [`optics`] holds the biaxial-indicatrix/
//! extraordinary-index/GPU-routing methods. Every path reachable as `materials::X`
//! before the split is still reachable at exactly that path: [`GemMaterial`],
//! [`CrystalSystem`] and [`OpticalCharacter`] are still defined directly here.

use super::{absorption::AbsorptionTensor, dispersion::DispersionModel};
use glam::Vec3;

mod amethyst_through_citrine;
mod andalusite_through_glass;
mod aquamarine_through_citrine;
pub mod body_color;
mod custom;
mod diamond_through_emerald;
mod garnets_grossular_and_andradite;
mod garnets_pyrope_through_spessartine;
mod lookup;
mod optics;
mod peridot_through_benitoite;
mod rutile;
mod spinel_through_tourmaline;
mod tanzanite_through_cubic_zirconia;
#[cfg(test)]
mod tests;
mod zircon_through_topaz;

/// A gemstone material's crystallographic system.
///
/// [`OpticalCharacter`] selects the birefringence regime, but the crystal system also
/// gates the optics path: a material is treated as anisotropic only when its system is
/// not `Cubic` AND `|birefringence_delta| > 1e-4` (the `is_anisotropic` field built by
/// `build_ray_material_context` in `optics::raytracer::transport::inner`, mirrored by
/// `DispersionParams` in `renderer::buffers`' material encode). A `Cubic` system
/// therefore forces the isotropic path whatever the stored `birefringence_delta` says;
/// the same predicate is true for biaxial (orthorhombic) materials, which then branch
/// on `biaxial_delta_beta_alpha` instead of the uniaxial machinery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CrystalSystem {
    Cubic,
    Tetragonal,
    Hexagonal,
    Trigonal,
    Orthorhombic,
    Monoclinic,
    Triclinic,
}

/// Which birefringence regime a material's optics take.
///
/// None (isotropic), uniaxial with a positive or negative sign (`n_e` above or below
/// `n_o`), or biaxial with a positive or negative sign (the same convention applied to
/// `n_beta`'s position between `n_alpha` and `n_gamma`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum OpticalCharacter {
    Isotropic,
    UniaxialPositive,
    UniaxialNegative,
    BiaxialPositive,
    BiaxialNegative,
}

/// A gemstone material's full optical description.
///
/// Dispersion, birefringence, pleochroic absorption, and optional inclusion
/// scattering / edge rounding / path scale. Built by [`Self::all_materials`] for the
/// built-in table, or [`Self::new_custom`] for a caller-supplied one.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GemMaterial {
    /// Display name of the material.
    pub name: String,
    /// Crystal system of the material.
    pub crystal_system: CrystalSystem,
    /// Optical character (isotropic, uniaxial or biaxial).
    pub optical_character: OpticalCharacter,
    /// Wavelength-dependent refractive-index model.
    pub dispersion: DispersionModel,
    /// Birefringence `n_e - n_o` (or max - min for biaxial).
    pub birefringence_delta: f32,
    /// Absorption bands per eigenmode.
    pub absorption: AbsorptionTensor,
    /// Optical (crystallographic) c-axis direction, in crystal/model space. Uniaxial
    /// birefringence (`effective_extraordinary_index`, `extraordinary_poynting_dir`)
    /// is evaluated against this axis. Defaults to `Vec3::Y`. For a biaxial material
    /// (`biaxial_delta_beta_alpha.is_some()`) this doubles as the `n_gamma` principal
    /// axis -- see `biaxial_indicatrix`.
    pub c_axis: Vec3,
    /// `n_beta - n_alpha` at the sodium D line, for the three biaxial (orthorhombic)
    /// built-ins. `None` for every isotropic/uniaxial material, which keep using the
    /// `c_axis` + `birefringence_delta` uniaxial machinery.
    ///
    /// Convention: `birefringence_delta` is `n_gamma - n_alpha` for a biaxial entry, and
    /// the base `dispersion` curve is `n_beta(lambda)`, the middle principal index (see
    /// `biaxial_indicatrix`). This one extra scalar then places all three principal
    /// indices `n_alpha <= n_beta <= n_gamma`, treating the SPREAD between them as
    /// wavelength-independent (only the base curve disperses) -- the same
    /// achromatic-delta approximation the uniaxial `n_e = n_o + birefringence_delta`
    /// already makes.
    pub biaxial_delta_beta_alpha: Option<f32>,
    /// Inclusion/subsurface scattering: the homogeneous Henyey-Greenstein scattering
    /// coefficient (`sigma_s`, a physical, linear coefficient in inverse model units of
    /// path length -- no perceptual/logarithmic remapping) modeling silk, rutile
    /// needles, and clouds as a single averaged-out volumetric density.
    ///
    /// `0.0` (every built-in's own stored value, and `new_custom`'s) means no
    /// scattering medium: `raytracer::apply_absorption`'s deterministic Beer-Lambert
    /// path is taken unconditionally whenever this is `<= 0.0`. See
    /// `raytracer::maybe_scatter_or_extinguish` for the estimator this feeds once
    /// nonzero (extinction `sigma_t = sigma_a + sigma_s`, free-path distance sampling,
    /// single-scattering albedo `sigma_s / sigma_t`). Use [`Self::with_scattering`] (or
    /// [`Self::with_recommended_scattering`] for a per-species starting point) to opt a
    /// material into a nonzero value.
    ///
    /// # Useful range
    ///
    /// The built-in cuts (`geometry::StandardGemCuts`) have a girdle radius of order 1
    /// model unit, so a typical internal chord length is roughly 0.5-2 units and the
    /// mean free path between scattering events is `1/sigma_s`. `sigma_s` in roughly
    /// `0.05` (barely perceptible haze) to `3.0` (milky/heavily included) covers the
    /// visually meaningful range for this geometry scale.
    pub scattering_sigma_s: f32,
    /// The Henyey-Greenstein phase function's asymmetry parameter `g` in `(-1, 1)`: `0`
    /// is isotropic scattering, positive values forward-scatter (silk/rutile needle
    /// inclusions are usually forward-scattering), negative values back-scatter.
    /// Meaningless while `scattering_sigma_s <= 0.0`; defaults to `0.0` alongside it.
    /// See [`Self::DEFAULT_SCATTERING_G`] for a sensible default when a caller only
    /// wants to control the amount ([`Self::with_scattering_amount`]).
    pub scattering_g: f32,
    /// Facet edge rounding: the micron-scale rounding radius real meet-point edges
    /// have, in the same world units as `scattering_sigma_s` (girdle radius of order 1
    /// model unit) -- so `0.01` models an edge rounded over about 1% of the stone's
    /// scale, comfortably in the "throws a soft glint, does not visibly bevel the
    /// facet" range. `0.0` (every built-in) disables the effect entirely:
    /// `raytracer::shading_normal_near_edge` returns the flat facet normal unperturbed
    /// whenever this is `<= 0.0`. See [`Self::with_edge_rounding`] to opt in.
    pub edge_rounding_radius: f32,
    /// Model units to absorption-length units: every interior path length is
    /// multiplied by this before Beer-Lambert absorption and inclusion scattering.
    /// `1.0` (every built-in, and `new_custom`'s default) is a no-op. See
    /// [`Self::with_absorption_path_scale`] to opt a material into a different
    /// physical size (e.g. a larger or smaller real-world stone rendered at the same
    /// ~1-model-unit girdle radius as every built-in cut).
    pub absorption_path_scale: f32,
    /// An optional, genuinely wavelength-dependent extraordinary-ray dispersion curve
    /// for a uniaxial material, evaluated instead of the constant-offset approximation
    /// `n_e(lambda) = n_o(lambda) + birefringence_delta` (see
    /// [`Self::extraordinary_index_at`], the single place both are read). Real
    /// birefringence is not wavelength-flat, but modelling that needs the
    /// extraordinary ray's own independent dispersion curve. Four built-ins set it:
    /// Quartz, Amethyst and Citrine (all one `SiO2` host, from G. Ghosh 1999 -- see the
    /// Quartz entry's comment) and Rutile (`DeVore` 1951).
    ///
    /// `None` (every other built-in, and [`Self::new_custom`]'s default) falls back to
    /// `n_o + birefringence_delta` in `extraordinary_index_at`. Meaningless for an
    /// isotropic or biaxial material -- only `optics::raytracer::refraction`'s uniaxial
    /// per-channel index lookups read it.
    ///
    /// Threaded through to the GPU backend: `renderer::buffers::GpuGemMaterial` carries
    /// this curve as its own `has_extraordinary_dispersion`/`extraordinary_model_type`/
    /// `extraordinary_param_a`/`extraordinary_param_b` fields (via
    /// `renderer::buffers::encode_dispersion_model`). The WGSL twin of
    /// [`Self::extraordinary_index_at`] is the per-channel loop in
    /// `shaders/transport_bounce/08_bounce_step.wgsl` (and the hero seed in
    /// `09_finalize_and_ray_gen.wgsl`), which calls `extraordinary_dispersion_evaluate`
    /// (`shaders/transport_physics/03_dispersion_absorption_frosted.wgsl`, the same
    /// formula and `n >= 1.0` floor) when `has_extraordinary_dispersion != 0` and
    /// otherwise uses `n_o + birefringence_delta`. The standalone
    /// `per_channel_uniaxial_index` in that physics file takes only the constant-offset
    /// form and does not read this curve.
    pub uniaxial_extraordinary_dispersion: Option<DispersionModel>,
}

impl GemMaterial {
    /// The full built-in material table, aggregated from each species-range builder
    /// module (see this module's own doc comment for the split rationale).
    #[must_use]
    pub fn all_materials() -> Vec<Self> {
        let mut materials = Self::built_in_materials_diamond_through_emerald();
        materials.extend(Self::built_in_materials_zircon_through_topaz());
        materials.extend(Self::built_in_materials_spinel_through_tourmaline());
        materials.extend(Self::built_in_materials_tanzanite_through_cubic_zirconia());
        // Four more quarters of new species. See
        // [`Self::built_in_materials_aquamarine_through_citrine`]'s module-level note
        // for Sphene (left out) and [`Self::built_in_material_rutile`] for why Rutile
        // is included.
        materials.extend(Self::built_in_materials_aquamarine_through_citrine());
        materials.extend(Self::built_in_materials_amethyst_through_citrine());
        materials.extend(Self::built_in_materials_garnets_pyrope_through_spessartine());
        materials.extend(Self::built_in_materials_garnets_grossular_and_andradite());
        materials.extend(Self::built_in_materials_peridot_through_benitoite());
        materials.extend(Self::built_in_materials_andalusite_through_glass());
        materials.push(Self::built_in_material_rutile());
        materials
    }
}
