//! Pure-CPU solid rasterizer and 2D faceting diagram for a design's
//! [`indicatrix::geometry::stone_metrics::SolidMesh`], independent of the GPU path
//! tracer -- see this crate's own README for the full picture.
//!
//! [`raster`] is a pure-Rust rasterizer with its own unit tests. [`mesh_cache`]
//! caches the `SolidMesh` build plus a one-time ring-simplification pass against
//! the design's plane-set hash. [`facet_map`] maps a rasterized `facet_id` back to
//! its tier/orbit-member for hover/click/selection and the critical-angle overlay;
//! [`live_update`] decides which geometry to draw after an edit (`plan_preview`,
//! a budgeted `resolve_dirty` with a pinned/fresh/stale/unsolvable outcome, driven
//! by an injected [`live_update::Clock`] rather than `std::time::Instant`, which
//! panics at runtime on `wasm32-unknown-unknown`); [`edges_layer`] is the "Both"
//! view mode's transparent-fill, opaque-edges render; [`diagram2d`] is the
//! GemCAD-style three-panel (crown/pavilion/profile) 2D faceting diagram, with its
//! own per-pixel facet-picking buffer. [`pixel_font`] is the shared 5x7 bitmap
//! font [`diagram2d`]'s panel labels draw with. [`preview`] is the frame pipeline
//! on top of them (plan, resolve, rasterize, with hover/selection overlays), which
//! the desktop's worker threads and the web app's main thread both drive.
//!
//! No GUI toolkit types, no threads, no filesystem access anywhere in this crate --
//! every caller (the desktop editor, the wasm web app) converts the RGBA8 output to
//! its own pixel-buffer type at its own boundary. The desktop's
//! `apps/indicatrix-cut/src/gui/solid_preview` re-exports every module here at its
//! old paths, plus keeps its own `preview_state` (worker thread), `diagram_wiring`
//! (Slint callback wiring), and `to_pixel_buffer`/`to_diagram_pixel_buffer`
//! conversions -- see this crate's README.

pub mod diagram2d;
pub mod edges_layer;
pub mod facet_map;
pub mod live_update;
pub mod mesh_cache;
pub mod pixel_font;
pub mod preview;
pub mod raster;
