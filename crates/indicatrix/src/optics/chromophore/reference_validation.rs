//! Validation of the user-delivered research spectra (`docs/chromophores/research-2026-10-polarized-spectra*.md`)
//! against their own CIELAB, with OUR body-color code. These tables are secondary and, per
//! `research-2026-10-primary-data.md`, their stated CIELAB is no target: the recipes are fitted to
//! the primary data (`primary_data_tests`).
//!
//! Every tabulated spectrum is a fixture in `data/reference_spectra/*.csv` (metadata header,
//! `lambda_nm` plus one Napierian alpha column per polarisation, and `# lab:` lines with the
//! research's measured CIELAB). A spectrum is turned into a color exactly as the plan's forward
//! model does: `T = exp(-alpha * L)`, D65 or A (Planckian 2856 K), 380-780 nm at 1 nm, the CIE
//! 1931 2 degree observer, Lab relative to the illuminant's own white.
//!
//! Handling of the sparse lines: a few rows are single points of a narrow line (ruby R1/R2 at
//! 694.3/692.8 nm, chrysoberyl R lines at 678.5/680.4, emerald R1 at 683.5, the report-2 ruby at
//! 694). Interpolating them linearly with their neighbours would turn a 1 nm line into a
//! triangle 20-80 nm wide. The line-aware reading removes those rows from the continuum,
//! interpolates the continuum linearly, and puts back a Gaussian (in energy) of the stated
//! linewidth with the tabulated excess as peak. The raw (naive) reading is reported next to it.
//! Unpolarised colors: a uniaxial `mean_uniax` is the plan's rule `(2 T_o + T_e) / 3`; a
//! `mean(a,b)` spec is the plain mean of the named columns (light along b of a biaxial crystal
//! vibrates along a and c, so `(T_alpha + T_gamma) / 2`; the plan's three-mode rule is evaluated
//! next to it for alexandrite).

#![expect(
    clippy::suboptimal_flops,
    clippy::while_float,
    clippy::doc_markdown,
    clippy::items_after_statements,
    clippy::manual_midpoint,
    reason = "test code written as the formulas and research notation read"
)]

use crate::color::body_color::{Illuminant, body_color, delta_e_2000};

/// All reference fixtures `(file stem, text)`.
pub(super) const FIXTURES: &[(&str, &str)] = &[
    (
        "alexandrite_natural_russian",
        include_str!("../../../data/reference_spectra/alexandrite_natural_russian.csv"),
    ),
    (
        "amethyst_80ppmw_fe",
        include_str!("../../../data/reference_spectra/amethyst_80ppmw_fe.csv"),
    ),
    (
        "citrine_annealed_amethyst",
        include_str!("../../../data/reference_spectra/citrine_annealed_amethyst.csv"),
    ),
    (
        "corundum_v_0p26wt_v2o3",
        include_str!("../../../data/reference_spectra/corundum_v_0p26wt_v2o3.csv"),
    ),
    (
        "emerald_colombian_muzo",
        include_str!("../../../data/reference_spectra/emerald_colombian_muzo.csv"),
    ),
    (
        "peridot_fo90",
        include_str!("../../../data/reference_spectra/peridot_fo90.csv"),
    ),
    (
        "ruby2_synthesised_0p25wt",
        include_str!("../../../data/reference_spectra/ruby2_synthesised_0p25wt.csv"),
    ),
    (
        "ruby_r1_synthetic_verneuil_0p05wt",
        include_str!("../../../data/reference_spectra/ruby_r1_synthetic_verneuil_0p05wt.csv"),
    ),
    (
        "ruby_r2_synthetic_flux_0p25wt",
        include_str!("../../../data/reference_spectra/ruby_r2_synthetic_flux_0p25wt.csv"),
    ),
    (
        "ruby_r3_mogok_0p72wt",
        include_str!("../../../data/reference_spectra/ruby_r3_mogok_0p72wt.csv"),
    ),
    (
        "sapphire2_moderate_4mm",
        include_str!("../../../data/reference_spectra/sapphire2_moderate_4mm.csv"),
    ),
    (
        "sapphire_s_bas_basaltic",
        include_str!("../../../data/reference_spectra/sapphire_s_bas_basaltic.csv"),
    ),
    (
        "sapphire_s_met_metamorphic",
        include_str!("../../../data/reference_spectra/sapphire_s_met_metamorphic.csv"),
    ),
    (
        "tanzanite_heated_0p38wt_v2o3",
        include_str!("../../../data/reference_spectra/tanzanite_heated_0p38wt_v2o3.csv"),
    ),
    (
        "tanzanite_natural_0p38v_0p12ti",
        include_str!("../../../data/reference_spectra/tanzanite_natural_0p38v_0p12ti.csv"),
    ),
];

/// Single-point lines `(file stem prefix, nm, FWHM in cm^-1)` of the sparse rows.
const LINE_POINTS: &[(&str, f64, f64)] = &[
    ("ruby_r", 692.8, 16.0),
    ("ruby_r", 694.3, 18.0),
    ("ruby2_", 694.0, 18.0),
    ("alexandrite_", 678.5, 12.0),
    ("alexandrite_", 680.4, 12.0),
    ("emerald_", 683.5, 15.0),
];

/// One measured color of the research: the polarisation spec, illuminant, path and Lab.
#[derive(Debug, Clone)]
pub(super) struct LabRef {
    pub spec: String,
    pub illuminant: Illuminant,
    pub illuminant_name: String,
    pub path_cm: f64,
    pub lab: [f64; 3],
}

/// A parsed fixture.
#[derive(Debug, Clone)]
pub(super) struct Spectrum {
    pub name: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<f64>>,
    pub labs: Vec<LabRef>,
    pub header: Vec<String>,
}

/// Parses a fixture CSV.
pub(super) fn parse(name: &str, text: &str) -> Spectrum {
    let (mut header, mut labs, mut columns, mut rows) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Some(rest) = line.strip_prefix("# lab:") {
            let p: Vec<&str> = rest.split('|').map(str::trim).collect();
            let num = |i: usize| p[i].parse::<f64>().expect("lab number");
            let (illuminant, illuminant_name) = match p[1] {
                "D65" => (Illuminant::D65, "D65"),
                "A" => (Illuminant::Planckian(2856.0), "A"),
                other => panic!("unknown illuminant {other}"),
            };
            labs.push(LabRef {
                spec: p[0].to_string(),
                illuminant,
                illuminant_name: illuminant_name.to_string(),
                path_cm: num(2),
                lab: [num(3), num(4), num(5)],
            });
        } else if line.starts_with('#') {
            header.push(line.to_string());
        } else if columns.is_empty() {
            columns = line.split(',').skip(1).map(str::to_string).collect();
        } else {
            rows.push(
                line.split(',')
                    .map(|v| v.parse().expect("number"))
                    .collect(),
            );
        }
    }
    Spectrum {
        name: name.to_string(),
        columns,
        rows,
        labs,
        header,
    }
}

/// All fixtures parsed.
pub(super) fn all() -> Vec<Spectrum> {
    FIXTURES.iter().map(|(n, t)| parse(n, t)).collect()
}

/// Metadata value of a `# key: value` header line.
pub(super) fn meta<'a>(s: &'a Spectrum, key: &str) -> Option<&'a str> {
    s.header
        .iter()
        .find_map(|l| l.strip_prefix(&format!("# {key}:")))
        .map(str::trim)
}

fn energy_gauss(lambda: f64, centre_nm: f64, fwhm_cm1: f64, peak: f64) -> f64 {
    let d = 1e7 / lambda - 1e7 / centre_nm;
    peak * (-4.0 * std::f64::consts::LN_2 * (d / fwhm_cm1).powi(2)).exp()
}

impl Spectrum {
    fn column(&self, name: &str) -> usize {
        self.columns
            .iter()
            .position(|c| c == name)
            .unwrap_or_else(|| panic!("{}: no column {name}", self.name))
    }

    fn line_rows(&self) -> Vec<(f64, f64)> {
        LINE_POINTS
            .iter()
            .filter(|(prefix, _, _)| self.name.starts_with(prefix))
            .map(|&(_, nm, fw)| (nm, fw))
            .collect()
    }

    /// Napierian alpha (cm^-1) of column `col` at `lambda`, linear interpolation of the
    /// tabulated points; `line_aware` removes single-point lines from the continuum and adds
    /// them back as Gaussians.
    pub(super) fn alpha(&self, col: &str, lambda: f64, line_aware: bool) -> f64 {
        let c = self.column(col) + 1;
        let lines = if line_aware {
            self.line_rows()
        } else {
            Vec::new()
        };
        let is_line = |nm: f64| lines.iter().any(|(l, _)| (l - nm).abs() < 0.05);
        let pts: Vec<(f64, f64)> = self
            .rows
            .iter()
            .filter(|r| !is_line(r[0]))
            .map(|r| (r[0], r[c]))
            .collect();
        let continuum = |x: f64| -> f64 {
            if x <= pts[0].0 {
                return pts[0].1;
            }
            for w in pts.windows(2) {
                if x <= w[1].0 {
                    let t = (x - w[0].0) / (w[1].0 - w[0].0);
                    return w[0].1 + t * (w[1].1 - w[0].1);
                }
            }
            pts[pts.len() - 1].1
        };
        let mut a = continuum(lambda);
        for (nm, fwhm) in &lines {
            let row = self.rows.iter().find(|r| (r[0] - nm).abs() < 0.05);
            if let Some(r) = row {
                let excess = (r[c] - continuum(*nm)).max(0.0);
                a += energy_gauss(lambda, *nm, *fwhm, excess);
            }
        }
        a
    }

    /// Our body color for a polarisation spec at path `path_cm` (see the module docs).
    pub(super) fn lab(
        &self,
        spec: &str,
        ill: Illuminant,
        path_cm: f64,
        line_aware: bool,
    ) -> [f64; 3] {
        let cols: Vec<String> = if spec == "mean_uniax" {
            vec!["E_perp_c".into(), "E_par_c".into()]
        } else if let Some(inner) = spec.strip_prefix("mean(") {
            inner
                .trim_end_matches(')')
                .split(',')
                .map(str::to_string)
                .collect()
        } else if spec == "mean3" {
            vec![
                "alpha_par_c".into(),
                "beta_par_b".into(),
                "gamma_par_a".into(),
            ]
        } else {
            vec![spec.to_string()]
        };
        let weights: Vec<f64> = if spec == "mean_uniax" {
            vec![2.0 / 3.0, 1.0 / 3.0]
        } else {
            vec![1.0 / cols.len() as f64; cols.len()]
        };
        let path_mm = path_cm * 10.0;
        // body_color takes alpha per mm; the (possibly polarisation-averaged) transmittance is
        // expressed as the equivalent alpha so the very same colorimetry code runs.
        let eq_alpha = |lambda: f64| -> f64 {
            let t: f64 = cols
                .iter()
                .zip(&weights)
                .map(|(c, w)| w * (-self.alpha(c, lambda, line_aware) * path_cm).exp())
                .sum();
            -t.max(1e-300).ln() / path_mm
        };
        body_color(eq_alpha, path_mm, ill).lab
    }
}

/// `(sample, spec, illuminant, path_cm, dE00 with lines handled, dE00 raw)` for every measured
/// color of every fixture.
pub(super) fn validation_table() -> Vec<(String, LabRef, f64, f64)> {
    let mut out = Vec::new();
    for s in all() {
        for l in &s.labs {
            let aware = delta_e_2000(l.lab, s.lab(&l.spec, l.illuminant, l.path_cm, true));
            let raw = delta_e_2000(l.lab, s.lab(&l.spec, l.illuminant, l.path_cm, false));
            out.push((s.name.clone(), l.clone(), aware, raw));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{color::body_color::xyz_to_lab, optics::chromophore::ChromophoreCatalogue};

    /// Pinned `(fixture, spec, illuminant, path_cm, dE00 of the research spectrum vs its own
    /// CIELAB, line-aware)`. Tolerance 0.3 (the interpolation and line handling are
    /// deterministic; the pins document the research validation).
    const PINNED: &[(&str, &str, &str, f64, f64)] = &[
        (
            "ruby_r1_synthetic_verneuil_0p05wt",
            "mean_uniax",
            "D65",
            0.50,
            15.88,
        ),
        (
            "ruby_r2_synthetic_flux_0p25wt",
            "mean_uniax",
            "D65",
            0.50,
            24.30,
        ),
        ("ruby_r3_mogok_0p72wt", "mean_uniax", "D65", 0.25, 25.78),
        ("ruby_r3_mogok_0p72wt", "mean_uniax", "D65", 0.50, 20.25),
        ("ruby2_synthesised_0p25wt", "mean_uniax", "D65", 0.50, 26.06),
        ("sapphire_s_met_metamorphic", "E_perp_c", "D65", 0.20, 4.09),
        ("sapphire_s_met_metamorphic", "E_par_c", "D65", 0.20, 7.71),
        ("sapphire_s_bas_basaltic", "E_perp_c", "D65", 0.20, 6.73),
        ("sapphire_s_bas_basaltic", "E_perp_c", "A", 0.20, 8.43),
        ("sapphire_s_bas_basaltic", "E_perp_c", "D65", 0.50, 5.05),
        ("sapphire2_moderate_4mm", "E_perp_c", "D65", 0.40, 9.49),
        (
            "alexandrite_natural_russian",
            "alpha_par_c",
            "D65",
            0.20,
            20.08,
        ),
        (
            "alexandrite_natural_russian",
            "alpha_par_c",
            "A",
            0.20,
            31.83,
        ),
        (
            "alexandrite_natural_russian",
            "beta_par_b",
            "D65",
            0.20,
            30.20,
        ),
        (
            "alexandrite_natural_russian",
            "beta_par_b",
            "A",
            0.20,
            38.08,
        ),
        (
            "alexandrite_natural_russian",
            "gamma_par_a",
            "D65",
            0.20,
            28.45,
        ),
        (
            "alexandrite_natural_russian",
            "gamma_par_a",
            "A",
            0.20,
            23.61,
        ),
        (
            "alexandrite_natural_russian",
            "mean(alpha_par_c,gamma_par_a)",
            "D65",
            0.20,
            11.30,
        ),
        (
            "alexandrite_natural_russian",
            "mean(alpha_par_c,gamma_par_a)",
            "A",
            0.20,
            23.26,
        ),
        (
            "tanzanite_heated_0p38wt_v2o3",
            "beta_par_b",
            "D65",
            0.40,
            21.18,
        ),
        (
            "tanzanite_natural_0p38v_0p12ti",
            "gamma_par_c",
            "D65",
            0.40,
            17.03,
        ),
        ("peridot_fo90", "alpha_par_b", "D65", 0.50, 12.46),
        ("amethyst_80ppmw_fe", "E_perp_c", "D65", 0.60, 3.44),
        (
            "citrine_annealed_amethyst",
            "unpolarised",
            "D65",
            0.60,
            12.70,
        ),
        ("emerald_colombian_muzo", "E_perp_c", "D65", 0.30, 9.86),
        ("emerald_colombian_muzo", "E_par_c", "D65", 0.30, 12.27),
    ];

    fn fixture(name: &str) -> Spectrum {
        let (n, t) = FIXTURES
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("fixture {name}"));
        parse(n, t)
    }

    /// The research validation: every tabulated spectrum through OUR body-color code against the
    /// research's own measured CIELAB. Only the two samples with dE00 <= 4.1 are consistent
    /// (S-Met E-perp-c, amethyst); everything else disagrees with its own CIELAB, which is a
    /// finding about the research, not about our colorimetry (validated against Sharma).
    #[test]
    fn research_spectra_vs_their_own_cielab_pinned() {
        let table = validation_table();
        let mut seen = 0;
        for (name, spec, ill, path, expected) in PINNED {
            let row = table
                .iter()
                .find(|(n, l, _, _)| {
                    n == name
                        && &l.spec == spec
                        && l.illuminant_name == *ill
                        && (l.path_cm - path).abs() < 1e-9
                })
                .unwrap_or_else(|| panic!("{name} {spec} {ill} {path}"));
            assert!(
                (row.2 - expected).abs() < 0.3,
                "{name} {spec} {ill} L={path}: dE00 {:.2}, pinned {expected}",
                row.2
            );
            seen += 1;
        }
        assert_eq!(seen, PINNED.len());
        // Every measured color of every fixture is pinned (the S-Met unpolarised c-cut row
        // duplicates E_perp_c and is covered by it).
        assert!(table.len() >= PINNED.len());
        let consistent: Vec<&str> = table
            .iter()
            .filter(|(_, _, aware, _)| *aware <= 4.5)
            .map(|(n, _, _, _)| n.as_str())
            .collect();
        assert!(
            consistent
                .iter()
                .all(|n| ["sapphire_s_met_metamorphic", "amethyst_80ppmw_fe"].contains(n)),
            "only S-Met and amethyst reproduce their CIELAB: {consistent:?}"
        );
        // Both the ruby spectra and their CIELAB cannot be right: every ruby is > 15.
        assert!(
            table
                .iter()
                .filter(|(n, ..)| n.starts_with("ruby"))
                .all(|(_, _, d, _)| *d > 15.0)
        );
    }

    /// The sparse lines hardly move the color (line-aware vs naive triangle): at most 2.1
    /// dE00 for the alexandrite A rows, below 0.3 elsewhere.
    #[test]
    fn sparse_line_handling_changes_color_little() {
        for (name, l, aware, raw) in validation_table() {
            let tol = if name.starts_with("alexandrite") {
                2.1
            } else {
                0.3
            };
            assert!(
                (aware - raw).abs() <= tol,
                "{name} {} {}: aware {aware:.2} vs raw {raw:.2}",
                l.spec,
                l.illuminant_name
            );
        }
    }

    /// Ruby U band: Dodd's decadic k555 = 22.1 cm^-1/wt% is 50.89 Napierian, Dubinsky's sigma
    /// gives 51.2 through our number density; R-1 and R-2 tables carry 51.2 per wt%, R-3 carries
    /// 55.4 (+8 %, Fe-containing sample: not the Cr U band alone).
    #[test]
    fn ruby_u_band_agrees_with_the_primary_values() {
        let cat = ChromophoreCatalogue::global();
        let n_per_wt = cat
            .host("corundum")
            .expect("corundum")
            .n_site_for_unit("wt_pct_oxide:Cr2O3");
        let dubinsky = 1.62e-19 * n_per_wt;
        let dodd = 22.1 * std::f64::consts::LN_10;
        assert!((dubinsky / dodd - 1.0).abs() < 0.02, "{dubinsky} vs {dodd}");
        let per_wt = |name: &str, wt: f64| fixture(name).alpha("E_perp_c", 555.0, true) / wt;
        let r1 = per_wt("ruby_r1_synthetic_verneuil_0p05wt", 0.05);
        let r2 = per_wt("ruby_r2_synthetic_flux_0p25wt", 0.25);
        let r3 = per_wt("ruby_r3_mogok_0p72wt", 0.72);
        assert!((r1 / dubinsky - 1.0).abs() < 0.01 && (r2 / dubinsky - 1.0).abs() < 0.01);
        assert!(
            (r3 / r2 - 1.08).abs() < 0.01,
            "R-3 is {:.3} x R-2 per wt%",
            r3 / r2
        );
    }

    /// R-1: the sample list says 1.00 cm, the color table 0.50 cm. The spectrum agrees better
    /// with neither (dE00 15.9 at 0.5 cm), but 1.00 cm is worse.
    #[test]
    fn r1_path_length_is_ambiguous_in_the_research() {
        let s = fixture("ruby_r1_synthetic_verneuil_0p05wt");
        let l = &s.labs[0];
        let at = |path: f64| delta_e_2000(l.lab, s.lab(&l.spec, l.illuminant, path, true));
        assert!(at(0.5) < at(1.0), "{} vs {}", at(0.5), at(1.0));
    }

    /// Unit findings. V2O3: 0.26 wt% is 700 ppma of all atoms (1750 ppm of Al sites, 3500 per
    /// formula unit), NOT 305; the V-corundum spectrum with the research sigma = 2.05e-19 cm^2
    /// implies 305 ppma_all (0.113 wt% V2O3), so the spectrum supports 305 ppma and not the label.
    /// Mg: 12 ppmw is 50 ppm per Al2O3 formula unit (the research's "ppma"), 10 ppma of all atoms:
    /// the research ppma for Fe/Ti/Mg are per formula unit, 5x its own all-atom convention.
    #[test]
    fn unit_findings_v2o3_and_mg() {
        let cat = ChromophoreCatalogue::global();
        let cor = cat.host("corundum").expect("corundum");
        let n_label = 0.26 * cor.n_site_for_unit("wt_pct_oxide:V2O3");
        let all_atoms = n_label / cor.n_site_for_unit("ppma_all");
        let al_sites = n_label / cor.n_site_for_unit("ppm_site");
        assert!((all_atoms - 706.0).abs() < 10.0, "{all_atoms}");
        assert!((al_sites - 1770.0).abs() < 30.0, "{al_sites}");
        let peak = fixture("corundum_v_0p26wt_v2o3").alpha("E_perp_c", 575.0, true);
        let n_spec = peak / 2.05e-19 / cor.n_site_for_unit("ppma_all");
        assert!(
            (n_spec - 305.0).abs() < 10.0,
            "spectrum implies {n_spec} ppma_all"
        );
        assert!(
            n_label / n_spec > 2.2,
            "label is {:.2}x the spectrum",
            n_label / n_spec
        );

        let per_fu = |ppmw: f64, m: f64| ppmw * 101.96 / m;
        assert!((per_fu(12.0, 24.305) - 50.0).abs() < 0.5);
        assert!((per_fu(22.0, 24.305) - 92.0).abs() < 0.5);
        assert!((per_fu(160.0, 47.867) - 340.0).abs() < 1.0);
        assert!((per_fu(450.0, 55.845) - 820.0).abs() < 2.0);
        let all = |ppmw: f64, m: f64| ppmw * (101.96 / 5.0) / m;
        // Ti_active equal in both samples within 2 % on either basis.
        let met = all(160.0, 47.867) - all(12.0, 24.305);
        let bas = all(180.0, 47.867) - all(22.0, 24.305);
        assert!((met / bas - 1.0).abs() < 0.02, "{met} vs {bas}");
    }

    /// Pairing-law evidence of the two sapphires (equal Ti_active, Fe differs 6.2x): the net
    /// Fe2+-Ti4+ peak (578 nm minus the 500 nm continuum) differs by < 1.5x, so the law is linear
    /// in Ti_active (clustered_min), not Fe x Ti (which would give ~6x); the 880 nm Fe2+-Fe3+ band
    /// scales with ~Fe^2 (ratio 39 vs 38).
    #[test]
    fn sapphire_pairing_law_evidence() {
        let met = fixture("sapphire_s_met_metamorphic");
        let bas = fixture("sapphire_s_bas_basaltic");
        let net =
            |s: &Spectrum| s.alpha("E_perp_c", 578.0, true) - s.alpha("E_perp_c", 500.0, true);
        let ratio = net(&bas) / net(&met);
        assert!(ratio > 0.8 && ratio < 1.5, "net pair peak ratio {ratio}");
        let fe_ratio = 2800.0 / 450.0;
        assert!(
            ratio < 0.3 * fe_ratio,
            "a Fe x Ti product law would give ~{fe_ratio}"
        );
        let r880 = bas.alpha("E_perp_c", 880.0, true) / met.alpha("E_perp_c", 880.0, true);
        assert!(
            (r880 / (fe_ratio * fe_ratio) - 1.0).abs() < 0.1,
            "880 nm ratio {r880} vs Fe^2 {}",
            fe_ratio * fe_ratio
        );
    }

    /// The tabulated bands are narrower than the stated analytic FWHM: ruby U (stated 3180
    /// cm^-1) and the sapphire Fe2+-Ti4+ band (stated 5100) measured at half maximum of the
    /// tabulated spectra.
    #[test]
    fn tabulated_widths_are_narrower_than_the_stated_fwhm() {
        let width_at_half = |s: &Spectrum, col: &str, peak_nm: f64, floor: f64| {
            let peak = s.alpha(col, peak_nm, true);
            let half = floor + 0.5 * (peak - floor);
            let mut blue = peak_nm;
            while s.alpha(col, blue - 0.5, true) > half {
                blue -= 0.5;
            }
            let mut red = peak_nm;
            while s.alpha(col, red + 0.5, true) > half {
                red += 0.5;
            }
            1e7 / blue - 1e7 / red
        };
        let r2 = fixture("ruby_r2_synthetic_flux_0p25wt");
        let u = width_at_half(&r2, "E_perp_c", 555.0, 0.0);
        assert!(u < 0.8 * 3180.0, "ruby U table FWHM {u:.0} vs stated 3180");
        let sm = fixture("sapphire_s_met_metamorphic");
        let ti = width_at_half(&sm, "E_perp_c", 578.0, 0.8);
        assert!(ti < 0.7 * 5100.0, "Fe-Ti table FWHM {ti:.0} vs stated 5100");
    }

    fn lab_to_xyz(lab: [f64; 3], white: [f64; 3]) -> [f64; 3] {
        let fy = (lab[0] + 16.0) / 116.0;
        let (fx, fz) = (fy + lab[1] / 500.0, fy - lab[2] / 200.0);
        let inv = |f: f64| {
            let c = f * f * f;
            if c > 216.0 / 24389.0 {
                c
            } else {
                (116.0 * f - 16.0) / (24389.0 / 27.0)
            }
        };
        [inv(fx) * white[0], inv(fy) * white[1], inv(fz) * white[2]]
    }

    /// Internal consistency of the alexandrite colors: XYZ is linear in T, so the stated
    /// "unpolarised, along b" color must equal the XYZ mean of the stated ray colors that
    /// vibrate in the a-c plane, (alpha + gamma) / 2, NOT the plan's three-mode mean. D65 agrees
    /// to dE00 ~3.4 (the A row to ~6.4: its own inconsistency); the three-mode mean is > 10 off.
    #[test]
    fn alexandrite_unpolarised_is_the_alpha_gamma_mean() {
        let white = |ill: &str| match ill {
            "D65" => [95.04, 100.0, 108.88],
            _ => [109.85, 100.0, 35.58],
        };
        let s = fixture("alexandrite_natural_russian");
        let get = |spec: &str, ill: &str| {
            s.labs
                .iter()
                .find(|l| l.spec == spec && l.illuminant_name == ill)
                .unwrap_or_else(|| panic!("{spec} {ill}"))
                .lab
        };
        for (ill, lo, hi) in [("D65", 2.5, 4.5), ("A", 5.5, 7.5)] {
            let w = white(ill);
            let mean = |specs: &[&str]| {
                let mut x = [0.0; 3];
                for sp in specs {
                    let v = lab_to_xyz(get(sp, ill), w);
                    for k in 0..3 {
                        x[k] += v[k] / specs.len() as f64;
                    }
                }
                xyz_to_lab(x.map(|v| v / 100.0), w.map(|v| v / 100.0))
            };
            let stated = get("mean(alpha_par_c,gamma_par_a)", ill);
            let d_ag = delta_e_2000(stated, mean(&["alpha_par_c", "gamma_par_a"]));
            assert!(
                d_ag > lo && d_ag < hi,
                "{ill}: (alpha+gamma)/2 dE00 {d_ag:.2}"
            );
            let d3 = delta_e_2000(stated, mean(&["alpha_par_c", "beta_par_b", "gamma_par_a"]));
            assert!(d3 > 10.0, "{ill}: three-mode mean dE00 {d3:.2}");
        }
    }

    /// The report-1 V-corundum TABLE gives a color change below 6 dE00 (D65 vs 3200 K, up to 10
    /// mm at the 305 ppma the table supports). That is a finding about the table, not about
    /// vanadium sapphire: the table follows the paper TEXT sigma (1.0e-19 cm^2 at 580 nm) and the
    /// GIA xlsx cross-section gives 8-25 dE00 (`primary_data_tests`,
    /// `v_corundum_color_change_matches_the_gia_derived_range`; the catalogue uses the xlsx).
    #[test]
    fn v_corundum_color_change_is_small_in_the_tabulated_spectrum() {
        let s = fixture("corundum_v_0p26wt_v2o3");
        for path in [0.1, 0.2, 0.5, 1.0] {
            let d65 = s.lab("E_perp_c", Illuminant::D65, path, true);
            let a = s.lab("E_perp_c", Illuminant::Planckian(3200.0), path, true);
            assert!(
                delta_e_2000(d65, a) < 6.0,
                "{path} cm: {:.2}",
                delta_e_2000(d65, a)
            );
        }
    }

    /// Evidence for the report-2 contradictions that the tables themselves decide.
    /// Fe3+ 377/388/450 nm: the S-Bas/S-Met net bump ratios are 5.6-7.0 for a Fe ratio of 6.2, i.e.
    /// linear in Fe; report 2's "377 nm proportional to Fe^2" would give 38.
    #[test]
    fn fe3_bumps_are_linear_in_iron() {
        let met = fixture("sapphire_s_met_metamorphic");
        let bas = fixture("sapphire_s_bas_basaltic");
        fn a(s: &Spectrum, nm: f64) -> f64 {
            s.alpha("E_perp_c", nm, true)
        }
        fn bump450(s: &Spectrum) -> f64 {
            a(s, 450.0) - 0.5 * (a(s, 440.0) + a(s, 460.0))
        }
        fn bump377(s: &Spectrum) -> f64 {
            a(s, 377.0) - (a(s, 370.0) + 0.7 * (a(s, 380.0) - a(s, 370.0)))
        }
        fn bump388(s: &Spectrum) -> f64 {
            a(s, 388.0) - (a(s, 380.0) + 0.4 * (a(s, 400.0) - a(s, 380.0)))
        }
        for (name, f) in [
            ("450", bump450 as fn(&Spectrum) -> f64),
            ("377", bump377),
            ("388", bump388),
        ] {
            let r = f(&bas) / f(&met);
            assert!(
                (4.5..9.0).contains(&r),
                "{name} nm bump ratio {r:.2} (Fe ratio 6.2, Fe^2 ratio 38)"
            );
        }
        // The 450 nm bump is narrower than 20 nm: the neighbours 440/460 sit on the continuum, so
        // a 1500 cm^-1 FWHM (report 2) is excluded and ~500 cm^-1 (report 1) is supported.
        let drop = (a(&met, 440.0) + a(&met, 460.0)) / 2.0 / a(&met, 450.0);
        assert!(drop < 0.45, "440/460 nm are {drop:.2} of the 450 nm value");
    }

    /// Ruby: both tabulated sets are narrower than either stated U FWHM (3180 / 3350 cm^-1) and
    /// the E-par-c/E-perp-c U ratio is 0.71 (report 1) vs 0.53 (report 2); the two reports'
    /// 0.25 wt% CIELAB differ by dE00 4.3.
    #[test]
    fn ruby_report_1_and_2_tables_compared() {
        let half_width = |s: &Spectrum, col: &str| {
            let (mut peak, mut best_nm) = (0.0, 0.0);
            let mut nm = 500.0;
            while nm <= 640.0 {
                let v = s.alpha(col, nm, true);
                if v > peak {
                    (peak, best_nm) = (v, nm);
                }
                nm += 0.25;
            }
            let (mut lo, mut hi) = (best_nm, best_nm);
            while s.alpha(col, lo - 0.25, true) >= peak / 2.0 {
                lo -= 0.25;
            }
            while s.alpha(col, hi + 0.25, true) >= peak / 2.0 {
                hi += 0.25;
            }
            (1e7 / lo - 1e7 / hi, peak)
        };
        let r1 = fixture("ruby_r2_synthetic_flux_0p25wt");
        let r2 = fixture("ruby2_synthesised_0p25wt");
        let (w1, p1) = half_width(&r1, "E_perp_c");
        let (w2, p2) = half_width(&r2, "E_perp_c");
        assert!(w1 < 2500.0 && w2 < 2800.0 && w1 < w2, "{w1:.0} / {w2:.0}");
        assert!(
            (p1 - p2).abs() < 0.1,
            "both tables have the same U peak 12.8"
        );
        let ratio = |s: &Spectrum| half_width(s, "E_par_c").1 / half_width(s, "E_perp_c").1;
        assert!((ratio(&r1) - 0.71).abs() < 0.01 && (ratio(&r2) - 0.53).abs() < 0.02);
        let d = delta_e_2000([37.8, 69.5, 24.8], [42.5, 65.2, 22.1]);
        assert!((d - 4.34).abs() < 0.05, "{d}");
    }

    /// Fixture hygiene: every fixture has metadata (sample, composition, units, path, source)
    /// and at least one measured CIELAB except the V-corundum spectrum.
    #[test]
    fn fixtures_carry_metadata() {
        for s in all() {
            for key in ["sample", "composition", "units", "path_cm", "source"] {
                assert!(meta(&s, key).is_some(), "{}: missing # {key}:", s.name);
            }
            assert!(s.rows.len() >= 19, "{}: {} rows", s.name, s.rows.len());
            assert!(
                s.rows.windows(2).all(|w| w[0][0] < w[1][0]),
                "{}: wavelengths not increasing",
                s.name
            );
            assert!(
                s.rows.first().expect("rows")[0] <= 380.0
                    && s.rows.last().expect("rows")[0] >= 780.0
            );
            if s.name != "corundum_v_0p26wt_v2o3" {
                assert!(!s.labs.is_empty(), "{}: no measured CIELAB", s.name);
            }
        }
        assert_eq!(FIXTURES.len(), 15);
    }
}
