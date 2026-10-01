//! The compute side of the indicatrix browser app, with no DOM and no Slint in it.
//!
//! Everything here except [`host`] builds and is tested natively, so the logic a Web
//! Worker runs is checked by `cargo test -p indicatrix-web-core` rather than only in a
//! browser.
//!
//! - [`protocol`]: the messages between the page and its Workers ([`protocol::ToWorker`],
//!   [`protocol::FromWorker`]), postcard-encoded into a transferred `ArrayBuffer`, and
//!   the [`protocol::PROTOCOL_VERSION`] checked on `Init`.
//! - [`scene`]: [`scene::SceneSpec`], the plain-data description of a render scene, and
//!   [`scene::OwnedScene`], which builds exactly the scene the desktop builds for the
//!   same settings and lends out a `FrameScene`.
//! - [`render`]: the per-chunk CPU trace ([`render::handle_trace_chunk`]), the full-frame
//!   [`render::Accumulator`] and the [`render::ChunkPlanner`] that sizes chunks.
//! - [`solve`]: [`solve::handle_solve`], the solve worker's job: a native-TOML design in,
//!   the solved masts, planes, status and warnings out.
//! - [`solve_error`]: [`solve_error::SolveError`], the typed reason a solve-role job
//!   produced no answer (superseded, cancelled, failed).
//! - [`display`]: pixels from an accumulation -- the live tone map, the settled
//!   denoise, the PNG export and its file name -- each the desktop's own call.
//! - [`settings`]: the persisted render settings and session payload, and
//!   [`settings::scene_spec`], which turns them into a [`scene::SceneSpec`] with the
//!   desktop's conversions.
//! - [`custom_material`]: the Design settings dialog's custom-material fields turned into
//!   a `GemMaterial` and the snapshot a native file stores for it.
//! - [`guide`]: the guided walkthrough's tab-session state (open, step, collapsed,
//!   where the floating panel was dragged to).
//! - [`hdr`]: the browser's HDR limits and the render-worker count rule for a map.
//! - [`input`]: the size limits checked on a picked or dropped file before it is read.
//! - [`worker`]: [`worker::WorkerHandler`], the whole message-handling state machine a
//!   Worker runs; the `indicatrix-web-compute` crate only moves bytes in and out of it.
//! - [`host`] (wasm32 only): the page-side pool, [`host::WorkerPool`] with its
//!   [`host::RenderPool`] and [`host::SolveClient`].

pub mod custom_material;
pub mod display;
pub mod guide;
pub mod hdr;
#[cfg(target_arch = "wasm32")]
pub mod host;
pub mod input;
pub mod protocol;
pub mod render;
pub mod scene;
pub mod settings;
pub mod solve;
pub mod solve_error;
pub mod worker;

/// Where the host finds the Worker script: the loader shim Trunk writes next to the
/// app for `apps/indicatrix-web/index.html`'s `data-type="worker"` link.
///
/// Trunk names worker outputs after the Cargo package, never with a content hash
/// (`trunk` 0.21's `hashed_wasm_base` skips hashing for `data-type="worker"`), and
/// `data-loader-shim` adds `<name>_loader.js`, a classic-worker script that runs
/// `importScripts("./indicatrix-web-compute.js")` and then instantiates
/// `./indicatrix-web-compute_bg.wasm`. The URL is relative, so it resolves against the
/// page and keeps working when the app is served from a sub-path.
pub const WORKER_LOADER_URL: &str = "./indicatrix-web-compute_loader.js";
