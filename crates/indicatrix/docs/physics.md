# indicatrix — physics notes

What the tracer, the lighting models and the metrics actually compute today, the
deliberate deviations from physical truth, the known simplifications, and the
bit-exact golden tests that guard against unintended drift. Every statement cites the
function that implements it (paths are relative to `src/`). For the public API and the
feature-flag summary see [the README](../README.md); for the GPU port and its
equivalence harness see [gpu.md](gpu.md).

When a change alters one of the behaviours below, update this file in the same commit:
a sentence here that the code no longer satisfies is worse than no sentence.

## 1. The estimator in one page

- **Spectral sampling.** Each traced ray carries `NUM_CHANNELS = 8` wavelengths: a
  hero drawn over 380-780 nm plus seven companions rotated round the visible range by
  50 nm steps (`optics/raytracer/transport/mod.rs::wrapped_hero_wavelengths`). The hero
  is always slot 0 (`transport/inner.rs::trace_spectral_ray_inner`, `hero_idx = 0`).
- **One geometric path, eight Stokes vectors.** The hero's refractive indices decide
  every reflect/transmit branch and every direction; each channel keeps its own 4-vector
  Stokes state and its own running density `path_pdf[k]` (the density of "channel k as
  hero would have produced this exact path"). Polarisation is tracked with Mueller
  matrices (`optics/polarization`) in the frame of the current plane of incidence,
  rotated between bounces by `absorption.rs::rotate_stokes_to_plane_of_incidence`.
- **Wave normal versus Poynting direction.** Snell's law, Fresnel coefficients and the
  plane-of-incidence frame use the wave normal `k`; intersection and path length use the
  energy direction `S`. They differ only for the extraordinary / biaxial mode B while
  inside the crystal (`refraction/geometry.rs::poynting_dir_for_mode`).
- **Deterministic randomness.** Every stochastic decision is a pure hash of
  `(rng_seed, bounce, stream id)` (`optics/raytracer/sampling.rs::hash_u32` and the
  `*_STREAM` constants), so a sample is a pure function of its inputs.
- **Spectral MIS.** The eight channels are combined with the balance-heuristic weight
  `N * path_pdf[hero] / sum_k path_pdf[k]`, applied as one shared scalar
  (`optics/raytracer/color.rs::spectral_mis_weight`). With exit-event splitting (always
  on at the public entry points, `transport/mod.rs::trace_spectral_ray`) the sum runs
  over each channel's own compatibility family instead
  (`color.rs::integrate_channels_to_xyz_families`, `refraction/context.rs::narrow_compat`).
  If the sum underflows (`<= 1e-12`) both functions fall back to weight 1.0 instead of
  dividing by zero; a path of clamped `1e-4` factors can reach it, and the effect has
  not been measured.
- **Termination.** Russian roulette after bounce 4 with survival probability
  `q = max_intensity.clamp(0.05, 1.0)` and a `1/q` rescale of every Stokes vector and of
  the staged split radiance (`transport/bounce.rs::apply_russian_roulette`); the loop is
  also capped by `max_bounces`.
- **Output.** Per-channel radiance is integrated against the tabulated CIE 1931
  observer (`color.rs::cie_1931_cmf`, delegating to `color/cie1931.rs`) into XYZ, then
  optionally von-Kries white balanced (section 5.4), then encoded (section 6).

## 2. Interfaces

### 2.1 Which code path handles which material

| Situation | Path | Where |
|---|---|---|
| Uniaxial air -> crystal entry, wave normal not parallel to the optic axis | closed-form uniaxial Fresnel (Lekner 1991), both modes at once | `refraction/uniaxial_entry.rs::apply_uniaxial_entry_bounce`, `uniaxial_fresnel/` |
| Uniaxial hero-forced total internal reflection | closed-form `internal_solve` per channel | `refraction/tir.rs::apply_tir_bounce` |
| Any other uniaxial internal event (sub-critical partial reflection, exit into air) | closed-form `internal_solve` per channel | `refraction/uniaxial_internal.rs::apply_uniaxial_internal_bounce` |
| Isotropic material | scalar Fresnel, per-channel index | `refraction/reflect_refract.rs` |
| Biaxial material (entry and all internal events) | scalar Fresnel with a per-mode index | `refraction/reflect_refract.rs`, `refraction/dispatch.rs::apply_partial_fresnel_bounce` |
| Uniaxial wave normal within `1e-6` (squared cross product) of the optic axis | falls through to the scalar path at `n_o`, where both eigenmodes coincide | `refraction/dispatch.rs::try_dispatch_uniaxial_bounce` |
| Frosted facet | diffuse BSDF instead of any of the above | `scattering/frosted.rs::apply_frosted_bounce` |

### 2.2 Reflect versus transmit selection

The branch is drawn once, from the hero, with probability `r_unpol` (the unpolarised
reflectance), and every channel's contribution is divided by the probability that was
actually used (`1/r_unpol` for reflect, `1/(1 - r_unpol)` for transmit):

- **Selection clamp `[0.02, 0.98]`.** The drawn probability is clamped to this range
  (`refraction/dispatch.rs::apply_partial_fresnel_bounce`, `R_UNPOL_SELECT_MIN/MAX`; the
  same constants in `uniaxial_entry.rs::apply_uniaxial_entry_bounce` and
  `uniaxial_internal.rs::apply_uniaxial_internal_bounce`). The weights use the clamped
  value too, so the estimator stays unbiased while the `1/p` factor is capped at 50.
- **Path-density clamp `[1e-4, 1 - 1e-4]`.** The per-channel factors folded into
  `path_pdf[k]` are clamped to this range (`reflect_refract.rs::apply_partial_reflect_bounce`,
  `apply_channel_transmission_match`; `exit_split.rs::compute_channel_transmission`;
  `tir.rs::apply_tir_bounce`; `uniaxial_entry.rs`; `uniaxial_internal.rs`). They only
  feed the MIS weights, and the clamp keeps every density positive.
- **Hero-forced TIR** (`tir.rs::apply_tir_bounce`) is a probability-1 reflect with no
  division; a channel that is itself past its own critical angle receives the exact TIR
  phase retardation at its own index (`tir.rs::tir_phase_delta`), one that is not
  receives its partial-reflection matrix.

### 2.3 Entry mode selection (air -> birefringent crystal)

One geometric ray stands for two physical rays (ordinary / extraordinary, or mode A /
mode B), so a path is assigned one mode at entry. The selection is polarisation
weighted, and nothing is divided by 0.5:

- **Uniaxial, closed form.** The hero's Poynting-flux-weighted ordinary power fraction
  `p_o` (clamped to `[0.02, 0.98]`) comes from `uniaxial_fresnel::mode_power` fed the
  hero's actual Stokes state; every channel deposits `P_mode(stokes[k]) / p_mode_hero`,
  ordinary importance sampling with the true selection probability
  (`uniaxial_entry.rs::apply_uniaxial_entry_bounce`, `apply_uniaxial_entry_transmit_channels`,
  the `deposit_i` line).
- **Scalar path (biaxial, and the degenerate uniaxial limit).**
  `p_o = 0.5 * (1 + DoP_linear * cos(2 (psi - psi_o)))` from the hero's Stokes state
  (`uniaxial_entry.rs::entry_eigenmode_selection`, Malus's law); it falls back to 0.5
  when there is no linear polarisation to weight with. The selected mode's transmitted
  intensity is **not** divided by the selection probability, because that probability is
  the mode's energy share and the two cancel
  (`reflect_refract.rs::apply_channel_transmission_match`, the `incident_k` block and the
  comment above `scale(1.0 / (1.0 - r_unpol))`;
  `refraction/dispatch.rs::resolve_entry_mode_selection`). A biaxial entry always uses
  `p_o = 0.5` (no uniaxial ordinary eigenmode to weight against) and passes the Stokes
  vector through unprojected.
- **Frosted entry** is a flat 50/50 draw with no polarisation weighting and no division
  (`scattering/frosted.rs::apply_frosted_bounce`, `split_rand < 0.5`); the Stokes state is
  already depolarised there.

### 2.4 Internal o <-> e (A <-> B) mode coupling

Coupling is always on at the public entry points (`transport/mod.rs::trace_spectral_ray`
passes `enable_internal_mode_coupling = true`; only the transport tests turn it off) and
acts at every internal reflection inside an anisotropic crystal
(`transport/bounce.rs::maybe_apply_internal_mode_coupling`). It is a **relabelling**, not
a split: after each internal reflection the path's single mode label is redrawn
(`bounce.rs::apply_internal_mode_coupling`), which changes which index the next bounce
uses and touches neither `stokes` nor `path_pdf`.

- Uniaxial, closed-form reflections draw ordinary with the hero's exact Poynting-weighted
  share `R_o / (R_o + R_e)` (`tir.rs::apply_tir_bounce` and
  `uniaxial_internal.rs::apply_uniaxial_internal_bounce` return it as `exact_p_o`).
- **Known simplification:** there is one mode label per path, so the seven companion
  channels use the hero's share even though their own `R_o / (R_o + R_e)` differ slightly
  at other wavelengths.
- Biaxial materials relabel 50/50; uniaxial reflections without a closed-form share
  (degenerate axis, frosted internal reflection) use the polarisation-weighted
  `entry_eigenmode_selection` heuristic (`bounce.rs::apply_internal_mode_coupling`).

### 2.5 Biaxial entry: reflect probability versus transmission

At a biaxial entry the reflect/transmit branch is decided with mode **B**'s index
(`refraction/dispatch.rs::compute_entry_reflect_probability` keeps `geo.n2` unless the
material is uniaxial with the ordinary mode selected), and reflection uses the same
index (`reflect_refract.rs::apply_partial_reflect_bounce`), while the transmitted branch
uses the **selected** mode's index. The expected energy is therefore
`R_B + 0.5 (T_A + T_B) = 1 + 0.5 (R_B - R_A)`, not 1: an excess (or deficit) of
`0.5 (R_B - R_A)`, about 4e-4 of the incident energy for Alexandrite (an estimate from
the `n_A` / `n_B` gap, not a pinned value). The doc comment on
`apply_partial_fresnel_bounce` that says `R + T == 1` per mode holds for the uniaxial
path only. Uniaxial entries are exact (`compute_entry_reflect_probability` switches to
`n_o_hero` when the ordinary mode is drawn).

### 2.6 Dispersion, direction mismatch and exit-event splitting

Channels whose own Snell direction differs from the hero's are **not** dropped for the
whole trace:

- The match test is `dot >= DIRECTION_MATCH_COS_TOL (= 1 - 1e-6)`
  (`reflect_refract.rs::apply_refract_channel`, `uniaxial_entry.rs::apply_uniaxial_entry_transmit_channels`,
  `uniaxial_internal.rs`, `refraction/context.rs::narrow_compat`).
- A mismatched channel's **radiance** is zeroed at that event, but its `path_pdf[k]`
  keeps accumulating (technique k remains a member of every compatible channel's MIS
  family), and `narrow_compat` records the incompatibility at entry events.
- At the **exit** event (leaving the gem) the mismatched channel is resolved along its
  own refracted direction by one extra intersection test and a deterministic environment
  lookup at its own wavelength (`refraction/exit_split.rs::try_split_exit_channel`,
  Fresnel transmission from `compute_channel_transmission`). A probe that would re-enter
  the stone declines (an energy-loss truncation, like `max_bounces`). The staged
  `split_radiance` is committed only if the shared path itself escapes.
- A channel that cannot transmit at all where the hero did (`sin2_t_k > 1`) has density
  zero for that path.
- With splitting disabled (tests only) the same mismatch is plain chromatic termination
  (`path_pdf[k] = 0`).

## 3. Absorption, pleochroism, inclusions, finishes

- **Absorption** is Beer-Lambert with an absorption tensor, evaluated along the path's
  *assigned* eigenmode: `alpha = e_mode . A . e_mode`
  (`raytracer/absorption.rs::channel_absorption_alphas_assigned`, `apply_absorption`).
  For an optically isotropic material the coefficient is the midpoint of the two
  eigen-directions' quadratic forms, which equals `alpha_o` for every built-in because
  `alpha_o == alpha_e` there. Path lengths are multiplied by
  `GemMaterial::absorption_path_scale` (1.0 for every built-in). Nothing is re-emitted at
  another wavelength unless the scene carries a `Fluorescence` beside the material
  (section 3.1); a material without one has no fluorescence.
- **Fluorescence** is described in section 3.1.
- **Physical chromophore mode (`optics::chromophore`).** Custom materials can specify a physical
  gem recipe (`colorRecipe`) combining a host crystal (24 supported hosts) with real mineralogical
  chromophores (transition metals, intervalence charge-transfer pairs, radiation centres). The CPU
  forward model resolves the recipe into $\le 8$ energy-domain Gaussian bands
  (`BandShape::GaussianEnergy`) per eigenmode (ordinary, extraordinary, and optional beta ray for
  biaxial hosts), converting catalogue coefficients ($\text{cm}^{-1}$) to model units ($\text{mm}^{-1}$).
  Colorimetry (`color::body_color`) integrates transmittance at 1 nm resolution (380–780 nm) against
  CIE 1931 2° observer functions under CIE Standard Illuminant D65 and Incandescent (Planckian 3200 K),
  using CIEDE2000 ($\Delta E_{00}$) for color matching and color-change quantification.
  When stone width is unset (`0.0 mm`), materials with physics color default to
  `PHYSICS_DEFAULT_STONE_WIDTH_MM = 7.0 mm` via `effective_stone_width_mm`.
- **Inclusion scattering (opt-in, `scattering_sigma_s > 0`).** Homogeneous
  Henyey-Greenstein medium: one free-path distance and one scattered direction are drawn
  from the hero's `sigma_t` and shared by all channels; each channel's weight carries its
  own extinction (`scattering/hg.rs::maybe_scatter_or_extinguish`,
  `sample_henyey_greenstein_direction`). A scatter depolarises the Stokes vector and sets
  `k = S`. Russian roulette runs at scatter events too (`hg.rs::try_scatter_step`).
- **Frosted facets (opt-in per facet, `FacetFinish::Frosted`).** The polished dispatch is
  replaced by a cosine-weighted diffuse reflect/transmit
  (`scattering/frosted.rs::apply_frosted_bounce`). One broadband `r_unpol` from the **hero**
  channel (clamped `[1e-4, 1 - 1e-4]`) selects the branch for every channel and is folded
  into `path_pdf`; all channels share the direction; the Stokes vector is depolarised.
  Because the branch probability equals the energy fraction, no throughput division
  appears.
- **Edge rounding (opt-in, `GemMaterial::edge_rounding_radius > 0`).** A *shading-normal*
  perturbation, not geometry: within the radius of the nearest neighbouring facet plane the
  normal is blended toward the angle bisector of the two facet normals with a smoothstep
  (`optics/raytracer/intersect.rs::shading_normal_near_edge`, called from
  `transport/inner.rs::trace_spectral_ray_inner`). Intersections still use the flat
  planes. With radius `0.0` (every built-in) the function returns the facet normal
  untouched.

### 3.1 Fluorescence and the spectral range below 380 nm

CPU only (`optics/fluorescence.rs`, `raytracer/transport/inner.rs`). A material glows
through a `Fluorescence` that travels **beside** the `GemMaterial` (as the concave tools
travel beside the planes): `trace_spectral_ray_geom`, `trace_spectral_ray_with_finish_soa_geom`
and `trace_pixels_interleaved_geom` take a `&Fluorescence`, and `SceneState::fluorescence`
(net v20) carries it to a worker. An empty `Fluorescence` is the old renderer bit for bit:
no extra RNG draw, no change of the wavelength comb, `scene_routes_to_gpu` unchanged.

A `FluorescentEmitter` is one chromophore: `excitation` (its own absorption bands
`alpha_c(lambda)`, per mm), `emission` (Gaussian lines `f_c(lambda)`, weights normalised to
`Int f_c = 1`, lines narrower than 1 nm widened to 1 nm) and the effective quantum yield
`Phi_c`.

**Transport (backward, single wavelength).** The camera path runs at a visible `lambda_em`.
The emission term of the transfer equation, with isotropic emission, is
`Int dt T(t) Sum_c Phi_c f_c(lambda_em) Int_{300}^{lambda_em} alpha_c(l) (l / lambda_em) L_in(l) dl`;
the factor `l / lambda_em` is the photon-energy ratio, so a Stokes shift loses its energy to
heat and never gains any. It is estimated with one in-medium vertex per path:

1. The pseudo-extinction `mu_f(lambda_em) = Sum_c Phi_c f_c(lambda_em) A_c(lambda_em)`, with
   `A_c = Int_{300}^{lambda_em} alpha_c`, is the rate of a vertex along the path. `A_c` and the
   excitation distribution come from a per-emitter table at 1 nm steps (the bin average of
   the analytic bands), built lazily and cached in the `Fluorescence`.
2. On each interior segment a vertex distance is drawn from a truncated exponential, competing
   with the Henyey-Greenstein scatter sample (which stops at the vertex distance, so the two
   clocks race by memorylessness) and the boundary. The sampling rate is
   `mu_s = min(mu_f, 2 / segment)`: a strongly emitting line (`mu_f * segment` in the tens, a
   ruby R line) would otherwise give the paths that go on weights of `exp(mu_f * segment)`. The
   estimator weights compensate exactly: `mu_f / mu_s * exp(mu_s t)` at a vertex and
   `exp(mu_s ell)` for a path that reaches the boundary (or a scatter event) at `ell`. The
   medium's Beer-Lambert extinction up to the vertex is applied as for any segment.
3. At the vertex the emitter is chosen proportional to `Phi_c f_c A_c`, `lambda_ex` proportional
   to `alpha_c` on `[300, lambda_em)`, the new direction is isotropic and unpolarized, the weight
   is multiplied by `lambda_ex / lambda_em`, and the path continues at `lambda_ex`: refraction,
   Beer-Lambert, dispersion and the lamp are all evaluated there. The result is integrated at
   `lambda_em` (the camera wavelength).
4. A scene with fluorescence traces **single-wavelength** paths: all eight channels carry the
   one `lambda_em` over 380-780 nm, so the 8-channel hero comb and exit-event splitting
   are off. `lambda_em` is drawn from a 50/50 mixture of the uniform density and one
   proportional to `mu_f(lambda)`, and the path is weighted by `1 / (400 p(lambda_em))`
   (`Fluorescence::camera_wavelength`): a uniform draw would almost never land on a 2 nm line,
   and the uniform half keeps every wavelength reachable, so the image is still unbiased. (The channels are redundant but keep every other code path unchanged; the image
   converges unbiased.) At most one vertex per path; `lambda_ex < lambda_em` only; no delayed
   emission. RNG: five new hash streams (`FLUORESCENCE_*_STREAM`), drawn only when a vertex is
   sampled.

A UV lamp (`LightingPreset::UvLamp365`/`UvLamp395`, section 5.1) is CPU-only too.
`scene_routes_to_gpu(material, geom, fluorescence, lighting)` is false for a non-empty
`Fluorescence` or a UV lamp, as for tools.

**Spectral range.** Visible camera wavelengths stay 380-780 nm. Everything a fluorescence path
evaluates must be valid down to `lambda_ex = 300 nm`:

- *Illuminants:* `d65_relative_spectral_power` is extended to 300 nm with the CIE 15:2004 table
  (10 nm steps, 0.0341 at 300 nm to 49.9755 at 380 nm); the values at and above 380 nm are
  bit-identical (the extension is a separate branch below 380 nm). Blackbody is analytic; the UV
  lamps are Gaussians. (The WGSL port keeps the 380 nm clamp: UV scenes never reach the GPU.)
- *Dispersion:* `DispersionModel::evaluate` holds the index at `min_valid_nm()` below that
  wavelength: 380 nm for a `Cauchy` fit (visible-range-only, its `1/lambda^4` term runs away in
  the UV), 300 nm for a Sellmeier curve (or 5 % above its nearest UV resonance when that lies
  above 285 nm). A flat deep-UV index is a bounded approximation; at and above 380 nm nothing
  changes.
- *Absorption:* `AbsorptionBand::evaluate` is analytic and needs no change.

## 4. Next-event estimation and light/BSDF MIS

NEE exists only for an `EnvironmentSource::HdrMap`: the analytic studio rig has no
importance distribution (`transport/mod.rs` passes `enable_nee = matches!(environment,
HdrMap)`; `environment/rig.rs::sample_environment_for_nee` returns `None` for `Studio`).
It fires at two kinds of event:

- **Henyey-Greenstein scatter points** (`scattering/hg.rs::nee_contribution_hg_scatter`):
  a shadow ray to the exit facet, the exit Fresnel transmittance, refraction of the
  sampled direction through that facet, the medium transmittance, then a balance-heuristic
  weight against the phase-function density. Frosted exit facets are skipped here (they
  are handled by the frosted NEE). The exit transmittance is **one scalar computed at the
  hero's index** and shared by every channel, not a per-channel value. On a **concave**
  stone (tools present) the probe walks up to four boundary crossings: every exit
  multiplies in its own Fresnel transmittance, an entry into a cavity's far wall carries
  no factor, and the sample is dropped above the cap, on total internal reflection or a
  frosted exit at any crossing. The walk keeps the interior direction through the air
  gaps and bends it once, at the last exit, where the environment is looked up: an
  approximation, exact only when no cavity is in the way.
- **Frosted exterior events** (`scattering/frosted.rs::nee_contribution_frosted_exterior`):
  only for the two outcomes that leave the surface into the outward half-space, against a
  Lambertian `cos / pi` density. No shadow ray is needed for a convex stone: a surface
  point moving outward along its own outward normal cannot re-enter a convex solid. That
  argument fails for a concave stone, whose groove walls face a cavity, so with tools a
  real shadow probe is cast against the whole stone and the sample is deposited only on a
  miss. A frosted
  *entry* transmit (into the crystal) is deliberately not NEE-sampled.

The competing (phase / BSDF-sampled) continuation carries its density in
`pending_light_mis` until the path escapes, including through a polished exit refraction
(`transport/bounce.rs::dispatch_bounce`, `ExitSplitCtx::split_mis_weight`), and the
escape is weighted by `scattering/mod.rs::balance_heuristic` against
`environment_nee_pdf` at the carried interior direction
(`transport/inner.rs::trace_spectral_ray_inner`). Samples that land in the wrong
hemisphere, or are totally internally reflected at the exit facet, contribute zero by
design (the continuation keeps full weight there).

## 5. Lighting

### 5.1 Four models, nine presets

`environment/mod.rs::LightingPreset` has nine presets over four
`LightingModel`s. The CPU radiance is `environment/rig.rs::direction_lighting`; the WGSL
twins live under `renderer/shaders/` (see [gpu.md](gpu.md)).

| Preset | Model | Illuminant | White balance |
|---|---|---|---|
| Daylight (default) | Studio | tabulated CIE D65 | identity |
| Incandescent | Studio | Planckian 3200 K | Planckian -> D65 |
| RingLights | Studio | Planckian 5000 K | Planckian -> D65 |
| DarkSpotlight | Studio | Planckian 6000 K | Planckian -> D65 |
| IsoHemisphere | IsoHemisphere | tabulated CIE D65 | identity |
| LightTent | LightTent | Planckian 5000 K | Planckian -> D65 |
| DaylightDome | DaylightDome | tabulated CIE D65 | identity |
| UvLamp365 | Studio | Gaussian 365 nm, FWHM 10 nm | none (identity) |
| UvLamp395 | Studio | Gaussian 395 nm, FWHM 12 nm | none (identity) |

`LightingPreset::params` gives each preset's color temperature and `spot_mult`
(Incandescent 1.2, RingLights 1.6, DarkSpotlight 2.4, all others 1.0). The D65 curve is
the CIE 15:2004 table, not a 6500 K blackbody (`environment/spectral.rs::d65_relative_spectral_power`).

The two UV lamps (appended at the end of the enum, indices 7 and 8, so every earlier postcard
index is unchanged) use the analytic studio geometry of Daylight (key softbox, fill, ring) with
a unit-peak Gaussian lamp spectrum in place of D65 and **no ambient backdrop term**
(`LightingPreset::has_ambient_fill`). The 395 nm LED's tail reaches 380-420 nm, so it lights a
non-fluorescent stone faintly violet; that is kept. They are CPU-only (section 3.1);
`LightingPreset::params` returns the placeholder 6500 K for them (unused).

### 5.2 What each model computes

All rig directions come from `optics/studio_rig.rs::StudioRig::new(light_yaw,
light_pitch)`: the key at `(cos p sin y, sin p, cos p cos y)`, the fill at yaw
`+0.78 pi` and pitch `clamp(0.65 p, 0.15, 1.2)`, and sixteen ring emitters whose slot 0
shares the key's azimuth and whose following slots are `22.5 deg` apart in the key's
rotation sense.

- **Studio** (`rig.rs::studio_rig_lighting`): a dark charcoal backdrop
  (`0.015 + 0.012 (0.5 y + 0.5)`, floor `0.005`), a key softbox `key_dot^28 * 12 *
  spot_mult`, a fill softbox `fill_dot^18 * 4.5`, and the sixteen ring pinpoints
  (`((dot - 0.96) / 0.04)^6 * 22 * spot_mult` inside `dot > 0.96`), all times exposure.
  It ignores the observer.
- **IsoHemisphere** (`rig.rs::iso_hemisphere_lighting`): radiance 1 above the girdle plane
  (smoothstep over `y = -0.05..0.05`), zero below, times the observer head-shadow term.
- **LightTent** (`rig.rs::light_tent_lighting`): tent walls `0.14 + 0.08 max(y, 0)` cut to
  a tenth by three black cards at ring slots 4, 8 and 12 (`CARD_RING_SLOTS`, the ring
  positions a quarter, half and three quarters of a turn round from the key), an overhead
  softbox `1.4 * spot_mult`, a spark light at the fill direction `5 * spot_mult`, and
  `0.02` black velvet below the horizon.
- **DaylightDome** (`rig.rs::daylight_dome_lighting`): sky `0.10 + 0.08 (1 - max(y, 0))`,
  an aureole `0.30 * max(sun_dot, 0)^8`, a sun disc of radiance `10` (full within 2
  degrees of the key, gone by 4), and `0.04` ground.
- **Head shadow.** The three lit models darken every exit direction inside the observer
  cone (dark within 14 degrees of the eye direction, gone by 18;
  `rig.rs::observer_visibility`, `HEAD_SHADOW_*_COS`). The observer is the reverse of the
  pixel's primary ray (`transport/inner.rs::trace_spectral_ray_inner`, `observer`).
- **Backdrop card.** With `EnvironmentSource::Studio { backdrop > 0 }` the *camera ray*
  that misses the stone receives `backdrop * illuminant power` (`rig.rs::fill_backdrop`);
  the stone's own optics never see it.

### 5.3 HDR panoramas

`EnvironmentSource::HdrMap` looks up RGB and lifts it to a spectrum per channel with
`renderer/env_map_spectrum.rs::rgb_to_spectral_radiance`. The triple is split into a
neutral part `w = min(r, g, b)` and a non-negative chroma remainder `(r - w, g - w, b - w)`.
The neutral part is lifted with three wide asymmetric Gaussian bumps centred at 615, 545
and 465 nm, whose coefficients come through a fixed 3x3 matrix (`NEUTRAL_TO_BUMP`) chosen
so the spectrum's XYZ equals the linear-sRGB image of `(w, w, w)`; a grey texel therefore
keeps its smooth wide-bump spectrum unchanged. The chroma is lifted with three narrower
bumps centred at 635, 540 and 450 nm through an entrywise-positive matrix
(`CHROMA_TO_BUMP`), so its coefficients are never negative and the final `max(0, ..)` never
acts. XYZ is linear in the spectrum, so the sum reproduces the XYZ of the input triple
exactly (to `f32` rounding) for every non-negative RGB, saturated primaries included.
There is no metamerism, and values above 1.0 scale the bump height instead of acting
narrow-band. No white balance is applied to an HDR frame (section 5.4). The CPU constants
and the `mul_add` order are mirrored by the WGSL twins
(`transport_physics/05_nee_env_sampling.wgsl`, `environment.wgsl`).

### 5.4 White balance

For `EnvironmentSource::Studio` the final XYZ is multiplied by a Bradford-LMS von Kries
scale that maps the preset's illuminant white to D65 at equal luminance
(`color.rs::compute_illuminant_white_balance`, `apply_von_kries_white_balance`, applied in
`transport/inner.rs::trace_spectral_ray_inner`). The rule is explicit:
`LightingPreset::uses_white_balance()` is true only for the presets with a Planckian
illuminant, so only those four are adapted: Incandescent, RingLights, DarkSpotlight and
LightTent. The scale is the identity for the presets that sample the tabulated D65 curve
(`uses_d65`: Daylight, IsoHemisphere, DaylightDome) and for the UV lamps, which have no white
point to adapt from (the stone shows the lamp's own color; `color.rs::illuminant_white_balance`). `HdrMap` skips the transform entirely (the Bradford matrices are not exact
inverses, so running it at scale one would not be the identity in `f32`).

## 6. Tone mapping and encoding

`color/space.rs::ColorSpace::encode` with `ToneMap::AcesFilmic`: the ACES curve
(`color.rs::aces_tonemap`) is applied to **luminance only**, the XYZ vector is rescaled by
that one ratio, any channel above 1.0 is brought back with the bounded radial gamut walk
toward the white point (`color/gamut.rs::project_to_gamut_bounded`), then the true
piecewise transfer function is applied and each channel is quantised by rounding to the
nearest code value (`mul_add(255.0, 0.5)`).

## 7. The metrics model

`color/metrics/evaluate.rs::evaluate_gem_optical_metrics` does **not** run the path
tracer and does not describe the rendered image's lighting. It is an analytic fan:

- **One scalar index per line.** Classification uses the d-line index, the fire replay
  uses the F- and C-line indices (`evaluate.rs::build_grid_eval_setup`); each ray is
  refracted in and followed for at most 10 internal bounces with unpolarised scalar
  Fresnel transmittance (`color/metrics/ray_trace.rs::trace_wavelength`). Birefringence,
  pleochroism, scattering and facet finishes are ignored.
- **No lighting preset input.** The signature takes `light_yaw` / `light_pitch` only, so
  Studio, IsoHemisphere, LightTent and DaylightDome give identical numbers. "Returned to
  the observer" is a fixed test: outside a 16-degree head-shadow cone, and within
  `key_dot > 0.70` (about 46 degrees) or `fill_dot > 0.75` (about 41 degrees) or the ring
  annulus (`color/metrics/visibility.rs::ray_is_visibly_returned`). The renderer's key
  lobe is `key_dot^28` (half power at about 12.7 degrees), so the metrics' acceptance
  cones are several times wider than the lit region.
- **Sampling disc.** An 18 x 18 grid is kept where `u^2 + v^2 <= 0.70` and the ray origin
  is scaled by 0.95 (`evaluate.rs::evaluate_gem_optical_metrics`,
  `classify.rs::classify_aperture_sample`), a disc of radius about 0.80 model units
  regardless of the design's size: the central ~80% of a unit-radius girdle (about 63% of
  its area); the crown rim is never sampled. Each cell uses five sub-aperture directions.
- **Fire** is the transmittance- and exit-cosine-weighted F/C exit-direction separation,
  normalised by all incident rays and scaled by `FIRE_DEGREES_TO_DISPLAY_SCALE = 275`
  (a display scale, not a fit); a pair contributes only when both traces leave by the same
  facet after the same bounce count (`classify.rs`, the bifurcation gate).

## 8. The GPU routing predicate

`GemMaterial::gpu_supported` is `const fn ... { true }`
(`optics/materials/optics.rs::gpu_supported`): every built-in and every custom material is
routed to the GPU when a scene is otherwise eligible. It is an API seam kept so a future
incompatible material can opt out, not a per-scene check that currently discriminates.
Whether the GPU actually reproduces the CPU is what [gpu.md](gpu.md) and the recorded
harness run are for.

`renderer::gpu_backend::scene_routes_to_gpu(material, geom, fluorescence, lighting)` is the
per-scene predicate: it is false for a stone with tools, a non-empty `Fluorescence`, or a UV
lamp lighting (section 3.1), all of which only the CPU tracer handles.

## 9. Deliberate deviations from physical truth — do not "fix" these

- **The reflect/transmit selection clamp** (section 2.2) — `[0.02, 0.98]`, used for the
  branch draw *and* its weight. Without it, grazing incidence puts `1/p` at 1e4 and
  produces fireflies; with it the estimator is still unbiased.
- **The path-density clamp** (section 2.2) — `[1e-4, 1 - 1e-4]`, only inside `path_pdf`.
  It keeps every technique's density positive so the MIS weights stay finite.
- **The Russian-roulette survival floor** — `q = max_intensity.clamp(0.05, 1.0)` with a
  compensating `1/q` weight (`bounce.rs::apply_russian_roulette`). A hard cutoff would
  bias the estimator dark on the long internal-bounce trains of high-index stones.
- **Ray-offset epsilon** — a re-traced ray starts `1e-4` past the hit point along its new
  direction (`bounce.rs::dispatch_bounce`; the same offset in
  `exit_split.rs::try_split_exit_channel` and the NEE shadow rays) so it does not
  immediately re-hit the facet it just left.
- **The direction-match tolerance** (`DIRECTION_MATCH_COS_TOL = 1 - 1e-6`, section 2.6) —
  two evaluations of the same refraction formula at the same index agree to a few ULPs,
  two different indices differ by orders of magnitude more; the tolerance separates those
  two cases and is not exact float equality.
- **Luminance-only tone mapping** (section 6) — a per-channel tonemap is a hue-shifting
  operator, exactly wrong for saturated dispersion colors.
- **Bounded exit-split probe** — one extra intersection test per split channel, with a
  re-entering probe declined rather than recursed (section 2.6).

## 10. Known simplifications and asymmetries (measured or bounded, not deliberate)

- **Biaxial entry energy** is off by `0.5 (R_B - R_A)` (section 2.5).
- **Mode relabelling** uses the hero's `R_o / (R_o + R_e)` for all eight channels, and 50/50
  for biaxial materials (section 2.4).
- **NEE exit transmittance** and the **frosted BSDF split** use the hero channel's index /
  reflectance for every channel (sections 3 and 4).
- **Frosted entry mode draw** ignores incident polarisation (section 2.3).
- **`spectral_mis_weight` fallback** to weight 1.0 when the density sum underflows
  (section 1).
- **Chromatic termination at interior events**: a companion whose direction diverges at
  an entry event loses its radiance for the rest of the trace (only the exit event is
  split), and the mismatched fraction is re-sampled by other samples that draw that
  channel as hero.
- **Mismatched-direction re-entry** at an exit probe is truncated (energy loss, section 2.6).
- **Mueller depolarisation** at scatter and frosted events is total.
- **HDR spectral lift** has no round-trip guarantee for saturated colors (section 5.3).
- **Metrics** do not model the renderer's lighting, birefringence, absorption or the
  crown rim (section 7).

## 11. Bit-exact golden tests

`tests/raytracer_tests/golden_regression.rs` pins `f32::to_bits()` patterns of
`trace_spectral_ray` on real materials; `optics/raytracer/environment/tests.rs`
(`BASELINE_BITS`) pins the studio environment; `renderer/tonemap.rs` and
`renderer/frame_denoise.rs` pin output hashes; `tests/denoise_tests.rs` pins the
denoiser; and, with the `gpu` feature, `renderer/gpu/pin_tests.rs` pins whole-frame
`cpu_accumulate` output. They exist to catch *any* unintended drift in render output,
however small, across a refactor.

**The convention is not "these values must never change" — it is "these values must never
change silently."** When a genuine physics fix moves the pinned bits (a white-balance
correction, a new spectral lift, a sampling-rule change), the new values are captured
*with* a comment recording the old ones and why they moved, and, where practical,
cross-checked by an independent method (for example re-running with the new code path
forced off). Do not rebaseline one of these tests just to make a change pass: an
unexplained rebaseline defeats the purpose of the test, which is to notice that physics
drifted when nobody meant it to.

A default `cargo test -p indicatrix` does not compile the `gpu`-gated pins; run
`cargo test -p indicatrix --features gpu` after touching anything under `renderer::gpu`
(see the README).

## Concave stones: transport shortcuts

A stone with tools (`StoneGeometry::is_convex() == false`) is traced by the same loop as a
planar one; every convexity shortcut is gated on that predicate, so a planar design takes
the old code and stays bit-identical.

- **Bounce loop.** `intersect_stone_soa` replaces `intersect_polyhedron_soa`. `inside_gem`
  still toggles per crossing, which stays correct because every boundary of the material is
  a crossing; a hit after an exit is a legitimate re-entry across a cavity. Debug builds
  assert the flag against a point classification just before the hit.
- **Exit split.** `try_split_exit_channel` still drops a channel whose exit probe re-enters
  the stone. On a concave stone that is the largest energy risk, so debug builds count the
  dropped intensity (`exit_split_truncated_energy_take`) and the concave white-furnace test
  reports it next to its gate. If the measured loss is above the gate, the v2 fix is a
  bounded (four-crossing) continuation of the split channel through the re-entry.
- **Edge rounding.** `shading_normal_near_edge` blends toward the nearest *plane*, so it is
  skipped for a hit on a tool surface and for a plane hit within the rounding radius of a
  tool.
- **Metrics.** The grid, scintillation and tilt-sweep tracers use the same intersector. Their
  single-wavelength loop still ends at the first exit, so a ray that would re-enter across a
  groove counts as having left. The 18 x 18 fan is a fixed sample set and under-samples a
  narrow groove just as it under-samples a narrow facet.
- **Caches.** `hash_geometry` extends `hash_planes` with the tool bytes only when tools are
  present, so a planar key is unchanged.
