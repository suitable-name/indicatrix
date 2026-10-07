/// A refractive-index-vs-wavelength dispersion curve, in one of two closed forms.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DispersionModel {
    /// A single-term Sellmeier equation: `n^2 = 1 + b1*lambda^2/(lambda^2 - c1)`
    /// (`lambda` in micrometers).
    Sellmeier1 { b1: f32, c1: f32 },
    /// A three-term Sellmeier equation, summing three [`Self::Sellmeier1`]-shaped
    /// terms.
    Sellmeier3 { b: [f32; 3], c: [f32; 3] },
    /// A 2- or 3-parameter Cauchy fit, `n(lambda) = a + b/lambda^2 + c/lambda^4`
    /// (`lambda` in micrometers). Every Cauchy fit in `materials::GemMaterial::
    /// all_materials` is a VISIBLE-RANGE-ONLY approximation, solved from a mean index
    /// and a Fraunhofer F-C dispersion figure at/near the sodium D line (589.3nm) --
    /// see each entry's own sourcing comment. Treat these fits as reliable roughly
    /// 400-700nm; an extrapolation to this renderer's actual sampled band edges
    /// (380nm violet / 780nm red) is NOT independently validated against a primary
    /// source and can overshoot or (for some coefficient combinations) dip below
    /// physically-impossible `n < 1` there -- see [`Self::evaluate`]'s `.max(1.0)`
    /// floor below, and `materials::tests::
    /// every_builtin_dispersion_curve_stays_physical_at_the_sampled_band_edges` for the
    /// regression pinning `n >= 1.0` and finite at exactly those two edges for every
    /// built-in material.
    Cauchy { a: f32, b: f32, c: f32 },
}

/// The visible camera range's lower edge; the models are untouched at and above it.
const VISIBLE_MIN_NM: f32 = 380.0;
/// The visible camera range's upper edge.
const VISIBLE_MAX_NM: f32 = 780.0;
/// The shortest wavelength a fluorescence excitation path evaluates (see
/// `optics::fluorescence`); the floor of every model's validity range.
const UV_MIN_NM: f32 = 300.0;
/// The upper edge of the band a Sellmeier resonance must stay out of (see
/// [`DispersionModel::validate`]).
const RESONANCE_MAX_NM: f32 = 800.0;
/// The sodium D line, where a gem's "refractive index" is quoted.
const LINE_D_NM: f32 = 589.3;
/// The Fraunhofer F line (hydrogen blue).
const LINE_F_NM: f32 = 486.1;
/// The Fraunhofer C line (hydrogen red).
const LINE_C_NM: f32 = 656.3;
/// Spacing of the wavelength samples [`DispersionModel::validate`] and
/// [`DispersionModel::warnings`] walk across the visible band.
const BAND_STEP_NM: f32 = 5.0;
/// Number of [`BAND_STEP_NM`] steps from [`VISIBLE_MIN_NM`] to [`VISIBLE_MAX_NM`].
const BAND_STEPS: u16 = 80;
/// An index rise larger than this between neighbouring samples counts as anomalous
/// dispersion; a spread smaller than this counts as no dispersion at all.
const INDEX_TOLERANCE: f32 = 1e-6;
/// The Abbe numbers of real gem materials sit well inside this range; outside it the
/// figures are worth a second look (a diamond is about 55, rutile about 10).
const ABBE_RANGE: (f32, f32) = (5.0, 120.0);

/// Why a [`DispersionModel`] cannot be used to render a stone (see
/// [`DispersionModel::validate`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DispersionError {
    /// A coefficient is not a finite number.
    NonFiniteCoefficient,
    /// A Sellmeier term has a resonance, `sqrt(C)` in micrometers, inside the 300-800 nm
    /// band the renderer samples (the index would blow up there).
    ResonanceInBand {
        /// Where the resonance sits, in nanometers.
        resonance_nm: f32,
    },
    /// The index is not a finite number above 1 at this wavelength.
    IndexOutOfRange {
        /// The first sampled wavelength that fails, in nanometers.
        lambda_nm: f32,
        /// The index the model gives there (`NaN` or infinite when it has no real value).
        n: f32,
    },
}

impl std::fmt::Display for DispersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::NonFiniteCoefficient => f.write_str("A coefficient is not a finite number."),
            Self::ResonanceInBand { resonance_nm } => write!(
                f,
                "A C value puts a resonance at {resonance_nm:.0} nm, inside the 300-800 nm \
                 range the stone is rendered in. C is in \u{b5}m\u{b2}, so 0.01 means a \
                 resonance at 100 nm."
            ),
            Self::IndexOutOfRange { lambda_nm, n } if n.is_finite() => write!(
                f,
                "The refractive index comes out as {n:.3} at {lambda_nm:.0} nm. It must stay \
                 above 1. Check the signs and sizes of the coefficients."
            ),
            Self::IndexOutOfRange { lambda_nm, .. } => write!(
                f,
                "The refractive index cannot be calculated at {lambda_nm:.0} nm. Check the \
                 signs and sizes of the coefficients."
            ),
        }
    }
}

impl std::error::Error for DispersionError {}

/// Things worth a second look in a [`DispersionModel`] that still renders (see
/// [`DispersionModel::warnings`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DispersionWarnings {
    /// The index rises with wavelength somewhere in the visible band; real gems fall
    /// from violet to red.
    pub anomalous: bool,
    /// The Abbe number is outside 5-120.
    pub abbe_out_of_range: bool,
    /// The index is the same at the F and C lines, so the stone shows no fire.
    pub no_dispersion: bool,
}

impl DispersionWarnings {
    /// Whether any warning is set.
    #[must_use]
    pub const fn any(self) -> bool {
        self.anomalous || self.abbe_out_of_range || self.no_dispersion
    }
}

/// The wavelengths (nm) from 380 to 780 in 5 nm steps.
fn band_samples() -> impl Iterator<Item = f32> {
    (0..=BAND_STEPS).map(|step| f32::from(step).mul_add(BAND_STEP_NM, VISIBLE_MIN_NM))
}

impl DispersionModel {
    /// The shortest wavelength (nm) of the visible band the renderer samples.
    pub const BAND_MIN_NM: f32 = VISIBLE_MIN_NM;
    /// The longest wavelength (nm) of the visible band the renderer samples.
    pub const BAND_MAX_NM: f32 = VISIBLE_MAX_NM;

    /// The shortest wavelength (nm) this model is evaluated at: below it
    /// [`Self::evaluate`] holds the index at its value there.
    ///
    /// Fluorescence paths evaluate the material at excitation wavelengths down to 300 nm,
    /// below the 380 nm the built-in curves were fitted and checked for:
    /// - [`Self::Cauchy`] fits are visible-range-only (see the variant's doc), and their
    ///   `1/lambda^4` term runs away in the UV, so they clamp at 380 nm.
    /// - A Sellmeier curve is physical down to its shortest-wavelength resonance
    ///   (`lambda^2 = c`), where `n` diverges, so it clamps at 300 nm, or 5 % above its
    ///   nearest UV resonance when that lies above 285 nm (never above 380 nm).
    ///
    /// The clamp makes `n(lambda)` constant below the limit, a bounded, physical
    /// approximation of a flat index in the deep UV; it never touches `lambda >= 380`.
    #[must_use]
    pub fn min_valid_nm(&self) -> f32 {
        let uv_pole_floor = |c: &[f32]| {
            c.iter()
                .filter(|&&c| c > 0.0)
                .map(|&c| c.sqrt() * 1e3)
                .filter(|&pole_nm| pole_nm < VISIBLE_MIN_NM)
                .fold(UV_MIN_NM, |floor, pole_nm| floor.max(pole_nm * 1.05))
                .min(VISIBLE_MIN_NM)
        };
        match self {
            Self::Sellmeier1 { c1, .. } => uv_pole_floor(&[*c1]),
            Self::Sellmeier3 { c, .. } => uv_pole_floor(c),
            Self::Cauchy { .. } => VISIBLE_MIN_NM,
        }
    }

    /// Evaluates the refractive index at a given wavelength (in nanometers).
    ///
    /// Wavelengths below [`Self::min_valid_nm`] (only reachable by fluorescence
    /// excitation paths; the camera range starts at 380 nm) evaluate the model at that
    /// limit instead. At and above 380 nm the result is the plain model, bit for bit.
    #[must_use]
    pub fn evaluate(&self, lambda_nm: f32) -> f32 {
        if lambda_nm < VISIBLE_MIN_NM {
            self.evaluate_unclamped(lambda_nm.max(self.min_valid_nm()))
        } else {
            self.evaluate_unclamped(lambda_nm)
        }
    }

    /// The model itself, evaluated exactly at `lambda_nm`.
    fn evaluate_unclamped(&self, lambda_nm: f32) -> f32 {
        let lambda_um = lambda_nm * 1e-3;
        let l2 = lambda_um * lambda_um;

        match self {
            Self::Sellmeier1 { b1, c1 } => {
                let n2 = 1.0 + (b1 * l2) / (l2 - c1);
                n2.max(1.0).sqrt()
            }
            Self::Sellmeier3 { b, c } => {
                let mut n2 = 1.0;
                n2 += (b[0] * l2) / (l2 - c[0]);
                n2 += (b[1] * l2) / (l2 - c[1]);
                n2 += (b[2] * l2) / (l2 - c[2]);
                n2.max(1.0).sqrt()
            }
            Self::Cauchy {
                a,
                b: b_coeff,
                c: c_coeff,
            } => {
                let l4 = l2 * l2;
                // Floored at 1.0, matching the Sellmeier variants' own `.max(1.0)`
                // above -- see this variant's doc comment for why an out-of-fit-range
                // extrapolation (this renderer samples down to 380nm/up to 780nm; most
                // Cauchy fits here are only solved/verified at/near the sodium D line
                // and the F/C Fraunhofer lines, 486-656nm) must never be allowed to
                // produce a physically-impossible n < 1 index.
                (a + (b_coeff / l2) + (c_coeff / l4)).max(1.0)
            }
        }
    }

    /// The model's index with none of [`Self::evaluate`]'s floors: below 1 when the
    /// coefficients say so, `NaN` where a Sellmeier `n^2` is negative, infinite on a pole.
    /// Only [`Self::validate`] wants to see those; the renderer must never.
    fn raw_index(&self, lambda_nm: f32) -> f32 {
        let lambda_um = lambda_nm * 1e-3;
        let l2 = lambda_um * lambda_um;
        match self {
            Self::Sellmeier1 { b1, c1 } => (1.0 + (b1 * l2) / (l2 - c1)).sqrt(),
            Self::Sellmeier3 { b, c } => {
                let mut n2 = 1.0;
                n2 += (b[0] * l2) / (l2 - c[0]);
                n2 += (b[1] * l2) / (l2 - c[1]);
                n2 += (b[2] * l2) / (l2 - c[2]);
                n2.sqrt()
            }
            Self::Cauchy { a, b, c } => a + (b / l2) + (c / (l2 * l2)),
        }
    }

    /// Whether every coefficient is a finite number.
    fn coefficients_are_finite(&self) -> bool {
        match self {
            Self::Sellmeier1 { b1, c1 } => b1.is_finite() && c1.is_finite(),
            Self::Sellmeier3 { b, c } => b.iter().chain(c.iter()).all(|v| v.is_finite()),
            Self::Cauchy { a, b, c } => a.is_finite() && b.is_finite() && c.is_finite(),
        }
    }

    /// The first Sellmeier resonance (nm) that sits inside the 300-800 nm band.
    ///
    /// Every term counts, whatever its `B`: a term with `B = 0` and a resonance in the band
    /// still divides by zero at that one wavelength. An unused term is entered as `C = 0`
    /// (or any `C` outside the band), which has no resonance to speak of.
    fn resonance_in_band(&self) -> Option<f32> {
        let in_band = |c: f32| -> Option<f32> {
            if c <= 0.0 {
                return None;
            }
            let resonance_nm = c.sqrt() * 1e3;
            (UV_MIN_NM..=RESONANCE_MAX_NM)
                .contains(&resonance_nm)
                .then_some(resonance_nm)
        };
        match self {
            Self::Sellmeier1 { c1, .. } => in_band(*c1),
            Self::Sellmeier3 { c, .. } => c.iter().find_map(|&c_term| in_band(c_term)),
            Self::Cauchy { .. } => None,
        }
    }

    /// The index at the sodium D line (589.3 nm): the "refractive index" a gem is quoted at.
    #[must_use]
    pub fn n_d(&self) -> f32 {
        self.evaluate(LINE_D_NM)
    }

    /// The index at the Fraunhofer F line (486.1 nm, blue).
    #[must_use]
    pub fn n_f(&self) -> f32 {
        self.evaluate(LINE_F_NM)
    }

    /// The index at the Fraunhofer C line (656.3 nm, red).
    #[must_use]
    pub fn n_c(&self) -> f32 {
        self.evaluate(LINE_C_NM)
    }

    /// The Fraunhofer dispersion `n_F - n_C`: positive for a normal material, and the
    /// "Dispersion" figure the simple material editor takes.
    #[must_use]
    pub fn delta_f_c(&self) -> f32 {
        self.n_f() - self.n_c()
    }

    /// The Abbe number `V_d = (n_d - 1) / (n_F - n_C)`; lower means more dispersive.
    ///
    /// `None` when the model has no positive `n_F - n_C` (no dispersion at all, or the
    /// index rises with wavelength), where the figure has no meaning.
    #[must_use]
    pub fn abbe_number(&self) -> Option<f32> {
        let delta = self.delta_f_c();
        let n_d = self.n_d();
        (delta > 0.0 && delta.is_finite() && n_d.is_finite()).then_some((n_d - 1.0) / delta)
    }

    /// Checks that the model can render a stone.
    ///
    /// Every coefficient must be finite, no Sellmeier term that carries weight may have its
    /// resonance (`sqrt(C)` in micrometers) inside 300-800 nm, and the index must be a
    /// finite number above 1 at every wavelength from 380 to 780 nm in 5 nm steps.
    /// [`Self::evaluate`] quietly floors a bad curve at 1, which would render a flat clear
    /// block with no hint why; this is where such a curve is refused instead.
    ///
    /// # Errors
    ///
    /// The first problem found, as a [`DispersionError`].
    pub fn validate(&self) -> Result<(), DispersionError> {
        if !self.coefficients_are_finite() {
            return Err(DispersionError::NonFiniteCoefficient);
        }
        if let Some(resonance_nm) = self.resonance_in_band() {
            return Err(DispersionError::ResonanceInBand { resonance_nm });
        }
        for lambda_nm in band_samples() {
            let n = self.raw_index(lambda_nm);
            let physical = n.is_finite() && n > 1.0;
            if !physical {
                return Err(DispersionError::IndexOutOfRange { lambda_nm, n });
            }
        }
        Ok(())
    }

    /// What is unusual about a model that [`Self::validate`] accepts: an index that rises
    /// with wavelength, an Abbe number outside 5-120, or no dispersion at all.
    #[must_use]
    pub fn warnings(&self) -> DispersionWarnings {
        let mut previous = self.evaluate(VISIBLE_MIN_NM);
        let mut anomalous = false;
        for lambda_nm in band_samples().skip(1) {
            let n = self.evaluate(lambda_nm);
            if n > previous + INDEX_TOLERANCE {
                anomalous = true;
                break;
            }
            previous = n;
        }
        DispersionWarnings {
            anomalous,
            abbe_out_of_range: self
                .abbe_number()
                .is_some_and(|abbe| !(ABBE_RANGE.0..=ABBE_RANGE.1).contains(&abbe)),
            no_dispersion: self.delta_f_c().abs() <= INDEX_TOLERANCE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optics::materials::GemMaterial;

    /// At and above 380 nm the clamp is invisible: bit-identical to the plain model for
    /// every built-in material.
    #[test]
    fn evaluate_is_unchanged_at_and_above_380nm() {
        for material in GemMaterial::all_materials() {
            let model = material.dispersion;
            for step in 0..=108 {
                let lambda = (step as f32).mul_add(3.7, 380.0);
                assert_eq!(
                    model.evaluate(lambda).to_bits(),
                    model.evaluate_unclamped(lambda).to_bits(),
                    "{} at {lambda} nm",
                    material.name
                );
            }
        }
    }

    /// Below 380 nm every built-in is finite, physical (n >= 1) and held at the model's
    /// validity minimum: a Cauchy fit at its 380 nm value, a Sellmeier curve at no less
    /// than 300 nm.
    #[test]
    fn evaluate_clamps_to_the_validity_minimum_below_380nm() {
        for material in GemMaterial::all_materials() {
            let model = material.dispersion;
            let min = model.min_valid_nm();
            assert!((300.0..=380.0).contains(&min), "{}: {min}", material.name);
            if matches!(model, DispersionModel::Cauchy { .. }) {
                assert_eq!(min, 380.0, "{}", material.name);
            }
            for lambda in [300.0f32, 320.0, 345.5, 379.9] {
                let n = model.evaluate(lambda);
                assert!(
                    n.is_finite() && n >= 1.0,
                    "{} at {lambda}: {n}",
                    material.name
                );
                assert_eq!(
                    n.to_bits(),
                    model.evaluate(lambda.max(min)).to_bits(),
                    "{} at {lambda}",
                    material.name
                );
            }
            // Never below the clamp value (no runaway towards a UV pole).
            assert_eq!(
                model.evaluate(300.0).to_bits(),
                model.evaluate(min).to_bits()
            );
        }
    }

    /// Sapphire's Sellmeier fit is used down to 300 nm (`n_o` ~ 1.81 there), a Cauchy fit
    /// is held flat.
    #[test]
    fn sellmeier_is_evaluated_in_the_uv_and_cauchy_is_held() {
        let sapphire = GemMaterial::by_name("Sapphire").unwrap().dispersion;
        assert_eq!(sapphire.min_valid_nm(), 300.0);
        let n300 = sapphire.evaluate(300.0);
        assert!(
            n300 > sapphire.evaluate(380.0) && (1.78..1.9).contains(&n300),
            "{n300}"
        );
        let cauchy = DispersionModel::Cauchy {
            a: 1.7,
            b: 0.01,
            c: 0.0,
        };
        assert_eq!(cauchy.evaluate(300.0), cauchy.evaluate(380.0));
        // A Sellmeier pole just below 380 nm raises the floor above 300 nm.
        let near_pole = DispersionModel::Sellmeier1 {
            b1: 1.0,
            c1: 0.1225,
        };
        assert!(near_pole.min_valid_nm() > 360.0 && near_pole.min_valid_nm() <= 380.0);
        assert!(near_pole.evaluate(300.0).is_finite());
    }

    // ---- validate / warnings / line helpers ----

    /// Every built-in curve (ordinary and extraordinary) is a model the validator accepts:
    /// the rules must never refuse a real material.
    #[test]
    fn every_builtin_curve_validates() {
        for material in GemMaterial::all_materials() {
            assert_eq!(
                material.dispersion.validate(),
                Ok(()),
                "{} (ordinary)",
                material.name
            );
            if let Some(extraordinary) = material.uniaxial_extraordinary_dispersion {
                assert_eq!(
                    extraordinary.validate(),
                    Ok(()),
                    "{} (extraordinary)",
                    material.name
                );
            }
        }
    }

    /// The line helpers are `evaluate` at the three Fraunhofer wavelengths, and the Abbe
    /// number follows its definition.
    #[test]
    fn line_helpers_and_abbe_number_follow_the_definition() {
        let diamond = GemMaterial::diamond().dispersion;
        assert_eq!(diamond.n_d().to_bits(), diamond.evaluate(589.3).to_bits());
        assert_eq!(diamond.n_f().to_bits(), diamond.evaluate(486.1).to_bits());
        assert_eq!(diamond.n_c().to_bits(), diamond.evaluate(656.3).to_bits());
        assert_eq!(
            diamond.delta_f_c().to_bits(),
            (diamond.n_f() - diamond.n_c()).to_bits()
        );
        let abbe = diamond.abbe_number().expect("diamond disperses");
        let by_hand = (diamond.n_d() - 1.0) / (diamond.n_f() - diamond.n_c());
        assert_eq!(abbe.to_bits(), by_hand.to_bits());
        // The published figure the built-in is tested against elsewhere.
        assert!((abbe - 55.27).abs() < 0.5, "{abbe}");
        assert!((diamond.n_d() - 2.417).abs() < 0.005, "{}", diamond.n_d());
    }

    /// No spread between F and C: no Abbe number, and a "no dispersion" warning instead of
    /// an anomalous one.
    #[test]
    fn a_flat_curve_has_no_abbe_number_and_says_so() {
        let flat = DispersionModel::Cauchy {
            a: 1.7,
            b: 0.0,
            c: 0.0,
        };
        assert_eq!(flat.validate(), Ok(()));
        assert_eq!(flat.abbe_number(), None);
        assert_eq!(
            flat.warnings(),
            DispersionWarnings {
                anomalous: false,
                abbe_out_of_range: false,
                no_dispersion: true,
            }
        );
    }

    /// A typical gem curve raises no warning at all.
    #[test]
    fn a_typical_gem_curve_has_no_warnings() {
        let sapphire = GemMaterial::sapphire().dispersion;
        assert_eq!(sapphire.warnings(), DispersionWarnings::default());
        assert!(!sapphire.warnings().any());
    }

    /// An index that rises with wavelength is valid but flagged, and has no Abbe number.
    #[test]
    fn a_rising_index_is_anomalous() {
        let rising = DispersionModel::Cauchy {
            a: 1.5,
            b: -0.002,
            c: 0.0,
        };
        assert_eq!(rising.validate(), Ok(()));
        let warnings = rising.warnings();
        assert!(warnings.anomalous);
        assert!(!warnings.no_dispersion);
        assert_eq!(rising.abbe_number(), None);
        assert!(warnings.any());
    }

    /// An Abbe number outside 5-120 is flagged at both ends.
    #[test]
    fn an_abbe_number_outside_five_to_one_twenty_warns() {
        // Barely dispersive: Abbe number about 470.
        let weak = DispersionModel::Cauchy {
            a: 1.45,
            b: 0.0005,
            c: 0.0,
        };
        assert!(weak.abbe_number().expect("disperses") > 120.0);
        assert!(weak.warnings().abbe_out_of_range);
        // Extremely dispersive: Abbe number about 1.
        let strong = DispersionModel::Cauchy {
            a: 2.0,
            b: 0.5,
            c: 0.0,
        };
        assert_eq!(strong.validate(), Ok(()));
        assert!(strong.abbe_number().expect("disperses") < 5.0);
        assert!(strong.warnings().abbe_out_of_range);
        assert!(!strong.warnings().anomalous);
    }

    /// A resonance anywhere in 300-800 nm is refused, close to either edge too; a UV one
    /// below the band is accepted.
    #[test]
    fn a_sellmeier_resonance_in_the_band_is_refused() {
        let at = |c1: f32| DispersionModel::Sellmeier1 { b1: 1.0, c1 };
        // 600 nm, in the middle of the visible band.
        match at(0.36).validate() {
            Err(DispersionError::ResonanceInBand { resonance_nm }) => {
                assert!((resonance_nm - 600.0).abs() < 0.5, "{resonance_nm}");
            }
            other => panic!("expected a resonance error, got {other:?}"),
        }
        // 316 nm and 787 nm, close to the two edges of the band.
        assert!(matches!(
            at(0.1).validate(),
            Err(DispersionError::ResonanceInBand { .. })
        ));
        assert!(matches!(
            at(0.62).validate(),
            Err(DispersionError::ResonanceInBand { .. })
        ));
        // 290 nm: a UV resonance below the band is fine.
        assert_eq!(at(0.0841).validate(), Ok(()));
    }

    /// The resonance rule covers every term of a three-term model, whatever its weight.
    #[test]
    fn a_resonance_in_any_sellmeier3_term_is_refused() {
        let model = DispersionModel::Sellmeier3 {
            b: [1.0, 0.0, 0.5],
            c: [0.01, 0.36, 0.02],
        };
        assert!(matches!(
            model.validate(),
            Err(DispersionError::ResonanceInBand { .. })
        ));
        // Unused terms entered as zero are fine.
        let unused = DispersionModel::Sellmeier3 {
            b: [1.0, 0.0, 0.0],
            c: [0.01, 0.0, 0.0],
        };
        assert_eq!(unused.validate(), Ok(()));
    }

    /// A resonance just beyond the red edge is outside the pole band, but it drags the
    /// whole visible band below an index of 1, which the sampled check refuses.
    #[test]
    fn a_pole_beyond_the_red_edge_drops_the_index_below_one() {
        let model = DispersionModel::Sellmeier1 { b1: 1.0, c1: 0.7 };
        match model.validate() {
            Err(DispersionError::IndexOutOfRange { lambda_nm, n }) => {
                assert_eq!(lambda_nm, 380.0);
                assert!(n.is_finite() && n < 1.0, "{n}");
            }
            other => panic!("expected an index error, got {other:?}"),
        }
    }

    /// A negative `n^2` has no real index; it is refused, not floored.
    #[test]
    fn a_negative_index_square_cannot_be_calculated() {
        let model = DispersionModel::Sellmeier1 { b1: -3.0, c1: 0.01 };
        match model.validate() {
            Err(DispersionError::IndexOutOfRange { lambda_nm, n }) => {
                assert_eq!(lambda_nm, 380.0);
                assert!(n.is_nan(), "{n}");
            }
            other => panic!("expected an index error, got {other:?}"),
        }
        // `evaluate` still answers (a floor of 1), which is why validation comes first.
        assert_eq!(model.evaluate(500.0), 1.0);
    }

    /// `NaN` and infinity in any coefficient are refused before anything is evaluated.
    #[test]
    fn non_finite_coefficients_are_refused() {
        for model in [
            DispersionModel::Cauchy {
                a: f32::NAN,
                b: 0.0,
                c: 0.0,
            },
            DispersionModel::Sellmeier1 {
                b1: f32::INFINITY,
                c1: 0.01,
            },
            DispersionModel::Sellmeier3 {
                b: [1.0, 1.0, 1.0],
                c: [0.01, f32::NEG_INFINITY, 0.02],
            },
        ] {
            assert_eq!(
                model.validate(),
                Err(DispersionError::NonFiniteCoefficient),
                "{model:?}"
            );
        }
    }

    /// An index at or below 1 is refused at the first sampled wavelength it fails, where
    /// `evaluate` would have floored it to 1 and drawn a clear block.
    #[test]
    fn an_index_at_or_below_one_is_refused() {
        let low = DispersionModel::Cauchy {
            a: 0.9,
            b: 0.0,
            c: 0.0,
        };
        assert_eq!(
            low.validate(),
            Err(DispersionError::IndexOutOfRange {
                lambda_nm: 380.0,
                n: 0.9
            })
        );
        // The floor hides it from `evaluate`, which is the point of validating first.
        assert_eq!(low.evaluate(500.0), 1.0);
        // Falls below 1 only at the violet end.
        let violet_dip = DispersionModel::Cauchy {
            a: 1.3,
            b: -0.1,
            c: 0.0,
        };
        match violet_dip.validate() {
            Err(DispersionError::IndexOutOfRange { lambda_nm, n }) => {
                assert_eq!(lambda_nm, 380.0);
                assert!(n < 1.0);
            }
            other => panic!("expected an index error, got {other:?}"),
        }
    }

    /// The error texts name what to check, in plain words.
    #[test]
    fn error_messages_are_plain_sentences() {
        let resonance = DispersionError::ResonanceInBand {
            resonance_nm: 600.0,
        }
        .to_string();
        assert!(resonance.contains("600 nm") && resonance.contains("C value"));
        let low = DispersionError::IndexOutOfRange {
            lambda_nm: 380.0,
            n: 0.9,
        }
        .to_string();
        assert!(low.contains("0.900") && low.contains("380 nm"));
        let nan = DispersionError::IndexOutOfRange {
            lambda_nm: 700.0,
            n: f32::NAN,
        }
        .to_string();
        assert!(nan.contains("cannot be calculated"));
        assert_ne!(DispersionError::NonFiniteCoefficient.to_string(), "");
    }
}
