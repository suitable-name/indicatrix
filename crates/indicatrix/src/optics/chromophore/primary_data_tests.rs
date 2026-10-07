//! Data version 4: the physics recipes against the PRIMARY data of
//! `docs/chromophores/research-2026-10-primary-data.md` (`data/reference_spectra/gia_*` and
//! `caltech_*`), replacing the comparisons with the research's discredited stated CIELAB.
//!
//! Method (as `reference_validation`): `T = exp(-alpha L)`, D65 or A (Planckian 2856 K),
//! 380-780 nm at 1 nm, CIE 1931 2 degrees, Lab relative to the illuminant's own white, through
//! `color::body_color`. The GIA cross-sections sigma (cm^2 per ion, Napierian) become
//! `alpha = sigma N` with `N = areal density x 1.178e17 cm^-3 per ppma of all atoms`.
//!
//! - GIA: the pipeline reproduces GIA's published Appendix 2 colors (dE00 1-2.5), the Cr3+, V3+
//!   and Fe2+-Ti4+ recipes reproduce the GIA cross-sections (shape and magnitude).
//! - Caltech: shape references only (no composition): the fitted ray spectra are compared with
//!   a free scale and flat offset.

#![expect(
    clippy::doc_markdown,
    clippy::suboptimal_flops,
    reason = "test code quoting the primary data's symbols; the formulas read as the data's notation"
)]

use super::*;
use crate::{
    color::{
        body_color::{Illuminant, body_color, body_colors, delta_e_2000},
        cie1931::cie_1931_cmf,
    },
    optics::absorption::AbsorptionTensor,
};

/// All primary fixtures `(file stem, text)`.
const PRIMARY: &[(&str, &str)] = &[
    (
        "caltech_chrysoberyl_grr874_5nm",
        include_str!("../../../data/reference_spectra/caltech_chrysoberyl_grr874_5nm.csv"),
    ),
    (
        "caltech_chrysoberyl_grr874_beta_ch874a_0p717mm",
        include_str!(
            "../../../data/reference_spectra/caltech_chrysoberyl_grr874_beta_ch874a_0p717mm.csv"
        ),
    ),
    (
        "caltech_emerald_grr3570_5nm",
        include_str!("../../../data/reference_spectra/caltech_emerald_grr3570_5nm.csv"),
    ),
    (
        "caltech_olivine_grr418_alpha",
        include_str!("../../../data/reference_spectra/caltech_olivine_grr418_alpha.csv"),
    ),
    (
        "caltech_olivine_grr418_beta",
        include_str!("../../../data/reference_spectra/caltech_olivine_grr418_beta.csv"),
    ),
    (
        "caltech_olivine_grr418_gamma",
        include_str!("../../../data/reference_spectra/caltech_olivine_grr418_gamma.csv"),
    ),
    (
        "caltech_quartz_amethyst_ameth_bo_5nm",
        include_str!("../../../data/reference_spectra/caltech_quartz_amethyst_ameth_bo_5nm.csv"),
    ),
    (
        "caltech_ruby_grr1843_eperpc_0p787mm",
        include_str!("../../../data/reference_spectra/caltech_ruby_grr1843_eperpc_0p787mm.csv"),
    ),
    (
        "caltech_sapphire_grr1020a_eparc_4p289mm",
        include_str!("../../../data/reference_spectra/caltech_sapphire_grr1020a_eparc_4p289mm.csv"),
    ),
    (
        "caltech_sapphire_grr1020a_eperpc_4p289mm",
        include_str!(
            "../../../data/reference_spectra/caltech_sapphire_grr1020a_eperpc_4p289mm.csv"
        ),
    ),
    (
        "caltech_zoisite_grr1265_5nm",
        include_str!("../../../data/reference_spectra/caltech_zoisite_grr1265_5nm.csv"),
    ),
    (
        "caltech_zoisite_grr2292_blue_digest_10nm",
        include_str!(
            "../../../data/reference_spectra/caltech_zoisite_grr2292_blue_digest_10nm.csv"
        ),
    ),
    (
        "gia_cr3plus_sample1110_sigma",
        include_str!("../../../data/reference_spectra/gia_cr3plus_sample1110_sigma.csv"),
    ),
    (
        "gia_v3plus_fe2plus_ti4plus_sigma_5nm",
        include_str!("../../../data/reference_spectra/gia_v3plus_fe2plus_ti4plus_sigma_5nm.csv"),
    ),
];

/// 1 ppma of all atoms in corundum in cm^-3 (GIA 2020 Box A: `5 N_A rho / M`, rho 3.99).
pub(super) const PPMA_CM3: f64 = 1.178e17;
/// Illuminant A.
pub(super) const A: Illuminant = Illuminant::Planckian(2856.0);

/// A parsed primary fixture: header lines, column names (after `lambda_nm`) and numeric rows.
pub(super) struct Table {
    name: String,
    header: Vec<String>,
    columns: Vec<String>,
    rows: Vec<Vec<f64>>,
}

/// One `# appendix2_lab:` line of a GIA fixture: GIA's CALCULATED color.
pub(super) struct Appendix2 {
    pub(super) spec: String,
    pub(super) illuminant: Illuminant,
    pub(super) ppma_cm: f64,
    pub(super) lab: [f64; 3],
}

pub(super) fn table(name: &str) -> Table {
    let (_, text) = PRIMARY
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("primary fixture {name}"));
    parse(name, text)
}

fn parse(name: &str, text: &str) -> Table {
    let (mut header, mut columns, mut rows) = (Vec::new(), Vec::new(), Vec::new());
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if line.starts_with('#') {
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
    Table {
        name: name.to_string(),
        header,
        columns,
        rows,
    }
}

impl Table {
    fn meta(&self, key: &str) -> Option<&str> {
        self.header
            .iter()
            .find_map(|l| l.strip_prefix(&format!("# {key}:")))
            .map(str::trim)
    }

    pub(super) fn col(&self, name: &str) -> usize {
        self.columns
            .iter()
            .position(|c| c == name)
            .unwrap_or_else(|| panic!("{}: no column {name}", self.name))
            + 1
    }

    /// Column `col` (index into a row) at `lambda`, linearly interpolated, held at the ends.
    pub(super) fn at(&self, col: usize, lambda: f64) -> f64 {
        let r = &self.rows;
        if lambda <= r[0][0] {
            return r[0][col];
        }
        for w in r.windows(2) {
            if lambda <= w[1][0] {
                let t = (lambda - w[0][0]) / (w[1][0] - w[0][0]);
                return w[0][col] + t * (w[1][col] - w[0][col]);
            }
        }
        r[r.len() - 1][col]
    }

    pub(super) fn appendix2(&self) -> Vec<Appendix2> {
        self.header
            .iter()
            .filter_map(|l| l.strip_prefix("# appendix2_lab:"))
            .map(|l| {
                let p: Vec<&str> = l.split('|').map(str::trim).collect();
                let n = |i: usize| p[i].parse::<f64>().expect("number");
                Appendix2 {
                    spec: p[0].to_string(),
                    illuminant: if p[1] == "D65" { Illuminant::D65 } else { A },
                    ppma_cm: n(2),
                    lab: [n(3), n(4), n(5)],
                }
            })
            .collect()
    }
}

fn cat() -> &'static ChromophoreCatalogue {
    ChromophoreCatalogue::global()
}

pub(super) fn hue_deg(lab: [f64; 3]) -> f64 {
    lab[2].atan2(lab[1]).to_degrees().rem_euclid(360.0)
}

/// Equivalent absorption coefficient (per mm) of the unpolarised mix `(2 T_o + T_e) / 3`.
fn mix_alpha(alpha_o: f64, alpha_e: f64, path_mm: f64) -> f64 {
    let t = (2.0 * (-alpha_o * path_mm).exp() + (-alpha_e * path_mm).exp()) / 3.0;
    -t.ln() / path_mm
}

/// color of a GIA sigma column pair (`o` / `e` column index or both) at `ppma_cm` ppma*cm of
/// absorbers over a 10 mm path. `which`: `"E_perp_c"`, `"E_par_c"` or `"both"`.
pub(super) fn sigma_lab(
    t: &Table,
    cols: (usize, usize),
    which: &str,
    ppma_cm: f64,
    ill: Illuminant,
) -> [f64; 3] {
    let n = ppma_cm * PPMA_CM3; // 1 cm path: ppma*cm / cm
    let alpha_mm = |l: f64, c: usize| t.at(c, l) * n / 10.0;
    match which {
        "E_perp_c" => body_color(|l| alpha_mm(l, cols.0), 10.0, ill).lab,
        "E_par_c" => body_color(|l| alpha_mm(l, cols.1), 10.0, ill).lab,
        _ => {
            body_color(
                |l| mix_alpha(alpha_mm(l, cols.0), alpha_mm(l, cols.1), 10.0),
                10.0,
                ill,
            )
            .lab
        }
    }
}

/// The corundum Cr recipe of `ppma_cm` ppma*cm of Cr over a 10 mm path (wt% Cr2O3 through the
/// catalogue's number density).
pub(super) fn cr_recipe(ppma_cm: f64) -> ColorRecipe {
    let n_wt = cat()
        .host("corundum")
        .expect("corundum")
        .n_site_for_unit("wt_pct_oxide:Cr2O3");
    let mut r = ColorRecipe::new("corundum", cat().data_version);
    assert!(r.set_amount("Cr", ppma_cm * PPMA_CM3 / n_wt));
    r
}

pub(super) fn tensor(r: &ColorRecipe) -> AbsorptionTensor {
    resolve(r, cat()).expect("resolves").0
}

/// Absorption coefficient (cm^-1) of the chromophore `id` of `host` for the pol key `pol` at
/// `lambda_nm` and concentration `conc` in the chromophore's unit, straight from the data.
pub(super) fn data_alpha(host: &str, id: &str, pol: &str, lambda_nm: f64, conc: f64) -> f64 {
    let c = cat()
        .host(host)
        .expect("host")
        .chromophores
        .iter()
        .find(|c| c.id == id)
        .expect("chromophore");
    c.usable_bands()
        .map(|b| {
            let w = b.pol.get(pol).copied().unwrap_or(0.0);
            let d = 1e7 / lambda_nm - 1e7 / b.centre_nm;
            let g = (-4.0 * std::f64::consts::LN_2 * (d / b.fwhm_cm1.expect("fwhm")).powi(2)).exp();
            w * (b.peak_coeff.expect("peak") * conc
                + b.quadratic_coeff.unwrap_or(0.0) * conc * conc)
                * g
        })
        .sum()
}

fn ray_alpha(t: &AbsorptionTensor, ray: char, lambda_nm: f64) -> f64 {
    let bands = match ray {
        'o' => &t.o_ray,
        'e' => &t.e_ray,
        _ => t.beta_ray.as_ref().expect("beta ray"),
    };
    f64::from(
        bands
            .iter()
            .map(|b| b.evaluate(lambda_nm as f32))
            .sum::<f32>(),
    )
}

/// color of one ray of `t` over `path_mm`.
pub(super) fn ray_lab(t: &AbsorptionTensor, ray: char, path_mm: f64, ill: Illuminant) -> [f64; 3] {
    body_color(|l| ray_alpha(t, ray, l), path_mm, ill).lab
}

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

/// Hygiene: every primary fixture has the metadata header of the existing fixtures, increasing
/// wavelengths, and the Caltech files say that they carry no composition (shape only).
#[test]
fn primary_fixtures_carry_metadata_and_caltech_files_are_marked_shape_only() {
    assert_eq!(PRIMARY.len(), 14);
    for (name, text) in PRIMARY {
        let t = parse(name, text);
        for key in [
            "sample",
            "host",
            "composition",
            "units",
            "path_cm",
            "source",
        ] {
            assert!(t.meta(key).is_some(), "{name}: missing # {key}:");
        }
        assert!(t.rows.len() >= 9, "{name}: {} rows", t.rows.len());
        assert!(
            t.rows.windows(2).all(|w| w[0][0] < w[1][0]),
            "{name}: wavelengths not increasing"
        );
        if name.starts_with("caltech_") {
            assert!(
                t.meta("composition")
                    .is_some_and(|c| c.contains("no composition, shape only")),
                "{name}: Caltech files carry no composition"
            );
            assert!(t.columns.iter().all(|c| c.starts_with("A_")), "{name}");
        }
    }
    // The GIA Cr3+ sheet is the sample-1110 cross-section with the 36 Appendix 2 colors.
    let gia = table("gia_cr3plus_sample1110_sigma");
    assert_eq!(gia.appendix2().len(), 36);
    assert_eq!(gia.rows.first().expect("rows")[0], 200.0);
    assert_eq!(gia.rows.last().expect("rows")[0], 800.0);
}

// ---------------------------------------------------------------------------------------------
// GIA pipeline and the Cr3+ recipe
// ---------------------------------------------------------------------------------------------

/// Pipeline validation of the primary-data report section 1a: our color code applied to the GIA
/// sample-1110 cross-sections reproduces GIA's Appendix 2 colors. Single polarisations to
/// dE00 <= 1.7 up to 375 ppma*cm; the "both" column (2 E_perp + E_par) / 3 to ~2.5 (GIA does not
/// state its exact mix); the residual grows for very dark colors (L* < 35, 1500+ ppma*cm).
#[test]
fn gia_sigma_through_our_color_code_reproduces_appendix_2() {
    let t = table("gia_cr3plus_sample1110_sigma");
    let cols = (t.col("sigma_E_perp_c_cm2"), t.col("sigma_E_par_c_cm2"));
    let mut worst: f64 = 0.0;
    for a in t.appendix2().iter().filter(|a| a.ppma_cm <= 375.0) {
        let lab = sigma_lab(&t, cols, &a.spec, a.ppma_cm, a.illuminant);
        let de = delta_e_2000(lab, a.lab);
        eprintln!(
            "GIA {} {:?} {} ppma*cm: ours {lab:.1?} GIA {:.1?} dE00 {de:.2}",
            a.spec, a.illuminant, a.ppma_cm, a.lab
        );
        let limit = if a.spec == "both" { 3.0 } else { 2.0 };
        assert!(
            de <= limit,
            "{} {} ppma*cm: dE00 {de:.2}",
            a.spec,
            a.ppma_cm
        );
        worst = worst.max(de);
    }
    assert!(worst > 0.5, "the check compares real colors: {worst}");
}

/// Task D: the corundum Cr3+ recipe at 100 and 375 ppma*cm reproduces GIA's Appendix 2
/// calculated colors (E_perp_c, E_par_c; D65 and A) within dE00 3 (measured: <= 1.8); the
/// unpolarised mix (GIA's "both") within 4 (the data themselves are 2.5 off, see above).
#[test]
fn cr_recipe_reproduces_gia_appendix_2_colors() {
    let t = table("gia_cr3plus_sample1110_sigma");
    for a in t
        .appendix2()
        .iter()
        .filter(|a| a.ppma_cm == 100.0 || a.ppma_cm == 375.0)
    {
        let r = cr_recipe(a.ppma_cm);
        let c = body_colors(&tensor(&r), 10.0, a.illuminant);
        let lab = match a.spec.as_str() {
            "E_perp_c" => c.o_ray.lab,
            "E_par_c" => c.e_ray.lab,
            _ => c.unpolarised.lab,
        };
        let de = delta_e_2000(lab, a.lab);
        eprintln!(
            "Cr recipe {} {:?} {} ppma*cm: {lab:.1?} vs GIA {:.1?} dE00 {de:.2}",
            a.spec, a.illuminant, a.ppma_cm, a.lab
        );
        let limit = if a.spec == "both" { 4.0 } else { 3.0 };
        assert!(
            de <= limit,
            "{} {:?} {} ppma*cm: dE00 {de:.2}",
            a.spec,
            a.illuminant,
            a.ppma_cm
        );
    }
}

/// The recipe follows the GIA cross-section itself (not only the published colors) from 100 to
/// 3000 ppma*cm: dE00 <= 3 per polarisation (measured <= 2.6, <= 1.4 up to 750 ppma*cm), including the dark colors
/// where Appendix 2 and the cross-section differ.
#[test]
fn cr_recipe_tracks_the_gia_sigma_color_up_to_3000_ppma_cm() {
    let t = table("gia_cr3plus_sample1110_sigma");
    let cols = (t.col("sigma_E_perp_c_cm2"), t.col("sigma_E_par_c_cm2"));
    for d in [30.0, 100.0, 200.0, 375.0, 750.0, 1500.0, 3000.0] {
        let tens = tensor(&cr_recipe(d));
        for ill in [Illuminant::D65, A] {
            for (which, ray) in [("E_perp_c", 'o'), ("E_par_c", 'e')] {
                let de = delta_e_2000(
                    ray_lab(&tens, ray, 10.0, ill),
                    sigma_lab(&t, cols, which, d, ill),
                );
                assert!(de <= 3.0, "{which} {ill:?} {d} ppma*cm: dE00 {de:.2}");
            }
        }
    }
}

/// The 450-500 nm window is real and the data_version 3 "LMCT / Urbach" band that filled it is
/// gone: the recipe's Cr absorption at 450 / 475 / 500 nm is the GIA value (E_perp_c sigma
/// 3.1e-20 / 1.5e-20 / 3.6e-20 cm^2) within 25 %, no Cr3+ band is centred in 430-520 nm with a
/// width above 4500 cm^-1 (the removed one: 454.55 nm, 16000 cm^-1), and the only charge-transfer
/// bands are the UV edge tails below 380 nm.
#[test]
fn ruby_window_is_open_and_the_lmct_window_band_is_removed() {
    let t = table("gia_cr3plus_sample1110_sigma");
    let c = t.col("sigma_E_perp_c_cm2");
    let n_wt = cat()
        .host("corundum")
        .expect("corundum")
        .n_site_for_unit("wt_pct_oxide:Cr2O3");
    for nm in [450.0, 475.0, 500.0] {
        let model = data_alpha("corundum", "Cr3+", "o", nm, 1.0);
        let gia = t.at(c, nm) * n_wt;
        eprintln!("Cr window {nm} nm: recipe {model:.2} vs GIA {gia:.2} cm^-1 per wt%");
        assert!(
            (model / gia - 1.0).abs() < 0.25,
            "{nm} nm: {model:.2} vs GIA {gia:.2}"
        );
    }
    let cr = cat()
        .host("corundum")
        .expect("corundum")
        .chromophores
        .iter()
        .find(|c| c.id == "Cr3+")
        .expect("Cr3+");
    for b in cr.usable_bands() {
        let fwhm = b.fwhm_cm1.expect("fwhm");
        assert!(
            !(430.0..=520.0).contains(&b.centre_nm) || fwhm < 4500.0,
            "no broad window band: {} nm, {fwhm} cm^-1",
            b.centre_nm
        );
        assert!(
            b.transition_class.as_deref() != Some("charge_transfer") || b.centre_nm < 380.0,
            "charge-transfer bands are UV edge tails: {} nm",
            b.centre_nm
        );
    }
}

/// Ruby E_perp_c is purplish red (negative b*) at moderate Cr*L with GIA's hue: 100-750 ppma*cm
/// have b* < 0, the hue is within 4 degrees of Appendix 2 (D65), and b* turns positive only
/// near 1500+ ppma*cm (GIA: 2.6 at 1500, 23.8 at 3000).
#[test]
fn ruby_e_perp_c_is_purplish_red_with_gia_hue() {
    let t = table("gia_cr3plus_sample1110_sigma");
    for d in [50.0, 100.0, 200.0, 375.0, 750.0] {
        let lab = ray_lab(&tensor(&cr_recipe(d)), 'o', 10.0, Illuminant::D65);
        assert!(
            lab[2] < 0.0,
            "{d} ppma*cm: b* {:.1} must be negative",
            lab[2]
        );
        assert!(lab[1] > 20.0, "{d} ppma*cm: red, a* {:.1}", lab[1]);
    }
    for a in t
        .appendix2()
        .iter()
        .filter(|a| a.spec == "E_perp_c" && matches!(a.illuminant, Illuminant::D65))
        .filter(|a| a.ppma_cm <= 750.0)
    {
        let lab = ray_lab(&tensor(&cr_recipe(a.ppma_cm)), 'o', 10.0, Illuminant::D65);
        let dh = (hue_deg(lab) - hue_deg(a.lab) + 540.0).rem_euclid(360.0) - 180.0;
        eprintln!(
            "ruby E_perp_c {} ppma*cm: hue {:.1} vs GIA {:.1}",
            a.ppma_cm,
            hue_deg(lab),
            hue_deg(a.lab)
        );
        assert!(
            dh.abs() < 4.0,
            "{} ppma*cm: hue off by {dh:.1} deg",
            a.ppma_cm
        );
    }
    let dark = ray_lab(&tensor(&cr_recipe(3000.0)), 'o', 10.0, Illuminant::D65);
    assert!(
        dark[2] > 10.0,
        "b* turns positive only at very high Cr*L: {dark:.1?}"
    );
}

// ---------------------------------------------------------------------------------------------
// V3+
// ---------------------------------------------------------------------------------------------

/// V3+ sigma: the recipe's E_perp_c absorption at 580 nm is the GIA xlsx value 3.4e-20 cm^2 per
/// ion (the paper text quotes 1.0e-19, which the xlsx contradicts), and the 400 nm / 405 nm
/// peaks of both polarisations follow the xlsx within 10 %.
#[test]
fn v3_sigma_follows_the_gia_xlsx_not_the_paper_text() {
    let t = table("gia_v3plus_fe2plus_ti4plus_sigma_5nm");
    let n_all = cat()
        .host("corundum")
        .expect("corundum")
        .n_site_for_unit("ppma_all");
    let (vp, ve) = (
        t.col("sigma_V3_E_perp_c_cm2"),
        t.col("sigma_V3_E_par_c_cm2"),
    );
    let sigma = |pol: &str, nm: f64| data_alpha("corundum", "V3+", pol, nm, 1.0) / n_all;
    assert!(
        (sigma("o", 580.0) / t.at(vp, 580.0) - 1.0).abs() < 0.1,
        "{:e}",
        sigma("o", 580.0)
    );
    assert!(
        sigma("o", 580.0) < 5.0e-20,
        "the text value 1.0e-19 is not used"
    );
    assert!((sigma("o", 400.0) / t.at(vp, 400.0) - 1.0).abs() < 0.1);
    assert!((sigma("e", 405.0) / t.at(ve, 405.0) - 1.0).abs() < 0.1);
}

/// V-corundum color change from the GIA xlsx sigma: D65 vs A dE00 is 8-25 (the report-1 table's
/// 2.9-5.8 is wrong). The recipe reproduces the sigma-derived color change (<= 0.8 apart) at
/// V*L = 110 and 340 ppma*cm and is >= 8 for E_par_c at 340 ppma*cm (24.8).
#[test]
fn v_corundum_color_change_matches_the_gia_derived_range() {
    let t = table("gia_v3plus_fe2plus_ti4plus_sigma_5nm");
    let cols = (
        t.col("sigma_V3_E_perp_c_cm2"),
        t.col("sigma_V3_E_par_c_cm2"),
    );
    for d in [110.0, 340.0] {
        let mut r = ColorRecipe::new("corundum", cat().data_version);
        // 1 ppma of all atoms = 2.5 ppm of Al sites; V is entered in ppm of Al sites.
        assert!(r.set_amount("V", d * 2.5));
        let tens = tensor(&r);
        for (which, ray) in [("E_perp_c", 'o'), ("E_par_c", 'e')] {
            let cc_sigma = delta_e_2000(
                sigma_lab(&t, cols, which, d, Illuminant::D65),
                sigma_lab(&t, cols, which, d, A),
            );
            let cc_recipe = delta_e_2000(
                ray_lab(&tens, ray, 10.0, Illuminant::D65),
                ray_lab(&tens, ray, 10.0, A),
            );
            eprintln!("V {d} ppma*cm {which}: sigma {cc_sigma:.1}, recipe {cc_recipe:.1}");
            assert!(
                (cc_recipe - cc_sigma).abs() <= 0.8,
                "{which} {d}: recipe {cc_recipe:.2} vs sigma {cc_sigma:.2}"
            );
            assert!(
                (8.0..=26.0).contains(&cc_recipe),
                "{which} {d}: {cc_recipe:.1}"
            );
        }
        let mix = |ill| body_colors(&tens, 10.0, ill).unpolarised.lab;
        let cc_mix = delta_e_2000(mix(Illuminant::D65), mix(A));
        assert!(
            (8.0..=25.0).contains(&cc_mix),
            "unpolarised {d}: {cc_mix:.1}"
        );
    }
    let mut r = ColorRecipe::new("corundum", cat().data_version);
    assert!(r.set_amount("V", 340.0 * 2.5));
    let tens = tensor(&r);
    let cc_e = delta_e_2000(
        ray_lab(&tens, 'e', 10.0, Illuminant::D65),
        ray_lab(&tens, 'e', 10.0, A),
    );
    assert!(cc_e >= 8.0, "V 340 ppma*cm E_par_c: dE_cc {cc_e:.2}");
}

// ---------------------------------------------------------------------------------------------
// Fe2+-Ti4+
// ---------------------------------------------------------------------------------------------

/// The Fe2+-Ti4+ band height of the resolved o-ray (mm^-1) at the strongest o band of the pair
/// chromophore, for `fe` / `ti` / `mg` in ppm of Al sites.
fn pair_peak(fe: f64, ti: f64, mg: f64) -> f64 {
    let c = cat().host("corundum").expect("corundum");
    let pair = c
        .chromophores
        .iter()
        .find(|c| c.id == "Fe2+-Ti4+")
        .expect("pair");
    let band = pair
        .usable_bands()
        .filter(|b| b.pol.get("o").copied().unwrap_or(0.0) > 0.0)
        .max_by(|a, b| a.peak_coeff.partial_cmp(&b.peak_coeff).expect("finite"))
        .expect("o band");
    let mut r = ColorRecipe::new("corundum", cat().data_version);
    r.set_amount("Fe", fe);
    r.set_amount("Ti", ti);
    if mg > 0.0 {
        r.set_amount("Mg", mg);
    }
    tensor(&r)
        .o_ray
        .iter()
        .find(|b| (f64::from(b.center_nm) - band.centre_nm).abs() < 0.5)
        .map_or(0.0, |b| f64::from(b.peak))
}

/// GIA 2020 / Emmett 2003 (primary-data report section 7): the Fe2+-Ti4+ pair count is linear
/// in the available Ti = Ti - Mg with Fe in excess (no Fe x Ti product), and a Mg >= Ti
/// compensation zeroes the band.
#[test]
fn fe_ti_pairs_are_linear_in_available_ti_with_fe_in_excess() {
    // Fe in excess: the band does not depend on Fe (Fe2+ = 85 % of Fe >> Ti).
    let (p_lo, p_hi) = (pair_peak(1000.0, 60.0, 0.0), pair_peak(3000.0, 60.0, 0.0));
    assert!(
        p_lo > 0.0 && (p_hi / p_lo - 1.0).abs() < 1e-5,
        "{p_lo} vs {p_hi}"
    );
    // Linear in Ti.
    let p100 = pair_peak(3000.0, 100.0, 0.0);
    for ti in [20.0, 50.0, 80.0] {
        let p = pair_peak(3000.0, ti, 0.0);
        assert!(
            (p / p100 - ti / 100.0).abs() < 1e-4,
            "Ti {ti}: {p} vs {p100}"
        );
    }
    // Mg compensation: linear in Ti - Mg, zero for Mg >= Ti.
    for mg in [0.0, 25.0, 50.0, 75.0] {
        let p = pair_peak(3000.0, 100.0, mg);
        assert!(
            (p / p100 - (100.0 - mg) / 100.0).abs() < 1e-4,
            "Mg {mg}: {p} vs {p100}"
        );
    }
    assert_eq!(pair_peak(3000.0, 100.0, 100.0), 0.0, "Mg = Ti: no pairs");
    assert_eq!(pair_peak(3000.0, 100.0, 250.0), 0.0, "Mg > Ti: no pairs");
    // Fe-limited: Ti beyond the Fe2+ (85 % of Fe) adds nothing.
    let capped = pair_peak(100.0, 400.0, 0.0);
    assert!((capped / pair_peak(100.0, 85.0, 0.0) - 1.0).abs() < 1e-4);
    // Not a product law: tripling Fe at fixed Ti leaves the pairs unchanged (above), tripling
    // Ti triples them (below Fe2+).
    assert!((pair_peak(3000.0, 90.0, 0.0) / pair_peak(3000.0, 30.0, 0.0) - 3.0).abs() < 1e-4);
}

/// The Fe2+-Ti4+ bands are the GIA cross-section per pair (shape and magnitude): sigma(580 nm,
/// E_perp_c) = 1.98e-18 cm^2 within 3 %, sigma(700 nm, E_par_c) = 1.25e-18 within 3 %, i.e.
/// 0.23 cm^-1 per ppma of all atoms (0.091 per ppm of Al sites) and not the 0.05 per ppm of
/// data_version 3. Shape: E_perp_c within 8 % of the cross-section over 400-660 nm and E_par_c
/// over 400-780 nm. The E_perp_c red tail beyond 670 nm is up to 50 % low at 780 nm (two
/// Gaussians fitted in color terms: the tail carries no color weight, the fit's color error
/// against the sigma is <= 0.5 dE00).
#[test]
fn fe_ti_follows_the_gia_cross_section_in_shape_and_magnitude() {
    let t = table("gia_v3plus_fe2plus_ti4plus_sigma_5nm");
    let (fp, fe) = (
        t.col("sigma_FeTi_E_perp_c_cm2"),
        t.col("sigma_FeTi_E_par_c_cm2"),
    );
    let n_site = cat()
        .host("corundum")
        .expect("corundum")
        .n_site_for_unit("ppm_site");
    let sigma = |pol: &str, nm: f64| data_alpha("corundum", "Fe2+-Ti4+", pol, nm, 1.0) / n_site;
    let (s580, s700) = (sigma("o", 580.0), sigma("e", 700.0));
    eprintln!("Fe-Ti sigma 580 nm E_perp_c {s580:.3e}, 700 nm E_par_c {s700:.3e}");
    assert!((s580 / t.at(fp, 580.0) - 1.0).abs() < 0.03);
    assert!((s700 / t.at(fe, 700.0) - 1.0).abs() < 0.03);
    let per_ppma_all = s580
        * cat()
            .host("corundum")
            .expect("corundum")
            .n_site_for_unit("ppma_all");
    assert!((per_ppma_all - 0.229).abs() < 0.01, "{per_ppma_all}");
    for nm in (400..=780).step_by(20) {
        let nm = f64::from(nm);
        let e = sigma("e", nm) / t.at(fe, nm) - 1.0;
        assert!(e.abs() < 0.08, "E_par_c {nm} nm: {e:+.3}");
        if nm <= 660.0 {
            let o = sigma("o", nm) / t.at(fp, nm) - 1.0;
            assert!(o.abs() < 0.08, "E_perp_c {nm} nm: {o:+.3}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Axis mappings and ray colors
// ---------------------------------------------------------------------------------------------

/// Axis mappings are stored by axis length (the cell settings only relabel a/b/c): chrysoberyl
/// alpha 4.43, beta 9.40, gamma 5.48 A; zoisite alpha 5.55, beta 10.0, gamma 16.2 A; forsterite
/// alpha 10.2, beta 6.0, gamma 4.76 A.
#[test]
fn ray_axes_are_stored_by_axis_length() {
    let axes = |host: &str| cat().host(host).expect("host").ray_axis_angstrom.clone();
    let chry = axes("chrysoberyl");
    assert_eq!(chry.len(), 3);
    assert!((chry["a"] - 4.427).abs() < 0.01 && (chry["b"] - 9.404).abs() < 0.01);
    assert!((chry["g"] - 5.476).abs() < 0.01);
    assert!(
        chry["a"] < chry["g"] && chry["g"] < chry["b"],
        "alpha 4.43 < gamma 5.48 < beta 9.40"
    );
    let zoi = axes("tanzanite");
    assert!((zoi["a"] - 5.55).abs() < 0.01 && (zoi["b"] - 10.0).abs() < 0.01);
    assert!((zoi["g"] - 16.2).abs() < 0.01);
    assert!(zoi["a"] < zoi["b"] && zoi["b"] < zoi["g"]);
    let ol = axes("peridot");
    assert!(
        ol["a"] > ol["b"] && ol["b"] > ol["g"],
        "olivine: alpha is the longest axis"
    );
    assert_eq!(
        cat()
            .host("chrysoberyl")
            .expect("host")
            .axis_mapping_confidence
            .as_deref(),
        Some("verified")
    );
    assert_eq!(
        cat()
            .host("tanzanite")
            .expect("host")
            .axis_mapping_confidence
            .as_deref(),
        Some("verified")
    );
}

/// Alexandrite: alpha (4.43 A axis) purple-violet, beta (9.40 A) yellow-orange, gamma (5.48 A)
/// blue-green. beta and gamma follow the Caltech GRR 874 rays (D65 L*a*b* 74.1/-9.6/39.6 and
/// 51.8/-57.5/-13.4 at 0.717 mm); alpha has no primary spectrum (its shape is the report's
/// alpha column) and is violet-blue under A, blue-grey under D65: never yellow or green.
#[test]
fn alexandrite_rays_have_the_right_colors_by_axis_length() {
    let mut r = ColorRecipe::new("chrysoberyl", cat().data_version);
    r.set_amount("Cr", 0.32);
    r.set_amount("Fe", 0.15);
    let t = tensor(&r);
    let h = |ray: char, ill| hue_deg(ray_lab(&t, ray, 2.0, ill));
    eprintln!(
        "alexandrite hues D65 alpha {:.0} beta {:.0} gamma {:.0}; A alpha {:.0} beta {:.0} gamma {:.0}",
        h('o', Illuminant::D65),
        h('b', Illuminant::D65),
        h('e', Illuminant::D65),
        h('o', A),
        h('b', A),
        h('e', A)
    );
    // beta: yellow-orange in both illuminants.
    for ill in [Illuminant::D65, A] {
        assert!(
            (60.0..=120.0).contains(&h('b', ill)),
            "beta hue {:.0}",
            h('b', ill)
        );
    }
    // gamma: blue-green (cyan-green) in D65, still green-blue under A.
    assert!(
        (170.0..=230.0).contains(&h('e', Illuminant::D65)),
        "gamma D65 {:.0}",
        h('e', Illuminant::D65)
    );
    // alpha: violet-blue under A, never yellow or green.
    assert!(
        (250.0..=330.0).contains(&h('o', A)),
        "alpha A {:.0}",
        h('o', A)
    );
    assert!(
        h('o', Illuminant::D65) >= 200.0,
        "alpha D65 {:.0}",
        h('o', Illuminant::D65)
    );
    // The stone changes color: unpolarised D65 vs A.
    let cc = {
        let u = |ill| body_colors(&t, 20.0, ill).unpolarised.lab;
        delta_e_2000(u(Illuminant::D65), u(A))
    };
    assert!(cc > 8.0, "alexandrite color change {cc:.1}");
}

/// Caltech GRR 874 itself through the recipe at the stone that matches the gamma 4T2 magnitude
/// (1.05 wt% Cr2O3 equivalent, 0.717 mm): beta yellow (hue 100 +- 15), gamma blue-green.
#[test]
fn alexandrite_recipe_reproduces_the_caltech_ray_colors() {
    let cal = table("caltech_chrysoberyl_grr874_5nm");
    let (cb, cg) = (cal.col("A_beta"), cal.col("A_gamma"));
    let path_cm = 0.0717;
    let caltech = |c: usize| {
        body_color(
            |l| std::f64::consts::LN_10 * cal.at(c, l) / path_cm / 10.0,
            path_cm * 10.0,
            Illuminant::D65,
        )
        .lab
    };
    let mut r = ColorRecipe::new("chrysoberyl", cat().data_version);
    r.set_amount("Cr", 1.046);
    let t = tensor(&r);
    for (ray, c) in [('b', cb), ('e', cg)] {
        let de = delta_e_2000(
            ray_lab(&t, ray, path_cm * 10.0, Illuminant::D65),
            caltech(c),
        );
        eprintln!("GRR 874 {ray}: dE00 {de:.1} (the Caltech spectrum keeps its pedestal)");
        assert!(de < 12.0, "ray {ray}: dE00 {de:.1}");
    }
}

/// Tanzanite (unheated, V 7670 ppm of Al sites, 4 mm): alpha (5.55 A axis) red-violet, beta
/// (10.0 A) blue, gamma (16.2 A) yellow-green, as the Caltech GRR 1265 rays; heating bleaches
/// the 440 nm band that makes gamma yellow-green, and gamma turns blue-violet.
#[test]
fn tanzanite_rays_have_the_right_colors_by_axis_length() {
    let mk = |heated: bool| {
        let mut r = ColorRecipe::new("tanzanite", cat().data_version);
        r.set_amount("V", 7670.0);
        if heated {
            r.treatments.push("air_anneal_550".to_string());
        }
        tensor(&r)
    };
    let (raw, heated) = (mk(false), mk(true));
    let h = |t: &AbsorptionTensor, ray: char| hue_deg(ray_lab(t, ray, 4.0, Illuminant::D65));
    eprintln!(
        "tanzanite hues alpha {:.0} beta {:.0} gamma {:.0}; heated gamma {:.0}",
        h(&raw, 'o'),
        h(&raw, 'b'),
        h(&raw, 'e'),
        h(&heated, 'e')
    );
    assert!(
        (300.0..=345.0).contains(&h(&raw, 'o')),
        "alpha red-violet {:.0}",
        h(&raw, 'o')
    );
    assert!(
        (235.0..=280.0).contains(&h(&raw, 'b')),
        "beta blue {:.0}",
        h(&raw, 'b')
    );
    assert!(
        (100.0..=160.0).contains(&h(&raw, 'e')),
        "gamma yellow-green {:.0}",
        h(&raw, 'e')
    );
    assert!(
        (240.0..=300.0).contains(&h(&heated, 'e')),
        "heated gamma {:.0}",
        h(&heated, 'e')
    );
    // alpha and beta are untouched by the heating.
    assert!((h(&heated, 'o') - h(&raw, 'o')).abs() < 1e-9);
    assert!((h(&heated, 'b') - h(&raw, 'b')).abs() < 1e-9);
}

/// Peridot: all three rays yellow-green; beta is the strongest absorber (Caltech GRR 418: A at
/// 450 nm 3.9 against 2.2 / 2.1 cm^-1 for alpha / gamma).
#[test]
fn peridot_rays_are_yellow_green_and_beta_absorbs_most() {
    let mut r = ColorRecipe::new("peridot", cat().data_version);
    r.set_amount("fayalite", 0.10);
    let t = tensor(&r);
    let lab = |ray| ray_lab(&t, ray, 5.0, Illuminant::D65);
    for ray in ['o', 'b', 'e'] {
        assert!(
            (90.0..=130.0).contains(&hue_deg(lab(ray))),
            "{ray}: {:.0}",
            hue_deg(lab(ray))
        );
    }
    assert!(
        lab('b')[0] < lab('o')[0] && lab('b')[0] < lab('e')[0],
        "beta darkest"
    );
}

// ---------------------------------------------------------------------------------------------
// Caltech shape references (no composition)
// ---------------------------------------------------------------------------------------------

/// color weight `xbar + ybar + zbar` at `lambda` (the weight of the fits).
fn weight(lambda: f64) -> f64 {
    let c = cie_1931_cmf(lambda as f32);
    f64::from(c[0] + c[1] + c[2])
}

/// color-weighted shape residual of `model` (alpha per unit, cm^-1) against `target` (cm^-1)
/// over `from_nm`-780 nm at 1 nm after the best scale and flat offset: returns
/// `(scale, offset, rms / weighted spread of the target)`.
fn shape_residual(
    model: &dyn Fn(f64) -> f64,
    target: &dyn Fn(f64) -> f64,
    from_nm: i32,
) -> (f64, f64, f64) {
    let pts: Vec<(f64, f64, f64)> = (from_nm..=780)
        .map(|l| {
            let l = f64::from(l);
            (model(l), target(l), weight(l))
        })
        .collect();
    let (mut sw, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for &(x, y, w) in &pts {
        let w = w * w;
        sw += w;
        sx += w * x;
        sy += w * y;
        sxx += w * x * x;
        sxy += w * x * y;
    }
    let scale = (sw * sxy - sx * sy) / (sw * sxx - sx.powi(2));
    let offset = (sy - scale * sx) / sw;
    let (mut res, mut spread) = (0.0, 0.0);
    for &(x, y, w) in &pts {
        let w = w * w;
        res += w * (scale * x + offset - y).powi(2);
        spread += w * (y - sy / sw).powi(2);
    }
    (scale, offset, (res / spread).sqrt())
}

/// Napierian alpha (cm^-1) of an `A_` column of a Caltech file of path `path_cm`.
fn caltech_alpha(t: &Table, col: usize, path_cm: f64) -> impl Fn(f64) -> f64 + '_ {
    move |l| std::f64::consts::LN_10 * t.at(col, l) / path_cm
}

/// The recipe's alpha (cm^-1 per unit) of one chromophore and polarisation, as a function of nm.
fn eval(host: &str, id: &str, pol: &str) -> impl Fn(f64) -> f64 + use<> {
    let (host, id, pol) = (host.to_string(), id.to_string(), pol.to_string());
    move |l: f64| data_alpha(&host, &id, &pol, l, 1.0)
}

/// Prints each `(name, residual, limit)` row and asserts the residual is within its limit.
fn assert_residuals_within_limits(checks: Vec<(&str, f64, f64)>) {
    for (name, rel, limit) in checks {
        eprintln!(
            "shape {name}: residual {:.1} % of the weighted spread",
            100.0 * rel
        );
        assert!(
            rel <= limit,
            "{name}: {:.1} % > {:.0} %",
            100.0 * rel,
            100.0 * limit
        );
    }
}

/// Shape of the refitted hosts against the Caltech raw spectra: the color-weighted residual
/// after a free scale and flat offset (the pedestal), as a share of the target's weighted
/// spread. The scale is not asserted (no composition); the shapes are.
#[test]
fn recipe_shapes_follow_the_caltech_spectra() {
    // Chrysoberyl beta (raw ch874.a) and gamma (5 nm digest of ch874.b).
    let raw = table("caltech_chrysoberyl_grr874_beta_ch874a_0p717mm");
    let digest = table("caltech_chrysoberyl_grr874_5nm");
    let checks: Vec<(&str, f64, f64)> = vec![
        (
            "chrysoberyl beta",
            shape_residual(
                &eval("chrysoberyl", "Cr3+", "b"),
                &caltech_alpha(&raw, 1, 0.0717),
                380,
            )
            .2,
            0.10,
        ),
        (
            "chrysoberyl gamma",
            shape_residual(
                &eval("chrysoberyl", "Cr3+", "g"),
                &caltech_alpha(&digest, digest.col("A_gamma"), 0.0717),
                380,
            )
            .2,
            0.15,
        ),
        // Emerald GRR 3570.
        (
            "beryl Cr3+ o",
            {
                let t = table("caltech_emerald_grr3570_5nm");
                shape_residual(
                    &eval("beryl", "Cr3+", "o"),
                    &caltech_alpha(&t, t.col("A_E_perp_c"), 0.0859),
                    380,
                )
                .2
            },
            0.12,
        ),
        (
            "beryl Cr3+ e",
            {
                let t = table("caltech_emerald_grr3570_5nm");
                shape_residual(
                    &eval("beryl", "Cr3+", "e"),
                    &caltech_alpha(&t, t.col("A_E_par_c"), 0.0859),
                    380,
                )
                .2
            },
            0.25,
        ),
        // Peridot GRR 418 (the three raw files).
        (
            "peridot alpha",
            shape_residual(
                &eval("peridot", "Fe2+ (fayalite component, M1/M2)", "a"),
                &caltech_alpha(&table("caltech_olivine_grr418_alpha"), 1, 0.1141),
                380,
            )
            .2,
            0.12,
        ),
        (
            "peridot beta",
            shape_residual(
                &eval("peridot", "Fe2+ (fayalite component, M1/M2)", "b"),
                &caltech_alpha(&table("caltech_olivine_grr418_beta"), 1, 0.11381),
                380,
            )
            .2,
            0.12,
        ),
        (
            "peridot gamma",
            shape_residual(
                &eval("peridot", "Fe2+ (fayalite component, M1/M2)", "g"),
                &caltech_alpha(&table("caltech_olivine_grr418_gamma"), 1, 0.066),
                380,
            )
            .2,
            0.15,
        ),
        // Amethyst (Anahi).
        (
            "quartz amethyst centre",
            {
                let t = table("caltech_quartz_amethyst_ameth_bo_5nm");
                shape_residual(
                    &eval("quartz", "Fe4+ (amethyst centre)", "o"),
                    &caltech_alpha(&t, t.col("A_E_perp_c"), 0.617),
                    380,
                )
                .2
            },
            0.15,
        ),
    ];
    assert_residuals_within_limits(checks);
}

/// Zoisite GRR 1265 (405-780 nm only; no data below): the tanzanite shapes.
#[test]
fn tanzanite_shapes_follow_the_caltech_zoisite_spectra() {
    let z = table("caltech_zoisite_grr1265_5nm");
    let id = "V3+ (natural unheated, trichroic)";
    for (name, pol, col, limit) in [
        ("tanzanite alpha", "a", "A_b_alpha", 0.08),
        ("tanzanite beta", "b", "A_c_beta", 0.40),
        ("tanzanite gamma", "g", "A_a_gamma", 0.20),
    ] {
        let rel = shape_residual(
            &eval("tanzanite", id, pol),
            &caltech_alpha(&z, z.col(col), 0.3),
            405,
        )
        .2;
        eprintln!("shape {name}: residual {:.1} %", 100.0 * rel);
        assert!(rel <= limit, "{name}: {:.1} %", 100.0 * rel);
    }
}

/// The GIA Cr3+ sigma itself through the same metric (scale ~ 1 per wt%).
#[test]
fn the_gia_cr3plus_sigma_fits_the_recipe_with_unit_scale() {
    let g = table("gia_cr3plus_sample1110_sigma");
    let n_wt = cat()
        .host("corundum")
        .expect("corundum")
        .n_site_for_unit("wt_pct_oxide:Cr2O3");
    for (pol, col, limit) in [
        ("o", "sigma_E_perp_c_cm2", 0.18),
        ("e", "sigma_E_par_c_cm2", 0.18),
    ] {
        let c = g.col(col);
        let (s, _, rel) =
            shape_residual(&eval("corundum", "Cr3+", pol), &|l| g.at(c, l) * n_wt, 380);
        eprintln!(
            "shape Cr3+ {pol}: scale {s:.3}, residual {:.1} %",
            100.0 * rel
        );
        assert!(
            (s - 1.0).abs() < 0.08 && rel <= limit,
            "{pol}: scale {s:.3}, {:.1} %",
            100.0 * rel
        );
    }
}

/// The Caltech ruby GRR 1843 (E_perp_c, no composition) has a negative b* like the GIA ruby:
/// computed from the raw file, D65 b* -12.6 (0.787 mm) with the file's own baseline offset.
#[test]
fn caltech_ruby_e_perp_c_is_purplish_red_too() {
    let t = table("caltech_ruby_grr1843_eperpc_0p787mm");
    let lab = body_color(
        |l| std::f64::consts::LN_10 * t.at(1, l) / 0.0787 / 10.0,
        0.787,
        Illuminant::D65,
    )
    .lab;
    eprintln!("Caltech GRR 1843 E_perp_c D65: {lab:.1?}");
    assert!(lab[1] > 25.0 && lab[2] < -5.0, "{lab:.1?}");
}

/// The Caltech blue sapphire GRR 1020a has the Fe2+-Fe3+ maximum at 870-890 nm and E_par_c
/// weaker than E_perp_c at 580 nm, as the recipe's Fe2+-Ti4+ cross-sections (E_par_c / E_perp_c
/// = 0.42 at 580 nm); the file's pedestal makes the absolute ratio unusable.
#[test]
fn caltech_sapphire_has_the_expected_pleochroism() {
    let o = table("caltech_sapphire_grr1020a_eperpc_4p289mm");
    let e = table("caltech_sapphire_grr1020a_eparc_4p289mm");
    let peak = o
        .rows
        .iter()
        .filter(|r| r[0] > 700.0 && r[0] < 1000.0)
        .max_by(|a, b| a[1].total_cmp(&b[1]))
        .expect("rows");
    assert!(
        (860.0..=900.0).contains(&peak[0]),
        "Fe2+-Fe3+ maximum at {} nm",
        peak[0]
    );
    assert!(e.at(1, 580.0) < o.at(1, 580.0));
    let (ro, re) = (
        data_alpha("corundum", "Fe2+-Ti4+", "o", 580.0, 1.0),
        data_alpha("corundum", "Fe2+-Ti4+", "e", 580.0, 1.0),
    );
    assert!(re < ro);
}
