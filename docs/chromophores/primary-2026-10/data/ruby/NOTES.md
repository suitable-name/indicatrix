# Ruby (Cr3+:Al2O3) measured absorption data - collection notes

Status: P1 NOT met (only 1 sample with analysed Cr + polarised E-perp-c and E-par-c; 0 natural rubies with composition + both polarisations). See "Not found".
Tags: [primary: ref, loc] = number read from the source; [derived: arithmetic] = my arithmetic on primary numbers; [review] = secondary source.
Fetched 2026-10-04 via the built-in browser (Caltech) and the browser plus pdf.js/SheetJS (GIA). The proxy blocks the shell.

## Sample A - Caltech GRR 1843 (natural ruby, Thailand) - E-perp-c only, composition NOT published
- Source: Caltech Mineral Spectroscopy Server, corundum page (revised 3-Jul-2022). http://minerals.gps.caltech.edu/FILES/Visible/corundum/Index.html
- Sample id / locality: GRR 1843, Chanthaburi, Thailand (file header). Page text says "viewed down the c-axis", so E-perp-c. [primary: Caltech page + file header]
- Composition: NOT given on the Caltech page or the Caltech corundum reference list. No Cr, Fe or Ti analysis found. (No LA-ICP-MS or microprobe is attached to this sample anywhere I could reach.)
- Path length: 0.787 mm [primary: file header]
- Polarisation: E-perp-c only. No E-par-c file exists: I probed ru1843c.a, ru1843a.a, ru1843_c.a, ru1843.c, ru1843.cm and ru1843.am, all HTTP 404. The page lists only the .a file.
- Units: column 2 is dimensionless decadic absorbance, A = -log10(I/I0), instrument output. The file does not say whether reflection loss was removed. The baseline at 800-1100 nm is ~0.215-0.23, which is far above the ~0.07 expected from two-surface Fresnel loss for corundum (n~1.76) [derived]. So there is an unexplained offset or scatter term of ~0.15, and reflection/offset are NOT corrected.
- Range and step: 330-1100 nm, ~0.6 nm step, 1296 lines including 2 header lines.
- color: none measured.
- Saved file (verbatim except CRLF line endings became LF; header lines and values unchanged): `/home/claude/data/ruby/caltech_GRR1843_ruby_Chanthaburi_Eperpc_0.787mm.a`
- Direct URL: http://minerals.gps.caltech.edu/FILES/Visible/corundum/ru1843.a
- Not saved (same sample, 0.758 mm, E-perp-c, 390-1100 nm, temperature series), URLs only: .../corundum1843_27C.a, .../corundum1843_300C.a, .../corundum1843_600C.a (same directory; 27C is the room-temperature run, 1197 lines; sibling at 0.758 mm differs from the 0.787 mm piece).
- Features read from the saved file (A is the raw column):
  - A(413.7 nm) = 1.7965, the band peak at ~410 nm.
  - A(560.6 nm) = 1.6257, the band peak at ~560 nm.
  - A(450.2) = 0.7545, A(474.1) = 0.3864 (minimum between the bands), A(475.25) = 0.3925, A(500.3) = 0.5021.
  - Narrow features: R-line region at 692-694 nm, B-lines at ~468-476 nm. UV: A rises from 0.67 at 330 nm, passes a shoulder at ~341 nm (0.78), a minimum at 355 nm (0.743), then the U band. The data stop at 330 nm, so the UV edge is not covered.
  - [primary: ru1843.a]
- alpha = 2.303 A / t, t = 0.0787 cm, so the factor is 29.26 cm^-1 per unit A. Napierian, not reflection- or offset-corrected [derived]:

| wavelength | alpha raw | alpha with 0.217 subtracted (offset removed by me, not a rigorous correction) |
|---|---|---|
| 450.2 nm | 22.1 cm^-1 | 15.7 cm^-1 |
| 475.25 nm | 11.5 cm^-1 | 5.1 cm^-1 |
| 500.3 nm | 14.7 cm^-1 | 8.3 cm^-1 |

- Sanity check only, NOT a composition: if Cr3+ has the Sample B cross section (a cross-sample assumption), the 560 nm net alpha of ~41 cm^-1 implies roughly 2000 ppma-of-all-atoms Cr, i.e. ~0.8 wt% Cr2O3 [derived, indicative]. Do not use it as the composition of GRR 1843.

## Sample B - GIA / Dubinsky et al. 2020, sample 1110 (synthetic Czochralski ruby, laser-grade) - E-perp-c AND E-par-c, composition measured
- Source: Dubinsky E.V., Stone-Sundberg J., Emmett J.L. (2020) "A quantitative description of the causes of color in corundum", Gems & Gemology 56(1), 2-28. PDF: https://www.gia.edu/dam/migrated-assets/docs/doc1/sp20-corundum-chromophores.pdf
- Data: https://www.gia.edu/dam/migrated-assets/docs/doc1/sp20-corundum-chromophores-absorption-cross-section-data.xlsx (the paper's footnote link under /doc/ returns 404; the file is under the /dam/migrated-assets/docs/doc1/ path above).
- Appendix 1 (sample list): https://www.gia.edu/dam/migrated-assets/docs/doc1/sp20-corundum-chromophores-appendix1.xlsx
- Appendix 2 (L*a*b*): https://www.gia.edu/dam/migrated-assets/docs/doc1/sp20-corundum-chromophores-appendix2.xlsx
- Sample: Cr chromophore wafer 1110, St. Gobain, Czochralski, boule SS75, wafer plane parallel to c, SIMS analysed (Be, Mg, Si, Ti, V, Cr, Fe, Ga), spectra E-perp-c and E-par-c. [primary: Appendix 1]
- Appendix 1 also lists 1108 (Cr, boule SS29) and 1111 (Cr, boule SS74), both plane perpendicular to c, E-perp-c only, no analysis. Which wafer carries the quoted 134 ppma is not stated in the text. The text says the Cr cross section is "from the synthetic ruby with lowest 226 nm absorption", chosen from 4 boules. The Fig 2 caption and Appendix 1 point to 1110 as the E-perp-c/E-par-c SIMS sample. Treat the sample-to-number mapping as 90% probable, not certain.
- Composition: Cr = 134 ppma +/- 7.6% (SIMS, calibrated with ion implants; detection limit 0.002 ppma) [primary: paper, "The Cr3+ chromophore" section]. The text calls the sample "high-purity" with Cr only. A numeric Fe or Ti value is not printed for this sample in the text. The paper says Fe/Ti are chosen to be minimal and says nothing more numeric (the web summary said "below quantification limits"; I did not see that sentence in the PDF text).
- Basis of ppma: the paper defines 1 ppma = 5 A rho / M = 1.178e17 ions/cm^3, with rho = 3.99 g/cm^3, M = 101.961 g/mol [primary: Box A]. 5 is the number of atoms per Al2O3 formula unit, so ppma means parts per million of ALL atoms (Al + O + dopant), not of cation sites.
- Conversions [derived, with the paper's density 3.99]:
  - 134 ppma = 1.58e19 Cr/cm^3.
  - Cr2O3 = 0.050 wt% (1.58e19 / 6.022e23 x 75.995 g/mol / 3.99 g/cm^3 = 4.99e-4).
  - Cr/(Al+Cr) = 3.35e-4, i.e. 0.0335 at% of the cation sites (cation density 4.714e22/cm^3).
  - This is at the LOW edge of your 0.05-1 wt% window.
- Path length: NOT stated for 1110 anywhere I could read (text, Appendix 1, cross-section xlsx). The published quantity is the thickness-independent cross section, so the path length is not needed to reuse the data but it is missing from the metadata.
- Units: absorption cross section sigma = alpha / c, in cm^2 per Cr ion, with alpha = 2.303 A / t (Napierian, cm^-1) and A the TRUE decadic absorbance corrected for multiple reflections between the two polished faces (Sellmeier index of corundum, Tatian 1984) [primary: Box A]. Resolution 1.5 nm (Hitachi 2910 modified for rotatable polarisation), data at 1 nm intervals, 200-1100 nm.
- Measured color: none measured by an instrument. Appendix 2 gives CALCULATED CIE L*a*b* from these cross sections for illuminants A (2856 K) and D65 at set areal densities; the numbers are in the table below. A computed, not measured, color. The "spectrum multiplier" column is inconsistent with the stated ppma-cm labels (e.g. 100 ppma-cm lists 5.89e18, and 375 lists 2.356e19, but 100 x 1.178e17 = 1.178e19); I could not resolve this, so re-derive any use of the areal density yourself.
- Saved file (200-800 nm only; wavelength descending as in the sheet; columns: wavelength nm, Cr3+ sigma E-perp-c cm^2, Cr3+ sigma E-par-c cm^2; CSV text produced by SheetJS from the xlsx; the xlsx itself cannot be saved with the Write tool; the sheet continues to 1100 nm): `/home/claude/data/ruby/GIA_Dubinsky2020_sample1110_Cr3+_cross_section_200-800nm.csv`. The file has no header line; add: wavelength_nm, sigma_perp_c_cm2, sigma_par_c_cm2.

Appendix 2, Cr3+ rows (calculated L*, a*, b*; columns: areal density ppma-cm | E-perp-c A | E-perp-c D65 | E-par-c A | E-par-c D65 | both A | both D65):
- 100: (72.69, 42.42, -22.05) | (70.46, 37.74, -23.77) | (89.61, 18.98, 6.72) | (87.94, 12.45, 7.64) | (80, 32.04, -9.48) | (78.05, 25.96, -9.92)
- 200: (59.87, 61.51, -27.72) | (55.91, 57.02, -31.67) | (81.64, 34.32, 8.32) | (78.53, 26.39, 8.02) | (67.93, 51.69, -14.22) | (64.34, 44.64, -16.54)
- 375: (49.20, 71.72, -23.68) | (43.29, 68.84, -30.64) | (70.84, 54.55, 10.57) | (65.45, 47.68, 7.06) | (55.78, 68.31, -13.22) | (50, 62.74, -18.87)
- 750: (40.8, 72.12, -8.57) | (33.33, 69.94, -18.47) | (60, 70.84, 19.47) | (52.09, 67.15, 11.41) | (46.36, 73.79, -0.51) | (38.67, 70.1, -9.7)
- 1500: (33.13, 67.29, 14.87) | (24.72, 64.82, 2.59) | (51.26, 76.61, 38.73) | (41.72, 75.1, 26.76) | (38.5, 71.64, 21.91) | (29.65, 68.84, 9.75)
- 3000: (25.46, 59.24, 37.27) | (17.25, 56.37, 23.78) | (43.65, 75.38, 62.07) | (33.68, 74.32, 47.31) | (31.01, 65.41, 45.21) | (22.1, 62.79, 31)
[primary: Appendix 2, sheet "single chromophores"; calculated]

Key cross-section numbers for sample 1110, read from the saved CSV (sigma in cm^2 per Cr ion; alpha = sigma x 1.5785e19 cm^-3, derived):

| wavelength | sigma E-perp-c | sigma E-par-c | alpha E-perp-c | alpha E-par-c |
|---|---|---|---|---|
| 450 nm | 3.074e-20 | 3.695e-20 | 0.485 cm^-1 | 0.583 cm^-1 |
| 475 nm | 1.472e-20 | 1.178e-20 | 0.232 cm^-1 | 0.186 cm^-1 |
| 500 nm | 3.630e-20 | 1.964e-20 | 0.573 cm^-1 | 0.310 cm^-1 |
| 560 nm | 1.621e-19 | 4.859e-20 | 2.56 cm^-1 | 0.77 cm^-1 |
| 300 nm | 4.307e-21 | 3.011e-21 | | |
| 202 nm (UV edge, maximum) | 7.386e-19 | 7.285e-19 | | |

- Alpha values: [derived from primary sigma x concentration; Napierian, reflection-corrected, wt%/ppma basis as above].
- The minimum between the two bands is at ~472-474 nm: sigma ~1.2e-20 (perp) and ~0.99e-20 (par); B-lines near 470-476 nm sit on it.
- Band peaks, sigma [primary: CSV]:
  - 409-411 nm band: perp 1.653e-19; par peak at 398 nm, 2.985e-19.
  - 558-559 nm band: perp 1.623e-19; par peak at 545-548 nm, 5.503e-20.
  - The paper itself quotes 1.62e-19 +/- 7.6% for E-perp-c at 560 nm [primary: p.9].
  - The paper cites Maiman et al. 1961 for ~1.8e-19 (E-perp-c) [review].

## Item 3 - what absorbs at 430-500 nm
- Cr3+ own bands. The short-wave band's long-wavelength flank and the long-wave band's short-wave flank overlap and bottom out at ~472-474 nm, the transmission window of ruby. Values above for sample 1110. By the cross-section data, the "Cr3+ only" alpha at 475 nm is ~0.23 (perp) and ~0.19 (par) cm^-1 for 134 ppma; scale linearly with Cr (the paper says the cross section does not depend on host: same in synthetic ruby, natural ruby and purple sapphire) [primary: p.9]. Not demonstrated to be linear at 1000s ppma; the paper's own concentration test is only on Fe3+.
- O2- -> Cr3+ charge transfer: the cross section in sample 1110 rises to ~7.4e-19 cm^2 at 200-202 nm and the paper puts the first Cr3+ charge-transfer band below ~226 nm [primary: p.8]. At 300 nm sigma is already 4.3e-21 (perp), 7x lower than at 450 nm, so by this dataset the charge-transfer tail contributes little at 430-500 nm [derived]. The paper does not discuss the tail beyond that.
- Fe3+: weak. The paper puts the peak at 450 nm (Fe3+-Fe3+ pair) at no more than 2.5e-20 cm^2 and says the pair band alpha grows as ~C^1.5 with total Fe3+ [primary: p.16-17], so the cross section is concentration dependent (rises ~ sqrt(C)). The xlsx Fe3+ columns give 2.244e-20 (perp) and 1.854e-20 (par) at 450 nm, 6.8e-21 and 5.5e-21 at 475 nm, 2.25e-21 and 1.48e-21 at 500 nm, but the xlsx does not say which Fe concentration those correspond to. The paper's own conclusion is that Fe3+ is "a very weak chromophore". Other Fe3+ lines: 377, 388 nm [primary: p.17].
- color centres: h.-Cr3+ (trapped hole + Cr3+, orange) and h.-Fe3+ (yellow), seen in Mg- or Be-containing stones. They are by far the strongest chromophores, peak sigma ~1.3e-17 cm^2 each [primary: Table A-1], and the xlsx gives sigma(450 nm, perp) = 1.308e-17 (h.-Cr3+) and 9.142e-18 (h.-Fe3+). Concentrations of these defects are ~0.8 ppma Mg / 12.6 ppma Cr for sample 991 [primary: paper summary; I did not open the sample-991 section]. Per Emmett: natural stones carry Mg; Be-diffused stones can be orange.
- Natural versus synthetic: the paper does not compare flux or Verneuil ruby. It uses Czochralski wafers and natural Yogo sapphire for Fe3+/Fe2+-Ti4+.
- Not located in primary sources: separate numbers for flux-grown or Verneuil ruby at 450-500 nm.
- Fe2+-Ti4+ intervalence band (sigma 1.9e-18, Table A-1) also absorbs here in purple/violet sapphires and in Ti- and Fe-bearing natural rubies; it is a different mechanism from the above but belongs on the list for natural stones [primary: Table A-1; I did not check band shape at 450 nm beyond the xlsx column: Fe2+-Ti4+ sigma(450) = 4.538e-19 (perp), 4.904e-19 (par)].

## Item 4 - contradictions
Nomenclature note: the GIA paper calls the ~560 nm band "U" and the ~400 nm bands "Y" [primary: p.9]. The established ruby convention is the other way round (U = violet ~410 nm, Y = yellow-green ~560 nm). The quoted FWHM claims must be matched to a band by wavelength, not by letter.

Computed by me from the saved sample-1110 data (half maximum of each peak, linear interpolation, no baseline subtraction, no deconvolution of overlap) [derived]:

| band | polarisation | peak | FWHM |
|---|---|---|---|
| ~410 nm band | E-perp-c | 409 nm (24 450 cm^-1) | ~54.7 nm = 3290 cm^-1 |
| ~410 nm band | E-par-c | 398 nm | ~48.5 nm = 3020 cm^-1 |
| ~560 nm band | E-perp-c | 558 nm (17 920 cm^-1) | ~70.7 nm = 2300 cm^-1 |
| ~560 nm band | E-par-c | 545 nm | ~70.3 nm = 2390 cm^-1 |

- The ~410 nm band E-perp-c result of ~3290 cm^-1 falls inside the 3180-3350 cm^-1 claim. The ~560 nm band is narrower (~2300-2390), not 2630. I did not find any measured number of ~2630; it could come from a different baseline or an overlap-deconvolved fit.
- The 410 nm band's FWHM is overlap- and edge-dependent (the long-wave flank of the 410 nm band lies on top of the rising UV edge, and the Y-band flank sits under it near 440-470 nm); numbers other than my half-height read are fit-model dependent.
- The 410 nm half-height of the natural ruby GRR 1843 is unreliable because the baseline there is unknown (UV absorption at 350 nm is already A = 0.74); a flat 0.217 baseline subtraction gives a width of ~3800 cm^-1 [derived, not trusted].
- Y-band (560) / U-band (410) peak ratio:
  - Sample 1110 E-perp-c: 1.623e-19 / 1.653e-19 = 0.98 [derived, primary].
  - Sample 1110 E-par-c: 5.503e-20 / 2.985e-19 = 0.18 [derived, primary].
  - GRR 1843 E-perp-c raw A: 1.6257 / 1.7965 = 0.905 (0.892 after subtracting a 0.217 flat offset) [derived; a lower value than the Cr-only synthetic, consistent with additional absorption under the 410 nm band in the natural stone, but the cause is not established].
- The UV claims (3180-3350 vs ~2630) are NOT resolved by a primary spectrum with both ends; McClure 1962, Wood et al. 1963 and Fritsch and Rossman 1987-88 were not obtained (see below).

## What I looked for and did NOT find
- An E-par-c spectrum of Caltech GRR 1843, and any composition, color or LA-ICP-MS for it (not on the Caltech corundum page, reference list or the unlisted file names I probed).
- Caltech Emmett "high-quality absolute intensity" graphs (Cr, V, Fe, O-/Cr3+, O-/Fe3+, Fe2+-Ti4+): only .jpg images are linked (Emmett_Cr_1110.jpg etc.; the Cr one is sample 1110 again), no numeric data. Caltech also has violet/blue corundum GRR 3614 and 3615 (Colombia) as E-perp-c and E-par-c files (.asc), but those are not ruby and carry no analysis; I did not save them.
- A second and third ruby with analysed Cr, Fe, Ti, a path length, both polarisations and a UV edge: none found on Caltech, none in the GIA paper (its other Cr entries 1108 and 1111 have no analysis and are E-perp-c only).
- A tabulated LA-ICP-MS + polarised UV-Vis ruby set (Mozambique, Burmese, flux, Verneuil) in Gems & Gemology or elsewhere: web search returned nothing usable. The MDPI Crystals 2023 paper on heat-treated Madagascar/Mozambique/Tanzania rubies gives no numeric Cr/Fe table, path lengths, polarisation or band widths [web summary, not verified against the full text].
- Measured CIELAB of any real ruby sample (only the calculated Appendix 2 colors).
- McClure 1962 J Chem Phys 36, 2757; Wood et al. 1963; Fritsch & Rossman 1987-88 Gems & Gemology "An update on color in gems" part 1; Peretti et al. 1995 (Mong Hsu); Bosshart 1982: not retrieved (paywalled or not reached in the time available).
- Numeric alpha at 450/475/500 for natural versus flux versus Verneuil ruby, and the dependence on Fe, beyond the Fe3+ cross-section columns above.
- The Dubinsky sample path length for 1110, and the Fe and Ti values of the Cr sample.
