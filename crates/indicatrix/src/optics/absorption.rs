/// Which domain an [`AbsorptionBand`] is Gaussian in.
///
/// Real chromophore absorption bands (electronic transitions broadened by
/// vibronic/Frank-Condon coupling) are closer to Gaussian in PHOTON ENERGY (equivalently,
/// wavenumber) than in wavelength -- the two only coincide exactly in the narrow-band
/// limit. Spectroscopy papers commonly cite band widths in cm^-1 or eV (energy-domain
/// units), which cannot be plugged into a wavelength-domain Gaussian's `width_nm`
/// directly without distorting the line shape (the wavelength axis is a nonlinear
/// reparametrization of the energy axis, so a Gaussian in one is not exactly Gaussian in
/// the other away from the peak). This variant lets a band be authored directly in the
/// domain its source data was measured in.
///
/// Defaults to [`Self::GaussianWavelength`] -- every existing built-in material keeps
/// its historical wavelength-domain shape bit-identically (see [`AbsorptionBand::new`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BandShape {
    /// Gaussian in wavelength (nm): `peak * exp(-0.5 * ((lambda_nm - center_nm) /
    /// width_nm)^2)`. The historical/default shape; `width_nm` is a standard deviation
    /// in nanometers.
    #[default]
    GaussianWavelength,
    /// Gaussian in wavenumber (cm^-1), i.e. in photon energy: `peak * exp(-0.5 *
    /// ((nu - nu0) / width_nm)^2)` where `nu = 1e7 / lambda_nm` and `nu0 = 1e7 /
    /// center_nm` are wavenumbers in cm^-1. For THIS variant only,
    /// [`width_nm`](AbsorptionBand::width_nm) is reinterpreted as a standard deviation in
    /// cm^-1, not nanometers (the field is not renamed, to keep every existing
    /// `GaussianWavelength` literal and the GPU band encoding untouched -- see
    /// [`AbsorptionBand::energy`]'s doc comment for the constructor that fills it in
    /// correctly).
    GaussianEnergy,
}

/// A single Gaussian absorption band: a chromophore electronic transition's
/// contribution to the material's absorption coefficient, as a function of wavelength.
///
/// (Or, see [`BandShape::GaussianEnergy`], as a function of photon energy instead.) The
/// form spectroscopic literature publishes gem chromophore data in (peak wavelength,
/// width, peak intensity); cheap to evaluate per-channel in the ray loop.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AbsorptionBand {
    /// Band centre wavelength, nanometers.
    pub center_nm: f32,
    /// Gaussian width (standard deviation): nanometers for
    /// [`BandShape::GaussianWavelength`], cm^-1 for [`BandShape::GaussianEnergy`] -- see
    /// [`BandShape`]'s own doc comment.
    pub width_nm: f32,
    /// Peak absorption coefficient at `center_nm`.
    pub peak: f32,
    /// Which domain this band is Gaussian in. Defaults to
    /// [`BandShape::GaussianWavelength`] via [`Self::new`].
    pub shape: BandShape,
}

impl AbsorptionBand {
    /// Builds a wavelength-domain (historical) band -- unchanged signature, so every
    /// existing call site (every built-in material's literal band data) keeps compiling
    /// and evaluating bit-identically to before `shape` was added.
    #[must_use]
    pub const fn new(center_nm: f32, width_nm: f32, peak: f32) -> Self {
        Self {
            center_nm,
            width_nm,
            peak,
            shape: BandShape::GaussianWavelength,
        }
    }

    /// Builds an energy-domain band directly from spectroscopy-convention units: a
    /// center wavelength in nanometers plus a FULL WIDTH AT HALF MAXIMUM in cm^-1
    /// (`fwhm_cm_inv`) -- the form most cited gem chromophore papers actually publish,
    /// rather than a raw standard deviation. Converted to the standard deviation
    /// [`Self::evaluate`] needs via the standard Gaussian relation
    /// `fwhm = 2 * sqrt(2 * ln(2)) * sigma`.
    ///
    /// # Converting an existing wavelength-domain width instead
    ///
    /// A band already characterized by a wavelength-domain standard deviation
    /// `sigma_lambda` (nm) at center `center_nm` converts to an approximately equivalent
    /// energy-domain standard deviation via the local derivative of `nu = 1e7 /
    /// lambda_nm`:
    ///
    /// ```text
    /// sigma_nu (cm^-1) ~= sigma_lambda (nm) * 1e7 / center_nm^2
    /// ```
    ///
    /// This is exact only in the limit `sigma_lambda << center_nm` (a first-order Taylor
    /// expansion of `nu(lambda)` about `center_nm`); see this module's
    /// `band_shape_tests::converted_energy_width_agrees_with_wavelength_width_near_center`
    /// for how closely the two shapes actually track near the peak. This constructor
    /// takes an FWHM directly rather than this converted sigma, since that is the form
    /// the doc comment above (and most cited sources) actually starts from -- construct
    /// [`Self`]'s fields directly (all `pub`) if a raw energy-domain sigma is already in
    /// hand.
    #[must_use]
    pub const fn energy(center_nm: f32, fwhm_cm_inv: f32, peak: f32) -> Self {
        // fwhm = 2*sqrt(2*ln(2))*sigma  =>  sigma = fwhm / (2*sqrt(2*ln(2))).
        const FWHM_TO_SIGMA: f32 = 2.354_82; // 2*sqrt(2*ln(2))
        Self {
            center_nm,
            width_nm: fwhm_cm_inv / FWHM_TO_SIGMA,
            peak,
            shape: BandShape::GaussianEnergy,
        }
    }

    /// This band's own contribution to the absorption coefficient at `lambda_nm`.
    #[must_use]
    pub fn evaluate(&self, lambda_nm: f32) -> f32 {
        match self.shape {
            BandShape::GaussianWavelength => {
                let t = (lambda_nm - self.center_nm) / self.width_nm;
                self.peak * (-0.5 * t * t).exp()
            }
            BandShape::GaussianEnergy => {
                // Wavenumber DIFFERENCE in cm^-1, well-conditioned form: `nu - nu0 =
                // 1e7/lambda_nm - 1e7/center_nm = 1e7*(center_nm - lambda_nm) /
                // (lambda_nm*center_nm)`. The naive two-reciprocal form (subtracting
                // `1e7/lambda_nm` and `1e7/center_nm`, each ~1e4..2e4 with a relative ULP
                // of ~1e-7) loses ~4-5 decimal digits to catastrophic cancellation before
                // dividing by `width_nm` and squaring into `exp`; on a GPU whose `/` is
                // only guaranteed to 2.5 ULP (Vulkan spec), that cancellation amplifies a
                // few-ULP division error into ~1000 ULP of output divergence between the
                // CPU and WGSL twins. This form does ONE division of a well-conditioned
                // quantity (numerator a few hundred nm, denominator
                // `lambda_nm*center_nm` ~ 1e5) instead. Verified against an f64
                // reference over the GPU equivalence harness's `spectral_absorption`
                // case bank: this form's max relative error vs f64 truth is 2.1e-6,
                // versus 4.6e-5 for the naive two-reciprocal form -- see
                // `spectral_absorption`'s own module doc comment for the probe.
                // Matches `transport_physics.wgsl`'s `spectral_absorption` translation
                // (its `band.shape == 1u` branch) op-for-op; keep both in lock-step.
                let delta_nm = self.center_nm - lambda_nm;
                let nu_diff = 1.0e7 * delta_nm / (lambda_nm * self.center_nm);
                let t = nu_diff / self.width_nm;
                self.peak * (-0.5 * t * t).exp()
            }
        }
    }
}

#[cfg(test)]
mod band_shape_tests {
    use super::*;

    /// The default constructor must keep every existing (wavelength-domain) band
    /// bit-identical: `shape` defaults to `GaussianWavelength`, so `evaluate` takes
    /// exactly the old code path.
    #[test]
    fn new_defaults_to_gaussian_wavelength() {
        let band = AbsorptionBand::new(550.0, 20.0, 2.0);
        assert_eq!(band.shape, BandShape::GaussianWavelength);
    }

    /// An energy-domain band is symmetric in WAVENUMBER, not wavelength: two
    /// wavelengths equidistant in wavenumber from the center (not equidistant in
    /// wavelength) must evaluate identically, and the small wavelength-domain asymmetry
    /// this implies must match what the nonlinear `nu = 1e7 / lambda` mapping's own
    /// curvature predicts for THIS band's `center_nm`/`delta_nu` -- not an arbitrary
    /// nanometre count.
    ///
    /// The asymmetry bound must come from what the nonlinear `nu = 1e7 / lambda`
    /// mapping's own curvature predicts, not from an arbitrary flat threshold:
    /// `lambda(nu) = 1e7 / nu` is convex, so two points equally spaced in `nu` around
    /// `nu0` land unequally spaced in `lambda`, with the leading-order gap given by a
    /// second-order Taylor expansion of `lambda(nu)` around `nu0`: `2 * delta_nu^2 *
    /// 1e7 / nu0^3`. For `center_nm = 500`, `delta_nu = 150` that works out to ~0.056
    /// nm -- a genuine, correctly-signed effect, just over an order of magnitude below
    /// a flat `1.0` nm bound, which would be the wrong number for these parameters.
    ///
    /// `response_plus`/`response_minus` use a `5e-6` absolute tolerance, not exact
    /// equality: verified against an f64 reference (a standalone probe replicating
    /// this exact test's inputs), the two responses are NOT perfectly equal even in
    /// f64. `evaluate`'s `GaussianEnergy` branch computes `nu - nu0` via the
    /// well-conditioned `nu - nu0 = 1e7 * (center_nm - lambda_nm) / (lambda_nm *
    /// center_nm)` form (see that branch's own doc comment), and `lambda_plus`/
    /// `lambda_minus` are themselves computed via `1.0e7 / (nu0 +/- delta_nu)` in
    /// `f32` above, which is not perfectly symmetric about `nu0` to begin with, so the
    /// true (f64) difference in response is `~2.98e-6`, not `0.0`. This `f32` code
    /// reproduces that true difference to 7 significant figures (`2.9802322e-6`
    /// measured vs `2.9809507e-6` true, relative error `~1.9e-8`); the naive
    /// two-reciprocal form's `0.0` exact-cancellation result is a rounding
    /// coincidence, not genuine accuracy -- its own per-point relative error against
    /// the same f64 reference is `~1.7e-6`/`~2.7e-6`, two orders of magnitude worse
    /// than this code's `~1.9e-8`/`~1.8e-8`.
    #[test]
    fn energy_domain_band_is_symmetric_in_wavenumber_not_wavelength() {
        let center_nm = 500.0f32;
        let band = AbsorptionBand::energy(center_nm, 400.0, 1.0);
        let nu0 = 1.0e7 / center_nm;
        let delta_nu = 150.0f32; // cm^-1
        let lambda_plus = 1.0e7 / (nu0 + delta_nu);
        let lambda_minus = 1.0e7 / (nu0 - delta_nu);

        // Equidistant in wavenumber -> (near-)identical response -- see this test's own
        // doc comment above for why `5e-6`, not `0.0`, is the physically-correct bound.
        let response_plus = band.evaluate(lambda_plus);
        let response_minus = band.evaluate(lambda_minus);
        assert!(
            (response_plus - response_minus).abs() < 5e-6,
            "wavenumber-symmetric points must give near-equal response (got {response_plus} vs \
             {response_minus})"
        );

        // The wavelength-domain asymmetry is real but small, and its SIZE is set by this
        // band's own center/delta_nu via the mapping's curvature -- not a fixed nanometre
        // count. See the doc comment above for the derivation.
        let actual_asymmetry_nm =
            ((lambda_plus - center_nm).abs() - (center_nm - lambda_minus).abs()).abs();
        let leading_order_asymmetry_nm = 2.0 * delta_nu * delta_nu * 1.0e7 / (nu0 * nu0 * nu0);
        assert!(
            (actual_asymmetry_nm - leading_order_asymmetry_nm).abs()
                < 0.05 * leading_order_asymmetry_nm,
            "wavelength asymmetry ({actual_asymmetry_nm} nm) must match the leading-order \
             estimate derived from this band's own center_nm/delta_nu \
             ({leading_order_asymmetry_nm} nm) to within 5%"
        );

        // ...and it must stay a small perturbation relative to center_nm, confirming this
        // is the near-center Taylor regime the derivation above assumes, not some blown-up
        // discrepancy that would actually indicate a bug.
        assert!(
            actual_asymmetry_nm < 0.001 * center_nm,
            "test premise: the wavelength asymmetry ({actual_asymmetry_nm} nm) must stay a \
             tiny fraction of center_nm ({center_nm} nm) for the near-center approximation \
             above to hold"
        );
    }

    /// At the band center, both shapes must agree exactly: `lambda_nm == center_nm`
    /// gives `t == 0.0` for either formula (`nu == nu0` too), so both collapse to
    /// exactly `peak`.
    #[test]
    fn both_shapes_agree_at_band_center() {
        let wavelength_band = AbsorptionBand::new(620.0, 30.0, 1.5);
        let energy_band = AbsorptionBand::energy(620.0, 200.0, 1.5);
        assert_eq!(wavelength_band.evaluate(620.0), 1.5);
        assert!(
            (energy_band.evaluate(620.0) - 1.5).abs() < 1e-6,
            "energy-domain band must also read exactly `peak` at its own center"
        );
    }

    /// Converting a wavelength-domain band's width to the energy domain via the
    /// documented `sigma_nu ~= sigma_lambda * 1e7 / center_nm^2` formula must agree with
    /// the original wavelength-domain band to within 1% NEAR the center (a first-order
    /// Taylor approximation, so this is not exact away from the peak).
    #[test]
    fn converted_energy_width_agrees_with_wavelength_width_near_center() {
        let center_nm = 550.0f32;
        let sigma_lambda = 20.0f32;
        let peak = 2.0f32;
        let wavelength_band = AbsorptionBand::new(center_nm, sigma_lambda, peak);

        let sigma_nu = sigma_lambda * 1.0e7 / (center_nm * center_nm);
        let energy_band = AbsorptionBand {
            center_nm,
            width_nm: sigma_nu,
            peak,
            shape: BandShape::GaussianEnergy,
        };

        // "Near center": a small offset relative to the band width.
        let probe_nm = center_nm + 2.0;
        let expected = wavelength_band.evaluate(probe_nm);
        let actual = energy_band.evaluate(probe_nm);
        assert!(
            (actual - expected).abs() < 0.01 * expected,
            "converted energy-domain width must agree with the wavelength-domain band to \
             within 1% near center (expected {expected}, got {actual})"
        );
    }
}

/// A gem material's absorption spectrum, expressed as a sum of Gaussian
/// [`AbsorptionBand`]s, one set per birefringent eigenmode (ordinary / extraordinary).
///
/// Real gem colour comes from specific, narrow electronic transitions of
/// transition-metal-ion chromophores (Cr3+, Fe2+/Ti4+, etc.); three wide, fixed-position
/// lobes on the sRGB primaries cannot represent a transmission window between two
/// absorption peaks (e.g. ruby's narrow red window plus a smaller blue one) no matter
/// how the RGB triple is tuned. See `materials::GemMaterial::all_materials` for the
/// cited band sets per species.
///
/// `o_ray`/`e_ray` are kept as two independent band sets (rather than collapsed) so a
/// polarization-aware blend can address each mode's bands independently.
///
/// `beta_ray` adds an optional third band set for genuinely biaxial (trichroic)
/// materials, carrying three distinct principal absorption coefficients instead of the
/// uniaxial ordinary/extraordinary pair. `None` is the two-set uniaxial representation
/// (the default for `isotropic`/`uniaxial`, and every material except Alexandrite).
/// Naming convention: `o_ray` -> `n_alpha`, `beta_ray` -> `n_beta`, `e_ray` -> `n_gamma`
/// -- see `birefringence::AbsorptionTensor3::biaxial` for which world axis each lands on.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AbsorptionTensor {
    /// Absorption bands for the ordinary ray (uniaxial) / the `n_alpha` principal
    /// direction (biaxial, when `beta_ray` is `Some`).
    pub o_ray: Vec<AbsorptionBand>,
    /// Absorption bands for the extraordinary ray (uniaxial) / the `n_gamma` principal
    /// direction (biaxial, when `beta_ray` is `Some`).
    pub e_ray: Vec<AbsorptionBand>,
    /// The third, `n_beta`, principal direction's absorption bands, for a genuinely
    /// biaxial (trichroic) material; `None` for uniaxial/isotropic materials and for
    /// biaxial materials still using the two-set approximation. See [`Self::biaxial`].
    pub beta_ray: Option<Vec<AbsorptionBand>>,
    pub is_pleochroic: bool,
}

impl AbsorptionTensor {
    /// A non-pleochroic material: the same band set applies to both eigenmodes (also
    /// used for isotropic/cubic materials, which have no o-ray/e-ray distinction).
    ///
    /// An empty `Vec` (the three colourless built-ins: Diamond, Moissanite, Cubic
    /// Zirconia) means zero absorption at every wavelength.
    #[must_use]
    pub fn isotropic(bands: Vec<AbsorptionBand>) -> Self {
        Self {
            o_ray: bands.clone(),
            e_ray: bands,
            beta_ray: None,
            is_pleochroic: false,
        }
    }

    #[must_use]
    pub const fn uniaxial(o: Vec<AbsorptionBand>, e: Vec<AbsorptionBand>) -> Self {
        Self {
            o_ray: o,
            e_ray: e,
            beta_ray: None,
            is_pleochroic: true,
        }
    }

    /// A genuinely biaxial (trichroic) material: three independent band sets, one per
    /// principal direction. `alpha`/`beta`/`gamma` map to `o_ray`/`beta_ray`/`e_ray`
    /// respectively -- see [`Self`]'s doc comment for the world-axis convention.
    ///
    /// Degenerate case: `alpha == beta` produces a tensor identical to
    /// <code>[Self::uniaxial](alpha, gamma)</code> at every wavelength, since `beta_ray` then
    /// holds the same data as `o_ray` -- no special-cased collapse needed.
    #[must_use]
    pub const fn biaxial(
        alpha: Vec<AbsorptionBand>,
        beta: Vec<AbsorptionBand>,
        gamma: Vec<AbsorptionBand>,
    ) -> Self {
        Self {
            o_ray: alpha,
            e_ray: gamma,
            beta_ray: Some(beta),
            is_pleochroic: true,
        }
    }
}

/// Builds a 3-band absorption set from an `[R, G, B]` peak-coefficient triple, using
/// band centres 620/540/450 nm, widths 55/45/45 nm.
///
/// Kept `pub` because [`materials::GemMaterial::new_custom`](crate::optics::materials::GemMaterial::new_custom)
/// -- the public API for user-authored custom materials -- calls it directly: a plain
/// `[R, G, B]` triple is the right shape there since there's no chromophore
/// spectroscopy to cite for an arbitrary user material.
///
/// Not numerically identical to the old normalized-blend model: this sums the three
/// bands as independent contributions (consistent with how real absorption spectra
/// combine), so overlapping bands compound rather than blend. Only the three
/// colourless built-ins (empty `Vec`) render bit-identically to before.
#[must_use]
pub fn legacy_rgb_bands(rgb: [f32; 3]) -> Vec<AbsorptionBand> {
    vec![
        AbsorptionBand::new(620.0, 55.0, rgb[0]),
        AbsorptionBand::new(540.0, 45.0, rgb[1]),
        AbsorptionBand::new(450.0, 45.0, rgb[2]),
    ]
}
