//! Browser build of `indicatrix`: upload a `GemCAD` `.asc` cutting schedule, get back an
//! interactive rendered stone. No design library, no remote worker, no database -- see
//! `README.md` for the full "what this deliberately omits and why" list.
//!
//! # Why almost everything here is `#[cfg(target_arch = "wasm32")]`
//!
//! This crate exists for exactly one target triple: acquiring a WebGPU device without
//! blocking the browser's thread, driving a Slint UI through `wasm-bindgen`, reading an
//! uploaded file through the DOM's File API. Rather than make every module decide
//! whether it's meaningful on a native host, everything real lives under [`mod@app`],
//! gated on `wasm32`, so a non-wasm32 build compiles to nothing.
//!
//! That matters because `cargo check --workspace` from a native host builds this crate
//! too, alongside the real desktop/server products `apps/indicatrix-cut` and
//! `apps/indicatrix-worker`; an empty native build keeps a browser demo from costing
//! either of them a slower build or a compile error.

#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod render;
#[cfg(target_arch = "wasm32")]
mod scene;

// `slint::include_modules!()` must run in a crate root (it expands to an `include!` of
// build.rs's generated file, which uses `super`-relative paths). Kept here rather than
// in `app.rs` so that file stays the one that actually drives the UI.
#[cfg(target_arch = "wasm32")]
slint::include_modules!();
