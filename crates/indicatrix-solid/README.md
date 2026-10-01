# indicatrix-solid

A pure-CPU flat-shaded solid renderer and 2D faceting diagram for a design's
`SolidMesh`, independent of the GPU path tracer.

`indicatrix-solid` is the "Solid" and "Diagram" preview the desktop editor and the
wasm web app both need: given a design's plane arrangement, it builds a closed
`SolidMesh` (via `indicatrix::geometry::stone_metrics`), caches it, and rasterizes it
either as a perspective flat-shaded/edge-outlined view or as a three-panel
GemCAD-style crown/pavilion/profile diagram, with a per-pixel facet-picking buffer
for hover/click/selection in both. It holds no GUI toolkit types, no threads and no
filesystem access, so it links unchanged into a `wasm32-unknown-unknown` build.

## Main types

- **`raster::SolidRasterizer`** — the perspective solid render: RGBA8 color, depth,
  and pick buffers, plus `FillMode`/`SolidStyle` for flagged/pending/selected
  overlays and the orientation marker.
- **`diagram2d::{render_diagram, render_diagram_single_panel}`** / **`DiagramFrame`**
  — the three-panel (or single-panel) 2D faceting diagram, with its own pick, panel,
  and index-wheel-tooth buffers.
- **`facet_map::FacetMap`** — maps a rasterized `facet_id` back to the tier/orbit
  member it came from, for hover text, labels, and the critical-angle overlay.
- **`mesh_cache::MeshCache`** / **`CachedMesh`** — caches one `SolidMesh` build (plus
  a one-time ring-simplification pass) against the plane arrangement's hash, so a
  burst of redraws carrying the same planes never re-solves or re-simplifies.
- **`preview::FrameGeometry`** — carried by every `RenderedFrame`: the drawn mesh's
  distinct corner points and per-facet centroids (built once per mesh, `Arc`-shared)
  with the exact camera pose and pixel size the raster used, so the direct-manipulation
  handles project onto the same pixels the pick buffer holds.
- **`preview::Outlines`** / **`SharedOutlines`** — the provisional-tier and drag-follower
  outlines an embedding app sets out-of-band (`WorkerMemory::outlines`): read when a
  frame is drawn, so a replan that rebuilds the style, or an overlay update superseded
  in the request gate, can neither drop nor resurrect them. `None` (the default, and the
  web app's) changes nothing.
- **`edges_layer::render_edges_layer`** — the transparent-fill, opaque-edges render
  used to composite solid edges over a path-traced image.
- **`live_update::plan_preview`** — chooses which geometry to draw after an edit
  (pinned / fresh / stale / unsolvable), budgeted via an injected
  **`live_update::Clock`** rather than `std::time::Instant` (which panics at runtime
  on `wasm32-unknown-unknown`): the desktop passes `live_update::InstantClock`, the
  web app passes one backed by `performance.now()`.
- **`pixel_font`** — the shared 5x7 bitmap font `diagram2d`'s panel labels draw with.

## Not in this crate

Threads, a Slint (or any GUI toolkit) pixel-buffer conversion, and the worker/cache
controller that owns them across redraws stay with each caller: the desktop's
`apps/indicatrix-cut/src/gui/solid_preview` keeps `preview_state` (the dedicated
worker thread and `RedrawGate` coalescing), `diagram_wiring` (Slint callback wiring),
and the `to_pixel_buffer`/`to_diagram_pixel_buffer` conversions into
`slint::SharedPixelBuffer`, re-exporting everything else from this crate at its old
module paths so the rest of the desktop app compiles unchanged.
