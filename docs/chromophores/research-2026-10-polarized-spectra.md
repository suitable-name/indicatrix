# Quantitative polarized absorption spectra and physical color modelling of gemstone chromophores

User-supplied research result (2026-10), answering
`docs/chromophore-research-query.md`. Transcribed verbatim from the delivered text;
tables re-laid as Markdown. Sources are named in the text; individual numbers are not
individually cited, and the delivery contained citation artefacts ("[cite: 14]").
Treat every number as `secondary` until checked (see the integration notes at the end).

## Conventions

- Uniaxial: o-ray E⊥c, e-ray E∥c. Biaxial: n_α < n_β < n_γ along X, Y, Z.
- Decadic absorbance A₁₀ = −log₁₀(I/I₀). Napierian α = ln(10)·A₁₀/L (cm⁻¹, L in cm);
  T = exp(−α·L).
- σ(λ) = α(λ)/N (cm² per absorber), N in absorbers/cm³.
- Corundum: R-3c, V_cell = 254.81 Å³, Z = 6, ρ = 3.989 g/cm³, M = 101.961 g/mol;
  4.712×10²² Al sites/cm³; N_tot = 1.178×10²³ atoms/cm³; 1 ppma (all atoms) =
  1.178×10¹⁷ ions/cm³. 1.0 wt% Cr₂O₃ → N_Cr = 3.161×10²⁰ cm⁻³.
- CIELAB from T(λ)·S(λ) with CIE 1931 2°, 380–780 nm, Y_n = 100.
  D65 white (95.04, 100.00, 108.88); illuminant A (2856 K) white (109.85, 100.00, 35.58).

## 1. Cr³⁺ in corundum (ruby)

Sources: Dodd, Wood & Barnes (1964); Powell (1998); Dubinsky, Stone-Sundberg &
Emmett (2020). Samples: R-1 synthetic Verneuil 0.050 wt% Cr₂O₃ (1.58×10¹⁹ cm⁻³,
134 ppma), L = 1.00 cm; R-2 synthetic flux 0.250 wt% (7.90×10¹⁹, 671 ppma), L = 0.50 cm;
R-3 natural Mogok 0.720 wt% Cr₂O₃ (2.28×10²⁰, 1932 ppma) + 0.015 wt% Fe₂O₃ (106 ppma),
L = 0.25 cm.

Line shape (skewed Gaussian in wavenumber):
α(ν̃) = α_peak · exp(−ln2 · [ln(1 + 2b(ν̃ − ν̃₀)/Δν̃) / b]²),
ν̃₀ centre (cm⁻¹), Δν̃ FWHM (cm⁻¹), b skew.

| Feature | Transition | Pol. | λ₀ (nm) | ν̃₀ (cm⁻¹) | FWHM (cm⁻¹) | b | σ_peak (10⁻¹⁹ cm²) | α_peak per 1.0 wt% Cr₂O₃ (cm⁻¹) |
|---|---|---|---|---|---|---|---|---|
| U | ⁴A₂→⁴T₂ | E⊥c | 555.0 | 18018 | 3180 | 0.08 | 1.62 | 51.21 |
| U | ⁴A₂→⁴T₂ | E∥c | 548.0 | 18248 | 3050 | 0.06 | 1.15 | 36.35 |
| Y | ⁴A₂→⁴T₁(⁴F) | E⊥c | 408.0 | 24510 | 3450 | 0.12 | 1.48 | 46.78 |
| Y | ⁴A₂→⁴T₁(⁴F) | E∥c | 404.0 | 24752 | 3300 | 0.10 | 1.30 | 41.09 |
| R₁ | ⁴A₂→²E | E⊥c | 694.3 | 14403 | 18 | 0.00 | 0.28 | 8.85 |
| R₂ | ⁴A₂→²E | E⊥c | 692.8 | 14434 | 16 | 0.00 | 0.18 | 5.69 |
| B lines | ⁴A₂→²T₂ | E⊥c | 468.5, 475.1, 476.5 | 21345, 21048, 20986 | 40 | 0.00 | 0.04 | 1.26 |

U-band check: Dodd et al. decadic k₅₅₅ = 22.1 ± 0.3 cm⁻¹ per 1.0 wt% Cr₂O₃ (E⊥c) →
α = 50.887 cm⁻¹ → σ = 1.610×10⁻¹⁹ cm²; Dubinsky et al. (2020) SIMS σ = 1.62×10⁻¹⁹
± 7.6 % at 560 nm (E⊥c).

Napierian α (cm⁻¹):

| λ (nm) | 0.05 E⊥c | 0.05 E∥c | 0.25 E⊥c | 0.25 E∥c | 0.72 E⊥c | 0.72 E∥c |
|---|---|---|---|---|---|---|
| 350 | 0.62 | 0.54 | 3.10 | 2.70 | 9.42 | 8.18 |
| 360 | 0.78 | 0.69 | 3.90 | 3.45 | 11.85 | 10.45 |
| 370 | 1.10 | 0.97 | 5.50 | 4.85 | 16.70 | 14.70 |
| 380 | 1.55 | 1.36 | 7.75 | 6.80 | 23.55 | 20.65 |
| 390 | 1.98 | 1.74 | 9.90 | 8.70 | 30.10 | 26.40 |
| 400 | 2.25 | 1.98 | 11.25 | 9.90 | 34.20 | 30.10 |
| 405 | 2.33 | 2.05 | 11.65 | 10.25 | 35.42 | 31.15 |
| 410 | 2.31 | 2.01 | 11.55 | 10.05 | 35.10 | 30.55 |
| 415 | 2.20 | 1.90 | 11.00 | 9.50 | 33.45 | 28.90 |
| 420 | 2.05 | 1.76 | 10.25 | 8.80 | 31.15 | 26.75 |
| 430 | 1.52 | 1.28 | 7.60 | 6.40 | 23.10 | 19.45 |
| 440 | 0.95 | 0.78 | 4.75 | 3.90 | 14.85 | 11.95 |
| 450 | 0.52 | 0.41 | 2.60 | 2.05 | 8.52 | 6.35 |
| 460 | 0.28 | 0.21 | 1.40 | 1.05 | 4.60 | 3.28 |
| 468.5 | 0.25 | 0.16 | 1.25 | 0.80 | 4.05 | 2.58 |
| 470 | 0.21 | 0.14 | 1.05 | 0.70 | 3.48 | 2.22 |
| 475 | 0.19 | 0.12 | 0.95 | 0.60 | 3.12 | 1.90 |
| 480 | 0.20 | 0.12 | 1.00 | 0.60 | 3.20 | 1.92 |
| 490 | 0.26 | 0.16 | 1.30 | 0.80 | 4.10 | 2.50 |
| 500 | 0.45 | 0.29 | 2.25 | 1.45 | 7.10 | 4.52 |
| 510 | 0.82 | 0.54 | 4.10 | 2.70 | 12.85 | 8.42 |
| 520 | 1.35 | 0.92 | 6.75 | 4.60 | 21.05 | 14.30 |
| 530 | 1.91 | 1.33 | 9.55 | 6.65 | 29.80 | 20.75 |
| 540 | 2.35 | 1.68 | 11.75 | 8.40 | 36.65 | 26.20 |
| 550 | 2.54 | 1.82 | 12.70 | 9.10 | 39.60 | 28.38 |
| 555 | 2.56 | 1.80 | 12.80 | 9.00 | 39.90 | 28.05 |
| 560 | 2.53 | 1.74 | 12.65 | 8.70 | 39.45 | 27.15 |
| 570 | 2.30 | 1.51 | 11.50 | 7.55 | 35.85 | 23.55 |
| 580 | 1.85 | 1.15 | 9.25 | 5.75 | 28.85 | 17.90 |
| 590 | 1.32 | 0.77 | 6.60 | 3.85 | 20.60 | 12.00 |
| 600 | 0.82 | 0.44 | 4.10 | 2.20 | 12.80 | 6.85 |
| 610 | 0.46 | 0.23 | 2.30 | 1.15 | 7.20 | 3.60 |
| 620 | 0.24 | 0.12 | 1.20 | 0.60 | 3.75 | 1.88 |
| 640 | 0.08 | 0.04 | 0.40 | 0.20 | 1.25 | 0.62 |
| 660 | 0.05 | 0.03 | 0.25 | 0.15 | 0.80 | 0.48 |
| 680 | 0.04 | 0.02 | 0.20 | 0.10 | 0.65 | 0.32 |
| 692.8 | 0.35 | 0.06 | 1.75 | 0.30 | 5.50 | 0.95 |
| 694.3 | 0.48 | 0.08 | 2.40 | 0.40 | 7.52 | 1.25 |
| 700 | 0.03 | 0.02 | 0.15 | 0.10 | 0.48 | 0.32 |
| 720 | 0.02 | 0.01 | 0.10 | 0.05 | 0.32 | 0.16 |
| 750 | 0.01 | 0.01 | 0.05 | 0.03 | 0.16 | 0.09 |
| 800 | 0.01 | 0.01 | 0.03 | 0.02 | 0.10 | 0.06 |

450–500 nm window: minimum at 476–480 nm, α_min/α_max(U) ≈ 0.075–0.078. Short side set
by (1) the low-energy vibronic tail of ⁴T₁ and the B lines; (2) Fe³⁺ 450 nm
(22220 cm⁻¹, FWHM ≈ 500 cm⁻¹), adding 0.5–2.5 cm⁻¹ at 450 nm for 100–300 ppmw Fe;
(3) the O²⁻→Cr³⁺ LMCT near 178 nm (56000 cm⁻¹, f ~ 0.1) whose Urbach tail reaches
300–360 nm. The corundum band gap (8.8 eV, ~141 nm) plays no visible role.

Stated cause of a too-purple hue: Gaussians symmetric in *wavelength* (our model uses
energy Gaussians, so this does not apply directly), plus missing trace Fe³⁺ 450 nm and
missing R-line luminescence (η ≈ 0.70).

Chroma maximum near 0.25 wt% at 5 mm is stated to be real (CIELAB solid compression).

Measured color (D65, 2°):

| Sample | Composition (wt%) | L (cm) | L* | a* | b* | C* | h_ab (°) |
|---|---|---|---|---|---|---|---|
| R-1 synthetic | 0.05 Cr₂O₃ | 0.50 | 68.4 | 32.1 | 5.4 | 32.5 | 9.5 |
| R-2 synthetic | 0.25 Cr₂O₃ | 0.50 | 37.8 | 69.5 | 24.8 | 73.8 | 19.6 |
| R-3 Mogok | 0.72 Cr₂O₃, 0.015 Fe₂O₃ | 0.25 | 26.2 | 58.4 | 28.5 | 65.0 | 26.0 |
| R-3 Mogok | 0.72 Cr₂O₃, 0.015 Fe₂O₃ | 0.50 | 14.1 | 35.8 | 18.2 | 40.2 | 26.9 |

## 2. Blue sapphire: Fe²⁺–Ti⁴⁺ and iron in corundum

- Edge-sharing pairs (basal plane) → E⊥c band 578–588 nm (17300–17000 cm⁻¹).
- Face-sharing pairs (∥c) → E∥c band 700–735 nm (14300–13600 cm⁻¹).
- Marcus line shape: α(ν̃) = (α₀/ν̃)·exp(−(hcν̃ − E_op)² / (4 k_B T λ_reorg)).
- Pairing law: Mg²⁺ binds Ti⁴⁺ into inactive [Mg–Ti]; [Ti]_active = max(0, [Ti] − [Mg]).
  With Fe ≫ Ti_active, α scales **linearly with [Ti]_active** (not with [Fe]·[Ti]).
  σ_pair (E⊥c, 578 nm) = 2.4×10⁻¹⁸ cm² → 0.28 cm⁻¹ per ppma active Ti.
- Fe³⁺ single + pair: α = c₁[Fe³⁺] + c₂[Fe³⁺]² (Ferguson & Fielding 1971, 1972).
- Fe²⁺–Fe³⁺ IVCT at 880 nm (11360 cm⁻¹, FWHM ≈ 3800 cm⁻¹), tail past 700 nm.

| Feature | Pol. | λ₀ (nm) | ν̃₀ (cm⁻¹) | FWHM (cm⁻¹) | Profile | Scaling / cross-section |
|---|---|---|---|---|---|---|
| Fe²⁺–Ti⁴⁺ IVCT | E⊥c | 578.0 | 17301 | 5100 | Marcus energy Gaussian | σ = 2.4×10⁻¹⁸ cm²/pair |
| Fe²⁺–Ti⁴⁺ IVCT | E∥c | 730.0 | 13698 | 4950 | Marcus energy Gaussian | σ = 1.1×10⁻¹⁸ cm²/pair |
| Fe³⁺ single/pair | E⊥c | 450.0 | 22222 | 550 | skewed Lorentzian | α₄₅₀ = 0.0035[Fe] + 1.2×10⁻⁶[Fe]² |
| Fe³⁺ single/pair | E⊥c | 388.0 | 25773 | 620 | Gaussian | α₃₈₈ = 0.0051[Fe] + 1.8×10⁻⁶[Fe]² |
| Fe³⁺ single/pair | E⊥c | 377.0 | 26525 | 500 | Gaussian | α₃₇₇ = 0.0082[Fe] + 2.6×10⁻⁶[Fe]² |
| Fe²⁺–Fe³⁺ IVCT | E⊥c | 880.0 | 11364 | 3800 | Marcus energy Gaussian | σ = 3.2×10⁻¹⁹ cm²/pair |

([Fe] units for the quadratic laws are not stated; ppma is the most likely reading.)

Samples: S-Met (metamorphic, Sri Lanka) Fe 450 ppmw (820 ppma), Ti 160 ppmw (340 ppma),
Mg 12 ppmw (50 ppma), Ti_active 290 ppma. S-Bas (basaltic, Australia) Fe 2800 ppmw
(5100 ppma), Ti 180 ppmw (383 ppma), Mg 22 ppmw (92 ppma), Ti_active 291 ppma.
(Note: 12 ppmw Mg → 50 ppma and 22 ppmw → 92 ppma look inconsistent with the Fe/Ti
conversions; check.)

Napierian α (cm⁻¹):

| λ (nm) | S-Met E⊥c | S-Met E∥c | S-Bas E⊥c | S-Bas E∥c |
|---|---|---|---|---|
| 350 | 1.82 | 1.55 | 12.45 | 9.80 |
| 360 | 1.95 | 1.62 | 13.50 | 10.40 |
| 370 | 2.10 | 1.70 | 14.80 | 11.20 |
| 377 | 3.45 | 2.10 | 24.10 | 15.60 |
| 380 | 2.60 | 1.85 | 18.20 | 12.80 |
| 388 | 3.82 | 2.30 | 26.50 | 17.10 |
| 400 | 1.65 | 1.15 | 10.90 | 7.40 |
| 410 | 1.25 | 0.88 | 7.80 | 5.40 |
| 420 | 0.95 | 0.65 | 5.80 | 4.10 |
| 430 | 0.88 | 0.58 | 5.10 | 3.60 |
| 440 | 0.82 | 0.52 | 4.60 | 3.20 |
| 450 | 2.15 | 1.10 | 11.85 | 5.40 |
| 460 | 0.78 | 0.48 | 3.90 | 2.65 |
| 470 | 0.82 | 0.49 | 3.75 | 2.50 |
| 480 | 0.88 | 0.51 | 3.65 | 2.45 |
| 490 | 1.05 | 0.58 | 3.80 | 2.55 |
| 500 | 1.35 | 0.68 | 4.10 | 2.70 |
| 520 | 2.25 | 1.02 | 5.25 | 3.20 |
| 540 | 4.10 | 1.65 | 7.45 | 4.15 |
| 560 | 6.85 | 2.55 | 10.60 | 5.50 |
| 578 | 8.22 | 3.45 | 12.35 | 6.75 |
| 600 | 7.90 | 4.60 | 12.20 | 8.20 |
| 620 | 6.75 | 5.80 | 11.15 | 9.75 |
| 640 | 5.20 | 6.85 | 9.70 | 11.20 |
| 660 | 3.85 | 7.60 | 8.45 | 12.30 |
| 680 | 2.70 | 8.10 | 7.40 | 13.05 |
| 700 | 1.85 | 8.35 | 6.65 | 13.45 |
| 730 | 1.05 | 8.20 | 6.10 | 13.25 |
| 760 | 0.62 | 6.95 | 5.85 | 11.85 |
| 800 | 0.35 | 4.85 | 5.95 | 9.40 |
| 840 | 0.22 | 3.10 | 6.45 | 7.20 |
| 880 | 0.18 | 1.85 | 7.10 | 5.60 |
| 950 | 0.12 | 0.95 | 5.40 | 3.80 |
| 1050 | 0.08 | 0.45 | 2.80 | 1.95 |

Measured color:

| Sample / orientation | Composition | Illuminant | L (cm) | L* | a* | b* | C* | h_ab (°) |
|---|---|---|---|---|---|---|---|---|
| S-Met E⊥c | low Fe, Ti_act 290 ppma | D65 | 0.20 | 64.2 | −6.8 | −41.2 | 41.8 | 260.6 |
| S-Met E∥c | same | D65 | 0.20 | 73.5 | −14.2 | −18.5 | 23.3 | 232.5 |
| S-Met unpolarised (c-cut basal slab) | same | D65 | 0.20 | 64.0 | −6.5 | −41.0 | 41.5 | 261.0 |
| S-Bas E⊥c | Fe 5100 ppma | D65 | 0.20 | 48.6 | −12.4 | −28.6 | 31.2 | 246.6 |
| S-Bas E⊥c | Fe 5100 ppma | A | 0.20 | 46.2 | −8.1 | −22.4 | 23.8 | 250.1 |
| S-Bas thick | Fe 5100 ppma | D65 | 0.50 | 21.5 | −14.8 | −16.2 | 21.9 | 227.6 |

## 3. V³⁺ in corundum; alexandrite (Cr³⁺ in chrysoberyl)

V³⁺ (3d², octahedral) in corundum, E⊥c:
- ³T₁(F)→³T₂(F): 575 nm (17390 cm⁻¹), FWHM ≈ 3350 cm⁻¹, σ = 2.05×10⁻¹⁹ cm²
  (0.0241 cm⁻¹ per ppma V).
- ³T₁(F)→³T₁(P): 398 nm (25125 cm⁻¹), FWHM ≈ 3600 cm⁻¹, σ = 2.12×10⁻¹⁹ cm².

color change relies on window I (475–490 nm) vs window II (> 650 nm); excess Cr →
permanently red; excess Fe³⁺ or Fe–Ti kills a window.

Alexandrite: BeAl₂O₄, Pnma, a = 9.404, b = 5.476, c = 4.427 Å, Z = 4.
Indicatrix: α (n ≈ 1.745) ∥ c; β (1.748) ∥ b; γ (1.754) ∥ a.
Cr³⁺ on the mirror site Al2 (C_s) ≈ 78 ± 3 %, inversion site Al1 (C_i) 22 %.

| Ray | ⁴T₂ band | ⁴T₁ band | Pleochroic color |
|---|---|---|---|
| α (E∥c) | 572 nm (17480 cm⁻¹) | 415 nm (24100 cm⁻¹) | bluish green |
| β (E∥b) | 560 nm (17860 cm⁻¹) | 405 nm (24690 cm⁻¹) | orange-yellow |
| γ (E∥a) | 590 nm (16950 cm⁻¹) | 422 nm (23700 cm⁻¹) | purplish red |

Samples: synthetic V-corundum 0.26 wt% V₂O₃ (305 ppma V); natural Russian alexandrite
0.32 wt% Cr₂O₃ + 0.15 wt% Fe₂O₃. (Note: 0.26 wt% V₂O₃ → ~1700 ppma V, not 305; check.)
Napierian α (cm⁻¹):

| λ (nm) | V-corundum E⊥c | Alex α (E∥c) | Alex β (E∥b) | Alex γ (E∥a) |
|---|---|---|---|---|
| 350 | 2.10 | 4.80 | 5.20 | 6.40 |
| 360 | 2.75 | 6.10 | 6.80 | 8.30 |
| 370 | 3.65 | 7.95 | 9.10 | 11.20 |
| 380 | 4.90 | 10.30 | 11.80 | 14.50 |
| 390 | 6.50 | 12.80 | 14.50 | 17.60 |
| 398 | 7.35 | 14.50 | 16.00 | 19.40 |
| 400 | 7.42 | 14.90 | 16.20 | 19.80 |
| 405 | 7.38 | 15.40 | 16.10 | 20.50 |
| 410 | 7.15 | 15.60 | 15.80 | 20.90 |
| 415 | 6.60 | 15.40 | 15.10 | 21.15 |
| 420 | 5.80 | 14.80 | 13.90 | 21.20 |
| 430 | 3.90 | 12.10 | 10.80 | 18.50 |
| 440 | 2.25 | 8.70 | 7.40 | 14.10 |
| 450 | 1.20 | 5.60 | 4.60 | 9.80 |
| 460 | 0.70 | 3.60 | 2.90 | 6.60 |
| 470 | 0.48 | 2.45 | 2.10 | 4.60 |
| 480 | 0.42 | 1.85 | 1.65 | 3.40 |
| 490 | 0.51 | 1.70 | 1.55 | 2.95 |
| 500 | 0.82 | 1.95 | 1.80 | 3.10 |
| 510 | 1.35 | 2.50 | 2.30 | 3.75 |
| 520 | 2.10 | 3.40 | 3.10 | 4.85 |
| 530 | 3.25 | 4.80 | 4.40 | 6.45 |
| 540 | 4.65 | 6.80 | 6.20 | 8.60 |
| 550 | 5.95 | 8.90 | 8.30 | 11.10 |
| 560 | 6.80 | 10.90 | 10.40 | 13.80 |
| 570 | 7.25 | 12.20 | 10.50 | 15.80 |
| 575 | 7.35 | 12.60 | 10.10 | 16.40 |
| 580 | 7.20 | 12.50 | 9.40 | 16.90 |
| 590 | 6.45 | 11.80 | 7.90 | 17.20 |
| 600 | 5.30 | 10.10 | 6.00 | 16.10 |
| 610 | 4.10 | 7.90 | 4.20 | 12.80 |
| 620 | 2.90 | 5.60 | 2.70 | 9.30 |
| 630 | 1.85 | 3.80 | 1.70 | 6.40 |
| 640 | 1.10 | 2.40 | 1.05 | 4.10 |
| 650 | 0.65 | 1.45 | 0.65 | 2.30 |
| 660 | 0.40 | 0.90 | 0.42 | 1.40 |
| 670 | 0.28 | 0.62 | 0.30 | 0.95 |
| 678.5 | 0.22 | 3.90 | 1.45 | 4.60 |
| 680.4 | 0.18 | 4.80 | 1.85 | 5.90 |
| 700 | 0.08 | 0.35 | 0.18 | 0.45 |
| 750 | 0.02 | 0.08 | 0.05 | 0.10 |
| 800 | 0.01 | 0.03 | 0.02 | 0.04 |

Alexandrite color (L = 0.20 cm):

| Ray | Illuminant | L* | a* | b* | C* | h_ab (°) | Visual |
|---|---|---|---|---|---|---|---|
| α (E∥c) | D65 | 66.8 | −18.2 | 12.4 | 22.0 | 145.7 | bluish green |
| α (E∥c) | A | 65.4 | −7.5 | 16.8 | 18.4 | 114.1 | yellowish green |
| β (E∥b) | D65 | 72.1 | 2.4 | 36.8 | 36.9 | 86.3 | orange-yellow |
| β (E∥b) | A | 71.8 | 12.2 | 44.5 | 46.1 | 74.7 | reddish orange |
| γ (E∥a) | D65 | 52.1 | 6.4 | −8.2 | 10.4 | 308.0 | greyish violet |
| γ (E∥a) | A | 54.8 | 24.5 | 5.2 | 25.0 | 12.0 | purplish red |
| unpolarised (b-axis) | D65 | 60.4 | −11.5 | 4.2 | 12.2 | 159.9 | bluish green |
| unpolarised (b-axis) | A | 61.2 | 13.8 | 11.6 | 18.0 | 40.0 | purplish red |

Stated for 2.0 mm along b, unpolarised: Δh_ab = 119.9°, ΔE*_ab = 26.4 (D65 vs A).

## 4. Reference gems

- **Tanzanite** (zoisite, Pnma, a = 16.19, b = 5.55, c = 10.03 Å; α ∥ a, β ∥ b, γ ∥ c).
  Natural: ~0.38 wt% V₂O₃, ~0.12 wt% TiO₂, ~0.04 wt% Fe₂O₃. α: V³⁺ 525 nm
  (19050 cm⁻¹) reddish purple. β: window at 470 nm bounded by V³⁺ 585 and 740 nm,
  sapphire blue. γ: IVCT 450 nm (22220 cm⁻¹; Ti³⁺–Ti⁴⁺ or Fe²⁺–Ti⁴⁺) + 680 nm band,
  olive yellow-brown. Heating 450–550 °C oxidising: Ti³⁺→Ti⁴⁺, bleaches 450 nm in γ;
  γ becomes like β (dichroic: blue β,γ; purple α).
- **Peridot** (Fo₉₀Fa₁₀, FeO ≈ 9.25 wt%, Pbnm; α ∥ b, β ∥ c, γ ∥ a). Fe²⁺ M1 bands
  11800 cm⁻¹ (847 nm) and 9500 cm⁻¹ (1053 nm); M2 7800 cm⁻¹ (1282 nm). Visible-red tail
  α ≈ 1.5–3.8 cm⁻¹ at 600–750 nm. Spin-forbidden 453, 473, 493 nm on a rising UV
  charge-transfer edge. Window 515–540 nm.
- **Amethyst** ([FeO₄]⁰ centre): 545 nm (18350 cm⁻¹, FWHM ≈ 5200 cm⁻¹) and 357 nm
  (28010 cm⁻¹); E⊥c bluish violet, E∥c reddish violet. Centre density ∝ [Fe]·dose until
  saturation (10–50 kGy). Heating 450–500 °C bleaches; further heating → Fe-oxide
  nanoparticles (citrine): monotonic edge rising from 550 nm into the UV.
- **Emerald** (beryl, P6/mcc; Cr³⁺/V³⁺ on D₃ sites). ⁴T₂: 605 nm (16530 cm⁻¹,
  FWHM ≈ 2400) E⊥c, 635 nm (15750) E∥c. ⁴T₁: 430 nm (23250, FWHM ≈ 2900) E⊥c, 425 nm
  (23530) E∥c. R₁ 683.5, R₂ 680.5 nm. Transmission maximum 520 nm (E⊥c, green), 505 nm
  (E∥c, bluish green).

Napierian α (cm⁻¹) — heated tanzanite 0.38 wt% V₂O₃; natural tanzanite 0.38 V₂O₃ +
0.12 TiO₂; peridot Fo₉₀; amethyst 80 ppmw Fe; citrine (annealed); Colombian emerald
0.55 wt% Cr₂O₃ + 0.10 wt% V₂O₃:

| λ (nm) | Tanz. heated β∥b | Tanz. natural γ∥c | Peridot α∥b | Peridot γ∥a | Amethyst E⊥c | Citrine unpol. | Emerald E⊥c | Emerald E∥c |
|---|---|---|---|---|---|---|---|---|
| 350 | 3.40 | 12.80 | 18.50 | 14.20 | 4.80 | 14.50 | 16.20 | 14.10 |
| 370 | 2.60 | 11.90 | 10.40 | 7.90 | 3.90 | 11.20 | 13.80 | 11.90 |
| 400 | 1.85 | 10.20 | 4.10 | 3.10 | 2.60 | 7.40 | 9.80 | 8.20 |
| 425 | 1.25 | 9.80 | 2.10 | 1.60 | 1.85 | 4.60 | 14.20 | 12.60 |
| 450 | 0.85 | 11.20 | 1.75 | 1.25 | 1.40 | 2.80 | 9.10 | 7.40 |
| 475 | 0.65 | 7.80 | 1.15 | 0.85 | 1.25 | 1.65 | 3.80 | 2.90 |
| 500 | 0.95 | 4.60 | 0.75 | 0.55 | 1.45 | 0.95 | 1.60 | 1.25 |
| 520 | 1.60 | 2.90 | 0.58 | 0.42 | 1.95 | 0.58 | 1.15 | 0.95 |
| 545 | 2.95 | 2.10 | 0.65 | 0.45 | 2.80 | 0.35 | 1.95 | 1.70 |
| 585 | 5.80 | 2.65 | 0.92 | 0.62 | 2.20 | 0.20 | 5.80 | 4.90 |
| 605 | 5.40 | 3.40 | 1.18 | 0.78 | 1.45 | 0.12 | 8.40 | 6.80 |
| 635 | 4.10 | 4.80 | 1.60 | 1.05 | 0.85 | 0.08 | 7.20 | 10.20 |
| 660 | 3.20 | 5.90 | 2.10 | 1.40 | 0.48 | 0.05 | 4.10 | 5.90 |
| 683.5 | 2.65 | 5.10 | 2.65 | 1.80 | 0.32 | 0.04 | 7.90 | 4.80 |
| 740 | 4.90 | 2.10 | 3.80 | 2.60 | 0.18 | 0.02 | 0.85 | 1.10 |
| 800 | 2.10 | 0.95 | 5.60 | 3.90 | 0.12 | 0.02 | 0.40 | 0.52 |
| 850 | 0.85 | 0.45 | 7.90 | 5.40 | 0.15 | 0.01 | 0.25 | 0.30 |
| 950 | 0.30 | 0.20 | 11.20 | 7.60 | 0.42 | 0.01 | 0.15 | 0.18 |
| 1050 | 0.15 | 0.12 | 13.80 | 9.20 | 0.22 | 0.01 | 0.10 | 0.12 |

Measured color (D65, 2°):

| Gem | Chemistry | Ray | L (cm) | L* | a* | b* | C* | h_ab (°) |
|---|---|---|---|---|---|---|---|---|
| Tanzanite (heated) | 0.38 wt% V₂O₃ | β ∥ b | 0.40 | 45.2 | 8.5 | −46.2 | 47.0 | 280.4 |
| Tanzanite (heated) | 0.38 wt% V₂O₃ | α ∥ a | 0.40 | 49.8 | 31.4 | −28.6 | 42.5 | 317.7 |
| Tanzanite (unheated) | 0.38 V₂O₃, 0.12 TiO₂ | γ ∥ c | 0.40 | 52.4 | −4.2 | 34.6 | 34.9 | 96.9 |
| Peridot (Fo₉₀) | 9.25 wt% FeO | α ∥ b | 0.50 | 78.5 | −18.4 | 42.1 | 45.9 | 113.6 |
| Amethyst | 80 ppmw Fe (irradiated) | E⊥c | 0.60 | 62.4 | 22.8 | −18.5 | 29.4 | 321.0 |
| Citrine | annealed amethyst | unpol. | 0.60 | 76.8 | 6.8 | 58.4 | 58.8 | 83.4 |
| Emerald (Muzo) | 0.55 Cr₂O₃, 0.10 V₂O₃ | E⊥c | 0.30 | 58.2 | −54.6 | 28.4 | 61.5 | 152.5 |
| Emerald (Muzo) | 0.55 Cr₂O₃, 0.10 V₂O₃ | E∥c | 0.30 | 61.4 | −44.2 | 14.8 | 46.6 | 161.5 |

## 5. Dopant levels and band pruning

| Host | Chromophore | Gem content | Dominant bands (nm) | Share of color | Pruning |
|---|---|---|---|---|---|
| Cubic zirconia | Co²⁺ (tet) | 0.01–0.08 wt% CoO | 535, 595, 645 | > 98 % | keep triplet; drop UV/IR shoulders |
| Cubic zirconia | Er³⁺ | 0.50–2.50 wt% Er₂O₃ | 487, 522 | > 90 % | keep sharp lines (≤ 2 nm sampling); drop UV |
| Cubic zirconia | Ce⁴⁺ CT edge | 0.10–0.50 wt% CeO₂ | edge 420–480 | — | — |
| Cubic zirconia | Cr³⁺ | 0.05–0.30 wt% Cr₂O₃ | 425, 610 | — | — |
| Pyralspite | Fe²⁺ (dod) | 2.0–35.0 wt% FeO | 504, 521, 570 (+ NIR 1280 tail to 700) | > 85 % | keep triplet + NIR tail; drop 435, 460 |
| Pyralspite | Mn²⁺ (dod) | 0.5–42.0 wt% MnO | 410, 485 | > 95 % | keep 410, 485; drop 422, 430 |
| Pyralspite | Cr³⁺ (oct, chrome pyrope) | 0.2–7.5 wt% Cr₂O₃ | U 570, Y 410, R 687 | — | — |
| Spinel | Co²⁺ (tet) | 0.005–0.08 wt% CoO | 550, 580, 625 | > 99 % | keep triplet; drop iron |
| Spinel | Cr³⁺ (oct) | 0.05–2.50 wt% Cr₂O₃ | 395, 540, 685.5 | > 95 % | keep broad bands + 685.5; drop vibronics |
| Spinel | Fe²⁺ (tet) | 0.10–4.00 wt% FeO | NIR 2000 tail; 458, 480 | — | — |
| Silicate glass | Fe²⁺ (oct) | 0.50–2.50 wt% FeO | 1050–1100 (FWHM ≈ 4000; tail to 550) | > 90 % | one skewed energy Gaussian |
| Silicate glass | Co²⁺ (tet) | 0.02–0.15 wt% CoO | 530, 590, 645 | — | — |
| Silicate glass | Cu²⁺ (oct) | 0.50–3.00 wt% CuO | 780–800 (tail to 600) | > 95 % | one broad wavenumber Gaussian |

Glass: no sharp lines; each species one skewed energy Gaussian.

## Recommendations (as delivered)

1. Band profiles in wavenumber/eV; crystal-field bands as energy-skewed Gaussians;
   IVCT as Marcus shapes.
2. Corundum Fe²⁺–Ti⁴⁺: [Ti]_active = max(0, [Ti] − [Mg]); α linear in [Ti]_active when
   Fe is in excess. Include the Fe³⁺ quadratic pair terms and the 880 nm Fe²⁺–Fe³⁺ tail.
3. Include LMCT Urbach tails (350–420 nm) for ruby, emerald, alexandrite.
4. In low-Fe Cr gems, treat the R lines as emitters under daylight (fluorescence).

## Integration notes (coordinator)

- Unverified secondary source; several internal unit inconsistencies flagged above
  (Mg ppmw→ppma, V₂O₃ wt%→ppma). The integration must check every table against its
  own CIELAB values with our colorimetry before adopting it.
