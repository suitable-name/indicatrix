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
/// The shortest wavelength a fluorescence excitation path evaluates (see
/// `optics::fluorescence`); the floor of every model's validity range.
const UV_MIN_NM: f32 = 300.0;

impl DispersionModel {
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
}
