# Research request 2: primary measured spectra with same-sample color

## Why a second request

Two reports answered the first request (`chromophore-research-query.md`).
We checked each tabulated spectrum against the CIELAB color stated next to it
(method below). Most do not agree with their own color, by ΔE00 10–38.

- **Reproduced:**
  - S-Met sapphire E⊥c (ΔE00 4.1);
  - amethyst (ΔE00 3.4).
- **Borderline:** S-Bas sapphire (ΔE00 5–8).
- **Not reproduced:**
  - ruby R-1/R-2/R-3 (ΔE00 16–26);
  - alexandrite (ΔE00 11–38);
  - tanzanite (ΔE00 17–21);
  - peridot (ΔE00 12.5);
  - emerald (ΔE00 10–12);
  - citrine (ΔE00 12.7).
- **Report 2:** the ruby table was synthesised from model curves, not measured,
  and its sapphire table has no sample metadata.

The recurring defect is a b* offset of −25 to −45 for ruby, alexandrite and
tanzanite. The tabulated 450–500 nm windows are far more transparent than the
stated colors allow.

In several places a unit label contradicts the numbers:
- 0.26 wt% V₂O₃ was labelled "305 ppma";
- ppma was meant per formula unit in one place and per all atoms in another;
- ppmw was labelled ppma.

The two reports also contradict each other in 18 places. Full details are in
`docs/chromophores/research-2026-10-validation.md`.

We therefore need **traceable primary data, not reconstructions**.

## Hard requirements (a source that fails any of these is not usable)

1. **Measured data only.** No spectra synthesised from band parameters, and
   no "typical" curves. A spectrum digitised from a published figure is
   acceptable only with the figure number, the axis units, and the digitising
   tool and its resolution.
2. **Same sample.** The composition, path length, polarisation and (where
   given) the color must all belong to the one physical sample that was
   measured. Do not combine numbers from different papers into one "sample".
3. **Raw files preferred.** Downloadable data files are best, for example:
   - Caltech Mineral Spectroscopy Server (Rossman);
   - RRUFF;
   - supplementary data of papers;
   - GIA / SSEF / Gübelin open data.
   
   Give the direct URL of each file and keep the original numbers; do not
   resample them.
4. **Units stated exactly:**
   - absorbance (decadic) vs α in cm⁻¹ (Napierian or decadic);
   - whether reflection losses were removed;
   - concentrations with an explicit basis: wt% oxide, ppmw, ppma of
     **all atoms**, or apfu. Never "ppma" without a basis.
5. **Self-check before delivery.** For every sample that has both a spectrum
   and a color, compute the color from the spectrum as below and report the
   ΔE00. If it exceeds 3, say so and give the most likely reason. Do not
   adjust the numbers.
   - T(λ) = 10^(−A) or exp(−α_Nap·L);
   - range 380–780 nm at 1–5 nm;
   - CIE 1931 2° observer;
   - D65, or illuminant A at **2856 K** (not 3200 K);
   - CIELAB relative to the illuminant's own white (T = 1 → L* = 100);
   - polarisation mix for an unpolarised uniaxial sample: (2·T_o + T_e)/3,
     unless the source says otherwise.
6. **Every number gets a tag:** `[primary: ref, table/fig]`, `[review: ref]` or
   `[derived: show the arithmetic]`.

## What we need, by priority

### P1. Ruby (Cr³⁺:Al₂O₃): the 400–500 nm region

1. At least three samples between 0.05 and 1 wt% Cr₂O₃, each with analysed Cr,
   Fe and Ti, the path length, and polarised (E⊥c, E∥c) spectra covering
   **300–750 nm**, including the UV edge.
2. Measured color of the same samples, if any exists. If no paper gives
   same-sample color, give spectra and composition only and say that.
3. What absorbs between 430 and 500 nm in natural and in synthetic
   (flux/Verneuil) ruby: the Cr³⁺ Y-band tail, the O²⁻→Cr³⁺ charge-transfer
   tail, Fe³⁺, or color centres. Give α at 450, 475 and 500 nm per
   polarisation, and their dependence on Cr and Fe.

### P2. Alexandrite (Cr³⁺:BeAl₂O₄) and tanzanite (V³⁺ zoisite)

- Polarised spectra (α, β, γ) with the **axis mapping stated by the source**
  (which ray vibrates along which crystallographic axis), the analysed Cr or V
  content, the path length, and 380–750 nm coverage.
- Tanzanite: natural (unheated) and heated material, ideally the same crystal
  before and after heating.
- Measured color (D65 and A) of the same samples, if available.

### P3. Single-sample checks (one good sample each is enough)

Each needs a polarised or clearly unpolarised spectrum with composition and path
length:
- peridot (forsterite, Fa content);
- emerald (Cr and V **separately** analysed);
- citrine (heated amethyst, and natural);
- blue sapphire with **E∥c** data (we have no reliable E∥c).

### P4. Resolve these contradictions from primary sources

| Question | Report 1 | Report 2 |
|---|---|---|
| Fe²⁺–Ti⁴⁺ pairing law in sapphire | linear in [Ti] − [Mg] | ∝ [Fe]·[Ti] |
| Fe²⁺–Ti⁴⁺ absorption per active Ti | 0.28 cm⁻¹/ppma | — |
| Ruby U-band FWHM (cm⁻¹) | 3180–3350 | ~2630 |
| Ruby Y-band peak relative to U | — | ~1.7× report 1 |
| Alexandrite axis mapping | α∥c, β∥b, γ∥a | different |
| Tanzanite axis mapping | — | different |
| V-corundum color change ΔE | small | "often > 25" |

For the Fe²⁺–Ti⁴⁺ coefficient, the sapphire tables imply about 0.05 cm⁻¹ per
ppm of Al sites, against the stated 0.28 cm⁻¹/ppma.

Our own data cannot settle the V-corundum color change: report 1's V spectrum
gives ΔE00 2.9–5.8 between D65 and A over 1–10 mm, and report 2's claim is
"often > 25". We need a color-change sapphire with both a measured spectrum
and a measured D65/A color.

For each row: what the primary literature says, with the reference.

### P5. Low priority: fluorescence data (for the UV-lamp rendering)

For Cr³⁺ in corundum, spinel, chrysoberyl and beryl, diamond N3/H3/NV, and
Mn²⁺ where relevant:
- quantum yield;
- emission line positions and widths, and the vibronic sideband shape;
- the excitation spectrum at 300–420 nm;
- quenching by Fe (the concentration at which the yield halves).

Tag every value as primary or estimate.

## Deliverable

1. One Markdown report with one section per sample. Each section has:
   - metadata;
   - the self-check ΔE00;
   - the spectrum table (CSV, λ in nm vs α in cm⁻¹ or absorbance) or a file
     URL.
2. The raw data files attached or linked.
3. A short list of what was looked for and **not found**.
