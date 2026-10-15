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
| [`apps/indicatrix-mobile-android`](apps/indicatrix-mobile-android/README.md) | The Android phone and tablet app: a touch-first Slint UI over `indicatrix-mobile-core`, the JNI platform services (files, share, print) and a desktop preview of the same UI. Not in an app store yet; build and install it yourself. |
| [`crates/indicatrix-mobile-core`](crates/indicatrix-mobile-core/README.md) | The UI-free core of the mobile app: app state, settings, the library, the compute threads (render, solve, analysis), cutting progress and the platform-service trait. Shared by the Android app and any later iOS app; tested natively. |
| [`apps/indicatrix-worker`](apps/indicatrix-worker/README.md) | Headless render CLI and remote server — `serve` is a coordinator that serves a design library over mutual TLS and (on a `worker` build) spreads render requests over the machines that `join` it, rendering itself too with `--render`. |
| [`apps/indicatrix-cli`](apps/indicatrix-cli/README.md) | Headless command-line tool for designs, no GUI and no GPU: `info`, `solve`, `metrics`, `validate`, `optimize`, `retarget`, `sweep` and `export` on `.isdd`, `.asc`, `.gem` and `.gcs` files, with the editor's own engines, deterministic text, JSON and CSV output, and exit codes a script can branch on. |

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

### Designs from a script

`indicatrix-cli` runs the editor's engines on design files without opening a window.
It takes `.isdd` (and old `.indicatrix`), `.asc`, `.gem` and `.gcs` files, and its output is the same
bytes every time, so it can be diffed and checked in:

```
cargo run -p indicatrix-cli -- info round.asc
cargo run -p indicatrix-cli -- validate round.asc            # exit code 4 when the verdict is Problem
cargo run -p indicatrix-cli -- metrics round.asc --ri 1.76 --json
cargo run -p indicatrix-cli -- retarget round.asc --ri 1.7681 --crown-fraction 0.33 --out sapphire.isdd
cargo run -p indicatrix-cli -- sweep round.asc --tier "Pavilion Main" --from -43 --to -39 --step 0.5 --csv sweep.csv
cargo run -p indicatrix-cli -- export sapphire.isdd --format html --out sheet.html
```

`retarget` uses the Retarget dialog's engine and validity gate (by default the crown follows
the pavilion's stretch, so the stone keeps its silhouette; `--crown-fraction` and `--crown-ratio`
choose an older rule); a result the gate refuses exits with code 2, prints the reasons and
writes nothing. Custom materials come from a
design library with `--db facet_diagrams.sqlite` (opened read-only). Exit codes are 0
success, 1 usage, 2 design or solve, 3 file, 4 `validate` found a Problem. See
[`apps/indicatrix-cli`'s README](apps/indicatrix-cli/README.md) for every command and flag.

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

The desktop UI targets of `indicatrix-cut` also run only on command (`test = false`, so the
workspace commands above skip them):

```
cargo ui-harness                                                    # the UI harness: every flow, report in target/ui-harness/
cargo ui-static                                                     # static scan of the .slint sources
INDICATRIX_HARNESS_LIBRARY=facet_diagrams.sqlite cargo ui-screens   # rewrite the screenshots in assets/
cargo ui-clippy                                                     # clippy over the three targets
```

The aliases live in `.cargo/config.toml`. `ui-screens` draws with Slint's software renderer in a
throw-away profile and photographs a copy of the library you name; add `-- main cad` to retake
only some shots.

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
plus the `IsoHemisphere`/`LightTent`/`DaylightDome`/`DaylightSun`/`Aset` lit models) and a synthetic HDR
equirectangular map; the meet-point solver's three phases plus the verified repair
search (and through it the SIMD candidate-vertex/plane-intersection kernels) against
both synthetic AND real, catalogue-sourced `.asc` designs -- including the crate's own
205-facet-plane CrackOtto-Step fixture -- plus external solid measurement and
CAD-preview mesh extraction on the solved geometry; the `.asc` reader *and* writer,
B-Rep solid reconstruction; and the tilt-performance sweep. With the `gpu` feature (and
a real adapter present) it also trains the CPU-side GPU chunk dispatch/readback
orchestration. See that example's own doc comment for the exact stage-by-stage coverage
table and its `PGO_SCALE`/`PGO_TRAIN_SKIP_GPU` environment knobs.

For the `cut` target both scripts additionally train `indicatrix-cut-core`'s OWN
`pgo_train_cutcore` example -- the editor-core layer built on top of `indicatrix`:
`Design::solve`, `.asc` export, the `.isdd` design file save/load round trip,
`resolve_after_edit`, and `optimize_design`'s coordinate-search loop, run against every
built-in template plus the same CrackOtto-Step fixture. It is built in the SAME `cargo
build` invocation as everything else for that target (`indicatrix-cut-core` is in the
package list and both `--example pgo_train --example pgo_train_cutcore` are passed), so
the instrumented and optimised builds resolve an identical dependency graph and
`indicatrix-cut-core`'s crate hash matches. The two examples need distinct names because
Cargo's example-binary output-path uplift is keyed by name alone, not by package; an
earlier separate cut-core invocation had a different dependency graph and its profile
never applied. The `worker` target does not link cut-core and skips this step.

Both scripts also train a third binary, `indicatrix-net`'s `scene_roundtrip`
integration test, under the same instrumentation: `SceneState`'s postcard
wire-protocol encode/decode, the one hot path `pgo_train` cannot reach directly (a
`indicatrix` example can't depend on `indicatrix-net`, which itself depends on
`indicatrix`). Both scripts then rebuild both applications against the merged profile
and check that the profile applied: `indicatrix` for every target, plus
`indicatrix-cut-core` for the `cut` target (each crate's hash is compared separately).

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

By default (`--target all` / `-Target all`) both scripts build `indicatrix-worker` and
`indicatrix-cut` together: per combination ONE instrumented cargo invocation over both
apps, one training run, one merged profile and one optimised build that emits both
binaries. A single package set gives `indicatrix` one crate hash for both apps, so one
profile applies to both and the work is done once instead of twice; the trade-off is that
the worker is compiled with cut's (additive) features enabled, a somewhat larger binary
with unchanged behaviour. `--target worker` / `--target cut` still build one app on its
own, with its own profile.

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
| `apps/indicatrix-cut` | `physical-color` | Shows the physics-based color editor in the Gem Material Editor (the Fantasy/Physics toggle and the host-crystal and element-recipe section). UI only: a material's stored color mode and recipe are kept and rendered in every build, with or without this feature; without it the editor always shows the plain Fantasy colors. Not part of the shipped or PGO builds. |
| `apps/indicatrix-worker` | `worker` | Render capacity: the coordinator's worker port and job execution on `serve`, `serve --render`, the `join` subcommand, and the `render` subcommand actually working. Turns on `indicatrix-net/render`. Without it, `indicatrix-worker` serves a read-only design library only. |
| `apps/indicatrix-worker` | `gpu` | Implies `worker`. Routes `render`/`serve --render`/`join` tracing through `indicatrix`'s GPU megakernel, falling back to the CPU tracer whenever it declines. |

## Screenshots

All screenshots live in [`assets/`](assets/); click one for full size.

### Library and live render

[![The main window: design library on the left, the 3D spectral preview of the selected design on the right, with the material, lighting and metrics bar above it](assets/main.png)](assets/main.png)

| | |
|---|---|
| [<img src="assets/solid_rendering.png" alt="Solid view: the untextured stone for checking the geometry" width="400">](assets/solid_rendering.png)<br>Solid view, for checking the geometry | [<img src="assets/cutting_instructions.png" alt="Cutting Instructions tab: one row per step with facet, angle and index-gear positions" width="400">](assets/cutting_instructions.png)<br>Cutting instructions |
| [<img src="assets/tilt_performance.png" alt="Tilt Performance dialog: brilliance, windowing and extinction against tilt angle" width="400">](assets/tilt_performance.png)<br>Tilt performance curve | [<img src="assets/tilt_performance_export.png" alt="The tilt dialog with the tilt-video export options unfolded" width="400">](assets/tilt_performance_export.png)<br>Tilt video export |
| [<img src="assets/emerald_export_settings_4k.png" alt="Export High-Resolution Render dialog during a 4K export" width="400">](assets/emerald_export_settings_4k.png)<br>High-resolution export | [<img src="assets/cutting_mode.png" alt="Cutting Mode: one step of the cutting instructions per page with the index wheel and the step strip" width="400">](assets/cutting_mode.png)<br>Cutting mode |

| | | |
|---|---|---|
| [<img src="assets/parameters_stone.png" alt="Stone tab of the Parameters dock: material, size and color of the stone" width="260">](assets/parameters_stone.png)<br>Parameters: Stone | [<img src="assets/parameters_lighting.png" alt="Lighting and Environment tab: lighting rigs and environment map" width="260">](assets/parameters_lighting.png)<br>Parameters: Lighting & Environment | [<img src="assets/parameters_render.png" alt="Render and Output tab: resolution, samples and compute target" width="260">](assets/parameters_render.png)<br>Parameters: Render & Output |

### Design editor

| | |
|---|---|
| [<img src="assets/cad.png" alt="Edit mode: solid view with a crown tier selected, the tier table and the design panel" width="400">](assets/cad.png)<br>Edit mode | [<img src="assets/cad_solved_diagram.png" alt="Diagram view in Edit mode: crown, pavilion and profile panels on the index-gear scale" width="400">](assets/cad_solved_diagram.png)<br>Faceting diagram |
| [<img src="assets/diagram_view_detail.png" alt="The crown panel of the diagram view, every facet labelled with its tier code" width="400">](assets/diagram_view_detail.png)<br>Crown diagram ([full diagram view](assets/diagram_view.png)) | [<img src="assets/tilt_performance_cad.png" alt="The tilt dialog opened from the editor's view bar over the diagram view" width="400">](assets/tilt_performance_cad.png)<br>Tilt curve from the editor |
| [<img src="assets/retarget_dialog.png" alt="Retarget for Material dialog: before and after side by side, path traced" width="400">](assets/retarget_dialog.png)<br>Retarget for another material | [<img src="assets/retarget_dialog_1.png" alt="The Retarget dialog in overlay mode: the stone before (red) and after (green)" width="400">](assets/retarget_dialog_1.png)<br>Retarget overlay |
| [<img src="assets/retarget_gear_2.png" alt="Confirm Index Remap dialog after choosing a different index gear" width="400">](assets/retarget_gear_2.png)<br>Index-gear remap | [<img src="assets/retarget_merge.png" alt="A gear change from 96 to 64 teeth, before and after" width="400">](assets/retarget_merge.png)<br>Gear change, before and after |
| [<img src="assets/optimizer_1.png" alt="Optimize tab with the objective list open" width="400">](assets/optimizer_1.png)<br>Optimize: objectives | [<img src="assets/optimizer_2.png" alt="Optimize tab after a run: the start design and five candidates with their scores" width="400">](assets/optimizer_2.png)<br>Optimize: candidates |
| [<img src="assets/optimizer__compare_1.png" alt="Compare window in overlay mode: removed facets red, added facets green" width="400">](assets/optimizer__compare_1.png)<br>Compare, overlay | [<img src="assets/optimizer__compare_2.png" alt="Compare window side by side, path traced, with tilt-averaged metrics" width="400">](assets/optimizer__compare_2.png)<br>Compare, path traced |

### Dialogs and tools

| | |
|---|---|
| [<img src="assets/new_design_dialog.png" alt="New Design dialog: starting shape, gear and material" width="400">](assets/new_design_dialog.png)<br>New Design | [<img src="assets/material_editor.png" alt="Gem Material Editor: refractive index, dispersion, crystal class and color" width="400">](assets/material_editor.png)<br>Material editor |
| [<img src="assets/material_zones.png" alt="Material dialog with the color zones of the built-in Ametrine" width="400">](assets/material_zones.png)<br>Color zones (Ametrine) | [<img src="assets/tier_form.png" alt="Tier tab of the inspector with a crown tier selected" width="400">](assets/tier_form.png)<br>Tier form |
| [<img src="assets/concave_tier_form.png" alt="Tier tab on the blank concave tier form" width="400">](assets/concave_tier_form.png)<br>Concave tier | [<img src="assets/angle_sweep.png" alt="Angle Sweep dialog: table, chart and Use This Angle" width="400">](assets/angle_sweep.png)<br>Angle sweep |
| [<img src="assets/history_steps.png" alt="History tab: one step for each change" width="400">](assets/history_steps.png)<br>History | [<img src="assets/history_variants.png" alt="Variants view with two saved variants" width="400">](assets/history_variants.png)<br>Variants |
| [<img src="assets/command_palette.png" alt="Command palette searching for export" width="400">](assets/command_palette.png)<br>Command palette | [<img src="assets/preferences.png" alt="Preferences dialog" width="400">](assets/preferences.png)<br>Preferences |

### Guided build

| | | |
|---|---|---|
| [<img src="assets/tutorial_from_design.png" alt="Library card context menu with Build this design" width="260">](assets/tutorial_from_design.png)<br>Build this design | [<img src="assets/tutorial_from_design_2.png" alt="Guide panel, step 1: start a new design" width="260">](assets/tutorial_from_design_2.png)<br>Step 1 | [<img src="assets/tutorial_from_design_3.png" alt="Guide panel, step 2: add the girdle tier" width="260">](assets/tutorial_from_design_3.png)<br>Step 2 |

### Rough planner

| | |
|---|---|
| [<img src="assets/rough_planner.png" alt="Rough Planner before planning: a sapphire block with its edge cuts" width="400">](assets/rough_planner.png)<br>A block of rough | [<img src="assets/rough_planner_result.png" alt="The same rough after planning: the stones of the selected layout inside the block" width="400">](assets/rough_planner_result.png)<br>Planned layout |

### Remote rendering and the web app

[![A distributed export in flight: the Export High-Resolution Render dialog in the middle, surrounded by the GPU load of the local and remote machines that render it](assets/hybrid_remote_rendering.png)](assets/hybrid_remote_rendering.png)

| | |
|---|---|
| [<img src="assets/web_ui.png" alt="The web app's start screen in a desktop browser" width="400">](assets/web_ui.png)<br>Web app, desktop browser | [<img src="assets/web_ui_mobile.png" alt="The web app's start screen on a phone" width="200">](assets/web_ui_mobile.png)<br>Web app, phone |
| [<img src="assets/remote_coordinator.png" alt="Remote Coordinator dialog with an endpoint entered" width="400">](assets/remote_coordinator.png)<br>Remote coordinator | [<img src="assets/render_jobs.png" alt="Render Jobs window with two queued pictures" width="400">](assets/render_jobs.png)<br>Render jobs queue |

### Mobile app

Indicatrix also runs on Android phones and tablets. The app is not in an app store yet, but you
can build it and install it on your own device (a debug APK from Android Studio, or `gradlew
assembleDebug` plus `adb install`; see
[`apps/indicatrix-mobile-android/README.md`](apps/indicatrix-mobile-android/README.md)). The
same UI also runs on the desktop:
`cargo run -p indicatrix-mobile-android --features desktop-preview --bin indicatrix-mobile-preview -- --size phone`.

| | | | |
|---|---|---|---|
| [<img src="assets/mobile/phone_library.png" alt="The mobile library on a phone" width="200">](assets/mobile/phone_library.png)<br>Library | [<img src="assets/mobile/phone_design_render.png" alt="A design rendered on a phone" width="200">](assets/mobile/phone_design_render.png)<br>Render | [<img src="assets/mobile/phone_tiers_sheet.png" alt="The Tiers sheet over the solid view on a phone" width="200">](assets/mobile/phone_tiers_sheet.png)<br>Tiers | [<img src="assets/mobile/phone_cut.png" alt="The cutting instructions on a phone" width="200">](assets/mobile/phone_cut.png)<br>Cutting |

[![The mobile app on a tablet in landscape: the design rendered beside its panels](assets/mobile/tablet_design_render.png)](assets/mobile/tablet_design_render.png)

More phone and tablet shots are in [`assets/mobile/`](assets/mobile/); `cargo ui-screens-mobile`
rewrites them from the desktop preview, `cargo ui-screens` the desktop ones.

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
