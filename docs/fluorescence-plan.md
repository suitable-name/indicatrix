# Fluorescence and UV lamps — design

Status: design (2026-10). Extends `chromophore-color-plan.md` §9 (F1–F3),
which it supersedes. CPU only; the GPU path is out of scope.

## Goal

A physics-mode material glows the way its elements make it glow. Ruby (Cr³⁺)
shows its red R-line emission under a 365 nm or 395 nm UV lamp, and weakly in
daylight. Iron quenches it. Diamond N3 glows blue, and so on. The glow is
derived from the material's color recipe: the same elements and
concentrations that give the body color also give the fluorescence.

## Constraints

- Existing materials, lighting presets, goldens, `render_setup_pins`, the white
  furnace and `GemMaterial` stay bit-identical. Fluorescence data travels
  **beside** the material (the same pattern as concave `tools` beside the
  scene), never inside `GemMaterial`.
- A scene with fluorescence or a UV lamp renders on the CPU:
  `scene_routes_to_gpu` returns false, as it does for tools.
- The net protocol bumps 19 → 20 once, for both new lighting presets and
  `SceneState.fluorescence`. Older peers are refused cleanly.

## 1. Lighting presets

Append two variants **at the end** of `LightingPreset` (keep existing
discriminants and postcard indices): `UvLamp365` ("UV lamp 365 nm") and
`UvLamp395` ("UV lamp 395 nm").

- Spectra (`IlluminantSpectrum`): a Gaussian at 365 nm with FWHM 10 nm, and a
  Gaussian at 395 nm with FWHM 12 nm. The 395 nm LED's tail reaches 380–420 nm,
  so it lights the stone faintly violet. That is realistic and is kept.
- Rig: the analytic studio geometry of `Daylight`, with the lamp spectrum
  replacing D65 and no ambient fill.
- White balance: none (identity). Make the rule explicit
  (`uses_white_balance()` or similar) instead of overloading `uses_d65()`, and
  update `illuminant_white_balance_matches_direct_computation_for_all_presets`
  so every *existing* preset keeps its value.
- `studio_presets_are_unchanged_by_the_split`: existing baseline rows are
  unchanged; add rows for the two new presets, recorded from the
  implementation.
- UI lists (desktop `viewport.slint` lighting options, web
  `lighting_options()`) gain the two entries. Saved-scene label parsing
  accepts them, and older builds fall back to Daylight as today.

## 2. Spectral range below 380 nm

Visible camera wavelengths stay 380–780 nm. Fluorescence paths switch to an
**excitation wavelength** λ_ex ∈ [300, λ_em). Everything evaluated on such a
path must be valid down to 300 nm:

- **Illuminants:** extend the D65 table to 300 nm with the CIE values; values
  at ≥ 380 nm stay bit-identical. Blackbody is analytic. UV lamps are as above.
- **Dispersion:** evaluate the existing models at λ_ex. Clamp to each model's
  stated validity minimum and document the clamp.
- **Absorption:** `AbsorptionBand::evaluate` is analytic, so it already works.

## 3. Emitters (data → renderer)

```rust
pub struct FluorescentEmitter {
    pub excitation: Vec<AbsorptionBand>,  // the chromophore's own absorption bands (α_c(λ)), per mm
    pub emission: Vec<EmissionBand>,      // centre_nm, fwhm_nm, weight; normalised to ∫ f(λ) dλ = 1
    pub quantum_yield: f32,               // Φ_eff after quenching, 0..1
}
pub struct Fluorescence { pub emitters: Vec<FluorescentEmitter> } // empty = none
```

- Built from the color recipe by
  `chromophore::resolve_fluorescence(catalogue, recipe) -> Fluorescence`, next
  to `resolve` (absorption).
- Excitation reuses the chromophore's resolved absorption bands, scaled by
  its C_eff, so "what absorbs is what glows". The UV edge of the host competes
  for UV photons but does not emit.
- **Quenching** (data): `quench = [{ by = "Fe", unit = "...", c_half = …, n = … }]`
  gives Φ_eff = Φ₀ · Π 1/(1 + (c_q/c_half)ⁿ). Ruby: Φ₀ ≈ 0.9; Fe quenching
  complete by 0.5–1.0 wt% FeO (Report p. 31), so pick c_half ≈ 0.2 wt% FeO,
  n = 2, and document it as an estimate. (Primary-data batch 1, 2026-10-04: the only
  figure found for a ruby yield, QE 0.83 +- 0.10, is a coating-level value including escape losses and
  is not an intrinsic Phi_0; no primary Fe-halving concentration exists for ruby; the emerald value
  c_half = 0.0849 wt% FeO is derived from a GIA 2017 abstract, see the table.)
- **Emission:** narrow lines (Cr³⁺ R1/R2 692.9/694.3 nm) plus their vibronic
  sideband (~700–720 nm) where the data have it. Lines narrower than 1 nm are
  widened to 1 nm for sampling and documented.
- Chromophores with `uv = inert`, no emission, or Φ = 0 yield no emitter.
- The catalogue data gets the fields it lacks: `quantum_yield`,
  `emission_fwhm_nm` per line, and `quench`. Values come from the research
  files and the Report; confidence follows the color plan's rules.

## 4. Transport (backward, single wavelength)

The path tracer runs from the camera at visible λ_em. Fluorescence adds an
inelastic in-medium event:

1. **Rate.** At λ_em, the fluorescence pseudo-extinction is
   μ_f(λ_em) = Σ_c Φ_c · f_c(λ_em) · A_c, with A_c = ∫_{300}^{λ_em} α_c(λ') dλ'.
2. **Event.** Inside the stone, sample a fluorescence vertex at distance t from
   exp(μ_f), competing with the existing scattering and boundary distance
   sampling (the same structure as `maybe_scatter_or_extinguish` in
   `raytracer/scattering/hg.rs`).
3. **At the vertex:**
   - choose c ∝ Φ_c f_c(λ_em) A_c;
   - sample λ_ex from α_c(λ') on [300, λ_em) (tabulated CDF, 1 nm);
   - draw a new direction isotropically;
   - multiply the path weight by λ_ex/λ_em (photon energy, so energy is
     conserved);
   - continue the path at λ_ex. Beer–Lambert, refraction, NEE and the lamp are
     all evaluated at λ_ex.
4. **Wavelength channels.** Scenes with fluorescence trace **single-wavelength**
   paths: the 8-channel hero comb is off and the hero channel only carries the
   path. Each path still samples λ_em uniformly over 380–780, so the image
   converges unbiased. If the transport cannot run one channel, the implementer
   must split the path per channel at the fluorescence vertex, without bias, and
   document the choice.
5. **Limits.** At most one fluorescence vertex per path. Stokes shift only
   (λ_ex < λ_em). No delayed emission or phosphorescence.
6. **Bit-identity.** With an empty `Fluorescence`, the transport code path is
   bit-identical to today: no extra RNG draws, no channel-mode change.

## 5. Plumbing

- `SceneState.fluorescence: Fluorescence` (net v20, `serde(default)`), worker
  validation (finite values, Φ ∈ [0,1], ≤ 16 emitters, ≤ 8 bands each), and
  CPU routing.
- Desktop: the render context carries the active material's fluorescence
  (resolved from the custom material's recipe), next to the tools. The viewport
  CPU tracer, export, the remote scene and the metrics guide pass all receive it.
- Editor swatches: a "UV 365" and a "UV 395" glow swatch in the physics color
  panel. Each shows the emitted color and its relative brightness, computed
  analytically as Σ_c Φ_eff,c · (α_c(λ_lamp)/α_total(λ_lamp)) · f_c(λ) → XYZ.
  The panel also shows a "Fluorescence: none / weak / strong <color>" readout
  and, when the color is quenched, a note such as "quenched by Fe".

## 6. Tests and acceptance

1. Existing presets: the baselines, the white-balance values and every golden
   are unchanged (0 ULP).
2. Empty fluorescence: the transport is bit-identical; `scene_routes_to_gpu` is
   unchanged.
3. Energy: a furnace with Φ = 1 and an emission spectrum equal to the
   excitation spectrum gives total energy equal to the non-fluorescent case to
   within 1 % plus the λ_ex/λ_em factor. The test must fail with a deliberately
   doubled weight.
4. Ruby under UvLamp365: corundum + Cr₂O₃ 0.5 wt% renders a mean pixel color
   with hue h_ab ∈ [0°, 40°] and luminance > 10× the Φ = 0 render. With FeO
   1.0 wt% added, luminance < 10 % of the unquenched render.
5. UvLamp395 lights a non-fluorescent material faintly (violet tail). Its mean
   luminance is > 0 and < 5 % of Daylight.
6. Diamond N3 recipe under 365 nm: hue h_ab ∈ [230°, 290°].
7. Determinism: same seed gives the same bits.
8. Net: SceneState with fluorescence round-trips; a v19 peer is refused.

## Implementation status

F-A (renderer) and F-B (recipe coupling, desktop, editor, web core) are in. Not done:
the GPU path (out of scope by design) and the real-time viewport's own glow swatch
(the editor swatches are analytic, nothing is traced for them).

### Catalogue (`data/chromophores.toml`, `optics/chromophore/catalogue.rs`)

The new fields are optional and additive, and nothing changes for a chromophore that does not use
them (the fluorescence work itself kept `data_version` 3; the primary-data color refit of 2026-10-04
bumped it to 4, see the table notes below). A `[[host.chromophore.fluorescence]]` entry now carries
`emission_nm` (lines), `emission_fwhm_nm` (one value or one per line), `emission_weight`
(area share per line, default equal), `sideband = { centre_nm, fwhm_nm, weight }` (a Gaussian
carrying `weight` of all photons; the lines share the rest), `quantum_yield` (`Phi_0`),
`quench = [{ by, unit, c_half, n }]`, `confidence` and `source`. An entry emits only with
`quantum_yield > 0` and at least one line; `uv = inert`, no lines or `Phi = 0` give no
emitter. Lines under 1 nm are widened to 1 nm by the resolver (area kept). The loader checks
the fields (finite, ranges, at most 8 emission bands, a width for every emitting entry).

Values (all marked `confidence = "estimate"` unless noted; sources are the research files
and the Report cited in the data):

| Host / chromophore | Emission | Phi_0 | Quench (Fe) |
|---|---|---|---|
| corundum Cr3+ | R1 694.3 / R2 692.9 nm, 0.4 nm wide, weights 0.6 / 0.4, sideband 707 nm / 18 nm FWHM, 20 % | 0.9 (Report p.7-8, p.31); the only primary figure found, QE 0.83 +- 0.10 (Berdahl et al. 2016, NIST J. Res. 122:043), is a **coating-level** value of a ruby pigment including escape losses, not an intrinsic yield | `FeO`, c_half 0.2 wt%, n 2 (estimate from "gone by 0.5-1.0 wt% FeO", p.31); no primary Fe-halving value found |
| spinel Cr3+ | 685.5 nm, 1.5 nm, sideband 705 / 30 nm, 35 % | 0.9 (Report p.31) | same law as corundum |
| spinel Mn2+ | 515 nm, 40 nm wide | 0.5 (estimate) | none |
| chrysoberyl Cr3+ | 680.4 / 678.5 nm, sideband 750 / 60 nm, 30 % | 0.5 (estimate) | `Fe2O3`, c_half 0.3 wt%, n 2 (Report p.8-9 names the quenching, no number) |
| beryl Cr3+ (emerald) | 683 / 680 nm, sideband 700 / 25 nm, 30 % | 0.3 (estimate, weak) | `FeO`, c_half **0.0849 wt% (= 660 ppm Fe)**, n 2: derived estimate from the GIA Fall 2017 emerald PL study (abstract: count rate falls exponentially, tenfold at 2200 ppm Fe, basis unstated, probably ppmw; a pure exponential halves at 2200 x log10(2) = 660 ppm; the n = 2 law gives 12-fold at 2200 ppm), primary-data report section 8; data_version 3: 0.1 |
| diamond N3 | 415.2 nm ZPL (15 %), one band at 470 / 85 nm FWHM (85 %) | 0.3 (estimate) | none |
| diamond H3 | 503.2 nm ZPL (25 %), band 525 / 40 nm (75 %) | 0.6 (estimate) | none |
| diamond NV | 575 and 637 nm, band 680 / 65 nm (50 %) | 0.7 (estimate; primary range 0.5-1: NV0 ~1, NV- ~1 falling to 0.5 with pump power, arXiv:1501.03714; a conflicting study gives 0.05-0.55 depending on host size, arXiv:1712.07877) | none |
| YAG Ce3+ | 540 nm, 100 nm wide | 0.9 (Report p.12-13) | none |
| glass uranyl | 488 / 510 / 533 / 557 nm, 12-16 nm wide | 0.3 (estimate) | none |

Not found in the primary literature (research-2026-10-primary-data section 8): intrinsic quantum yields except NV, vibronic sideband shapes, excitation spectra over 300-420 nm, 365 : 254 nm excitation ratios, room-temperature R-line widths, Fe-halving concentrations except the derived emerald one, N3 / H3 yields, spinel / willemite / kunzite data; the values above stay estimates.

The N3 band is one Gaussian standing for the zero-phonon line's phonon replicas and the blue-green
band of Cape diamonds; its centre is chosen so that the glow reads sky blue (CIELAB hue about
268 degrees) as real stones do, not violet (a single 440 nm band reads 318 degrees).

Excitation of the diamond centres needs UV absorption the data lacked, so three UV bands were
added (estimates): N3 at 350 nm (4000 cm-1 FWHM, peak 0.0008 cm-1 per ppma, the strength of
its 415 nm band), H3 at 440 nm (6000 cm-1, 0.0006) and NV at 420 nm (8000 cm-1, 0.6). H3 and NV
only exist after a treatment, so only the N3 band changes an untreated recipe: it adds about
10 % to the 400 nm absorption of a Cape diamond. Uranyl keeps its single 420 nm band, so the glow
is excited by the 395 nm lamp and only through the band's tail by the 365 nm one; spinel Mn2+
has only its weak 450 nm band and glows faintly. These are data limits, not model limits.

Diamond treatments (`irradiation_anneal_h3`, `irradiation_anneal_nv`) use the existing
`create_centre` effect: 2 % (H3) and 1 % (NV) of the recipe's nitrogen becomes the centre,
capped at the centre's `conc_max` (estimates).

### Recipe to emitters (`optics/chromophore/fluorescence.rs`)

`resolve_fluorescence(catalogue, recipe)` returns the `Fluorescence`; `fluorescence_report`
adds the per-emitter `Phi_0`, `Phi_eff` and quenchers, and the competing background absorption.
Excitation is the chromophore's own bands scaled the way `resolve` scales them (effective
concentration, strength, treatment removals), polarisation averaged ((2 o + e) / 3 uniaxial,
(a + b + g) / 3 biaxial) and merged to at most 8 bands by moment matching; at most 16 emitters
(the strongest) are kept, so the result always passes `Fluorescence::validate`. `c_q` of a
quencher is the recipe amount of its element converted into the law's unit through the host's
number densities (corundum: 1.0 wt% FeO is about 7100 ppm of Al sites). An unknown host or an
inconsistent recipe gives an empty `Fluorescence`.

`uv_glow` / `FluorescenceReport::glow` is the analytic swatch of section 5: the emitted photon
yield per lamp photon `(1 - exp(-alpha_total L)) Sum_c Phi_c alpha_c / alpha_total`, its XYZ and
CIELAB hue (at relative luminance 0.4), a swatch color darkened with the yield, and the
none / weak (>= 1 %) / strong (>= 20 %) class. The host UV edge is not part of `alpha_total`
(the catalogue has no edge band for it).

`requires` / `excludes` of the catalogue are free text; where an entry names a chromophore of
the host (equal id or a prefix of it) a violated relation is reported as
`ResolveWarning::Relation`. It is advisory: the bands are untouched, because enforcing it
would change the existing colors.

### Plumbing

- Desktop: `RenderContext::custom_material_fluorescence` (a side channel beside
  `custom_material_physics`, written by the same five writers, resolved through
  `ColorMode::fluorescence`) and `RenderContext::active_fluorescence()`; it reaches the
  viewport CPU tracer and the GPU routing (`FrameInputs`, `BackendFrame`), the export
  (`SceneSnapshot::fluorescence`), the live and export remote scene (`SceneState::fluorescence`),
  the tilt hover preview and the scene identity (a change restarts accumulation). Fantasy
  materials give `None`. The guide pass and the optical metrics do not depend on the material's
  light and take no fluorescence; the batch tilt-curve request stays non-fluorescent.
- Editor: the physics color panel shows the "Fluorescence: none / weak / strong <color>"
  readout, "quenched by Fe", and the UV 365 / UV 395 swatches with their relative brightness.
- Web core: the web scene holds built materials, not recipes, so `OwnedScene` gets the
  emitters from `OwnedScene::build_with_customs(spec, hdr, &[CustomMaterialSpec])`
  (`CustomMaterialSpec::fluorescence()`), and `handle_trace_chunk` passes them to the tracer. The
  wire format of `SceneSpec` is unchanged.

### Tests

`optics/chromophore/fluorescence.rs` (ruby emitter, iron quenching, no emitter, validation of
every host at its maximum concentrations, glow hues: ruby 37 degrees, diamond N3 268 degrees),
`resolve.rs` (relation warnings), `tests/raytracer_tests/fluorescence.rs` (plan items 4 and 6
through the recipe route; the diamond one is `#[ignore]`, slow in debug), web core scene tests.
