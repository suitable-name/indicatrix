# Validation of the 2026-10 research against our colorimetry

Status: integrated (`data_version` 3); **sections 3, 6 and 7 are superseded in part by section 8 (`data_version` 4, primary data batch 1): the stated CIELAB below is no fitting target any more and several fitted bands were removed.** Scope: `research-2026-10-polarized-spectra.md` (report 1) and
`research-2026-10-polarized-spectra-2.md` (report 2). Every tabulated spectrum is a fixture in
`crates/indicatrix/data/reference_spectra/*.csv` (metadata header: sample, composition, units, path,
source; `# lab:` lines hold the measured CIELAB). The tests are
`crates/indicatrix/src/optics/chromophore/reference_validation.rs` (validation) and
`measured_color_tests.rs` (recipes and built-ins against the measured colors).

## 1. Method

T(lambda) = exp(-alpha L) (Napierian alpha as tabulated), D65 or A (Planckian 2856 K), 380-780 nm at
1 nm, CIE 1931 2 degrees, Lab relative to the illuminant's own white (L* = 100 for T = 1), computed
with `color::body_color` (CIEDE2000 validated against the Sharma vectors). Tabulated points are
interpolated linearly. Unpolarised colors: a uniaxial `mean_uniax` is the plan's rule
(2 T_o + T_e) / 3. For the alexandrite "unpolarised, along b" color the mean of the transmittances
of the two rays that vibrate in the a-c plane, (T_alpha + T_gamma) / 2, is used (the plan's
three-mode mean is evaluated next to it); the S-Met "c-cut basal slab" is the o-ray.

Sparse lines. A few rows are single points of a narrow line (ruby R2/R1 692.8/694.3 nm, chrysoberyl
678.5/680.4, emerald 683.5, report-2 ruby 694). Interpolated linearly with their neighbours they
become 20-80 nm triangles. They are therefore removed from the continuum, the continuum is
interpolated linearly, and the excess is put back as a Gaussian in energy of the stated linewidth
(12-18 cm^-1). The B lines (468.5/475.1/476.5 nm) are not visible in any table (468.5 nm lies on the
smooth continuum) and are not added. Effect on the color: <= 0.3 dE00 everywhere except the
alexandrite A rows (<= 2.1, the 680 nm line sits in a steep part of the A spectrum).

## 2. dE00 of each tabulated spectrum against its own measured CIELAB

| Sample (fixture) | color | L (cm) | dE00 |
|---|---|---|---|
| S-Met sapphire E-perp-c (`sapphire_s_met_metamorphic`) | D65 | 0.20 | **4.1** |
| S-Met E-par-c | D65 | 0.20 | 7.7 |
| S-Met unpolarised (c-cut, = o-ray) | D65 | 0.20 | 4.3 |
| S-Bas sapphire E-perp-c (`sapphire_s_bas_basaltic`) | D65 / A / D65 thick | 0.20 / 0.20 / 0.50 | 6.7 / 8.4 / 5.1 |
| Amethyst E-perp-c | D65 | 0.60 | **3.4** |
| R-1 ruby 0.05 wt% | D65 | 0.50 | 15.9 |
| R-2 ruby 0.25 wt% | D65 | 0.50 | 24.3 |
| R-3 ruby 0.72 wt% | D65 | 0.25 / 0.50 | 25.8 / 20.3 |
| Report-2 ruby 0.25 wt% (synthesised) | D65 | 0.50 | 26.1 |
| Report-2 sapphire (4 mm stated) | D65 | 0.40 | 9.5 |
| Alexandrite alpha / beta / gamma | D65 | 0.20 | 20.1 / 30.2 / 28.5 |
| Alexandrite alpha / beta / gamma | A | 0.20 | 31.8 / 38.1 / 23.6 |
| Alexandrite unpolarised (along b) | D65 / A | 0.20 | 11.3 / 23.3 |
| Heated tanzanite beta | D65 | 0.40 | 21.2 |
| Natural tanzanite gamma | D65 | 0.40 | 17.0 |
| Peridot alpha | D65 | 0.50 | 12.5 |
| Emerald E-perp-c / E-par-c | D65 | 0.30 | 9.9 / 12.3 |
| Citrine unpolarised | D65 | 0.60 | 12.7 |

Only S-Met E-perp-c and the amethyst reproduce their CIELAB (dE00 <= 4.3); S-Bas is borderline
(5-8). Everything else disagrees with its own CIELAB, by 10-38 dE00 for the ruby, alexandrite,
tanzanite, peridot, emerald and citrine rows. This is a finding about the research, not about our
colorimetry.

## 3. Where dE00 > 3: what is inconsistent

Hypotheses tested for the failing rows (none removes the discrepancy):

- path length: the best-fit path is 1.4-5 times the stated one for the rubies and still leaves
  dE00 13-19 (R-3 at 0.5 cm: 6.6) (R-1 states 1.00 cm in the sample list and 0.50 cm in the color
  table; the spectrum fits 0.50 cm better than 1.00);
- decadic instead of Napierian alpha: worse;
- polarisation mix of the unpolarised ruby (o, e, (o+e)/2, (2o+e)/3): after refitting the band
  family every mix leaves R-2 at dE00 4.6-7.5;
- the A illuminant at 3200 K instead of 2856 K: moves the A rows by 0.3 (S-Bas) to 4.5 (alexandrite
  gamma) dE00 and no row becomes consistent.

The common pattern is a **b* offset of -25 to -45** for ruby, alexandrite and the tanzanite
gamma/beta rows (the tabulated windows at 450-500 nm are too open): the stated CIELAB values need an
additional broad absorption at 450-500 nm. One extra Gaussian (centre 452 nm, FWHM ~7500 cm^-1,
~26 cm^-1 per wt% Cr2O3) brings the four ruby CIELAB values within dE00 3.5-6.8. Report 2's
synthesised table has the same defect (b* -46 against +22). This is the Cr-O LMCT Urbach tail the
research itself lists (report 2: reaching ~500 nm; report 1: 300-360 nm), and it is added to the
catalogue as a fitted band.

Further internal findings:

- **Widths.** The tabulated bands are narrower than the stated FWHM: ruby U half-width 2350 cm^-1
  (report 2: 2630) against 3180 / 3350 stated; sapphire Fe2+-Ti4+ 3100 against 5100.
- **R-3** carries 55.4 cm^-1 per wt% at 555 nm against 51.2 for R-1/R-2 (+8 %, the Fe-containing
  sample).
- **Alexandrite colors.** The stated "unpolarised along b" color equals the XYZ mean of the stated
  alpha and gamma colors to dE00 3.4 (D65) and 6.4 (A) (the three-ray mean: 12.2 / 10.3). The stated
  Delta E*ab = 26.4 is a CIELAB-76 difference between two colors that each refer to their own white;
  as CIEDE2000 it is 30.1.
- **color change.** Report 2's V-corundum "Delta E often > 25" is refuted by report 1's V spectrum:
  D65 vs 3200 K stays at 2.9-4.5 dE00 over 1-10 mm (D65 vs A 3.8-5.8).
- **Chroma maximum.** The measured C* at 0.5 cm is 32.5 / 73.8 / 40.2 for 0.05 / 0.25 / 0.72 wt%:
  the maximum near 0.25 wt% is in the measurements (plan criterion 1 expects a monotonic rise).
- **Tanzanite.** 0.38 wt% V2O3 is 7670 ppm of Al sites, 2.5 times the previous `conc_max`
  (3000); the spectra do not fix the composition (sigma unknown).
- **Heated tanzanite.** "Heated gamma becomes like beta" is not supported: the natural gamma table
  has 2.65 cm^-1 at 585 nm against 5.8 for the heated beta table.

## 4. Unit findings

- **V2O3.** 0.26 wt% V2O3 is 706 ppma of all atoms (1770 ppm of Al sites, 3500 per Al2O3 formula
  unit), not 305. The V-corundum spectrum with the research sigma = 2.05e-19 cm^2
  (0.0241 cm^-1 per ppma) implies 305 ppma of all atoms (0.113 wt%): **the spectrum supports 305
  ppma, not the 0.26 wt% label** (2.3 times apart). The catalogue V3+ coefficient (per ppma of all
  atoms, 0.0239 at 575 nm) is fitted to the spectrum at 305 ppma. ("0.015 wt% Fe2O3 = 106 ppma Fe"
  of R-3 is 105 ppmw, i.e. a ppmw/ppma slip as well; it is 38 ppma of all atoms.)
- **Mg.** The Mg, Fe and Ti "ppma" of the sapphire samples are **per Al2O3 formula unit**:
  12 ppmw Mg x 101.96 / 24.305 = 50 (the report's number, consistent, not a slip), 22 ppmw -> 92,
  160 ppmw Ti -> 340, 450 ppmw Fe -> 820. On the all-atom basis the report states elsewhere
  (1 ppma = 1.178e17 cm^-3) they are 5 times smaller (Mg 10, Ti 68, Fe 164). Ti_active is the same
  in both samples within 2 % on either basis (58 ppma of all atoms, 145 ppm of Al sites).
- **Sapphire coefficient.** "0.28 cm^-1 per ppma active Ti" is sigma = 2.4e-18 x 1.178e17 per ppma of
  all atoms; with the report's own 290 ppma (per formula unit) it predicts 81 cm^-1 at 578 nm
  against 8.2 tabulated; even with the correct 58 ppma of all atoms it predicts 16 cm^-1, twice the
  table. The tables support ~0.052 cm^-1 per ppm of Al sites (0.13 per ppma of all atoms,
  sigma_eff ~1.1e-18), which is the value the catalogue already had (0.05); that is what is fitted.

## 5. Pairing laws (two reports differ)

S-Met and S-Bas have equal Ti_active and a Fe ratio of 6.2. The net Fe2+-Ti4+ peak (578 nm minus the
500 nm continuum) differs by 1.2x: the law is **linear in [Ti]_active (`clustered_min`)**, not the
product [Fe]·[Ti]_eff of report 2 (which predicts ~6x). The 880 nm Fe2+-Fe3+ band scales as ~Fe^2
(ratio 39 against 38), which is a **`product`** law (z = 6). Both laws are now selectable per pair in
the data (`pairing = "clustered_min" | "product"`; `"clustered"` and `"random"` stay as aliases).
The Fe3+ 377/388/450 nm bumps are linear in Fe (ratios 5.6-7.0), so the quadratic term
(`quadratic_coeff`, research c2/c1) is kept at the research ratio but cannot be separated from the
linear term by two samples (<= 6 % effect at the highest Fe).

## 6. Fits (`data_version` 3)

Least squares in color terms: the spectral residual is the transmittance difference weighted by
xbar + ybar + zbar over 380-780 nm (so tails where T is large dominate, not the opaque peaks), per
polarisation, plus the CIELAB of the same sample (D65 and A where stated). Skewed bands and Marcus
IVCT shapes are 1-2 energy Gaussians (`AbsorptionBand::energy`). The U-band skew fit (1 Gaussian)
has a relative rms of 1.3 % (U E-perp-c) and 0.9 % (E-par-c) against the analytic skewed profile; the
Y bands 5.7 % / 5.3 % with one Gaussian (2.2 % / 1.8 % with two, not used: band budget). Where the
tabulated spectrum and its CIELAB disagree (section 2) the CIELAB is the co-target and the tables
keep the shape prior (bounds around the stated centres and widths); the largest dE00 over the
sample's colors is stored in every band as `fit_residual_de00`, `confidence = "secondary"`,
`source = "research-2026-10 §n, fitted to <fixture>"`.

| Chromophore (host) | Fixture(s) | Bands (energy Gaussians) | Residual dE00 vs measured CIELAB |
|---|---|---|---|
| Cr3+ ruby: U, Y (o and e), LMCT/Urbach window, R lines (merged), B lines (merged) | R-1, R-2, R-3 | 7 | R-1 4.3, R-2 7.0, R-3 2.9 / 3.0 (the Cr-only ruby D65 -> 3200 K change is kept < 5, criterion 14) |
| Fe3+ 450 / 388 / 377 + O-Fe CT edge, `quadratic_coeff` (corundum) | S-Met, S-Bas | 4 | S-Met E-perp 4.0, E-par 7.6; S-Bas 6.1 / 7.5 (A) / 4.9 (0.5 cm): the tables' own inconsistency is 4.1 / 7.7 / 6.7 / 8.4 / 5.1 |
| Fe2+-Ti4+ o and e (Marcus -> 2 Gaussians each) | S-Met, S-Bas | 4 | as above |
| Fe2+-Fe3+ 880 nm (`product`, z = 6) | S-Met, S-Bas | 2 | as above |
| V3+ (corundum, per ppma of all atoms) | corundum_v_0p26wt_v2o3 at 305 ppma | 2 | vs the table: 0.3 / 0.8 / 3.3 at 1 / 2 / 5 mm (no CIELAB given) |
| Cr3+ chrysoberyl alpha/beta/gamma + R line | alexandrite_natural_russian | 3 per ray + R | unpolarised (alpha+gamma)/2: D65 4.0, A 3.7; color change 27.7 against 30.1; single rays 1.2-8.6 |
| V3+ zoisite (natural, trichroic) incl. brown 450 / 680 nm bands (heated = bands removed) | tanzanite_heated, tanzanite_natural (+ heated alpha CIELAB) | 6 | heated beta 2.2, natural gamma 0.4, heated alpha 0.0 |
| Fe2+ forsterite (x_Fa) alpha/gamma, beta = mean | peridot_fo90 | 5 | alpha 1.2 (spectrum rms 0.8 cm^-1 over 380-700 nm) |
| [FeO4]0 545 nm (FWHM 3480) + 357 nm | amethyst_80ppmw_fe | 2 (+ the prior NIR band) | 1.8 |
| Citrine edge (annealed amethyst), 2 Gaussians | citrine_annealed_amethyst | 2 | 8.9 (spectrum and CIELAB disagree by 12.7) |
| Cr3+ beryl (emerald) 4T1, 4T2 (o and e), LMCT, R1 | emerald_colombian_muzo | 5 + B 476 + R | E-perp 2.2, E-par 1.9; V3+ held at the catalogue values (one composition cannot separate Cr and V) |

Notes on the fitted values: the ruby U band is anchored at 49-53 cm^-1/wt% (Dodd/Dubinsky 51.2),
the centre stays at 548-549 nm (bound 18250 cm^-1) and the width at 2200 cm^-1 (bound), i.e. the
CIELAB wants a narrower and bluer U band than either stated parameter set; the Urbach/LMCT window
band is an *effective* broad band (centre ~455 nm, FWHM 16000 cm^-1). The ruby Y bands moved to 432-435 nm and the E-par-c Y amplitude (67 cm^-1/wt%, against 37 E-perp-c) is a fit artefact: only the unpolarised CIELAB constrains the e-ray, so ruby pleochroism from this entry is not reliable. The E-par-c weights without
data (amethyst, beryl R line, V3+ in corundum, peridot beta) are priors and are marked as such in
the band sources.

## 7. Model and data changes

- `Mg` is a selectable element of corundum (`element_units`); the Fe2+-Ti4+ pair carries
  `compensator = { element = "Mg", subtract_from = "Ti4+" }`: [Ti]_active = max(0, Ti - Mg) on the
  same ppm_site basis, pair = min(Fe2+, Ti_active), linear.
- `quadratic_coeff` per band: peak = k c + q c^2 (resolve and the solver's linear model).
- `pairing = "clustered_min" | "product"` per pair (aliases `clustered`, `random`).
- Corundum default valence: Fe2+ 0.85 / Fe3+ 0.15 (was 0.1 / 0.9): the fitted sapphire samples need
  Fe2+ >= Ti_active, and the plan's blue recipe (Fe 1000 + Ti 100 ppm) only reaches h_ab <= 270
  at Fe2+ >= 0.82.
- Band budget: before the weakest bands are dropped, the cheapest pairs of energy Gaussians are
  merged (color-weighted moment matching, `ResolveWarning::BandsMerged` with its dE00). Research
  section 5 pruning in the data: CZ and glass Er3+ UV bands dropped, pyralspite Mn2+ 409/422/430
  merged, spinel Fe2+ 460/480 merged. At `conc_typical` (all elements at once) the merges of
  cubic_zirconia, garnet_pyralspite, glass_silicate and spinel measure dE00 0.25, 0.0001, 0.46 and 0.014
  (< 1; corundum 0.20 / 0.85, beryl 0.13, yag 0.24) and the four hosts are on the allow-list of
  criterion 5, which is re-enabled for all hosts.
- Tanzanite V `conc_max` 3000 -> 8000 ppm of Al sites (the stated 0.38 wt% V2O3 is 7670).
- Chrysoberyl `axis_mapping_confidence = "secondary"` (alpha ∥ c, beta ∥ b, gamma ∥ a).

## 8. Primary data batch 1 (2026-10-04, `data_version` 4)

Source: `docs/chromophores/research-2026-10-primary-data.md` and `docs/chromophores/primary-2026-10/`
(GIA Dubinsky et al. 2020 cross-sections, Caltech raw spectra). Its blocking finding: **no sample
anywhere has both a measured spectrum and a measured color**, so the "stated CIELAB" values of
sections 2-3 (which disagree with their own spectra by dE00 10-38) are not trustworthy targets. They
are not used any more; the recipes are fitted to the primary spectra only (color-weighted spectral
residual of section 6, no CIELAB co-target). Limits of this batch: the full GIA xlsx (1 nm, 200-1100 nm; Fe3+,
h•-Cr3+, h•-Fe3+ columns) could not be downloaded from the build environment (the proxy refuses the
CONNECT), only the 5 nm V3+ / Fe2+-Ti4+ digest and the 200-800 nm Cr3+ sheet of the zip are used; the Fe3+
and Fe2+-Fe3+ bands therefore stay the section-6 fits and the h• centres stay hidden (`suspect`).

### 8.1 What changed

| Host / chromophore | Fitted to | Bands (energy Gaussians) | Magnitude |
|---|---|---|---|
| corundum Cr3+ | GIA sample 1110 sigma, E_perp_c and E_par_c separately (`gia_cr3plus_sample1110_sigma.csv`) | E_perp_c: UV edge 345 nm, Y 404, U wing 538, U 560, R lines 693 (merged); E_par_c: UV edge 345, Y 400, U 538, R weight 0.13 | sigma x 3.154e20 Cr/cm^3 per wt% Cr2O3 (U peak 1.62e-19 cm^2 = 51 cm^-1/wt%, Y 1.7e-19 / 2.8e-19) |
| corundum V3+ | GIA xlsx V3+ sigma (`gia_v3plus_fe2plus_ti4plus_sigma_5nm.csv`) | 3 per polarisation (Y ~405, shoulder ~548, U ~580) | sigma x 1.175e17 per ppma of all atoms; **the xlsx sigma(580 nm, E_perp_c) = 3.4e-20 cm^2 is used, not the 1.0e-19 of the paper text** (the report-1 V table follows the text: 8.9e-20 at its label composition 706 ppma, 2.6x the xlsx; its 400 nm peak agrees with the xlsx) |
| corundum Fe2+-Ti4+ | GIA xlsx pair sigma | E_perp_c: 419 + 591 nm, E_par_c: 542 (14400 cm^-1 wide) + 727 nm | sigma 1.98e-18 cm^2 (E_perp_c, 580 nm), 1.25e-18 (E_par_c, 700 nm) x 4.7015e16 sites per cm^3 = **0.093 cm^-1 per ppm of Al sites** (0.229 per ppma of all atoms; data_version 3: ~0.05); pairs = min(Fe2+, Ti - Mg) as before (linear in available Ti, Fe in excess: confirmed by GIA 2020 / Emmett 2003) |
| chrysoberyl Cr3+ | Caltech GRR 874 beta (raw `ch874.a`) and gamma (5 nm digest of `ch874.b`) for the shapes; the report's alpha column for alpha (no Caltech alpha ray exists) | alpha, beta, gamma: 3 bands each (Y 4A2->4T1 and the 4T2 band with its shoulder) + the R line | gamma 4T2 peak = the report-table value per wt% (53.7; data_version 3 carried the CIELAB-inflated 65.4), one scale for beta and gamma (the Caltech slab is 1.05 wt% Cr2O3 equivalent) |
| tanzanite V3+ | Caltech GRR 1265 alpha, beta, gamma (405-780 nm; 380-405 nm from the GRR 2292 blue-zone digest) | 4 per ray: UV edge, mid band, 4T2 band, near-IR; the gamma ray carries the 440 nm brown band that heating bleaches | scaled by one factor (3.5) so that the beta 600 nm band keeps the data_version-3 value at 0.38 wt% V2O3 (7670 ppm of Al sites) |
| peridot Fe2+ | Caltech GRR 418 alpha, beta, gamma (raw) | 5 per ray: UV edge, 450 nm, 650 nm, M2 ~800 nm, M1 ~1060 nm | scale 1.0 at x_Fa = 0.10 (the report table and Caltech agree within 1.1-1.4x at 400-500 nm; no Fa analysis) |
| beryl Cr3+ (emerald) | Caltech GRR 3570 E_perp_c / E_par_c (5 nm digest) | 4 per polarisation (violet, blue, green shoulder, 4T2) + the 476 nm and R1 lines | E_perp_c 4T2 peak 9.45 per wt% (data_version 3: 9.73) |
| quartz [FeO4]0 | Caltech Anahi amethyst o-ray | 540 + 366 nm (data_version 3: 555 nm / FWHM 3484 was 41 % off the Caltech shape) | 0.879 per ppma kept |
| quartz citrine edge | the table spectrum alone (no Caltech file, the stated CIELAB disagrees with the table by dE00 12.7) | 3 bands | 10 ppma |
| fluorescence | primary-data report section 8 | emerald Fe quench c_half 0.0849 wt% FeO (= 660 ppm Fe), n 2 (derived estimate); ruby QE 0.83 is a coating-level value; NV QY 0.5-1 | `docs/fluorescence-plan.md` |

Model / code: `ray_axis_angstrom` (the axis length each ray vibrates along, replacing the a/b/c
letters: chrysoberyl alpha 4.43, beta 9.40, gamma 5.48 A; zoisite alpha 5.55, beta 10.0, gamma 16.2 A;
forsterite alpha 10.2, beta 6.0, gamma 4.76 A; chrysoberyl and tanzanite `axis_mapping_confidence =
"verified"`: the letter conflicts of the reports are cell settings, not physics); `fit_residual_de00` now
means dE00 against the color of the fitted primary spectrum. The Mg compensator is unchanged; the
**Si donor of GIA 2020 ([Ti]avail = Ti - max(0, Mg - Si)) is not modelled**: adding it as a selectable
element changes the solver's search space (6 free elements in corundum) and the seeded acceptance
tests, so Mg - Si is entered as Mg for now.

### 8.2 What was removed, and why

- **Corundum Cr3+ "LMCT / Urbach window" band** (454.55 nm, FWHM 16000 cm^-1, 10.8 per wt%) and the
  473 nm B-line band: they existed only to push b* up to the stated CIELAB (section 3). The 450-500 nm
  window is real (GIA sigma 1.3e-20 cm^2 at its 472 nm minimum; the R-1 table already is the GIA
  cross-section, see `research_ruby_r1_table_is_the_gia_cross_section...`). The B lines are a bump of
  <= 2e-21 cm^2 on the continuum and are contained in the fit.
- **Chrysoberyl Cr-O LMCT window-closing bands** (472-479 nm, 25-32 per wt% in all three rays): Caltech
  GRR 874 has A 0.19 at 490 nm against 2.0 at the Y peak (net of the 0.11-0.17 pedestal the window is open to
  3-5 % of the peak), the data_version 3 rays closed it to 80 %.
- **Emerald O->Cr LMCT band** (400 nm, FWHM 14000 cm^-1, 7.4 per wt%): it closed the 480-540 nm window 2.5x
  more than both the Caltech spectrum and the report table.
- **Tanzanite 656 nm "brown" band**: only in the report's table; the Caltech GRR 1265 gamma ray dips at 680 nm.
  The heating treatment now bleaches the 440 nm band only.
- The data_version 3 corundum Cr U / Y band shapes (single Gaussians of 2200 / 4500 cm^-1 with the
  stated-color pull: Y band at 432-435 nm, E_par_c Y 67 cm^-1/wt% "fit artefact").
- **Ruby plausibility claims that moved:** the ruby "Cr-only has no color change < 5" (GIA: 2.4-5.9, now < 7),
  the "chroma maximum near 0.25 wt%" (GIA: monotonic rise, saturating near 700 ppma*cm), the blue-recipe hue
  [220, 270] (GIA Fe-Ti: violet-blue, 291 degrees), the V color change "2.9-5.8" (GIA: 8-25) and the
  alexandrite-like recipe Cr 0.1 + V 2000 ppm (now a ruby-like 5.65; V 2000 + Cr 0.02: 13).

### 8.3 Fit residuals

dE00 of the recipes against the colors calculated by GIA (Appendix 2, illuminants D65 and A 2856 K, our color
code on the same sigma agrees with Appendix 2 to 0.7-2.3 at <= 375 ppma*cm: the data floor):

| ppma*cm | E_perp_c A / D65 | E_par_c A / D65 | unpolarised A / D65 | data floor (sigma vs Appendix 2, E_perp / E_par / mix) |
|---|---|---|---|---|
| 100 | 0.51 / 0.51 | 1.03 / 1.35 | 1.40 / 1.75 | 1.1-1.2 / 0.7-0.8 / 1.9-2.2 |
| 200 | 1.06 / 1.11 | 1.26 / 1.65 | 1.12 / 1.04 | 1.5-1.7 / 1.2-1.3 / 1.0-1.1 |
| 375 | 1.38 / 1.43 | 0.95 / 0.97 | 2.87 / 3.07 | 1.0-1.2 / 1.0-1.1 / 2.3-2.5 |
| 750 | 2.61 / 2.37 | 2.19 / 2.36 | - | 1.7-2.1 / 2.2-2.3 |
| 1500 | 3.56 / 3.17 | 4.65 / 3.95 | - | 2.7-3.4 / 3.6-4.6 |
| 3000 | 3.73 / 3.38 | 5.93 / 5.17 | - | 4.9-5.8 / 5.7-7.3 |

Against the cross-section itself (the part the fit controls): Cr3+ <= 1.4 up to 750 ppma*cm, 2.5 at 3000
(E_perp_c / E_par_c, D65 and A, 30-3000 ppma*cm); U and Y peaks within 6 % of the sigma; the 450 / 475 / 500 nm
window (450 / 475 / 500 nm) within 9 % of the sigma. V3+ <= 0.6 (10-1100 ppma*cm, E_perp_c / E_par_c); Fe2+-Ti4+ <= 0.5 (3-1000
ppma*cm of pairs), sigma(580 nm) within 1 %, E_par_c within 8 % at 400-780 nm, E_perp_c within 8 % at 400-660 nm
(the E_perp_c red tail beyond 670 nm is up to 50 % low at 780 nm: color-weight free). V color change D65 vs A
(recipe / sigma): E_perp_c 8.1 / 8.0 (110 ppma*cm), 13.9 / 13.8 (340); E_par_c 13.0 / 13.0, 24.8 / 24.8.

Shape of the other hosts against the Caltech spectra (color-weighted residual after the best scale and flat
offset, share of the weighted spread of the target; `recipe_shapes_follow_the_caltech_spectra`):

| Caltech spectrum | residual | color dE00 of the fit against its target |
|---|---|---|
| chrysoberyl GRR 874 beta / gamma | 3.8 % / 8.0 % | 0.9 / 0.9 (alpha: 1.1 against the table column) |
| peridot GRR 418 alpha / beta / gamma | 6.0 % / 6.7 % / 8.5 % | 0.13 / 0.20 / 0.11 |
| emerald GRR 3570 E_perp_c / E_par_c | 7.0 % / 16.9 % | 0.11 / 0.88 |
| tanzanite GRR 1265 alpha / beta / gamma (405-780 nm) | 2.2 % / 30.6 % / 14.5 % | 0.06 / 2.2 / 2.0 |
| amethyst (Anahi) o-ray | 4.6 % | 0.5 |
| GIA sigma, Cr3+ E_perp_c / E_par_c (scale 1.00 / 0.95) | 13.3 % / 8.5 % | see above |

Ray colors: alexandrite (0.32 wt% Cr2O3 + 0.15 Fe2O3, 2 mm) beta hue 104 (D65) / 77 (A), gamma 189 / 214,
alpha 227 / 292 (violet-blue under A; the alpha ray has no primary spectrum and keeps the report table's
shape); at the Caltech stone (1.05 wt% equivalent, 0.717 mm) beta and gamma are 5.8 / 8.2 dE00 from the Caltech
colors (74.1 / -9.6 / 39.6 and 51.8 / -57.5 / -13.4), which keep the file's own baseline. Tanzanite
(7670 ppm, 4 mm, D65): alpha 331 (red-violet), beta 255 (blue), gamma 135 (yellow-green, the brown band);
heated gamma 274 (blue-violet, the band removed; no numeric same-crystal data to check this). In data_version 3
the heated gamma ray was red-violet (316) and the alexandrite alpha ray green (144), gamma purple (357).

### 8.4 Findings that change sections 2-5

- **Fe2+-Ti4+ magnitude** (section 4): the GIA per-pair sigma gives 0.093 cm^-1 per ppm of Al sites; the sapphire
  tables S-Met / S-Bas imply 1.0e-18 / 1.2e-18 cm^2, 0.5-0.6 of it (GIA: non-Yogo stones deviate by 2-3x:
  annealing, Fe clustering). The recipes follow GIA; the tables' pair counts would be 50-60 % of Ti_active.
- **Blue sapphire hue**: the GIA Fe-Ti color is violet-blue (hue 281-313 for 20-100 pairs ppm over 2-5 mm,
  E_par_c 245-270); the recipe with its Fe3+ is 266.
- **Ruby**: purplish red up to ~1000 ppma*cm (b* positive only at >= 1500 in GIA); Caltech GRR 1843 (E_perp_c, no
  composition) has D65 b* -12.6 itself.
- **Tanzanite magnitude**: the Caltech GRR 1265 slab is ~3.5x weaker than the report-table magnitude at the
  header composition V2O3 0.35 (unit and method unstated); the earlier anchor is kept as instructed and flagged
  here.

### 8.5 Open items (primary-data report section 9, as they affect us)

1. No sample with analysed Cr, Fe, Ti, path length and both polarisations besides GIA 1110 (which has no Fe or Ti
   numbers): natural-ruby windows (Fe3+, Fe-Ti, h• centres) cannot be checked; the h•-Cr3+ / h•-Fe3+ columns of the
   GIA xlsx were not available (network).
2. No analysed flux / Verneuil rubies with 450-500 nm spectra.
3. **No measured color with its own spectrum for any gem**: absolute body colors cannot be validated, only
   against GIA's calculated colors.
4. No E_par_c Caltech spectrum or composition of GRR 1843; no Caltech chrysoberyl alpha ray and no Cr analysis
   (the alexandrite alpha ray and the Cr magnitude rest on the report table); the Fe3+ weights of chrysoberyl are a prior
   (Caltech GRR 171 suggests gamma strongest, beta weakest).
5. No numeric same-crystal before / after heating tanzanite spectra (the heated gamma color is unverified;
   Pluthametwisute 2020 Fig. 4 would need digitising).
6. No digital natural citrine spectrum (the citrine edge stays a fit to the table); no Fa analysis for GRR 418; no
   Cr / V analysis for any digital emerald spectrum (V3+ of beryl stays the report-table value); compositions of
   the Caltech sapphires 1020a / 3614 / 3615 and the 4.289 vs 3.00 mm question; GIA Appendix 1 compositions of the
   individual Fe-Ti samples.
7. Not obtained: McClure 1962 (the ~2630 cm^-1 ruby U width remains unexplained: GIA measures 2300-2390 for the 560
   nm band), Townsend 1968, Moon and Phillips 1994, Tsang and Ghose 1971, Sun et al. 2017 (alexandrite
   colorimetry; the best lead for the alpha ray).
8. Fluorescence: intrinsic yields (except NV), sideband shapes, excitation spectra, Fe-halving values (except the
   derived emerald estimate) not found.
9. The Si donor of the Fe-Ti pairing is not modelled (section 8.1).
