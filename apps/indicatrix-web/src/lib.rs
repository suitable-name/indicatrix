//! Browser build of `indicatrix`: open a faceting design (a self-contained
//! `.indicatrix` file, `.asc`, an older `.asc` + `.indicatrix.toml` pair, `.gem`,
//! `.gcs`), keep it in the shared `indicatrix_editor::EditorSession`, and save it
//! back as a browser download. See `README.md` for what works today and what the
//! later phases add.
//!
//! # Modules
//!
//! - `app`: the one `app::state::WebApp` (in an `Rc<RefCell<..>>`), the entry
//!   point, callback wiring, the `push_*` UI refresh, persistence to
//!   `sessionStorage`, the solve (tiny designs on the page, the rest in the solve
//!   Worker), diagnostics.
//! - `io`: file input (picker + drag-and-drop), per-kind loaders, the HDR cap, and
//!   file output (`Blob` downloads).
//! - `render`: the CPU-worker live render loop, its settings panel, the PNG export
//!   and the viewport-size tracking.
//! - `workers`: the one lazily created Worker pool (render Workers + solve Worker, and
//!   the analysis Worker on first use).
//! - `metrics`: the Render tab's gemological-metrics HUD and the Tilt Performance dialog,
//!   both computed in the analysis Worker.
//!
//! - `views`: the Solid and Diagram tabs.
//! - `editor`: the CAD panels of the Design dock -- the tier table, its command bar and
//!   editing actions, and the unsaved-changes dialog.
//!
//! # Why everything is `#[cfg(target_arch = "wasm32")]`
//!
//! This crate exists for one target: it drives a Slint UI through `wasm-bindgen`
//! and reads files through the DOM. A native build compiles to nothing, so
//! `cargo check --workspace` on a native host costs the desktop apps nothing.
//! Pure logic that deserves native tests lives in the shared crates instead
//! (`indicatrix-editor`'s `files` module, for one).

#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod editor;
#[cfg(target_arch = "wasm32")]
mod io;
#[cfg(target_arch = "wasm32")]
mod metrics;
#[cfg(target_arch = "wasm32")]
mod render;
#[cfg(target_arch = "wasm32")]
mod views;
#[cfg(target_arch = "wasm32")]
mod workers;

// Must run in the crate root: it `include!`s build.rs's generated module.
#[cfg(target_arch = "wasm32")]
slint::include_modules!();
