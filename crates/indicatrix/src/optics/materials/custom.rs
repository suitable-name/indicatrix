//! Builder-style constructors and opt-in setters for [`super::GemMaterial`]:
//! [`super::GemMaterial::new_custom`] and the `with_*`/scattering-recommendation methods.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor, legacy_rgb_bands},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Creates a custom gemstone material with specified physical optical properties.
    ///
    /// # `dispersion_delta` convention: Fraunhofer F-C, not B-G
    ///
    /// `dispersion_delta` is interpreted as the Fraunhofer **F-C** interval,
    /// `n(486.1nm) - n(656.3nm)`, matching every built-in material in
    /// [`Self::all_materials`] (whose Cauchy/Sellmeier fits were, per their own sourcing
    /// comments, deliberately normalised to F-C -- gemological tables usually publish
    /// the wider Fraunhofer **B-G** interval, `n(430.8nm) - n(686.7nm)`, instead, and
    /// every one of those comments records converting B-G -> F-C before fitting).
    /// `new_custom` picks the same convention so a caller mixing a custom material into
    /// a scene with built-ins gets a consistent, comparable dispersion figure -- a
    /// caller with a genuine B-G figure in hand should convert it first (multiply by
    /// 0.579, the B-G -> F-C ratio every built-in's sourcing comments use, range
    /// 0.569-0.587; equivalently B-G is roughly 1.73 times F-C for typical gemstone
    /// Cauchy curves). The desktop
    /// material editor's preset templates carry F-C values measured from the matching
    /// built-in entries (Diamond 0.0256, Sapphire 0.0106, ...), so picking one
    /// reproduces that built-in's dispersion.
    ///
    /// Solved in closed form from the single-term Cauchy fit this constructor always
    /// builds (`n(lambda) = a + b / lambda_um^2`, `c = 0`): the F-C delta this produces
    /// is `b` times a fixed geometric factor (the difference of `1 / lambda^2` at F and
    /// C), so dividing the requested delta by that SAME factor for `b` reproduces the
    /// requested F-C delta exactly (up to floating-point rounding) rather than only
    /// approximately. Computed here from the wavelengths directly (not a hand-rounded
    /// literal) so the reciprocal-square arithmetic is exact regardless of how many
    /// digits get typed into a comment; evaluates to a factor of ~0.5235. A flat
    /// `0.347` multiplier, by contrast, has no derivation behind it and measurably
    /// under-delivers the requested F-C delta (only ~66% of it) while over-delivering
    /// a B-G-interpreted delta (~113% of it) -- i.e. it matches neither convention.
    ///
    /// See this module's own test
    /// `new_custom_dispersion_delta_measures_exactly_at_f_and_c` for the regression
    /// pinning this.
    #[must_use]
    pub fn new_custom(
        name: &str,
        mean_ri: f32,
        dispersion_delta: f32,
        birefringence_delta: f32,
        absorption_rgb: [f32; 3],
    ) -> Self {
        // Cauchy dispersion model fit: n(lambda) = A + B / lambda^2
        // where lambda is in um, lambda_D = 0.5893 um (sodium D line).
        const LAMBDA_D_UM: f32 = 0.5893;
        // Fraunhofer F (486.1nm) and C (656.3nm) lines, matching every other F-C
        // measurement in this crate (see `color::metrics::evaluate_gem_optical_metrics`
        // and `all_materials`' own verification comments, which evaluate at these exact
        // two wavelengths).
        const LAMBDA_F_UM: f32 = 0.4861;
        const LAMBDA_C_UM: f32 = 0.6563;
        let lambda_d_sq = LAMBDA_D_UM * LAMBDA_D_UM;
        let k_fc = 1.0 / (1.0 / (LAMBDA_F_UM * LAMBDA_F_UM) - 1.0 / (LAMBDA_C_UM * LAMBDA_C_UM));
        let b = (dispersion_delta * k_fc).max(0.0);
        let a = (mean_ri - b / lambda_d_sq).max(1.0);

        Self {
            name: name.to_string(),
            crystal_system: if birefringence_delta.abs() > 1e-4 {
                CrystalSystem::Trigonal
            } else {
                CrystalSystem::Cubic
            },
            optical_character: if birefringence_delta > 1e-4 {
                OpticalCharacter::UniaxialPositive
            } else if birefringence_delta < -1e-4 {
                OpticalCharacter::UniaxialNegative
            } else {
                OpticalCharacter::Isotropic
            },
            dispersion: DispersionModel::Cauchy { a, b, c: 0.0 },
            birefringence_delta,
            // `new_custom`'s public signature keeps accepting a plain
            // `[R, G, B]` triple (callers, including existing tests, pass one) --
            // internally converted to the band-set representation via
            // `legacy_rgb_bands` (see that function's doc comment for the three-lobe
            // shape it produces).
            absorption: AbsorptionTensor::isotropic(legacy_rgb_bands(absorption_rgb)),
            // No caller currently supplies a c-axis for a custom material, so
            // default to Vec3::Y, keeping `new_custom` behaviour unchanged rather
            // than adding a new parameter every call site would need to be updated
            // for.
            c_axis: Vec3::Y,
            // No caller currently supplies biaxial principal-index data
            // for a custom material -- `new_custom` remains a uniaxial/isotropic
            // constructor (see `optical_character`/`crystal_system` above, which never
            // produce Biaxial* for this constructor either).
            biaxial_delta_beta_alpha: None,
            scattering_sigma_s: 0.0,
            scattering_g: 0.0,
            edge_rounding_radius: 0.0,
            absorption_path_scale: 1.0,
            uniaxial_extraordinary_dispersion: None,
        }
    }

    /// Creates a custom gemstone material from a full dispersion model (Sellmeier with one
    /// or three terms, or Cauchy) instead of [`Self::new_custom`]'s refractive index plus
    /// `n_F - n_C` pair.
    ///
    /// The mean refractive index is the model's own [`DispersionModel::n_d`]. Everything
    /// else (crystal system and optical character from the sign of `birefringence_delta`,
    /// the isotropic absorption bands of `absorption_rgb`, no scattering) is exactly what
    /// `new_custom` builds for the same values, so the two constructors differ only in the
    /// dispersion curve.
    ///
    /// The model is used as given. Call [`DispersionModel::validate`] first: an unchecked
    /// curve is floored at an index of 1 rather than rejected, which renders a clear block.
    #[must_use]
    pub fn new_custom_with_dispersion(
        name: &str,
        dispersion: DispersionModel,
        birefringence_delta: f32,
        absorption_rgb: [f32; 3],
    ) -> Self {
        let mut material = Self::new_custom(
            name,
            dispersion.n_d(),
            0.0,
            birefringence_delta,
            absorption_rgb,
        );
        material.dispersion = dispersion;
        material
    }

    /// Inclusion/subsurface scattering: opts an existing material into a
    /// homogeneous Henyey-Greenstein scattering medium (silk/rutile/cloud inclusions),
    /// leaving every other field -- crucially including `absorption` -- untouched. A
    /// pure builder-style setter (`GemMaterial::ruby().with_scattering(0.4, 0.3)`),
    /// added so scenes/tests can opt individual materials in without a breaking change
    /// to `new_custom`'s or any built-in constructor's signature. See
    /// `scattering_sigma_s`/`scattering_g`'s own doc comments for what each parameter
    /// means; `sigma_s <= 0.0` (the default every built-in keeps) disables the feature
    /// entirely, reproducing today's exact deterministic Beer-Lambert path.
    #[must_use]
    pub const fn with_scattering(mut self, sigma_s: f32, g: f32) -> Self {
        self.scattering_sigma_s = sigma_s;
        self.scattering_g = g;
        self
    }

    /// A sensible default Henyey-Greenstein asymmetry for a caller who only wants to
    /// dial the AMOUNT of scattering ([`Self::with_scattering_amount`]) without thinking
    /// about anisotropy separately: mild forward scattering, physically typical for
    /// small needle-like/particulate inclusions (silk, rutile) at visible wavelengths --
    /// not a measured value for any specific species, just a reasonable "character"
    /// default.
    pub const DEFAULT_SCATTERING_G: f32 = 0.4;

    /// [`Self::with_scattering`] with [`Self::DEFAULT_SCATTERING_G`] for `g`, for a
    /// caller who wants a single "how much inclusion haze" knob. Two independent
    /// physical parameters genuinely exist here -- `sigma_s` (amount) and `g`
    /// (character: forward-scattering silk reads very differently from near-isotropic
    /// cloud) -- so this is a convenience on top of [`Self::with_scattering`], not a
    /// replacement for it; a caller who cares about the distinction should call
    /// `with_scattering` directly with an explicit `g`.
    #[must_use]
    pub const fn with_scattering_amount(self, sigma_s: f32) -> Self {
        self.with_scattering(sigma_s, Self::DEFAULT_SCATTERING_G)
    }

    /// A plausible per-species `(sigma_s, g)` starting point for "this species is
    /// typically included/hazy" -- e.g. Emerald's proverbial *jardin* -- keyed by this
    /// material's own `name`.
    ///
    /// # These are aesthetic choices, not measurements
    ///
    /// Unlike this material's Sellmeier dispersion coefficients or its pleochroic
    /// absorption bands (both cited to specific spectroscopic sources in
    /// [`Self::all_materials`]), there is no published "typical `sigma_s`" for any gem
    /// species -- inclusion density varies enormously by individual specimen, locality,
    /// and treatment, and clarity is conventionally assessed by eye/loupe grading, not a
    /// volumetric scattering coefficient. These numbers were chosen to LOOK plausible
    /// (documented per species below in descriptive terms -- "typically included",
    /// "usually eye-clean" -- deliberately NOT any standardized clarity-grade vocabulary
    /// like GIA's VVS/VS/SI scale, which grades visible-inclusion appearance under 10x
    /// magnification, a different thing this coefficient does not claim to reproduce),
    /// not derived from any citable source. Every built-in material's OWN
    /// `scattering_sigma_s` still stays exactly `0.0` (see that field's doc comment) --
    /// this method has no effect until a caller explicitly opts in via
    /// `material.with_recommended_scattering()`.
    #[must_use]
    pub fn recommended_scattering(&self) -> (f32, f32) {
        Self::recommended_scattering_arm(&self.name).unwrap_or((0.0, Self::DEFAULT_SCATTERING_G))
    }

    /// The explicit per-species arm [`Self::recommended_scattering`] delegates to, or
    /// `None` for any name with no dedicated arm (that method then falls back to
    /// `(0.0, Self::DEFAULT_SCATTERING_G)`).
    ///
    /// Pulled out as its own `Option`-returning function, rather than inlining this
    /// `match` directly in [`Self::recommended_scattering`] with a `_ =>` catch-all, so
    /// `tests::every_builtin_has_an_explicit_scattering_arm` can distinguish "this name
    /// has a real entry" from "this name silently fell through to the default," which a
    /// catch-all's return value alone cannot do (a species genuinely tuned to the same
    /// numbers as the default would look identical to one that was simply forgotten).
    /// A `_ =>` catch-all is also fragile to name drift: matching the literal
    /// `"Moissanite"` instead of the built-in material's actual name,
    /// `"Synthetic Moissanite"` (see `Self::all_materials`), would let the intended
    /// `(0.0, 0.0)` arm silently never fire, so every Moissanite render would use the
    /// generic default instead -- hence the explicit-arm-plus-test structure to catch
    /// this class of bug.
    pub(super) fn recommended_scattering_arm(name: &str) -> Option<(f32, f32)> {
        match name {
            // Typically visibly included (the proverbial "jardin", French for garden --
            // multi-phase fluid inclusions and growth-tube silk are part of how an
            // untreated natural emerald is expected to look, not a flaw to hide).
            "Emerald" => Some((0.6, 0.35)),
            // Silk (fine rutile needles) is common in natural corundum; heat treatment
            // (the overwhelming majority of the commercial supply) dissolves much of it,
            // so this is a moderate, not extreme, default.
            "Ruby" => Some((0.3, 0.5)),
            "Sapphire" => Some((0.25, 0.5)),
            // Natural rutilated quartz is famous for coarse, strongly forward-scattering
            // needles; ordinary rock crystal is usually clean. This default sits toward
            // the light-haze end since "Quartz" here is the generic rock-crystal entry.
            // Peridot shares this same light-haze tier: it commonly shows "lily pad"
            // (chromite + stress-fracture) inclusions.
            "Quartz" | "Peridot" => Some((0.15, 0.3)),
            // Elbaite tourmaline commonly carries visible needle/fingerprint inclusions.
            "Tourmaline" => Some((0.25, 0.4)),
            // Faceted gem-quality diamond is typically eye-clean; a small isotropic
            // haze (cloud inclusions) rather than a strong directional character.
            "Diamond" => Some((0.02, 0.0)),
            // Typically eye-clean once faceted (heat-treated zircon in particular).
            // Ordinary (non-color-change) chrysoberyl, amethyst, citrine, pyrope,
            // spessartine, benitoite and andalusite share this same light-default tier:
            // they are likewise typically eye-clean.
            "Zircon"
            | "Alexandrite"
            | "Topaz"
            | "Spinel"
            | "Tanzanite"
            | "Chrysoberyl (Yellow)"
            | "Amethyst"
            | "Citrine"
            | "Pyrope Garnet"
            | "Spessartine Garnet"
            | "Benitoite"
            | "Andalusite" => Some((0.02, 0.2)),
            // Lab-grown, essentially inclusion-free by construction. "Synthetic
            // Moissanite" -- see this function's own doc comment -- is
            // [`Self::all_materials`]'s actual built-in name; matching the bare
            // "Moissanite" instead would silently miss it, since that name never
            // appears there. Lab-grown YAG/GGG and Schott catalogue glass share this
            // same "essentially inclusion-free" tier.
            "Synthetic Moissanite"
            | "Cubic Zirconia"
            | "YAG"
            | "GGG"
            | "Glass (N-BK7)"
            | "Glass (F2)" => Some((0.0, 0.0)),
            // Aquamarine/Morganite: typically eye-clean beryl varieties (unlike
            // Emerald's proverbial jardin above -- beryl's clarity expectation varies
            // sharply by variety, not just by species).
            "Aquamarine" => Some((0.1, 0.3)),
            // Rutile joins Morganite in this same light-default tier -- rutile's own
            // name is synonymous with the fine acicular inclusions it forms in OTHER
            // species (see e.g. Ruby/Sapphire's "silk" comment above), but as a species
            // in its own right it is typically faceted from clean synthetic boules.
            "Morganite" | "Rutile" => Some((0.05, 0.2)),
            // Almandine commonly carries needle/rutile inclusions.
            "Almandine Garnet" => Some((0.15, 0.4)),
            // Tsavorite is frequently included (fine growth-tube inclusions,
            // moderately forward-scattering).
            "Grossular Garnet (Tsavorite)" => Some((0.2, 0.3)),
            // Demantoid's "horsetail" byssolite inclusions are a famous, often
            // deliberately showcased identification feature -- strongly
            // forward-scattering, not a flaw to hide (the same "characteristic
            // inclusion, not a defect" reasoning as Emerald's jardin above).
            "Andradite Garnet (Demantoid)" => Some((0.35, 0.6)),
            // Common (body-color) opal: some visible internal haze/crazing is
            // typical, though this entry deliberately does not model
            // play-of-color -- see this entry's own comment in
            // `built_in_materials_andalusite_through_glass`.
            "Opal" => Some((0.1, 0.2)),
            _ => None,
        }
    }

    /// [`Self::with_scattering`] using [`Self::recommended_scattering`]'s per-species
    /// `(sigma_s, g)` pair -- see that method's doc comment for why these are aesthetic
    /// defaults, not measurements. `GemMaterial::emerald().with_recommended_scattering()`
    /// reads at the call site the way a caller who wants "this species' typical included
    /// look" would expect.
    #[must_use]
    pub fn with_recommended_scattering(self) -> Self {
        let (sigma_s, g) = self.recommended_scattering();
        self.with_scattering(sigma_s, g)
    }

    /// Body-color variant: replaces this material's absorption with the isotropic
    /// band set `absorption_rgb` expands to (the same `[R, G, B]` triple
    /// [`Self::new_custom`] takes -- see [`super::body_color::BODY_COLOR_PRESETS`]
    /// for the fixed presets), leaving every other field -- dispersion,
    /// birefringence, c-axis, biaxial data, scattering, edge rounding, path scale --
    /// untouched. `GemMaterial::sapphire().with_body_color(yellow)` is a yellow
    /// sapphire with sapphire's own optics.
    ///
    /// A color variant is deliberately ISOTROPIC: whatever pleochroism the base
    /// material's own absorption tensor models is dropped. This is a quick what-if
    /// ("this design in a yellow instead of a blue sapphire"), not a mineralogical
    /// record of a real yellow specimen. `name` is left unchanged on purpose, so
    /// every lookup keyed by name (specific gravity, the RI presets, the render
    /// material dropdown) keeps resolving to the same species.
    #[must_use]
    pub fn with_body_color(mut self, absorption_rgb: [f32; 3]) -> Self {
        self.absorption = AbsorptionTensor::isotropic(legacy_rgb_bands(absorption_rgb));
        self
    }

    /// N-band body-color variant (the path-aware L*C*h editor): like
    /// [`Self::with_body_color`] but the isotropic absorption is the explicit band list
    /// `rows` (`[centre_nm, width_nm, amplitude_per_mm]` each, see
    /// `absorption::body_color_bands`) and [`Self::absorption_path_scale`] becomes
    /// `path_scale` (millimetres per model unit), so the stone's colour is the one the
    /// solver predicted for its real size. Rows with a non-positive or non-finite
    /// amplitude, width or centre are dropped; an empty result is a colourless stone.
    #[must_use]
    pub fn with_body_color_bands(mut self, rows: &[[f32; 3]], path_scale: f32) -> Self {
        let bands = rows
            .iter()
            .filter(|r| r.iter().all(|v| v.is_finite()) && r[1] > 0.0 && r[2] > 0.0)
            .map(|r| AbsorptionBand::new(r[0], r[1], r[2]))
            .collect();
        self.absorption = AbsorptionTensor::isotropic(bands);
        if path_scale.is_finite() && path_scale > 0.0 {
            self.absorption_path_scale = path_scale;
        }
        self
    }

    /// Facet edge rounding: opts a material into a nonzero
    /// meet-edge rounding radius -- see [`Self::edge_rounding_radius`]'s doc comment for
    /// units/range. `radius <= 0.0` (the default every built-in keeps) reproduces
    /// today's perfectly sharp, measure-zero edges exactly.
    #[must_use]
    pub const fn with_edge_rounding(mut self, radius: f32) -> Self {
        self.edge_rounding_radius = radius;
        self
    }

    /// Model-units-to-absorption-length-units scale: opts a material into a
    /// physical size other than the implicit "girdle radius ~1 model unit" every
    /// built-in cut renders at -- see [`Self::absorption_path_scale`]'s doc comment.
    /// `scale == 1.0` (the default every built-in keeps) reproduces today's
    /// behaviour exactly (a multiply by exactly `1.0` is an IEEE 754 no-op).
    #[must_use]
    pub const fn with_absorption_path_scale(mut self, scale: f32) -> Self {
        self.absorption_path_scale = scale;
        self
    }

    /// Replaces this material's absorption tensor with physically-based chromophore
    /// absorption bands.
    #[must_use]
    pub fn with_chromophore_absorption(mut self, tensor: AbsorptionTensor) -> Self {
        self.absorption = tensor;
        self
    }

    /// Legacy heuristic: true if the bands are energy-domain Gaussians rather than legacy
    /// fantasy RGB bands. A pure-host recipe has no bands, so this is not a reliable color-mode
    /// test; `render_setup` and the desktop take an explicit physics flag instead. Kept only
    /// until the web scene path passes one.
    #[must_use]
    pub fn is_physics_color(&self) -> bool {
        self.absorption
            .o_ray
            .iter()
            .any(|b| b.shape == crate::optics::absorption::BandShape::GaussianEnergy)
            || self
                .absorption
                .e_ray
                .iter()
                .any(|b| b.shape == crate::optics::absorption::BandShape::GaussianEnergy)
            || self.absorption.beta_ray.as_ref().is_some_and(|bands| {
                bands
                    .iter()
                    .any(|b| b.shape == crate::optics::absorption::BandShape::GaussianEnergy)
            })
    }
}
