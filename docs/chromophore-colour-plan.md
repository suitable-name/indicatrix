# Physically based gem color — implementation guide

Status: plan (2026-10), revised after the user-supplied report "Quantitative
Optical Spectroscopy and Comprehensive Chromophore Catalogue for Natural,
Synthetic, and Amorphous Gemstone Hosts" (48 pp., 23 references, cited below
as *Report*) and after a 25-point plan review (log at the end). Companion data:
`docs/chromophores/*.md` (research files merged with the Report; status in
§2.4). Related: `crates/indicatrix/docs/physics.md` §3, manual chapter 6
(materials).

## 0. Summary

Custom materials get two color modes:

- **Fantasy mode** — today's behaviour: any color, no physical constraint.
  Gains a free color picker in addition to the nine presets.
- **Physics mode** — the user picks a *host crystal* and the *color-giving
  elements* that really occur in it (Cr in corundum, Fe+Ti in sapphire, V in
  tanzanite, …), each with a concentration. The color is computed from
  absorption spectroscopy. Picking a color solves back to elements and
  fractions. Both directions are always in sync.

The chromophore recipe is resolved **on the CPU into ordinary
`AbsorptionBand`s** (`crates/indicatrix/src/optics/absorption.rs`), so the
renderer, the GPU material layout, the WGSL, the net protocol (v19) and every
existing material stay untouched and bit-identical. `GemMaterial` gains no
field. New physics lives in a new module, a new colorimetry function, a
solver, the custom-material data formats and the material editor UI.

**The recipe is the single source of truth in physics mode; the color picker
is an input event, never written back from the forward model** (§4.4).

Fluorescence (UV and visible-excited response) is a separate, later stage (§9).
The data model already carries what it needs.

## 1. Current state (verified by code survey)

| Concern | Today | File |
|---|---|---|
| Band model | `AbsorptionBand { center_nm, width_nm (σ; nm or cm⁻¹ by shape), peak, shape }`, `BandShape::{GaussianWavelength, GaussianEnergy}`; `AbsorptionBand::energy(center, fwhm_cm⁻¹, peak)` | `optics/absorption.rs:17-140` |
| Tensor | `AbsorptionTensor { o_ray, e_ray, beta_ray: Option, is_pleochroic }` with `isotropic/uniaxial/biaxial` | `absorption.rs:297-347` |
| Transport | Beer–Lambert per channel, eigenmode-projected tensor (`channel_absorption_alphas_assigned`), `exp(-α·L·absorption_path_scale)`; with inclusions, `scattering/hg.rs::maybe_scatter_or_extinguish` samples one free path from `σ_t,hero = α_hero + σ_s` and weights absorption analytically (no absorption event) | `raytracer/absorption.rs:28-200`, `raytracer/scattering/hg.rs:94` |
| Spectral range | 380–780 nm, 8 channels: wrapped hero comb, spacing 50 nm (`wrapped_hero_wavelengths`) | `transport/mod.rs:54-59` |
| Units | `peak` is per **model unit**; `absorption_path_scale` (default 1.0) converts model → absorption-length units; `render_setup::apply_material_overrides` sets it to `stone_width_mm / model_width` **only if** `stone_width_mm > 0` and `model_width` measured (> 1e-9); `with_absorption_path_scale` *replaces* the scale | `render_setup/materials.rs:115-121`, `materials/custom.rs:311-314` |
| Stone width sources | desktop: `ctx.stone_width_mm` set from the open design's `girdle_diameter_mm.unwrap_or(0.0)` (`gui/editor/view/inspector.rs:353`) or the global setting (`DEFAULT_STONE_WIDTH_MM = 0.0`; `gui/render/material_quality.rs:410`, `gui/startup_settings.rs:198`); read by the render thread (`context/frame.rs:129`, `context/materials.rs:37`) and by export (`export_thread/scene_snapshot.rs:132`). Web-core: `render_material` (`settings.rs:335/341`) → `material_overrides` → `scene/mod.rs:427` | |
| GPU | `MAX_ABSORPTION_BANDS = 8` **per eigenmode**; bands inline in `GpuGemMaterial` (offsets asserted) | `renderer/buffers/material.rs:93-284` |
| Custom color | 9 presets (`BODY_COLOR_PRESETS`) → `legacy_rgb_bands(rgb)` = 3 bands at 620/540/450 nm; *no free picker*; no body-color computation (swatches are hard-coded hex) | `materials/body_color.rs`, `absorption.rs:375`, `ui/components/material_editor/color_presets.slint` |
| Editor | `material_editor_dialog.slint` (modal; dirty baseline only, lines 94-114) → `save_material(…, color_idx, …)` → `gui/optics/custom_materials.rs::apply_custom_material_save` → `GemMaterial::new_custom` | |
| Storage | vault `custom_gem_materials` (`absorption_r/g/b`, additive migrations; rows shared across designs); native `CustomMaterialSnapshot.absorption_rgb` (+ flattened `unknown`, no version bump for optional fields); web-core `CustomMaterialSpec` (postcard, web `PROTOCOL_VERSION = 8`) | |
| Per-design override | `MaterialSelection.body_color_override: Option<[f32;3]>` → `with_body_color` (replaces the tensor, collapses to isotropic) | `cut-core/src/material/selection.rs` |
| Lighting | `LightingPreset::Incandescent` ("Incandescent (3200K)", Planckian 3200 K → D65 adaptation, physics.md:246/314) built on `blackbody_spectrum(lambda_nm, temp_k)` (normalised to 1.0 at 560 nm) | `raytracer/environment/mod.rs:83`, `environment/spectral.rs:84` |
| Metrics | color metrics ignore absorption; cache key = `Debug` of `GemMaterial` | `color/metrics/cache.rs:42` |

Consequences for the design:

1. A recipe must compile to **≤ 8 Gaussian bands per eigenmode** (o, e, β).
2. Keep `GemMaterial` / `AbsorptionTensor` unchanged → no protocol bump, no
   re-baselined `render_setup_pins`, goldens untouched.
3. Absorption coefficients must be expressed per mm and the material must be
   rendered with a real stone size (§3.4).
4. A body-color function (spectrum → color) does not exist yet and is needed
   by the swatch, the solver and the sanity tests.

## 2. Chromophore data

### 2.1 Concepts

- **Host** — a crystal (or glass) with an optical character
  (isotropic/uniaxial/biaxial), a UV edge, and the list of chromophores that
  occur in it. A host maps to built-in material names (e.g. Corundum → Ruby,
  Sapphire) so templates can preselect it. Biaxial hosts carry
  `axis_mapping_confidence` (`verified|secondary|unknown`) for the a/b/g ↔
  crystallographic-axis assignment (§2.4).
- **Chromophore** — what absorbs. `kind`:
  - `ion` — one ion on one site (Cr³⁺ in corundum). α ∝ c.
  - `ivct_pair` — intervalence charge transfer between two ions on adjacent
    sites (Fe²⁺–Ti⁴⁺ and Fe²⁺–Fe³⁺ in corundum, Fe²⁺–Fe³⁺ and Mn²⁺–Ti⁴⁺ in
    tourmaline). Needs both elements; the pair appears automatically when both
    are present (§2.3). Its effective concentration follows a **per-pair**
    `pairing` law stored in the data, never a global default:
    - `random` (disordered solid solution): c_pair = z·c_A·c_B / N_sites, with
      z the number of qualifying neighbour sites and N_sites the host's site
      density, so c_pair ≤ min(c_A, c_B) automatically at gem concentrations;
    - `clustered` (charge-compensated, ordered or clustered arrangement):
      c_pair = min(c_A, c_B).
    - **Corundum Fe²⁺–Ti⁴⁺ is `clustered`** (charge-compensated pairing:
      Emmett 2003; Dubinsky/Stone/Emmett 2020); Fe²⁺–Fe³⁺ in corundum is
      `clustered` provisionally (flag `pairing_confidence = "estimate"`);
      tourmaline pairs stay `random` until C2 decides otherwise.
    The coefficient is per unit of *pairs*, so pair and single-ion
    coefficients are comparable and checkable. Reason for per-pair data: with
    `random` pairing and the Report's σ, Fe 1000 ppm + Ti 100 ppm gives
    α ≈ 0.03 cm⁻¹, 29× below the empirical ≈ 5e-5 cm⁻¹/ppm² × 1000 × 100 ≈
    5 cm⁻¹, i.e. blue sapphire would render colorless. The empirical
    per-ppm² coefficients are kept in the data as optional secondary field
    `peak_coeff_empirical_per_ppm2` (used only for the consistency check and
    until C2 re-derives the pair coefficients; they are not used by the
    forward model).
  - `end_member` — color from an essential cation of a solid-solution series
    (Fe²⁺ in almandine, Mn²⁺ in spessartine). Concentration is a mole fraction;
    fractions of one host sum to exactly 1 with an explicit colorless
    remainder end member (§5.1, garnet).
  - `color_centre` — a defect, not an element (amethyst [FeO₄]⁰, smoky
    [AlO₄]⁰, blue-topaz O⁻, sapphire [h•–Fe³⁺], diamond N3/H3/NV⁻/GR1). It is
    *produced* by a treatment acting on a precursor element (§2.6), with an
    intensity bounded by the precursor's amount.
  - `colloid` — metal/chalcogenide particles in glass (Au, Ag, Se).
- **Band** — `centre_nm`, `fwhm_cm1`, shape (`gaussian_energy` default),
  `peak_coeff` (cm⁻¹ per concentration unit at peak), polarization weights
  (`{o,e}` uniaxial, `{a,b,g}` biaxial, `{o}` isotropic), and optional
  `sigma_cm2`, `epsilon_l_mol_cm`, `transition_class`
  (`spin_allowed|spin_forbidden|ivct|charge_transfer|centre`) so that the
  consistency checks (§2.4) can run on the schema rather than on comments.
  Calibration-only entries (IR lines, ZPL-only data, e.g. the diamond C-centre
  8850 nm / 1130 cm⁻¹ line) do **not** live in `[[band]]`; they go in the
  chromophore's `calibration` field (`{ centre_nm, label, coeff, note }`) and
  never reach the validator or the forward model.
- **Rules** — `requires`, `excludes`, `quenched_by` (fluorescence only),
  `conc_typical`, `conc_max`, and host-level `rules` (e.g. garnet: Σ
  end-member fractions = 1; Cr/V color change only with x_Alm ≤ 0.10 and
  Cr₂O₃+V₂O₃ ≈ 0.5–1.5 wt%).
- **Bound semantics.** `conc_max` / `conc_typical` are analysed element totals
  in `conc_unit`, for the stones the chromophore colors. A multi-element
  chromophore bounds the sum of its elements (the loader scales them down
  together). The recipe bound of an element is the largest `conc_max` of the
  chromophores it can feed under the recipe's treatments.
- **Band centre = vibronic envelope maximum, not the zero-phonon line.** The
  Report lists ZPLs as centres for diamond N3 (415.2), H3 (503.2), H4 (496.0)
  and NV⁻ (637 vs. its own prose 560 nm). A broad Gaussian placed at a ZPL
  puts the absorption in the wrong place. Model a defect as its envelope band
  plus, optionally, a narrow ZPL band (FWHM ≈ 100–300 cm⁻¹); the normaliser
  must convert every ZPL-centred entry (NV⁻ envelope 560 nm and N3 ZPL 415.2
  nm agree with independent values).

### 2.2 Canonical data file

`crates/indicatrix/data/chromophores.toml`, embedded with `include_str!`,
parsed once into an ordered, validated `ChromophoreCatalogue` (`OnceLock`).
Schema = the research files' schema (`docs/chromophores/*.md`) with these
normalisations:

- **Unknown is absent**, not a sentinel. The research files use both `-1.0`
  and `0.0` for unknown; the normaliser must map both to a missing field.
- **Confidence** is an enum with strict meanings:
  `measured` (fitted by us to a measured spectrum with known thickness and
  composition, §2.4), `verified` (number read in the primary source),
  `secondary` (review/handbook), `estimate` (derived; reasoning recorded),
  `unknown`. The research agents' "verified" meant *seen in a search snippet*
  and must be downgraded to `secondary` during normalisation.
- `data_version: u32` at the top; bumped whenever any number changes.
- **Concentration bases are explicit.** Allowed units: `ppm_site` (per cation
  site), `ppma_all` (per all atoms; corundum: 1 ppma_all = 2.5 ppm_site Al),
  `wt_pct_oxide:<oxide>` (oxide named, e.g. `Cr2O3`), `mol_fraction`
  (end members), `intensity` (color centres). The Report mixes them: "wt%"
  without an oxide, "ppm" without basis, ppma vs. site ppm differing between
  corundum and YAG, Ho in ppma under a wt% header. Each host carries its
  number densities (formula units, cation sites per cm³) so the loader
  converts everything to sites per cm³ internally. **`conc_unit` is frozen per
  chromophore id**: changing a unit basis means a new id (the old one is kept
  as deprecated and resolves via its stored `resolved_bands`, §6).
- **Suspect values** carry `suspect = "<reason>"`. They stay in the data for
  traceability but are excluded from physics mode until resolved (§2.4).
- Stable ids: `"<host>/<chromophore>"`, e.g. `"corundum/Cr3+"`,
  `"corundum/Fe2+-Ti4+"`. Ids never change once released (recipes store them).
- **Loader-time band normalisation.** (a) Weak bands adjacent to a UV edge
  (e.g. Fe³⁺ 377/388 nm in the same chromophore) are merged into the edge band
  at load time when both lie within 1 FWHM and the merged edge differs from
  the sum by ΔE₀₀ < 0.5 at the reference conditions; the merge is a
  load-time data transformation and its result is a test fixture. (b) UV edges
  are attached to the chromophore that causes them (Fe³⁺, N) where the data
  say so; a host-level edge exists only where the edge is intrinsic (host
  band gap, Urbach tail). Test: a pure host with its edge present has
  Y ≥ 0.98 at 10 mm (colorless).

Validator (unit tests, run in CI):

- ids unique; every `requires`/`excludes`/`quenched_by` id exists in the host;
- polarization keys match the host's optical character;
- **per band**: `fwhm_cm1 > 0`, `peak_coeff ≥ 0`, `centre_nm` finite and within
  250–1200 nm (bands outside 380–780 nm are allowed, their tails matter);
- `conc_typical ⊆ [0, conc_max]`; units from a fixed list;
- every `materials` name exists in the built-in table;
- hosts and chromophores sorted canonically (determinism, §8).

### 2.3 Selectable vs. internal chromophores

The UI shows **elements and treatments**, not raw chromophores:

- An element row (e.g. "Fe") may drive several chromophores (Fe²⁺, Fe³⁺, and
  the Fe²⁺–Ti⁴⁺ pair if Ti is also present). Valence split per host is a
  data field (`valence_split`, default from the host's typical redox state;
  advanced users can change it). A species that exists only in a treated state
  (the copper-ruby colloid, prasiolite Fe²⁺) has fraction 0 in the default
  split and is populated by the treatment's `valence_shift`, so the exclusion
  between the two states never fires on an untreated recipe.
- Pairs and `requires` are enforced: selecting Ti alone in corundum gives no
  color (correct — Ti⁴⁺ alone is colorless); the pair activates when Fe is
  added.
- `excludes` disables impossible combinations in the UI with a tooltip
  ("V and Cr color change in garnet only occurs in pyrope–spessartine").
- **Offer rule is per band.** A chromophore is offered if at least one of its
  bands has `centre_nm`, `fwhm_cm1` and `peak_coeff`. A band without a
  coefficient (corundum Cr³⁺ R1 694.3 nm, chrysoberyl Cr³⁺ R1 680.4 nm, any
  ZPL) is skipped by the forward model with a note in the Sources popover; the
  chromophore stays offered. A chromophore with no usable band is hidden.
  `estimate` data are offered; `unknown` bands are skipped. Test: Corundum
  offers Cr, Fe, Ti, V; Chrysoberyl offers Cr; Diamond offers N (C-centre) —
  asserted by id.

### 2.4 Data status and calibration (blocking for "physics")

After merging the Report: 24 hosts (cordierite is a stub — the Report names
it but gives no data; its host block is commented out since 2026-10-12, so the
catalogue loads 23 hosts), 108 chromophores, 208 bands, 95 fluorescence entries.
Band confidence as normalised: 195 secondary, 10 estimate; 17 bands still
lack a width or a coefficient. The earlier "3 verified" count is **not** a
count of verified visible absorption coefficients: the diamond C-centre entry
is an IR calibration line (moved to `calibration`, §2.1) and Pr³⁺:CZ is
position-only. **The effective number of verified visible coefficients is 0**;
`measured`/`verified` may be set only through C2. Each research file lists its
discrepancies (prior vs. Report) and the Report-internal inconsistencies.

What the Report settles and what it does not:

- **Settled enough to build on** (secondary): band positions, widths and
  polarization directions, including the pleochroism rules for IVCT pairs
  (corundum Fe²⁺–Ti⁴⁺ o-ray w_o = 1.0/w_e = 0.1; Fe²⁺–Fe³⁺ e-ray w_e =
  1.0/w_o = 0.3; tourmaline Fe²⁺–Fe³⁺ and Mn²⁺–Ti⁴⁺ o-ray; Paraíba Cu²⁺ e-ray
  w_e = 1.0/w_o = 0.2), treatment transitions (§2.6) and fluorescence
  behaviour (Cr³⁺ Φ ≈ 0.9, quenched completely above 0.5–1.0 wt% FeO).
  Spot checks that agree with independent values: ruby σ ≈ 1.6–1.7×10⁻¹⁹ cm²
  (→ 51 cm⁻¹/wt% Cr₂O₃), Fe²⁺–Ti⁴⁺ ε ≈ 500 L·mol⁻¹·cm⁻¹, NV⁻ envelope 560
  nm, N3 ZPL 415.2 nm, zircon U⁴⁺ 653.5/690 nm, zircon "Dy³⁺ 580 nm" = Nd³⁺.
- **Not settled — absorption strength per concentration.** The Report does
  not tie numbers to its references, and many of its peak coefficients are
  *identical* to the earlier recalled estimates (YAG, Co²⁺:spinel,
  Fe²⁺–Ti⁴⁺, garnet), so their agreement is not independent confirmation.
  Its σ and α columns disagree internally: Cr³⁺:corundum 25.9 cm⁻¹/wt% is a
  factor-2 slip (half the true Cr density; the correct value is ≈ 51);
  diamond C-centre ×6.8, GR1 ×10 (GR1 is internally inconsistent), YAG Ce³⁺
  39 %. Several values are an order of magnitude off (topaz Cr³⁺ ≈ 30× beryl
  Cr³⁺; heliodor Fe³⁺ ε outside the Report's own spin-forbidden range; opal
  Cu²⁺/Ni²⁺; spinel Mn²⁺; garnet IVCT 20–30× below its own ε; sapphire
  [h•–Fe³⁺] 7 cm⁻¹/ppma is ≈ 10× too strong). Corundum Fe³⁺ 450 nm is
  pair-enhanced (∝ c², not ∝ c), so linear modelling is an approximation and
  carries `suspect = "pair-enhanced, linear approximation"` only if it fails
  the 2× check. Some rows are mislabelled (p. 12 "YAG" Co/Cr/Fe rows are cubic
  zirconia; tourmaline "Fe²⁺ 430 nm" likely Fe³⁺).
- **Biaxial axis mapping is stored by axis length.** The a/b/g pol keys are the
  vibration directions alpha, beta, gamma (smallest to largest refractive index); the
  crystallographic letters differ with the cell setting (the "reversed" tanzanite and
  chrysoberyl assignments of the prior and the Report are Pnma / Pbnm / Pmnb relabellings, primary-data
  report section 7), so `ray_axis_angstrom` names the axis length each ray vibrates along
  (chrysoberyl alpha 4.43, beta 9.40, gamma 5.48 A; zoisite alpha 5.55, beta 10.0, gamma 16.2 A;
  forsterite alpha 10.2, beta 6.0, gamma 4.76 A). Each biaxial host carries
  `axis_mapping_confidence`; while it is `unknown` the UI shows the per-axis swatches labelled "axis
  assignment unverified". The heated-tanzanite row reduces both coefficients
  and weights (double counting) and is marked `suspect`.

**Status 2026-10 (`data_version` 3): user-supplied spectroscopy integrated.** Two research
reports (`docs/chromophores/research-2026-10-polarized-spectra{,-2}.md`; validation and
resolution in `docs/chromophores/research-2026-10-validation.md`) were checked with our
colorimetry before use: 15 tabulated spectra are fixtures in
`crates/indicatrix/data/reference_spectra/`, and only 2 of them (S-Met sapphire E-perp-c,
amethyst) reproduce their own measured CIELAB (dE00 4.1, 3.4); the ruby, alexandrite, tanzanite,
peridot, emerald and citrine tables miss their CIELAB by 10-38 dE00 (the tables leave the
450-500 nm window open). Fitted from them (confidence `secondary`, still **not**
`measured`/`verified`, residual dE00 stored per band): corundum Cr3+, Fe3+, Fe2+-Ti4+, Fe2+-Fe3+,
V3+; chrysoberyl Cr3+; tanzanite V3+; peridot Fe2+; quartz [FeO4]0 and the citrine edge; beryl
Cr3+. Resolved units: V2O3 0.26 wt% is 706 ppma (the spectrum supports 305), the research
"ppma" of Fe/Ti/Mg are per formula unit, the sapphire pair coefficient is 0.05 cm^-1 per ppm of
Al sites (not 0.28 per ppma). Axis mappings: chrysoberyl `secondary`, tanzanite still `unknown`.
New data-driven model features: pair `compensator` (corundum Mg), `quadratic_coeff`,
`clustered_min` / `product` pairing per pair, color-fitted band merging before the drop step.

**Status 2026-10-04 (`data_version` 4): primary data batch 1.** No sample anywhere has both a
measured spectrum and a measured color (`docs/chromophores/research-2026-10-primary-data.md`), so
the stated CIELAB of the two reports is **no target any more**. Corundum Cr3+, V3+ and Fe2+-Ti4+
are fitted to the GIA 2020 (Dubinsky et al.) cross-sections per ion / per pair (E_perp_c and
E_par_c separately, shape and magnitude; `confidence = "measured"` for Cr3+ and V3+, `secondary`
for the pair: the peak is the average of 14 Yogo wafers, +-25 %), chrysoberyl, tanzanite, peridot,
emerald and the amethyst centre to the Caltech raw spectra as SHAPE references (no composition;
earlier magnitude anchors kept). Removed: the fitted ruby "LMCT / Urbach window" band and the 473 nm
B-line band, the chrysoberyl and emerald LMCT window bands, the tanzanite 656 nm band (details and
residuals: validation doc, "Primary data batch 1"). The ruby E_perp_c is purplish red (b* < 0:
GIA 100-375 ppma*cm: -24 to -31), the Fe2+-Ti4+ pair is 0.093 cm^-1 per ppm of Al sites (0.229 per
ppma of all atoms, E_perp_c), V3+ color change D65 vs A is 8-25 dE00. Axis mappings are stored
**by axis length** (`ray_axis_angstrom`; chrysoberyl alpha 4.43, beta 9.40, gamma 5.48 A; zoisite
alpha 5.55, beta 10.0, gamma 16.2 A; forsterite alpha 10.2, beta 6.0, gamma 4.76 A): the apparent
letter conflicts are cell settings (Pnma, Pbnm, Pmnb), not physics; chrysoberyl and tanzanite are
`verified`. Not modelled: the Si donor of the Fe2+-Ti4+ pairing (GIA: [Ti]avail = Ti - max(0, Mg -
Si); enter Mg - Si as Mg).

**Status 2026-10-12 (`data_version` 5): valence states as treatments and physical bounds.** Glass
copper and quartz iron default to one species (Cu²⁺, Fe³⁺); the copper-ruby colloid and prasiolite
Fe²⁺ are produced by the new `reducing_strike` and `thermal_anneal_550` treatments, so the exclusions
between the states hold without warning on untreated recipes. Element bounds follow the recipe's
treatments and two-element chromophores bound the sum of their elements.

Consequences:

1. Physics mode can ship with **relative** correctness (which elements give
   which hue, pleochroism, treatments) before absolute calibration. The UI
   shows **one banner per host** ("Strengths uncalibrated — approximate") when
   any item of the host is not `measured`/`verified`; per-item confidence is
   listed in the Sources popover (confidence enum, DOI list, sample id). There
   are no per-item badges. The concentration readout is labelled approximate
   while the banner shows.
2. All `suspect` values are hidden until C2 resolves them.
3. Calibration remains its own work package (C2):

1. **Sources:** Rossman's mineral spectroscopy server
   (minerals.gps.caltech.edu), RRUFF, published polarized spectra with
   stated thickness *and* microprobe/LA-ICP-MS composition of the same sample
   (Gems & Gemology, American Mineralogist, J. Gemmology). Needs network
   access to those hosts, or the spectra supplied as files.
2. **Fit:** for each sample, `A(λ) = Σ_b c_b·k_b·pol·g_b(λ)·d + baseline`;
   non-negative least squares for `k_b`, with centres and widths shared across
   samples of one host; Gaussian-in-energy shapes. Store sample id, DOI and
   residual next to each fitted band (`confidence = "measured"`).
3. **Consistency checks (automatic, C1; run on schema fields):** when a band
   has `sigma_cm2`, the loader computes α_peak = σ·N_site (N_site from the
   host's number density and the conc unit) and sets `suspect` if it differs
   from `peak_coeff` by more than 2× in either direction; when `epsilon_l_mol_cm`
   is present it must lie in the range of its `transition_class`
   (spin-allowed d–d 1–50, spin-forbidden 0.05–2, IVCT 100–2000
   L·mol⁻¹·cm⁻¹, Report p. 1–2); FWHM plausible for the class; polarization
   keys match the optical class. Failures set `suspect`. Test: the Report's
   Cr³⁺:corundum 25.9 value is flagged; 51 passes.
4. **Sanity checks:** the built-in Ruby, Sapphire, Emerald, Tanzanite,
   Peridot, Amethyst … bands (literature-cited already) must be reproduced by
   a plausible recipe within ΔE₀₀ ≤ 3 at the reference conditions — a test, not
   a change to the built-ins. Equivalence is stated explicitly: built-in
   peaks are per **model unit** at `absorption_path_scale = 1.0`, and a
   model unit is `W_mm / model_width` mm for a stone of girdle width `W_mm`
   (model_width ≈ 2 for the built-in cuts, measured by `measure_model_width`).
   The test converts each built-in peak to per mm with W_mm = 7 and the
   measured model_width of the built-in's reference cut before comparing.
   `data_version` 4: with the recipes fitted to the primary data only 3 of the 8 are reproduced
   within dE00 3 (Ruby 0.14, Aquamarine 2.52, Citrine 2.41 since `data_version` 5, 2.69 before;
   Emerald 3.06 and Sapphire 3.49 just miss, Amethyst 11.25, Tanzanite 21.31, Peridot 23.79):
   the built-in Ruby IS the GIA purplish
   red, the other three differ from the primary-data recipes (see criterion 6). Aquamarine was
   5.81 until 2026-10-12, when the beryl Mn chromophores got a sane `conc_max` and the solver's
   log grid could reach the trace Mn of the best recipe (Fe 1.56 wt% FeO + Mn 0.013 wt% MnO).

### 2.6 Treatments act on the recipe

The Report's treatment table (p. 31) maps to operators on the recipe, not to
free-standing color items:

| Host | Treatment | Effect on the recipe |
|---|---|---|
| Corundum | reducing anneal (H₂, 1200–1600 °C) | Fe³⁺ → Fe²⁺ (valence split shifts), enabling Fe²⁺–Ti⁴⁺ blue |
| Corundum | oxidising anneal (O₂, ~1600 °C), Mg/Be > Ti | Fe²⁺ → Fe³⁺; creates [h•–Fe³⁺] golden centre (from ≈ 1.5 ppma) |
| Zoisite | air anneal 550 °C | Ti³⁺ → Ti⁴⁺; removes the brown ~440 nm band of the γ ray (16.2 Å axis; data_version 3: α-axis 455/520 nm bands and a 656 nm band that the Caltech GRR 1265 spectra do not show); γ turns blue-violet |
| Quartz | γ-irradiation | [FeO₄]⁻ → [FeO₄]⁰ (amethyst 545 nm); [AlO₄]⁻ → [AlO₄]⁰ (smoky 470 nm) |
| Quartz (amethyst) | anneal ~450 °C | bleaches [FeO₄]⁰; Fe₂O₃ nanoparticles → citrine edge |
| Quartz (amethyst) | anneal ~550 °C | destroys [FeO₄]⁰; Fe³⁺ → interstitial Fe²⁺, prasiolite 720 nm band |
| Glass | reducing melt + strike (Sn²⁺, ~550-600 °C) | Cu²⁺ → Cu⁰ colloid (valence split shifts); copper ruby 570 nm plasmon |
| Topaz | e⁻/neutron irradiation + 250 °C | creates O⁻ centre, 620 nm (β-axis) blue |
| Zircon | reducing anneal 900–1000 °C | brown → blue electron-trap centre, 650 nm |

Each treatment is a data entry `{ host, id, conditions, requires, effects:
[valence_shift | create_centre | remove_centre] }`. **`remove_centre` rule:
reduce the centre's coefficient (intensity), keep its polarization weights
unchanged** (weights describe geometry, not amount); heated tanzanite is
modelled this way, which removes the current double reduction. The UI offers
only the treatments valid for the host and the present elements; the solver
may choose a treatment as one of its discrete options (§4.2).

## 3. Forward model (recipe → spectrum → color)

### 3.1 Recipe

```rust
pub struct ColorRecipe {
    pub host: HostId,                 // "corundum"
    pub data_version: u32,            // catalogue version the recipe was made with
    pub entries: Vec<RecipeEntry>,    // sorted by element id
    pub treatments: Vec<TreatmentId>, // applied in catalogue order
    pub strength: f64,                // multiplies every item's resulting α; default 1.0
    pub reference_path_mm: f32,       // thickness the preview/solver uses
    pub resolved_bands: ResolvedBands,// the budgeted tensor actually rendered (§6)
}
pub struct RecipeEntry { pub id: SelectableId, pub amount: f64 } // in the item's frozen conc_unit
```

### 3.2 Absorption

For eigenmode m ∈ {o, e, β}:

α_m(λ) = strength · Σ_chromophores Σ_bands k_b · w_{b,m} · C_eff · g_b(λ)

with C_eff = c (ion, end member), the pairing law of §2.1 (pair),
intensity (color centre), after the valence split and the treatments,
g_b a unit-peak Gaussian in energy. The host's/chromophore's UV edge enters
as in §2.2 (chromophore-attached where the data say so); no new band shape.
Coefficients convert cm⁻¹ → mm⁻¹ (÷10). Bands without `peak_coeff` are
skipped (§2.3).

### 3.3 Band budget (≤ 8 per eigenmode)

`resolve(recipe) -> Result<AbsorptionTensor, ResolveError>`:

1. Evaluate all contributing bands with their C_eff.
2. Merge bands with equal centre and width exactly (sum peaks). Weak
   edge-adjacent bands are merged at catalogue load time (§2.2), not here, so
   this step is only a safety net.
3. If still > 8 for a mode, keep the 8 with the largest CMF-weighted
   integrated absorbance; report the dropped ones and the ΔE₀₀ change
   (`ResolveWarning::BandsDropped { de }`). Deterministic order: by strength,
   ties by id. The drop step stays: a realistic recipe can reach 8 bands per
   mode, and GPU and net paths would drop differently.
4. Emit `AbsorptionTensor::isotropic/uniaxial/biaxial` according to the host.
   Physics materials are genuinely pleochroic (unlike `with_body_color`).

**WYSIWYG rule:** `resolve()` runs first; the swatch, the solver's final
scoring and `resolved_bands` are all computed from the *budgeted* tensor
(`body_colors(&resolved_tensor, …)`). The solver's linear forward model
(§4.2) works on pre-budget spectra for speed and is re-scored through
`resolve()` before a result is returned (§4.2 step 6). CI test: for every
host, "all elements at `conc_typical`" has ≤ 8 bands per mode after the
loader merge, or the host is on an explicit allow-list with a pinned
`BandsDropped` ΔE₀₀ < 1.

### 3.4 Units and stone size

Physics-mode bands are per mm. `absorption_path_scale` converts model units to
mm only when `apply_material_overrides` receives `stone_width_mm > 0` **and**
a measured `model_width`; `with_absorption_path_scale` replaces the scale, and
the material cannot hold a default itself because `model_width` is per design.
With no stone size a per-mm material would render with its path in model units
(girdle radius ≈ 1 unit ≈ a 2 mm stone: far too pale). Therefore:

- **Absorption unit rule (supersedes the earlier `physics_color` flag).** The
  material carries its own unit, `GemMaterial.absorption_unit:
  AbsorptionUnit { ModelUnit (default), PerMm }`. `PerMm` is set by
  `with_chromophore_absorption` and `with_body_color_bands` (LCh and library
  band rows); built-ins, legacy colour triples and fantasy customs are
  `ModelUnit`. The only entry is `render_setup::apply_material_overrides(
  material, overrides, model_width)`, which sets the scale through
  `absorption_path_scale_for(unit, stone_width_mm, model_width)`:
  `ModelUnit` → `(W / 7) / K` with `K = MODEL_UNIT_FACE_UP_PATH = 2.52` (measured 2026-10-08) (independent of
  `model_width`; always `Some`, `1 / K` while no width is set, so a 7 mm stone traced over
  `K` face-up units shows the colour of the 1-unit swatch), `PerMm` → `effective_width / model_width` with
  `effective_stone_width_mm(requested_mm: f32) -> f32` (single argument:
  `requested_mm` when `> 0.0`, otherwise
  `PHYSICS_DEFAULT_STONE_WIDTH_MM = 7.0`). `needs_model_width(&material)`
  tells a caller when to measure the plane arrangement (`PerMm` only), and
  `material_for_stone(material, width_mm, planes)` is the size step alone for
  scoring outside the render loop. A stored `absorption_path_scale_override`
  no longer decides the scale of a `PerMm` design (the renderer supplies it).
- **Call sites** (all places that turn a stone width into `MaterialOverrides`):
  desktop `gui/editor/view/inspector.rs:353` (design girdle) and the global
  setting path (`gui/render/material_quality.rs:410`,
  `gui/startup_settings.rs:198`), **and** the render-thread/export consumers
  `context/frame.rs:129`, `context/materials.rs:37`,
  `export_thread/scene_snapshot.rs:132`; web-core `render_material`
  (`settings.rs:335/341`) and `scene/mod.rs:427`. Decision: the review named
  only the inspector and web-core overrides; the render thread and export
  also read `ctx.stone_width_mm` independently, so the substitution is
  applied once where `MaterialOverrides` is built from `ctx` (frame.rs) and in
  the web `MaterialOverridesSpec` construction, with the inspector path
  covered by the same function. The web path is phase 2 (C9): phase 1 web
  renders the fantasy fallback and needs no substitution.
- **Degenerate geometry.** If `model_width` cannot be measured (`None`) the
  scale stays 1.0 and the path is in model units; this is a documented
  limitation shown as a UI note ("stone size unavailable"), pinned by a test.
- **Scattering.** `absorption_path_scale` multiplies the path for
  *both* absorption and inclusion scattering (`hit_t_scaled = hit_t ·
  path_scale` in `maybe_scatter_or_extinguish`, with `σ_s` in absorption-length
  units). A 7 mm default therefore also rescales `inclusion_sigma_s`
  effects; the UI note states this. Test: with σ_s > 0 the same default is
  used for both.
- **Pinned tests:** (1) `PerMm` material, `requested = 0` → `effective =
  7.0` and the resolved scale equals `7.0 / model_width` within 1e-6
  relative; (2) `ModelUnit` material (built-in, fantasy) with `requested = 0`
  → scale `1 / K`, at 7 mm `1 / K` (the 7 mm look = the 1-unit swatch), at `W` mm `W / (7 K)`; (3) `requested
  = 5.0` → 5.0 for `PerMm`, `5 / (7 K)` for `ModelUnit`.

### 3.5 Body color (new colorimetry)

`color/body_color.rs` (new):

```rust
pub enum Illuminant { D65, Planckian(f32) }   // Planckian(3200.0) = the Incandescent preset
pub struct BodyColor { pub xyz: [f64;3], pub lab: [f64;3], pub srgb: [u8;3], pub out_of_gamut: bool }
pub fn body_color(alpha_per_mm: impl Fn(f64) -> f64, path_mm: f64, ill: Illuminant) -> BodyColor
pub fn body_colors(tensor: &AbsorptionTensor, path_mm: f64, ill: Illuminant) -> BodyColors // o, e, β, unpolarised
pub fn delta_e_2000(a: [f64;3], b: [f64;3]) -> f64
```

- T(λ) = exp(−α(λ)·L), integrated 380–780 nm with the CIE 1931 2° observer
  (`color/cie1931.rs`), **at 1 nm** (the analytic band integral per CMF bin is
  an acceptable alternative), because narrow lines (ruby R1 ≈ 0.7 nm, zircon
  U⁴⁺) alias at 5 nm. Normalised so T ≡ 1 gives Y = 1.
- **Illuminants.** `D65` (from `raytracer/environment/spectral.rs`) and
  `Planckian(T)` via the existing `blackbody_spectrum(lambda_nm, temp_k)`
  (`environment/spectral.rs:84`). The A-type swatch uses the **existing
  Incandescent preset's 3200 K** (`LightingPreset::Incandescent`,
  `environment/mod.rs:83`) and its Planckian → D65 adaptation (physics.md:314)
  so swatch and render agree. Decision: the review suggested A (2856 K); the
  renderer's Incandescent preset is 3200 K, and a swatch that disagrees with
  the render under the same name would break WYSIWYG. `blackbody_spectrum`
  clamps to [0.01, 20] (inactive for 380–780 nm at 3200 K; asserted by a
  test). If `blackbody_spectrum` is not reachable from `color/`, re-export it
  from `environment` (visibility only).
- Unpolarised = mean of the eigenmode transmittances (uniaxial: (2·T_o +
  T_e)/3 for a random orientation; document the choice), plus separate o/e/β
  swatches to show pleochroism.
- sRGB via `ColorSpace::xyz_to_linear` and `color/gamut.rs::project_to_srgb`.
- Pure host (no entries) → white/colorless at Y = 1.
- **color-change readout.** `ΔE_cc = ΔE₀₀(lab_D65, lab_A)` of the unpolarised
  color; the UI shows both swatches side by side and, when ΔE_cc > 5,
  labels the recipe "color change". Alexandrite, V/Cr corundum, Cr/V
  pyrope–spessartine and Ho/Pr cubic zirconia depend on this.

This function also gives fantasy mode an honest swatch instead of hard-coded
hex values; the fantasy swatch convention is path L = 1.0 model unit (legacy
bands are per model unit), stated in the UI.

## 4. Inverse model (color → elements and fractions)

### 4.1 Problem

Given a target color (CIELAB, from the picker) under illuminant D65, host H,
reference path L, optionally entries the user has **locked**, and optionally a
second target under Planckian 3200 K (color-change gems):

minimise  cost(S) = ΔE₀₀(target_D65, color_D65(c))²
                   [ + w_A·ΔE₀₀(target_A, color_A(c))² ]   (w_A = 1, only with a second target)
                   + κ·|S|

over subsets S of selectable elements and amounts c, subject to
0 ≤ c_i ≤ c_max,i, `requires`/`excludes`, locked entries fixed. κ = 1.0 is the
item-count penalty (|S| = number of non-locked items). Garnet end members are
exempt from the item count (§5.1). Within one subset size, a tie-break
`1e-6·Σ ln(c_i/c_max,i)` applies (smaller concentrations preferred); it never
trades across subset sizes. A "closest reachable" result states its
illuminant(s) (D65, or D65 + 3200 K).

### 4.2 Algorithm (deterministic, staged)

Linearity: per-item unit spectra S_i,m(λ) (per eigenmode, 1 nm grid, 401
points, after valence split and treatment) are precomputed once per
(host, treatment set) and cached in a `OnceLock` map. Ions, end members and
color centres give α_m(λ) = strength · Σ_i c_i·S_i,m(λ); IVCT pairs
contribute strength · P(c_A, c_B)·S_pair,m(λ) with P the closed-form pairing
law of §2.1. One forward evaluation is one weighted sum over ≤ 10 bands'
worth of spectra plus 401 `exp` per eigenmode.

1. **Stage 1 — size 1.** For each valid single element (canonical order, ×
   valid treatment sets, usually ≤ 3): coarse log grid of 24 points over
   [c_min, c_max] (c_min = 1e-3·c_max), best point, then ≤ 20 golden-section
   iterations in log c. Cost includes κ·1. **If the best ΔE₀₀ ≤ 1.0, stop.**
2. **Stage 2 — size 2** (only if stage 1 ended with ΔE₀₀ > 1.0): all valid
   pairs (respecting requires/excludes/locks). One start per dimension on an
   8×8 coarse log grid (best cell), then projected Levenberg–Marquardt in
   log c with finite-difference Jacobian, iteration cap 20. Stop if the best
   total cost ≤ 1.0 + 2κ... precisely: stop if best ΔE₀₀ ≤ 1.0.
3. **Stage 3 — size 3** (only if still ΔE₀₀ > 1.0): all valid triples, 6×6×6
   coarse log grid, LM cap 20.
4. Candidates are compared by cost; ties (|Δcost| < 1e-9) → fewer items →
   lexicographic ids. Treatments are discrete; the solver never adds one the
   user has excluded ("natural only" switch).
5. A fixed **evaluation cap** (`MAX_EVALS = 40_000`, counted per solve) bounds
   the work; stages beyond the cap are skipped in canonical order and the
   result is flagged `capped`. Estimated worst-case work is ≈ 19 000
   evaluations for an 8-element host (350 + 28·≈100 + 56·≈276), ≈ 8 µs each
   → ≈ 150 ms; target **≤ 250 ms** on a laptop-class core for the largest
   host, run off the UI thread; cancellation (latest request wins) discards,
   it never truncates.
6. **Re-scoring (WYSIWYG).** The winning candidate is passed through
   `resolve()`; its ΔE₀₀ is recomputed from the budgeted tensor. If the drop
   step changed ΔE₀₀ by > 0.5, up to 5 further LM iterations are run with the
   budgeted tensor as forward model; the reported `achieved` is always the
   budgeted color.
7. Report `SolveResult { recipe, achieved: BodyColor, delta_e, reachable:
   bool, capped: bool }`; **`reachable = delta_e ≤ 1.0`** (same as the stage
   accept threshold). Decision: the earlier "reachable = ΔE ≤ 2" conflicted
   with the round-trip criterion ΔE ≤ 1; one threshold is used. Between 1.0
   and 2.0 the UI shows "ΔE x.x from your pick" without a warning color;
   above 2.0 the warning color.

**Determinism.** Fixed grids, fixed start points, fixed iteration and
evaluation caps, `total_cmp`, no wall-clock in the decision path. Bit-identical
results are asserted on one platform/toolchain only (finite-difference
Jacobians and `exp` differ across libm); cross-platform tests compare with a
tolerance of ΔE₀₀ < 0.05 and equal selected subset.

### 4.3 Reachability

Absorption only removes light: a color lighter or more saturated than the
host can transmit at path L is unreachable. The solver then returns the
closest reachable color and the UI shows both swatches with ΔE and the
illuminant. Optional: precompute a coarse reachable gamut per host and L to
grey out impossible picker regions.

### 4.4 Two-way sync, history

- **Source of truth in physics mode = the recipe.** The color is always
  derived (`resolve()` → `body_colors`).
- Editing an element or fraction → forward model → swatch updates (cheap,
  synchronous).
- Picking a color → solver → recipe replaced (except locked entries) →
  swatch shows the *achieved* color, with "ΔE x.x from your pick" if not
  exact. No feedback loop: the picker value is an input event, never written
  back from the forward model.
- **Recipe history stack** in the dialog state (the material editor is a
  modal dialog with only a dirty baseline; cut-core `edit::History` covers
  designs, not dialogs): a `Vec<ColorRecipe>` (cap 50) pushed before every
  solver write and before every slider commit (release, not drag); Ctrl-Z pops
  and restores exactly, Ctrl-Y re-applies. Covered by an acceptance test (§11).

### 4.5 Fractions and strength

The user controls a recipe in two equivalent views, toggled in the UI:

- **Strength + fractions:** fractions f_i summing to 100 % set each item's
  amount = f_i·c_ref,i, where c_ref,i is the item's typical
  concentration (color centres: a nominal intensity 1.0); the "strength"
  slider sets `recipe.strength` s, **a multiplier on each item's resulting α**
  (§3.2). C_eff ∝ s for ions and pairs alike, so the hue does not change when
  the slider moves (amount-scaling would make IVCT go as s² and ions as s).
  Decision: adopted as proposed; the per-item "amount" readout is the
  effective amount (s·amount for ions; the pair density for IVCT, labelled).
- **Absolute:** amounts in the item's unit (ppm or wt% oxide) with sliders
  bounded by `conc_max`; s = 1.

The solver writes amounts with s = 1; both views are derived from them.

### 4.6 Fantasy mode uses the same solver

Fantasy mode's free picker solves for the three legacy band peaks
(`legacy_rgb_bands`) with the same solver machinery, an unconstrained "host"
whose items are the three legacy bands at L = 1.0 model unit. The result is
stored as today's `absorption_rgb`, so fantasy materials stay in the existing
format. Presets keep their exact triples (bit-identical). Three fixed
Gaussians reach a thin set of colors; the UI says so ("closest reachable
with three bands"). Option (non-preset path only): the fantasy picker may emit
up to 8 free bands (a `fantasy_bands` payload, legacy-compatible only for
presets); not in the first release unless the solver shows ΔE₀₀ > 5 for
common targets.

## 5. Data model in the apps

```rust
// crates/indicatrix-cut-core/src/material/color.rs (new)
pub enum ActiveColor { Fantasy, Physics }
pub struct ColorMode {
    pub active: ActiveColor,
    pub fantasy_rgb: [f32;3],
    pub last_recipe: Option<ColorRecipe>,
}
```

- `GemMaterial::new_custom` stays; add
  `GemMaterial::with_chromophore_absorption(tensor)` (or build via
  `resolve`). Physics mode fixes `crystal_system`/`optical_character` from the
  host; RI, dispersion and SG keep their fields but are prefilled from the
  host's built-in material.
- **Mode switching keeps both payloads.** Switching changes `active` only;
  `fantasy_rgb` and `last_recipe` are both persisted. First switch
  fantasy → physics with no `last_recipe`: solve the fantasy color into the
  chosen host (closest reachable). First switch physics → fantasy with default
  `fantasy_rgb`: seed `fantasy_rgb` from the achieved sRGB via the legacy
  solver; later switches never overwrite the inactive payload.
- Per-design body color override (`MaterialSelection.body_color_override`)
  stays fantasy-only in the first release. Because `with_body_color` replaces
  the whole tensor, the override control is **greyed out for physics
  materials** with the reason "replaces the physical color", and a warning
  is shown if an override is already set when a physics material is selected.
  A physics override is a follow-up (it needs the host of the design's
  material).

### 5.1 Garnet end members

Garnet items are mole fractions with **Σ x = 1** and an explicit colorless
remainder end member (pyrope/grossular; chosen per host rule). End members are
exempt from the item-count penalty in §4.1 (otherwise x_Alm is penalised). RI
and SG are interpolated linearly over the end-member fractions (pyrope 1.714,
almandine 1.83, …) when the host is garnet; prefill rather than lock.

## 6. Persistence and compatibility

| Store | Change | Version |
|---|---|---|
| Vault `custom_gem_materials` | new nullable `color_recipe_json TEXT` (additive migration, `column_exists` guard) holding both payloads, the recipe, `resolved_bands` and `data_version` | none (additive) |
| Native `CustomMaterialSnapshot` | `color_recipe: Option<ColorRecipeDto>` (`serde(default, skip_serializing_if)`) **and** keep writing `absorption_rgb` = nearest legacy color, so older builds show an approximate color. The DTO also stores the fallback rgb it wrote; on load, if the top-level `absorption_rgb` differs from it, an older build edited the file: treat the material as fantasy-edited and ask "keep physics recipe / use edited color" | `FORMAT_VERSION`/`DESIGN_VERSION` unchanged |
| Resolved bands | `resolved_bands` (the budgeted tensor actually rendered) and `data_version` are stored in **both** the vault row and the native DTO. Rendering **always** uses `resolved_bands`, never a silent re-resolve (vault rows are shared across designs: a recalibration must not recolor them). If `data_version` ≠ the current catalogue's, a non-modal "data updated" badge offers "update to current data"; never silent | |
| Unit safety | a `data_version` change that alters a unit basis is impossible by construction (`conc_unit` frozen per id, §2.2); a unit change is a new id and old recipes keep rendering from `resolved_bands` | |
| Net protocol | unchanged: `SceneState.material` carries the resolved `GemMaterial` | v19 stays |
| Web-core `CustomMaterialSpec` | phase 1: web uses the `absorption_rgb` fallback (fantasy approximation). Phase 2 (C9): add the recipe, bump web `PROTOCOL_VERSION` 8 → 9 | |

Restore paths: `gem_material_from_row` (desktop), `gem_material_from_custom_snapshot`
(cut-core native), `custom_material_snapshot_for_save` (save helpers) must all
build from `resolved_bands` when a recipe is present.

## 7. Desktop UI

Material editor dialog (`material_editor_dialog.slint`), color section:

```
color   (•) Fantasy   ( ) Physics
```

**Fantasy:** today's 9 presets + a "Pick…" button opening a color picker
(hue/saturation/value square + hex field). Live swatch from `body_color`
(path 1.0 model unit); text "closest reachable with three bands".

**Physics:**

```
Host crystal  [Corundum ▾]   (locks crystal system / optical character)
              ⚠ Strengths uncalibrated — approximate           (one banner per host)
Elements      [+ Add ▾]  (only valid items for the host; impossible ones greyed with reason)
  Cr   ▮▮▮▮▮▯▯  0.20 wt% Cr₂O₃   [lock]  ✕     Sources ⓘ
  Fe   ▮▮▯▯▯▯▯  0.05 wt% FeO     [lock]  ✕
  Ti   ▮▯▯▯▯▯▯  80 ppm           [lock]  ✕
View          (•) Strength + fractions  ( ) Absolute
Strength      ▮▮▮▮▯▯▯
Fractions     Cr 70 %  Fe 20 %  Ti 10 %
Treatments    [ ] Heated  [ ] Irradiated (only those valid for the host)
Equivalent path [5.0] mm   (default 1.5 × girdle diameter when the design has one)
color        D65: [■ unpolarised] [■ o-ray] [■ e-ray]
              A 3200 K: [■ unpolarised]   color change ΔE 12.4
              At this stone's size: [■ D65] [■ A]
              [Pick color…]   ΔE 0.8 from your pick (D65) · closest reachable shown
```

- New component `ui/components/material_editor/chromophore_picker.slint`;
  the template selector preselects the host.
- The solver runs on a worker thread; the dialog shows "solving…" and
  cancels on new input.
- `save_material` gains the mode and a serialized `ColorMode` (a JSON string
  property, to keep the Slint callback signature manageable).
- **Reference path.** `reference_path_mm` defaults to 1.5 × the open design's
  `girdle_diameter_mm` (labelled "equivalent path"; a rough proxy for the
  table-to-culet path plus return) and to 5.0 mm without a design. A second
  swatch row "at this stone's size" uses the same value when it differs from
  the user-set reference path.
- The "Sources" popover per item lists the confidence enum, DOIs, sample ids
  and skipped bands (§2.3); no per-item badges (§2.4).
- Manual: rewrite the "Gem Body Color & Transmission" row in chapter 6; add a
  "Physics color mode" section and an appendix listing hosts and elements.

## 8. Determinism and bit-identity

- Catalogue, recipe entries, subsets and bands in canonical sorted order;
  `BTreeMap` only; `total_cmp`; fixed solver grids, iteration and evaluation
  caps; no wall-clock budget that changes results (cancellation discards,
  never truncates).
- "Same input → same bits" is asserted on one platform/toolchain; elsewhere a
  tolerance applies (§4.2).
- Band emission order is part of the output (f32 summation order). Sort by
  (centre, width, id).
- Existing materials, presets, built-ins and saved designs without a recipe
  render bit-identically: no change to `GemMaterial`, `AbsorptionTensor`,
  GPU layout or goldens. `render_setup_pins` and the metrics fingerprint stay
  unchanged for them; the `ModelUnit` size rule (`(W / 7) / K`, `1 / K` while unset; 7 mm = the 1-unit swatch) gives them scale `1 / K` when no size is set, so their pins are re-pinned.

## 9. Fluorescence (later stage)

Data already captured per chromophore (`[[host.chromophore.fluorescence]]`:
excitation, emission, quantum yield, LW/SW UV response, `quenched_by`).
Beer–Lambert in the current transport is deterministic attenuation: no
absorption *event* exists, so fluorescence needs an event model that is
consistent with `maybe_scatter_or_extinguish`
(`scattering/hg.rs:94`), which already draws one exponential free path from
`σ_t,hero = α_hero + σ_s` (absorption-length units) and weights the other
channels analytically.

- **Quenching.** Cr³⁺ R-line emission (Φ ≈ 0.9 isolated in ruby and spinel)
  falls to zero for FeO above 0.5–1.0 wt%; Fe²⁺-rich hosts (almandine,
  fayalitic olivine) are inert; metamict zircon quenches REE emission.
  Φ_eff,i = Φ₀,i · q(c_quencher) with a per-pair quenching curve in the data.
- **F1 — spectral range** below 380 nm for illuminants and absorption (UV
  lamps: 365 nm LW, 254 nm SW); UV illuminant presets. Visible excitation
  needs no range extension: **ruby's visible-excited red emission (Cr³⁺ bands
  at ≈ 400 and ≈ 550 nm → R-lines 692/694 nm) is the gemmologically relevant
  case**, and F2 handles it first.
- **F2 — extinction-event model (CPU only).**
  1. *Per-chromophore σ_a.* The resolved absorption α_hero(λ) is split into
     per-chromophore parts α_i(λ) = n_i·σ_a,i(λ) (from the band data), with
     α_nonfl = α_hero − Σ_fl α_i for chromophores without emission. Only
     fluorescent items (Φ_eff,i > 0) are tracked.
  2. *Event sampling.* Sample t_free from σ_t = α_hero + σ_s exactly as the
     existing estimator does. At an event (t_free < hit_t_scaled) choose
     scatter with probability σ_s/σ_t, otherwise absorption; the absorbing
     chromophore is chosen with probability α_i/α_hero (the
     chromophore-attributed fraction of α). The existing analytic weights
     (`tr·σ_s/pdf_hero`) are kept for the scatter branch; the absorption
     branch terminates the primary path with weight `tr_k·α_hero/pdf_hero`.
  3. *Re-emission.* With probability Φ_eff,i the absorbed energy is
     re-emitted: wavelength λ' drawn from the emission spectrum of i, direction
     isotropic, position at the event. Energy conservation: the re-emitted
     path carries energy · (λ_abs/λ') (Stokes factor ≤ 1 for downshift) ·
     (1/P_emit) so that, in expectation, emitted energy = Φ_eff · (λ_abs/λ') ·
     absorbed energy ≤ absorbed energy. Test: furnace with Φ = 1, λ' = λ_abs
     returns total energy within 1 %.
  4. *Hero comb.* The 8-channel comb is a wrapped 50 nm lattice anchored on
     the hero (`wrapped_hero_wavelengths`). A wavelength change at re-emission
     starts a new comb anchored on λ' (hero = λ'; the other seven channels
     are the wrapped lattice around it); channel weights are the emission
     spectrum e(λ_k)/e(λ') ratios, so non-hero channels remain consistent
     estimates. Fluoresced paths are therefore not "one hero wavelength per
     path"; the transport carries a `fluoresced` flag to apply this branch.
  5. *MIS/NEE.* Next-event estimation at the emission point uses the new
     comb; the HG/MIS carry (`nee_contribution_hg_scatter`) is reset at the
     event.
  6. **The GPU port and WGSL are explicitly out of scope** for F2 (CPU
     reference only; GPU materials are never given fluorescence data, so v19
     and the goldens stay untouched).
- **F3 — "UV lamp" lighting mode** in the UI (CPU renders only).

## 10. Work packages

| WP | Content | Depends on | Network |
|---|---|---|---|
| C1 | Normalise research files → `crates/indicatrix/data/chromophores.toml`; schema incl. `sigma_cm2`/`epsilon_l_mol_cm`/`transition_class`, `calibration`, `axis_mapping_confidence`, per-pair `pairing`; explicit concentration bases and host number densities; frozen units; ZPL → envelope conversion; loader-time edge merge; treatment entries (§2.6); automatic consistency checks setting `suspect` (§2.4; needs the schema fields in the same WP); validator tests | — | no |
| C2 | Calibration of absorption strengths from measured spectra (§2.4); resolve every `suspect`; re-derive pair coefficients; set `measured`/`verified`; sanity tests vs built-ins | C1 | **yes** (or user-provided spectra) |
| C4 | `color/body_color.rs`: spectrum → XYZ/Lab/sRGB at 1 nm, D65 and Planckian illuminants, ΔE₀₀ (CIEDE2000 reference test vectors, Sharma 2005) | — | no |
| C3 | `optics/chromophore/{catalogue,recipe,resolve}.rs`: forward model, band budget, units, `effective_stone_width_mm`; tests: single-band analytic Beer–Lambert, pair scaling, pleochroism weights, ≤ 8 bands, determinism, pinned snapshot, blue-sapphire color test (use C4) | C1, C4 | no |
| C5 | Inverse solver (§4); tests as §11 | C3, C4 | no |
| C6 | cut-core `ColorMode`/`ColorRecipe`, editor form parsing and validation, mode switching with both payloads, recipe history stack, legacy-basis solver for fantasy picker | C3–C5 | no |
| C7 | Persistence: vault migration, native snapshot DTO with `resolved_bands`, `data_version`, fallback rgb; restore paths, round-trip tests, old-file compatibility, old-build-edit detection | C6 | no |
| C8 | Desktop UI (§7), manual chapter 6 + appendix, physics.md §3; app-side `effective_stone_width_mm` wiring (§3.4) | C6, C7 | no |
| C9 | Web app physics mode, web protocol 8 → 9, web-side default width | C8 | no |
| F1–F3 | Fluorescence (§9) | C3 | no |

C1 and C4 can run in parallel; C3 starts after both. C2 can run any time
network access exists and only changes data (bumping `data_version`). Until
C2 is done, physics mode ships with the per-host "uncalibrated" banner.

## 11. Acceptance criteria

All thresholds use ΔE₀₀ under D65 at the stated path unless noted. "Blue
recipe" = corundum Fe 1000 ppm + Ti 100 ppm (units per the catalogue, fixed in
C1) at the 5.0 mm reference path.

1. **Forward colors.** Corundum + Cr at 0.05 → 0.5 wt% Cr₂O₃: hue angle of
   the unpolarised color h_ab in [0°, 30°] ∪ [330°, 360°) (GIA Appendix 2 "both", D65,
   100-3000 ppma·cm: 339-26°) and chroma C*
   increases monotonically over 5 steps (GIA: rises monotonically over 67-670 ppma·cm and
   saturates near 70). Blue recipe: ΔE from colorless
   (Y = 1, a* = b* = 0) **> 20**, hue h_ab within [GIA − 50°, GIA + 20°] of the unpolarised color of the GIA
   Fe²⁺–Ti⁴⁺ cross-section at the recipe's 100 pairs over 5 mm (291°; the recipe's Fe³⁺ shifts it to 266°) and in
   [220°, 310°] (data_version 3: [220°, 270°], from the stated color of the research's S-Met
   sapphire, hue 261°, which GIA's violet-blue Fe–Ti color does not support). Ti alone (any
   amount ≤ c_max): ΔE from colorless **< 1**. Blue recipe o/e pleochroism:
   ΔE(o, e) **> 3**.
2. **Pairing regression.** Blue recipe with `pairing = random` would give
   α_peak < 0.1 cm⁻¹ (documents the failure); with the catalogue's
   `clustered` law α_peak ≥ 2 cm⁻¹ within 5 % of the GIA value: σ = 1.98·10⁻¹⁸ cm²
   at 580 nm × 100 pairs (ppm of Al sites) = 9.3 cm⁻¹, 0.093 cm⁻¹ per pair-ppm
   (data_version 3: "empirical 5 cm⁻¹", within 2.5×). Pure host: Y ≥ 0.98 at 10 mm.
3. **Solver round trip.** For 200 reproducibly seeded recipes of ≤ 3 items
   (amounts ≤ 0.5·c_max, corundum, beryl, garnet, tanzanite), solve their
   forward color: ΔE₀₀ ≤ 1.0 for ≥ 95 % and ≤ 2.0 for 100 %; `reachable`
   true for every case with ΔE ≤ 1.0. Picking a ruby red (L* 40, a* 55,
   b* 30 ± 5) in corundum yields a Cr-dominated recipe (Cr share of α ≥ 70 %; a
   b* > 0 ruby is unreachable from the GIA Cr³⁺ cross-section, ΔE₀₀ 12–16), and so do the GIA Appendix 2
   ruby colors (unpolarised, D65, 100–375 ppma·cm: b* −10 … −19), reached to ΔE₀₀ ≤ 3;
   picking an unreachable green returns `delta_e` reported and
   `reachable = false` when ΔE > 1.0. Locks honoured: locked amounts unchanged
   to 1e-12. color-change target pair (D65 + A) for alexandrite: ΔE_D65 ≤ 3
   and ΔE_A ≤ 3.
4. **Solver cost and budget.** Largest host: ≤ 250 ms wall (release build,
   reference laptop) and ≤ 40 000 evaluations; stage 1 exits for a size-1
   target with ΔE ≤ 1 after ≤ 400 evaluations. Item-count penalty: a
   pure-Cr target never selects 3 items (|S| = 1). Determinism: identical
   input → identical bits on one platform; ΔE₀₀ < 0.05 and equal subset
   across platforms.
5. **WYSIWYG.** For 50 random recipes per host, ΔE₀₀(swatch from
   `body_colors(resolved)`, swatch recomputed from the stored
   `resolved_bands`) = 0 (bitwise) and ΔE₀₀(solver-reported `achieved`,
   swatch) < 0.01; ≤ 8 bands per mode after the loader merge for
   `conc_typical` recipes.
6. **Stone-size default.** `effective_stone_width_mm(0.0) = 7.0`,
   `(5.0) = 5.0` (a `ModelUnit` material instead gets `(W / 7) / K`, and `1 / K` while
   unset; 7 mm equals the 1-unit swatch); the resolved scale for a physics
   material with no girdle size equals 7.0/model_width within 1e-6 relative;
   the built-in sanity test (§2.4) passes ΔE₀₀ ≤ 3 for ≥ 6 of the 8 listed
   built-ins. **Status (2026-10-19, after the measured color refits): met.** 6 of 8 are
   within ΔE₀₀ 3: Ruby 1.00, Sapphire 0.22, Emerald 1.01, Peridot 0.15, Aquamarine 0.75
   and Citrine 2.41. Tanzanite (7.27) and Amethyst (11.25) still differ from the primary-data
   recipes. The ≥ 6/8 test is no longer ignored, and
   `criterion_6_built_in_colors_vs_primary_data_recipes_are_pinned` pins the 8 values.
7. **Undo.** Pick → edit an amount → undo: recipe equals the solved recipe
   exactly (field-wise equality). 50 consecutive undo steps work; the 51st
   push drops the oldest.
8. **Mode switching.** fantasy → physics → fantasy: `fantasy_rgb` unchanged
   bitwise; physics → fantasy → physics: `last_recipe` equal to the original.
9. **Persistence.** Vault and native round-trip of a recipe: `resolved_bands`
   bit-identical and rendered color ΔE₀₀ = 0. After a catalogue
   `data_version` bump the stored material still renders from
   `resolved_bands` with ΔE₀₀ = 0 and shows the badge. A native file whose
   top-level `absorption_rgb` differs from the DTO's fallback rgb triggers the
   prompt (one test). An older build opens a physics material with its
   approximate fantasy color: ΔE₀₀ ≤ 15 against the physics color.
10. **Pinned snapshot.** Corundum + Cr₂O₃ 0.20 wt%, path 5.0 mm, catalogue
    `data_version = N` (value recorded by C3; currently 5): the resolved band list
    (centres, widths, peaks per mode) equals a pinned fixture to 1e-6
    relative; changing a catalogue number requires bumping `data_version` and
    the fixture together.
11. **Offer rule.** Corundum offers Cr, Fe, Ti, V; chrysoberyl offers Cr;
    diamond offers its visible N centres; no band outside 250–1200 nm exists
    in `[[band]]`.
12. **Consistency checks.** The Report's Cr³⁺:corundum 25.9 cm⁻¹/wt% is flagged
    `suspect`; 51 passes; the GR1 and [h•–Fe³⁺] rows are flagged.
13. **Narrow lines.** Ruby R1 (0.7 nm) swatch at 1 nm differs from a 0.1 nm
    reference integral by ΔE₀₀ < 0.3 (5 nm would exceed 1).
14. **color change.** Alexandrite-like V/Cr corundum recipe (V 2000 ppm of Al
    sites, Cr 0.02 wt%): ΔE_cc (D65 vs 3200 K) > 8 (GIA V³⁺ cross-section: 8–25, data_version 3:
    Cr 0.1 + V 2000 ppm, now a ruby-like 5.65); a Cr-only ruby recipe: ΔE_cc < 7 and within 0.5 of the GIA
    cross-section's color change (2.4 / 2.8 / 5.0 / 5.9 at 0.05 / 0.1 / 0.3 / 0.5 wt%, 5 mm; data_version
    3: < 5).
15. **Bit-identity.** Fantasy presets and every existing material render
    bit-identically; all goldens, `render_setup_pins` and the white furnace
    are unchanged (0 pixel/ULP differences); net protocol stays v19.
16. **Fluorescence (when F2 lands).** Furnace with Φ = 1, λ' = λ_abs:
    total energy within 1 % of the non-fluorescent case; Φ = 0 reproduces the
    current output bit-identically; GPU path unchanged.
17. **Primary data (data_version 4, `primary_data_tests`).** Our color code applied to the GIA sample-1110
    cross-section reproduces GIA Appendix 2 (E⊥c, E∥c, D65 and A, 100–375 ppma·cm) to ΔE₀₀ ≤ 2
    (the "both" column ≤ 3, GIA's mix is unstated); the Cr recipe reproduces the same colors to ΔE₀₀ ≤ 3
    (measured ≤ 1.4; "both" ≤ 4) and the cross-section itself to ≤ 3 up to 3000 ppma·cm; ruby E⊥c has b* < 0
    at 50–750 ppma·cm with GIA's hue (±4°); the V³⁺ color change at 340 ppma·cm E∥c is ≥ 8 (24.8) and the
    V³⁺ σ(580 nm) is the xlsx 3.4·10⁻²⁰ cm² (not the text 1.0·10⁻¹⁹); the Fe²⁺–Ti⁴⁺ pairs are linear in Ti − Mg with Fe
    in excess (Mg ≥ Ti zeroes them) and the bands are σ = 1.98·10⁻¹⁸ cm² per pair; alexandrite (alpha purple-violet
    under A, beta yellow-orange, gamma blue-green) and tanzanite (alpha red-violet, beta blue, gamma yellow-green) rays
    land on the right axis lengths; the shapes follow the Caltech spectra (residual shares in the validation doc).

## Review log

| # | Resolution | § |
|---|---|---|
| 1 | Offer rule is per band; IR/ZPL entries moved to `calibration`; Corundum/chrysoberyl/diamond offer test | 2.1, 2.3, 11 (11) |
| 2 | `effective_stone_width_mm` (7.0 mm) in `render_setup/materials.rs`, call sites named, σ_s scaling stated, equivalence for built-in checks stated, pinned tests. Decision: also applied at render-thread/export consumers, not only the inspector | 3.4, 2.4, 11 (6) |
| 3 | Per-pair `pairing` in data (Fe²⁺–Ti⁴⁺ clustered, Fe²⁺–Fe³⁺ provisional clustered); empirical per-ppm² kept secondary; blue recipe ΔE > 20 test | 2.1, 11 (1, 2) |
| 4 | `body_color(…, Illuminant)`, D65 + Incandescent swatches, ΔE_cc, optional second solver target, illuminant stated. Decision: 3200 K (existing preset, `blackbody_spectrum`) instead of 2856 K | 3.5, 4.1, 7, 11 (14) |
| 5 | Linear precomputed spectra, staged search, coarse log grids, cap 20, ≤ 250 ms, fixed-grid determinism, evaluation cap | 4.2, 11 (4) |
| 6 | κ·k item-count penalty plus "accept ΔE ≤ 1" staging; concentration term only as tie-break in one size | 4.1, 4.2 |
| 7 | `resolve()` first; swatch/solver/`resolved_bands` use the budgeted tensor; solver re-scores through `resolve()`; loader merge keeps ≤ 8 | 3.3, 4.2 (6), 11 (5) |
| 8 | Explicit recipe history stack in dialog state with acceptance test | 4.4, 11 (7) |
| 9 | `ColorMode` holds `fantasy_rgb` and `last_recipe`; both persisted | 5, 6, 11 (8) |
| 10 | `resolved_bands` + `data_version` in vault and native; render from stored bands; "data updated" badge; `conc_unit` frozen per id | 2.2, 6, 11 (9) |
| 11 | Strength is a multiplier on each item's α (`recipe.strength`) | 3.1, 3.2, 4.5 |
| 12 | Extinction-event model, per-chromophore σ_a, visible excitation (ruby), comb handling, energy conservation, GPU out of scope | 9, 11 (16) |
| 13 | `sigma_cm2`, `epsilon_l_mol_cm`, `transition_class` fields; loader α = σ·N_site check (2×) | 2.1, 2.4, 11 (12) |
| 14 | Effective verified count 0; one banner per host, per-item confidence in Sources popover; measured/verified only via C2; spot-check results recorded | 2.4, 7 |
| 15 | `axis_mapping_confidence`; heated tanzanite `suspect`; `remove_centre` rule (reduce coefficient, keep weights) | 2.1, 2.4, 2.6 |
| 16 | Garnet Σx = 1 with colorless remainder, end members exempt from item count, linear RI/SG interpolation | 2.1, 4.1, 5.1 |
| 17 | Fantasy convention L = 1.0 model unit stated; "three bands" UI note; optional 8-band free picker (deferred) | 3.5, 4.6, 7 |
| 18 | Per-design override greyed out for physics materials, warning if already set | 5 |
| 19 | Default `reference_path_mm` = 1.5 × girdle ("equivalent path"); "at this stone's size" swatch | 7 |
| 20 | 1 nm integration (or analytic per CMF bin); narrow-line test | 3.5, 11 (13) |
| 21 | Chromophore-attached edges; host edge only if intrinsic; pure host Y ≥ 0.98 test | 2.2, 11 (2) |
| 22 | Fallback rgb stored in the DTO; mismatch → treat as fantasy-edited and ask | 6, 11 (9) |
| 23 | Bit-identity scoped to one platform/toolchain; tolerance elsewhere | 4.2, 8, 11 (4) |
| 24 | WP order C4 before C3, dependencies fixed, C1 owns the finding-13 fields; numeric criteria; reachable = ΔE ≤ 1 (one threshold); pinned corundum + Cr snapshot | 10, 11 (3, 10), 4.2 (7) |
| 25 | Weak edge-adjacent bands merged at load time; drop step kept; step-2 merge demoted to safety net | 2.2, 3.3 |
