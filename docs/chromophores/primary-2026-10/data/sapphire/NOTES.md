# Blue sapphire (Fe2+-Ti4+) and V color-change corundum: measured-data notes

Tags: [primary: ref, loc] / [review: ref] / [derived: arithmetic shown] / [secondary: read through a summarising fetch tool, not verified against the paper itself].

## 1. Saved raw files (in /home/claude/data/sapphire/)

Fetch route: Caltech server through the browser pane (same-origin fetch). Shell is blocked for this host.
Fidelity: the numeric content is identical to the server file. Line endings were normalised from CRLF to LF and leading blanks on data lines were dropped. Check: line count, sum of column 2 and a position-weighted sum of column 1, computed in the browser on the live file and locally on the saved file, agree exactly for all three saved files.

| local file | URL | lines (non-blank) |
|---|---|---|
| sa1020a.am | http://minerals.gps.caltech.edu/FILES/Visible/corundum/sa1020a.am | 803 |
| sa1020a.cm | http://minerals.gps.caltech.edu/FILES/Visible/corundum/sa1020a.cm | 1619 |
| GRR3615_blue_Colombia_perp_c.asc | http://minerals.gps.caltech.edu/FILES/Visible/corundum/Corundum,%20GRR%203615,%20blue,%20Colombia%20perp%20c.asc | 1404 |

Not saved verbatim (time/token cost; only read the header and first lines, or the index page):
- GRR 3615 E||c: .../Corundum,%20GRR%203615,%20blue,%20Colombia.%20pol%20c.asc (22,537 chars; header says "xxx mm", so path length unknown, hence unusable for absolute units).
- GRR 3614 violet, 6.26 mm: E perp c .../Corundum,%20GRR%203614,%20Violet,%20Colombia%20perp%20c.asc (200 OK, header "Corundum, GRR 3614, Purple, Colombia / 6.26 mm, E perp c"). The index page links the E||c file as ".../Corundum,%20GRR%203614,%20Violet,%20Colombia,%20pol%20c.asc" (comma before "pol"; I did not fetch it).
- grr1798.a (Montana pale blue, 3.23 mm), sa1870 zoned Laos crystal (frameset index.htm -> sap1.htm / sap2.htm, not opened), Emmett JPG graphs (no numeric data).
- Index page: http://minerals.gps.caltech.edu/FILES/Visible/corundum/ (HTML read, not saved).

## 2. Sample metadata

### 2.1 GRR 1020a, "medium blue sapphire", locality unknown (Australia?) -- the only blue sapphire with both polarisations and a stated path length
- Source: Caltech Mineral Spectroscopy Server, Rossman group (collection GRR). Files sa1020a.am (E perp c) and sa1020a.cm (E||c).
- Composition: NOT PUBLISHED on the server or in the Caltech corundum reference list I could open. No Fe/Ti/Mg/Ga/V values exist for this sample in anything I found. Do not assign one.
- Path length: file headers both say 4.289 mm [primary: file header]. Index page says the spectra are "normalized to 3.00 mm thick" [primary: index page text]. Unresolved. The data cannot settle it (no thickness-independent feature). Both readings are carried below.
- Quantity/units: dimensionless absorbance, presumed decadic (log10), not stated on the server. Reflection loss: not stated; the spectra show a pedestal (see below) larger than a two-surface Fresnel loss, so assume NOT corrected.
- Ranges: E perp c 351.0-1599.2 nm (step ~1.2 nm, listed descending), E||c 350.36-1599.22 nm (~0.6 nm step).
- Selected values read from the saved files [primary: sa1020a.am/.cm]:
  - E perp c: A = 1.2249 at 580.2 nm; 1.4675 at 700.6 nm; 2.66 max near 870-889 nm; 0.9477 at 451.4 nm (sharp Fe3+ 450 nm feature); the lowest value in the visible is 0.5046 at 420.3 nm; local dip 0.6136 at 482.4 nm; 0.154 at 1599.2 nm.
  - E||c: A = 0.6025 at 580.25 nm; 0.8788 at 700 nm; 0.936 (local max) at 453.2 nm; minimum 0.5774 at ~500.3 nm; 0.92 plateau 745-790 nm; 0.167 at 1599 nm.
- Pedestal: both polarisations keep A = 0.5 to 0.6 at the transmission minimum (420-500 nm). A reflection loss of two sapphire faces is about 0.07 in A, so the rest is unexplained (scatter, inclusions, baseline offset, or the 3.00 mm normalisation). It must be subtracted or treated as unknown before any pair quantification.
- Decadic absorption coefficient alpha_dec = A/t at 580 nm, E perp c [derived]:
  - t = 0.4289 cm: 1.2249/0.4289 = 2.856 cm^-1
  - t = 0.300 cm: 1.2249/0.300 = 4.083 cm^-1
  - Napierian = 2.303 x decadic: 6.58 cm^-1 or 9.40 cm^-1. The two readings differ by 4.289/3.00 = 1.43.
- Illustration only, not a composition: if ALL of the 580 nm absorption were Fe2+-Ti4+ with sigma = 1.94e-18 cm^2 (Dubinsky 2020) and t = 4.289 mm, c = 6.58 / 1.94e-18 = 3.39e18 pairs/cm^3 = 3.39e18 / 1.178e17 = 28.8 ppma (all atoms) [derived]. This ignores the pedestal, Fe2+-Fe3+ absorption (the 880 nm band is the strongest feature here), and the Fe-Ti cross-section anomalies noted by Dubinsky 2020. It is an upper bound on pairs under that single assumption.
- Measured color: none.
- Verdict for P3: usable for band SHAPE in both polarisations and as a raw measured spectrum, not for absolute chromophore quantification (no composition, ambiguous thickness, uncorrected pedestal).

### 2.2 GRR 3615 "blue", Colombia
- Files: perp c saved; E||c file header says "xxx mm" (not saved).
- E perp c: header 4.44 mm [primary: file header], absorbance (decadic presumed), 289.03-1124.22 nm; saturated values of exactly 12.0000 below ~308 nm and spurious points in the UV. Reflection correction unstated.
- Spectrum is weak: A = 0.683 at 580 nm over a smooth baseline near 0.49-0.58 (min 0.4906 at 485.78 nm); no distinct 880 nm band. Composition not published. No color data.

### 2.3 GRR 3614 "violet/purple", Colombia, 6.26 mm
- Header "Corundum, GRR 3614, Purple, Colombia / 6.26 mm, E perp c" [primary]. Same 6.26 mm as the Dubinsky 2020 V-doped synthetic mentioned in Box A (116 ppma V3+, E perp c) but that is a coincidence of thickness only; nothing links the two. Do not merge.
- No composition found. Not saved.

### 2.4 Yogo Gulch sapphire (composition-only reference, no spectrum)
- Emmett et al. 2017, G&G 53(1) 42-47, Table 2, one typical Yogo wafer, SIMS (Caltech), ppma (atoms per 1e6 of all atoms): Mg 84.3 +/-1.32, Si 24.6 +/-0.44, Ti 79.7 +/-0.72, V 2.7 +/-0.05, Cr 2.2 +/-0.08, Fe 1130 +/-20, Ga 10.4 +/-0.3 [primary: Emmett 2017, Table 2]. Wafer 0.93 mm thick, c perpendicular to surface [primary: Fig 3 caption]. The paper prints no spectrum of that wafer.
- Derived in the paper: available Ti = Ti - (Mg - Si) = 79.7 - 59.7 = 20.0 ppma, SD 6.7 ppma including standards.
- Dubinsky 2020 used 14 Yogo samples for the Fe2+-Ti4+ peak cross-section; their individual spectra and compositions are in its Appendix 1 / figures, not available to me (the PDF has no appendix text). So no blue sapphire exists in the sources I could open that has spectra (both polarisations), composition, and path length together.

## 3. P4 answers

### 3a. Pair-concentration law: linear in available Ti, with Fe in excess; no Fe x Ti product law in the Gemological Institute of America (GIA) papers
- Emmett et al. 2003, G&G 39(2) 84-135 [primary: text near Fig 9 and Table 2 discussion]: with Fe much larger than Ti, Si, Mg, Be, Ti first charge-compensates Mg, the remainder pairs with Fe. Worked examples: 60 ppma Ti and no Mg gives 60 ppma pairs; 40 ppma Mg leaves 20 ppma; 60 ppma Mg leaves none. So [pairs] = [Ti] - [Mg] while positive. Same paper says the actual defect may contain other ions or defects besides one Fe and one Ti, citing Moon and Phillips 1994 [primary-as-cited; I could not read Moon and Phillips]. Townsend 1968 is credited with the IVCT assignment.
- Correction: Emmett et al. 2017 (G&G 53(1) 42-47): the Si donor level lies above Ti, so Si pairs with Mg first. Rule: Si pairs with Mg and Fe before Ti does [primary: p. 44]. For the Yogo wafer this gives 20.0 ppma available Ti [primary: Table 2 + text].
- Dubinsky, Stone-Sundberg, Emmett 2020, G&G 56(1) 2-28 [primary: p. 18-19]: [Ti]avail = [Ti] + [Si - Mg - Ni] when [Si - Mg - Ni] < 0, else [Ti]. Pair count is limited by whichever of Fe2+ or Ti donor is scarcer; they treat Fe as ample. The SD of [Ti]avail can exceed its value. For the Czochralski synthetics, pairs largely dissociate at growth temperature; a 750 C air anneal for up to ~1500 h raised the 580 nm absorption 2-2.5x [primary: p. 19]; so for synthetics Fe and Ti totals do not fix the pair count. They also report non-Yogo natural stones (high Fe, OH-rich) whose apparent Fe-Ti cross-sections are 2-3x larger, and say they do not know whether that is real (Fe-pair/cluster enhancement) or an artefact of surface-only SIMS in zoned stones [primary: p. 20].
- Townsend 1968 (Solid State Commun 6, 81-83) and Moon and Phillips 1994 (J Am Ceram Soc 77, 356-367): full text not accessible. My fetches of related papers gave no usable statement; I make no claim about whether either states a product law. A [Fe][Ti] law would be the expected mass-action result for unpaired random pairing at low concentration; GIA papers instead state a stoichiometric (Ti-limited) rule. Unresolved without Moon and Phillips / Townsend.
- Answer: the primary GIA literature supports linear in available Ti ([Ti]-[Mg], now plus [Si]) with Fe in excess; it does not support Fe x Ti. Caveat: the rule is for the equilibrium/natural case; synthetics are annealing-dependent.

### 3b. Absorption coefficient per Fe2+-Ti4+ pair
Definitions in Dubinsky 2020 Box A [primary: p. 5-7]: true (reflection-corrected, multiple reflections summed with a Sellmeier index) decadic absorbance A; alpha = 2.303 A/t is the NAPERIAN coefficient, cm^-1; sigma = alpha/c in cm^2, c in ions/cm^3. 1 ppma = 1.178e17 ions/cm^3 = 5 x N_A x rho / M with rho = 3.99 g/cm^3, M = 101.961 g/mol, i.e. parts per million of ALL atoms in Al2O3 (5 atoms per formula unit; 400,000 Al + 600,000 O, as in Emmett 2017 footnote 1). The ppma-cm areal density uses the same basis.

Cross-sections:
- Fe2+-Ti4+, E perp c, peak 580 nm: 1.94e-18 cm^2, +/-25% [primary: Dubinsky 2020 text p. 19-20 and Table A-1 (1.9e-18)]. The GIA data sheet gives 1.980e-18 at 580 nm [primary: sp20-corundum-chromophores-absorption-cross-section-data.xlsx, column "Fe2+ - Ti4+ sigma", E perp c, row 580].
- E||c: the paper text states the E||c peak at 700 nm but gives no number; the data sheet gives 1.256e-18 cm^2 at 703 nm (1.254e-18 at 700 nm) and 8.32e-19 at 580 nm [primary: same xlsx]. E perp c at 700 nm is 1.42e-18.
- Peak cross-sections are from the average of 14 Yogo wafers with band shapes from Fe-Ti synthetic crystals, not from one sample [primary: p. 19].

Conversion [derived] (rho and M as above, Al sites = 2 of 5 atoms):
- Per ppma of all atoms: alpha_N = 1.94e-18 x 1.178e17 = 0.2285 cm^-1 (E perp c, 580 nm); with 1.980e-18 it is 0.2332. Decadic: 0.2285/2.303 = 0.0992 cm^-1.
- E||c 703 nm: 1.256e-18 x 1.178e17 = 0.148 cm^-1 Napierian (0.0642 decadic).
- Per ppm of Al SITES (1 ppm = 1e-6 of Al ions): N_Al = 2 x 3.99 x 6.022e23 / 101.961 = 4.713e22 cm^-3, so 1 ppm(Al sites) = 4.713e16 cm^-3 = 0.400 ppma(all atoms) (i.e. 1 ppma(all) = 2.5 ppm(Al sites)). Then alpha_N = 0.2285 x 0.400 = 0.0914 cm^-1 per ppm(Al sites) and decadic 0.0397 cm^-1.
- Your check: "~0.05 cm^-1 per ppm of Al sites" matches the decadic value 0.040 (E perp c) to within the +/-25% uncertainty, or 0.091 Napierian. A figure of "0.28 cm^-1/ppma" is NOT reproduced from this source: I find 0.229 (Napierian, per ppma of all atoms) and 0.099 (decadic). The only 0.28 in the paper is the Table A-1 FWHM fraction for h.-Fe3+ (0.28 of the 400-700 nm range). If a different source gave 0.28, its basis must be stated before use.
- Other chromophores in the same dataset, for scale [primary: Dubinsky 2020]: Cr3+ 1.62e-19 cm^2 at 560 nm (E perp c), V3+ stated 1.0e-19 at 580 nm; but the data sheet gives V3+ E perp c = 3.4e-20 at 580 nm and a maximum of 9.8e-20 at 400 nm, and E||c 4.6e-19 at 403 nm. The text value and the sheet differ at 580 nm; treat V3+ numbers cautiously and check Figure 5.

### 3c. color-change corundum: spectra and measured color on the same sample
- I did NOT find a color-change sapphire with a measured spectrum and a measured CIELAB / D65-vs-A / delta-E in the primary papers I could open.
- Dubinsky et al. 2020 [primary: Fig 5, 6, p. 14-15]: a V3+-doped synthetic (116 ppma V3+ by SIMS, "ppma" as above, 6.26 mm, c-axis in plane so both polarisations; heated 1750 C in O2, 10 h to set V3+) was measured; cross-section spectra for E perp c and E||c are in Fig 5 and the xlsx. color circles are CALCULATED from the cross-sections for areal densities 110-3400 ppma-cm (Fig 6): E perp c grayish blue, E||c distinctly green under D65, with the E||c and E perp c + E||c circles differing strongly between A and D65 ("certainly qualify as color change when cut"). No numeric L*a*b* or delta-E is printed in the text. This is a model of a synthetic, not a measured natural stone's color.
- GIA Lab Notes, Spring 2019 (color-change Burmese sapphire, 5.68 ct, Fe 11-13 ppma, V 300 ppma, grayish violet in fluorescent light to purple-pink in incandescent light) [secondary: summarising fetch]: no spectrum values, no polarisation, no CIELAB.
- Fuping (China) color-change sapphire, Crystals 2022, 12, 463 [secondary: summarising fetch]: composition by LA-ICP-MS (ppm, units basis not verified: Fe 4848-6742, Cr 160-1250, V 36-146, Ti 21-188), UV-Vis lines/bands for FU-5, qualitative color change (purple-blue sunlight to purple-red incandescent); no CIELAB or delta-E, no thickness or polarisation.
- Analogue method only (garnet), Qiu and Guo, Minerals 2021, 11, 865 [secondary: abstract read through fetch tool]: ten pyrope-spessartine garnets, CIE 1931 2-degree observer, D65 vs A, delta-E*ab from 9.10 to 14.87; correlation of delta-E with the 574 nm Cr3+/V3+ band area. Not corundum. (The earlier suggestion "Liu et al." was not confirmed; authors are Qiu and Guo.)
- Not accessed / not found: Schmetzer and Bank 1980/1981 color-change corundum papers; Hughes "Ruby & Sapphire" color chapter (Emmett, Dubinsky, Hughes, Scarratt 2017); any Palke/Sun corundum colorimetry. Searches for corundum delta-E returned nothing numeric. Magnitude of delta-E from primary corundum measurements: none found; only the garnet analogue range above.

## 4. Looked for and not found
- Composition of GRR 1020a, 3614, 3615, 1798 (Caltech refs page is a bibliography; I read its first part only).
- Resolution of 4.289 mm vs "normalised to 3.00 mm" for GRR 1020a; absorbance definition and reflection-correction status of the Caltech spectra.
- Dubinsky 2020 Appendix 1 (sample list with compositions, thicknesses); the E||c peak value for Fe-Ti in the text (only in the xlsx).
- Moon and Phillips 1994 and Townsend 1968 full text, hence their stance on the concentration law.
- A measured CIELAB / delta-E for any color-change corundum with matched spectrum and composition.
- Emmett contributed graphs (JPG) not digitised.
- Remaining Caltech files unsaved (see section 1). Note: the shared browser pane was sometimes navigated to other sites between my calls, so each call checked location.host before fetching.
