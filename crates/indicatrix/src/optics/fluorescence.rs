//! CPU-only fluorescence: emitter data and the in-medium fluorescence vertex sampling.
//!
//! This module holds the data types and the sampling maths of the vertex (see
//! `docs/fluorescence-plan.md` sections 3-4, and the "Fluorescence" section of
//! `crates/indicatrix/docs/physics.md`).
//!
//! # Model
//!
//! A [`FluorescentEmitter`] is one chromophore's glow: the absorption bands that
//! populate it (`excitation`, the chromophore's own `alpha_c(lambda)`), its emission
//! lines, and its effective quantum yield `Phi`. A [`Fluorescence`] is the set of
//! emitters of one material, and travels **beside** the `GemMaterial` (never inside it):
//! an empty `Fluorescence` is "no fluorescence" and leaves every render bit-identical.
//!
//! The renderer works backward from the camera at the visible wavelength `lambda_em`.
//! The emission term of the radiative-transfer equation is
//!
//! ```text
//! L_em(lambda_em) = Int dt T(t) * Sum_c Phi_c f_c(lambda_em) Int_{300}^{lambda_em} alpha_c(l) (l / lambda_em) L_in(l) dl
//! ```
//!
//! (isotropic emission; `f_c` normalised to `Int f_c = 1`; the `l / lambda_em` factor is
//! the photon-energy ratio, so the Stokes shift loses its energy to heat and no more).
//! It is estimated by sampling a vertex: the pseudo-extinction
//! `mu_f(lambda_em) = Sum_c Phi_c f_c(lambda_em) A_c(lambda_em)` with
//! `A_c(lambda_em) = Int_{300}^{lambda_em} alpha_c` gives the rate of a vertex along the
//! path, the emitter is chosen proportional to `Phi_c f_c A_c`, the excitation wavelength
//! `lambda_ex` proportional to `alpha_c` on `[300, lambda_em)`, and the path then
//! continues at `lambda_ex` in a new, isotropic direction, weighted by
//! `lambda_ex / lambda_em`. See `optics::raytracer::transport` for the loop around it.
//!
//! `A_c` and the `lambda_ex` distribution come from a per-emitter table at 1 nm steps
//! (300-780 nm), built lazily on first use and cached inside the [`Fluorescence`], so the
//! per-ray cost is a handful of table lookups. The tabulated `alpha_c` is the bin-average
//! of the analytic bands; the sampling and `A_c` use the same table, so the estimator
//! is exactly unbiased against it (and within 1e-3 of the analytic bands for every
//! band wider than a few nm).
//!
//! # Camera wavelength sampling
//!
//! A uniform `lambda_em` over 380-780 nm would almost never land on a 2 nm emission line
//! (ruby's R line is 0.5 % of the range), so a fluorescent scene draws `lambda_em` from a
//! 50/50 mixture of the uniform density and one proportional to `mu_f(lambda)`, and
//! multiplies the sample by `1 / (400 p(lambda))` ([`Fluorescence::camera_wavelength`]).
//! The mixture keeps a uniform part, so every wavelength stays reachable and the image
//! converges unbiased; it only spends more paths where the glow is.
//!
//! # Limits (documented, deliberate)
//!
//! - Emission lines narrower than [`MIN_EMISSION_FWHM_NM`] (1 nm) are widened to 1 nm,
//!   so the line is resolvable by the 1 nm spectral sampling; the line's total area
//!   (its weight) is unchanged.
//! - `lambda_ex < lambda_em` only (Stokes shift), at most one vertex per path, no delayed
//!   emission.

use crate::optics::absorption::AbsorptionBand;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Shortest wavelength an excitation can have: the lower end of the table and of every
/// spectral quantity a fluorescence path evaluates (D65 table, dispersion clamp).
pub const EXCITATION_MIN_NM: f32 = 300.0;
/// Longest wavelength a fluorescence path carries (the camera range's upper edge).
pub const EMISSION_MAX_NM: f32 = 780.0;
/// Emission lines are widened to at least this FWHM (nm) for sampling.
pub const MIN_EMISSION_FWHM_NM: f32 = 1.0;
/// Most emitters a valid [`Fluorescence`] has (see [`Fluorescence::validate`]).
pub const MAX_EMITTERS: usize = 16;
/// Most excitation bands, and most emission bands, one emitter has.
pub const MAX_BANDS: usize = 8;

/// `2 sqrt(2 ln 2)`: FWHM = this * sigma for a Gaussian.
const FWHM_TO_SIGMA: f32 = 2.354_82;
/// Number of 1 nm bins of the excitation table, `[300, 780]`.
const BINS: usize = (EMISSION_MAX_NM - EXCITATION_MIN_NM) as usize;

/// One Gaussian emission line or band.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EmissionBand {
    /// Centre wavelength, nm.
    pub centre_nm: f32,
    /// Full width at half maximum, nm (widened to [`MIN_EMISSION_FWHM_NM`] for sampling).
    pub fwhm_nm: f32,
    /// Relative weight (area share) of this band in its emitter's emission spectrum;
    /// the emitter normalises the weights to `Int f = 1`.
    pub weight: f32,
}

impl EmissionBand {
    /// Builds a band.
    #[must_use]
    pub const fn new(centre_nm: f32, fwhm_nm: f32, weight: f32) -> Self {
        Self {
            centre_nm,
            fwhm_nm,
            weight,
        }
    }
}

/// One fluorescent chromophore: what excites it, what it emits and how efficiently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FluorescentEmitter {
    /// The chromophore's own absorption bands `alpha_c(lambda)` (per mm, the same unit
    /// as the material's absorption), scaled by its effective concentration: "what
    /// absorbs is what glows".
    pub excitation: Vec<AbsorptionBand>,
    /// Emission lines and bands, `f_c(lambda)`; the weights are normalised so that
    /// `Int f_c dlambda = 1`.
    pub emission: Vec<EmissionBand>,
    /// Effective quantum yield `Phi_eff` after quenching, `0..=1`.
    pub quantum_yield: f32,
}

impl FluorescentEmitter {
    /// Total excitation absorption `alpha_c(lambda_nm)` (analytic, per mm; negative band
    /// peaks are floored at zero).
    #[must_use]
    pub fn excitation_absorption(&self, lambda_nm: f32) -> f32 {
        self.excitation
            .iter()
            .map(|b| b.evaluate(lambda_nm))
            .sum::<f32>()
            .max(0.0)
    }

    /// The normalised emission spectrum `f_c(lambda_nm)` in 1/nm (analytic;
    /// lines narrower than [`MIN_EMISSION_FWHM_NM`] widened).
    #[must_use]
    pub fn emission_density(&self, lambda_nm: f32) -> f32 {
        EmitterTable::lines(&self.emission)
            .iter()
            .map(|l| l.eval(lambda_nm))
            .sum()
    }
}

/// One normalised Gaussian line of an emission spectrum.
#[derive(Debug, Clone, Copy)]
struct Line {
    centre_nm: f32,
    inv_sigma: f32,
    /// `weight / (sigma sqrt(2 pi))`: the density at the centre, in 1/nm.
    amplitude: f32,
}

impl Line {
    fn eval(self, lambda_nm: f32) -> f32 {
        let z = (lambda_nm - self.centre_nm) * self.inv_sigma;
        self.amplitude * (-0.5 * z * z).exp()
    }
}

/// An emitter's precomputed sampling data.
#[derive(Debug, Clone)]
struct EmitterTable {
    quantum_yield: f32,
    lines: Vec<Line>,
    /// `cumulative[i] = Int_{300}^{300+i nm} alpha_c`, piecewise linear between the
    /// 1 nm nodes (so `alpha_c` is the bin average, constant inside each bin).
    cumulative: Vec<f32>,
}

impl EmitterTable {
    /// The normalised, widened emission lines of `emission`.
    fn lines(emission: &[EmissionBand]) -> Vec<Line> {
        let total: f32 = emission
            .iter()
            .map(|b| b.weight.max(0.0))
            .filter(|w| w.is_finite())
            .sum();
        if total <= 0.0 {
            return Vec::new();
        }
        emission
            .iter()
            .filter(|b| b.weight > 0.0 && b.weight.is_finite())
            .map(|b| {
                let sigma = b.fwhm_nm.max(MIN_EMISSION_FWHM_NM) / FWHM_TO_SIGMA;
                Line {
                    centre_nm: b.centre_nm,
                    inv_sigma: 1.0 / sigma,
                    amplitude: (b.weight / total) / (sigma * (2.0 * std::f32::consts::PI).sqrt()),
                }
            })
            .collect()
    }

    fn build(emitter: &FluorescentEmitter) -> Self {
        let alpha = |i: usize| emitter.excitation_absorption(EXCITATION_MIN_NM + i as f32);
        let mut cumulative = Vec::with_capacity(BINS + 1);
        cumulative.push(0.0f32);
        let mut acc = 0.0f64;
        let mut prev = alpha(0);
        for i in 1..=BINS {
            let next = alpha(i);
            acc = 0.5f64.mul_add(f64::from(prev + next), acc);
            cumulative.push(acc as f32);
            prev = next;
        }
        Self {
            quantum_yield: emitter.quantum_yield.clamp(0.0, 1.0),
            lines: Self::lines(&emitter.emission),
            cumulative,
        }
    }

    /// `Phi f_c(lambda)`.
    fn yield_density(&self, lambda_nm: f32) -> f32 {
        self.quantum_yield * self.lines.iter().map(|l| l.eval(lambda_nm)).sum::<f32>()
    }

    /// `A_c(lambda) = Int_{300}^{lambda} alpha_c`, interpolating the table.
    fn excitation_area(&self, lambda_nm: f32) -> f32 {
        let pos = lambda_nm.clamp(EXCITATION_MIN_NM, EMISSION_MAX_NM) - EXCITATION_MIN_NM;
        let i = (pos.floor() as usize).min(BINS - 1);
        let frac = pos - i as f32;
        frac.mul_add(
            self.cumulative[i + 1] - self.cumulative[i],
            self.cumulative[i],
        )
    }

    /// The excitation wavelength at CDF fraction `u` of `[300, lambda_em)`.
    fn sample_excitation(&self, lambda_em: f32, u: f32) -> f32 {
        let area = self.excitation_area(lambda_em);
        let target = u.clamp(0.0, 1.0) * area;
        // First node whose cumulative exceeds the target; the bin is the one before.
        let upper = self.cumulative.partition_point(|&c| c <= target);
        let i = upper.saturating_sub(1).min(BINS - 1);
        let span = self.cumulative[i + 1] - self.cumulative[i];
        let frac = if span > 0.0 {
            ((target - self.cumulative[i]) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let lambda = EXCITATION_MIN_NM + i as f32 + frac;
        // Strictly below the emission wavelength (Stokes shift only).
        lambda.min(lambda_em - 1e-3)
    }
}

/// Everything a trace needs, per emitter, built once.
#[derive(Debug)]
struct Tables {
    emitters: Vec<EmitterTable>,
    /// The guide density for the camera wavelength: the cumulative distribution of
    /// `mu_f(lambda)` over the 1 nm bins of `[380, 780]` (401 nodes, `0 ..= 1`), empty when
    /// no emitter can fire anywhere in the visible range.
    guide_cdf: Vec<f32>,
}

/// Camera wavelengths span the visible range.
const CAMERA_MIN_NM: f32 = 380.0;
const CAMERA_SPAN_NM: f32 = 400.0;

/// The wire form of [`Fluorescence`]: the emitters, nothing else (the cache is rebuilt).
#[derive(Serialize, Deserialize)]
struct FluorescenceWire {
    emitters: Vec<FluorescentEmitter>,
}

/// The fluorescent emitters of one material. Empty means "not fluorescent": every
/// trace with an empty `Fluorescence` is bit-identical to one without the feature.
///
/// Immutable after construction ([`Self::new`]), because it caches the sampling tables
/// the first trace builds. Serialises as `{ emitters }`.
#[derive(Serialize, Deserialize)]
#[serde(from = "FluorescenceWire", into = "FluorescenceWire")]
pub struct Fluorescence {
    emitters: Vec<FluorescentEmitter>,
    tables: OnceLock<Tables>,
}

impl Fluorescence {
    /// A `Fluorescence` of `emitters`.
    #[must_use]
    pub const fn new(emitters: Vec<FluorescentEmitter>) -> Self {
        Self {
            emitters,
            tables: OnceLock::new(),
        }
    }

    /// The shared empty `Fluorescence`, for call sites that have none.
    #[must_use]
    pub fn none() -> &'static Self {
        static NONE: Fluorescence = Fluorescence::new(Vec::new());
        &NONE
    }

    /// The emitters.
    #[must_use]
    pub fn emitters(&self) -> &[FluorescentEmitter] {
        &self.emitters
    }

    /// `true` for "no fluorescence": no emitters at all. (An emitter with `Phi = 0` still
    /// counts: the scene then traces single-wavelength paths with a zero vertex rate.)
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.emitters.is_empty()
    }

    /// Checks the limits the net layer enforces: at most [`MAX_EMITTERS`] emitters of at
    /// most [`MAX_BANDS`] excitation and [`MAX_BANDS`] emission bands each, every value
    /// finite, `Phi` in `[0, 1]`, widths positive, peaks and weights non-negative.
    ///
    /// # Errors
    ///
    /// A message naming the first violation.
    pub fn validate(&self) -> Result<(), String> {
        if self.emitters.len() > MAX_EMITTERS {
            return Err(format!(
                "fluorescence has {} emitters (max {MAX_EMITTERS})",
                self.emitters.len()
            ));
        }
        for (i, e) in self.emitters.iter().enumerate() {
            if !(e.quantum_yield.is_finite() && (0.0..=1.0).contains(&e.quantum_yield)) {
                return Err(format!(
                    "fluorescence emitter {i}: quantum_yield {} outside [0, 1]",
                    e.quantum_yield
                ));
            }
            if e.excitation.len() > MAX_BANDS || e.emission.len() > MAX_BANDS {
                return Err(format!(
                    "fluorescence emitter {i}: more than {MAX_BANDS} excitation or emission bands"
                ));
            }
            for b in &e.excitation {
                let ok = [b.center_nm, b.width_nm, b.peak]
                    .iter()
                    .all(|v| v.is_finite())
                    && b.width_nm > 0.0
                    && b.peak >= 0.0
                    && b.center_nm > 0.0;
                if !ok {
                    return Err(format!(
                        "fluorescence emitter {i}: invalid excitation band {b:?}"
                    ));
                }
            }
            for b in &e.emission {
                let ok = [b.centre_nm, b.fwhm_nm, b.weight]
                    .iter()
                    .all(|v| v.is_finite())
                    && b.fwhm_nm > 0.0
                    && b.weight >= 0.0
                    && b.centre_nm > 0.0;
                if !ok {
                    return Err(format!(
                        "fluorescence emitter {i}: invalid emission band {b:?}"
                    ));
                }
            }
        }
        Ok(())
    }

    fn tables(&self) -> &Tables {
        self.tables.get_or_init(|| {
            let emitters: Vec<EmitterTable> =
                self.emitters.iter().map(EmitterTable::build).collect();
            let mu = |i: usize| -> f32 {
                let lambda = CAMERA_MIN_NM + i as f32;
                emitters
                    .iter()
                    .map(|t| t.yield_density(lambda) * t.excitation_area(lambda))
                    .sum()
            };
            let bins = CAMERA_SPAN_NM as usize;
            let mut cdf = Vec::with_capacity(bins + 1);
            cdf.push(0.0f64);
            let mut prev = mu(0);
            for i in 1..=bins {
                let next = mu(i);
                let last = cdf[i - 1];
                cdf.push(0.5f64.mul_add(f64::from(prev + next), last));
                prev = next;
            }
            let total = cdf[bins];
            let guide_cdf = if total > 0.0 && total.is_finite() {
                cdf.iter().map(|&c| (c / total) as f32).collect()
            } else {
                Vec::new()
            };
            Tables {
                emitters,
                guide_cdf,
            }
        })
    }

    /// Draws the camera wavelength `lambda_em` of a path from `hero_rand` in `[0, 1)`, and
    /// the weight `1 / (400 p(lambda_em))` the path's result must be multiplied by.
    ///
    /// `p` is the 50/50 mixture of the uniform density over 380-780 nm and the density
    /// proportional to the pseudo-extinction `mu_f` (module doc, "Camera wavelength
    /// sampling"); with no emitter able to fire it is uniform and the weight is exactly 1.
    #[must_use]
    pub fn camera_wavelength(&self, hero_rand: f32) -> (f32, f32) {
        let cdf = &self.tables().guide_cdf;
        if cdf.is_empty() {
            return (hero_rand.mul_add(CAMERA_SPAN_NM, CAMERA_MIN_NM), 1.0);
        }
        let h = hero_rand.clamp(0.0, 1.0);
        let lambda = if h < 0.5 {
            (2.0 * h).mul_add(CAMERA_SPAN_NM, CAMERA_MIN_NM)
        } else {
            let u = 2.0 * (h - 0.5);
            let bins = cdf.len() - 1;
            let i = cdf
                .partition_point(|&c| c <= u)
                .saturating_sub(1)
                .min(bins - 1);
            let span = cdf[i + 1] - cdf[i];
            let frac = if span > 0.0 {
                ((u - cdf[i]) / span).clamp(0.0, 1.0)
            } else {
                0.0
            };
            CAMERA_MIN_NM + i as f32 + frac
        };
        let p = 0.5f32.mul_add(self.guide_density(lambda), 0.5 / CAMERA_SPAN_NM);
        (lambda, 1.0 / (CAMERA_SPAN_NM * p))
    }

    /// The guide density (per nm, integrating to 1 over 380-780 nm; 0 when no emitter can
    /// fire) at `lambda_nm`.
    fn guide_density(&self, lambda_nm: f32) -> f32 {
        let cdf = &self.tables().guide_cdf;
        if cdf.is_empty() {
            return 0.0;
        }
        let bins = cdf.len() - 1;
        let i = ((lambda_nm - CAMERA_MIN_NM).floor().max(0.0) as usize).min(bins - 1);
        cdf[i + 1] - cdf[i]
    }

    /// The fluorescence pseudo-extinction `mu_f(lambda_em) = Sum_c Phi_c f_c(lambda_em)
    /// A_c(lambda_em)` in 1/mm (the same unit as the material's absorption): the rate of
    /// a fluorescence vertex along a path at the camera wavelength `lambda_em`.
    #[must_use]
    pub fn pseudo_extinction(&self, lambda_em: f32) -> f32 {
        self.tables()
            .emitters
            .iter()
            .map(|t| t.yield_density(lambda_em) * t.excitation_area(lambda_em))
            .sum()
    }

    /// Picks the emitter (proportional to `Phi_c f_c A_c`, from `u_emitter`) and its
    /// excitation wavelength `lambda_ex` in `[300, lambda_em)` (proportional to
    /// `alpha_c`, from `u_lambda`). `None` when `mu_f(lambda_em)` is zero.
    #[must_use]
    pub fn sample_excitation(&self, lambda_em: f32, u_emitter: f32, u_lambda: f32) -> Option<f32> {
        let tables = self.tables();
        let weight = |t: &EmitterTable| t.yield_density(lambda_em) * t.excitation_area(lambda_em);
        let total: f32 = tables.emitters.iter().map(weight).sum();
        if total <= 0.0 || !total.is_finite() {
            return None;
        }
        let mut target = u_emitter.clamp(0.0, 1.0) * total;
        let mut chosen = None;
        for t in &tables.emitters {
            let w = weight(t);
            if w <= 0.0 {
                continue;
            }
            chosen = Some(t);
            if target < w {
                break;
            }
            target -= w;
        }
        chosen.map(|t| t.sample_excitation(lambda_em, u_lambda))
    }
}

impl Default for Fluorescence {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl Clone for Fluorescence {
    fn clone(&self) -> Self {
        Self::new(self.emitters.clone())
    }
}

impl PartialEq for Fluorescence {
    fn eq(&self, other: &Self) -> bool {
        self.emitters == other.emitters
    }
}

impl std::fmt::Debug for Fluorescence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The sampling-table cache is derived data and deliberately left out.
        f.debug_struct("Fluorescence")
            .field("emitters", &self.emitters)
            .finish_non_exhaustive()
    }
}

impl From<FluorescenceWire> for Fluorescence {
    fn from(wire: FluorescenceWire) -> Self {
        Self::new(wire.emitters)
    }
}

impl From<Fluorescence> for FluorescenceWire {
    fn from(fluorescence: Fluorescence) -> Self {
        Self {
            emitters: fluorescence.emitters,
        }
    }
}

/// A fluorescence vertex the transport decided to take.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FluorescenceVertex {
    /// Excitation wavelength the path continues at, nm.
    pub lambda_ex: f32,
    /// Photon-energy ratio `lambda_ex / lambda_em` (`< 1`): the path weight factor that
    /// conserves energy across the Stokes shift.
    pub energy_ratio: f32,
}

/// Samples the emitter and excitation wavelength of a vertex at camera wavelength
/// `lambda_em` and returns it with the energy ratio. `None` if no emitter can fire at
/// `lambda_em`.
pub(crate) fn sample_vertex(
    fluorescence: &Fluorescence,
    lambda_em: f32,
    u_emitter: f32,
    u_lambda: f32,
) -> Option<FluorescenceVertex> {
    let lambda_ex = fluorescence.sample_excitation(lambda_em, u_emitter, u_lambda)?;
    Some(FluorescenceVertex {
        lambda_ex,
        energy_ratio: lambda_ex / lambda_em,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ruby-like emitter: excitation bands at 410 nm (3500 cm^-1) and 556 nm
    /// (2800 cm^-1), the Cr3+ R-line at 694 nm (FWHM 2 nm), `Phi = 0.9`.
    fn ruby_like(peak: f32) -> FluorescentEmitter {
        FluorescentEmitter {
            excitation: vec![
                AbsorptionBand::energy(410.0, 3500.0, peak),
                AbsorptionBand::energy(556.0, 2800.0, peak),
            ],
            emission: vec![EmissionBand::new(694.0, 2.0, 1.0)],
            quantum_yield: 0.9,
        }
    }

    #[test]
    fn empty_fluorescence_is_inert() {
        let none = Fluorescence::default();
        assert!(none.is_empty());
        assert_eq!(none.pseudo_extinction(694.0), 0.0);
        assert_eq!(none.sample_excitation(694.0, 0.5, 0.5), None);
        assert!(Fluorescence::none().is_empty());
        assert_eq!(none, Fluorescence::new(Vec::new()));
    }

    #[test]
    fn emission_density_integrates_to_one_even_for_a_widened_line() {
        // A 0.2 nm line is widened to 1 nm; its area must stay 1.
        let mut e = ruby_like(1.0);
        e.emission = vec![
            EmissionBand::new(692.9, 0.2, 1.0),
            EmissionBand::new(694.3, 0.2, 2.0),
        ];
        let area: f32 = (600..800)
            .flat_map(|i| (0..20).map(move |j| (j as f32).mul_add(0.05, i as f32)))
            .map(|l| e.emission_density(l) * 0.05)
            .sum();
        assert!((area - 1.0).abs() < 2e-3, "area {area}");
    }

    #[test]
    fn pseudo_extinction_matches_brute_force_quadrature() {
        let e = ruby_like(0.8);
        let fl = Fluorescence::new(vec![e.clone()]);
        for lambda_em in [450.0f32, 560.0, 693.0, 694.0, 695.0] {
            // Independent of the table: fine quadrature of the analytic bands.
            let steps = (lambda_em - EXCITATION_MIN_NM) * 20.0;
            let n = steps as usize;
            let area: f64 = (0..n)
                .map(|i| {
                    let l = EXCITATION_MIN_NM + (i as f32 + 0.5) / 20.0;
                    f64::from(e.excitation_absorption(l)) / 20.0
                })
                .sum();
            let expected = 0.9 * f64::from(e.emission_density(lambda_em)) * area;
            let got = f64::from(fl.pseudo_extinction(lambda_em));
            let tol = 2e-3 * expected.max(1e-3);
            assert!(
                (got - expected).abs() < tol,
                "{lambda_em}: {got} vs {expected}"
            );
        }
    }

    #[test]
    fn excitation_samples_follow_the_absorption_bands_below_the_emission() {
        let fl = Fluorescence::new(vec![ruby_like(1.0)]);
        let mut near_410 = 0u32;
        let mut near_556 = 0u32;
        let n = 4000u32;
        for k in 0..n {
            let u = (k as f32 + 0.5) / n as f32;
            let l = fl.sample_excitation(694.0, 0.5, u).unwrap();
            assert!((EXCITATION_MIN_NM..694.0).contains(&l), "{l}");
            if (l - 410.0).abs() < 60.0 {
                near_410 += 1;
            }
            if (l - 556.0).abs() < 60.0 {
                near_556 += 1;
            }
        }
        // Both bands carry most of the weight; the 556 nm band is wider in nm.
        assert!(near_410 + near_556 > n * 9 / 10);
        assert!(near_556 > near_410, "{near_410} vs {near_556}");
        // Emission below the first band's reach: no excitation window at all.
        let blue = Fluorescence::new(vec![FluorescentEmitter {
            emission: vec![EmissionBand::new(305.0, 2.0, 1.0)],
            ..ruby_like(1.0)
        }]);
        assert!(blue.pseudo_extinction(450.0) < 1e-6);
    }

    /// Furnace energy test (plan section 6, item 3), at the estimator level.
    ///
    /// Phi = 1, emission spectrum equal to the excitation spectrum. With unit incident
    /// radiance at every wavelength and no transmittance loss, the emitted spectral
    /// power at `lambda_em` is `mu_f(lambda_em) * E[energy_ratio]` per unit path (the
    /// expectation over the vertex's own `lambda_ex` sampling). Integrated over
    /// `lambda_em`, that must equal the brute-force double integral
    /// `Int f(l_em) Int_{300}^{l_em} alpha(l) l / l_em dl dl_em` of the analytic
    /// bands to 1 %: the absorbed energy `Int alpha` times the mean `lambda_ex /
    /// lambda_em` factor, and never more than the absorbed energy. A weight that were
    /// doubled (`2 * energy_ratio`) doubles the left side and fails both checks.
    #[test]
    fn furnace_emitted_energy_is_absorbed_energy_times_the_stokes_factor() {
        let band = AbsorptionBand::new(450.0, 25.0, 0.3);
        let emitter = FluorescentEmitter {
            excitation: vec![band],
            // Emission spectrum equal to the excitation spectrum (same centre, same
            // width: FWHM = 2.35482 sigma).
            emission: vec![EmissionBand::new(450.0, 25.0 * FWHM_TO_SIGMA, 1.0)],
            quantum_yield: 1.0,
        };
        let fl = Fluorescence::new(vec![emitter.clone()]);

        let draws = 2000u32;
        let mut emitted = 0.0f64;
        for lambda_em in 380..=780 {
            let lambda_em = lambda_em as f32;
            let mu_f = f64::from(fl.pseudo_extinction(lambda_em));
            if mu_f <= 0.0 {
                continue;
            }
            let mut ratio = 0.0f64;
            for k in 0..draws {
                let u = (k as f32 + 0.5) / draws as f32;
                if let Some(v) = sample_vertex(&fl, lambda_em, 0.5, u) {
                    ratio += f64::from(v.energy_ratio);
                }
            }
            emitted += mu_f * ratio / f64::from(draws);
        }

        // Brute force from the analytic bands, no tables.
        let mut expected = 0.0f64;
        let mut absorbed = 0.0f64;
        for i in 0..4800 {
            let l = (i as f32 + 0.5).mul_add(0.1, 300.0);
            absorbed = f64::from(emitter.excitation_absorption(l)).mul_add(0.1, absorbed);
        }
        for lambda_em in 380..=780 {
            let lambda_em = lambda_em as f32;
            let f = f64::from(emitter.emission_density(lambda_em));
            let mut inner = 0.0f64;
            for i in 0..((lambda_em - 300.0) * 10.0) as usize {
                let l = (i as f32 + 0.5).mul_add(0.1, 300.0);
                inner = (f64::from(emitter.excitation_absorption(l)) * 0.1)
                    .mul_add(f64::from(l / lambda_em), inner);
            }
            expected = f.mul_add(inner, expected);
        }
        let rel = (emitted - expected).abs() / expected;
        assert!(
            rel < 0.01,
            "emitted {emitted} vs expected {expected} ({rel})"
        );
        assert!(
            emitted < absorbed * 1.01,
            "emitted {emitted} exceeds the absorbed energy {absorbed}"
        );
        // And it is not trivially tiny: a real fraction of the absorbed energy.
        assert!(emitted > 0.3 * absorbed, "{emitted} vs {absorbed}");
    }

    /// The camera-wavelength mixture is a proper density: with weight `1 / (400 p)`, the
    /// weighted average of any integrand over the `hero_rand` stream is the plain
    /// 380-780 nm average, for the flat integrand and a smooth one, and it puts about half
    /// of the samples within a few nm of the emission line.
    #[test]
    fn camera_wavelength_mixture_is_unbiased_and_finds_the_line() {
        let fl = Fluorescence::new(vec![ruby_like(1.0)]);
        let n = 200_000u32;
        let (mut mean_w, mut mean_g, mut near_line) = (0.0f64, 0.0f64, 0u32);
        for k in 0..n {
            let h = (k as f32 + 0.5) / n as f32;
            let (lambda, w) = fl.camera_wavelength(h);
            assert!((380.0..=780.0).contains(&lambda), "{lambda}");
            mean_w += f64::from(w);
            let g = (-0.5 * ((lambda - 550.0) / 40.0).powi(2)).exp();
            mean_g = f64::from(w).mul_add(f64::from(g), mean_g);
            near_line += u32::from((lambda - 694.0).abs() < 4.0);
        }
        mean_w /= f64::from(n);
        mean_g /= f64::from(n);
        let exact_g: f64 = (380..780)
            .map(|i| (-0.5 * ((f64::from(i) + 0.5 - 550.0) / 40.0).powi(2)).exp())
            .sum::<f64>()
            / 400.0;
        assert!((mean_w - 1.0).abs() < 2e-3, "E[w] = {mean_w}");
        assert!(
            (mean_g - exact_g).abs() < 3e-3 * exact_g,
            "{mean_g} vs {exact_g}"
        );
        assert!(near_line > n * 4 / 10, "{near_line}");
        // No emitter: uniform, weight exactly 1.
        let none = Fluorescence::default();
        assert_eq!(none.camera_wavelength(0.25), (480.0, 1.0));
    }

    #[test]
    fn validate_rejects_bad_values() {
        assert!(Fluorescence::new(vec![ruby_like(1.0)]).validate().is_ok());
        let mut bad = ruby_like(1.0);
        bad.quantum_yield = 1.5;
        assert!(Fluorescence::new(vec![bad]).validate().is_err());
        let mut nan = ruby_like(1.0);
        nan.emission[0].centre_nm = f32::NAN;
        assert!(Fluorescence::new(vec![nan]).validate().is_err());
        assert!(
            Fluorescence::new(vec![ruby_like(1.0); 17])
                .validate()
                .is_err()
        );
        let mut many = ruby_like(1.0);
        many.excitation = vec![AbsorptionBand::new(450.0, 20.0, 1.0); 9];
        assert!(Fluorescence::new(vec![many]).validate().is_err());
    }
}
