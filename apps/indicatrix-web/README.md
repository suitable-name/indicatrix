# indicatrix-web

[![#MadeWithSlint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-light.svg#gh-light-mode-only)](https://slint.dev)
[![#MadeWithSlint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-dark.svg#gh-dark-mode-only)](https://slint.dev)

A browser build of `indicatrix`: upload a GemCAD `.asc` cutting schedule, get back an
interactive rendered stone. No design library, no remote worker, no database -- that
scoping is the whole reason this is tractable at all. Dropping the design library sheds
`rusqlite`/SQLite, HTTP, and TCP entirely; WebGPU consumes the same WGSL the native GPU
path already ships.

## Building and serving it

This crate is not built by the rest of this workspace's own tooling (`cargo
build`/`check`/`clippy --workspace` skip its actual work entirely on a native host --
see `src/lib.rs`'s crate doc comment). You need
[`trunk`](https://trunkrs.dev) installed yourself:

```sh
cargo install --locked trunk
```

Then, from this directory (`apps/indicatrix-web`):

```sh
trunk serve            # dev server with live rebuild, http://127.0.0.1:8080
trunk build --release  # a deployable dist/ you can host anywhere static files work
```

`wasm32-unknown-unknown` must already be installed as a Rust target
(`rustup target add wasm32-unknown-unknown`); nothing else beyond `trunk` itself.

## This build is WebGPU-only -- there is no CPU fallback

This is the single biggest way this crate differs from every other `indicatrix` consumer
in this workspace. `apps/indicatrix-cut` and `apps/indicatrix-worker` both go through
`indicatrix::renderer::gpu_backend::GpuBackend`, whose whole contract is "try the GPU,
silently fall back to the CPU tracer whenever it declines." `indicatrix-web` calls
`indicatrix::renderer::gpu::frame::GpuFrameRenderer` directly instead (see
`src/render.rs`'s module doc comment for the full reasoning) and has **no** fallback of
its own, for two compounding reasons:

- `wasm32-unknown-unknown` has no OS thread. `indicatrix`'s CPU tracers (the ones
  `apps/indicatrix-cut` and `apps/indicatrix-worker` use, and the pattern
  `apps/indicatrix-worker/src/render_core/mod.rs::trace_into` documents) parallelize with
  `std::thread::scope`, which has no thread to spawn here -- real multi-threading in a
  browser tab needs `SharedArrayBuffer` plus cross-origin-isolation response headers
  (`Cross-Origin-Opener-Policy`/`Cross-Origin-Embedder-Policy`) plus
  `wasm-bindgen-rayon`. This crate deliberately does not take that on: it is a real
  amount of infrastructure (server-side header configuration this crate doesn't
  control, since it's served as static files) for a browser demo, not a decision to
  make silently inside a rendering crate.
- Even granting a single-threaded software ray tracer, it would be slow enough in a
  browser tab to be a worse experience than a clear "this browser can't run
  `indicatrix-web`" message. So there isn't one, not even a slow one.

**What this means in practice:** on a browser or machine with no usable WebGPU (an
older browser, WebGPU disabled behind a flag, or a machine whose driver stack the
browser declines to expose), `indicatrix-web` shows a first-class error explaining exactly
that, naming what to check (a recent Chrome, Edge, or Firefox build, or enabling
WebGPU) -- never a blank canvas, a silent stall, or something that could be mistaken
for a hang. See `src/render.rs`'s `RenderError` and `src/app.rs`'s `render_loop` for
where that message is produced and shown.

The same "no fallback" reasoning is also why **this crate offers no HDR
environment-map picker**, even though `indicatrix` supports one. An HDR environment is the
one scene ingredient the GPU megakernel has no `env_mode` for
(`GpuFrameError::UnsupportedEnvironment`); on every other target that's a routine
decline the CPU path picks up silently, but here it would just be a control that can
only ever fail. Material, by contrast, is not a limitation on any target any more:
`GemMaterial::gpu_supported()` has been unconditionally `true` since the biaxial WGSL
port, so Alexandrite/Topaz/Tanzanite-class materials would render here exactly as
well as Diamond/Ruby/Sapphire/Emerald -- this crate's combo box just doesn't happen to
list them (see `src/scene.rs::material_for_index`), to keep the control surface small.

## Progressive accumulation

A render is not one shot at a fixed sample count -- it accumulates toward
`src/render.rs::TARGET_SPP` (256 samples/pixel, matching `apps/indicatrix-cut`'s own
desktop default exactly) in `src/render.rs::CHUNK_SPP`-sized steps, presenting the
tone-mapped partial image after every chunk (`src/app.rs::render_loop`) so the stone
visibly refines rather than staying blank or frozen until the last sample lands.
`ui/app.slint`'s progress badge ("128 / 256 spp") says exactly how far along the
current pass is.

This leans on `GpuFrameRenderer::accumulate_async`'s sample-range additivity
contract (see `indicatrix::renderer::gpu_backend::GpuBackend`'s "Sample-range
additivity" doc section, and `src/render.rs::Accumulator`'s own doc comment): each
chunk is dispatched starting at exactly the sample count already folded in, never
`0`, so successive chunks extend the estimate instead of biasing it. The one place
this matters for correctness, not just visuals: **a camera move, a material or
lighting change, or a newly loaded file always resets the accumulator to zero before
the next chunk** (`src/render.rs::Accumulator::reset`, called from
`src/app.rs::render_loop`) -- continuing to accumulate into a buffer whose earlier
samples were traced against a different scene would silently blend two physically
different images together.

`src/render.rs::CHUNK_SPP` is currently a reasoned default (`8`), not a
hardware-measured one -- see that constant's own doc comment for exactly why the
live measurement this was supposed to be based on could not be completed in the
session that built this (the available browser-automation tooling had no
functioning WebGPU adapter to measure against) and for the byte-budget math that
default IS grounded in. `accumulate_chunk` already logs each real chunk's wall-clock
time to the devtools console (`"indicatrix-web: chunk: N spp in Xms (...)"`) on whatever
machine actually runs this, so retuning it with a real measurement is a one-line
change once someone has a browser with a real adapter.

## What this deliberately omits, and why

- **No design library.** No catalogue browsing, no search, no saved designs. Loading a
  design library means `indicatrix-vault`'s `rusqlite`/SQLite, and
  `libsqlite3-sys`/SQLite's C source cannot target `wasm32-unknown-unknown` at all --
  this is the hardest blocker in the brief this crate was built against, and the
  reason "no library" isn't a UI simplification, it's the thing that makes a
  browser build possible in the first place. You upload one `.asc` file per session;
  there is nowhere to save it back to.
- **No remote worker.** `indicatrix-net`'s wire protocol runs over mutual-TLS `TcpStream`s
  (`rustls`); raw TCP sockets don't exist in a browser sandbox (`WebSocket`/`WebTransport`
  are the browser-native analogues, and `indicatrix-net` speaks neither). This crate's
  `Cargo.toml` depends on `indicatrix` and `indicatrix-formats` and nothing else from this workspace
  -- `cargo tree --target wasm32-unknown-unknown -p indicatrix-web` shows neither
  `indicatrix-net` nor `indicatrix-vault` anywhere in the graph.
- **No export presets, no batch rendering, no attachments.** All of that lives in
  `apps/indicatrix-cut`'s `bridge::export_thread`, which this crate does not depend on or
  reimplement. This is a viewer, not the desktop app's export pipeline.
- **A small, fixed control surface.** Camera orbit (drag the stone), distance,
  exposure, four material presets, four lighting presets. No custom-material editor,
  no crystal-optics dialog, no tilt-profile curves -- see `ui/app.slint`, which is a
  few hundred lines against `apps/indicatrix-cut/ui/app.slint`'s much larger tree of
  components.
- **A capped, but no longer fixed, render resolution.** The render target now tracks
  `ui/app.slint`'s render-surface element's actual on-screen size (see
  `src/scene.rs::clamp_render_dims`), reset through the same dirty-flag/`Accumulator`
  mechanism a camera move already used (see `Accumulator::reset`'s doc comment) and
  debounced against drag-resize (`src/app.rs::RESIZE_DEBOUNCE`). It is still bounded,
  deliberately: `src/scene.rs::MAX_RENDER_DIM` and `MAX_DEVICE_PIXEL_RATIO` cap the
  physical pixel count a maximised, high-DPR window can push through this spectral
  path tracer, independent of how large the layout itself grows -- see those
  constants' own doc comments for the reasoning and the exact numbers.

## How the GPU path avoids blocking the browser's one thread

`pollster::block_on` (what `indicatrix::renderer::gpu::context::GpuContext::acquire` and
the native GPU render loop use to wait on `wgpu`'s async device/adapter/submission
futures) parks the calling OS thread until the future resolves. A browser tab's main
thread must never block like that -- there is nothing else that could ever unpark it,
since the very JavaScript event loop that would deliver `wgpu`'s completion callback is
the thing being blocked.

`crates/indicatrix/src/renderer/gpu/frame.rs` therefore carries a second, `wasm32`-only
`impl GpuFrameRenderer` block (see that module's own doc comment) with genuinely
`async fn new_async`/`accumulate_async` that `.await` `wgpu`'s futures instead of
blocking on them, all the way down to buffer readback: `map_read_async` awaits
`wgpu::Buffer::map_async`'s own callback (resolved by the browser's microtask queue,
needing no `Device::poll` call on this backend at all) rather than native's
`compute::finish_map_read`, which calls `Device::poll(Maintain::Wait)` to block until
that same callback fires. `apps/indicatrix-web`'s `src/render.rs` and `src/app.rs` are what
actually run this async path, via `slint::spawn_local` (backed by
`wasm-bindgen-futures` on this target).

This is purely additive on the native side: `GpuFrameRenderer::new`/`accumulate` (the
synchronous, `pollster`-based entry points every desktop/server caller uses) are
completely unchanged, and the new methods live in their own `#[cfg(target_arch =
"wasm32")]` block that native builds never even compile.

## Verifying the build

From the repository root:

```sh
cargo check -p indicatrix-web --target wasm32-unknown-unknown
cargo tree --target wasm32-unknown-unknown -p indicatrix-web   # confirm no indicatrix-net/indicatrix-vault
```

Or, from this directory, `trunk build` runs the full pipeline including
`wasm-bindgen`, which is a stronger check than `cargo check` alone (it also validates
the final link step `cargo check` skips).

Both of those were run to verify this crate; loading the actual served page in a real
browser and confirming pixels appear was not, end to end, in the session that built
this -- the sandboxed browser-automation tool available had `navigator.gpu` present
but no adapter behind it (`request_adapter` returned `NotFound` even from a
UI-independent diagnostic that bypassed `ui/app.slint`'s canvas entirely), and no
connection to a real Chrome instance was available either. `trunk build`/`cargo
check` catch compile errors, not runtime/rendering ones -- if you have a browser with
a working WebGPU adapter, actually opening the page once is worth doing before
trusting this beyond "it compiles."
