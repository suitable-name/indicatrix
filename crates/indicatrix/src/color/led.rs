//! The CIE 15:2018 LED illuminants (the `zoning` feature).
//!
//! Nine relative spectral power distributions representing typical LED lamps: five
//! phosphor-converted blue-pumped whites (`B1` to `B5`), one blue-hybrid (`BH1`), one RGB mix
//! (`RGB1`) and two violet-pumped whites (`V1`, `V2`). They are the stand-in for the backlight
//! of the rig photos when the owner has no measured spectrum (plan section 4.4).
//!
//! Reached through [`Illuminant::Led`](super::body_color::Illuminant::Led), which carries a
//! [`LedKind`]. The variant (and this module) exist only with the `zoning` feature, so the
//! default build's `Illuminant` is unchanged.
//!
//! # Data
//!
//! `data/cie_illum_leds_5nm.csv`: 81 rows, 380 to 780 nm in 5 nm steps, columns in the order
//! of [`LedKind::ALL`]. It is every fifth row of the CIE's 1 nm dataset (source, DOI, licence
//! note and retrieval caveat are in the file header). Values are relative; they are not
//! normalised to a common level by the CIE, so [`LedKind::spectral_power`] normalises each
//! curve to a peak of 1.
//!
//! The dataset is embedded with `include_str!` and parsed once on first use.

use std::sync::OnceLock;

/// First tabulated wavelength in nanometres.
pub const LED_FIRST_NM: f64 = 380.0;
/// Spacing of the tabulated wavelengths in nanometres.
pub const LED_STEP_NM: f64 = 5.0;
/// Number of tabulated wavelengths (380 to 780 nm inclusive at 5 nm).
pub const LED_SAMPLES: usize = 81;

/// Wavelength in nanometres of the table's `index`-th sample.
#[must_use]
pub const fn sample_wavelength_nm(index: usize) -> f64 {
    LED_STEP_NM.mul_add(index as f64, LED_FIRST_NM)
}

const CSV: &str = include_str!("../../data/cie_illum_leds_5nm.csv");

/// One of the nine CIE 15:2018 LED illuminants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LedKind {
    /// Phosphor-converted blue LED, CCT about 2733 K.
    B1,
    /// Phosphor-converted blue LED, CCT about 2998 K.
    B2,
    /// Phosphor-converted blue LED, CCT about 4103 K.
    B3,
    /// Phosphor-converted blue LED, CCT about 5109 K.
    B4,
    /// Phosphor-converted blue LED, CCT about 6598 K.
    B5,
    /// Blue LED with a red LED added (hybrid), CCT about 2851 K.
    Bh1,
    /// Red, green and blue LED mix, CCT about 2840 K.
    Rgb1,
    /// Violet-pumped phosphor LED, CCT about 2724 K.
    V1,
    /// Violet-pumped phosphor LED, CCT about 4070 K.
    V2,
}

impl LedKind {
    /// All nine, in the column order of the data file.
    pub const ALL: [Self; 9] = [
        Self::B1,
        Self::B2,
        Self::B3,
        Self::B4,
        Self::B5,
        Self::Bh1,
        Self::Rgb1,
        Self::V1,
        Self::V2,
    ];

    /// The CIE's name, for example `"LED-B3"`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::B1 => "LED-B1",
            Self::B2 => "LED-B2",
            Self::B3 => "LED-B3",
            Self::B4 => "LED-B4",
            Self::B5 => "LED-B5",
            Self::Bh1 => "LED-BH1",
            Self::Rgb1 => "LED-RGB1",
            Self::V1 => "LED-V1",
            Self::V2 => "LED-V2",
        }
    }

    /// The correlated colour temperature in kelvin the CIE lists for this illuminant (CIE
    /// 15:2018 table 12.2, rounded to whole kelvin). Used to pick the closest LED to a
    /// chosen CCT and by the tests as the chromaticity check.
    #[must_use]
    pub const fn published_cct_k(self) -> f64 {
        match self {
            Self::B1 => 2733.0,
            Self::B2 => 2998.0,
            Self::B3 => 4103.0,
            Self::B4 => 5109.0,
            Self::B5 => 6598.0,
            Self::Bh1 => 2851.0,
            Self::Rgb1 => 2840.0,
            Self::V1 => 2724.0,
            Self::V2 => 4070.0,
        }
    }

    /// The illuminant whose published CCT is closest to `cct_k` (the first in
    /// [`Self::ALL`] order on an exact tie).
    #[must_use]
    pub fn closest_to_cct(cct_k: f64) -> Self {
        let mut best = Self::B1;
        let mut best_gap = f64::INFINITY;
        for kind in Self::ALL {
            let gap = (kind.published_cct_k() - cct_k).abs();
            if gap < best_gap {
                best = kind;
                best_gap = gap;
            }
        }
        best
    }

    /// Index of this illuminant's column in [`Self::ALL`] and in the data file.
    #[must_use]
    pub const fn column(self) -> usize {
        match self {
            Self::B1 => 0,
            Self::B2 => 1,
            Self::B3 => 2,
            Self::B4 => 3,
            Self::B5 => 4,
            Self::Bh1 => 5,
            Self::Rgb1 => 6,
            Self::V1 => 7,
            Self::V2 => 8,
        }
    }

    /// The tabulated curve at 5 nm from 380 nm ([`LED_SAMPLES`] values), exactly as published
    /// (relative, not normalised).
    #[must_use]
    pub fn samples(self) -> &'static [f64; LED_SAMPLES] {
        &table().columns[self.column()]
    }

    /// Relative spectral power at `lambda_nm`, linearly interpolated between the 5 nm samples
    /// and normalised so the curve's peak is 1. Zero outside 380 to 780 nm.
    #[must_use]
    pub fn spectral_power(self, lambda_nm: f64) -> f64 {
        let t = table();
        let pos = (lambda_nm - LED_FIRST_NM) / LED_STEP_NM;
        if !(0.0..=(LED_SAMPLES - 1) as f64).contains(&pos) {
            return 0.0;
        }
        let col = &t.columns[self.column()];
        let lo = (pos.floor() as usize).min(LED_SAMPLES - 2);
        let frac = pos - lo as f64;
        let value = col[lo].mul_add(1.0 - frac, col[lo + 1] * frac);
        value / t.peaks[self.column()]
    }
}

struct LedTable {
    columns: [[f64; LED_SAMPLES]; 9],
    peaks: [f64; 9],
}

fn table() -> &'static LedTable {
    static TABLE: OnceLock<LedTable> = OnceLock::new();
    TABLE.get_or_init(|| parse(CSV))
}

/// Parses the embedded CSV. Panics on malformed data, which is a build-time constant that the
/// tests exercise.
fn parse(csv: &str) -> LedTable {
    let mut columns = [[0.0_f64; LED_SAMPLES]; 9];
    let mut row = 0_usize;
    for line in csv.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split(',');
        let nm: f64 = fields
            .next()
            .and_then(|f| f.trim().parse().ok())
            .expect("CIE LED table: wavelength field");
        assert!(
            row < LED_SAMPLES && (nm - sample_wavelength_nm(row)).abs() < 1e-9,
            "CIE LED table: row {row} has wavelength {nm}, expected a 5 nm grid from 380"
        );
        for column in &mut columns {
            column[row] = fields
                .next()
                .and_then(|f| f.trim().parse().ok())
                .expect("CIE LED table: value field");
        }
        assert!(
            fields.next().is_none(),
            "CIE LED table: extra field in row {row}"
        );
        row += 1;
    }
    assert_eq!(
        row, LED_SAMPLES,
        "CIE LED table: expected {LED_SAMPLES} rows"
    );
    let mut peaks = [1.0_f64; 9];
    for (peak, column) in peaks.iter_mut().zip(&columns) {
        *peak = column.iter().copied().fold(0.0, f64::max);
        assert!(*peak > 0.0, "CIE LED table: an all-zero column");
    }
    LedTable { columns, peaks }
}

#[cfg(test)]
mod tests {
    use super::{LED_SAMPLES, LedKind, sample_wavelength_nm};
    use crate::color::cie1931::cie_1931_cmf;

    /// CIE 1931 xy of the curve, integrated at the table's own 5 nm grid with the crate's CMF.
    fn xy(kind: LedKind) -> (f64, f64) {
        let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);
        for (i, &s) in kind.samples().iter().enumerate() {
            let cmf = cie_1931_cmf(sample_wavelength_nm(i) as f32);
            x = s.mul_add(f64::from(cmf[0]), x);
            y = s.mul_add(f64::from(cmf[1]), y);
            z = s.mul_add(f64::from(cmf[2]), z);
        }
        let sum = x + y + z;
        (x / sum, y / sum)
    }

    /// `McCamy`'s CCT approximation (accurate to a few kelvin near the Planckian locus).
    fn mccamy_cct(x: f64, y: f64) -> f64 {
        let n = (x - 0.3320) / (0.1858 - y);
        449.0_f64
            .mul_add(n, 3525.0)
            .mul_add(n, 6823.3)
            .mul_add(n, 5520.33)
    }

    #[test]
    fn table_parses_with_the_expected_shape() {
        for kind in LedKind::ALL {
            let s = kind.samples();
            assert_eq!(s.len(), LED_SAMPLES);
            assert!(
                s.iter().all(|v| v.is_finite() && *v > 0.0),
                "{}",
                kind.name()
            );
        }
        // First and last published values of LED-B1 (1 nm file rows 380 and 780).
        assert!((LedKind::B1.samples()[0] - 0.002_748_895_001_264_59).abs() < 1e-15);
        assert!((LedKind::B1.samples()[LED_SAMPLES - 1] - 0.609_884_583_683_814).abs() < 1e-12);
    }

    #[test]
    fn column_order_matches_all() {
        for (i, kind) in LedKind::ALL.iter().enumerate() {
            assert_eq!(kind.column(), i);
        }
    }

    /// Tolerance: the published CCT is rounded to whole kelvin and `McCamy`'s formula is
    /// accurate to about 2 K on the locus, but the LEDs sit slightly off the locus and the
    /// CMF here is the crate's own 5 nm sum, so allow 2 % of the CCT. Source of the CCTs:
    /// CIE 15:2018 table 12.2 (`LedKind::published_cct_k`). The same check was run during
    /// authoring with an independent analytic CMF and agreed within 1.3 %.
    #[test]
    fn chromaticity_matches_the_published_cct() {
        for kind in LedKind::ALL {
            let (x, y) = xy(kind);
            let cct = mccamy_cct(x, y);
            let want = kind.published_cct_k();
            assert!(
                (cct - want).abs() < 0.02 * want,
                "{}: xy=({x:.4}, {y:.4}) gives CCT {cct:.0} K, published {want} K",
                kind.name()
            );
        }
    }

    #[test]
    fn warm_to_cool_ordering_of_the_phosphor_whites() {
        let cct = |k| {
            let (x, y) = xy(k);
            mccamy_cct(x, y)
        };
        assert!(cct(LedKind::B1) < cct(LedKind::B2));
        assert!(cct(LedKind::B2) < cct(LedKind::B3));
        assert!(cct(LedKind::B3) < cct(LedKind::B4));
        assert!(cct(LedKind::B4) < cct(LedKind::B5));
    }

    #[test]
    fn spectral_power_is_peak_normalised_interpolated_and_zero_outside() {
        for kind in LedKind::ALL {
            let peak = (0..LED_SAMPLES)
                .map(|i| kind.spectral_power(sample_wavelength_nm(i)))
                .fold(0.0, f64::max);
            assert!((peak - 1.0).abs() < 1e-12, "{}", kind.name());
            assert!(kind.spectral_power(379.9).abs() < f64::EPSILON);
            assert!(kind.spectral_power(780.1).abs() < f64::EPSILON);
        }
        // Midpoint between two samples is their mean (linear interpolation).
        let k = LedKind::B3;
        let a = k.spectral_power(500.0);
        let b = k.spectral_power(505.0);
        assert!((k.spectral_power(502.5) - f64::midpoint(a, b)).abs() < 1e-12);
        // The blue pump of B5 is near 450 nm; the RGB mix peaks near 635 nm.
        assert!(LedKind::B5.spectral_power(450.0) > 0.95);
        assert!(LedKind::Rgb1.spectral_power(635.0) > 0.99);
    }

    #[test]
    fn closest_to_cct_picks_the_nearest_published_value() {
        assert_eq!(LedKind::closest_to_cct(6500.0), LedKind::B5);
        assert_eq!(LedKind::closest_to_cct(5000.0), LedKind::B4);
        assert_eq!(LedKind::closest_to_cct(4090.0), LedKind::B3);
        assert_eq!(LedKind::closest_to_cct(1000.0), LedKind::V1);
    }
}
