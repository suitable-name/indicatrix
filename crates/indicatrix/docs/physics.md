# indicatrix — physics notes

The deliberate deviations from strict physical truth, the known simplifications,
and the bit-exact golden tests that guard against unintended drift. For the
public API and the feature-flag summary, see [the README](../README.md); for the
GPU port and its equivalence harness, see [gpu.md](gpu.md).

## Deliberate deviations from physical truth — do not "fix" these

- **The `r_unpol` clamp** (`optics/raytracer.rs`) — the unpolarized Fresnel
  reflectance used as a Monte-Carlo branch-selection probability (and as a
  divisor, for importance-sampling weight correction) is clamped away from exactly
  0 or 1 (`clamp(1e-4, 1.0 - 1e-4)`) before use. Without this, grazing or
  near-normal incidence can drive a divide-by-zero / infinite-weight blowup in the
  estimator.
- **The Russian-roulette survival floor** — path termination survives with
  probability `q = max_intensity.clamp(0.05, 1.0)`, not a hard cutoff below some
  intensity threshold. A hard cutoff discards the remaining energy outright and
  biases the estimator dark, worst on the long internal-bounce trains inside
  high-index stones; a probabilistic floor with compensating `1/q` weighting on
  survival keeps the estimator unbiased in expectation while still bounding path
  length. The `0.05` floor guarantees a minimum 5% survival chance regardless of
  how dim a path has become.
- **Ray-offset epsilons** — a re-traced ray's origin is nudged along its direction
  by a small fixed epsilon (`1e-4`) past the hit point, so it doesn't immediately
  re-intersect the facet it just left due to floating-point rounding. Standard
  practice, but a magic number that looks removable until you remove it.
- **The direction-match tolerance** (`DIRECTION_MATCH_COS_TOL = 1.0 - 1e-6`) — used
  to decide whether a companion spectral channel's independently-computed
  refraction direction has diverged from the hero-driven shared path (in which
  case its contribution is dropped to exactly zero, not down-weighted). This is
  deliberately not exact float equality: two evaluations of the *same* refraction
  formula with the *same* index agree to a handful of ULPs, but two *different*
  indices produce a direction difference many orders of magnitude larger — the
  tolerance is chosen to separate those two cases, not to be "close enough" in any
  looser sense.
- **Luminance-only tone mapping** — the ACES filmic tone-mapping curve is applied
  to luminance only, and the whole RGB/XYZ vector is then rescaled by that single
  scalar ratio, never applied per-channel. A per-channel tonemap is a hue-shifting
  operator, which is exactly wrong for saturated dispersion "fire" colors — this
  renderer's marquee output. The gamut-projection step downstream of this
  (`color::gamut::project_to_gamut_bounded`) uses the same reasoning: an
  over-bright color is desaturated gracefully via a bounded radial walk toward the
  white point, never hard-clamped per channel, for the same hue-preservation
  reason.

## Known simplifications

- **No ordinary↔extraordinary mode coupling at internal reflections.** Entry into
  a birefringent material stochastically assigns a path to the ordinary or
  extraordinary eigenmode once, at the air→crystal interface (50/50, with the
  throughput divided by the 0.5 selection probability). Every subsequent internal
  TIR bounce keeps using that same eigenmode's index — mode-splitting does not
  happen again at internal reflections, only at entry. Exiting the crystal (or any
  refraction event inside an isotropic material) reduces to ordinary single-index
  behavior.
- **No surface-roughness / polish model.** Every facet — including the girdle — is
  a perfectly smooth half-space plane; there is no microfacet distribution, no
  frosted/matte finish, and no polish-quality variation.
- **Sharp facet edges.** Facet-to-facet transitions are geometric edges with no
  rounding or fillet, matching how faceting schedules themselves specify perfectly
  flat planes meeting at exact edges.
- **No fluorescence and no inclusion/subsurface scattering.** Absorption is purely
  directional Beer–Lambert (`AbsorptionTensor`) — light that enters a stone either
  transmits, reflects, or is absorbed along a straight path between facet hits;
  there is no re-emission at a different wavelength and no scattering off internal
  inclusions.
- **Every material renders on both CPU and GPU, biaxial ones included** — the
  `BiaxialIndicatrix` machinery is ported to WGSL and verified at the same
  Tier 2 / Tier 3 bar as the rest of the transport physics (see [gpu.md](gpu.md)),
  so `GemMaterial::gpu_supported()` returns `true` unconditionally. It remains a
  real per-scene routing predicate, not a stub that always returns `true`, so a
  future incompatible material or a regression can return `false`.

## Bit-exact golden tests

`tests/raytracer_tests.rs` and `tests/denoise_tests.rs` pin exact `f32::to_bits()`
hex patterns for `trace_spectral_ray`'s output on real materials, and for the
denoiser's filtering output — these exist to catch *any* unintended drift in
render output, however small, across a refactor.

**The convention observed in this codebase is not "these values must never
change" — it's "these values must never change silently."** When a genuine
physics fix does change the pinned bits (the test file's own history records
real examples: a white-balance correction, pleochroism populating previously-empty
absorption bands), the new golden values are captured *with* an inline comment
recording the old superseded values and exactly why they changed, and — where
practical — cross-checked against an independent method (e.g. re-running with the
new code path forced off, or against the pre-change tree). Do not rebaseline one
of these tests just to make a change pass without narrating why the values moved
and confirming the new values are the physics working *correctly*, not a new
bug that happens to also change the bits. An unexplained rebaseline defeats the
entire purpose of these tests, which is to be the thing that notices physics
drifted when nobody meant it to.
