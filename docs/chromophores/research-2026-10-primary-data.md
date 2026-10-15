# Research request 2: primary measured spectra with same-sample color

Compiled 2026-10-04. Answers *Research request 2* (follow-up to `chromophore-research-query.md`).

## 0. Summary

**The hard requirements could be met only in part.** The blocking finding is that **no sample was found that has both a measured spectrum and a measured color.** The ΔE00 self-check, as specified, therefore cannot be run on any sample. Section 1 replaces it with two checks that can be run:

- **(a) Pipeline validation.** Your color method reproduces GIA's published colors from GIA's own measured cross-sections to ΔE00 ≈ 1.
- **(b) Spectrum colors.** The CIELAB color of every primary raw spectrum below, so that you can check any color a source states against it.

| Priority | Status |
|---|---|
| P1 ruby | **Not met.** One analysed ruby with both polarisations (synthetic, 0.05 wt% Cr₂O₃). One natural ruby, E⊥c only, with no composition. No measured color exists for either. |
| P2 alexandrite / tanzanite | **Partly met.** Raw polarised files exist on the Caltech server. Alexandrite has only 2 of the 3 rays and no Cr analysis. Tanzanite GRR 1265 has all 3 rays and V₂O₃ in the file header. No measured color exists. Same-crystal before/after-heating data exist only as images. |
| P3 single-sample checks | Raw spectra exist for peridot, emerald, blue sapphire (E∥c included) and amethyst. **None has an analysis made on the same sample.** No digital citrine spectrum was found. |
| P4 contradictions | **Five of seven rows resolved** (§7). The ruby Y/U ratio is answered from one sample, and the U-band FWHM claim of ~2630 cm⁻¹ is unexplained. |
| P5 fluorescence | **Thin** (§8). Most values are not in the primary literature that could be reached. |

The most useful single primary source is **GIA, Dubinsky, Stone-Sundberg & Emmett 2020** (*Gems & Gemology* 56(1), 2–28).

- It provides a 1 nm, 200–1100 nm, reflection-corrected table of absorption **cross-sections** for E⊥c and E∥c, for Cr³⁺, V³⁺, Fe²⁺–Ti⁴⁺, h•–Fe³⁺, h•–Cr³⁺ and Fe³⁺.
- Its ppma is defined explicitly as parts per million of **all atoms**.
- It is the cleanest basis for a renderer. Your pipeline reproduces its colors (§1a).
- Data file: <https://www.gia.edu/dam/migrated-assets/docs/doc1/sp20-corundum-chromophores-absorption-cross-section-data.xlsx>. The link printed in the paper, under `/doc/`, returns 404.
- Appendix 1 lists the samples: <https://www.gia.edu/dam/migrated-assets/docs/doc1/sp20-corundum-chromophores-appendix1.xlsx>
- Appendix 2 gives calculated L\*a\*b\*: <https://www.gia.edu/dam/migrated-assets/docs/doc1/sp20-corundum-chromophores-appendix2.xlsx>
- Paper PDF: <https://www.gia.edu/dam/migrated-assets/docs/doc1/sp20-corundum-chromophores.pdf>

**Access notes.**

- The Caltech Mineral Spectroscopy Server (Rossman) is up: index last updated 13 Aug 2025. Its pages use `Index.html`; the old `Index.htm` URLs return 404.
- RRUFF could not be reached from the research environment.
- Every Caltech file below was read directly from the server.
- Local copies are in the attached zip. Their line endings were converted from CRLF to LF and, for the sapphire files, leading blanks were dropped. The numbers are unchanged; the URLs are the canonical copies.

**Tag key.**

| Tag | Meaning |
|---|---|
| `[primary: …]` | Read from the source |
| `[derived: …]` | Arithmetic shown |
| `[review: …]` | Secondary source |
| `[unverified]` | Read only through a summarising fetch, not checked against the full text |

---

## 1. Self-check

### 1a. Pipeline validation against GIA Appendix 2 (Cr³⁺, sample 1110)

**Method** (exactly as you specified):

- T = exp(−σ·N), with N = areal density in ppma·cm × 1.178×10¹⁷ cm⁻³.
- 380–780 nm at 5 nm.
- CIE 1931 2° observer.
- D65, and A at 2856 K.
- CIELAB relative to the illuminant's own white.
- CMFs and illuminants from color-science 0.4.7.

**Result** (ΔE00, computed vs GIA Appendix 2) [derived; script `selfcheck/cie.py`]:

| ppma·cm | E⊥c A | E⊥c D65 | E∥c A | E∥c D65 | mix (2⊥+∥)/3, A | mix (2⊥+∥)/3, D65 |
|---|---|---|---|---|---|---|
| 100 | 1.1 | 1.2 | 0.8 | 0.9 | 1.9 | 2.2 |
| 375 | 1.3 | 1.0 | 1.1 | 1.0 | 2.2 | 2.4 |
| 1500 | 3.4 | 2.7 | 4.6 | 3.5 | 2.7 | 2.2 |

**Reading the result:**

- Single polarisations reproduce to ΔE00 ≈ 1 at moderate density. The residuals grow for very dark colors (L\* < 35). The likely causes are GIA's 1 nm integration and/or a different range or white normalisation.
- GIA's "both polarisations" column fits (2⊥+∥)/3 better than (⊥+∥)/2: ΔE00 2.2–2.4 against up to 6.8 at 375 ppma·cm. The exact mix GIA used is not stated.
- **The "spectrum multiplier" column in Appendix 2 is internally inconsistent** (e.g. 5.89×10¹⁸ listed for 100 ppma·cm). Using it instead of 1.178×10¹⁷ per ppma·cm gives ΔE00 7–18. The ppma·cm labels are right and the multiplier column is wrong.

### 1b. color of the primary raw spectra (no measured color exists to compare against)

**Method:**

- T = 10^(−A), using the file values as given. The values are decadic absorbance per the file headers, and include reflection loss and baseline offset.
- Same CIE settings as 1a.
- Where a file starts after 380 nm, the first value is held constant down to 380 nm. The start wavelength is listed in the table.
- The files were resampled to 5 nm for this calculation only, in `selfcheck/caltech_grid5nm.json`.

CIELAB values are given as L\*/a\*/b\*.

| Sample (URL in §2–6) | Path | Pol | Data from | D65 | A | ΔE00 D65↔A |
|---|---|---|---|---|---|---|
| Ruby GRR 1843 `ru1843.a` | 0.787 mm | E⊥c | 380 | 45.6/34.7/−12.6 | 48.3/44.8/−12.1 | 4.6 |
| Ruby GRR 1843 `corundum1843_27C.a` | 0.758 mm | E⊥c | 395 | 53.8/37.8/−15.1 | 56.7/50.1/−15.0 | 5.1 |
| Corundum GRR 3614 violet | 6.26 mm | E⊥c | 380 | 88.3/9.3/−9.4 | 88.5/17.2/−12.0 | 5.9 |
| Corundum GRR 3614 violet | 6.26 mm | E∥c | 395 | 96.8/−4.9/17.3 | 97.2/3.5/13.8 | 10.3 |
| Emerald GRR 3570 | 0.859 mm | E⊥c | 380 | 73.1/−38.1/16.6 | 70.4/−27.3/7.4 | 6.4 |
| Emerald GRR 3570 | 0.859 mm | E∥c | 380 | 72.3/−41.0/2.5 | 68.5/−39.3/−7.7 | 7.1 |
| Emerald GRR 3570 | 0.859 mm | (2o+e)/3 | 380 | 72.8/−39.1/11.7 | 69.8/−31.1/2.2 | — |
| Alexandrite GRR 874 | 0.717 mm | E∥a = β | 380 | 74.1/−9.6/39.6 | 74.8/9.5/31.1 | 15.0 |
| Alexandrite GRR 874 | 0.717 mm | E∥b = γ | 380 | 51.8/−57.5/−13.4 | 45.2/−43.0/−31.4 | 13.4 |
| Alexandrite GRR 874 | 0.717 mm | view ∥ c, (β+γ)/2 | 380 | 64.5/−24.2/15.3 | 62.8/−4.6/4.4 | 14.7 |
| Tanzanite GRR 1265 | 3.00 mm (norm.) | E∥a = γ | 405 | 84.5/−11.7/13.2 | 84.1/−5.8/10.0 | 5.4 |
| Tanzanite GRR 1265 | 3.00 mm | E∥b = α | 405 | 76.7/11.7/−9.0 | 77.4/11.6/−7.2 | 1.3 |
| Tanzanite GRR 1265 | 3.00 mm | E∥c = β | 405 | 79.4/−1.6/−4.9 | 78.9/−1.7/−6.0 | 0.9 |
| Amethyst Anahí `ameth.bo` | 6.17 mm ⊥ c | o-ray | 380 | 68.6/10.6/−6.8 | 69.3/9.8/−5.2 | 1.3 |

**Observations relevant to your b\* defect** [derived]:

- A real natural ruby measured E⊥c (GRR 1843) has **b\* ≈ −13 to −15**, i.e. purplish red. GIA's computed E⊥c ruby colors are also negative in b\*: −24 to −31 at 100–375 ppma·cm.
- The ~473 nm Cr³⁺ window is real and deep. Ruby b\* turns positive only at high Cr·L (≥ ~1500 ppma·cm in GIA Appendix 2), in E∥c, or when other absorbers are present (Fe³⁺, h•-centres, Fe–Ti).
- So **a transparent 450–500 nm window is not by itself an error.** The "stated colors" that the old tables were compared against may be the faulty side. That cannot be decided without a measured same-sample color.
- The tanzanite file starts at 400.9 nm, so its b\* is uncertain, because the 380–400 nm region matters for b\*.

**Baseline issues found in the raw files** (they affect any absolute use):

- `ru1843.a` has A ≈ 0.22 at 800–1100 nm, where Cr³⁺ is transparent. The same stone's `corundum1843_27C.a` has 0.067. Two-surface Fresnel loss for corundum is about 0.07. So `ru1843.a` carries a ~0.15 offset, and **`corundum1843_27C.a` is the cleaner file.**
- The GRR 3614 files go negative (baseline subtracted below zero).
- GRR 1020a has a pedestal of 0.5–0.6.

---

## 2. P1 – Ruby

### 2.1 GIA sample 1110: synthetic Czochralski ruby. Both polarisations, analysed Cr

| Field | Value |
|---|---|
| Source | Dubinsky, Stone-Sundberg & Emmett 2020, G&G 56(1) 2–28; data xlsx (URL in §0) |
| Sample | Cr wafer 1110 (St Gobain, boule SS75), wafer plane ∥ c, SIMS-analysed [primary: Appendix 1]. The link between this wafer and the Cr value in the text is ~90 % likely, not stated outright. |
| Cr | 134 ppma ± 7.6 % (SIMS) [primary: text, Cr³⁺ section] |
| Cr basis | ppma of **all atoms**. 1 ppma = 5·N_A·ρ/M = 1.178×10¹⁷ cm⁻³, with ρ = 3.99 g cm⁻³ and M = 101.961 g mol⁻¹ [primary: Box A] |
| Cr converted | 134 × 1.178×10¹⁷ = 1.58×10¹⁹ cm⁻³ → 1.58×10¹⁹/6.022×10²³ × 75.995/3.99 = **0.050 wt% Cr₂O₃** = 3.35×10⁻⁴ Cr/(Al+Cr) [derived] |
| Fe, Ti | Not given numerically. The paper describes the crystal as high-purity, "Cr only" [primary] |
| Path length | **Not stated.** Only the thickness-independent σ is published |
| Quantity | σ = α/c in cm² per ion. α = 2.303·A/t is Napierian, and A is corrected for multiple internal reflections (Sellmeier index) [primary: Box A]. 1.5 nm resolution, 1 nm grid, 200–1100 nm |
| Measured color | None. Appendix 2 L\*a\*b\* are **calculated** (see §1a) |
| Data | xlsx columns "Cr3+ sigma, E⊥c / E‖c". Local copy, 200–800 nm: `data/ruby/GIA_Dubinsky2020_sample1110_Cr3+_cross_section_200-800nm.csv` (no header; columns are λ nm, σ⊥, σ∥) |

**Self-check ΔE00:** not applicable, because no measured color exists. The pipeline check against the calculated colors is in §1a.

**α at 450 / 475 / 500 nm for this sample** (Napierian, cm⁻¹; α = σ × 1.5785×10¹⁹ cm⁻³) [derived from primary σ]:

| λ | σ⊥ (cm²) | σ∥ (cm²) | α⊥ | α∥ |
|---|---|---|---|---|
| 450 | 3.074e-20 | 3.695e-20 | 0.485 | 0.583 |
| 475 | 1.472e-20 | 1.178e-20 | 0.232 | 0.186 |
| 500 | 3.630e-20 | 1.964e-20 | 0.573 | 0.310 |
| 560 | 1.621e-19 | 4.859e-20 | 2.56 | 0.77 |

**Other features:**

- The transmission minimum is at 472–474 nm, where the Cr³⁺ B-lines sit.
- The UV edge rises to σ ≈ 7.4×10⁻¹⁹ cm² at 200–202 nm. At 300 nm σ⊥ is only 4.3×10⁻²¹ cm² [primary: xlsx].

### 2.2 Caltech GRR 1843: natural ruby, Chanthaburi, Thailand. E⊥c only, no composition

| Field | Value |
|---|---|
| Files | <http://minerals.gps.caltech.edu/FILES/Visible/corundum/ru1843.a> (0.787 mm, 330–1099 nm, ~0.6 nm step)<br><http://minerals.gps.caltech.edu/FILES/Visible/corundum/corundum1843_27C.a> (0.758 mm, 390–1101 nm; recommended)<br>`_300C.a` and `_600C.a` hold the same piece at 300 and 600 °C |
| Polarisation | E⊥c, viewed down the c-axis [primary: page and header]. **No E∥c file exists** (six name variants tried, all 404) |
| Units | Decadic absorbance as recorded. Reflection loss is not removed (see §1b) |
| Composition | **Not published.** Not on the server and not in the Caltech corundum reference list |
| color | None measured. Computed in §1b |

**Absorbance values from `ru1843.a`** [primary]:

| λ (nm) | A |
|---|---|
| 413.7 (peak) | 1.797 |
| 450.2 | 0.755 |
| 474.1 (minimum) | 0.386 |
| 500.3 | 0.502 |
| 560.6 (peak) | 1.626 |

**Converted to α** (Napierian; α = 2.303·A/0.0787 cm) [derived]:

| λ (nm) | α raw (cm⁻¹) | α with the 0.217 offset removed (cm⁻¹) |
|---|---|---|
| 450 | 22.1 | 15.7 |
| 475 | 11.5 | 5.1 |
| 500 | 14.7 | 8.3 |

Do not use this file for the UV edge: the data stop at 330 nm.

### 2.3 What absorbs at 430–500 nm

The source for all items below is the GIA 2020 text and xlsx [primary] unless stated.

- **Cr³⁺ band flanks.** The flanks of the ~410 nm and ~560 nm bands meet at ~473 nm. α scales with Cr. GIA states that the Cr³⁺ cross-section is host-independent: the same in synthetic ruby, natural ruby and purple sapphire. Linearity at thousands of ppma is not demonstrated in the paper.
- **O²⁻→Cr³⁺ charge transfer.** The first CT band lies below ~226 nm. Its tail at 300 nm is already about 7× weaker than the Cr³⁺ absorption at 450 nm, so it contributes little at 430–500 nm [derived].
- **Fe³⁺.**
  - Fe³⁺ is a weak chromophore. The Fe³⁺–Fe³⁺ pair feature at 450 nm has σ ≤ 2.5×10⁻²⁰ cm².
  - The pair-band α grows as ~[Fe]^1.5.
  - xlsx σ⊥/σ∥ = 2.24/1.85×10⁻²⁰ cm² at 450 nm, 0.68/0.55×10⁻²⁰ at 475 nm and 0.23/0.15×10⁻²⁰ at 500 nm.
  - The Fe concentration these values belong to is not stated.
  - Other Fe³⁺ lines are at 377 and 388 nm.
- **color centres h•–Cr³⁺ and h•–Fe³⁺.** These are trapped holes, charge-compensated by Mg or Be.
  - They are the strongest absorbers per defect: σ⊥(450 nm) = 1.31×10⁻¹⁷ cm² (h•–Cr³⁺) and 9.1×10⁻¹⁸ cm² (h•–Fe³⁺).
  - That is ~400× the Cr³⁺ value.
  - Sub-ppma Mg can therefore close the 450–500 nm window.
- **Fe²⁺–Ti⁴⁺.** σ⊥(450 nm) = 4.5×10⁻¹⁹ cm². This matters in Fe- and Ti-bearing natural rubies.
- **Natural vs flux vs Verneuil ruby.** **Not found.** No primary source compares 450–500 nm α across growth types with analyses.
  - The main expected differentiators are Fe³⁺ (high in Fe-rich marble-free natural stones), and Mg or Ti, which create h•-centres or Fe–Ti pairs [review: inference from GIA 2020].
  - This is not a measured comparison.

---

## 3. P2 – Alexandrite and tanzanite

### 3.1 Alexandrite: Caltech GRR 874, Lake Manyara, Tanzania

| Field | Value |
|---|---|
| Files | <http://minerals.gps.caltech.edu/FILES/Visible/CHRYSOBeryl/ch874.a> (E∥a = β, "yellow")<br><http://minerals.gps.caltech.edu/FILES/Visible/CHRYSOBeryl/ch874.b> (E∥b = γ, "blue") |
| Path | 0.717 mm (both files) [primary: headers] |
| Rays | Two only. **No α (E∥c) file exists** for any chrysoberyl on the server |
| Axis mapping | As stated in the headers: a = β, b = γ, hence c = α. This is the Pnma cell (a 9.40, b 5.48, c 4.43 Å) [derived] |
| Range | 330–1099 nm |
| Units | Absorbance; reflection correction not stated |
| Cr | **Not analysed** |
| color | None measured. Computed in §1b: ΔE00 D65↔A of 13–15 per ray |
| Local copy | `data/alexandrite/ch874.a` only. `ch874.b` is linked |

**Band peaks** [primary]:

- β ray: 429 nm (A = 2.02) and 564 nm (A = 0.46).
- γ ray: 414 nm (A = 1.25), 596 nm (A = 1.91) and an R line at 679 nm.

Other Caltech chrysoberyl files: GRR 171, a yellow Fe³⁺ chrysoberyl from Minas Gerais, 1.193 mm, files `ch171.a` and `ch171.b`. It is not alexandrite and is useful only as an Fe³⁺ reference.

**Literature with Cr analyses, but no usable spectra or color:**

- Schmetzer 2019, G&G 55(1), synthetic stones: 0.18–0.26 wt% Cr₂O₃ (flux) and 0.30–0.43 wt% (HOC). It gives visual colors only [primary].
- Farrell & Newnham 1965, Am Min 50:1972. The sample is natural, measured at 77 K along a and b only. The composition is given only as the formula Al₁.₉₆(Cr,Fe)₀.₀₄BeO₄ (apfu, Cr+Fe combined). α is defined as (1/t)·log₁₀(I₀/I), i.e. decadic, with no reflection correction [primary].
- Sun et al. 2017, Am Min 102:1747. It has 3-axis spectra and CIE colorimetry on a synthetic Cr-chrysoberyl, but is **paywalled and not read.** It is the best lead for P2 alexandrite.

### 3.2 Tanzanite: Caltech zoisite files

Index: <http://minerals.gps.caltech.edu/FILES/Visible/zoisite/Index.html>

Axis mapping in the headers: E∥a = γ (yellow-green), E∥b = α (red-violet), E∥c = β (blue). This is the Pnma cell (a ≈ 16.2, b ≈ 5.55, c ≈ 10.0 Å; the cell itself is not printed on the server). Hurlbut 1969 (Am Min 54:702) agrees: X∥b, Y∥c, Z∥a [primary].

| Sample | Files (same directory) | Path | Composition | Notes |
|---|---|---|---|---|
| **GRR 1265**, Tanzania, natural | `zo1265.a/.b/.c` | 3.00 mm, "normalized" (original thickness not given) | V₂O₃ = 0.35, FeO = 0.027 in the header. **No unit or method is stated**; probably wt% oxide [unverified] | Range 400.9–1103.8 nm, so **380–400 nm is missing** |
| GRR 2292, Arusha, blue zone and yellow zone | `Zoisite2292_blue_a/b/c.asc`, `Zoisite2292_yellow_a/b/c.asc` | a and c rays: 3.150 mm (blue), 3.237 mm (yellow). b ray: 5.142 mm, from a different piece | None | 360–1124 nm. The yellow zone has a ~446 nm band (A = 0.75), the band that heating removes |
| GRR 2891, Merelani, sun-bleached blue | `Zoisite2891a/b/c.asc` | a and c rays: 5.587 mm. b ray: 6.810 mm, from a different piece | None | 390–4749 nm |
| GRR 3045, before and after heating at 500 °C (5.747 mm) | **JPG images only** | — | — | Same crystal, but no numeric data |

**Literature:**

- Pluthametwisute et al. 2020, *Molecules* 25:3743 (<https://pmc.ncbi.nlm.nih.gov/articles/PMC7465713/>).
  - Five natural stones, each measured before and after heating at 550 °C.
  - EPMA in wt% oxide, e.g. T04 V₂O₃ = 0.39/0.43/0.38 (three spots) [primary: Table 2].
  - Spectra along a, b and c appear in **Fig. 4 only**. No thickness is given and no CIELAB; colors are GIA GemSet codes.
  - Heating removes the ~460 nm band (strongest along c). The 585–600 nm band is unchanged [primary].
  - This is the best same-crystal before/after source. Using it requires digitising Fig. 4 and asking the authors for the thicknesses.
- Not reached: Tsang & Ghose 1971, Faye & Nickel 1970 (Caltech shows only a GIF of their before/after spectra) and Olivier 2006.

---

## 4. P3 – Single-sample checks

None of these meets the "composition analysed on the same sample" rule. They are listed so you can use the raw spectra if that rule is relaxed.

| Gem | Sample and file | Path / polarisation | Composition | Notes |
|---|---|---|---|---|
| Peridot | Caltech GRR 418, San Carlos: <http://minerals.gps.caltech.edu/FILES/Visible/olivine/ol418.a>, `ol418.b`, `ol418.g` | 1.141 mm (α), 1.1381 mm (β), 0.66 mm (γ; separate slab). Mapping from the index page: 10 Å axis = α, 6 Å = β, 4.7 Å = γ | **No Fa analysed** | Units unstated; probably absorbance. Lead: Runciman et al. 1973, Am Min 58:451 (Burmese peridot, microprobe FeO 9.97 wt% [unverified], polarised spectra in figures only) |
| Emerald | Caltech GRR 3570, Afghanistan: <http://minerals.gps.caltech.edu/FILES/Visible/Beryl/Emerald_3570_a.asc> (E⊥c) and `Emerald_3570_c.asc` (E∥c) | 0.859 mm | **No Cr or V analysis** | Other emerald files are on the same index (Zambia, Nigeria, Yemen, Ural, synthetics), none analysed. Lead: Kamakanga emeralds, *Crystals* 13:1605 (2023): Cr and V by LA-ICP-MS in ppmw, plus CIELAB, but spectra in figures only [unverified] |
| Citrine (heated amethyst) | Caltech GRR 3910, Rio Grande do Sul: **JPG only** (`quartz/Citrine_3910_spectrum.jpg`) | 2.29 mm | — | No data file |
| Citrine (natural) | **Not found** | — | — | — |
| Amethyst (reference) | <http://minerals.gps.caltech.edu/FILES/Visible/quartz/ameth.bo>, Anahí, Bolivia, unheated | 6.17 mm, slab ⊥ c (o-ray) | None | Computed color in §1b. A Sci. Rep. 2020 heating study (doi 10.1038/s41598-020-71786-1) gives only population-mean CIELAB, so it is not usable under the same-sample rule |
| Blue sapphire, E∥c | Caltech GRR 1020a: <http://minerals.gps.caltech.edu/FILES/Visible/corundum/sa1020a.am> (E⊥c) and <http://minerals.gps.caltech.edu/FILES/Visible/corundum/sa1020a.cm> (E∥c) | Headers say 4.289 mm; the index page says "normalized to 3.00 mm" (**unresolved**) | **None** | 350–1600 nm. Pedestal of 0.5–0.6 A. Good for band shape, not for absolute α |

The GRR 1020a sapphire is the only blue sapphire found with both polarisations, at the same nominal path, from one source.

- **E⊥c** [primary]: A = 1.225 at 580 nm and 1.468 at 701 nm. The maximum, ~2.66, is at 870–890 nm (Fe²⁺–Fe³⁺).
- **E∥c** [primary]: A = 0.603 at 580 nm and 0.879 at 700 nm.

Other Caltech blue and violet corundum, none analysed:

- GRR 3615, Colombia, 4.44 mm. The E∥c header says "xxx mm".
- GRR 3614, violet, Colombia, 6.26 mm, both polarisations.
- GRR 1798, Montana. `grr1798.a` is 404.
- The zoned Laos crystal, `corundum/sa1870/index.htm`.

---

## 5. Concentration-unit conversions (corundum)

All derived, with ρ = 3.99 g cm⁻³ and M(Al₂O₃) = 101.961 g mol⁻¹ (GIA 2020):

- 1 ppma (all atoms) = 1.178×10¹⁷ cm⁻³ = **2.5 ppm of Al sites**. Al₂O₃ has 2 Al among 5 atoms.
- 1 ppm of Al sites = 4.713×10¹⁶ cm⁻³ = 0.400 ppma (all atoms).
- Cr: 1 ppma (all atoms) ≈ 3.73×10⁻⁴ wt% Cr₂O₃, i.e. 100 ppma ≈ 0.037 wt% Cr₂O₃.
- V: 0.26 wt% V₂O₃ = 0.26/100 × 3.99 g cm⁻³ × 2 × N_A / 149.88 g mol⁻¹ = 8.34×10¹⁹ V cm⁻³ = **708 ppma (all atoms)** = 1770 ppm of Al sites.
  - Your earlier label of "305 ppma" fits none of these bases.

---

## 6. color-change V-corundum (P4 row 7)

**No color-change corundum was found that has both a measured spectrum and a measured D65/A color.** The closest primary material is the following.

**GIA 2020, V³⁺-doped synthetic** [primary: text and Figs 5–6]:

- 116 ppma V³⁺ by SIMS; 6.26 mm thick; both polarisations; heated at 1750 °C in O₂ for 10 h.
- σ is in the xlsx.
- GIA shows calculated color circles only, with no numeric ΔE. It judges the E∥c and mixed views to qualify as color change.
- Caution: the text quotes a V³⁺ σ of 1.0×10⁻¹⁹ at 580 nm, but the xlsx gives 3.4×10⁻²⁰ at 580 nm (E⊥c). The xlsx maximum is 9.8×10⁻²⁰ at 400 nm (E⊥c), and 4.6×10⁻¹⁹ at 405 nm (E∥c). Check against Fig. 5 before relying on either.

**color change computed from that measured cross-section** with your method [derived; script output in the zip, `selfcheck/gia2020_V_FeTi_5nm_4sig.csv`]:

| V·L (ppma·cm) | ΔE00 E⊥c | ΔE00 E∥c | ΔE00 (2⊥+∥)/3 |
|---|---|---|---|
| 110 (≈ 1 mm at 1100 ppma, or 1 cm at 110) | 8.0 | 13.0 | 11.4 |
| 340 | 13.6 | 24.9 | 18.7 |
| 1100 | 10.3 | 22.1 | 12.6 |
| 3400 | 8.3 | 9.3 | 9.0 |

**What this shows:**

- From a measured V³⁺ spectrum, ΔE00 between D65 and A is **8–25**, peaking in E∥c at a few hundred ppma·cm.
- Report 1's 2.9–5.8 is too small. Report 2's "often > 25" is at the upper edge and would hold for ΔE\*ab rather than ΔE00: at 340 ppma·cm E∥c, ΔE\*ab ≈ 23.
- Both numbers depend on the adaptation model. Lab relative to each illuminant's own white is an implicit von-Kries-in-XYZ adaptation; a CAT02/CAT16 adaptation would give different values.
- This is a derived prediction, not a measured color.

**Measured color-change ΔE exists only for analogues:**

- Pyrope–spessartine garnet: ΔE\*ab = 9.10–14.87 (D65 vs A, 2°) across ten stones [unverified: Qiu & Guo, *Minerals* 11:865, 2021, abstract only].
- Corundum color-change reports (GIA Lab Notes 2019 Burmese V-sapphire; Fuping, China, *Crystals* 12:463, 2022) give compositions and qualitative color, with no CIELAB [unverified].

---

## 7. P4 – Contradictions

| Question | Primary literature | Verdict |
|---|---|---|
| Fe²⁺–Ti⁴⁺ pairing law | Emmett et al. 2003, G&G 39(2) 84–135, worked examples: Ti first compensates Mg, and the remainder pairs with Fe when Fe is in excess. So pairs = [Ti] − [Mg] while positive. Emmett et al. 2017, G&G 53(1) 42–47, adds Si as a donor that pairs with Mg first. GIA 2020 pp. 18–19: [Ti]avail = [Ti] + ([Si] − [Mg] − [Ni]) when that bracket is < 0 [primary]. For Czochralski synthetics the pair count also depends on annealing: a 750 °C anneal raised the 580 nm absorption 2–2.5× [primary: GIA 2020] | **Report 1 is correct**: linear in available Ti (Ti − Mg + Si), with Fe in excess. No GIA source uses an [Fe]·[Ti] product. Townsend 1968 and Moon & Phillips 1994 were not accessible |
| Fe²⁺–Ti⁴⁺ absorption per pair | σ⊥ = 1.94×10⁻¹⁸ cm² ± 25 % at 580 nm [primary: GIA 2020 text; xlsx 1.98×10⁻¹⁸]. σ∥ = 1.256×10⁻¹⁸ at 703 nm [primary: xlsx]. The magnitude is the average of 14 Yogo wafers and the band shape comes from synthetics, so **this is not a single-sample value.** Per ppma (all atoms): α⊥ = 1.94e-18 × 1.178e17 = **0.229 cm⁻¹ Napierian = 0.099 cm⁻¹ decadic**. Per ppm of Al sites: 0.0914 Napierian, 0.040 decadic [derived] | **0.28 cm⁻¹/ppma is not supported.** Your ~0.05 per ppm of Al sites matches the decadic value (0.040) within ±25 %. GIA also notes that non-Yogo, high-Fe naturals show 2–3× larger apparent σ, of unresolved cause |
| Ruby band FWHM | Band letters are a trap: **GIA 2020 calls the ~560 nm band "U" and ~400 nm "Y"**, opposite to the classical ruby usage. FWHM measured from sample 1110 by half height, without deconvolution [derived]: ~410 nm band 3290 cm⁻¹ (⊥) and 3020 cm⁻¹ (∥); ~560 nm band 2300 cm⁻¹ (⊥) and 2390 cm⁻¹ (∥) | Report 1's 3180–3350 cm⁻¹ matches the **410 nm** band. ~2630 cm⁻¹ matches neither band, and is probably a fitted width or a different baseline. Unresolved without McClure 1962 |
| Ruby Y/U peak ratio | σ(560 nm)/σ(410 nm) for sample 1110: **0.98 in E⊥c** and **0.18 in E∥c** (the E∥c short-wave peak is at 398 nm). Natural GRR 1843 E⊥c: 0.90 [derived] | Strongly polarisation-dependent. Any single "Y/U" number without a polarisation is meaningless. A "1.7×" factor cannot be checked without knowing which band report 2 called Y |
| Alexandrite axis mapping | Physical assignment, consistent across sources: α = 4.43 Å axis, β = 9.40 Å axis, γ = 5.48 Å axis. Pnma (Farrell & Newnham 1965; Caltech): α∥c, β∥a, γ∥b. Pbnm (Schmetzer 2019): α∥a, β∥b, γ∥c [primary]. "α∥c, β∥b, γ∥a" equals the Pmnb setting [derived]. Pleochroic colors agree: α purple, β yellow-orange, γ blue-green | **Not a physical disagreement.** The difference is only the cell setting. Store mappings by axis length, not by letter |
| Tanzanite axis mapping | Pnma (Hurlbut 1969; Caltech headers): α∥b (5.55 Å, red-violet), β∥c (10.0 Å, blue), γ∥a (16.2 Å, yellow-green). Other sources (e.g. Schmetzer & Bank; Pluthametwisute 2020) write X∥a, Y∥b, Z∥c, which equals the Pmcn relabelling [derived]. colors agree | Same conclusion: **setting relabelling, not a physical conflict.** Caution: some papers write "X axis" meaning the optical direction X, not the crystallographic a |
| V-corundum color change ΔE | See §6 | Measured value **not found**. Derived from a measured spectrum: ΔE00 8–25 |

---

## 8. P5 – Fluorescence (low priority; mostly gaps)

Tags: `[review]` = secondary source; `[estimate]` = not from a source.

| Centre | Lines | Lifetime / yield | Fe quenching | Source |
|---|---|---|---|---|
| Ruby (Cr³⁺:Al₂O₃) | R1 ≈ 14403 cm⁻¹, R2 ≈ 14443 cm⁻¹ (≈ 694.3 / 692.4 nm). Shift −0.140 cm⁻¹/K. Excitation through the ~400 and ~555 nm bands | Lifetime "a few ms". QE 0.83 ± 0.10 is a **coating-level** value (ruby pigment, including escape losses), not the intrinsic yield. Cr self-quenching above ~3 wt% Cr₂O₃ | **Not found** | [review: NIST J. Res. 122:043 (2017)]; [primary: Berdahl et al., Sol. Energy Mater. Sol. Cells 157 (2016), abstract] |
| Emerald (Cr³⁺:beryl) | R1 683.8 nm (Colombia), 684.0 nm (Afghanistan), 684.2 nm (Zambia) | — | PL count rate falls exponentially with Fe, by 10× at 2200 ppm Fe (basis unstated; probably ppmw). If the decay is pure exponential, the intensity halves at ≈ 2200·log₁₀2 ≈ **660 ppm Fe** [estimate, derived] | [primary: GIA G&G Fall 2017, "Photoluminescence spectra of emeralds", abstract-level, unverified] |
| Alexandrite (Cr³⁺:BeAl₂O₄) | Mirror site R ≈ 678–679 nm (~70 % of Cr, τ ≈ 2 ms). Inversion site ≈ 690 nm (τ 26–53 ms) | — | Natural stones contain 600–10,000 ppm Fe (basis unstated). No halving value | [unverified: *Minerals* 13:419 (2023)] |
| Diamond NV⁰ / NV⁻ | ZPL 575 / 637 nm. Radiative τ 19.2 / 21.7 ns (nanodiamond) | QY ~1 for NV⁰; ~1 falling to 0.5 with pump power for NV⁻ (arXiv:1501.03714). A conflicting study (arXiv:1712.07877) gives 5 % to ~0.55, depending on host size | n/a | [primary: arXiv abstracts] |
| Diamond N3 / H3 | ZPL 415.2 / 503.2 nm | **Not found** | n/a | [review: Zaitsev 2001, from memory, not re-verified] |
| Mn²⁺ in calcite | CL band 540–675 nm. Activation at ~15–30 ppm Mn | — | No quenching up to 200 ppm Fe (cathodoluminescence, not UV PL) | [unverified: Neth. J. Geosci., CL experiments] |
| Spinel, willemite, kunzite | — | — | — | **Not found** |

**Not found for any centre:**

- Intrinsic quantum yields, except NV.
- Vibronic sideband shapes.
- Excitation spectra over 300–420 nm.
- 365 nm : 254 nm excitation ratios.
- Primary room-temperature linewidths.
- Fe-halving concentrations, other than the derived emerald estimate.

---

## 9. Looked for and not found

1. A ruby with **analysed Cr, Fe and Ti, a stated path length, and both polarisations**, other than GIA 1110 (which has no Fe, Ti or path length).
2. Analysed flux or Verneuil rubies with 450–500 nm spectra.
3. **Any gem sample with a measured CIELAB (D65 or A) and its own spectrum.** This covers ruby, sapphire, color-change sapphire, alexandrite, tanzanite, peridot, emerald and citrine.
4. A Caltech GRR 1843 E∥c spectrum and composition.
5. A Caltech chrysoberyl α-ray spectrum and any Cr analysis.
6. Numeric same-crystal before/after-heating tanzanite spectra (images and figures only).
7. Natural citrine with a digital spectrum; a digital heated-amethyst spectrum.
8. Fa content for Caltech GRR 418.
9. Cr and V for any digitally available emerald spectrum.
10. Composition of Caltech sapphires 1020a, 3614 and 3615. The 4.289 mm vs 3.00 mm path question for 1020a.
11. GIA 2020 Appendix-1 compositions and thicknesses for the individual Fe–Ti (Yogo) samples.
12. Full texts not obtained:
    - McClure 1962, J Chem Phys 36:2757
    - Wood et al. 1963
    - Fritsch & Rossman 1987–88
    - Townsend 1968
    - Moon & Phillips 1994
    - Tsang & Ghose 1971
    - Faye & Nickel 1970
    - Sun et al. 2017 (Am Min, alexandrite colorimetry)
    - Schmetzer & Bank 1980–81 (color-change corundum)
13. RRUFF (unreachable) and GIA, SSEF or Gübelin open data. No openly downloadable per-sample spectrum-plus-color sets were found at these labs.

**Best next steps** (ranked by expected yield):

- **(a)** Obtain Sun et al. 2017 (Am Min 102:1747). It has alexandrite spectra plus CIE colorimetry on one sample.
- **(b)** Email G. Rossman (Caltech) for compositions of GRR 1843, 1020a, 874 and 1265. Several were probably analysed for publications.
- **(c)** Ask GIA (Dubinsky) for Appendix-1 thicknesses and the Fe and Ti values of sample 1110.
- **(d)** Digitise Pluthametwisute 2020 Fig. 4 for tanzanite before and after heating.

---

## Attachments (zip)

| Path | Contents |
|---|---|
| `data/ruby/` | `caltech_GRR1843_ruby_Chanthaburi_Eperpc_0.787mm.a`; the GIA sample 1110 Cr cross-section CSV; `NOTES.md` |
| `data/sapphire/` | `sa1020a.am`, `sa1020a.cm`, `GRR3615_blue_Colombia_perp_c.asc`; `NOTES.md` |
| `data/alexandrite/` | `ch874.a`; 10 nm digest; `NOTES.md` |
| `data/tanzanite/` | Digest of all Caltech zoisite files; `NOTES.md` (no raw files; all are linked above) |
| `data/peridot/` | `ol418.a`, `ol418.b`, `ol418.g`; `NOTES.md` |
| `data/emerald/`, `data/citrine/`, `data/fluorescence/` | `NOTES.md` each |
| `selfcheck/cie.py` | The color method used throughout (CIE 1931 2°, D65/A, 5 nm) |
| `selfcheck/caltech_grid5nm.json` | Caltech spectra on the 5 nm grid used for §1b (derived) |
| `selfcheck/gia2020_V_FeTi_5nm_4sig.csv` | GIA V³⁺ and Fe–Ti σ at 5 nm, rounded to 4 significant figures (derived) |

The per-gem `NOTES.md` files are the research agents' working notes. They contain more detail, and every value in them is tagged.
