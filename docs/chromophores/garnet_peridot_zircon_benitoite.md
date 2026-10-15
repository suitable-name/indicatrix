# Chromophore dataset: garnet group, peridot, zircon, benitoite

Target: physically based spectral renderer, 380-780 nm, Beer-Lambert, `alpha(lambda) = sum_i peak_coeff_i * conc_i * pol * g_i(lambda)`.

Revision note: merged with the user-supplied report "Gem Chromophore Spectroscopy Verification" (48 pp., cited as `Report p.N`). Where the report gives a value it replaces the prior one (prior value kept in a comment when it differs), `confidence = "secondary"` and `source = "Report p.N (...)"`. Values the report does not give keep their earlier confidence. Unknown values are now **absent fields** with a `# unknown` comment (no more `-1.0`).

## Conventions and honesty statement

- `peak_coeff` is the **peak absorption coefficient in cm^-1 per unit of `conc_unit`** (per 1.0 mol fraction, per 1 wt% oxide, per ppm U, per relative centre density). A **missing** `peak_coeff` (comment `# unknown`) means no honest value exists: the renderer must skip that band. Same for any other absent field (`fwhm_cm1`, `quantum_yield`, `conc_typical`).
- Band shapes: `gaussian_energy` = Gaussian in wavenumber (FWHM in cm^-1); `lorentzian` = sharp f-f / spin-forbidden lines; `edge` = absorption-edge tail labelled "Edge" by the report, no FWHM given, renderer must define the profile (new, report-derived). nm = 1e7 / cm^-1.
- Confidence tiers:
  - `verified` = the number was read in an excerpt of a retrieved source in the earlier session (search-result excerpts only, full PDFs not opened). **No band in this file is `verified` any more**: every band that the report also gives was moved to `secondary` (the old tier is kept in a comment, e.g. `# prior: verified`), because the report does not attribute numbers to references.
  - `secondary` = value taken from the report (or, for the few unchanged items, a textbook value: Burns 1993; Nassau 1983; Rossman) and not independently re-measured here.
  - `estimate` = my own construction (explained in the `source` string). **Caveat found during the merge:** the report's peak coefficients, concentrations (25.9 / 25.4 / 15.2 mol/L) and epsilon values are numerically identical to the prior file's `estimate` values (e.g. 15/20/25 for almandine, 270/190/190 for peridot, 60/25/400 for andradite, 8/8/20 for zircon centres). The report names no primary table, so its agreement with the prior file is **not independent corroboration**; the coefficients are still epsilon x concentration estimates (epsilon 10-40 for spin-allowed d-d, 0.1-2 for spin-forbidden, 100-1000 for IVCT). Calibrate against measured spectra before shipping.
- No Rossman Caltech spectra pages or RRUFF spectra were retrieved. This is the largest verification gap.
- Fluorescence blocks: an absent `quantum_yield` means unknown; `quantum_yield = 0.0` is kept only where the block is asserted non-luminescent (inert). `emission_fwhm_nm` is omitted where nothing is emitted. `uv = { lw, sw }` is the observed gem-lab response.

### Reference list (short IDs used in `source` fields)

| ID | Reference |
|---|---|
| R | Report: "Gem Chromophore Spectroscopy Verification" (user-supplied, 48 pp.; garnet prose pp. 14-17, peridot p. 18-19, zircon p. 19-20, benitoite p. 20-21; rules pp. 30-31 and 46; directory pp. 32-46: garnets pp. 38-39, peridot/zircon/benitoite pp. 40-41). Cited as `Report p.N`. |
| S1 | Fe2+-Fe3+ IVCT in garnets (dodecahedral Fe2+ / octahedral Fe3+, band near 21,700 cm^-1, FWHM ~7300), Am. Mineral. 92, 2007, doi:10.2138/am.2007.2163 (https://www.degruyterbrill.com/document/doi/10.2138/am.2007.2163/html; authors not confirmed from the excerpt) |
| S2 | Taran, Geiger, Vyshnevskyi, Rossman, spin-forbidden Fe2+/Mn2+/Fe3+ bands in almandine/spessartine garnets, Am. Mineral. 108, 2023, 1149-1171 (https://msaweb.org/MSA/AmMin/products/AmMin/AM108P1149.pdf; https://msaweb.org/MSA/AmMin/TOC/2023/Abstracts/AM108P1161.pdf) |
| S3 | Almandine Fe2+ spin-allowed NIR bands 4500, 6100, 7800 cm^-1 (excerpt from the search "almandine garnet optical absorption", repository.geologyscience.ru/handle/123456789/51303; primary not identified) |
| S4 | Characterization of Peridot from Sardinia, Italy, GIA (https://www.gia.edu/doc/Characterization-of-Peridot-from-Sardinia-Italy.pdf) |
| S5 | Burns R.G. 1970, Mineralogical Applications of Crystal Field Theory, Cambridge UP; Burns 1993 2nd ed. (not retrieved; cited via S4 and the arXiv MGM paper https://arxiv.org/pdf/2003.07856) |
| S6 | color-change pyrope-spessartine, Laser & Optoelectronics Progress 59(13) 1333001, 2022 (https://www.researching.cn/ArticlePdf/m00002/2022/59/13/1333001.pdf): 571 nm Cr3+/V3+ band, Mn2+ 420 nm tail |
| S7 | Vanadium/chromium-bearing pink pyrope, Gems & Gemology 51(4), Winter 2015 (https://www.gia.edu/gems-gemology/winter-2015-vanadium-chromium-bearing-pink-pyrope-garnet-characterization-quantitative-colorimetric-analysis); Schmetzer et al. 2009 as cited there |
| S8 | Gem-quality Cr-bearing demantoid, Dobsina, Slovakia, Minerals 2019 (https://doaj.org/article/d40d883585e54299ba1b9300d8417897): bands 438, 587, 623, 861 nm; Cr up to 0.15 apfu, V up to 0.006 apfu |
| S9 | Renfro et al., Reversible color modification in blue zircon by LWUV, Gems & Gemology 52(3), Fall 2016 (https://gia.edu/gems-gemology/fall-2016-reversible-color-modification-blue-zircon-long-wave-ultraviolet-radiation) |
| S10 | Zeug et al., Ratanakiri zircon, J. Gemmology 2018 (https://www.duo.uio.no/bitstream/10852/67410/1/Zeug_2018_JGe_Ratanakiri-zircon.pdf): U4+ at 654/690 nm |
| S11 | Fe2+-Ti4+ IVCT in Ti-rich silicates (taramellite, traskite, neptunite), band 415-460 nm, epsilon ~100-1300 M^-1 cm^-1 (Rossman group; via https://repository.geologyscience.ru/handle/123456789/24431 and Eur. J. Mineral. 12, dumortierite paper) |
| S12 | Benitoite: gemdat https://www.gemdat.org/gem-624.html ; LibreTexts https://geo.libretexts.org/Bookshelves/Geology/Gemology/16%3A_Gemstones/16.23%3A_Benitoite |
| S13 | GNI, Discovery of color-change chrome grossular, Ethiopia, G&G Summer 2018 (https://origin.prod.gia.edu/gems-gemology/summer-2018-gemnews-discovery-of-color-change-chrome-grossular-garnets-from-ethiopia); GNI Blue-green pyrope-spessartine, G&G Fall 2017 (https://www.gia.edu/gems-gemology/fall-2017-gemnews-blue-green-pyrope-spessartine) |
| S14 | Nassau K. 1983, The Physics and Chemistry of Color, Wiley (not retrieved; general crystal-field and charge-transfer classification) |
| S15 | Rossman G.R., Caltech mineral spectroscopy server, minerals.gps.caltech.edu (not retrieved; spectra not read) |
| EST | Estimate by the author of this dataset; method in the field's own text |

---

## 1. Garnet solid-solution model (applies to all garnet hosts)

Formula `X3Y2Z3O12`, X = dodecahedral (Mg, Fe2+, Mn2+, Ca), Y = octahedral (Al, Fe3+, Cr3+, V3+), Z = Si.

- **Pyralspite** (Y = Al): pyrope Mg3Al2Si3O12, almandine Fe3Al2Si3O12, spessartine Mn3Al2Si3O12.
- **Ugrandite** (X = Ca): grossular Ca3Al2Si3O12, andradite Ca3Fe3+2Si3O12 (uvarovite Ca3Cr2Si3O12 not requested).
- Pyralspite-ugrandite miscibility is limited (gem pyralspites carry <~15-20 mol% Ca; ugrandites carry <~5 mol% Fe2+/Mg; EST, not in the report), so the two ugrandite hosts are modelled separately.

**How end-member fractions combine** (Report p.14: site constraint `sum x_i <= 1.0`, `C_i = n_site * x_i * rho / M_fu` with n_site = 3 for dodecahedral X and 2 for octahedral Y; all garnet polarisation weights `w_o = 1.0`; Report p.46: coefficients scale linearly with site occupancy fractions). The same rules are stored in each garnet host's `rules` array below.

1. Essential-cation chromophores are `kind = "end_member"` with `conc_unit = "mol fraction end-member"`. Fe2+ dodecahedral sites per formula unit = `3 * x_Alm`, so the molar concentration is `c = 3 * x * rho / M_fu` (1000 factor for mol/L). Reference values (Report p.14-16, recomputed here): almandine rho 4.3 g/cm3, M 497.8 -> 3 x 4300 / 497.8 = 25.9 mol/L Fe2+ at x = 1; spessartine rho 4.19, M 495.0 -> 25.4 mol/L Mn2+; andradite rho 3.85, M 508.2 -> 2 x 3850 / 508.2 = 15.2 mol/L Fe3+ (2 per f.u.).
2. `peak_coeff` of an end-member chromophore is already normalised to **x = 1.0**, so `alpha = peak_coeff * x`. This is Beer's law linear in site occupancy; band positions are taken composition-independent (real shifts of a few nm between pyrope-rich and almandine-rich hosts are neglected; magnitude unknown).
3. Trace transition-metal ions (Cr3+, V3+) are `kind = "ion"` in `wt% oxide` (Cr2O3, V2O3) and add independently: `alpha_total = sum alpha_i`.
4. `Fe2+-Fe3+` IVCT (Y-site Fe3+ with X-site Fe2+) is an `ivct_pair`; its strength scales with the **product** `x_Alm * x_Fe3+` (Report p.15 and the general product rule c_eff ~ c_A * c_B on p.30), where `x_Fe3+` is the Fe3+ fraction on Y. For pyralspite Fe3+ is <~1 wt% Fe2O3 (skiagite component, EST), so the contribution is small except in Fe-rich garnet.
5. Site-sum constraint: the renderer must reject sets where `sum x > 1`.
6. Presets (no separate app material): **Rhodolite** (Report p.15) = Pyrope Garnet with `x_Alm` 0.30-0.45, `x_Prp` 0.55-0.70, `x_Sps` <= 0.05 (pink-purple-red; Fe2+ bands + low Cr/V). **color-change pyrope-spessartine** (Report p.15) = Pyrope Garnet with `x_Alm` <= 0.10, `x_Sps` about 0.15-0.45, Cr2O3 + V2O3 about **0.5-1.5 wt%** (prior file: 0.2-1.5; the report's lower bound is adopted) (see 2 below). `x_Prp` 0.50-0.80 is the prior file's EST and is not in the report. **Hessonite** and **topazolite** are Grossular / Andradite presets (see hosts 2 and 3). **Tsavorite** = Grossular Garnet (Tsavorite) with V/Cr.
7. color-change threshold (Report p.15 and p.46, rules): if `x_Alm > 0.10` the Fe2+ spin-forbidden bands at 504 and 527 nm fill the blue-green (480-520 nm) transmission window and suppress the color change. Stored as `excludes` on Cr3+/V3+ and as a host rule.

Allowed (physically occurring) element choices per garnet host (EST except where the report states it):

| Element / end-member | Pyrope-rich | Almandine | Spessartine | Grossular | Andradite |
|---|---|---|---|---|---|
| Fe2+ (X site, almandine component) | yes | yes (essential) | minor | no (traces only) | no |
| Mn2+ (X site) | yes (<~0.2) | minor | yes (essential) | yes (hessonite, traces; Mn-grossular) | trace |
| Cr3+ (Y) | yes (pink-red-color-change) | rare | rare/yes in CC pyrope-spessartine | yes (tsavorite, CC chrome grossular) | yes (demantoid) |
| V3+ (Y) | yes (color-change pyrope-spessartine) | no (masked) | yes (with Prp) | yes (tsavorite) | trace (up to ~0.006 apfu) |
| Fe3+ (Y) | trace | trace | trace | yes (hessonite, Gro-And) | yes (essential) |
| color-change from Cr/V | yes if x_Alm <= 0.10 (report) | **no** | only in Prp-Sps mix | yes (Ethiopia chrome grossular, rare; not in the report) | **no** (Fe3+ edge dominates; not reported) |

---

## 2. Garnet (pyralspite): pyrope, almandine, spessartine

### Overview

- Isotropic cubic; all `pol = { o = 1.0 }` (Report p.14).
- Pyrope is colorless when pure; color is from the almandine and spessartine components, plus trace Cr3+/V3+ (pink to red, color-change). Almandine is deep red through Fe2+ (X site) spin-forbidden bands at 504, 527, 576 nm superposed on a strong UV/blue edge; spessartine is orange-yellow from Mn2+ spin-forbidden bands at 409-486 nm that absorb blue-violet.
- Fe2+ absorptions at 4500, 6100, 7800 cm^-1 (S3, Report p.14) are in the NIR and outside the 380-780 nm window. They are not included as bands.
- color-change pyrope-spessartine (Report p.15): Cr3+ (571 nm) and V3+ (574 nm) absorb yellow, the Mn2+ ~420 nm band cuts off the deep violet; two transmission windows, blue-green 480-520 nm and red > 620 nm; blue-green in daylight, purple-red under incandescent light. Condition: x_Alm <= 0.10, x_Sps ~0.15-0.45, Cr2O3 + V2O3 ~0.5-1.5 wt%. See Discrepancies for the report's red-window claim.
- UV edge: pyrope, almandine, spessartine charge-transfer edge near 300-330 nm for gem material (edge shifts to ~350-400 nm with Fe3+/Fe2+). Not given in the report; value stays `estimate`.
- Fluorescence (Report pp.15, 31, 38): almandine, IVCT, spessartine and V3+ inert; Cr3+ pink pyrope "inert to weak red (690 nm)"; Fe2+ quenches by non-radiative cross-relaxation.

```toml
[[host]]
name = "Garnet (pyralspite)"
formula = "(Mg,Fe2+,Mn2+)3 (Al,Cr3+,V3+,Fe3+)2 Si3 O12"
optical = "isotropic"
uv_edge_nm = 320.0          # estimate (CT edge of gem-quality pyralspite; Fe3+ tail pushes to ~350-400); not given in the report
materials = ["Pyrope Garnet", "Almandine Garnet", "Spessartine Garnet"]
rules = [
  "site_sum: x_Prp + x_Alm + x_Sps (+ x_Grs + x_And + minor Kno) <= 1.0; reject sets with sum x > 1 (Report p.14)",
  "end_member concentration: C = n_site * x * rho / M_fu, n_site = 3 (X) or 2 (Y); alpha = peak_coeff * x, linear in site occupancy (Report p.14, p.46)",
  "ivct_product: Fe2+-Fe3+ IVCT scales with x_Alm * x_Fe3+(Y) (Report p.15; product rule p.30)",
  "color_change: Cr3+/V3+ color change requires x_Alm <= 0.10 (Report p.15, p.46); above 0.10 the Fe2+ bands at 504/527 nm fill the 480-520 nm window and suppress it",
  "color_change_window: x_Sps ~0.15-0.45 and Cr2O3 + V2O3 ~0.5-1.5 wt% (Report p.15)",
  "rhodolite: x_Prp 0.55-0.70, x_Alm 0.30-0.45, x_Sps <= 0.05 (Report p.15)",
]

[[host.chromophore]]
id = "Fe2+ (almandine)"
kind = "end_member"
elements = ["Fe"]
site = "X (dodecahedral, D2), Fe2+ 3 per f.u. at x=1"
conc_unit = "mol fraction end-member"
conc_typical = [0.0, 0.85]   # Report p.15 "x_Alm = 0.0-0.85" (prior 0.0-0.8, +6 %); rhodolite 0.30-0.45
conc_max = 1.0
color = "deep red to purplish red (bands at 504/527/576 nm, strong blue-green absorption)"
requires = []
excludes = []
quenched_by = ["self (Fe2+ cross-relaxation)"]   # Report p.31
confidence = "secondary"
sources = ["Report p.15 (Fe2+ (Almandine) row)", "Report p.38 (Pyralspite directory row)", "S2", "S3", "S5"]
[[host.chromophore.band]]
centre_nm = 504.0
fwhm_cm1 = 350.0
shape = "gaussian_energy"
peak_coeff = 15.0            # report: alpha(504) = 15.0 cm^-1 at x_Alm = 1.0 (25.9 mol/L); implied epsilon 15.0/25.9 = 0.58 M^-1 cm^-1
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.15 (Fe2+ (Almandine) row); 5T2 -> 3T1; prior: estimate for FWHM/coefficient, centre secondary (S2) - same numbers"
[[host.chromophore.band]]
centre_nm = 527.0
fwhm_cm1 = 400.0
shape = "gaussian_energy"
peak_coeff = 20.0            # implied epsilon 20.0/25.9 = 0.77 M^-1 cm^-1
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.15 (Fe2+ (Almandine) row); 5T2 -> 3T2"
[[host.chromophore.band]]
centre_nm = 576.0
fwhm_cm1 = 450.0
shape = "gaussian_energy"
peak_coeff = 25.0            # implied epsilon 25.0/25.9 = 0.97 M^-1 cm^-1 (strongest of the triplet)
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.15 (Fe2+ (Almandine) row); 5T2 -> 3A2"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
# emission_fwhm_nm: n/a (no emission)
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "Report p.38 (Pyralspite row: Inert, Fe-quenched); p.31"

[[host.chromophore]]
id = "Fe2+-Fe3+ IVCT"
kind = "ivct_pair"
elements = ["Fe"]
site = "Fe2+ at X (dodecahedral) <-> Fe3+ at Y (octahedral)"
conc_unit = "mol fraction end-member"   # use x_Alm * x_Fe3+(Y); x_Fe3+ typically 0.0-0.05
conc_typical = [0.0, 0.05]               # product x_Alm * x_Fe3+; Report p.15 "x_prod = 0.0-0.05"
conc_max = 1.0
color = "adds broad blue-violet absorption, darkens almandine"
requires = ["Fe2+ (almandine)"]
excludes = ["Cr3+ (pyrope)"]             # no enforced exclusion physically; keep Fe3+ small when Cr is present (EST)
quenched_by = []
confidence = "secondary"                 # prior: verified (S1: 21,700 cm^-1, FWHM ~7300) - same numbers in the report
sources = ["Report p.15 (IVCT pair row)", "Report p.38", "S1"]
[[host.chromophore.band]]
centre_nm = 461.0            # 21,700 cm^-1 (1e7/21700 = 460.8 nm)
fwhm_cm1 = 7300.0
shape = "gaussian_energy"
peak_coeff = 400.0            # cm^-1 per unit product x_Alm*x_Fe3+; report p.15. NB: report prose gives epsilon ~300-500 M^-1cm^-1; epsilon x 25.9 mol/L = 7,770-12,950 would be 19-32x larger -> see Discrepancies (400 kept, more plausible)
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.15 (centre, FWHM, epsilon 300-500 in text) and p.15 table (peak 400.0 cm^-1/(x_Alm * x_Fe3+)); prior: verified centre/FWHM (S1), estimate coefficient"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "Report p.38 (Pyralspite row: Inert)"

[[host.chromophore]]
id = "Mn2+ (spessartine)"
kind = "end_member"
elements = ["Mn"]
site = "X (dodecahedral), Mn2+ 3 per f.u. at x=1"
conc_unit = "mol fraction end-member"
conc_typical = [0.0, 0.95]   # Report p.15 "x_Sps = 0.0-0.95" (prior 0.0-0.9); CC pyrope-spessartine 0.15-0.45
conc_max = 1.0
color = "yellow-orange to orange-red (absorbs 400-490 nm)"
requires = []
excludes = []
quenched_by = ["Fe2+ (almandine) energy transfer (> ~0.1)"]   # EST, not in the report
confidence = "secondary"    # prior: verified (S2 centres)
sources = ["Report p.15 (Mn2+ (Spessartine) row)", "Report p.38", "S2", "S6"]
[[host.chromophore.band]]
centre_nm = 409.0
fwhm_cm1 = 450.0
shape = "gaussian_energy"
peak_coeff = 6.0              # report: 6.0 cm^-1/x at x_Sps = 1 (25.4 mol/L: 3 x 4190 / 495.0); implied epsilon 6.0/25.4 = 0.24
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.15 (Mn2+ (Spessartine) row); prior: verified centre (S2 excerpt), FWHM/coefficient estimate - same numbers"
[[host.chromophore.band]]
centre_nm = 422.0
fwhm_cm1 = 450.0
shape = "gaussian_energy"
peak_coeff = 8.0              # implied epsilon 8.0/25.4 = 0.31
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.15 (Mn2+ (Spessartine) row)"
[[host.chromophore.band]]
centre_nm = 430.0
fwhm_cm1 = 500.0
shape = "gaussian_energy"
peak_coeff = 12.0             # strongest 6A1 -> 4A1,4E(G); implied epsilon 12.0/25.4 = 0.47
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.15 (Mn2+ (Spessartine) row, 'intense band at 430 nm')"
[[host.chromophore.band]]
centre_nm = 486.0
fwhm_cm1 = 600.0
shape = "gaussian_energy"
peak_coeff = 8.0              # implied epsilon 0.31
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.15 (Mn2+ (Spessartine) row; 6A1 -> 4T2(G) as assigned by the report)"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "Report p.38 (Pyralspite row: Inert); Mn2+ concentration-quenched (secondary)"

[[host.chromophore]]
id = "Cr3+"
kind = "ion"
elements = ["Cr"]
site = "Y (octahedral Al site, C3i as labelled in the report), Cr3+"
conc_unit = "wt% Cr2O3"
conc_typical = [0.0, 1.5]    # Report p.15 "0.0-1.5 wt% Cr2O3"; pink/red gem pyrope 0.05-1; CC pyrope-spessartine Cr2O3+V2O3 0.5-1.5
conc_max = 5.0               # secondary: Cr-pyrope to ~5 wt% in mantle garnets (not in the report)
color = "pink to red (alone); blue-green/purple color change with V and Mn"
requires = []
excludes = ["Fe2+ (almandine) when x_Alm > 0.10 for color change"]   # Report p.15, p.46
quenched_by = []
confidence = "secondary"
sources = ["Report p.15 (Cr3+ row)", "Report p.38", "S6", "S7"]
[[host.chromophore.band]]
centre_nm = 571.0
fwhm_cm1 = 3000.0
shape = "gaussian_energy"
peak_coeff = 14.0             # report: 14.0 cm^-1 per wt%; check 1 wt% Cr2O3 in pyrope rho 3.58: 35.8 g/L / 151.99 x 2 = 0.471 mol/L Cr -> epsilon 29.7 M^-1cm^-1
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.15 (Cr3+ row; 4A2g -> 4T2g); prior: verified centre (S6), FWHM/coefficient estimate - same numbers"
[[host.chromophore.band]]
centre_nm = 410.0
fwhm_cm1 = 3500.0
shape = "gaussian_energy"
peak_coeff = 12.0             # epsilon 12.0/0.471 = 25.5
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.15 (Cr3+ row; 4A2g -> 4T1g(F)); prior: estimate - same numbers"
[[host.chromophore.fluorescence]]
excitation_nm = [410.0, 570.0]       # EST, not in the report
emission_nm = [690.0]                # Report p.38: 690
emission_fwhm_nm = 6.0               # EST (not in the report)
# quantum_yield: unknown
uv = { lw = "inert to weak red", sw = "inert to weak red" }
source = "Report p.38 (Pyralspite row: 'Inert to weak red (690)'); excitation/FWHM estimate; Fe2+ quenches"

[[host.chromophore]]
id = "V3+"
kind = "ion"
elements = ["V"]
site = "Y (octahedral Al site), V3+"
conc_unit = "wt% V2O3"
conc_typical = [0.0, 1.0]    # Report p.15-16 "0.0-1.0 wt% V2O3"
conc_max = 2.0                # EST (color-change pyrope-spessartine reported ~ up to ~1-2 wt%); not in the report
color = "blue-green/purple color change with Cr and Mn; pink in pyrope"
requires = []
excludes = ["Fe2+ (almandine) when x_Alm > 0.10 for color change"]   # Report p.15, p.46
quenched_by = []
confidence = "secondary"
sources = ["Report p.15-16 (V3+ row)", "Report p.38-39", "S6", "S7"]
[[host.chromophore.band]]
centre_nm = 574.0
fwhm_cm1 = 3000.0
shape = "gaussian_energy"
peak_coeff = 14.0             # 1 wt% V2O3 in pyrope: 35.8 g/L / 149.88 x 2 = 0.478 mol/L V -> epsilon 29.3
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.15-16 (V3+ row; 3T1g -> 3T2g(F) in the prior file); prior: estimate - same numbers"
[[host.chromophore.band]]
centre_nm = 405.0
fwhm_cm1 = 3500.0
shape = "gaussian_energy"
peak_coeff = 12.0             # epsilon 12.0/0.478 = 25.1
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.16 (V3+ row, second band); prior: estimate - same numbers"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "Report p.38 (Pyralspite row: Inert); V3+ is not a luminescence centre in garnet (secondary)"
```

---

## 3. Garnet (ugrandite): grossular (tsavorite, hessonite)

### Overview

- Isotropic (Report p.14: w_o = 1.0). Pure grossular is colorless.
- **Tsavorite** (Report p.16): green from octahedral V3+ (600 and 420 nm) with secondary Cr3+ (610 and 430 nm); green transmission window at 510-530 nm.
- **Hessonite** (Report p.16): 2-15 mol% andradite (x_And = 0.02-0.15), spin-forbidden Fe3+ band at 440 nm plus a steep charge-transfer edge (prose: "rising below 480 nm"; table: edge tail at 25,000 cm^-1 = 400 nm, 250 cm^-1/x_And), giving golden-yellow to cinnamon-brown.
- color-change chrome grossular exists (Ethiopia, S13) but is rare; not in the report.
- UV edge: ~280-290 nm for colorless grossular (estimate, not in the report); Fe3+ pushes the effective edge to 350-450 nm.
- Fluorescence: report lists V3+, Cr3+, Fe3+ rows as inert (p.39). Mn-bearing grossular weak orange SW is unverified.

```toml
[[host]]
name = "Garnet (grossular)"
formula = "Ca3 (Al,V3+,Cr3+,Fe3+)2 Si3 O12"
optical = "isotropic"
uv_edge_nm = 290.0          # estimate (not in the report)
materials = ["Grossular Garnet (Tsavorite)"]
rules = [
  "site_sum: x_Grs + x_And (+ pyralspite components) <= 1.0; X site is Ca (non-chromophoric), color from Y-site cations (Report p.14, p.16)",
  "end_member concentration: C = n_site * x * rho / M_fu with n_site = 2 for Y (Report p.14); alpha linear in x_And (Report p.46)",
  "no Cr3+/V3+ color change is modelled for grossular (rare Ethiopia chrome grossular, S13, not in the report)",
]

[[host.chromophore]]
id = "V3+"
kind = "ion"
elements = ["V"]
site = "Y (octahedral Al site)"
conc_unit = "wt% V2O3"
conc_typical = [0.1, 1.5]     # Report p.16 "0.1-1.5 wt% V2O3" (prior 0.0-1.0, EST)
conc_max = 2.0                # EST; no retrieved maximum
color = "green (with Cr3+); pale blue-green at low content"
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.16 (Grossular V3+ (Tsavorite) row)", "Report p.39 (Grossular directory row)", "S8"]
[[host.chromophore.band]]
centre_nm = 600.0
fwhm_cm1 = 2800.0
shape = "gaussian_energy"
peak_coeff = 13.0              # report 13.0 cm^-1 per wt%; check 1 wt% V2O3, rho 3.59: 35.9 g/L / 149.88 x 2 = 0.479 mol/L V -> epsilon 27.1 (the prior comment used 0.44 mol/L, an arithmetic slip; value unchanged)
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.16 (Grossular V3+ (Tsavorite) row, 600 nm); prior: estimate - same numbers"
[[host.chromophore.band]]
centre_nm = 420.0
fwhm_cm1 = 3000.0
shape = "gaussian_energy"
peak_coeff = 13.0              # epsilon 27.1
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.16 (Grossular V3+ (Tsavorite) row, 420 nm); prior: estimate - same numbers"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "Report p.39 (Grossular row: Inert)"

[[host.chromophore]]
id = "Cr3+"
kind = "ion"
elements = ["Cr"]
site = "Y (octahedral Al site)"
conc_unit = "wt% Cr2O3"
conc_typical = [0.05, 1.0]    # Report p.16 "0.05-1.0 wt% Cr2O3" (prior 0.0-0.5, EST)
conc_max = 1.5                 # EST (not in the report)
color = "green (with V3+); can make color-change with V in rare Ethiopia material"
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.16 (Grossular Cr3+ (Tsavorite) row)", "Report p.39", "S8", "S13"]
[[host.chromophore.band]]
centre_nm = 610.0
fwhm_cm1 = 2800.0
shape = "gaussian_energy"
peak_coeff = 13.0              # 1 wt% Cr2O3, rho 3.59: 35.9 / 151.99 x 2 = 0.472 mol/L Cr -> epsilon 27.5
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.16 (Grossular Cr3+ (Tsavorite) row, 610 nm); prior: estimate - same numbers"
[[host.chromophore.band]]
centre_nm = 430.0
fwhm_cm1 = 3000.0
shape = "gaussian_energy"
peak_coeff = 12.0              # epsilon 25.4
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.16 (Grossular Cr3+ (Tsavorite) row, 430 nm); prior: estimate - same numbers"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
# quantum_yield: unknown (report gives only the gem-lab response 'Inert'; Cr3+ 2E emission could exist but is not reported)
uv = { lw = "inert", sw = "inert" }
source = "Report p.39 (Grossular row: Inert)"

[[host.chromophore]]
id = "Fe3+ (andradite component)"
kind = "end_member"
elements = ["Fe"]
site = "Y (octahedral), Fe3+ 2 per f.u. at x_And = 1"
conc_unit = "mol fraction end-member"
conc_typical = [0.02, 0.15]   # Report p.16-17 "x_And = 0.02-0.15" (prior 0.0-0.15)
conc_max = 1.0
color = "orange-brown (hessonite), yellow-green (> ~0.3)"
requires = []
excludes = ["V3+ (tsavorite green is lost at high Fe3+)"]   # EST: V/Cr tsavorite is Fe-poor
quenched_by = []
confidence = "secondary"
sources = ["Report p.16-17 (Grossular Fe3+ (Hessonite) row)", "Report p.39", "S8 (440 nm Fe3+ band, andradite)"]
[[host.chromophore.band]]
centre_nm = 440.0
fwhm_cm1 = 1300.0
shape = "gaussian_energy"
peak_coeff = 60.0              # per mol fraction And; implied epsilon 60/15.2 = 3.9 M^-1cm^-1 (same as andradite)
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.16-17 (Grossular Fe3+ (Hessonite) row, 440 nm); prior: estimate - same numbers"
[[host.chromophore.band]]
centre_nm = 400.0              # report gives the edge tail as 25,000 cm^-1 = 1e7/25000 = 400 nm
# fwhm_cm1: unknown (report: "Edge")
shape = "edge"
peak_coeff = 250.0             # report: 250.0 cm^-1 per x_And
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.16-17 (Grossular Fe3+ (Hessonite) row: '440.0 Edge tail / 25,000 / Edge / 250.0 cm^-1/x_And'); NEW band (prior file had none); prose says the edge rises below 480 nm, table says 400 nm - see Discrepancies"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "Report p.39 (Grossular row: Inert); Fe3+ garnet non-fluorescent (Fe quenching, secondary)"

[[host.chromophore]]
id = "Mn2+ (grossular trace)"
kind = "ion"
elements = ["Mn"]
site = "X (Ca site)"
conc_unit = "wt% MnO"
conc_typical = [0.0, 1.0]
conc_max = 3.0                 # EST
color = "orange tint (hessonite, in combination with Fe3+)"
requires = []
excludes = []
quenched_by = ["Fe3+"]
confidence = "estimate"        # NOT in the report: unchanged
sources = ["EST: bands as 'Mn2+ (spessartine)' scaled by Mn concentration per wt%"]
[[host.chromophore.band]]
centre_nm = 430.0
fwhm_cm1 = 500.0
shape = "gaussian_energy"
peak_coeff = 1.0               # EST: 1 wt% MnO ~ 0.14 mol/L Mn x epsilon 7 - tiny
pol = { o = 1.0 }
confidence = "estimate"
source = "assumes same epsilon as in spessartine (Report p.15 / S2); site in Ca garnet shifts positions by an unknown amount; not in the report"
[[host.chromophore.fluorescence]]
excitation_nm = [254.0]
emission_nm = [590.0]
emission_fwhm_nm = 60.0
# quantum_yield: unknown
uv = { lw = "inert", sw = "weak orange (unverified)" }
source = "estimate: Mn2+ in Ca sites emits orange in other Ca silicates; not verified for grossular; not in the report"
```

---

## 4. Garnet (ugrandite): andradite (demantoid, topazolite)

### Overview

- Isotropic. Essential Fe3+ at Y; the O->Fe3+ charge-transfer edge makes the crystal opaque below 440 nm at x_And = 1 (Report p.16, 15.2 mol/L Fe3+), hence the high UV edge.
- Fe3+ spin-forbidden bands: 440 nm (6A1g -> 4A1,4E(G)) and 861 nm (6A1g -> 4T1g, NIR tail) (Report p.16; same positions as S8). 587 and 623 nm are the Cr3+ bands in the report; S8 had assigned them to Fe3+/Cr3+ without a split.
- **Demantoid**: Cr3+ (up to 0.15 apfu = ~2.2 wt% Cr2O3; check: 0.075 Cr2O3 x 151.99 / 508.2 = 2.24 wt%) supplies the 623/587 nm bands and, with the Fe3+ edge, a window at 520-550 nm; non-luminescent by concentration quenching on the Fe3+ sublattice (Report p.16). **Topazolite**: Fe3+-dominated, yellow to greenish-yellow, little Cr. V up to ~0.006 apfu (S8) is not in the report.
- UV edge: effective ~400 nm at 1 mm (estimate, kept); the report says "opaque below 440 nm" (see Discrepancies).

```toml
[[host]]
name = "Garnet (andradite)"
formula = "Ca3 (Fe3+,Cr3+,V3+)2 Si3 O12"
optical = "isotropic"
uv_edge_nm = 400.0          # estimate: O->Fe3+ CT edge, strong tail to ~500 nm; report p.16: 'opaque below 440 nm' (not adopted, thickness unspecified)
materials = ["Andradite Garnet (Demantoid)"]
rules = [
  "site_sum: x_And + x_Grs (+ pyralspite components) <= 1.0 (Report p.14)",
  "end_member concentration: C = n_site * x * rho / M_fu with n_site = 2 for Y; Fe3+ 15.2 mol/L at x_And = 1 (Report p.14, p.16)",
  "Cr3+ substitutes Fe3+ (up to 0.15 apfu ~ 2.2 wt% Cr2O3, Report p.16); no Cr/V color change in andradite (EST: Fe3+ edge dominates)",
  "demantoid non-luminescent: concentration quenching across the Fe3+ sublattice (Report p.16, p.39)",
]

[[host.chromophore]]
id = "Fe3+ (andradite)"
kind = "end_member"
elements = ["Fe"]
site = "Y (octahedral), Fe3+ 2 per f.u. at x = 1; some Fe3+ on tetrahedral Z in Ti-rich (melanite) is not modelled"
conc_unit = "mol fraction end-member"
conc_typical = [0.85, 1.0]    # Report p.17 "x_And = 0.85-1.0" (prior 0.85-0.99; S8: 1.72-1.85 Fe3+ apfu of 2 => 0.86-0.93)
conc_max = 1.0
color = "yellow-green to brownish yellow; topazolite yellow"
requires = []
excludes = []
quenched_by = ["self"]
confidence = "secondary"       # prior: verified (S8)
sources = ["Report p.17 (Andradite Fe3+ (Topazolite) row)", "Report p.39", "S8"]
[[host.chromophore.band]]
centre_nm = 440.0
fwhm_cm1 = 1300.0
shape = "gaussian_energy"
peak_coeff = 60.0              # report 60.0 per x_And; implied epsilon 60.0/15.2 = 3.9 M^-1cm^-1 (spin-forbidden, exchange-enhanced)
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.17 (Andradite Fe3+ (Topazolite) row, 440 nm); prior: verified centre (S8), FWHM/coefficient estimate - same numbers"
[[host.chromophore.band]]
centre_nm = 861.0
fwhm_cm1 = 2500.0
shape = "gaussian_energy"
peak_coeff = 25.0              # implied epsilon 25.0/15.2 = 1.6
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.17 (861 nm, 6A1g -> 4T1g); its tail enters 780 nm only; prior: verified centre (S8)"
[[host.chromophore.band]]
centre_nm = 380.0
fwhm_cm1 = 5000.0
shape = "gaussian_energy"
peak_coeff = 400.0             # check: at 440 nm this Gaussian gives 400 x exp(-ln2 (2 x 3589/5000)^2) = 96 cm^-1 per x, plus 60 -> opaque below ~440 nm in 1 mm, consistent with the report prose
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.17 (Andradite row '380.0 (CT)', FWHM 5000, 400.0 cm^-1/x); the report calls it a charge-transfer edge, so the Gaussian is a stand-in (prior: estimate - same numbers)"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "Report p.39 (Andradite row: Inert, Fe-quenched)"

[[host.chromophore]]
id = "Cr3+"
kind = "ion"
elements = ["Cr"]
site = "Y (octahedral); substitutes Fe3+"
conc_unit = "wt% Cr2O3"
conc_typical = [0.1, 2.2]    # Report p.17 "0.1-2.2 wt% Cr2O3" (prior 0.0-1.5)
conc_max = 2.5               # 0.15 apfu Cr x 151.99/2 / 508.2 = 2.24 wt%; margin kept
color = "vivid green (demantoid)"
requires = ["Fe3+ (andradite)"]
excludes = []
quenched_by = ["Fe3+"]
confidence = "secondary"       # prior: verified (S8)
sources = ["Report p.17 (Andradite Cr3+ (Demantoid) row)", "Report p.39", "S8"]
[[host.chromophore.band]]
centre_nm = 623.0
fwhm_cm1 = 2800.0
shape = "gaussian_energy"
peak_coeff = 12.0              # report 12.0 per wt%; 1 wt% Cr2O3, rho 3.85: 38.5 g/L / 151.99 x 2 = 0.507 mol/L Cr -> epsilon 23.7 (prior comment used 0.40 mol/L = slip; value unchanged)
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.17 (Andradite Cr3+ (Demantoid) row, 623 nm); prior: verified centre (S8), estimate coefficient - same numbers"
[[host.chromophore.band]]
centre_nm = 587.0
fwhm_cm1 = 2500.0
shape = "gaussian_energy"
peak_coeff = 6.0               # epsilon 11.8
pol = { o = 1.0 }
confidence = "secondary"
source = "Report p.17 (Andradite Cr3+ (Demantoid) row, 587 nm); prior: estimate (assignment ambiguity in S8, half weight) - same numbers"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "Report p.16 (demantoid non-luminescent by Fe3+ concentration quenching); p.39"

[[host.chromophore]]
id = "V3+"
kind = "ion"
elements = ["V"]
site = "Y (octahedral)"
conc_unit = "wt% V2O3"
conc_typical = [0.0, 0.1]
conc_max = 0.2               # S8: up to 0.006 apfu => ~ 0.1 wt% V2O3; margin
color = "negligible; minor green contribution"
requires = ["Fe3+ (andradite)"]
excludes = []
quenched_by = []
confidence = "estimate"        # NOT in the report: unchanged
sources = ["S8 (conc only)", "EST: bands set equal to V3+ in grossular, position shift unknown"]
[[host.chromophore.band]]
centre_nm = 610.0
fwhm_cm1 = 2800.0
shape = "gaussian_energy"
peak_coeff = 12.0
pol = { o = 1.0 }
confidence = "estimate"
source = "no V3+ band positions in andradite in the report or S8: value copied from grossular with a +10 nm shift guess"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "secondary"
```

---

## 5. Peridot (forsterite-olivine)

### Overview

- Biaxial (+), orthorhombic Pbnm (Report p.18). Gem Fo88-Fo93 (`x_Fa` 0.07-0.12); Mg2SiO4-Fe2SiO4 solid solution. Optical orientation and RI sets are not listed in the dataset (not requested); the renderer must supply them.
- color: Fe2+ in M1 (Ci) and M2 (Cs). Three spin-allowed components 870 nm (11,494 cm^-1, primarily M2), 1050 nm (9524 cm^-1, primarily M1) and 1250 nm (8000 cm^-1, M1); visible tails across 600-780 nm. Sharp spin-forbidden Fe2+ bands at 403, 450, 473, 490, 635 nm. With the steep UV CT tail below 400 nm they leave a window at 500-550 nm (yellow-green to olive-green).
- Polarisation (Report p.18, directory p.40; alpha, beta, gamma): 1050 nm {1.0, 0.8, 0.6}; 870 nm {0.6, 1.0, 0.8}; 1250 nm {1.0, 0.5, 0.5}; spin-forbidden bands {1.0, 1.0, 1.0}. The prior file had all weights 1.0 (unknown). The report's spin-forbidden weights look like a placeholder rather than a measurement.
- Ni2+ and Cr3+ occur in peridot but are not in the report and not offered.
- UV edge 370 nm by O -> Fe charge transfer (Report p.18).
- Fluorescence: inert in LW and SW (Fe2+ self-quenching, Report p.18, p.31, p.40).
- Linear scaling with x_Fa (Report p.46).

```toml
[[host]]
name = "Peridot (forsterite-olivine)"
formula = "(Mg,Fe2+)2 SiO4"
optical = "biaxial"
uv_edge_nm = 370.0          # Report p.18 (transmission cutoff 370 nm, O -> Fe CT); prior: estimate, same number
materials = ["Peridot"]
rules = [
  "alpha scales linearly with x_Fa (Report p.46); peak_coeff given per x_Fa; Fe2+ 2 per f.u. = ~45.2 mol/L per x_Fa (2 x 22.6 mol/L f.u. at Fo90)",
]

[[host.chromophore]]
id = "Fe2+ (fayalite component, M1/M2)"
kind = "end_member"
elements = ["Fe"]
site = "M1 (Ci) and M2 (Cs), octahedral, Fe2+ ; random-ish with slight M1 preference at gem Fo"
conc_unit = "mol fraction end-member"       # x_Fa; Fe2+ per f.u. = 2 * x_Fa
conc_typical = [0.07, 0.12]                 # Report p.18 Fo88-93 / Fa7-12 (prior 0.07-0.15)
conc_max = 1.0
color = "yellow-green to olive; more Fe = darker, yellower"
requires = []
excludes = []
quenched_by = ["self"]                       # Report p.18, p.31
confidence = "secondary"
sources = ["Report p.18-19 (Fe2+ spin-allowed / spin-forbidden rows)", "Report p.40 (Peridot directory row)", "S4", "S5"]
[[host.chromophore.band]]
centre_nm = 1050.0
fwhm_cm1 = 3000.0
shape = "gaussian_energy"
peak_coeff = 270.0         # report alpha(1050) = 270 cm^-1 per x_Fa; check: epsilon 6 M^-1cm^-1 x 45.2 mol/L = 271
pol = { a = 1.0, b = 0.8, g = 0.6 }     # prior: all 1.0
confidence = "secondary"
source = "Report p.18 (Fe2+ spin-allowed row; primarily M1), p.40; prior: centre verified (S4), coefficient estimate - same number; polarisation changed"
[[host.chromophore.band]]
centre_nm = 870.0
fwhm_cm1 = 2500.0
shape = "gaussian_energy"
peak_coeff = 190.0         # implied epsilon 190/45.2 = 4.2
pol = { a = 0.6, b = 1.0, g = 0.8 }
confidence = "secondary"
source = "Report p.18 (870 nm, primarily M2)"
[[host.chromophore.band]]
centre_nm = 1250.0
fwhm_cm1 = 3500.0
shape = "gaussian_energy"
peak_coeff = 190.0         # out of the 380-780 range, influences only via tail
pol = { a = 1.0, b = 0.5, g = 0.5 }
confidence = "secondary"
source = "Report p.18 (1250 nm, 8000 cm^-1, M1)"
[[host.chromophore.band]]
centre_nm = 450.0
fwhm_cm1 = 600.0
shape = "gaussian_energy"
peak_coeff = 4.0           # report alpha(450) = 4.0 per x_Fa; epsilon 4.0/45.2 = 0.09
pol = { a = 1.0, b = 1.0, g = 1.0 }
confidence = "secondary"
source = "Report p.18-19 (Fe2+ spin-forbidden row), p.40; prior: centre verified (S4)"
[[host.chromophore.band]]
centre_nm = 473.0
fwhm_cm1 = 600.0
shape = "gaussian_energy"
peak_coeff = 3.0           # epsilon 0.07
pol = { a = 1.0, b = 1.0, g = 1.0 }
confidence = "secondary"
source = "Report p.18-19 (spin-forbidden row; the table text wraps across pages, values read in order 450/473/490/403/635)"
[[host.chromophore.band]]
centre_nm = 490.0
fwhm_cm1 = 600.0
shape = "gaussian_energy"
peak_coeff = 3.0
pol = { a = 1.0, b = 1.0, g = 1.0 }
confidence = "secondary"
source = "Report p.18-19 (spin-forbidden row)"
[[host.chromophore.band]]
centre_nm = 403.0
fwhm_cm1 = 800.0
shape = "gaussian_energy"
peak_coeff = 3.0
pol = { a = 1.0, b = 1.0, g = 1.0 }
confidence = "secondary"
source = "Report p.19 (spin-forbidden row)"
[[host.chromophore.band]]
centre_nm = 635.0
fwhm_cm1 = 1000.0
shape = "gaussian_energy"
peak_coeff = 1.0           # epsilon 0.02
pol = { a = 1.0, b = 1.0, g = 1.0 }
confidence = "secondary"
source = "Report p.19 (spin-forbidden row)"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "Report p.18, p.40 (Inert, Fe-quenched)"

[[host.chromophore]]
id = "Fe3+ / O-Fe CT edge"
kind = "ion"
elements = ["Fe"]
site = "M sites; also Fe3+ in oxidised/heated (iddingsite) olivine"
conc_unit = "wt% Fe2O3"
conc_typical = [0.0, 0.3]
conc_max = 1.0
color = "olive/brownish tint; amplifies the UV cutoff"
requires = ["Fe2+ (fayalite component, M1/M2)"]
excludes = []
quenched_by = []
confidence = "estimate"        # NOT a separate chromophore in the report (it only mentions the 'steep UV charge-transfer tail below 400 nm', p.18)
sources = ["EST: no Fe3+ band data for olivine in the report or the earlier sources"]
[[host.chromophore.band]]
centre_nm = 380.0
fwhm_cm1 = 6000.0
shape = "gaussian_energy"
# peak_coeff: unknown (do not render until fitted)
pol = { a = 1.0, b = 1.0, g = 1.0 }
confidence = "estimate"
source = "placeholder for the UV CT tail; coefficient unknown"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "secondary"
```

Peridot coefficients are per `x_Fa` (report p.18, p.40): epsilon 6.0 / 4.2 for the spin-allowed NIR bands and 0.02-0.09 for the spin-forbidden visible bands, x 45.2 mol/L Fe per x_Fa. They are the same numbers as in the prior estimate (see conventions); polarisation weights are new in the report.

---

## 6. Zircon (natural, heat-treated, metamict)

### Overview

- Uniaxial (+), tetragonal I41/amd (Report p.19). colorless pure ZrSiO4; color is mainly **radiation damage + U4+ / REE**. UV edge ~300 nm (report, same as prior estimate).
- **U4+ sharp lines** (Report p.19): 5f-5f lines throughout the visible dominated by 653.5 nm and 690.0 nm; report gives Lorentzian FWHM 20 / 40 cm^-1 and coefficients 0.005 / 0.003 cm^-1 per ppm (basis of "ppm" not stated; U by weight is assumed). Polarisation {o 1.0, e 0.3}. Other U4+/REE lines are not listed in the report and are not invented here.
- **Natural blue / heat-treated blue** (Report p.19): heating brown Cambodian/Thai zircon to 900-1000 C in a reducing atmosphere gives an electron-trap centre associated with U4+: broad band at 650 nm, FWHM ~3500 cm^-1, onset ~500 nm, strongly dichroic {o 1.0, e 0.1}; oxidising conditions or UV exposure bleach it back to brown. `kind = "color_centre"`, treatment label "heat_reducing_900-1000C".
- **Brown centre** (Report p.19): alpha-recoil damage from U/Th gives a band at 485 nm (FWHM ~4500 cm^-1) and a continuum rising to 380 nm; annealing above 900 C repairs it. The prior file labelled the 485 nm band "LWUV_induced_reversible" (S9); the report attributes it to natural alpha-recoil damage. Both labels kept in the note, treatment label kept as in the prior file.
- **Metamict** damage (prior file; not in the report): relative damage parameter only.
- Fluorescence (Report p.19): low-damage zircon shows yellow to orange under 254 and 365 nm, emitting at 480 and 575 nm (Dy3+); quenched in high-U / metamict stones. U4+ is a quencher; blue and brown centres inert (p.40).

```toml
[[host]]
name = "Zircon"
formula = "(Zr,U4+,Th4+,REE3+,Hf) SiO4"
optical = "uniaxial"
uv_edge_nm = 300.0          # Report p.19 (fundamental absorption cutoff near 300 nm); prior: estimate, same number
materials = ["Zircon"]

[[host.chromophore]]
id = "U4+"
kind = "ion"
elements = ["U"]
site = "Zr (8-fold, D2d)"
conc_unit = "ppm U"
conc_typical = [100.0, 3000.0]     # estimate, no concentration in the report
conc_max = 20000.0                  # estimate
color = "sharp lines in red; contributes to blue-green/yellow tints"
requires = []
excludes = []
quenched_by = ["metamictisation (line broadening)"]
confidence = "secondary"
sources = ["Report p.19-20 (U4+ 5f-5f row)", "Report p.40 (Zircon directory row)", "S10 (lines)"]
[[host.chromophore.band]]
centre_nm = 653.5
fwhm_cm1 = 20.0
shape = "lorentzian"
peak_coeff = 0.005          # cm^-1 per ppm U (report); e.g. 1000 ppm -> 5 cm^-1, 3000 ppm -> 15 cm^-1. Prior: unknown (-1.0)
pol = { o = 1.0, e = 0.3 }
confidence = "secondary"
source = "Report p.19-20 (U4+ row, 653.5 nm; Lorentzian 20 cm^-1; 0.005 cm^-1/ppm; {1.0, 0.3}); prior: centre verified (S10), FWHM estimate"
[[host.chromophore.band]]
centre_nm = 690.0
fwhm_cm1 = 40.0
shape = "lorentzian"
peak_coeff = 0.003          # cm^-1 per ppm U; 1000 ppm -> 3 cm^-1
pol = { o = 1.0, e = 0.3 }
confidence = "secondary"
source = "Report p.19-20 (U4+ row, 690.0 nm; Lorentzian 40 cm^-1; 0.003 cm^-1/ppm; {1.0, 0.3}); prior: centre verified (S10)"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "Report p.19-20, p.40 (Inert (quencher))"

[[host.chromophore]]
id = "blue centre (heat-treated)"
kind = "color_centre"
elements = ["U"]
site = "radiation-damage electron-trap centre associated with U4+ (Report p.19)"
conc_unit = "relative centre density (0-1)"
conc_typical = [0.2, 1.0]
conc_max = 1.0
color = "sky blue, blue-green"
requires = ["U4+"]
excludes = ["brown centre (pre-heat)"]
quenched_by = ["oxidising heat treatment (centre bleaches)", "UV exposure (bleaches back to brown, Report p.19)"]
confidence = "secondary"       # prior: verified (S9, S10)
sources = ["Report p.19-20 (Blue Centre row)", "Report p.40", "S9", "S10"]
treatment = "heat_reducing_900-1000C"
[[host.chromophore.band]]
centre_nm = 650.0
fwhm_cm1 = 3500.0
shape = "gaussian_energy"
peak_coeff = 8.0              # cm^-1 per unit relative density (report: 8.0 cm^-1/unit); onset ~500 nm = 3.1 sigma from centre, consistent with FWHM 3500
pol = { o = 1.0, e = 0.1 }
confidence = "secondary"
source = "Report p.19-20 (Blue Centre row: 650 nm, FWHM 3500, 8.0 cm^-1/unit, {1.0, 0.1}); prior: verified centre/onset (S9), FWHM/coefficient/e-ray estimate - same numbers"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "Report p.20, p.40 (Inert)"

[[host.chromophore]]
id = "brown centre (natural / LWUV-induced)"
kind = "color_centre"
elements = ["U", "Th"]
site = "radiation-damage centre (alpha recoil damage, point-defect clusters)"
conc_unit = "relative centre density (0-1)"
conc_typical = [0.0, 1.0]
conc_max = 1.0
color = "brown to reddish brown"
requires = []
excludes = ["blue centre (heat-treated)"]
quenched_by = ["heating (bleaches at ~900-1000 C; annealing above 900 C repairs the lattice, Report p.19)"]
confidence = "secondary"       # prior: verified (S9, S10)
sources = ["Report p.19-20 (Brown Centre row)", "Report p.40", "S9", "S10"]
treatment = "LWUV_induced_reversible"   # prior label (S9); report calls the 485 nm band natural alpha-recoil damage
[[host.chromophore.band]]
centre_nm = 485.0
fwhm_cm1 = 4500.0
shape = "gaussian_energy"
peak_coeff = 8.0
pol = { o = 1.0, e = 0.5 }
confidence = "secondary"
source = "Report p.19-20 (Brown Centre row: 485 nm, FWHM 4500, 8.0 cm^-1/unit, {1.0, 0.5}); prior: verified centre (S9), others estimate - same numbers"
[[host.chromophore.band]]
centre_nm = 380.0
fwhm_cm1 = 6000.0
shape = "gaussian_energy"
peak_coeff = 20.0
pol = { o = 1.0, e = 0.5 }
confidence = "secondary"
source = "Report p.19-20 (Brown Centre row: 380 nm, FWHM 6000, 20.0 cm^-1/unit; report text 'continuum rising to 380 nm'); single stand-in Gaussian for the rising UV edge"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "Report p.20, p.40 (Inert)"

[[host.chromophore]]
id = "metamict damage"
kind = "color_centre"
elements = ["U", "Th"]
site = "lattice damage (alpha-recoil clusters); amorphous domains"
conc_unit = "relative damage fraction (0 crystalline - 1 fully metamict)"
conc_typical = [0.0, 0.5]
conc_max = 1.0
color = "dull brown / green-brown, resinous"
requires = ["U4+"]
excludes = []
quenched_by = ["thermal annealing at >~ 800-1000 C (partial recovery)"]
confidence = "estimate"        # NOT a separate chromophore in the report (only the Dy3+ quenching statement, p.19, p.31)
sources = ["S10 (Raman damage gauge)", "EST"]
treatment = "natural_metamict"
[[host.chromophore.band]]
centre_nm = 500.0
fwhm_cm1 = 6000.0
shape = "gaussian_energy"
# peak_coeff: unknown
pol = { o = 1.0, e = 1.0 }
confidence = "estimate"
source = "band shape and coefficient not available; the report folds damage absorption into the Brown Centre"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "Report p.19, p.31: metamict damage quenches Dy3+/REE emission"

[[host.chromophore]]
id = "Dy3+ / REE3+"
kind = "ion"
elements = ["Dy", "Sm", "Tb", "Nd"]    # report names only Dy3+; Sm/Tb/Nd from the prior file (EST) - see Discrepancies on the 580 nm line
site = "Zr (8-fold)"
conc_unit = "ppm REE2O3"
conc_typical = [10.0, 1000.0]
conc_max = 10000.0
color = "none visually (sharp lines); gives yellow fluorescence"
requires = []
excludes = []
quenched_by = ["metamictisation", "U/Th radiation"]     # Report p.19, p.31
confidence = "secondary"
sources = ["Report p.20 (Dy3+ row)", "Report p.40"]
[[host.chromophore.band]]
centre_nm = 580.0
fwhm_cm1 = 30.0
shape = "lorentzian"
# peak_coeff: unknown (report gives only the word "Trace")
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.20 (Dy3+ row: 580 nm, 17,241 cm^-1, FWHM 30, Lorentzian, 'Trace', {1.0, 1.0}); attribution to Dy3+ doubtful, see Discrepancies"
[[host.chromophore.fluorescence]]
excitation_nm = [254.0, 365.0]
emission_nm = [480.0, 575.0]
emission_fwhm_nm = 10.0     # EST (not in the report)
# quantum_yield: unknown
uv = { lw = "yellow to orange (crystalline, low U)", sw = "yellow to orange" }
source = "Report p.19-20 (254 and 365 nm excitation; 480/575 nm yellow, low-damage zircon only; quenched when metamict)"
```

---

## 7. Benitoite

### Overview

- Uniaxial (+), hexagonal (P-6c2), BaTiSi3O9 (Report p.20); Ti4+ is **essential** (30.6 wt% TiO2, edge 320 nm), so blue color needs trace Fe (Fe2+-Ti4+ IVCT). 
- Report p.20: IVCT band centred at 635 nm (FWHM ~4500 cm^-1), dipole parallel to c (w_e = 1.0, w_o = 0.05), peak 45.0 cm^-1 per wt% FeO. Transmission window in the blue (400-500 nm) for the e ray only; the o ray is nearly colorless. Check: 1 wt% FeO, rho ~3.65: 36.5 g/L / 71.84 = 0.508 mol/L Fe2+ -> implied epsilon 45.0 / 0.508 = 89 M^-1 cm^-1, at the low edge of the IVCT range in other Ti silicates (100-1300, S11), so plausible.
- Fluorescence (Report p.20-21, p.40-41): intrinsic ligand-to-metal CT in isolated [TiO6]8- clusters, excitation 254 nm, emission 415 nm (FWHM ~60 nm), bright chalky blue-white under SWUV, inert under LWUV. The report puts this on its own "intrinsic matrix" row; the Fe2+-Ti4+ row is inert. The earlier file had attached the emission to the IVCT chromophore.
- The Fe2+-Fe3+ IVCT block is a placeholder from the earlier file (S12 proposed); the report does not list it.

```toml
[[host]]
name = "Benitoite"
formula = "BaTiSi3O9"
optical = "uniaxial"
uv_edge_nm = 320.0          # Report p.20 (intrinsic band edge 320 nm); prior: estimate, same number
materials = ["Benitoite"]

[[host.chromophore]]
id = "Fe2+-Ti4+ IVCT"
kind = "ivct_pair"
elements = ["Fe", "Ti"]
site = "edge-shared Fe2+ / Ti4+ octahedra; Ti is essential"
conc_unit = "wt% FeO"
conc_typical = [0.0, 0.3]            # estimate: not in the report; at 0.3 wt% FeO the e-ray peak is 45 x 0.3 = 13.5 cm^-1
conc_max = 1.0                        # estimate
color = "blue to violet-blue (strongest e ray)"
requires = []
excludes = []
quenched_by = []
confidence = "secondary"              # prior: estimate (centre 620, coefficient unknown)
sources = ["Report p.20-21 (Fe2+-Ti4+ IVCT row)", "Report p.40-41", "S12", "S11"]
[[host.chromophore.band]]
centre_nm = 635.0                     # prior 620 (estimate)
fwhm_cm1 = 4500.0
shape = "gaussian_energy"
peak_coeff = 45.0                     # cm^-1 per wt% FeO (report); implied epsilon 89 M^-1cm^-1 (see overview). Prior: unknown
pol = { o = 0.05, e = 1.0 }
confidence = "secondary"
source = "Report p.20-21 (centre 635 nm, FWHM 4500, 45.0 cm^-1/wt% FeO, {0.05, 1.0}); prior polarisation S12 secondary - same"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "Report p.21, p.40 (Fe2+-Ti4+ row: Inert); emission moved to the intrinsic [TiO6] chromophore"

[[host.chromophore]]
id = "[TiO6]8- intrinsic (LMCT)"
kind = "intrinsic"
elements = ["Ti"]
site = "isolated [TiO6]8- octahedral clusters (host matrix)"
conc_unit = "matrix (not a dopant)"     # report: "Intrinsic matrix"; Ti is stoichiometric (30.6 wt% TiO2)
# conc_typical, conc_max: n/a (host matrix)
color = "none in the visible (UV absorption at 254 nm); drives the chalky blue SW fluorescence"
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.20-21 (Ligand-to-metal CT row)", "Report p.40-41"]
[[host.chromophore.band]]
centre_nm = 254.0                       # excitation band, outside the 380-780 nm window; 39,370 cm^-1
# fwhm_cm1: unknown (report: "-")
# shape: unknown
# peak_coeff: unknown (report: "Intrinsic matrix")
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.20-21 (Ligand-to-metal CT row: 254.0 nm (exc.), 39,370 cm^-1, {1.0, 1.0})"
[[host.chromophore.fluorescence]]
excitation_nm = [254.0]
emission_nm = [415.0]                   # prior 410 (estimate)
emission_fwhm_nm = 60.0                 # report p.20 prose "FWHM ~60 nm"
# quantum_yield: unknown
uv = { lw = "inert", sw = "intense chalky blue-white" }
source = "Report p.20 (prose), p.21 and p.41 (415 nm, bright chalky blue, SW only); S12"

[[host.chromophore]]
id = "Fe2+-Fe3+ IVCT (proposed)"
kind = "ivct_pair"
elements = ["Fe"]
site = "Fe2+/Fe3+ pair"
conc_unit = "wt% FeO"
conc_typical = [0.0, 0.3]
conc_max = 1.0
color = "contributes to blue/violet"
requires = []
excludes = []
quenched_by = []
confidence = "estimate"               # NOT in the report: unchanged
sources = ["S12"]
[[host.chromophore.band]]
centre_nm = 560.0
fwhm_cm1 = 4500.0
shape = "gaussian_energy"
# peak_coeff: unknown
pol = { o = 0.2, e = 1.0 }
confidence = "estimate"
source = "unknown; placeholder"
[[host.chromophore.fluorescence]]
excitation_nm = []
emission_nm = []
quantum_yield = 0.0
uv = { lw = "inert", sw = "inert" }
source = "secondary"
```

---

## Discrepancies with the report

Compared: prior file (the earlier session's `estimate`/`secondary`/`verified` values) vs the report, for the six hosts above. The report's directory (pp. 38-41) and its prose tables (pp. 15-21) agree with each other for every row checked. All wavenumber/nm pairs in the report were recomputed (1e7 / nm) and are consistent. Entries marked (kind) differ in kind, not in magnitude.

### A. Prior file vs report (> 10 % or in kind)

| # | Host / item | Prior file | Report | Which is more plausible |
|---|---|---|---|---|
| 1 | Zircon U4+ 653.5 / 690 nm `peak_coeff` (kind) | unknown (-1.0) | 0.005 / 0.003 cm^-1 per ppm (p.19-20, 40) | Report value is usable: 1000 ppm U gives 5 cm^-1, in the right range for gem zircon sharp lines (literature, from memory). Adopted, still `secondary`. |
| 2 | Benitoite IVCT `peak_coeff` (kind) | unknown (-1.0) | 45.0 cm^-1 per wt% FeO (p.20-21, 40) | Implied epsilon 89 M^-1cm^-1 (0.508 mol/L Fe per wt% FeO), just under the 100-1300 range of Fe2+-Ti4+ IVCT in other silicates (S11): plausible. Adopted. |
| 3 | Benitoite IVCT centre | 620 nm (estimate) | 635 nm (p.20) | 2.4 % apart, below the threshold; report adopted (blue window 400-500 nm e ray is consistent with either). |
| 4 | Benitoite fluorescence (kind) | emission 410 nm attached to the IVCT chromophore | emission 415 nm on a separate intrinsic [TiO6]8- LMCT row; IVCT row inert (p.20-21, 41) | Report's assignment is more plausible (emission is intrinsic, not Fe-related: Fe quenches). New chromophore added. |
| 5 | Peridot polarisation, 1050 / 870 / 1250 nm (kind) | all {1,1,1} (unknown) | {1.0,0.8,0.6} / {0.6,1.0,0.8} / {1.0,0.5,0.5} (alpha, beta, gamma; p.18, 40) | Anisotropy is physically required in a biaxial crystal (Burns: strongly polarised), so the report is better than all-1.0; the values themselves are unsourced. Spin-forbidden weights {1,1,1} remain a placeholder. |
| 6 | Peridot x_Fa range | 0.07-0.15 | 0.07-0.12 (Fo88-93, p.18) | Report (peridot gems are Fo88-93 in most literature; Fo85 is rare). Adopted. |
| 7 | Grossular Fe3+ edge band (kind) | none | edge tail at 25,000 cm^-1 (400 nm), 250 cm^-1 per x_And, FWHM "Edge" (p.16-17, 39) | Needed physically (hessonite brown comes from the CT edge). Added as `shape = "edge"`; see P2 for the internal inconsistency. |
| 8 | Grossular V3+ `conc_typical` | 0.0-1.0 wt% V2O3 | 0.1-1.5 (p.16) | Report (tsavorite V2O3 up to ~1-2 wt% is typical in the literature, from memory). Adopted. |
| 9 | Grossular Cr3+ `conc_typical` | 0.0-0.5 wt% Cr2O3 | 0.05-1.0 (p.16) | Report plausible; adopted. |
| 10 | Grossular Fe3+ `conc_typical` | 0.0-0.15 | 0.02-0.15 (p.16) | Lower bound is a definition of "hessonite" (2-15 mol% And); adopted. |
| 11 | Andradite Cr3+ `conc_typical` | 0.0-1.5 wt% | 0.1-2.2 (p.17) | 2.2 wt% is the S8 maximum (0.15 apfu), not a typical value; typical demantoid is <1 wt% in the literature (from memory). Adopted as listed, but `conc_max` 2.5 unchanged. |
| 12 | color-change window Cr2O3 + V2O3 | 0.2-1.5 wt% | 0.5-1.5 (p.15) | Report adopted; below ~0.5 wt% the 571 nm band is too weak to open a visible color change in a 0.5 cm stone (alpha = 14 x 0.3 = 4 cm^-1 -> 2 e-folds, borderline). |
| 13 | Zircon brown centre origin (kind) | 485 nm band labelled LWUV-induced reversible (S9) | 485 nm from natural alpha-recoil damage; blue centre bleached by oxidation or UV (p.19) | Both occur: pre-heat brown is the natural damage centre; S9 shows LWUV re-creates a 485 nm centre in blue stones. Treatment label kept, both noted. |
| 14 | Andradite UV edge | 400 nm (effective edge, 1 mm, estimate) | "opaque below 440 nm" at x_And = 1 (p.16) | Differ by 10 %; thickness is not stated by the report. The Gaussian CT stand-in reproduces 440 nm at about 150 cm^-1 per x, so both are consistent for ~1 mm. Prior kept. |
| 15 | Report items not in the prior file | - | Zircon Dy3+ emission 254 and 365 nm excitation (prior 254 only); Dy3+ 580 nm line has only "Trace" | Adopted both excitations; coefficient stays unknown. |

Below threshold and simply adopted: almandine `conc_typical` upper 0.8 vs 0.85 (6 %), spessartine 0.9 vs 0.95 (5 %), andradite Fe3+ upper 0.99 vs 1.0, IVCT epsilon 300 (prior) vs 300-500 (report).

All other bands (pyralspite 504/527/576/461/409/422/430/486/571/410/574/405; tsavorite 600/420/610/430; andradite 440/861/380/623/587; peridot FWHM and spin-forbidden coefficients; zircon 650/485/380 centres) are identical in the prior file and the report (see conventions: this is not independent confirmation).

### B. Items in the prior file that the report does not give (kept, confidence unchanged)

Grossular Mn2+ trace; andradite V3+; peridot Fe3+ CT-edge coefficient; zircon metamict damage band; benitoite Fe2+-Fe3+ IVCT; REE elements other than Dy; the host UV edges of the three garnets; Cr3+ in pyrope emission excitation and widths; prior concentration maxima.

### C. Physically implausible or doubtful report numbers

| # | Report statement | Problem |
|---|---|---|
| P1 | Pyralspite IVCT: peak 400.0 cm^-1 per x_Alm * x_Fe3+ together with epsilon ~300-500 M^-1cm^-1 and 25.9 mol/L Fe2+ (p.15, p.38) | Inconsistent by a factor of 19-32: epsilon x C = 300 x 25.9 = 7,770 (500 x 25.9 = 12,950) cm^-1 per unit product, versus 400. The 400 is the physically sensible magnitude (at x_prod = 0.01 it gives 4 cm^-1; 7,770 would give 78 cm^-1, i.e. an opaque 1 mm slab). Kept 400; the epsilon of 300-500 must refer to a per-pair, not per-site, convention. Unit/definition problem in the report. |
| P2 | Hessonite CT edge "rising below 480 nm" (p.16 text) vs edge tail at 25,000 cm^-1 = 400 nm (p.16-17 table) | 80 nm discrepancy inside the report. A Fe3+ CT edge at 400 nm with 440 nm band cannot produce the stated "cinnamon-brown" by itself; the 480 nm figure fits the visual better. FWHM is "Edge" (no number), so the shape cannot be rendered unambiguously. |
| P3 | color-change windows "blue-green 480-520 nm" and "red > 620 nm" (p.15) with Cr 571 nm band FWHM 3000 cm^-1 | A Gaussian of that width centred at 571 nm is still at 55 % of peak at 620 nm and 40 % at 520 nm (computed), so the stated windows are not supported by the report's own band parameters; real Cr3+/V3+ 4T2 bands are asymmetric with a steeper red side. A usable window needs >~ 650 nm. Treat the claimed transmission windows as qualitative. |
| P4 | Zircon Dy3+ "absorption" line at 580 nm, FWHM 30 cm^-1, "Trace" (p.20) | Dy3+ has no ground-state absorption near 17,240 cm^-1 (580 nm is the 4F9/2 -> 6H13/2 emission terminal, the report's own 575 nm emission). A sharp zircon absorption line at ~580 nm is characteristic of Nd3+ (4I9/2 -> 4G5/2, 2G7/2) (from memory, not in the report). Likely a mislabelled REE. |
| P5 | Mn2+ assignments (p.15): 409 and 422 nm both "6A1 -> 4E,4A1(G)", 430 nm "intense band", 486 nm "4T2(G)" | 4E,4A1(G) is a single, sharp band near 430-432 nm; 409/422 nm are other levels (4T2(D), 4E(D)); 486 nm corresponds to 4T1(G), with 4T2(G) near 525 nm (literature, from memory). Positions are fine (same as S2); the assignments are not. |
| P6 | Peridot spin-forbidden bands: all polarisation weights {1.0, 1.0, 1.0} (p.18-19) | In a biaxial crystal M1/M2 centres give polarised bands; equal weights are a placeholder, not data (same status as the prior file). Also the three NIR bands are assigned to two sites (M1: 1050, 1250; M2: 870) without the usual M1/M2 splitting, so site labels are not reliable. |
| P7 | Zircon U4+ "ppm" (p.19-20, 40) | Basis not stated (ppm U? ppm UO2?); the other directory rows use wt% oxide. I assumed ppm U by weight. A factor 1.13 (UO2/U) would apply for ppm UO2, below the accuracy of the number. No ppm-vs-wt% confusion found (garnet Cr2O3/V2O3 wt% and 0.15 apfu = 2.2 wt% check out). |
| P8 | Report coefficients are identical to the prior estimates | See conventions: the report quotes no source for any coefficient; the agreement is circular, not confirmation. |

Prior-file arithmetic slips found and fixed in comments (values unchanged): grossular V3+ concentration 0.44 -> 0.479 mol/L per wt% V2O3 (epsilon 27); andradite Cr3+ 0.40 -> 0.507 mol/L per wt% Cr2O3 (epsilon 24).

---

## Open questions

1. No Rossman Caltech spectra or RRUFF spectra were read; molar absorptivities (`peak_coeff`) are all estimates, and the report's agreement with them is circular. Fetch the polarised spectra of almandine, spessartine, tsavorite, demantoid, forsterite, zircon, benitoite and fit them.
2. IVCT definition in garnet: reconcile epsilon 300-500 M^-1cm^-1 with `peak_coeff` 400 per x_Alm * x_Fe3+ (P1).
3. Peridot: measured polarisation ratios of the 870/1050/1250 nm bands (alpha/beta/gamma) and of the spin-forbidden bands; the report's weights are unsourced.
4. Zircon: full U4+/REE line list with polarisation (the 580 nm line is probably Nd3+, P4); radiation-dose dependence of the brown centre; yellow/orange (oxidising) heat treatment; basis of the "ppm" unit.
5. Benitoite: confirm the 635 nm centre and the 45 cm^-1/wt% FeO coefficient against a polarised spectrum; Fe2+-Fe3+ IVCT placeholder still without band data; quantum yield of the 415 nm emission.
6. Hessonite CT edge: shape and FWHM ("Edge"), and the 400 vs 480 nm inconsistency (P2).
7. color-change pyrope-spessartine: verify the transmission windows (P3) and the x_Prp range (not in the report).
8. Garnet band shifts with composition (pyrope to almandine) are neglected.
9. Mn3+ (e.g. in Mn-rich grossular) and Ti3+/Fe-Ti IVCT in melanite/schorlomite are not modelled; V3+ in andradite, Mn2+ in grossular are not in the report.

## Coverage table

| Host | Chromophores | Bands | Report-sourced (secondary) | Coefficients | UV edge | Fluorescence |
|---|---|---|---|---|---|---|
| Garnet (pyralspite) | Fe2+, Fe2+-Fe3+, Mn2+, Cr3+, V3+ | 12 | 12 of 12 (all prior `verified` centres now `secondary`) | report (circular with estimate) | estimate (not in report) | inert; Cr3+ weak red 690 (report) |
| Garnet (grossular) | V3+, Cr3+, Fe3+, Mn2+ | 7 | 6 of 7 (Fe3+ edge new; Mn2+ not in report) | report; Mn2+ estimate | estimate | inert (report) |
| Garnet (andradite) | Fe3+, Cr3+, V3+ | 6 | 5 of 6 (V3+ not in report) | report; V3+ estimate | estimate (report: opaque < 440 nm) | inert (report) |
| Peridot | Fe2+, Fe3+ CT edge | 9 | 8 of 9 (Fe3+ edge band: coefficient unknown) | report; edge unknown | report (370 nm) | inert (report) |
| Zircon | U4+, blue/brown/metamict centres, Dy3+ | 7 | 6 of 7 (metamict band estimate; Dy3+ and metamict coefficients unknown) | report; 2 unknown | report (300 nm) | Dy3+ yellow 480/575 (report) |
| Benitoite | Fe2+-Ti4+, [TiO6] LMCT, Fe2+-Fe3+ | 3 | 2 of 3 (LMCT band only a centre) | IVCT 45 (report); LMCT and Fe-Fe unknown | report (320 nm) | SW chalky blue 415 (report) |
