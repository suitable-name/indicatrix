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
CPU counterpart at **max genuine ULP = 0**, with a further tier comparing real
rendered images (see below). This is the kernel the README's "What it
physically models" section means by "uniaxial birefringence runs on both CPU
and GPU."

## What is and isn't true about its state

- **The physics is complete and verified**, covering camera-ray generation
  through uniaxial birefringence as described above.
- **It is wired into the viewer, behind a feature flag.**
  `renderer::gpu::frame::GpuFrameRenderer` is the general entry point: hand it a
  scene (camera, planes, material, environment) and it accumulates samples into
  a caller-owned buffer. `apps/indicatrix-cut` uses it when built with its own
  `gpu` feature (`cargo build -p indicatrix-cut --features gpu`), falling back to
  the CPU tracer per frame whenever the GPU declines — no adapter, or an HDR
  environment map. `apps/indicatrix-worker` can enable it too
  (its own `gpu` feature), but never for the viewport itself — that binary has
  no viewport, only `render`/`serve` (see that app's README).
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
  material, so `GemMaterial::gpu_supported()` returns `true`
  unconditionally — including for the three biaxial built-ins (Alexandrite,
  Topaz, Tanzanite). It remains a real per-scene routing predicate that a caller
  assembling a scene should call per material and require `true` from before
  routing to the GPU backend, so a future incompatible material or a
  regression can return `false`.
- `renderer::pipeline` and `renderer::env_map_gpu` are unrelated, older dead
  scaffolding — see the README's "Key types and invariants" section — and say
  nothing about the transport kernel's state.

## Running the harness

```
cargo run --profile probe -p indicatrix --features gpu --example gpu_equivalence_harness
```

Needs a real GPU adapter — this is why it's an example with `required-features =
["gpu"]`, not a `cargo test` target: `cargo check`/`cargo test` without
`--features gpu` skip building it entirely. It prints `indicatrix::BUILD_ID` first for
traceability, and **exits nonzero on any check failure, or if no GPU adapter is
available at all** (a distinct exit code for "clean skip, nothing was tested" vs.
an actual divergence — read the printed message rather than just the exit code).

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
  IEEE-754 compliant. Every ported function currently measures **max genuine
  ULP = 0** — the allowed budget exists for honesty about what CPU/GPU floating
  point is and isn't guaranteed to do, not because any function actually needs
  slack today.
- **A furnace anchor** — a uniform environment with zero (or trivial) geometry,
  checked against an *analytically computable* expected result, not merely
  CPU-vs-GPU agreement. This is what catches the case where CPU and GPU agree with
  each other but both disagree with the actual physics.
- **Statistical image comparison of real rendered frames** — the full
  `transport_main` spectral estimator, run end to end on real materials
  (Spinel, Zircon, Tourmaline) at 48x48 pixels, 500 disjoint samples per side
  (CPU-traced and GPU-traced sample ranges are disjoint, mirroring how a real
  distributed render actually splits work — see `indicatrix-net`'s README), compared
  per-pixel via Welford mean/variance, a z-score threshold, and connected-component
  clustering of failing pixels — because at this level, exact or even ULP-level
  agreement isn't the right bar; a statistically consistent image is.

Related always-available modules: `renderer::gpu::determinism_check` (the
self-determinism tier) and `renderer::gpu::polyhedron_check` (a discrete
facet-index comparison for intersection results, with a documented allowance for
legitimate edge-grazing rays where two facets are within tolerance of the same
hit distance).
