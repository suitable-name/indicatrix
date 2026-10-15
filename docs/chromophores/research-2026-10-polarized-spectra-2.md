# Second research report: polarized absorption spectra of gem chromophores (German original, summarised)

User-supplied, 2026-10. A second answer to `docs/chromophore-research-query.md`.
**Important:** its ruby spectrum table is stated by the report itself to be
*synthesised* from energy-space Gaussians (Dubinsky et al. parameters) plus an Fe³⁺
baseline and B/R lines — it is a model output, not a measurement. Its sapphire table
is described as "spectrum of a moderately saturated sapphire" without sample
provenance. Use it as a cross-check of report 1 (`research-2026-10-polarized-spectra.md`),
not as independent measured data. Contradictions with report 1 are listed at the end.

## Premise

Bands are Gaussian (or Pekar-like, asymmetric) only in energy:
α(ν) = α_max·exp(−4 ln2 (ν − ν₀)²/FWHM²). Fitting Gaussians in λ underestimates red
absorption and pushes hue toward blue-violet. IVCT (Hush model) adds a (ν₀/ν) factor.

## A. Ruby (Cr³⁺:Al₂O₃)

Sources named: Dubinsky et al. (2020); Dodd, Wood & Barns (1964). σ(U, E⊥c, ~555 nm)
≈ 1.62×10⁻¹⁹ cm² confirmed.

Parameters for 0.25 wt% Cr₂O₃ (≈ 1700 ppmw, 7.87×10¹⁹ ions/cm³):

| Band | Pol. | Centre | FWHM (cm⁻¹) | Peak α_e (cm⁻¹) |
|---|---|---|---|---|
| U | E⊥c | 555 nm (18018 cm⁻¹) | 3350 | 12.75 |
| U | E∥c | 545 nm (18348 cm⁻¹) | 3100 | 6.60 |
| Y | E⊥c | 405 nm (24691 cm⁻¹) | 3600 | 19.60 |
| Y | E∥c | 390 nm (25641 cm⁻¹) | 3250 | 7.75 |

Window closure (450–500 nm): Fe³⁺ 377/388/450 nm (450 nm: 22222 cm⁻¹, FWHM 1500 cm⁻¹);
Cr³⁺ B lines 475/476 nm (FWHM ~180 cm⁻¹); O²⁻→Cr³⁺ LMCT Urbach tail reaching ~500 nm.

Stated result (D65, 2°, 0.25 wt%, 5 mm): L* ≈ 42.5, a* ≈ 65.2, b* ≈ 22.1, h_ab ≈ 18.7°,
C* ≈ 68.8 (maximum); at 1.0 wt% L* drops strongly, chroma falls.

Synthesised α_e (cm⁻¹), 0.25 wt% Cr₂O₃:

| λ | E⊥c | E∥c | λ | E⊥c | E∥c |
|---|---|---|---|---|---|
| 300 | 45.20 | 25.10 | 555 | 12.75 | 6.30 |
| 305 | 40.15 | 22.05 | 560 | 12.60 | 5.95 |
| 310 | 35.50 | 19.40 | 565 | 12.25 | 5.50 |
| 315 | 31.40 | 17.10 | 570 | 11.60 | 4.95 |
| 320 | 27.60 | 15.00 | 575 | 10.75 | 4.35 |
| 325 | 24.30 | 13.25 | 580 | 9.75 | 3.75 |
| 330 | 21.40 | 11.75 | 585 | 8.65 | 3.15 |
| 335 | 18.90 | 10.45 | 590 | 7.50 | 2.55 |
| 340 | 16.70 | 9.35 | 595 | 6.40 | 2.05 |
| 345 | 14.85 | 8.45 | 600 | 5.35 | 1.60 |
| 350 | 13.30 | 7.75 | 605 | 4.40 | 1.25 |
| 355 | 12.10 | 7.25 | 610 | 3.55 | 0.95 |
| 360 | 11.25 | 6.95 | 615 | 2.80 | 0.70 |
| 365 | 10.75 | 6.80 | 620 | 2.15 | 0.50 |
| 370 | 10.65 | 6.85 | 625 | 1.65 | 0.35 |
| 375 | 10.95 | 7.05 | 630 | 1.25 | 0.25 |
| 380 | 11.75 | 7.35 | 635 | 0.90 | 0.15 |
| 385 | 13.15 | 7.70 | 640 | 0.65 | 0.10 |
| 390 | 15.00 | 7.75 | 645 | 0.45 | 0.07 |
| 395 | 17.15 | 7.45 | 650 | 0.30 | 0.05 |
| 400 | 18.90 | 6.85 | 655 | 0.20 | 0.03 |
| 405 | 19.60 | 5.95 | 660 | 0.15 | 0.02 |
| 410 | 18.95 | 4.95 | 665 | 0.10 | 0.01 |
| 415 | 17.20 | 3.90 | 670 | 0.07 | 0.01 |
| 420 | 14.65 | 2.95 | 675 | 0.05 | 0.00 |
| 425 | 11.85 | 2.10 | 680 | 0.03 | 0.00 |
| 430 | 9.05 | 1.45 | 685 | 0.02 | 0.00 |
| 435 | 6.65 | 0.95 | 690 | 0.20 | 0.01 |
| 440 | 4.65 | 0.60 | 694 | 1.25 | 0.03 |
| 445 | 3.15 | 0.35 | 700 | 0.01 | 0.00 |
| 450 | 2.10 | 0.25 | 705 | 0.01 | 0.00 |
| 455 | 1.45 | 0.15 | 710–800 | 0.00 | 0.00 |
| 460 | 1.10 | 0.15 | | | |
| 465 | 0.95 | 0.20 | | | |
| 470 | 1.15 | 0.35 | | | |
| 475 | 1.85 | 0.65 | | | |
| 480 | 1.40 | 0.55 | | | |
| 485 | 1.45 | 0.65 | | | |
| 490 | 1.70 | 0.95 | | | |
| 495 | 2.20 | 1.35 | | | |
| 500 | 2.90 | 1.95 | | | |
| 505 | 3.90 | 2.65 | | | |
| 510 | 5.15 | 3.50 | | | |
| 515 | 6.55 | 4.40 | | | |
| 520 | 8.10 | 5.30 | | | |
| 525 | 9.70 | 6.05 | | | |
| 530 | 11.10 | 6.55 | | | |
| 535 | 12.15 | 6.75 | | | |
| 540 | 12.65 | 6.70 | | | |
| 545 | 12.75 | 6.60 | | | |
| 550 | 12.80 | 6.50 | | | |

## B. Blue sapphire

- Pairing rule: [Ti]_eff = max(0, Ti − Mg); **α ∝ [Fe_total]·[Ti]_eff** (product law).
  Mg > Ti → no Fe²⁺–Ti⁴⁺ blue.
- IVCT: E⊥c ~580 nm (17240 cm⁻¹), E∥c ~700 nm (14285 cm⁻¹).
  Hush shape: α(ν) = α_max (ν₀/ν) exp(−4 ln2 ((ν − ν₀)/FWHM)²).
- Fe²⁺–Fe³⁺ IVCT 880 nm (11360 cm⁻¹, FWHM 3800 cm⁻¹).
- Fe³⁺ 377 nm ∝ [Fe]² (pair), 388 nm ∝ [Fe] (linear).
- Basaltic blue sapphire, 800 ppmw Fe, 50 ppmw Ti, 0 Mg, 4.0 mm, D65:
  L* = 35.5, a* = −8.2, b* = −38.4, h_ab ≈ 257.9°.

α_e E⊥c (cm⁻¹), "moderately saturated sapphire" (no sample metadata):

| λ | α | λ | α | λ | α |
|---|---|---|---|---|---|
| 350 | 3.50 | 500 | 1.85 | 650 | 8.35 |
| 360 | 3.20 | 510 | 2.45 | 660 | 8.50 |
| 370 | 4.80 | 520 | 3.20 | 670 | 8.55 |
| 377 | 9.50 | 530 | 4.05 | 680 | 8.50 |
| 388 | 7.80 | 540 | 4.90 | 690 | 8.35 |
| 400 | 3.10 | 550 | 5.85 | 700 | 8.10 |
| 410 | 2.05 | 560 | 6.75 | 720 | 7.45 |
| 430 | 1.50 | 570 | 7.50 | 750 | 6.30 |
| 450 | 2.65 | 580 | 7.95 | 800 | 4.45 |
| 470 | 1.45 | 600 | 8.00 | 880 | 2.15 |

## C. color change

- V³⁺:corundum: ~400 nm (25000 cm⁻¹) and 570 nm (17543 cm⁻¹), FWHM ≈ 3200 cm⁻¹ each.
  D65: L* ≈ 45, a* ≈ 5, b* ≈ −18; illuminant A ("3200 K" as written): L* ≈ 40, a* ≈ 24,
  b* ≈ −4; ΔE often > 25 in fine stones.
- Alexandrite (~0.2 wt% Cr₂O₃, 4 mm), ⁴T₂ band:
  α (X ∥ a): 565 nm (17699 cm⁻¹), FWHM 2800; β (Y ∥ b): 560 nm (17857), FWHM 3000;
  γ (Z ∥ c): 590 nm (16949), FWHM 2700. D65 deep green (h ≈ 160°), A red (h ≈ 15°).

## D. Reference gems (qualitative)

- Tanzanite: axis mapping X ∥ c, Y ∥ b, Z ∥ a. V³⁺ bands 600 nm (16666 cm⁻¹) and
  740 nm (13513 cm⁻¹). Heating ~600 °C removes the brown Ti⁴⁺/Fe³⁺ CT.
- Peridot: Fe²⁺ ⁵T₂→⁵E at 1050 and 880 nm; the 880 nm band's short-wave flank blocks red
  from ~650 nm; trace Cr³⁺ adds weak 450/600 nm bands.
- Amethyst: [FeO₄]⁰ 545 nm (18350 cm⁻¹, FWHM ~3500 cm⁻¹) + 357 nm. Citrine: Fe₂O₃
  nanoparticles, edge rising from 600 nm into the UV.
- Emerald: ⁴T₂ 600 nm (16666 cm⁻¹) E⊥c, 630 nm E∥c; ⁴T₁ 430 nm (23255 cm⁻¹).
  ~5 mm: a* ≈ −45, b* ≈ 15.

## E. Concentrations and band reduction

- CZ: dopants (Nd³⁺, Er³⁺, Co²⁺) at 1.0–5.0 wt%. Er³⁺: model the strong peaks at
  380 and 520 nm only.
- Pyralspite: Fe²⁺/Mn²⁺ 10–30 wt%. Almandine Fe²⁺ 504, 520 nm dominant; spessartine
  Mn²⁺ 410, 422, 430 nm. Weak IVCT baselines as linear offsets.
- Co²⁺ spinel/glass: 0.01–0.05 wt%; the 550/580/594/624 nm structure can be merged
  into one broad Pekar-like band (590 nm, FWHM 4000 cm⁻¹).

## Contradictions with report 1 (coordinator)

| Topic | Report 1 | Report 2 |
|---|---|---|
| Ruby data provenance | tabulated as measured samples R-1/R-2/R-3 | explicitly synthesised from Gaussians |
| Ruby U FWHM (E⊥c) | 3180 cm⁻¹, skew b = 0.08 | 3350 cm⁻¹, symmetric |
| Ruby Y E⊥c peak, 0.25 wt% | ≈ 11.7 cm⁻¹ (46.78 per wt%) | 19.60 cm⁻¹ |
| Ruby Y E∥c centre | 404 nm | 390 nm |
| Ruby E∥c U peak, 0.25 wt% | ≈ 9.1 cm⁻¹ | 6.60 cm⁻¹ |
| Ruby CIELAB 0.25 wt%, 5 mm | L* 37.8, a* 69.5, b* 24.8, h 19.6° | L* 42.5, a* 65.2, b* 22.1, h 18.7° |
| Fe³⁺ 450 nm FWHM | 500–550 cm⁻¹ | 1500 cm⁻¹ |
| Fe–Ti pairing law | linear in [Ti]_active (Fe in excess) | product [Fe]·[Ti]_eff |
| Fe³⁺ 377 / 388 nm scaling | both linear + quadratic | 377 ∝ [Fe]², 388 ∝ [Fe] |
| Alexandrite axis mapping | α ∥ c, β ∥ b, γ ∥ a | α ∥ a, β ∥ b, γ ∥ c |
| Alexandrite α-ray ⁴T₂ | 572 nm | 565 nm |
| Tanzanite axis mapping | α ∥ a, β ∥ b, γ ∥ c | X ∥ c, Y ∥ b, Z ∥ a |
| Tanzanite V³⁺ bands | 525 nm (α), 585 + 740 nm (β) | 600 + 740 nm |
| Amethyst 545 nm FWHM | 5200 cm⁻¹ | 3500 cm⁻¹ |
| Emerald ⁴T₂ E⊥c | 605 nm | 600 nm |
| Er³⁺:CZ key lines | 487, 522 nm | 380, 520 nm |
| CZ dopant level | Co 0.01–0.08 wt%, Er 0.5–2.5 wt% | 1.0–5.0 wt% |
| Illuminant A | 2856 K | "3200 K" (A is 2856 K by definition) |

## Resolution

Integration of both reports (2026-10). Evidence is computed by the fixture tests in
`crates/indicatrix/src/optics/chromophore/reference_validation.rs` against the fixtures in
`crates/indicatrix/data/reference_spectra/` (report 2: `ruby2_synthesised_0p25wt.csv`,
`sapphire2_moderate_4mm.csv`). Rule order: (a) internal consistency of tabulated spectrum and
stated CIELAB under OUR colorimetry (D65 / A = Planckian 2856 K, 380-780 nm at 1 nm, CIE 1931
2 degrees, T = exp(-alpha L)); (b) the primary values both reports cite (Dodd 1964
k555 = 22.1 decadic/wt% = 50.89 Napierian; Dubinsky 2020 sigma = 1.62e-19 cm^2, which gives 51.2
through our number density, 0.6 % apart); (c) crystallographic convention; (d) physics.

Self-consistency of the two sets that report 2 adds: the synthesised ruby spectrum reproduces its
own stated CIELAB (L* 42.5, a* 65.2, b* 22.1, 0.25 wt%, 5 mm) with **dE00 26.1** (b* comes out
-46 instead of +22: the 450-500 nm window is open); the sapphire spectrum against its stated
4.0 mm D65 color (35.5, -8.2, -38.4) gives **dE00 9.5**, and the table carries no composition
(the section B color sample, 800 ppmw Fe / 50 ppmw Ti, is not stated to be the tabulated one).
Report 2's spectra are therefore cross-checks only; report 1's tables (which are no better for
ruby, dE00 16-26, but are labelled samples) supply the band amplitudes and the fits.

| # | Topic | Adopted | Why |
|---|---|---|---|
| 1 | Ruby data provenance | Neither set is measured data. Report 1 tables are used as samples with the CIELAB as co-target; report 2's synthesised table is not fitted. | (a) report 1 R-1/R-2/R-3 vs own CIELAB dE00 15.9/24.3/25.8/20.3, report 2 26.1: both fail. The CIELAB of both reports agree with each other (dE00 4.3 at 0.25 wt%), the tables disagree with both. A single extra broad absorption at 450-500 nm (the Cr-O LMCT Urbach tail) closes the gap, so the tables are missing it. |
| 2 | Ruby U FWHM E-perp-c | ~2200-2300 cm^-1 (fitted, lower bound 2200); neither 3180 (skewed) nor 3350. | (a) the half-width of the tabulated U band is 2350 cm^-1 in report 1 and 2630 in report 2 (test `ruby_report_1_and_2_tables_compared`): both stated widths are 20-30 % too wide for their own tables. (b) the peak (49-51 cm^-1/wt%) is anchored by Dodd/Dubinsky. |
| 3 | Ruby Y E-perp-c peak | 46.8 cm^-1/wt% (report 1) as prior; the CIELAB-weighted fit lands at 37 cm^-1/wt% (centre 435 nm, FWHM 4500) because the broad LMCT band and the Y band share the violet/blue absorption. | (a) report 1 own CIELAB is closer to its spectrum (24.3) than report 2's (26.1); the two Y peaks (11.65 vs 19.6 cm^-1 at 0.25 wt%) both reproduce their own table. No primary value exists for Y. |
| 4 | Ruby Y E-par-c centre | 404 nm (report 1) as prior; the CIELAB-weighted fit moves both Y bands to 432-435 nm (the data cannot fix the E-par-c band: no measured E-par-c color exists). | (a) the report-1 table peaks at 405 nm; the report-2 table at 390 nm is a model input, not data. The 450-500 nm window of both tables is too open for the stated CIELAB, which the fit closes by shifting/broadening Y; the fitted Y E-par-c amplitude (67 cm^-1/wt%) is unphysically large against E-perp-c (37) and is flagged as a fit artefact. |
| 5 | Ruby U E-par-c peak | 42 cm^-1/wt% (fit bound; E-par-c/E-perp-c 0.79, report 1: 0.71). | (a) the CIELAB fits push the e-ray U peak to the upper bound (0.71-0.79); report 2's 0.53 is never chosen. |
| 6 | Ruby CIELAB 0.25 wt%, 5 mm | Report 1 (37.8, 69.5, 24.8). | Measured-sample table vs a synthesised table's own output; the two differ by dE00 4.3, so either is a usable target; report 1 is also used for R-1, R-3. |
| 7 | Fe3+ 450 nm FWHM | ~500 cm^-1 (fitted 506). | (a) the S-Met/S-Bas tables show a spike on one 10-nm step (440/460 nm are < 45 % of the 450 nm value), which excludes 1500 cm^-1 (test `fe3_bumps_are_linear_in_iron`). |
| 8 | Fe-Ti pairing law | `clustered_min`, linear in [Ti]_active (selectable per pair in the data: `clustered_min` / `product`). | (a) S-Met and S-Bas have equal Ti_active (58 ppma of all atoms) and a Fe ratio of 6.2; their net 578 nm pair peaks differ by 1.2x, not 6x. The Fe2+-Fe3+ pair, by contrast, scales as Fe^2 (880 nm ratio 39 vs 38) and uses `product`. |
| 9 | Fe3+ 377 / 388 nm scaling | Both linear in Fe3+, with the research c2/c1 quadratic coefficient (1.8e-6/0.0051, 2.6e-6/0.0082) kept as `quadratic_coeff` (<= 6 % effect). | (a) net bump ratios S-Bas/S-Met are 6.9 (377) and 7.0 (388) for a Fe ratio of 6.2; "377 nm proportional to Fe^2" would give 38. |
| 10 | Alexandrite axis mapping | alpha ∥ c, beta ∥ b, gamma ∥ a (Pnma a = 9.404, b = 5.476, c = 4.427 Å), `axis_mapping_confidence = "secondary"`. | (c) report 2 gives no cell; its X ∥ a, Z ∥ c is the same physical assignment if its `a` is the 4.43 Å axis (Pbnm setting), so the reports do not contradict each other physically. (a) the stated colors do not discriminate (every ray color is dE00 > 12 from every table column), but the stated "unpolarised along b" color equals the XYZ mean of the stated alpha and gamma colors to dE00 3.4 (D65), not the three-ray mean (12.2): light along b vibrates along a and c. |
| 11 | Alexandrite alpha-ray 4T2 | 572 nm (report 1). | (a) the report-1 alpha column peaks at 575 nm; report 2's 565 nm belongs to a differently labelled axis (item 10). |
| 12 | Tanzanite axis mapping | `axis_mapping_confidence` stays `unknown`; the model uses the rays (alpha/beta/gamma colors of report 1's tables), which do not depend on the axis labels. | (c) the two reports assign a and c oppositely and no cell is given by report 2; neither can be confirmed here. |
| 13 | Tanzanite V3+ bands | Fitted 525 (alpha), ~588 and ~740 (beta) nm. | (a) the heated-beta table peaks between 585 and 605 nm (5.8 / 5.4 cm^-1): report 1's 585 and report 2's 600 nm bracket it. |
| 14 | Amethyst 545 nm FWHM | ~3500 cm^-1 (report 2). | (a) the E-perp-c amethyst table fits with FWHM 3480 (CIELAB dE00 1.8); 5200 cm^-1 would widen the 450-520 nm side beyond the table. |
| 15 | Emerald 4T2 E-perp-c | ~607 nm (fitted). | (a) the E-perp-c table peaks near 605-610 nm. |
| 16 | Er3+:CZ key lines | Visible lines (520/522 nm kept); UV 380 nm dropped. | (d) 4I15/2 -> 2H11/2 (522) and 4F7/2 (488) are the visible bands, 4G11/2 (379) the UV one; the color is set by the visible lines (research 1 section 5). |
| 17 | CZ dopant levels | Report 1 (Co 0.01-0.08 wt%, Er 0.5-2.5). | (b) Co2+ in CZ has 60 cm^-1 per wt% CoO: 1-5 wt% (report 2) would be opaque at 1 mm. |
| 18 | Illuminant A | 2856 K. | (d) A is 2856 K by definition. The validation uses Planckian(2856 K); the renderer's Incandescent preset (3200 K) is a separate, named illuminant used for the color-change readout. |

The V2O3 and Mg unit flags of report 1 and the sapphire coefficient are resolved in
`docs/chromophores/research-2026-10-validation.md`.
