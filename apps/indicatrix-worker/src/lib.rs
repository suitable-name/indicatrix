//! `indicatrix-worker`: a library server for a `indicatrix`-backed gemstone design catalogue,
//! with optional render capacity on top.
//!
//! **Default role: library server.** Running `serve` with no extra features accepts
//! `indicatrix-net`'s read-only design-library protocol (see [`serve::library`]) over
//! mutual TLS -- listing/searching designs, fetching one in full, fetching an
//! attachment's bytes. This is the default, not an add-on: a future Slint mobile client
//! talks to exactly this protocol and never has a renderer compiled in.
//!
//! **`worker` feature (off by default): render capacity.** Gates `RenderRequest`
//! handling, `stream_emit`, `render_core`, `render_cmd`, the `render` subcommand, and
//! `Backend` advertisement in `WELCOME` -- see `Cargo.toml` for exactly what that turns
//! on, and [`serve`]'s module docs for how one connection dispatches between the library
//! and render protocols once both are compiled in.
//!
//! - `render` (only meaningful with `worker` on): trace a scene straight to a PNG, no
//!   networking. See [`render_cmd::run`].
//! - `serve`: accept connections over TCP and serve the library protocol (always) and,
//!   under `worker`, `RenderRequest`s too. See [`serve::run`].
//!
//! # GPU
//!
//! An optional `gpu` feature (off by default, implies `worker`) routes tracing through
//! `indicatrix`'s GPU megakernel instead of `optics::raytracer::trace_spectral_ray`.
//! Verified end to end against a real adapter -- see
//! `crates/indicatrix/examples/gpu_equivalence_harness.rs`.
//!
//! [`indicatrix::renderer::gpu_backend::GpuBackend`] is the decline/fallback wrapper both
//! this crate and `apps/indicatrix-cut` drive. It lives in `indicatrix`, not here, since
//! a duplicated copy of a correctness policy (the HDR-environment decline) can drift
//! into silently rendering the wrong image.

// Trait-solver resource limit, not a lint: under `--features gpu`, proving the
// `thread::spawn` closure in `serve::run`'s accept loop is `Send` requires recursing
// through `wgpu`'s internal types deeper than the default limit allows. Unconditional
// (not `cfg_attr`-gated to `gpu`) since a single crate-wide limit is simpler than
// tracking which feature combination needs it, and it's a no-op when `wgpu` isn't
// pulled in. Do not remove: the overflow reproduces on every `--features gpu` build.
#![recursion_limit = "256"]

pub mod cli;
pub mod enroll;
pub mod enroll_client;
pub mod pki;
#[cfg(feature = "worker")]
pub mod png_out;
#[cfg(feature = "worker")]
pub mod render_cmd;
#[cfg(feature = "worker")]
pub mod render_core;
pub mod serve;
#[cfg(feature = "worker")]
mod stream_emit;
#[cfg(feature = "worker")]
pub mod validate;
