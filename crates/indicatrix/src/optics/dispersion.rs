#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DispersionModel {
    Sellmeier1 {
        b1: f32,
        c1: f32,
    },
    Sellmeier3 {
        b: [f32; 3],
        c: [f32; 3],
    },
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
    Cauchy {
        a: f32,
        b: f32,
        c: f32,
    },
}

impl DispersionModel {
    /// Evaluates the refractive index at a given wavelength (in nanometers).
    #[must_use]
    pub fn evaluate(&self, lambda_nm: f32) -> f32 {
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
