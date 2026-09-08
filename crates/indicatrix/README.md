# indicatrix

A physically-based spectral gemstone renderer. It turns GemCAD-style cutting
schedules — a table of facet angles and index positions, as used by faceting
diagrams — into rendered output: full spectral analytical / Monte-Carlo
raytracing through faceted gemstone geometry, plus the gemological brilliance /
fire / scintillation metrics derived along the way.

`indicatrix` has no dependency on any particular UI toolkit or data source: callers
supply plain facet-plane geometry and a material, and get back rendered pixels
and/or optical metrics. It is used by `apps/indicatrix-cut` (interactive viewer),
`apps/indicatrix-worker` (headless CLI / remote render server), and `crates/indicatrix-net`
(the wire protocol between the two).

## What it physically models

- **8-channel stratified hero-wavelength spectral sampling (HWSS)** — each ray
  carries 8 wavelengths at once (one "hero" plus 7 rotated companions), avoiding
  both the color-banding of naive RGB rendering and the noise of fully independent
  per-wavelength sampling.
- **Full Stokes–Mueller polarized light transport** — 4D Stokes vectors and
  Mueller matrices for Fresnel reflection/transmission, total-internal-reflection
  phase retardation, and Brewster-angle extinction, not a scalar reflectance
  approximation.
- **Sellmeier / Cauchy dispersion** — continuous, per-wavelength refractive index
  from real dispersion equations, not three fixed RGB indices.
- **Birefringence** — uniaxial materials split into ordinary/extraordinary rays
  with spatial walk-off. **Uniaxial and biaxial birefringence both run on CPU and
  GPU** (equivalence-checked, see below) — the `BiaxialIndicatrix` machinery is
  ported to WGSL, so `GemMaterial::gpu_supported` is `true` for every material,
  Alexandrite/Topaz/Tanzanite included.
- **Beer–Lambert absorption with pleochroism** — directional absorption tensors
  (isotropic / uniaxial o-ray+e-ray / biaxial alpha+beta+gamma) so a pleochroic
  stone's color genuinely depends on the electric-field direction relative to the
  crystal's optical axes, not just on wavelength.

13 built-in materials ship with heavily cited Sellmeier/absorption data
(`GemMaterial::all_materials()`, `GemMaterial::by_name(...)`): Diamond, Sapphire,
Ruby, Emerald, Zircon, Alexandrite, Topaz, Spinel, Quartz, Tourmaline, Tanzanite,
Moissanite, Cubic Zirconia.

## Documentation map

This file is the entry point: the public API, feature flags, and dependency
policy. Two companion documents hold the deeper material:

- [`docs/physics.md`](docs/physics.md) — deliberate deviations from physical
  truth (do not "fix" these), known simplifications, and the bit-exact golden
  tests that guard against unintended drift.
- [`docs/gpu.md`](docs/gpu.md) — the GPU port of the spectral transport
  physics and its tiered equivalence harness against the CPU tracer.

## Public API tour

The pipeline is: **facet planes + material → `trace_spectral_ray` (per sample) →
CIE XYZ → tone-map/encode (per pixel)**, and separately: **facet planes + material
→ `evaluate_gem_optical_metrics` → brilliance / fire / scintillation**.

### Rendering a sample

```rust
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Ray, LightingPreset, trace_spectral_ray, xyz_to_srgb_gamma, hash_u32},
    },
};
use glam::Vec3;

let planes = StandardGemCuts::standard_round_brilliant(); // or StandardGemCuts::from_asc_schedule(&schedule)
let material = GemMaterial::diamond();                    // or GemMaterial::by_name("Sapphire").unwrap()

let ray = Ray {
    origin: Vec3::new(0.0, 2.5, 0.0),
    dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
};
let seed = 12345u32;

let xyz = trace_spectral_ray(
    ray,
    &planes,
    &material,
    12,                                                    // max_bounces
    LightingPreset::RingLights.studio(1.0, 0.85, 0.95),    // exposure, light_yaw, light_pitch
    seed,
    (hash_u32(seed) as f32) / 4_294_967_295.0,             // hero_rand
    None,                                                  // primary_hit_out
);
let rgba: [u8; 4] = xyz_to_srgb_gamma(xyz);
```

Real rendering accumulates many samples per pixel across `Camera::generate_ray`'s
jittered rays and sums the resulting `Vec3` radiance — never averages inline; see
`indicatrix-net`'s README for why the sum, not the average, is the thing that
composes across a distributed render.

### Optical metrics (brilliance / fire / scintillation)

```rust
use indicatrix::color::metrics::evaluate_gem_optical_metrics;

let metrics = evaluate_gem_optical_metrics(&planes, &material, cam_yaw, cam_pitch, light_yaw, light_pitch);
println!(
    "brilliance {:.1}%  fire {:.2}  scintillation {:.1}%  windowing {:.1}%  extinction {:.1}%",
    metrics.brilliance_pct, metrics.fire_index, metrics.scintillation_pct,
    metrics.windowing_pct, metrics.extinction_pct,
);
```

This fires an analytic ray grid (5-point sub-aperture bundles, the standard GIA
0–6° eye cone) through the real facet geometry at the d/F/C Fraunhofer lines —
it does not require running the Monte-Carlo path tracer at all.

### From a real `.asc` cutting schedule

```rust
use indicatrix_formats::asc::parse_asc;
use indicatrix::geometry::cuts::StandardGemCuts;

let text = std::fs::read_to_string("design.asc")?;
let schedule = parse_asc(&text)?;
let planes = StandardGemCuts::from_asc_schedule(&schedule); // real mast-derived planes, not fabricated
# Ok::<(), String>(())
```

`from_asc_schedule` is the non-fabricated path: it uses each tier's real `mast`
(depth) value from the `.asc` file as a plane offset. `from_database_angles` is
the fallback used when only angle/index data is available (e.g. scraped from a
catalog page with no attached `.asc`) — it fabricates proportional offsets and is
honest about that in its own name.

## Key types and invariants

- **`geometry::GpuFacetPlane { normal: [f32;3], d: f32 }`** — the core half-space
  plane type (`#[repr(C)]`, `bytemuck::Pod`), used for both CPU intersection and
  GPU buffer upload. `GpuFacetPlane::new` normalizes `normal`.
- **`geometry::GemPolyhedron`** — a validated boundary representation (vertices,
  facet polygons, triangle indices) built from a plane set via polar duality and a
  3D convex hull (`chull`). Vertex welding uses a fixed tolerance
  (`VERTEX_WELD_EPS`); designs whose crease points aren't computed exactly (as
  opposed to rounded to a few decimal places) can end up with an intended-coincident
  vertex silently split into two nearby ones — see `StandardGemCuts::emerald_cut`
  for how a profile-derived cut avoids this by computing crease points exactly.
- **`optics::materials::GemMaterial`** — crystal system, optical character,
  dispersion model, absorption tensors, birefringence delta, and (for biaxial
  materials) `biaxial_delta_beta_alpha`. `new_custom(...)` builds a material from a
  mean RI + dispersion/birefringence deltas + RGB absorption for cases with no
  cited spectroscopic data.
- **`optics::raytracer::trace_spectral_ray`** — the single entry point for one
  sample's radiance. `hero_rand` is caller-supplied, not derived internally, so
  callers can drive stratified sampling across pixels/samples themselves.
- **`renderer::pipeline::IndicatrixtracerPipeline` and `renderer::env_map_gpu::GpuEnvironmentMap`
  are separate, older dead scaffolding** that predates the real GPU port described
  below, and is unrelated to its state. `IndicatrixtracerPipeline::new` *always
  panics* — its own doc comment says so — because the WGSL shader it would load is
  quarantined and no longer contains a valid compute entry point. Do not read this
  as evidence about whether GPU physics exists — it doesn't govern that; see
  [The GPU port](#the-gpu-port-and-its-equivalence-harness) below for what actually
  does.

## Feature flags

All three are **off by default** — the base dependency set (`glam`, `bytemuck`,
`chull`, `tracing`, `indicatrix-formats`) is deliberately small so the crate stays lean and
publishable standalone (see below).

| Feature | Pulls in | Unlocks |
|---|---|---|
| `gpu` | `wgpu`, `pollster` | `renderer::gpu`: a verified GPU port of the spectral transport physics (`shaders/spectral_transport.wgsl`'s `transport_main` compute kernel) plus the equivalence harness that checks it against the CPU path — see below. Also compiles `renderer::env_map_gpu` and `renderer::pipeline`, which are unrelated, older dead scaffolding (`renderer::pipeline::IndicatrixtracerPipeline::new` panics if constructed — see [Key types](#key-types-and-invariants)). |
| `hdr` | `image` (default-features off, `hdr` format only) | Radiance `.hdr` equirectangular environment-map *decoding* for `renderer::env_map::EnvironmentMap`. The CPU-side importance-sampling machinery itself (`Distribution2D`, `radiance_at`, `sample`/`pdf`) needs no extra dependency and is always available even without this feature — only file decoding is gated. |
| `serde` | `serde`, `glam/serde` | `Serialize`/`Deserialize` on the scene-description types `indicatrix-net`'s wire protocol needs: `GemMaterial` and its nested types, `GpuFacetPlane`, `LightingPreset`. |

## Near-zero-dependency policy

This crate is meant to be publishable standalone, and its `Cargo.toml` says so at
every optional dependency: `indicatrix-formats` is called out as "a zero-dependency format
crate — adds nothing to indicatrix's own dependency tree beyond itself"; the `hdr`
feature's `image` dependency is pulled in with `default-features = false` and only
the `hdr` format feature, specifically to avoid dragging in png/gif/webp/avif/exr
decoders indicatrix has no use for; `serde` is feature-gated off by default "so the
base `indicatrix` dependency count... stays publishable." Even a dev-only example
(`examples/meet_solver_validation.rs`) uses `std::thread::scope` instead of pulling
in `rayon`, citing this same policy. If you're adding a dependency here, ask
whether it can be feature-gated, and whether a zero-dependency alternative exists,
before adding it unconditionally.

## `BUILD_ID`

```rust
pub const indicatrix::BUILD_ID: &str = env!("INDICATRIX_BUILD_ID");
```

A deterministic content hash (16 lowercase hex chars, 64-bit FNV-1a) of every file
under `src/` ending in `.rs` **or `.wgsl`**, plus `Cargo.toml`, computed at build
time in `build.rs`. Files are visited in sorted, POSIX-normalized relative-path
order and line endings are normalized (`\r\n` → `\n`) before hashing, so the same
source produces the same `BUILD_ID` regardless of host OS or checkout line
endings; the relative path is hashed alongside each file's contents, so a rename
alone changes the id.

`.wgsl` is included alongside `.rs` deliberately: a worker running a GPU backend
executes WGSL as its physics, not as decoration on top of it, and two workers with
byte-identical Rust but divergent WGSL would otherwise handshake as identical when
they aren't.

**Why this exists**: `indicatrix-net`'s wire protocol handshake refuses to pair a
viewer and a remote render worker whose `indicatrix` builds disagree. Mixing samples
traced by two different physics implementations produces a silently, plausibly
wrong image — no crash, no error, just numbers that look like a converged render
and aren't. `indicatrix`'s physics has changed many times in rapid succession during
development (the intersection routine, the Fresnel PDF, the CMFs, wavelength
construction, birefringent splitting, absorption, dispersion, spectral MIS), which
is exactly the situation a hand-maintained version number can't catch, and this
repository has no commit history for a `git`-based identity to fall back on
either. See `indicatrix-net`'s README for the handshake itself.

## The GPU port and its equivalence harness

**There is a real, verified GPU port of the spectral transport physics.**
`renderer/shaders/spectral_transport.wgsl`'s `transport_main` compute kernel is
a complete megakernel covering camera-ray generation, polyhedron intersection,
Sellmeier dispersion, Fresnel/TIR, full Stokes–Mueller polarized transport,
pleochroic Beer–Lambert absorption, Russian roulette, spectral MIS, and uniaxial
birefringence — this is not a stub or a partial port. It renders real frames
today, checked against the CPU tracer at **max genuine ULP = 0** by a tiered
equivalence harness (bit-exact integer checks, per-function ULP budgets, an
analytically-computable furnace anchor, and statistical comparison of real
rendered images). `gpu` is off by default everywhere, but the port itself is
complete: isotropic, uniaxial and biaxial materials all render on it, so
`GemMaterial::gpu_supported()` is unconditionally `true`.

Full detail — what's wired in where, how to run the harness, and what each of
its four tiers actually checks — is in [`docs/gpu.md`](docs/gpu.md).

The physics's deliberate deviations from strict physical truth, its known
simplifications, and the bit-exact golden tests that guard render output
against unintended drift are in [`docs/physics.md`](docs/physics.md).

## Dependency policy

Base (no-feature) dependencies: `glam`, `bytemuck`, `chull`, `tracing`, `indicatrix-formats`
— five crates. Adding a sixth, unconditionally, should be treated as a deliberate
decision to be documented in `Cargo.toml`, in keeping with the near-zero-dependency
policy above; prefer a feature gate if the functionality is not needed by every
caller.

## Testing

```
cargo test -p indicatrix                    # default feature set
cargo test -p indicatrix --features gpu     # additionally compiles gpu-feature unit tests
```

**`cargo test -p indicatrix` alone does not exercise the `gpu` feature at all** — a
real bug once hid in exactly that gap for a long time. If you touch anything
under `renderer::gpu`, `renderer::env_map_gpu`, `renderer::pipeline`, or anything
gated `#[cfg(feature = "gpu")]`, you must additionally run
`cargo test -p indicatrix --features gpu` to know whether it still compiles and
passes; `cargo test -p indicatrix` will report success while never having looked at
that code. (See the workspace root README for the same warning at the
workspace-verification level.)

No `#[test]`-attributed function in this crate requires a live GPU adapter — the
`--features gpu` unit tests (in `renderer/gpu/furnace_check.rs` and
`renderer/gpu/ulp.rs`) are pure-logic tests of the comparison/tolerance helpers
themselves. All GPU-adapter-dependent equivalence checking lives exclusively in
`examples/gpu_equivalence_harness.rs` (above), which `cargo test` never runs on
its own.

`examples/meet_solver_validation.rs` is corpus-scale validation tooling for
`geometry::meet_solver` — the module that derives a facet's mast (depth)
distance from its angle, index position, gear, and meet constraints alone, the
inverse of the forward plane-construction path described above.

It re-derives every mast in a real catalogue from angles and meets alone and
compares against the file's own recorded values, which is perfect ground truth
since every `.asc` already carries the answer. That needs thousands of real
designs, so it reads a `facet_diagrams.sqlite` catalogue directly rather than
living in the test suite — no such catalogue ships with this repository, so the
harness only does anything if you already have one. Several constants in the
solver cite its full-corpus runs as their provenance.

```
cargo run --profile probe -p indicatrix --example meet_solver_validation
```

It reads a database and writes nothing. Don't treat it as an example of this
crate's public API; it exists to measure, not to demonstrate.

The solver's own logic is covered by 23 unit tests under
`geometry::meet_solver`, which `cargo test` runs normally. See that module's doc
comment for the solver's current accuracy rather than relying on this README.
