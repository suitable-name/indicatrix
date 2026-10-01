# indicatrix-web

[![#MadeWithSlint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-light.svg#gh-light-mode-only)](https://slint.dev)
[![#MadeWithSlint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-dark.svg#gh-dark-mode-only)](https://slint.dev)

The browser build of Indicatrix: open or start a faceting design, edit it in the same
`EditorSession` the desktop app uses, look at it as a solid, a diagram and a rendered
stone, and save it back as a download. It is a Slint UI compiled to
`wasm32-unknown-unknown`, served as static files, with its heavy work (rendering and
the slow solves) in Web Workers. It has no design library, no remote worker, no Deep
Solve, no GPU path and no database; a design lives in the browser tab's session and
nowhere else.

## What works

- **Shell.** A menu bar (File, Edit, Design, Help), a header with the render material
  and lighting choices and the Render / Solid / Diagram tabs, a right-hand Design dock,
  and a status strip (design name, unsaved marker, solve status, last message). The
  layout follows the browser window; the dock is resizable (drag its left edge, and the
  bar above the inspector), and below 900 px it becomes an overlay opened by a "Design"
  button at the main view's bottom-right corner. Every pill and icon button has a
  tooltip; Help has a short **User guide** and the **Keyboard shortcuts** list (also `?`).
- **Tier table and editing (Design dock).** The desktop's tier table with its columns
  (angle, name, indices, meets, mast, solve strategy, margin, orbit, Adopt / Pin /
  Detach / Add Anchor, move, duplicate, delete), manufacturability and unsolved-tier
  tints, a filter box, and a status panel (solving / solved / stale / cannot be solved /
  not a closed solid, plus the warnings, each click-through to its tier). Selecting a row
  selects the tier in the Solid and Diagram views and back; Ctrl / Shift-click
  multi-select. Inline angle edit (double-click, Enter / Esc, F2), Ctrl+wheel nudge,
  arrow-key navigation, Delete, Ctrl+D, Alt+Up/Down. The command bar has Undo, Redo,
  Solve, the auto-solve budget (Off, 150 ms, 300 ms, 1 s, 3 s -- the desktop's five
  steps, kept for the tab session), "+ Add Tier", Quick add, Complete orbit, Mirror
  indices and Delete; the table's toolbar has Adopt all, the step-ladder generator and
  "mirror to the other block". All edits go through `EditorSession`.
- **Inspector (Tier / Preform / Optimize / Schedule tabs).** "+ Add Tier" opens the Tier
  tab in Add mode with the cursor in Angle; a row's "Add Anchor" opens it on that tier
  with "Exact scale value" chosen and the cursor in the value. The Tier tab has the
  per-tier form with its dirty-draft guard and live critical-angle bar, the per-facet
  chips (detach / remove / add / rotate / mirror), cheater offset and note, and the
  read-only Solved section. The Preform tab has the starting rough, the proportion
  verdicts and the yield inputs and results; Schedule is the cutting instructions.
- **Design settings** (Design menu, or the button in the dock): material, RI override and
  colour, index gear (with the remap confirmation), symmetry and mirror, title / header /
  footnotes, printed proportions, and a compact custom-material editor (name, RI,
  dispersion, birefringence, specific gravity, colour). A custom material is offered in
  the material lists and kept with the design's native file (its snapshot: optics and
  body colour; a built-in material's name is refused, so a custom material never shadows
  one). The Render tab's "Linked to design" switch is the one control for whether the
  render follows the design's material.
- **Optimize and Retarget.** Optimize's coordinate search and Retarget's Optimize mode run
  in the solve Worker with live progress. Cancel is cooperative: the Worker looks at a
  revocable `blob:` URL between tier decisions, stops, and the best result so far is
  shown and can be applied (the desktop's behaviour). The Worker needs a few seconds to
  finish (one more evaluation, then the two full-fidelity scores), during which the button
  reads "Stop now"; pressing it, or a search that has not stopped after 30 s, terminates
  the Worker and keeps no partial result. A result computed for an older
  design stays on screen badged "Stale: design changed" with a Recompute.
  Snapshot design / Compare to snapshot lists what changed tier by tier.
- **Optical metrics and the Tilt Curve dialog.** The Render tab's HUD shows the desktop's
  five metrics (Brilliance, Fire, Scintillation, Windowing, Extinction) for the pose and
  light on screen, dimmed while a new pose is being measured. The Tilt Curve pill (Render
  and Solid tabs) opens the Tilt Performance dialog: brilliance, windowing and extinction
  against tilt from table-up, four azimuths (0 / 45 / 90 / 135 degrees) of 181 points,
  with Run / Cancel, progress, the axis and channel switches and the hover readout. Both
  run the desktop's own functions in a second, independent Worker (the analysis Worker,
  created when the Render tab first measures), so they never wait behind or preempt a
  design solve; the HUD waits while a sweep runs. The dialog omits the desktop's hover
  preview thumbnail, the data-table copy and the tilt video export. As on the desktop, the
  HUD is scored under the radiance the viewport is lit with: with an HDR map loaded the
  analysis Worker holds the same map and the HUD says "Scored under the HDR map" (or, when
  the memory budget left the analysis Worker no copy, names the preset it fell back to and
  that the map is not loaded). The tilt sweep stays under the lighting preset's rig, as the
  desktop's dialog does, and its dialog says so while a map is loaded.
- **Solve.** Small designs solve on the page, everything else in the solve Worker, with the
  desktop's own rules (`solve_policy`). The result is cached per design generation. F5
  solves (the page does not reload).
- **Solid and Diagram tabs.** The desktop's solid viewport in its Solid and Diagram modes,
  drawn by `indicatrix_solid::preview` (the same planner and rasterizer the desktop runs).
  Orbit and zoom, the Front / Top / Bottom / Left / Right / Fit / Reset camera pills, hover
  and click selection, the multi-selection outline and the Preform toggle; the Diagram tab
  shows the Crown, Pavilion and Profile panels; in a view narrower than 800 px it opens on
  the Crown panel alone (three panels' facet labels cannot be read there), and clicking the
  highlighted panel button again brings back all three.
- **Mouse manipulation in the Solid tab.** With a facet or tier selected, three handles
  fan out from the facet (A = angle, D = depth, I = index); drag one to tilt the tier,
  move it in or out (pinning its mast) or turn it around the index wheel. Shift snaps
  finely, the Snap pill turns snapping off, Escape cancels, and one drag is one undo
  step. The tiers that follow a drag are outlined, and the design auto-solves after
  release. The Slice pill (or S) cuts a new facet: drag a line across the stone, adjust
  the provisional tier with the same handles, then Keep (Enter) or Discard (Esc); Flip
  (F) cuts the other side. The math and wording are `indicatrix_editor::manipulate`,
  shared with the desktop.
- **Render tab.** A live spectral render on CPU Web Workers (one per core, up to eight),
  progressive with an optional denoise once settled (made by a Worker of its own, so the
  tracing never pauses for it), using the design's solved planes. A render Worker that
  crashes or goes silent is replaced and the render restarts; a solve that keeps failing
  is tried three times with a growing pause and then shown as an error until the design
  changes.
  The settings panel has exposure, light direction, backdrop, bounces, target samples,
  frosted girdle, inclusion haze, edge rounding, stone size and crystal-axis override,
  and an uploaded `.hdr` environment map. A view narrower than 16:10 is traced at 16:10
  and letterboxed, so the whole stone stays in the frame on a phone-width window.
  File > Export PNG renders a still at a chosen size and colour space (sRGB or
  Display P3, with an embedded profile), at the framing the view shows.
- **Guided walkthrough** (Help > Guided walkthrough, or the link on the empty state). The
  desktop's ten-step worked example, the same steps (`indicatrix_editor::guide`) in a
  panel that docks beside the Design dock in a wide window and floats (draggable,
  collapsible) elsewhere; the few lines that name a desktop-only place are reworded
  (`indicatrix_web_core::guide::WEB_WORDING`). While a step is active, every control outside
  the step's groups is dimmed and says why in its tooltip (the desktop's `GuideAllow`
  rules: New Design, tier form, tier table, design settings, Solve, Preform tab, Optimize /
  Retarget / Snapshot, the Render tab, file operations, Undo / Redo; Ctrl+Z / Y / O / S
  follow their menu items). Action steps complete by themselves once the design reaches
  their goal, whichever route led there; reading steps wait for Next. Closing the guide
  lifts every lock. Its place (open, step, collapsed, panel position) survives a reload.
- **New Design.** The template gallery (Empty plus the five built-in templates) inside the
  desktop's New Design dialog: preform, index gear, symmetry, mirror and starting material.
- **Opening files.** File > Open (several files at once) or drag and drop onto the page.

  | File | How it is read |
  |---|---|
  | `.asc` | `decode_asc_bytes` (Windows-1252 aware), then `indicatrix_editor::loading::design_from_asc_text` |
  | `.asc` + `.indicatrix.toml` | `indicatrix_cut_core::native::load_paired` (open or drop both files together) |
  | `.indicatrix.toml` alone | `indicatrix_cut_core::native::load_native_only` (self-contained native files) |
  | `.gem`, `.gcs` | `indicatrix_editor::files::convert_foreign_design`, then the `.asc` path |
  | `.hdr` | header checked against the browser caps (64 MiB, 8192 x 4096), then kept for rendering |

  A load error is shown as a message; it never stops the page. A file is checked against
  its size limit (16 MiB for a design, 64 MiB for an `.hdr`) from the browser's file size
  before any of it is read. Opening or starting a new design over unsaved changes asks
  first, in a Save / Discard / Cancel dialog (Save downloads the native pair, then
  continues); closing or reloading the tab with unsaved changes asks the browser's own
  leave-the-page question.
- **Saving (browser downloads).** Save `.asc`, Save native pair
  (`.asc` + `.indicatrix.toml`), Save native only (`.indicatrix.toml`), Download cutting
  sheet (HTML), Download diagram PNG and Export PNG. Each one uses the desktop's own
  function, so the bytes match what the desktop would write. File names follow the
  desktop's naming.
- **Session memory.** Settings (including the auto-solve budget), the current design and
  the guide's place are saved to `sessionStorage` about half a second after each change,
  under the keys `indicatrix.settings.v1` (JSON), `indicatrix.design.v1` (a
  self-contained native `.indicatrix.toml`) and `indicatrix.guide.v1` (JSON). They are
  restored after a reload of the same tab and are gone when the tab closes. A settings
  entry from an older build still loads. Where `sessionStorage` is unavailable (some
  private windows) the app runs the same and simply starts empty after a reload; a
  console note says storage could not be used.
- **Diagnostics:** Rust panics and `tracing` events from the shared crates appear in the
  browser console.

## Keyboard shortcuts

`?` (or Help > Keyboard shortcuts) lists them in the app. The desktop's set is kept where a
browser allows it; browser-reserved combinations are replaced.

| Keys | Action |
|---|---|
| Ctrl+O / Ctrl+S / Ctrl+Shift+S | Open / Save native pair / Save `.asc` |
| Alt+N | New Design (the desktop's Ctrl+N would open a new browser window) |
| Ctrl+Z, Ctrl+Y or Ctrl+Shift+Z | Undo, Redo (left to a text field while one has focus) |
| F5 | Solve (Slint's web backend cancels the browser's reload on it) |
| Alt+1 / Alt+2 / Alt+3 | Render / Solid / Diagram tab (the desktop's Ctrl+1..3 switch browser tabs) |
| Esc | Clear the tier selection and dismiss the message |
| ? | The shortcut list |
| Tier list: Up / Down / Home / End / Page Up / Page Down, Enter, F2, Delete, Ctrl+D, Alt+Up / Alt+Down | as on the desktop |
| Solid tab: S, Enter, F, Esc, Shift | Slice tool, keep, flip, cancel, fine snapping |

Not mapped: Ctrl+W, Ctrl+T (the browser owns them), Ctrl+E and Ctrl+F (the desktop's
Live Render toggle and search box have no web counterpart; Ctrl+F stays the browser's find).
On macOS, Option+letter types a symbol, so Alt+N and Alt+1..3 do not work there; use the
File menu and the tabs.

## Not in the web app

- **The design library** (catalogue, search, mirror), **remote rendering and the
  coordinator**, **Deep Solve**, **the GPU renderer** (the megakernel's pipeline compile
  takes Chrome's GPU process down; rendering is CPU-only for now), and **the database**.
- **Batch export, the tilt video and GIF export** and the vault's custom-material store.
- **Optimize and Retarget ghost preview in the viewport, and the before/after Compare
  window.** The results are listed in the inspector and the Retarget dialog; the stone in
  the Solid tab is not redrawn with the pending angles.
- **The full custom-material editor** (crystal-system and optical-character pickers,
  biaxial data, base templates). A design carries the snapshot of one custom material
  (the last one created or opened) with its optics and body colour; a file written before
  the colour was stored restores the material colourless.
- **The Preform tab's "Yield Material" picker.** The desktop removed it too (its index was
  never read); only the girdle diameter and a specific-gravity override are typed, and the
  material for the carat estimate follows Design settings.

## Limits

- **Rendering is on the CPU**: `clamp(cores - 1, 1, 8)` Workers (one when the browser
  reports no core count). A few hundred samples per pixel take tens of seconds to
  minutes depending on the machine and the view size; the live view traces at most
  960 px on an edge and a Retina-class scale factor counts at most 1.5x.
- **Live samples** 64 to 1024 (default 256); **export** 16 to 4096 samples and up to
  4096 px on the long edge; bounces 4 to 24.
- **Environment maps**: an `.hdr` up to 64 MiB and 8192 x 4096 texels; each render
  Worker and the analysis Worker hold their own decoded copy, so a large map is admitted on
  fewer render Workers (with a message) rather than downsampled. A map so large that the
  budget holds a single copy is kept by the render Worker; the metrics HUD then stays under
  the lighting preset and says so.
- **Memory**: the HDR budget is 1.5 GiB across the render Workers and the analysis Worker;
  a design and its history live in the page's wasm memory (4 GiB address space).
- **Persistence**: this tab's `sessionStorage` only (a few MB); nothing survives closing the
  tab, and nothing is ever uploaded.
- **Solves** that run more than 60 s in the solve Worker are stopped.

## Browser support

A current Chrome, Edge, Firefox or Safari (Chrome 96, Firefox 100, Safari 15.2 or newer): the
wasm needs bulk memory, sign extension, reference types and multivalue, and the page uses
Web Workers, WebGL2 (Slint's renderer), `sessionStorage` (optional) and Blob downloads. No
WebGPU, no `SharedArrayBuffer` and no special response headers (COOP / COEP) are needed, so
any static host works. Touch input arrives as mouse events, so a tablet works for
viewing and tapping but not for Ctrl / Shift gestures or the keyboard shortcuts.

## Building and serving

You need [`trunk`](https://trunkrs.dev) and the wasm target:

```sh
cargo install --locked trunk
rustup target add wasm32-unknown-unknown
```

Then, from this directory (`apps/indicatrix-web`):

```sh
trunk serve            # dev server with live rebuild, http://127.0.0.1:8080
trunk build            # a dev dist/ (optimised, not minimal)
```

A native `cargo check`/`clippy` of this crate compiles to nothing on purpose (see
`src/lib.rs`). To check the real code:

```sh
cargo clippy --target wasm32-unknown-unknown -p indicatrix-web -p indicatrix-web-compute
cargo test -p indicatrix-web-core
```

**Dev builds.** The Worker (`../indicatrix-web-compute`) is built by the same `trunk` run
with the workspace's `[profile.indicatrix-web-compute]` (opt-level 3, no debug info),
because an unoptimised Worker traces far too slowly to be usable; the page uses the same
profile, with only its own crate at opt-level 1 to keep rebuilds short (its hot loops live
in dependencies that stay at opt-level 3). Measured on the development machine: an edit to
a Rust file rebuilds the page in about 40 s, an edit to a `.slint` file in 3 to 6 minutes
(the generated UI code is recompiled whole; opt-level 0 would cut that to about 2 minutes
but makes the Diagram frame too slow), and a Solid frame takes 4 to 8 ms, a Diagram frame
8 to 23 ms. The dev `dist/` is large (the page's wasm is about 25 MB, 5 MB gzipped; the
Worker's 1.5 MB) because dev builds run no `wasm-opt`.

**Release build** (run by the owner, not by this repository's tooling):

```sh
cd apps/indicatrix-web
trunk build --release
```

It uses `[profile.indicatrix-web-release]` (`[profile.release]` plus opt-level `s` for the
page crate only) and runs `wasm-opt` (Trunk downloads it on first use): level `s` on the
page module, `3` on the Worker, with the wasm features rustc emits enabled by
`data-wasm-opt-params` on the two `<link>`s in `index.html`. If `wasm-opt` ever fails on the
toolchain's output, set `data-wasm-opt="0"` on that `<link>`. `no_sri = true` in `Trunk.toml`
keeps the page working behind a CDN that rewrites scripts.

**Post-build check** (`dist/`):

1. Files: `index.html`, one hashed `indicatrix-web-<hash>.js` and `_bg.wasm`, and the Worker's
   `indicatrix-web-compute.js`, `indicatrix-web-compute_bg.wasm` and
   `indicatrix-web-compute_loader.js` (unhashed; the page loads the loader by that name).
   Record the byte sizes of the two `.wasm` files and their gzip or brotli sizes; a release
   page wasm is expected to be a fraction of the dev one and the Worker about the size of
   the dev Worker or smaller. If either is larger than the dev build's, something is wrong.
2. Serve it (`python -m http.server -d dist 8080`, or the real host) and check that the host
   sends `.wasm` as `application/wasm` and compresses `.wasm` and `.js`.
3. Smoke test in Chrome and one other browser, with the console open (a single winit
   "Using exceptions for control flow" notice is expected):
   New Design (Standard Round Brilliant) -> the render starts and samples climb ->
   Solid tab, drag the angle handle -> Diagram tab, click a panel -> File > Save native pair
   downloads two files -> reload the tab, the design comes back -> Ctrl+Z / F5 / `?` work ->
   Tilt Curve runs and Cancel keeps the curves -> File > Export PNG at 256 px -> narrow
   the window to phone width and check the File menu and the dialogs still fit.

The console always shows one "Using exceptions for control flow" notice from
winit at start-up. That is expected on every Slint web app, not an error.

## Layout

| Path | Contents |
|---|---|
| `ui/app.slint` | window, menu bar, shortcuts, main area + dock + guide column, status strip, overlays; the export block |
| `ui/models/` | the `AppModel`, `GalleryModel` and `GuideModel` globals |
| `ui/components/` | header, status strip, toast, empty state, template gallery, render panel and export dialog, metrics HUD and Tilt dialog (`metrics_*`, `tilt_*`), guide panel and lock tip, the user guide and shortcut overlays, stale badge, theme |
| `ui/views/` | the Render, Solid and Diagram views |
| `ui/editor/` | the Design dock: command bar, status, quick-add, tier table (`tier_table*.slint`), the inspector and its tabs, design settings, New Design, Retarget, Snapshot, Optimize |
| `src/app/` | the `WebApp` state, entry point, callback wiring, `push_*` UI refresh, persistence, solve, diagnostics |
| `src/editor/` | the tier table's data (`table`), selection, editing actions (`edit*`), nudge, orbit tools, the unsaved-changes dialog, the inspector, design settings, New Design, Optimize, Retarget, snapshot and the guide (`guide`) |
| `src/views/` | the Solid and Diagram tabs, including the mouse manipulation (`manip`) |
| `src/render/` | the live render loop, its settings panel, PNG export and viewport-size tracking |
| `src/metrics/` | the metrics HUD's scheduling (`hud`) and the Tilt dialog's sweep (`tilt`), both in the analysis Worker |
| `src/io/` | picker, drag-and-drop, per-kind loaders, `.hdr` check, downloads |
| `src/workers/` | the lazily created Worker pool (render Workers and the solve Worker when a design first shows on the Render tab or a solve needs them; the analysis Worker on first use) |

The logic that can be tested natively lives in `crates/indicatrix-web-core` (the Worker
protocol and handlers, chunk planning, settings and guide persistence payloads, the
custom-material form) and in the shared `indicatrix-editor`.
