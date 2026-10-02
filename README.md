# Indicatrix

[![#MadeWithSlint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-light.svg#gh-light-mode-only)](https://slint.dev)
[![#MadeWithSlint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-dark.svg#gh-dark-mode-only)](https://slint.dev)

A physically-based spectral gemstone renderer and faceting-design library, in
Rust. It's for anyone who wants to see what a real cut gemstone actually looks
like from its real cutting instructions — spectral fire and polarization included,
not the RGB/scalar-IOR approximation every conventional renderer reaches for —
and for anyone building their own library of faceting designs (`.asc` files)
around one.

## What makes it technically interesting

At the center is `indicatrix`, a spectral path tracer that traces real gemstone
geometry rather than approximating it:

- **8-channel stratified hero-wavelength spectral sampling (HWSS)** — each ray
  carries 8 wavelengths at once (one "hero" plus 7 rotated companions),
  avoiding both the color-banding of naive RGB rendering and the noise of
  fully independent per-wavelength sampling.
- **Full Stokes–Mueller polarized light transport** — 4D Stokes vectors and
  Mueller matrices for Fresnel reflection/transmission, total-internal-
  reflection phase retardation, and Brewster-angle extinction, not a scalar
  reflectance approximation.
- **Sellmeier / Cauchy dispersion** — continuous, per-wavelength refractive
  index from real dispersion equations, not three fixed RGB indices.
- **Beer–Lambert absorption with pleochroism** — directional absorption
  tensors so a pleochroic stone's color genuinely depends on the
  electric-field direction relative to the crystal's optical axes, not just
  on wavelength.
- **An optional GPU megakernel** — `indicatrix`'s `gpu` feature compiles a
  complete WGSL port of the spectral transport physics, verified against the
  CPU tracer by a tiered equivalence harness: bit-exact integer checks,
  per-function ULP budgets (the harness reports the maximum genuine ULP per
  tier; see `crates/indicatrix/docs/gpu_harness_last_run.json` once the owner
  commits a run), an analytically-computable furnace anchor, and
  statistical comparison of real rendered images. See
  [`crates/indicatrix/docs/gpu.md`](crates/indicatrix/docs/gpu.md).

## Workspace map

| Crate / app | What it is |
|---|---|
| [`crates/indicatrix`](crates/indicatrix/README.md) | The spectral path tracer — the physics core. Facet planes + material in, rendered pixels and/or brilliance/fire/scintillation metrics out. |
| [`crates/indicatrix-net`](crates/indicatrix-net/README.md) | Wire protocol between a viewer, a coordinator and its render workers: offloading `indicatrix` sample tracing, and reading a remote design library. Types and framing only — no sockets. |
| `crates/indicatrix-dispatch` | Sample-range scheduling for render lanes: the shared disjoint sample cursor, per-lane rate models and chunk sizing, and a lane pool that merges chunk sums deterministically. No networking and no GUI types; used by both the desktop app and the coordinator. |
| `crates/indicatrix-cut-core` | The editor core behind `indicatrix-cut`'s Edit tab: preform, editable cutting instructions, undo/redo, design templates. |
| `crates/indicatrix-solid` | The solid and diagram previews: the software rasteriser, the GemCAD-style three-panel diagram, facet picking and the preview pipeline. No GUI types; used by the desktop app and the browser app. |
| `crates/indicatrix-editor` | The editing logic behind the design editor with no GUI in it: the editor session, tier forms, file loading and naming, retargeting, the cutting sheet, the guided walkthrough and the mouse-manipulation math. Used by the desktop app and the browser app. |
| `crates/indicatrix-web-core` | The compute side of the browser app (no DOM, no Slint): the Web Worker protocol, the CPU chunk tracer and its accumulation, the worker-side solve, Optimize, metrics and tilt sweeps, and the persisted settings. Tested natively. |
| [`crates/indicatrix-formats`](crates/indicatrix-formats/README.md) | Reader/writer for GemCAD-style `.asc` cutting-instructions files. Zero runtime dependencies. |
| [`crates/indicatrix-vault`](crates/indicatrix-vault/README.md) | Local SQLite-backed design library — models, storage, and local `.asc` import/export. |
| [`apps/indicatrix-cut`](apps/indicatrix-cut/README.md) | The desktop faceting-design editor, built with [Slint](https://slint.dev/): browse, search, and render your library in 3D, with a faceting editor for material retargeting and a solid inspection view. |
| [`apps/indicatrix-web`](apps/indicatrix-web/README.md) | The browser build (Slint compiled to WebAssembly, static files): open or start a design, edit it, view it as a solid, a diagram and a CPU-rendered stone, and save it back as downloads. Rendering and slow solves run in Web Workers (`apps/indicatrix-web-compute` is their entry point). No design library, remote rendering, Deep Solve or GPU path; state lives in the tab's session only. Built with `trunk`. |
| [`apps/indicatrix-worker`](apps/indicatrix-worker/README.md) | Headless render CLI and remote server — `serve` is a coordinator that serves a design library over mutual TLS and (on a `worker` build) spreads render requests over the machines that `join` it, rendering itself too with `--render`. |

## Build and run

```
git clone https://github.com/<your-username>/indicatrix.git
cd indicatrix
cargo build --workspace
cargo run -p indicatrix-cut
```

`cargo run -p indicatrix-cut` opens the editor; it looks for (and creates, if
missing) `facet_diagrams.sqlite` in whatever directory you launch it from —
see [The design library is yours to build](#the-design-library-is-yours-to-build)
below.

To render a scene headlessly instead of opening the GUI:

```
cargo run -p indicatrix-worker --features worker -- render --scene scene.json --out render.png --width 1920 --height 1080 --samples 256
```

`--features worker` is required — a default `indicatrix-worker` build compiles
neither `indicatrix` nor the render path in at all (see
[Feature flags](#feature-flags) below). The `render` subcommand still exists
without it, but running it just prints an error telling you to rebuild with
`--features worker`. See
[`apps/indicatrix-worker`'s README](apps/indicatrix-worker/README.md) for how to
produce a `scene.json`, and for the `serve`/`join`/`cert` subcommands that let
other machines' GPUs and CPUs help render over the network.

### Testing

```
cargo test --workspace                        # everything EXCEPT indicatrix's gpu feature
cargo test -p indicatrix --features gpu            # the gpu-feature tests the line above skips
```

**`cargo test --workspace` does not build or exercise `indicatrix`'s `gpu` feature
at all** — a real bug once hid in exactly that gap for a long time. If you've
touched anything under `indicatrix`'s `renderer::gpu` or `renderer::env_map_gpu`, or
anything else `#[cfg(feature = "gpu")]`, run the
second command too. `apps/indicatrix-cut` and `apps/indicatrix-worker` each have
their own `gpu` feature that forwards to `indicatrix/gpu`; the same rule applies
to those (`cargo test -p indicatrix-cut --features gpu`,
`cargo test -p indicatrix-worker --features gpu`).

Neither of those substitutes for `indicatrix`'s GPU equivalence harness, which
needs a real GPU adapter and isn't a `cargo test` target at all:

```
cargo run --profile probe -p indicatrix --features gpu --example gpu_equivalence_harness
```

See [`crates/indicatrix/docs/gpu.md`](crates/indicatrix/docs/gpu.md) for what each of
its verification tiers checks.

### Optimized builds

`cargo build --release` is fine for everyday use; `.cargo/config.toml` gives every x86-64
build the `x86-64-v3` baseline (AVX2 + FMA, Haswell / Zen or newer) so `f32::mul_add`
compiles to the fused instruction instead of an `fmaf` library call. Results are
bit-identical either way; override with `RUSTFLAGS="-C target-cpu=x86-64-v2"` for an
older machine. For distribution builds there are
two profile-guided scripts. Both build the instrumented and the optimised binaries with
IDENTICAL package sets, target, target-dir and `--features` -- only `RUSTFLAGS` (via
`CARGO_ENCODED_RUSTFLAGS`) differs between the two steps. This matters: a package-set or
feature difference between the two builds changes downstream crates' `-C metadata`
fingerprint (and therefore their mangled-symbol crate hash), which makes `-C
profile-use` silently find nothing to apply. Each script's own header comment explains
the parity rule, and its post-build crate-hash and warning checks fail the build when the
profile did not apply, instead of shipping a silently unoptimized binary.

Both train on `indicatrix`'s `pgo_train` example: the CPU spectral tracer across every
material optical character (isotropic, uniaxial +/-, biaxial +), dispersive and
near-non-dispersive built-ins, plain and frosted-girdle finishes, inclusion scattering,
the A-Trous denoiser and tone-mapper (both the linear-HDR float buffer and the
tonemapped 8-bit output); every lighting model (the analytic studio rig's four presets
plus the `IsoHemisphere`/`LightTent`/`DaylightDome` lit models) and a synthetic HDR
equirectangular map; the meet-point solver's three phases plus the verified repair
search (and through it the SIMD candidate-vertex/plane-intersection kernels) against
both synthetic AND real, catalogue-sourced `.asc` designs -- including the crate's own
205-facet-plane CrackOtto-Step fixture -- plus external solid measurement and
CAD-preview mesh extraction on the solved geometry; the `.asc` reader *and* writer,
B-Rep solid reconstruction; and the tilt-performance sweep. With the `gpu` feature (and
a real adapter present) it also trains the CPU-side GPU chunk dispatch/readback
orchestration. See that example's own doc comment for the exact stage-by-stage coverage
table and its `PGO_SCALE`/`PGO_TRAIN_SKIP_GPU` environment knobs.

Both scripts additionally train `indicatrix-cut-core`'s OWN `pgo_train` example -- the
editor-core layer built on top of `indicatrix`: `Design::solve`, `.asc` export, the
`.indicatrix` design file save/load round trip, `resolve_after_edit`, and
`optimize_design`'s coordinate-search loop, run against every built-in template plus
the same CrackOtto-Step fixture. Always built and run in its OWN, separate `cargo
build` invocation rather than folded into the SAME `-p indicatrix ... --example
pgo_train` command above: both crates ship a `pgo_train` example
(`indicatrix`'s is `examples/pgo_train/{main,stages}.rs`), and Cargo's
example-binary output-path uplift is keyed by name alone, not by package, so selecting
both in one invocation was measured to make Cargo emit an unresolvable "output filename
collision" between them (see each script's own header comment for the full story). Its
library code is still correctly instrumented and optimised either way, since
`indicatrix-cut-core` is already a transitive dependency of `indicatrix-cut` in the
main build; the separate invocation exists only to execute that code so it collects
real profile samples.

Both scripts also train a third binary, `indicatrix-net`'s `scene_roundtrip`
integration test, under the same instrumentation: `SceneState`'s postcard
wire-protocol encode/decode, the one hot path `pgo_train` cannot reach directly (a
`indicatrix` example can't depend on `indicatrix-net`, which itself depends on
`indicatrix`). Both scripts then rebuild both applications against the merged profile
and check that the profile applied to BOTH `indicatrix` and `indicatrix-cut-core`
(each trained by a build whose package and feature set is narrower than the optimised
build's, so each crate's hash is compared separately).

**Training coverage is a known limitation, not a complete picture.** The training
binaries do not exercise the radiance (HDR) image codecs, the solid rasterizer,
`guide_pass`, the `indicatrix-vault` database, the desktop editor's event loops and GUI
code, or the `GpuBackend` type, so those paths are optimised as cold code. The `gpu`
tier's `train_gpu` stage trains `render_hybrid`, which no app calls today. Do not read a
successful crate-hash check as "the whole program is profiled": it proves the profile
applied to the trained crates, nothing about how much of them it covers.

| Script | Platform | Does |
|---|---|---|
| [`scripts/pgo-build.ps1`](scripts/pgo-build.ps1) | Windows (PowerShell) | PGO, over AVX2/scalar x GPU/CPU by default (`-Cpu all`/`-Cpu avx512` auto-skip/refuse AVX-512 on a host without it) |
| [`scripts/pgo-bolt-build.sh`](scripts/pgo-bolt-build.sh) | Linux or Windows (bash); a Linux host can also build Windows binaries with `--os windows` | PGO over AVX2/scalar x GPU/CPU by default (`--isa all`/`--isa avx512` auto-skip/refuse the same way), plus BOLT on Linux |

Each build is emitted as `<binary>-<os>-<isa>-<gpu>`, so the variants can coexist in
one output directory. `--native` (bash) / `-Native` (PowerShell) builds only the
host-CPU tier (`-C target-cpu=native`) instead of the generic tiers, for a quick
"this machine only" build. The ISA tier is a compile-time `-C target-cpu` baseline and is
independent of `indicatrix`'s runtime SIMD dispatch, which detects AVX-512/AVX2 on any
build unless `INDICATRIX_SIMD` caps it; the training run sets that variable to match the
tier being built, which means a profile trained under `INDICATRIX_SIMD=avx2` leaves the
avx512 and scalar SIMD kernels with zero profile counts (LLVM then treats them as
cold/size-optimised in that binary) -- ship the avx2 build to AVX2-and-newer machines
(including AVX-512 ones) and the scalar build only to machines without AVX2.

Training executes the instrumented binaries, so it must use the SAME target triple as
the optimised build it feeds: a target triple is part of every crate's `-C metadata`
hash, and so of its mangled symbol names, so a profile trained under one triple
silently applies nothing to a build for another. `scripts/pgo-bolt-build.sh` builds for
the host's own OS by default; on a Linux host, `--os windows` cross-builds
`x86_64-pc-windows-gnu` binaries in one of two ways:

- **With a runner** (`PGO_CROSS_RUNNER` if set, else `wine64` or `wine` from `PATH`):
  the instrumented Windows binaries are trained under Wine and the result is a real
  PGO build. The GPU training stage is skipped under Wine, so GPU orchestration code
  stays cold in such a profile; run `scripts/pgo-build.ps1` on Windows to get it warm.
- **Without a runner**: a plain optimised release build, labelled "NO PGO" in the log
  and the final summary. The old script produced this silently: its Linux-trained
  profile never matched the Windows symbol hashes. `--collect-only` and `--skip-train`
  are refused in this mode.

Either way, cross-building needs the mingw-w64 toolchain (`x86_64-w64-mingw32-gcc`, or
`CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER`) and the `x86_64-pc-windows-gnu` rustup
target. Cross-built Windows binaries get their icon only when
`x86_64-w64-mingw32-windres` is installed (`winresource` warns and continues
otherwise). Building Linux binaries from a Windows host is not supported.

BOLT runs on Linux only — it has no PE/COFF backend — and only on `indicatrix-worker`,
which has a headless, deterministic `render` workload to profile (`render --scene
<scene.json> ... --only-cpu`; `scripts/pgo-bolt-build.sh` generates a fixed training
scene via `indicatrix-worker`'s `write_pgo_scene` example). `indicatrix-cut` is an
interactive desktop app with no scriptable workload, and a startup-only profile would
give it a worse code layout than no BOLT at all.

## Feature flags

All of the following are **off by default**, except `indicatrix-net`'s `compression`.

| Crate | Feature | Adds |
|---|---|---|
| `indicatrix-net` | `compression` (**on** by default) | The lossless compressed payload encodings (byte shuffle + zstd / LZ4) and the PNG display-frame codec. Without it the protocol still works, sending raw payloads. |
| `indicatrix` | `gpu` | `renderer::gpu`: the verified GPU port of the spectral transport physics and its equivalence harness. Pulls in `wgpu`, `pollster`. |
| `indicatrix` | `hdr` | Radiance `.hdr` equirectangular environment-map *decoding* for `renderer::env_map`. Pulls in `image` (default features off, `hdr` format only). |
| `indicatrix` | `serde` | `Serialize`/`Deserialize` on the scene-description types (`GemMaterial`, `GpuFacetPlane`, `LightingPreset`) that `indicatrix-net`'s wire protocol needs. |
| `indicatrix-net` | `render` | `SceneState`/`RenderRequest` and everything else that needs `indicatrix`'s resolved scene/material types. Off by default so a library-only `indicatrix-worker` build never compiles `indicatrix` in at all. |
| `apps/indicatrix-cut` | `gpu` | Routes the viewport's progressive accumulation and the high-resolution export worker through `indicatrix`'s GPU megakernel, falling back to the CPU tracer per frame whenever it declines. See [`apps/indicatrix-cut/docs/gpu.md`](apps/indicatrix-cut/docs/gpu.md). |
| `apps/indicatrix-worker` | `worker` | Render capacity: the coordinator's worker port and job execution on `serve`, `serve --render`, the `join` subcommand, and the `render` subcommand actually working. Turns on `indicatrix-net/render`. Without it, `indicatrix-worker` serves a read-only design library only. |
| `apps/indicatrix-worker` | `gpu` | Implies `worker`. Routes `render`/`serve --render`/`join` tracing through `indicatrix`'s GPU megakernel, falling back to the CPU tracer whenever it declines. |

## Screenshots

...

## The design library is yours to build

The SQLite design catalogue (`facet_diagrams.sqlite`) that `indicatrix-cut` and
`indicatrix-worker` open is **your own data, and it is not part of this
repository** — `.gitignore` excludes `*.sqlite`/`*.sqlite2`/`*.sqlite.bak`, so
nothing of the kind is tracked, shipped, or downloadable from here. There is
no starter catalogue bundled with this project. You build your own library
entirely by importing your own `.asc` cutting-instructions files, either through
`indicatrix-cut`'s Import button (see that app's README) or by calling
`indicatrix_vault::local::import_asc` directly.

## Documentation

Per-crate documentation is linked from the workspace map above — start with a
crate's own README, then its `docs/` folder if it has one:

- [`crates/indicatrix/docs/`](crates/indicatrix/docs/) — the physics's deliberate
  deviations and known simplifications, and the GPU equivalence harness.
- [`apps/indicatrix-cut/docs/`](apps/indicatrix-cut/docs/) — the settings file
  format, the preview-then-handoff remote-rendering model, import/export, and
  the `gpu` feature's fallback rules.
- [`apps/indicatrix-worker/docs/`](apps/indicatrix-worker/docs/) — the trust model
  behind its mutual-TLS server (viewer and worker roles), and its internal
  architecture, including the coordinator.

## License

MIT — see the `license` field in the workspace [`Cargo.toml`](Cargo.toml).
