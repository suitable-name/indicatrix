# indicatrix — the GPU port and its equivalence harness

How `renderer::gpu` is verified against the CPU tracer, tier by tier. For the
public API and feature-flag summary see [the README](../README.md); for the
physics simplifications and golden tests see [physics.md](physics.md).

**There is a real, verified GPU port of the spectral transport physics.**
`renderer/shaders/spectral_transport.wgsl`'s `transport_main` compute kernel is
a complete megakernel covering camera-ray generation, polyhedron intersection
(both entry and exit branches), Sellmeier dispersion, Fresnel/TIR with
PDF-divided throughput, full Stokes–Mueller polarized transport, pleochroic
Beer–Lambert absorption, Russian roulette, spectral MIS with per-channel
`path_pdf` and chromatic termination, and uniaxial birefringence (θc
iteration, walk-off, per-mode indices) — this is not a stub or a partial port.
It renders real frames today, and every ported function is checked against its
CPU counterpart by the harness below, which reports the maximum genuine ULP per
check and per tier (see [The recorded run](#the-recorded-run)); a further tier
compares real rendered images. The measured values are in the recorded run, not
in this prose. This is the kernel the README's "What it physically models"
section means by "uniaxial birefringence runs on both CPU and GPU."

## What is and isn't true about its state

- **The physics is ported in full**, covering camera-ray generation through
  biaxial birefringence as described above; whether it currently matches the CPU is
  what the harness and its recorded run show.
- **It is wired into the viewer, behind a feature flag.**
  `renderer::gpu::frame::GpuFrameRenderer` is the general entry point: hand it a
  scene (camera, planes, material, environment) and it accumulates samples into
  a caller-owned buffer. `apps/indicatrix-cut` uses it when built with its own
  `gpu` feature (`cargo build -p indicatrix-cut --features gpu`), falling back to
  the CPU tracer per frame whenever the GPU declines — no adapter, a lost device,
  or an HDR environment map too large for the device's storage-buffer limit (HDR
  maps otherwise render on the GPU through their own environment mode).
  `apps/indicatrix-worker` can enable it too (its own `gpu` feature), but never for
  a viewport — that binary has none, only `render`, `serve --render` and `join` (see
  that app's README).
- **`gpu` is off by default, everywhere.** Nothing in the workspace turns it on
  for you, so an ordinary `cargo build` still pulls neither `wgpu` nor
  `pollster`, and the CPU tracer remains the reference implementation that every
  GPU result is checked against.
- **The chunked dispatch path has its own check.** A frame too large for
  `frame::CHUNK_BUDGET_BYTES` is split into pixel chunks via
  `GpuTransportParams::pixel_offset`; `frame::run_chunk_equivalence` (run by the
  harness below) requires a chunked render to be *bit-identical* to the same
  frame rendered in one dispatch. Every other GPU check dispatches a whole frame
  at once and so leaves that path unexercised.
- **Biaxial materials render on the GPU.** The `BiaxialIndicatrix` machinery is
  ported to WGSL and verified at the same Tier 2 / Tier 3 bar as every other
  material, so `GemMaterial::gpu_supported()` (`optics/materials/optics.rs`) is a
  `const fn` that returns `true` — including for the three biaxial built-ins
  (Alexandrite, Topaz, Tanzanite). It is an API seam that a caller assembling a
  scene can keep calling so a future incompatible material can opt out; today it
  does not discriminate between materials.

## Running the harness

```
cargo run --profile probe -p indicatrix --features gpu --example gpu_equivalence_harness
```

Add `-- --json <path> --date <YYYY-MM-DD>` to record the run (see
[The recorded run](#the-recorded-run)).

Needs a real GPU adapter — this is why it's an example with `required-features =
["gpu"]`, not a `cargo test` target: `cargo check`/`cargo test` without
`--features gpu` skip building it entirely. It prints `indicatrix::BUILD_ID` first for
traceability, and **exits nonzero on any check failure, or if no GPU adapter is
available at all** (a distinct exit code for "clean skip, nothing was tested" vs.
an actual divergence — read the printed message rather than just the exit code):
0 all checks passed, 1 a check failed, 2 no adapter or a bad command line, 3 the
requested summary file could not be written.

It runs through several phases, roughly in increasing order of what's being
compared:

- **Bit-exact integer checks** — GPU self-determinism (two dispatches of the same
  input must produce byte-identical output), struct-layout echo tests (a value
  round-tripped through a GPU buffer and back must match exactly — this is what
  catches WGSL's stricter `vec3`/`vec4` alignment rules silently misplacing a
  field relative to Rust's `#[repr(C)]` layout), and RNG bit-exactness.
- **Per-function ULP budgets** — individual physics functions (camera ray
  generation, CIE color-matching, Fresnel reflection/transmission, TIR
  retardation, dispersion, absorption, eigen-polarization, ...) compared CPU vs.
  GPU against a small, explicit floating-point tolerance, since CPU and GPU
  floating point are not required to agree bit-for-bit even when both are
  IEEE-754 compliant. The harness prints, per check, the **maximum genuine ULP**
  (over comparisons not exempted by the check's absolute-difference floor), the
  maximum raw ULP (including the exempted ones) and the budget, and aggregates the
  maximum genuine ULP per tier. The budget exists for honesty about what CPU/GPU
  floating point is and isn't guaranteed to do; what a given build actually
  measures is in the recorded run below, not in this prose.
- **A furnace anchor** — a uniform environment with zero (or trivial) geometry,
  checked against an *analytically computable* expected result, not merely
  CPU-vs-GPU agreement. This is what catches the case where CPU and GPU agree with
  each other but both disagree with the actual physics.
- **Statistical image comparison of real rendered frames** — the full
  `transport_main` spectral estimator, run end to end on real materials (Spinel, the
  uniaxial and biaxial built-ins, and the scattering, frosted-girdle, edge-rounding,
  lighting-model and HDR-NEE variants), each at a small pixel grid with disjoint
  CPU-traced and GPU-traced sample ranges (mirroring how a real distributed render
  actually splits work — see `indicatrix-net`'s README; every comparison prints its own
  size and sample counts), compared
  per-pixel via Welford mean/variance, a z-score threshold, and connected-component
  clustering of failing pixels — because at this level, exact or even ULP-level
  agreement isn't the right bar; a statistically consistent image is.

Related always-available modules: `renderer::gpu::determinism_check` (the
self-determinism tier) and `renderer::gpu::polyhedron_check` (a discrete
facet-index comparison for intersection results, with a documented allowance for
legitimate edge-grazing rays where two facets are within tolerance of the same
hit distance).

## The recorded run

The harness is not a `cargo test` target, so nothing in a default `cargo test` run
proves the GPU still matches the CPU. The repository therefore keeps one artefact of
the last harness run, `crates/indicatrix/docs/gpu_harness_last_run.json`, and **the
owner commits a fresh one with each physics change** (a change to anything under
`optics/raytracer/`, `renderer/shaders/`, or the color pipeline). The file does not
exist until the first recorded run; its absence means nobody has recorded one yet,
not that the port is verified.

```
cargo run --profile probe -p indicatrix --features gpu --example gpu_equivalence_harness \
    -- --json crates/indicatrix/docs/gpu_harness_last_run.json --date 2026-09-30
```

`--date` is required with `--json` and is recorded verbatim (the harness never reads
the clock). The file is written even when a check failed, with `"passed": false`, and
is never written when no adapter was found. It contains:

| Key | Content |
|---|---|
| `schema`, `date`, `build_id`, `adapter` | format version, the `--date` value, `indicatrix::BUILD_ID` (Rust and WGSL source hash), the adapter's name / device type / backend |
| `passed` | the overall verdict (every check group passed) |
| `groups` | one `{name, passed}` entry per check group (Phase 0/1, Phase 2, uniaxial, biaxial, inclusion scattering, frosted girdle, edge rounding, absorption path scale, lighting models, chunked dispatch, wavefront pipeline, kernel specialisation, HDR NEE) |
| `tiers` | per ULP family (`tier2_per_function`, `furnace_per_tuple`): number of checks, `max_genuine_ulp`, `max_raw_ulp`, `within_budget` |
| `ulp_checks` | every ULP comparison: tier, label, comparisons, `max_genuine_ulp`, `max_raw_ulp`, exempted count, budget (`null` when the check reports none), passed |
| `image_comparisons` | every Tier 3 comparison: label, size, samples per pixel per side, mean z, `\|z\|>3` pixel count and its binomial expectation, max `\|z\|`, largest connected cluster, passed |

Bit-exact tiers (Tier 0 determinism, Tier 1 integer and layout checks) appear only as
pass/fail in `groups`. "Genuine" ULP excludes comparisons the check's absolute-difference
floor exempts (values that legitimately cross zero); the raw figure includes them.
Review the `tiers` block, not this prose, to know what the port measures.

## Hardware unit tests

A few `#[test]` functions need a real adapter: the seven in
`renderer/gpu/frame/gpu_hardware_tests.rs`, the merge test in
`renderer/gpu/merge_tests.rs`, and the ones in `renderer/gpu_backend/tests.rs`. On a
machine without an adapter they print `skipping <test>: no GPU adapter` and pass. Set
`INDICATRIX_REQUIRE_GPU=1` and a missing adapter fails them instead, which is how a CI
machine that is supposed to have a GPU must run them:

```
INDICATRIX_REQUIRE_GPU=1 cargo test -p indicatrix --features gpu -- --test-threads=1 gpu_hardware_tests
```

What they assert with an adapter is narrow (no error, some non-zero radiance, the
renderer's poisoning rules, the reduction's non-finite rule, a statistical merge test);
the CPU-versus-GPU equivalence evidence is the recorded harness run above, and the
whole-frame `cpu_accumulate` identity pins in `renderer/gpu/pin_tests.rs` are compiled
only with `--features gpu`.

**Concave stones are CPU-only.** The WGSL has no tool intersection yet, so `renderer::gpu_backend::scene_routes_to_gpu` sends a scene with tools to the CPU tracer; porting the tool kernel needs an adapter and is a separate hardware-session work package.

**Fluorescence and the UV lamps are CPU-only.** `scene_routes_to_gpu(material, geom, fluorescence, lighting)` is also false for a non-empty `Fluorescence` or a `UvLamp365`/`UvLamp395` lighting: the in-medium fluorescence vertex, the single-wavelength paths and the 300-380 nm D65 extension have no WGSL twin (the shader's D65 table still clamps at 380 nm).
