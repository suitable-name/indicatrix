# indicatrix-cut

[![#MadeWithSlint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-light.svg#gh-light-mode-only)](https://slint.dev)
[![#MadeWithSlint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-dark.svg#gh-dark-mode-only)](https://slint.dev)

The desktop faceting-design editor: browse, search, and render your own
faceting-design library with `indicatrix`'s full spectral renderer, and edit
cutting instructions with material retargeting and a solid inspection view.
Import and export your own `.asc` files.

Built with [Slint](https://slint.dev/). This crate is both a library
(`indicatrix_cut`) and a binary (`indicatrix-cut`) — the library exposes
`gui::build_main_window()`, a fully-wired, not-yet-shown `MainWindow`.

## Documentation map

This file is the entry point: building, what the app does, and the code
structure. Deeper material lives alongside it in `docs/`:

- [`docs/settings.md`](docs/settings.md) — the settings file's location,
  persistence guarantees, render-quality controls, and the remote endpoint
  (coordinator) configuration.
- [`docs/remote-rendering.md`](docs/remote-rendering.md) — the
  preview-then-handoff model: how the live view shares its sample budget with the
  remote coordinator, full-data vs. final-picture transfer, HDR scenes, and how
  denoising and the TLS connection are handled across that.
- [`docs/export.md`](docs/export.md) — how `.asc` import and export work.
- [`docs/gpu.md`](docs/gpu.md) — the `gpu` feature and exactly when and why a
  frame falls back from GPU to the CPU tracer.
- [`docs/manual/README.md`](docs/manual/README.md) — the user manual, for
  lapidaries using the app rather than developers working on it.

## Build and run

```
cargo build -p indicatrix-cut
cargo run -p indicatrix-cut
```

`build.rs` runs `slint_build::compile("ui/app.slint")` at build time, which pulls
in `theme.slint`, `types.slint`, and everything under `ui/components/`. No
platform-specific setup beyond what Slint itself needs (a Windows graphics
backend) was found in this crate's `Cargo.toml`.

**Working directory matters.** The local catalog database is opened as a
relative path (`facet_diagrams.sqlite` in the process's current working
directory, not a fixed config directory), so running the built `.exe` from a
different folder than expected opens (or creates) a *different* database.
Exports are unaffected by this — every export asks where to save via a native
dialog, and `./exports/` is only the suggested default.

### Icon and console behaviour

`build.rs` embeds `assets/icon.ico` as a Windows resource, which covers both Explorer's
view of the `.exe` and the window's own titlebar/taskbar icon — a Win32 window with no
icon of its own falls back to the executable's first icon resource, so nothing needs to
ship alongside the binary. Regenerate the icon with:

```
python scripts/make-icons.py
```

A **release** build sets `windows_subsystem = "windows"`, so launching the `.exe`
directly no longer opens an empty console beside the window. A **debug** build keeps its
console, so `cargo run` still shows panics during development. If a `tracing` subscriber
is ever added to this crate, it must write somewhere other than stdout — a release build
has no console to write to.

### The `gpu` feature

```
cargo build -p indicatrix-cut --features gpu
```

Routes both the viewport's progressive accumulation *and* the high-resolution
export worker through `indicatrix`'s verified GPU megakernel instead of the
multithreaded CPU tracer. **Off by default.** Measured on this project's
integrated AMD Radeon (Vulkan), at 960x540 / 64 spp / 12 bounces on Emerald:
**1.44 s on GPU vs 10.66 s across 16 CPU threads, a 7.4x speedup**. The
fallback to the CPU tracer is per frame (per batch, for an export), not per
session, and is a normal outcome, not an error — see
[`docs/gpu.md`](docs/gpu.md) for exactly when and why it happens.

## What it does

- Search and filter your library by title/designer, shape, gear, and range
  filters on refractive index, L/W ratio, volume, and facet count.
- View a diagram in 3D with `indicatrix`'s full spectral path tracer — orbit camera,
  adjustable lighting/exposure/material/quality, live optical-metrics readouts
  (brilliance/fire/scintillation/windowing/extinction) and a tilt-performance
  analysis graph — 19 measured tilt angles swept at four camera azimuths (0°,
  45°, 90°, 135°), switchable or overlaid, with a hover readout that
  interpolates between the measured points.
- View the cutting-instructions table (facet/angle/index/notes) and any attached
  original files.
- Edit tab: drag the angle, depth, or index handle on a selected facet in the
  Solid viewport (Solid, Path-traced, or Both view) to tilt, move, or turn its
  whole tier live, with snapping (Shift for fine steps, a Snap pill to turn it
  off), the tiers that follow outlined before you let go, and one undo step per
  drag — see Chapter 13 of the manual.
- Import your own `.asc` file(s) — a single file, or a folder, optionally
  including its subfolders — into the local library. Imports run off the UI
  thread with progress, and geometry-derived metadata (proportions, and a
  conservatively classified shape) is measured at import rather than left blank.
  Export a diagram back out as `.asc` — either its
  original attached file, byte-for-byte, or reconstructed from the stored
  angle/index table if no original is attached (a reconstructed file is
  explicitly marked as such — see `indicatrix-formats`'s and `indicatrix-vault`'s READMEs
  on `mark_reconstructed`).
- Rename/delete library entries.
- Export a high-resolution still — 1080p, 4K, or a custom size up to 8192×8192,
  at up to 32768 samples per pixel, in sRGB / Display P3 / Rec.2020 (the two
  wide-gamut choices carry an embedded ICC profile) — with a live thumbnail of
  the render as it progresses.
- Every path the app asks for — import source, environment map, certificate
  folder, and each export destination — is chosen through a native OS file
  dialog; the text fields remain, so a pasted path still works.
- Offload rendering to a remote `indicatrix-worker serve` coordinator (which can
  spread it over the render workers that join it) over mutual TLS, with a
  preview-then-handoff model while the camera is moving — see
  [`docs/remote-rendering.md`](docs/remote-rendering.md).

## Structure

```
src/
  lib.rs            slint::include_modules!(), pub mod gui, crate-private bridge/settings
  main.rs           entry point + the release-build windows_subsystem attribute
  gui/              callback wiring, one module per area of the UI
    mod.rs            module list; main_window.rs builds and wires the MainWindow
    library/          browse/search/filter, detail loading, import / rename / delete /
                      export-.asc, remote library switch and mirror
    editor/           the Edit sub-tab: tier list, solve, Deep Solve/Optimize/Retarget,
                      the worked-example guide, templates, native files
    render/           camera/lighting, materials/quality, the high-resolution export's
                      UI side (render_export/)
    remote/           the Remote Coordinator form, Test connection, enrollment, and
                      the preview-then-handoff orchestrator (orchestrator/)
    batch/            catalogue-wide preview and tilt-curve batches
    tilt/             tilt profile, hover preview, tilt video export
    solid_preview/    the solid inspection view
    optics/           crystal optics, custom materials, tilt-curve paths
  bridge/           everything off the UI thread, plus pure logic it depends on
    render_thread/    the local progressive render loop, RenderContext, denoise/display
    export_thread/    the high-resolution export worker (local + remote engines)
    remote/           enrollment, the mutual-TLS connection driver (remote_render/),
                      the pure handoff state machine (handoff.rs), the live remote
                      lane, the HDR/remote guard, HDR maps as protocol assets
    sample_cursor/    the shared sample cursor (from indicatrix-dispatch) + live epoch
    frame_cache/      per-geometry caches: remote-frame denoise guides (prepass:
                      indicatrix::renderer::guide_pass), girdle finish, stone width
    library/          library protocol client, local/remote source, pull mirror
    pixel_buffer.rs   zero-copy RGBA8 -> SharedPixelBuffer transfer
    preview_render.rs cached catalogue-preview rendering
  settings/
    model/            pure data/logic (no Slint or threading dependency)
    store.rs          on-disk TOML load/save, infallible on failure
    persist.rs        debounced background writer
assets/
  icon.ico, icon.png  generated by scripts/make-icons.py
ui/
  app.slint, theme.slint, types.slint, components/*.slint, icons/*.svg
```

`MainWindow` (`ui/app.slint`) layout: a top toolbar (search, shape/gear filters,
range sliders, import, the Remote Coordinator panel with its denoise toggle), a
diagram-list panel on the left, and on the right a detail header plus three
tabs — the 3D viewport, the cutting-instructions table, and attachments.

## Testing

```
cargo test -p indicatrix-cut
```

No `tests/` directory — all coverage is inline `#[cfg(test)]`. What's covered:
the `HandoffMachine` state machine exhaustively (including an all-state-pairs
no-panic sweep), settings TOML round trips (including the remote endpoint, the
migration from the old worker list, and `PreviewScale::Custom`), the debounced
settings writer, the HDR/remote guard, the live remote lane, tonemap/denoise
correctness and the "denoise is a no-op at converged sample counts" property,
quality-preset label/index round trips, and filename sanitization. **Nothing
exercises the actual Slint window, a live socket to a `indicatrix-worker`, or the
SQLite-backed end-to-end flow** — per the source's own doc comments, those are
left to manual verification.
