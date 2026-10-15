# Research request: quantitative polarized absorption spectra of gem chromophores

## Purpose and context

We render gemstone color physically: each color-causing species (an ion, an
intervalence pair or a color centre) contributes absorption bands, the
absorption coefficient α(λ) is summed per polarization, Beer–Lambert gives the
transmittance T(λ) = exp(−α·L), and T is integrated with the CIE 1931 2°
observer under D65 (and a 3200 K Planckian) to CIELAB. Our current band data
comes from secondary compilations. It reproduces the *kind* of color but not
the measured hue or depth, so we need **measured, quantitative** data.

Everything below is per host crystal and per polarization: E⊥c (o) and E∥c (e)
for uniaxial hosts, and α, β, γ (with the mapping to crystallographic axes
stated) for biaxial hosts. Wavelength range: **300–800 nm**, and to 1100 nm
where a band's tail reaches into the visible.

## Deliverable format (for every item)

For each host + chromophore:

1. **Primary sources**: author, year, journal, DOI or URL. Peer-reviewed
   spectroscopy papers, Gems & Gemology, J. Gemmology, American
   Mineralogist, Physics and Chemistry of Minerals, or Rossman's mineral
   spectroscopy database. Mark each number as read in the primary source,
   taken from a review, or derived (show the derivation).
2. **Sample description** for each measured spectrum:
   - analysed composition of *the same sample* (EPMA, LA-ICP-MS, SIMS or
     similar), with units and basis stated: wt% oxide, ppmw, ppma (per all
     atoms) or atoms per formula unit;
   - optical path length;
   - polarization;
   - natural vs synthetic, and any treatment (heat, irradiation, diffusion).
3. **The spectrum itself**, as a table (CSV-style) of λ (nm) vs absorption
   coefficient α (cm⁻¹, Napierian or decadic, stated) or absorbance plus
   thickness, at ≤ 5 nm spacing. Digitized from figures if no table exists;
   say so and give the figure number.
4. **Band decomposition** where the source gives one, or a fit you consider
   reliable:
   - band centre (nm and cm⁻¹);
   - FWHM (cm⁻¹);
   - line shape: Gaussian in energy, or asymmetric, with the asymmetry
     parameters (e.g. a skewed Gaussian, a sum of two Gaussians, or a Pekarian
     / vibronic progression);
   - peak absorption coefficient **per unit concentration** for each
     polarization;
   - for intervalence charge-transfer pairs: whether α scales with the product
     of the two concentrations (random pairing) or with the limiting cation
     (clustered / charge-compensated pairs), and the evidence.
5. **Measured color** for validation, where available: CIELAB L*, a*, b*
   (or chromaticity x, y, Y), the illuminant, the path length and the
   composition.

## Specific questions

### A. Ruby: Cr³⁺ in corundum (Al₂O₃)

1. Polarized (E⊥c, E∥c) α(λ) spectra for at least three Cr₂O₃ concentrations
   in the 0.05–1.0 wt% range, with analysed composition.
2. For the U band (~555 nm) and the Y band (~405 nm): centre, FWHM, any
   asymmetry and the peak α per wt% Cr₂O₃ for each polarization. Is σ ≈
   1.6–1.7×10⁻¹⁹ cm² (E⊥c, U band) correct?
3. How deep and how wide is the transmission window at 450–500 nm between the
   Y and U bands? Which features close it on the short-wavelength side: the
   Cr³⁺ charge-transfer edge, Fe³⁺, or the host edge? Their positions and
   strengths.
4. Measured CIELAB of rubies of known Cr content and thickness, to check the
   hue angle (we currently get h_ab ≈ 312–332°, which is too purple) and how
   chroma changes with concentration (we currently get a chroma maximum near
   0.25 wt% at 5 mm).

### B. Blue sapphire: Fe²⁺–Ti⁴⁺ and related iron bands in corundum

1. Polarized α(λ) of the Fe²⁺–Ti⁴⁺ intervalence band: centre per polarization
   (~565–580 nm E⊥c; the E∥c band position), FWHM, and **the shape of the
   long-wavelength tail**. Our symmetric Gaussian makes the color too violet
   (h_ab ≈ 281°).
2. Concentration dependence: α against [Fe] and [Ti] (and [Ti] − [Mg]
   compensation if relevant); the pairing law; the absorption per pair or per
   ppm².
3. The Fe³⁺ bands (377, 388, 450 nm) and the Fe³⁺ pair enhancement (∝ c²?).
   The Fe²⁺–Fe³⁺ band near 870–880 nm and its visible tail.
4. Measured CIELAB of blue sapphires with analysed Fe, Ti and Mg, and their
   thickness.

### C. color-change corundum (V³⁺ ± Cr³⁺) and alexandrite (Cr³⁺ in chrysoberyl)

1. Polarized α(λ) of V³⁺ in corundum: band centres (~400 and ~570 nm?),
   FWHM and α per concentration.
2. Spectra and composition of color-change sapphires. What gives the
   daylight/incandescent change, and which transmission windows (blue-green
   vs red) and band positions matter? Measured ΔE or chromaticity under D65
   vs illuminant A.
3. Alexandrite (Cr³⁺:BeAl₂O₄): α, β, γ polarized spectra with the axis
   mapping, Cr content, path length, and measured color change (CIELAB under
   D65 and A).

### D. Built-in reference gems (color validation set)

For each of these, give typical composition, polarized spectra and measured
CIELAB at a stated thickness:

- **tanzanite** (V³⁺ in zoisite, trichroic, natural vs heated, with the axis
  mapping);
- **peridot** (Fe²⁺ in forsterite: the M1/M2 bands near 1050 nm and their
  visible tails, plus any Fe³⁺/Cr contribution);
- **amethyst** (the [FeO₄]⁰ color centre at ~545 nm and the 357 nm band:
  intensity against Fe content and irradiation dose; polarization);
- **citrine** (natural vs heated amethyst: the Fe³⁺ / iron-oxide edge
  position and slope);
- **emerald** (Cr³⁺/V³⁺ in beryl, polarized);
- **blue sapphire**, as in B.

### E. Typical concentrations and band count (lower priority)

For cubic zirconia (commercial dopants), pyralspite garnet (Fe²⁺, Mn²⁺, Cr,
V), colored silicate glass and spinel (Cr, Fe, Co, V):

- realistic dopant levels in gem material;
- which bands dominate the visible color at those levels, ranked by their
  contribution to the transmitted color.

This tells us which weak bands can be merged or dropped.

## Out of scope

Refractive index and dispersion; fluorescence (unless it comes with the same
spectra); infrared bands beyond 1100 nm.
