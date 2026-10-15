# Chromophore dataset: beryl, topaz, tourmaline, andalusite, cordierite

Status of this revision (2026-10-03): merged with the user-supplied report "Gem Chromophore Spectroscopy
Verification" (48 pp., cited below as "Report p.N"; page numbers are PDF pages). Prior content came from
web-search digests and abstracts only; no primary paper was read.

Confidence levels in this file:
- `secondary`: value taken from the report (the report does not attribute single numbers to single references,
  so it is not `verified`), or from an authoritative abstract/digest in the prior revision where the report is silent.
- `estimate`: inferred by me; the reason is given in a comment.
- Unknown values are ABSENT (field deleted) and carry a `# unknown` comment. There is no `-1.0` / `0.0` sentinel any more.
  Renderer: a band without `peak_coeff` or `fwhm_cm1` cannot be rendered and must be skipped or calibrated
  from a measured spectrum (RRUFF / Caltech server).
- `pol` weights are relative (1.0 = strongest ray). Uniaxial: `{ o, e }`; biaxial: `{ a, b, g }` = alpha, beta, gamma.
- Centres given as nm, cm-1 = 1e7/nm. All report wavenumbers for these hosts were re-derived and agree with
  1e7/nm to within rounding, so no unit conversion was needed. The report FWHM is already in cm-1.
- `peak_coeff` is cm-1 per `conc_unit`. The report states only "cm-1/wt%" (oxide not named; the beryl table header
  carries a stray "wt% FeO" from the previous page), "cm-1/ppm" (topaz Cr; wt vs atoms basis not stated) or
  "cm-1/unit" (color centres; "unit" undefined). I keep this file's `conc_unit` and assume the report's wt% is that oxide.
- Band `shape` is not given by the report; it is retained from the prior revision (lorentzian for narrow
  spin-forbidden/R-lines, gaussian_energy for broad bands). New bands get a shape by the same rule.
- `fluorescence` blocks gain an optional `confidence` field (new in this revision; the report gives a luminescence
  column per row). Excitation wavelengths the report does not give stay as prior estimates (comment says so).
- Per-wt% extinction cross-check used below: epsilon = alpha / (2.303 * c), c in mol/L of the absorbing ion.
  Report's own ranges (p.2): spin-allowed d-d 1-50, spin-forbidden 0.05-2, IVCT 100-2000 L/mol/cm.

Key references (abbreviated in sources):
- [Report] "Gem Chromophore Spectroscopy Verification", 48 pp., user-supplied; 23 references, mostly GIA Gems & Gemology (pp. 47-48).
- [WN68] Wood & Nassau 1968, Am. Mineral. 53, 777-800, https://msaweb.org/AmMin/AM53/AM53_777.pdf
- [GR78] Goldman, Rossman, Parkin, Chaudhuri, Gorton 1978 "Channel constituents in beryl", Phys. Chem. Minerals 3(3) (Fe in beryl; found via search digest)
- [Andersson] Andersson 1979 (EPR, Maxixe vs Maxixe-type); Maxixe polarized spectra via GIA G&G Summer 2018 GNI https://www.gia.edu/gems-gemology/summer-2018-gemnews-green-blue-maxixe-type-beryl and Phys. Chem. Minerals 2009 https://link.springer.com/article/10.1007/s00269-009-0345-8
- [RedBeryl] Jahn-Teller distortion of Mn3+ in red beryl from Utah, optical spectroscopy (Univ. Vienna; Wah Wah Mts.) https://fgga.univie.ac.at/en/research/publications/publications-detail-view/pure/d94289e7-bbfd-46f6-8f24-57dbc95acc84/show/publ/Pure ; Shigley & Foord 1984 G&G 20(4) https://www.gia.edu/gems-gemology/winter-1984-beryl-utah-shigley
- [Cu-Tour] Rossman, Fritsch et al. 1990-91 Paraiba spectra; Ertl et al. 2012 Mineral. Petrol. https://link.springer.com/doi/10.1007/s00710-012-0234-6 ; GIA "Spectral Differentiation Between Copper and Iron Colorants" https://www.gia.edu/doc/Spectral-Differentiation-Between-Copper-and-Iron-Colorants.pdf ; Mineral. Mag. 54, 553 https://rruff.info/doclib/MinMag/Volume_54/54-377-553.htm
- [Tour-Fe] Mattson & Rossman 1987 "Fe2+-Fe3+ interactions in tourmaline" https://resolver.caltech.edu/CaltechAUTHORS:20140306-085514791 ; Mattson & Rossman 1988 (Mn tourmalines, Mn/Fe-Ti IVCT) Phys. Chem. Minerals; Caltech thesis https://thesis.caltech.edu/1307/
- [AndV] Carlson & Rossman 1988? "Vanadium- and chromium-bearing andalusite: occurrence and optical-absorption spectroscopy" Am. Mineral. 73 (via https://resolver.caltech.edu/CaltechAUTHORS:20130515-082554444; year to be confirmed)
- [AndMn] Abs-Wurmbach et al. / Langer, "manganian andalusite" polarized spectra (https://rruff.info/uploads/CM10_47.pdf and Can. Mineral. refs found by search; author/year to be confirmed)
- [TopazO] O- centre blue topaz: Priest, Wyon, Pontuer-type EPR/optical work; summary via Springer https://link.springer.com/doi/10.1007/s00269-005-0042-1 and RSC Adv. 2025 https://pubs.rsc.org/en/content/articlehtml/2025/ra/d5ra02029h (author-year details to be confirmed)
- [TopazCr] Imperial topaz Cr3+/Cr4+ and heat-induced 530 nm pink: RSC Adv. 2025 (above) and Am. Mineral. 60, 705 (https://msaweb.org/AmMin/AM60/AM60_705.pdf)
- [Cordierite-ref] Report reference 19: "Optical spectroscopic study of tuhualite and a re-examination of the beryl, cordierite and osumilite spectra" (title only; the report prints no cordierite numbers).

---

## 1. Beryl (Be3Al2Si6O18, hexagonal, uniaxial negative)

Overview. color comes from Cr3+ and V3+ on the octahedral Al site (emerald), Fe in several
sites (aquamarine blue by Fe2+-Fe3+ IVCT band near 680 nm plus Fe2+ near 835 nm; heliodor/golden
by Fe3+ with UV/blue tail, 370 and 426 nm lines), Mn2+/Mn3+ (morganite pink, red beryl red; Mn3+ band 567 nm in morganite, 560 nm and 1391 nm (7190 cm-1) in red beryl), and radiation color centres (Maxixe NO3 / CO3-, deep blue, bleaches in sunlight or above about 150 C per the report).
Goshenite: colorless; no chromophore entry (host only). Cr3+ and V3+ emerald differ in band positions (V3+ about 429 and 617 nm).
Non-additivity: Fe quenches Cr3+ R-line luminescence (Report p.21; Report p.31: above 0.5-1.0 wt% FeO R-line emission is completely quenched). Red beryl requires Mn3+ with very low Fe; Fe-rich red beryl does not occur (prior digest, not in report).
Maxixe centre is unstable (fades); color centre entries must carry a treatment flag and should not combine with strong Fe2+-Fe3+ blue in a stable gem.
Emerald Cr + V coexist commonly (Colombia, Zambia, Brazil), so allow Cr+V together.
Report p.22 table is a verbatim match of the directory rows on p.41-42; both are cited as one source.

```toml
[[host]]
name = "Beryl"
formula = "Be3Al2Si6O18"
optical = "uniaxial"
uv_edge_nm = 300.0   # Report p.21: "host UV transmission edge is established at 300 nm" (prior estimate had the same value)
materials = ["Emerald", "Aquamarine", "Morganite", "Heliodor", "Goshenite", "Red Beryl", "Maxixe"]

[[host.chromophore]]
id = "Cr3+"
kind = "ion"
elements = ["Cr"]
site = "Al octahedral (Cr3+, d3; D3)"
conc_unit = "wt% Cr2O3"
conc_typical = [0.05, 0.5]   # estimate: emerald literature range; not in report
conc_max = 2.0               # estimate: rare synthetic/Chivor-type extremes; unverified
color = "Green to bluish green; 600 nm 4T2 and 430 nm 4T1 bands leave transmission window near 500-550 nm and deep red; hue shifts yellower with V and with Fe3+."
requires = []
excludes = []
quenched_by = ["Fe2+", "Fe3+"]   # luminescence quench only; Report p.21 names Fe2+ energy transfer, Report p.31 gives threshold 0.5-1.0 wt% FeO for complete R-line quenching
confidence = "secondary"
sources = ["Report p.22 (Beryl table, Emerald Cr3+ row; directory p.41)", "[WN68] Am. Mineral. 53:777 https://msaweb.org/AmMin/AM53/AM53_777.pdf (bands 4300 A, 6000 A, sharp 4760, 6800, 6830 A per abstract digest)"]
# cross-check: 1 wt% Cr2O3, rho 2.70 g/cm3: 2.70*0.01/151.99*2 = 3.55e-4 mol Cr/cm3 = 0.355 mol/L; eps(600) = 18.0/(2.303*0.355) = 22 L/mol/cm, in the 1-50 spin-allowed range: plausible
[[host.chromophore.band]]
centre_nm = 430.0   # 23,256 cm-1 (1e7/430 = 23,256, matches report)
fwhm_cm1 = 3200.0
shape = "gaussian_energy"
peak_coeff = 16.0   # cm-1 per wt% Cr2O3; eps = 16.0/(2.303*0.355) = 19.6
pol = { o = 1.0, e = 0.60 }   # prior estimate was 0.6 (same)
confidence = "secondary"
source = "Report p.22 (Beryl table, Emerald Cr3+ row, 430 nm)"
[[host.chromophore.band]]
centre_nm = 600.0   # 16,667 cm-1
fwhm_cm1 = 2800.0
shape = "gaussian_energy"
peak_coeff = 18.0   # cm-1 per wt% Cr2O3; eps = 22
pol = { o = 1.0, e = 0.55 }   # prior estimate 0.5 (10 %, within tolerance); e-ray 4T2 is at longer wavelength than o-ray in reality, not modelled
confidence = "secondary"
source = "Report p.22 (Beryl table, Emerald Cr3+ row, 600 nm)"
[[host.chromophore.band]]
centre_nm = 476.0   # 21,008 cm-1; report labels it 2T1, which looks wrong, see Discrepancies
fwhm_cm1 = 300.0
shape = "lorentzian"
peak_coeff = 1.5    # cm-1 per wt% Cr2O3; eps = 1.5/(2.303*0.355) = 1.8, inside the spin-forbidden range 0.05-2
pol = { o = 1.0, e = 0.0 }
confidence = "secondary"
source = "Report p.22 (Beryl table, Emerald Cr3+ row, 476 nm); sharp 4760 A line also in [WN68]"
[[host.chromophore.band]]
centre_nm = 683.0   # 14,641 cm-1; R-line (2E), 680 nm companion of the doublet is in fluorescence.emission_nm
fwhm_cm1 = 20.0
shape = "lorentzian"
# peak_coeff: unknown (report table shows "-" for the R-line)
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.22 (Beryl table, Emerald Cr3+ row, 683 (R))"
[[host.chromophore.fluorescence]]
excitation_nm = [365.0]   # Report p.21: UV excitation at 365 nm; prior estimate was [430.0, 600.0] (absorption bands)
emission_nm = [680.0, 683.0]
# emission_fwhm_nm: unknown
# quantum_yield: unknown
uv = { lw = "red (R-lines), quenched by Fe", sw = "red (R-lines), quenched by Fe" }   # Report p.22 gives one luminescence entry (683.0, 680.0 nm, red, quenched by Fe), no LW/SW split; prior estimate was "weak to moderate red, absent in Fe-rich stones" / "weak red / inert"
confidence = "secondary"
source = "Report p.21-22 (emerald bullet and table luminescence column); intense in Fe-poor emerald"

[[host.chromophore]]
id = "V3+"
kind = "ion"
elements = ["V"]
site = "Al octahedral (V3+, d2)"
conc_unit = "wt% V2O3"
conc_typical = [0.05, 0.5]   # estimate
conc_max = 1.0               # estimate
color = "Green, slightly bluish-yellowish green; Cr-free emerald (WN68 abstract)."
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.22 (Beryl table, Emerald V3+ row; directory p.41)", "[WN68] (V3+ octahedral in emerald)"]
# cross-check: 1 wt% V2O3, rho 2.70, M 149.88: 2.70*0.01/149.88*2 = 3.60e-4 mol/cm3 = 0.360 mol/L; eps(617) = 16.0/(2.303*0.360) = 19.3, plausible
[[host.chromophore.band]]
centre_nm = 429.0   # 23,310 cm-1
fwhm_cm1 = 3200.0
shape = "gaussian_energy"
peak_coeff = 15.0   # cm-1 per wt% V2O3; eps = 18.1
pol = { o = 1.0, e = 0.60 }
confidence = "secondary"
source = "Report p.22 (Beryl table, Emerald V3+ row, 429 nm)"
[[host.chromophore.band]]
centre_nm = 617.0   # 16,207 cm-1
fwhm_cm1 = 2800.0
shape = "gaussian_energy"
peak_coeff = 16.0   # cm-1 per wt% V2O3; eps = 19.3
pol = { o = 1.0, e = 0.50 }
confidence = "secondary"
source = "Report p.22 (Beryl table, Emerald V3+ row, 617 nm)"
[[host.chromophore.fluorescence]]
# excitation_nm: unknown
# emission_nm: unknown
uv = { lw = "inert", sw = "inert" }   # report says "Inert" with no LW/SW split
confidence = "secondary"
source = "Report p.22 (Beryl table, Emerald V3+ row, luminescence 'Inert')"

[[host.chromophore]]
id = "Fe2+-Fe3+ IVCT"
kind = "ivct_pair"
elements = ["Fe", "Fe"]
site = "Fe2+ and Fe3+ in adjacent octahedral / channel sites (exact pair debated)"
conc_unit = "wt% FeO total"
conc_typical = [0.1, 1.5]    # estimate
conc_max = 3.0               # estimate
color = "Blue (aquamarine). Report: E||c band at 680 nm. Greenish when Fe3+ 427 nm tail adds. Direction of dichroism in the report looks reversed, see Discrepancies."
requires = []
excludes = ["Maxixe centre (practically not co-occurring)"]
quenched_by = []
confidence = "secondary"
sources = ["Report p.22 (Beryl table, Aquamarine Fe2+-Fe3+ IVCT row; directory p.41)", "Goldman et al. 1978 Phys. Chem. Minerals 3(3); Fe2+ band ~835 nm and Fe3+ 370, 426 nm (https://link.springer.com/article/10.1007/s00269-024-01285-6, to be confirmed)"]
# cross-check: 1 wt% FeO, rho 2.70, M 71.84: 3.76e-4 mol/cm3 = 0.376 mol/L; eps(680) = 12.0/(2.303*0.376) = 13.9 (below the report's own IVCT range 100-2000, but IVCT scales with Fe2+*Fe3+ pairs, not total FeO)
# NOTE: the 370 and 426 nm bands that the prior revision kept in this block are Fe3+ bands; the report lists them under heliodor, so they moved to "Fe3+ (heliodor)".
[[host.chromophore.band]]
centre_nm = 680.0   # 14,706 cm-1
fwhm_cm1 = 3500.0
shape = "gaussian_energy"
peak_coeff = 12.0   # cm-1 per wt% FeO total
pol = { o = 0.2, e = 1.0 }   # prior estimate identical; Report p.21 "strongly polarized parallel to c"
confidence = "secondary"
source = "Report p.22 (Beryl table, Aquamarine row, 680 nm)"
[[host.chromophore.band]]
centre_nm = 835.0   # 11,976 cm-1; isolated octahedral Fe2+ (E perp c)
fwhm_cm1 = 4000.0
shape = "gaussian_energy"
peak_coeff = 10.0   # cm-1 per wt% FeO total; eps = 10.0/(2.303*0.376) = 11.5
pol = { o = 1.0, e = 0.3 }
confidence = "secondary"
source = "Report p.22 (Beryl table, Aquamarine row, 835 nm (Fe2+))"
[[host.chromophore.fluorescence]]
# excitation_nm: unknown
# emission_nm: unknown
uv = { lw = "inert", sw = "inert" }   # report: "Inert (Fe-quenched)"
confidence = "secondary"
source = "Report p.22 (Beryl table, Aquamarine row, luminescence)"

[[host.chromophore]]
id = "Fe3+ (heliodor)"
kind = "ion"
elements = ["Fe"]
site = "Al octahedral and/or channel; debated"
conc_unit = "wt% Fe2O3"
conc_typical = [0.1, 1.0]    # estimate
conc_max = 2.0               # estimate
color = "Yellow to golden-greenish yellow from UV/blue absorption tail of 370/426 nm bands and CT edge below 400 nm; blue appears after reduction/irradiation."
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.21-22 (Heliodor bullet; Beryl table, Heliodor Fe3+ row; directory p.41)"]
# cross-check: 1 wt% Fe2O3, rho 2.70, M 159.69: 3.38e-4 mol Fe/cm3 = 0.338 mol/L; eps(370) = 15.0/(2.303*0.338) = 19.3, eps(426) = 8.0/(2.303*0.338) = 10.3. Both exceed the report's own spin-forbidden range 0.05-2 by 5-10x, see Discrepancies.
[[host.chromophore.band]]
centre_nm = 370.0   # 27,027 cm-1
fwhm_cm1 = 800.0
shape = "lorentzian"
peak_coeff = 15.0   # cm-1 per wt% Fe2O3
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.22 (Beryl table, Heliodor Fe3+ row, 370 nm)"
[[host.chromophore.band]]
centre_nm = 426.0   # 23,474 cm-1 (added: prior revision had it under the IVCT chromophore)
fwhm_cm1 = 600.0
shape = "lorentzian"
peak_coeff = 8.0    # cm-1 per wt% Fe2O3
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.22 (Beryl table, Heliodor Fe3+ row, 426 nm)"
[[host.chromophore.fluorescence]]
# excitation_nm: unknown
# emission_nm: unknown
uv = { lw = "inert", sw = "inert" }
confidence = "secondary"
source = "Report p.22 (Beryl table, Heliodor row, luminescence 'Inert')"

[[host.chromophore]]
id = "Mn3+ (red beryl)"
kind = "ion"
elements = ["Mn"]
site = "Al octahedral, Jahn-Teller elongated (report: severely distorted)"
conc_unit = "wt% MnO"
# conc_typical: unknown (report says only "high concentrations")
# conc_max: unknown
color = "Red / raspberry; Utah Wah Wah Mts. material."
requires = []
excludes = ["Fe2+-Fe3+ IVCT (high Fe absent in red beryl)"]   # prior digest, not in report
quenched_by = []
confidence = "secondary"
sources = ["Report p.21-22 (Red beryl bullet; Beryl table, Red Beryl Mn3+ row; directory p.41-42)", "[RedBeryl] Mn3+ UV-VIS-NIR, EPR, PL; broad NIR band ~7190 cm-1; Shigley & Foord 1984 G&G 20(4)"]
# cross-check: 1 wt% MnO, rho 2.70, M 70.94: 3.81e-4 mol/cm3 = 0.381 mol/L; eps(560) = 45.0/(2.303*0.381) = 51, eps(1391) = 30.0/(2.303*0.381) = 34 (a little above the 1-50 range; Jahn-Teller distorted site, acceptable)
[[host.chromophore.band]]
centre_nm = 1391.0   # 7190 cm-1; 1e7/1391 = 7189, matches report
fwhm_cm1 = 2500.0
shape = "gaussian_energy"
peak_coeff = 30.0    # cm-1 per wt% MnO
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.22 (Beryl table, Red Beryl row, 1391 nm (NIR))"
[[host.chromophore.band]]
centre_nm = 560.0   # 17,857 cm-1; was an estimate in the prior revision (expected 500-570 nm), now confirmed by the report
fwhm_cm1 = 3500.0
shape = "gaussian_energy"
peak_coeff = 45.0   # cm-1 per wt% MnO
pol = { o = 1.0, e = 0.50 }   # prior estimate identical
confidence = "secondary"
source = "Report p.22 (Beryl table, Red Beryl row, 560 nm; 5Eg -> 5T2g)"
[[host.chromophore.fluorescence]]
# excitation_nm: unknown
# emission_nm: unknown
uv = { lw = "inert", sw = "inert" }   # prior: unknown
confidence = "secondary"
source = "Report p.22 (Beryl table, Red Beryl row, luminescence 'Inert')"

[[host.chromophore]]
id = "Mn2+/Mn3+ (morganite)"
kind = "ion"
elements = ["Mn"]
site = "Al octahedral (Mn3+, d4), Mn2+ possible"
conc_unit = "wt% MnO"
# conc_typical: unknown
# conc_max: unknown
color = "Pink to peach (Mn3+ 567 nm gives pink; Fe3+ adds orange); heating/irradiation changes Mn valence."
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.21-22 (Morganite bullet; Beryl table, Morganite Mn3+ row; directory p.41)", "Digest: morganite Mn2+ and Mn3+ spin-allowed bands 450-600 nm; irradiation of morganite https://www.igc-gemmology.org/igc-2023/irradiation-treatment-and-gamma-spectroscopy-of-morganite/"]
# cross-check: eps(567) = 25.0/(2.303*0.381) = 28.5 L/mol/cm, plausible
[[host.chromophore.band]]
centre_nm = 567.0   # 17,637 cm-1
fwhm_cm1 = 3200.0
shape = "gaussian_energy"
peak_coeff = 25.0   # cm-1 per wt% MnO
pol = { o = 1.0, e = 0.35 }   # prior estimate 0.3 (17 % lower), see Discrepancies
confidence = "secondary"
source = "Report p.22 (Beryl table, Morganite Mn3+ row, 567 nm)"
[[host.chromophore.fluorescence]]
# excitation_nm: unknown
# emission_nm: unknown
uv = { lw = "inert", sw = "inert" }   # prior estimate was "inert to weak"
confidence = "secondary"
source = "Report p.22 (Beryl table, Morganite row, luminescence 'Inert')"

[[host.chromophore]]
id = "Maxixe centre (NO3 / CO3-)"
kind = "color_centre"
elements = []
treatment = "irradiation"
site = "channel radical trap: NO3 (natural Maxixe) or CO3- (irradiated Maxixe-type)"
conc_unit = "relative centre density"
# conc_typical: unknown
# conc_max: unknown
color = "Deep (cobalt) blue (also green-blue, violet seen in recent GNI); fades in sunlight / heat."
requires = []
excludes = ["Fe2+-Fe3+ IVCT as stable co-chromophore (not documented)"]
quenched_by = ["sunlight", "heat above 150 C (Report p.21-22; prior estimate ~200 C)"]
confidence = "secondary"
sources = ["Report p.21-22 (Maxixe bullet; Beryl table, Maxixe row; directory p.42)", "GIA G&G Summer 2018 GNI https://www.gia.edu/gems-gemology/summer-2018-gemnews-green-blue-maxixe-type-beryl (o-ray bands 500-700 nm, e-ray broad ~690 nm); Andersson 1979 (EPR)"]
# peak_coeff unit: report says "cm-1/unit" with no definition of "unit"; taken as per relative centre density
[[host.chromophore.band]]
centre_nm = 690.0   # 14,493 cm-1; E||c per report
fwhm_cm1 = 3000.0
shape = "gaussian_energy"
peak_coeff = 15.0   # cm-1 per unit of relative centre density
pol = { o = 0.3, e = 1.0 }   # prior placeholder was { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.22 (Beryl table, Maxixe row, 690 nm)"
[[host.chromophore.band]]
centre_nm = 550.0   # 18,182 cm-1; report: E perp c series across 550-650 nm, 550 is the tabulated representative
fwhm_cm1 = 3500.0
shape = "gaussian_energy"
peak_coeff = 8.0    # cm-1 per unit of relative centre density
pol = { o = 1.0, e = 0.2 }   # prior placeholder was { o = 1.0, e = 0.0 }
confidence = "secondary"
source = "Report p.22 (Beryl table, Maxixe row, 550 nm)"
[[host.chromophore.fluorescence]]
# excitation_nm: unknown
# emission_nm: unknown
uv = { lw = "inert", sw = "inert" }
confidence = "secondary"
source = "Report p.22 (Beryl table, Maxixe row, luminescence 'Inert (photobleaches)')"
```

---

## 2. Topaz (Al2SiO4(F,OH)2, orthorhombic, biaxial positive)

Overview. Natural topaz is colorless unless: Cr3+ (pink; imperial orange = Cr3+ plus radiation color centres per the report, Cr3+/Cr4+ per prior digests; heating imperial at 500 C bleaches the defect centres and leaves pure pink via the 530 nm Cr3+ band),
irradiation O- hole centre (blue, 620 nm band; irradiation then 200-300 C anneal gives London/Swiss blue), and brown hole centres (450 nm, bleach below 250 C or in sunlight).
Non-additive: Cr3+ pink and O- blue combine to violet; heating bleaches brown centres but not Cr3+.
Fe/Mn/Cr impurity attribution of the 620 nm band is disputed in some papers (see digest).
Report pages: prose p.22-23, table p.23, directory rows p.42.

```toml
[[host]]
name = "Topaz"
formula = "Al2SiO4(F,OH)2"
optical = "biaxial"
uv_edge_nm = 220.0   # Report p.22: "host cutoff edge is located deep in the ultraviolet at 220 nm" (prior estimate had the same value)
materials = ["Imperial Topaz", "Pink Topaz", "Blue Topaz", "Brown Topaz"]

[[host.chromophore]]
id = "Cr3+"
kind = "ion"
elements = ["Cr"]
site = "Al octahedral (Cs)"
conc_unit = "ppm Cr"   # report: "cm-1/ppm" with no wt/atoms basis stated; assumed wt ppm Cr as in the prior file
conc_typical = [10.0, 200.0]   # digest: imperial samples up to 204 ppm Cr (RSC Adv. 2025); lower bound estimate; not in report
conc_max = 204.0               # max reported in that digest only
color = "Pink to purple-pink; imperial orange when combined with radiation color centres. Heating at 500 C bleaches the centres and leaves pink."
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.22-23 (Topaz bullets; Topaz table, Cr3+ row; directory p.42)", "RSC Adv. 2025 https://pubs.rsc.org/en/content/articlehtml/2025/ra/d5ra02029h (digest); Am. Mineral. 60:705"]
# cross-check (see Discrepancies): 1 ppm Cr by weight, rho 3.55: 3.55e-6/52.0 = 6.83e-8 mol/cm3 = 6.83e-5 mol/L; eps(530) = 0.08/(2.303*6.83e-5) = 509, eps(420) = 0.06/(2.303*6.83e-5) = 381 L/mol/cm. On an atom-ppm basis (1.04e17 atoms/cm3 = 1.73e-4 mol/L): eps = 201 and 151. Both far above the report's own spin-allowed range 1-50. Per Cr ion, beryl Cr3+ is 18 cm-1/wt% Cr2O3 = 2.6e-3 cm-1/ppm Cr, so the topaz value is about 30x larger. Values kept as given, flagged implausible.
[[host.chromophore.band]]
centre_nm = 530.0   # 18,868 cm-1
fwhm_cm1 = 2800.0
shape = "gaussian_energy"
peak_coeff = 0.08   # cm-1 per ppm Cr, as given; probably 10-30x too high
pol = { a = 0.4, b = 1.0, g = 0.6 }   # prior estimate { 0.3, 1.0, 0.6 }
confidence = "secondary"
source = "Report p.23 (Topaz table, Cr3+ row, 530 nm)"
[[host.chromophore.band]]
centre_nm = 420.0   # 23,810 cm-1; was an estimate (Cr3+ 4A2 -> 4T1), now confirmed by the report
fwhm_cm1 = 3200.0
shape = "gaussian_energy"
peak_coeff = 0.06   # cm-1 per ppm Cr, as given; same plausibility problem
pol = { a = 0.4, b = 1.0, g = 0.6 }
confidence = "secondary"
source = "Report p.23 (Topaz table, Cr3+ row, 420 nm)"
[[host.chromophore.fluorescence]]
excitation_nm = [420.0, 530.0]   # prior estimate; the report says only "under UV excitation"
emission_nm = [690.0]
# emission_fwhm_nm: unknown
# quantum_yield: unknown
uv = { lw = "weak red", sw = "weak red" }   # report: "690.0 nm (R-line, red, LW weak)"; SW not stated, prior estimate "weak red/orange"
confidence = "secondary"
source = "Report p.23 (Topaz table, Cr3+ row, luminescence 690 nm); excitation_nm is a prior estimate"

[[host.chromophore]]
id = "O- hole centre (blue topaz)"
kind = "color_centre"
elements = []
treatment = "irradiation"
site = "O- polaron: unpaired hole on an oxygen adjacent to an Al vacancy or F site"
conc_unit = "relative centre density"
# conc_typical: unknown
# conc_max: unknown
color = "Blue (sky to ink, London/Swiss); with Cr3+ pink, violet. Needs electron/gamma/neutron irradiation then 200-300 C anneal."
requires = []
excludes = []
quenched_by = ["heat (annealing bleach, 250-500 C per Report p.31)"]
confidence = "secondary"
sources = ["Report p.23 (Topaz bullets; Topaz table, [O-] polaron row; directory p.42); Report p.31 (thermal bleaching table)", "Springer https://link.springer.com/doi/10.1007/s00269-005-0042-1 (digest: O- polaron, 620 nm; linear correlation of centre concentration with 620 nm absorbance); RSC Adv. 2025"]
# peak_coeff unit: report says "cm-1/unit", "unit" undefined; taken as per relative centre density
[[host.chromophore.band]]
centre_nm = 620.0   # 16,129 cm-1
fwhm_cm1 = 3500.0
shape = "gaussian_energy"
peak_coeff = 5.0    # cm-1 per unit of relative centre density
pol = { a = 0.5, b = 1.0, g = 0.5 }   # prior estimate identical; report: maximum along beta
confidence = "secondary"
source = "Report p.23 (Topaz table, [O-] Polaron row, 620 nm)"
[[host.chromophore.fluorescence]]
# excitation_nm: unknown
# emission_nm: unknown
uv = { lw = "inert", sw = "inert" }
confidence = "secondary"
source = "Report p.23 (Topaz table, [O-] row, luminescence 'Inert')"

[[host.chromophore]]
id = "Brown centre (topaz)"
kind = "color_centre"
elements = []
treatment = "irradiation"
site = "unstable hole centres (report gives no structural model)"
conc_unit = "relative centre density"
# conc_typical: unknown
# conc_max: unknown
color = "Brown to orange-brown with UV continuum, bleaches below 250 C or in direct sunlight."
requires = []
excludes = []
quenched_by = ["heat (below 250 C)", "sunlight"]
confidence = "secondary"
sources = ["Report p.23 (Topaz bullets; Topaz table, Brown Centre row; directory p.42)"]
# peak_coeff unit: "cm-1/unit", undefined unit
[[host.chromophore.band]]
centre_nm = 450.0   # 22,222 cm-1; was an unsourced placeholder, now in the report
fwhm_cm1 = 5000.0
shape = "gaussian_energy"
peak_coeff = 6.0    # cm-1 per unit of relative centre density
pol = { a = 0.6, b = 1.0, g = 0.6 }   # prior placeholder { 0.5, 1.0, 0.5 }
confidence = "secondary"
source = "Report p.23 (Topaz table, Brown Centre row, 450 nm)"
[[host.chromophore.fluorescence]]
# excitation_nm: unknown
# emission_nm: unknown
uv = { lw = "inert", sw = "inert" }   # prior: unknown
confidence = "secondary"
source = "Report p.23 (Topaz table, Brown Centre row, luminescence 'Inert (bleaches at 250 C)')"
```

---

## 3. Tourmaline (XY3Z6(BO3)3Si6O18V3W, trigonal, uniaxial negative)

Overview. Pleochroism strong (o much stronger than e for Fe and Mn species; the reverse E||c for Cu2+).
Report p.30: IVCT dipoles lie in the basal plane, so Fe2+-Fe3+ and Mn2+-Ti4+ have w_o = 1.0, w_e <= 0.1; Cu2+ reverses (w_e = 1.0, w_o = 0.2).
- Cu2+ (Paraiba): bands 700 and 920 nm, E||c stronger; Mn3+ at 520 nm shifts hue to violet; heating at 500 C reduces Mn3+ to Mn2+ and restores neon blue (Report p.23-24).
- Fe2+/Fe3+: IVCT at 720 nm and Fe2+ at 1100 nm (the report also lists "Fe2+ at 430 nm", which is a doubtful assignment); Fe2+ intensity enhanced ~200x when paired with Fe3+ (Mattson & Rossman 1987 digest, not in report).
- Mn: rubellite Mn3+ 520 nm from radiation oxidation of Mn2+; report says Fe2+ "quenches" it so rubellites must be Fe-poor. Canary/brown elbaite by Mn2+-Ti4+ and Fe2+-Ti4+ IVCT near 450 nm.
- Cr3+/V3+ (chrome dravite): bands at 600 and 420 nm.
- Strong Fe quenches the Cu blue (digest).
Report pages: prose p.23-24, table p.24-25, directory rows p.42-43.

```toml
[[host]]
name = "Tourmaline"
formula = "(Na,Ca)(Li,Al,Fe,Mg,Mn)3Al6(BO3)3Si6O18(OH,F)4"
optical = "uniaxial"
uv_edge_nm = 300.0   # Report p.23: Fe-O charge transfer cutoff at 300 nm (prior estimate had the same value)
materials = ["Rubellite", "Indicolite", "Verdelite", "Paraiba Tourmaline", "Chrome Tourmaline", "Schorl", "Dravite"]

[[host.chromophore]]
id = "Cu2+"
kind = "ion"
elements = ["Cu"]
site = "Y distorted octahedral (Jahn-Teller, d9)"
conc_unit = "wt% CuO"
# conc_typical: unknown (Ertl 2012 gives data; not extracted; report gives none)
# conc_max: unknown
color = "Neon blue to blue-green (Paraiba); violet-blue with Mn3+; heating at 500 C removes the Mn3+ 520 nm band and restores neon blue."
requires = ["Mn3+ for violet hues (optional)"]
excludes = ["high Fe2+ (dulls)"]
quenched_by = []
confidence = "secondary"
sources = ["Report p.23-24 (Paraiba bullet; Tourmaline table, Paraiba Cu2+ row; directory p.42)", "Ertl et al. 2012 Mineral. Petrol. https://link.springer.com/doi/10.1007/s00710-012-0234-6; Mineral. Mag. 54:553 https://rruff.info/doclib/MinMag/Volume_54/54-377-553.htm; GIA spectral differentiation doc"]
# cross-check: 1 wt% CuO, rho 3.1, M 79.55: 3.9e-4 mol/cm3 = 0.390 mol/L; eps(700) = 18.0/(2.303*0.390) = 20.0, eps(920) = 12.0/(2.303*0.390) = 13.4, plausible d9
[[host.chromophore.band]]
centre_nm = 700.0   # 14,286 cm-1
fwhm_cm1 = 2500.0
shape = "gaussian_energy"
peak_coeff = 18.0   # cm-1 per wt% CuO
pol = { o = 0.2, e = 1.0 }   # prior estimate identical; report p.30 and p.24 agree
confidence = "secondary"
source = "Report p.24 (Tourmaline table, Paraiba Cu2+ row, 700 nm)"
[[host.chromophore.band]]
centre_nm = 920.0   # 10,870 cm-1
fwhm_cm1 = 2800.0
shape = "gaussian_energy"
peak_coeff = 12.0   # cm-1 per wt% CuO
pol = { o = 0.2, e = 1.0 }
confidence = "secondary"
source = "Report p.24 (Tourmaline table, Paraiba Cu2+ row, 920 nm)"
[[host.chromophore.fluorescence]]
# excitation_nm: unknown
# emission_nm: unknown
uv = { lw = "inert", sw = "inert" }   # report: "Inert (Cu-quenched)"
confidence = "secondary"
source = "Report p.24 (Tourmaline table, Paraiba row, luminescence)"

[[host.chromophore]]
id = "Mn3+"
kind = "ion"
elements = ["Mn"]
site = "Y octahedral, distorted (d4)"
conc_unit = "wt% MnO"
# conc_typical: unknown
# conc_max: unknown
color = "Pink to deep ruby-red (rubellite, ionizing radiation oxidises Mn2+), violet in Paraiba."
requires = []
excludes = ["high Fe2+ (Report p.24: rubellites must be iron-poor; mechanism given by the report is wrong, see Discrepancies)"]
quenched_by = []
confidence = "secondary"
sources = ["Report p.24 (Rubellite bullet; Tourmaline table, Rubellite Mn3+ row; directory p.42)", "Ertl 2012 / Mineral. Mag. 54:553 digest: Mn3+ band at 520 nm"]
# cross-check: 1 wt% MnO, rho 3.1, M 70.94: 0.031/70.94 = 4.37e-4 mol/cm3 = 0.437 mol/L; eps(520) = 22.0/(2.303*0.437) = 21.9, plausible
[[host.chromophore.band]]
centre_nm = 520.0   # 19,231 cm-1
fwhm_cm1 = 3200.0
shape = "gaussian_energy"
peak_coeff = 22.0   # cm-1 per wt% MnO
pol = { o = 1.0, e = 0.2 }   # prior estimate identical
confidence = "secondary"
source = "Report p.24 (Tourmaline table, Rubellite Mn3+ row, 520 nm)"
[[host.chromophore.fluorescence]]
excitation_nm = [360.0]   # prior estimate, not in report
emission_nm = [580.0]     # prior estimate, not in report
# emission_fwhm_nm: unknown
# quantum_yield: unknown
uv = { lw = "inert to weak orange", sw = "inert to weak orange" }   # report: "Inert to weak orange", no LW/SW split; prior estimate was "weak to moderate yellow-orange" / "weak"
confidence = "secondary"
source = "Report p.24 (Tourmaline table, Rubellite row, luminescence 'Inert to weak orange'); excitation/emission wavelengths are prior estimates"

[[host.chromophore]]
id = "Fe2+-Fe3+ IVCT (and Fe2+)"
kind = "ivct_pair"
elements = ["Fe", "Fe"]
site = "Y-Y and Y-Z edge-sharing octahedra"
conc_unit = "wt% FeO"
# conc_typical: unknown
# conc_max: unknown
color = "Deep green (verdelite) to indigo-blue (indicolite), brown/black in schorl."
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.24 (Indicolite bullet; Tourmaline table, Indicolite Fe2+-Fe3+ IVCT row; directory p.42)", "Mattson & Rossman 1987 https://resolver.caltech.edu/CaltechAUTHORS:20140306-085514791: Fe2+-Fe3+ pairs intensify Fe2+ ~200x (digest, not in report)"]
# cross-check: 1 wt% FeO, rho 3.1, M 71.84: 4.32e-4 mol/cm3 = 0.432 mol/L; eps(720) = 35.0/(2.303*0.432) = 35, eps(1100) = 25.0/(2.303*0.432) = 25 (below the report's IVCT range 100-2000; IVCT scales with pair concentration)
[[host.chromophore.band]]
centre_nm = 720.0   # 13,889 cm-1
fwhm_cm1 = 3200.0
shape = "gaussian_energy"
peak_coeff = 35.0   # cm-1 per wt% FeO
pol = { o = 1.0, e = 0.1 }   # prior estimate identical
confidence = "secondary"
source = "Report p.24 (Tourmaline table, Indicolite row, 720 nm)"
[[host.chromophore.band]]
centre_nm = 1100.0   # 9,091 cm-1 (report prints 9090); NIR, outside render range anyway
fwhm_cm1 = 3800.0
shape = "gaussian_energy"
peak_coeff = 25.0    # cm-1 per wt% FeO
pol = { o = 1.0, e = 0.1 }
confidence = "secondary"
source = "Report p.24 (Tourmaline table, Indicolite row, 1100 nm); prior revision had 1100 as a mid-range placeholder of 918-1220 nm"
[[host.chromophore.band]]
centre_nm = 430.0   # 23,256 cm-1; report prose only, called "isolated octahedral Fe2+", assignment doubtful
# fwhm_cm1: unknown
shape = "gaussian_energy"   # estimate
# peak_coeff: unknown
pol = { o = 1.0, e = 0.1 }   # paragraph-level weights from the report (w_o = 1.0, w_e = 0.1)
confidence = "secondary"
source = "Report p.24 (Indicolite bullet, prose only; absent from the table)"
[[host.chromophore.fluorescence]]
# excitation_nm: unknown
# emission_nm: unknown
uv = { lw = "inert", sw = "inert" }   # report: "Inert (Fe-quenched)"
confidence = "secondary"
source = "Report p.24 (Tourmaline table, Indicolite row, luminescence)"

[[host.chromophore]]
id = "Fe2+-Ti4+ / Mn2+-Ti4+ IVCT"
kind = "ivct_pair"
elements = ["Fe", "Ti"]
site = "Y-Y edge-sharing octahedra"
conc_unit = "wt% TiO2"
# conc_typical: unknown
# conc_max: unknown
color = "Canary-yellow in Mn-rich elbaite; yellow-brown to dark brown in iron-bearing dravite-schorl (Fe2+-Ti4+)."
requires = ["Fe or Mn", "Ti"]
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.24 (Canary/brown elbaite bullet; Tourmaline table, Canary Mn2+-Ti4+ IVCT row; directory p.42)", "Mattson & Rossman (Caltech thesis https://thesis.caltech.edu/1307/) digest"]
# report gives "cm-1/wt%" without naming the oxide; IVCT scales with the product of both partners (Report p.30), so a per-wt% TiO2 coefficient is only an approximation
[[host.chromophore.band]]
centre_nm = 450.0   # 22,222 cm-1; prose gives a 415-450 nm range, table 450
fwhm_cm1 = 4000.0
shape = "gaussian_energy"
peak_coeff = 30.0   # cm-1 per wt%
pol = { o = 1.0, e = 0.2 }   # prior estimate identical; Report p.30 states w_e <= 0.1 for Mn2+-Ti4+ IVCT, see Discrepancies
confidence = "secondary"
source = "Report p.24 (Tourmaline table, Canary row, 450 nm)"
[[host.chromophore.fluorescence]]
# excitation_nm: unknown
# emission_nm: unknown
uv = { lw = "inert", sw = "inert" }
confidence = "secondary"
source = "Report p.24 (Tourmaline table, Canary row, luminescence 'Inert')"

[[host.chromophore]]
id = "Cr3+/V3+ (chrome tourmaline)"
kind = "ion"
elements = ["Cr", "V"]
site = "Y/Z octahedral (Z, Y)"
conc_unit = "wt% Cr2O3"
# conc_typical: unknown
# conc_max: unknown
color = "Deep green (chrome dravite), strong dichroism."
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.24-25 (Chrome tourmaline bullet; Tourmaline table, Chrome Cr3+/V3+ row; directory p.42-43)"]
# cross-check: 1 wt% Cr2O3, rho 3.1: 0.031/151.99*2 = 4.08e-4 mol/cm3 = 0.408 mol/L; eps(600) = 20.0/(2.303*0.408) = 21, eps(420) = 18.0/(2.303*0.408) = 19, plausible. FWHM 2800/3200 and Cr/V merged into one row are identical to beryl and topaz entries (see Discrepancies).
[[host.chromophore.band]]
centre_nm = 600.0   # 16,667 cm-1
fwhm_cm1 = 2800.0
shape = "gaussian_energy"
peak_coeff = 20.0   # cm-1 per wt% Cr2O3 (report: "wt%", oxide not named)
pol = { o = 1.0, e = 0.3 }   # prior estimate identical
confidence = "secondary"
source = "Report p.24 (Tourmaline table, Chrome Cr3+/V3+ row, 600 nm)"
[[host.chromophore.band]]
centre_nm = 420.0   # 23,810 cm-1; new band
fwhm_cm1 = 3200.0
shape = "gaussian_energy"
peak_coeff = 18.0   # cm-1 per wt% Cr2O3
pol = { o = 1.0, e = 0.3 }
confidence = "secondary"
source = "Report p.24 (Tourmaline table, Chrome Cr3+/V3+ row, 420 nm)"
[[host.chromophore.fluorescence]]
# excitation_nm: unknown
# emission_nm: unknown
uv = { lw = "inert", sw = "inert" }   # prior: unknown
confidence = "secondary"
source = "Report p.24 (Tourmaline table, Chrome row, luminescence 'Inert')"
```

---

## 4. Andalusite (Al2SiO5, orthorhombic, biaxial negative)

Overview. Strong pleochroism (green/brown/red), from Mn3+ (viridine variety) on the octahedral Al1 site and Fe3+ on Al1/Al2;
V3+ and Cr3+ give yellow-to-colorless pleochroism. Indicatrix: X || c (alpha), Y || a (beta), Z || b (gamma).
Report p.25: Mn3+ bands 702, 455, 426 nm (14,250, 22,000, 23,500 cm-1) give green along beta and gamma and yellow-red along alpha; Fe3+ 440 nm plus UV edge; V3+/Cr3+ 565 and 420 nm along alpha.
RI: alpha 1.629-1.640, beta 1.634-1.645, gamma 1.638-1.650 (Smith College digest, non-authoritative).
Report pages: prose and table p.25, directory rows p.43.

```toml
[[host]]
name = "Andalusite"
formula = "Al2SiO5"
optical = "biaxial"
uv_edge_nm = 300.0   # Report p.25: transmission edge at 300 nm (prior estimate had the same value)
materials = ["Andalusite", "Chiastolite", "Viridine"]

[[host.chromophore]]
id = "V3+ / Cr3+"
kind = "ion"
elements = ["V", "Cr"]
site = "Al1 octahedral"
conc_unit = "wt% V2O3"
# conc_typical: unknown
# conc_max: unknown
color = "Golden yellow (alpha), light green (beta), colorless (gamma); strong alpha absorption."
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.25 (Andalusite table, V3+/Cr3+ row; directory p.43)", "Carlson & Rossman, Am. Mineral. https://resolver.caltech.edu/CaltechAUTHORS:20130515-082554444 (digest)"]
# cross-check: 1 wt% V2O3, rho 3.15, M 149.88: 4.20e-4 mol/cm3 = 0.420 mol/L; eps(565) = 14.0/(2.303*0.420) = 14.5, eps(420) = 12.0/(2.303*0.420) = 12.4, plausible
[[host.chromophore.band]]
centre_nm = 420.0   # 23,810 cm-1
fwhm_cm1 = 3200.0
shape = "gaussian_energy"
peak_coeff = 12.0   # cm-1 per wt% V2O3 (report: "wt%")
pol = { a = 1.0, b = 0.3, g = 0.3 }   # prior estimate identical
confidence = "secondary"
source = "Report p.25 (Andalusite table, V3+/Cr3+ row, 420 nm)"
[[host.chromophore.band]]
centre_nm = 565.0   # 17,699 cm-1
fwhm_cm1 = 2800.0
shape = "gaussian_energy"
peak_coeff = 14.0   # cm-1 per wt% V2O3
pol = { a = 1.0, b = 0.3, g = 0.3 }
confidence = "secondary"
source = "Report p.25 (Andalusite table, V3+/Cr3+ row, 565 nm)"
[[host.chromophore.fluorescence]]
# excitation_nm: unknown
# emission_nm: unknown
uv = { lw = "inert", sw = "inert" }   # prior: unknown
confidence = "secondary"
source = "Report p.25 (Andalusite table, luminescence 'Inert')"

[[host.chromophore]]
id = "Mn3+"
kind = "ion"
elements = ["Mn"]
site = "Al1 octahedral (Jahn-Teller, d4)"
conc_unit = "wt% Mn2O3"
# conc_typical: unknown
# conc_max: unknown
color = "alpha yellow-red, beta green, gamma emerald green (viridine)."
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.25 (Andalusite table, Mn3+ (Viridine) row; directory p.43)", "Manganian andalusite polarized spectra https://rruff.info/uploads/CM10_47.pdf (digest: 14250, 22000, 23500 cm-1)"]
# cross-check: 1 wt% Mn2O3, rho 3.15, M 157.9: 3.99e-4 mol/cm3 = 0.399 mol/L; eps(702) = 18.0/(2.303*0.399) = 19.6, eps(455) = 16.3, eps(426) = 13.1, plausible
[[host.chromophore.band]]
centre_nm = 702.0   # 14,250 cm-1 (1e7/702 = 14,245)
fwhm_cm1 = 2500.0
shape = "gaussian_energy"
peak_coeff = 18.0   # cm-1 per wt% Mn2O3
pol = { a = 0.3, b = 0.8, g = 1.0 }   # prior estimate { 0.5, 0.5, 1.0 }
confidence = "secondary"
source = "Report p.25 (Andalusite table, Mn3+ row, 702 nm)"
[[host.chromophore.band]]
centre_nm = 455.0   # 22,000 cm-1
fwhm_cm1 = 2000.0
shape = "gaussian_energy"
peak_coeff = 15.0   # cm-1 per wt% Mn2O3
pol = { a = 0.4, b = 1.0, g = 0.8 }   # prior estimate { 0.5, 0.5, 1.0 }
confidence = "secondary"
source = "Report p.25 (Andalusite table, Mn3+ row, 455 nm)"
[[host.chromophore.band]]
centre_nm = 426.0   # 23,500 cm-1 (1e7/426 = 23,474)
fwhm_cm1 = 1800.0
shape = "gaussian_energy"
peak_coeff = 12.0   # cm-1 per wt% Mn2O3
pol = { a = 0.4, b = 1.0, g = 0.8 }
confidence = "secondary"
source = "Report p.25 (Andalusite table, Mn3+ row, 426 nm)"
[[host.chromophore.fluorescence]]
# excitation_nm: unknown
# emission_nm: unknown
uv = { lw = "inert", sw = "inert" }   # prior: unknown
confidence = "secondary"
source = "Report p.25 (Andalusite table, luminescence 'Inert')"

[[host.chromophore]]
id = "Fe3+"
kind = "ion"
elements = ["Fe"]
site = "Al1 / Al2 substitution"
conc_unit = "wt% Fe2O3"
conc_typical = [0.1, 1.5]   # estimate; not in report
conc_max = 3.0              # estimate; not in report
color = "alpha brown-yellow, beta olive-green, gamma dark green; adds to green pleochroism."
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.25 (Andalusite table, Fe3+ row; directory p.43)"]
# cross-check: 1 wt% Fe2O3, rho 3.15, M 159.69: 3.95e-4 mol/cm3 = 0.395 mol/L; eps(440) = 6.0/(2.303*0.395) = 6.6 (above the spin-forbidden range 0.05-2, mildly), 380 nm edge: 40.0/(2.303*0.395) = 44 (CT edge, fine)
[[host.chromophore.band]]
centre_nm = 440.0   # 22,727 cm-1; was an estimate, now in report
fwhm_cm1 = 1200.0
shape = "lorentzian"
peak_coeff = 6.0    # cm-1 per wt% Fe2O3
pol = { a = 0.4, b = 0.7, g = 1.0 }   # prior estimate { 0.3, 0.7, 1.0 }
confidence = "secondary"
source = "Report p.25 (Andalusite table, Fe3+ row, 440 nm)"
[[host.chromophore.band]]
centre_nm = 380.0   # 26,316 cm-1; UV charge-transfer edge, new band
fwhm_cm1 = 4500.0
shape = "gaussian_energy"   # edge treated as a broad gaussian (estimate; the report gives no shape)
peak_coeff = 40.0   # cm-1 per wt% Fe2O3
pol = { a = 0.5, b = 0.8, g = 1.0 }
confidence = "secondary"
source = "Report p.25 (Andalusite table, Fe3+ row, 380 nm (edge))"
[[host.chromophore.fluorescence]]
# excitation_nm: unknown
# emission_nm: unknown
uv = { lw = "inert", sw = "inert" }
confidence = "secondary"
source = "Report p.25 (Andalusite table, luminescence 'Inert')"
```

---

## 5. Cordierite (iolite)

The report has NO cordierite section and NO directory row. It mentions cordierite once, on p.46 (Conclusions: "Chromophores like blue sapphire, aquamarine, tanzanite, and cordierite cannot be simulated via single-cation absorption. Accurate color calculations depend on intervalence charge transfer pairs (Fe2+-Ti4+, Fe2+-Fe3+)"), and in the title of reference 19 (p.48). That is a statement of mechanism only, with no band centres, FWHM, coefficients, polarization weights or luminescence. The host is added as a stub so the renderer can list it, with no chromophores; nothing is invented.

```toml
[[host]]
name = "Cordierite"
formula = "Mg2Al4Si5O18"   # NOT in report; standard mineral formula, estimate
optical = "biaxial"        # NOT in report; literature: biaxial negative with strong trichroism (blue / pale yellow / violet-blue); estimate
# uv_edge_nm: unknown (report gives none)
materials = ["Iolite"]     # report p.46 uses only the name "cordierite"
# No [[host.chromophore]]: the report names the mechanism (Fe2+-Fe3+ and Fe2+-Ti4+ IVCT pairs, Report p.46) but gives no numbers.
# Beryl-type channel Fe2+ is also plausible (reference 19 treats beryl, cordierite and osumilite together); unknown.
```

---

## Discrepancies with the report

Part A: prior file versus report (differ by more than 10 % or in kind). "Prior" values were estimates or digest readings; the report value is now used.

| # | Item | Prior file | Report | Which is more plausible |
|---|---|---|---|---|
| 1 | Beryl Fe3+ 370 and 426 nm bands | listed under the Fe2+-Fe3+ IVCT chromophore (only 370 under heliodor) | both under Heliodor Fe3+ (p.22); IVCT row has only 680 and 835 | Report. These are Fe3+ ligand-field lines, not IVCT; moved. |
| 2 | Morganite 567 nm pol e | 0.3 | 0.35 (p.22) | Indistinguishable; report used. |
| 3 | Maxixe 690 nm pol | { o 1.0, e 1.0 } placeholder | { o 0.3, e 1.0 } | Report; consistent with the GIA digest (broad e-ray 690 nm). |
| 4 | Maxixe 550 nm pol e | 0.0 | 0.2 | Report; the o-ray series spans 500-700 nm and is not purely o. |
| 5 | Maxixe bleaching temperature | about 200 C (my estimate) | above 150 C (p.21-22) | Either; Maxixe is known to fade at modest heat, report used. |
| 6 | Beryl Cr3+ luminescence excitation | 430, 600 nm (absorption bands) | 365 nm UV (p.21) | Report for the gemmological LW/SW sense; the absorption bands also excite. |
| 7 | Topaz Cr3+ pol a (530 and 420 nm) | 0.3 | 0.4 (33 %) | Unknown; report used. |
| 8 | Topaz brown centre pol | { 0.5, 1.0, 0.5 } placeholder | { 0.6, 1.0, 0.6 } | Report (prior was an unsourced placeholder). |
| 9 | Imperial topaz origin | Cr3+ with Cr4+ per a 2025 digest | Cr3+ plus radiation color centres, 500 C bleaches the centres (p.22) | Report for the heat behaviour. The Cr4+ question stays open. |
| 10 | Tourmaline Mn3+ luminescence | weak to moderate yellow-orange (estimate; 360 nm / 580 nm) | "inert to weak orange" (p.24) | Report; the wavelengths remain estimates. |
| 11 | Andalusite Mn3+ 702 nm pol | { 0.5, 0.5, 1.0 } | { 0.3, 0.8, 1.0 } | Report (more consistent with its own color statement: green along beta and gamma). |
| 12 | Andalusite Mn3+ 455 and 426 nm pol | { 0.5, 0.5, 1.0 } | { 0.4, 1.0, 0.8 } | Report, same reason. |
| 13 | Andalusite Fe3+ pol a | 0.3 | 0.4 | Report used. |

Differences of exactly 10 % or less are not listed (for example Beryl Cr3+ 600 nm pol e 0.5 versus 0.55).
Not a discrepancy but new: report supplies FWHM and peak coefficients for every band (all `-1.0` before), 380 nm Fe3+ (andalusite), 420 nm chrome tourmaline, 426 nm heliodor, 430 nm tourmaline Fe2+ band (partial).

Part B: report values that look physically implausible or internally inconsistent (all were still entered as given, with a comment).

1. Topaz Cr3+ peak coefficients 0.08 cm-1/ppm (530 nm) and 0.06 cm-1/ppm (420 nm). On a weight-ppm basis this is eps of about 510 and 380 L/mol/cm; on an atom-ppm basis about 200 and 150. The report's own range for spin-allowed d-d is 1-50 (p.2), and beryl Cr3+ in the same report corresponds to 2.6e-3 cm-1/ppm Cr. The topaz coefficient is therefore about 30x too high, which would make a 200 ppm imperial stone alpha about 16 cm-1 (opaque). Probably an order of magnitude or two off (ppm vs 0.1 ppm or per-wt% confusion). The ppm basis (weight or atoms) is also not stated. Do not use for rendering until calibrated.
2. Heliodor Fe3+ 370 and 426 nm: 15.0 and 8.0 cm-1/wt% give eps of about 19 and 10; the report says spin-forbidden eps is 0.05-2 (p.2). Too high by 5-10x. Similarly andalusite Fe3+ 440 nm: eps of about 6.6.
3. Aquamarine dichroism direction. The report says the 680 nm IVCT band is E||c (w_e = 1.0, w_o = 0.2) and that this gives "blue transmission in the extraordinary ray and pale greenish-blue in the ordinary ray". In real aquamarine the deeper blue is seen in the ordinary ray (E perp c absorption of the Fe2+ near 820 nm and the IVCT), the reverse of the report. I recall the Fe2+-Fe3+ IVCT of beryl nearer 620 nm than 680 nm (Goldman 1978 and later work), and 680 nm collides with the Cr3+ R-lines; cannot verify offline. Prior file carried the same E||c reading from a digest flagged "verify", so the two agree but share the same weak basis.
4. Beryl 476 nm line labelled 2T1. In Cr3+ (d3) the 2T1 level lies next to 2E near 650 nm; a sharp line at 476 nm should be 2T2 (or similar). Centre value is consistent with Wood & Nassau (4760 A), only the label is wrong. Not used by the file.
5. Rubellite: "Co-existing Fe2+ quenches this color by non-radiative energy transfer" (p.24). Energy transfer quenches luminescence, not absorption color; Fe2+ cannot bleach the Mn3+ 520 nm band by this route. Fe2+ would instead add its own absorption (green/blue overlay) and dull the pink. Entered as an exclusion, not as a quench.
6. Tourmaline "ordinary-ray absorption exceeds extraordinary by up to 100:1 for iron species" (p.23) versus w_e = 0.1 (10:1) in the tables and in p.30 ("w_e <= 0.1" for Fe2+-Fe3+ and Mn2+-Ti4+). Also the canary Mn2+-Ti4+ row has w_e = 0.2 and chrome 0.3, contradicting the p.30 rule for Mn2+-Ti4+. The table values were used; the rule is not reproduced by them.
7. Tourmaline "isolated octahedral Fe2+ band at 430 nm" (p.24). Fe2+ has no strong spin-allowed band at 430 nm (its d-d bands are in the NIR); this is almost certainly Fe3+ (6A1 to 4A1,4E, near 430-440 nm). Entered without FWHM and coefficient.
8. Beryl and tourmaline Cr3+: "Cr3+/V3+" is one row for tourmaline with 600 and 420 nm FWHM 2800 and 3200 cm-1, the same FWHM as beryl emerald, topaz and grossular rows. These look templated rather than measured; sensible magnitudes but not host-specific.
9. Rule p.31 gives a threshold "0.5-1.0 wt% FeO completely quenches R-line emission" without naming the host; it is a generic statement taken from ruby and spinel and is applied to emerald only as a comment.
10. Peak coefficient units. The beryl table header carries a stray "wt% FeO" (p.21-22) although the rows are Cr, V and Mn species; color-centre rows use "cm-1/unit" with no definition; topaz Cr is "cm-1/ppm" without basis. No ppm-vs-wt% conflict was found in the numbers for these hosts, other than item 1, but the oxide basis of every "wt%" coefficient is an assumption.
11. Beryl aquamarine IVCT and tourmaline IVCT coefficients give eps of 12-35 L/mol/cm, below the report's own IVCT range 100-2000 (p.2). This is explainable by pair-scaling (effective concentration is the Fe2+ x Fe3+ product, Report p.30), so not flagged as an error, but the per-wt% FeO coefficient will overpredict at low Fe and underpredict at high Fe.
12. No FWHM-vs-centre inconsistency was found: all 34 FWHM values are between 20 and 5000 cm-1, with narrow ones only for sharp lines (476, 683 nm R-line, 370/426 nm Fe3+), and the polarization weights agree with each host's optical character (uniaxial {o, e}; biaxial {a, b, g}).
13. Cordierite: the report contains no cordierite data; the brief assumed it did. Only a mention on p.46.

## Open questions

1. Report coefficients are "cm-1 per wt%" with no oxide named. Confirm against GIA Spring 2020-style tables whether Cr2O3, V2O3, FeO or Fe2O3 is meant; topaz Cr "per ppm" basis (weight or atoms) must be settled and the 30x magnitude problem resolved before use.
2. Beryl: direction and band position of the Fe2+-Fe3+ IVCT (report 680 nm E||c versus my recollection of about 620 nm E perp c); exact Mn concentration ranges (red beryl, morganite); Fe3+ 370/426 coefficients (item B2).
3. Topaz: authoritative polarized spectra (Cr3+, O- centre, brown centre); the Cr4+ attribution in imperial topaz; alternative Cr3+/Fe2+/Mn2+ origin of the 620 nm band. Definition of "unit" for centre-density coefficients.
4. Tourmaline: concentration limits of Cu in elbaite (Ertl 2012); source of the 430 nm band in indicolite and its FWHM; Mn2+ luminescence wavelengths; whether the Fe2+-Ti4+ and Mn2+-Ti4+ IVCT should be split into two entries.
5. Andalusite: years and authors of the Mn andalusite and V/Cr andalusite papers to be confirmed; concentration ranges; the Cr3+ red luminescence that real andalusite may show versus the report's "Inert".
6. Cordierite (iolite): no data in the report. Needs a primary source for Fe2+-Fe3+ and channel-Fe2+ bands and polarized (alpha, beta, gamma) spectra; formula, optic class and edge were added from outside the report.
7. UV edges now come from the report (300, 220, 300, 300 nm) but the report gives them without a source.
8. Heliodor is a separate Fe3+ entry now; its debated origin (Fe3+ versus irradiated Fe3+ to Fe2+) is unchanged.

## Coverage table

| Host | Chromophore | Bands | Pol | Coeff | Conc range | Fluorescence |
|---|---|---|---|---|---|---|
| Beryl | Cr3+ | secondary (4 bands) | secondary | secondary (R-line unknown) | estimate | secondary |
| Beryl | V3+ | secondary (2) | secondary | secondary | estimate | secondary |
| Beryl | Fe2+-Fe3+ IVCT | secondary (2) | secondary | secondary | estimate | secondary |
| Beryl | Fe3+ heliodor | secondary (2) | secondary | secondary (implausibly high) | estimate | secondary |
| Beryl | Mn3+ red beryl | secondary (2) | secondary | secondary | unknown | secondary |
| Beryl | Mn morganite | secondary (1) | secondary | secondary | unknown | secondary |
| Beryl | Maxixe | secondary (2) | secondary | secondary (unit undefined) | unknown | secondary |
| Topaz | Cr3+ | secondary (2) | secondary | secondary (about 30x too high) | secondary (max 204 ppm) | secondary (excitation estimate) |
| Topaz | O- blue | secondary (1) | secondary | secondary (unit undefined) | unknown | secondary |
| Topaz | brown | secondary (1) | secondary | secondary (unit undefined) | unknown | secondary |
| Tourmaline | Cu2+ | secondary (2) | secondary | secondary | unknown | secondary |
| Tourmaline | Mn3+ | secondary (1) | secondary | secondary | unknown | secondary (wavelengths estimate) |
| Tourmaline | Fe2+/Fe3+ IVCT | secondary (3; 430 nm lacks FWHM and coeff) | secondary | secondary (2 of 3) | unknown | secondary |
| Tourmaline | Fe/Mn-Ti IVCT | secondary (1) | secondary | secondary | unknown | secondary |
| Tourmaline | Cr/V | secondary (2) | secondary | secondary | unknown | secondary |
| Andalusite | V3+/Cr3+ | secondary (2) | secondary | secondary | unknown | secondary |
| Andalusite | Mn3+ | secondary (3) | secondary | secondary | unknown | secondary |
| Andalusite | Fe3+ | secondary (2) | secondary | secondary | estimate | secondary |
| Cordierite | (none) | none | none | none | none | none (not in report) |
