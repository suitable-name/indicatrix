//! Wire protocol for `indicatrix-worker`.
//!
//! It covers the read-only design-library sync protocol every build speaks, and
//! (optionally) offloading `indicatrix` spectral ray-sample computation to it as a
//! remote render worker.
//!
//! Types, codec, and framing, plus one small client: [`enroll::claim`] opens a
//! `TcpStream` to a worker's enrollment listener. Everything else operates on an
//! in-memory buffer, a `[u8]` slice, or a generic `Read`/`Write`, so the crate is
//! testable with `std::io::Cursor`. `apps/indicatrix-worker` wires these to a real
//! `TcpStream`/TLS connection.
//!
//! # The `render` feature
//!
//! Off by default. [`scene`] and `messages::RenderRequest`/`ClientMessage::RenderRequest`
//! need `indicatrix`'s resolved scene/material types; everything else ([`library`],
//! [`tls`], [`enroll`], [`token`], [`framing`], [`handshake`]) is always available,
//! `indicatrix`-free.
//!
//! # The `compression` feature
//!
//! On by default. Adds the compressed payload encodings (`zstd`, `lz4_flex`) and the PNG
//! display codec (`png`). Without it the crate still speaks the full protocol but only
//! offers, accepts and decodes `PayloadEncoding::Raw` and raw RGBA8 display frames.
//!
//! # Why sample-index partitioning
//!
//! Samples are additive and order-independent, so a remote node's contribution is just
//! more terms in the viewer's `Vec<Vec3>` sum -- no per-tile stitching needed. Work
//! splits by SAMPLE INDEX rather than screen-space tile, since a gem only occupies part
//! of the frame and background pixels are nearly free to trace (tile partitioning would
//! load-balance badly). Relies on RNG seeds deriving from `hash_u32(pixel_index,
//! sample_number)` with decorrelated per-bounce streams. Additivity against
//! `trace_spectral_ray` itself is verified in `apps/indicatrix-worker`'s
//! `render_core::mod` (around its partition test) and `live_split` (around its own).
//!
//! # Modules
//!
//! - [`library`]: read-only design-library sync protocol, always available.
//! - [`scene`] (`render` feature): [`scene::SceneState`], everything a worker needs to
//!   trace a frame's samples, fully resolved.
//! - [`radiance`]: per-pixel `Vec<Vec3>` radiance-buffer codec, raw POD bytes via
//!   `bytemuck` (hot path, no serialization framework), plus the v14 lossless payload
//!   encodings (byte shuffle + zstd/LZ4) with bounded decoding.
//! - [`display`]: the v14 8-bit picture payloads (`DISPLAY_FRAME`, `FINAL_IMAGE`), raw
//!   RGBA8 or PNG.
//! - [`messages`]: `HELLO`/`WELCOME` and the tagged `ClientMessage`/`StreamEvent`
//!   families, with `postcard` encode/decode.
//! - [`framing`]: length-prefixed message framing over any `Read`/`Write`.
//! - [`handshake`]: build-compatibility check refusing to pair a viewer and worker
//!   running different `indicatrix` physics.
//! - [`tls`]: mutual-TLS config (private CA, TLS 1.3 only) plus the client-certificate
//!   fingerprint allowlist standing in for revocation -- "may this peer talk to me",
//!   separate from [`handshake`]'s "same physics" check. Backs both protocols.
//! - [`client`]: viewer-side protocol driver -- `HELLO`/`WELCOME`, a handshake-only test
//!   connection, `RenderRequest`/`CANCEL` framing, and the epoch-gated accumulator that
//!   sums `FRAME` deltas while keeping `PREVIEW` display-only.
//! - [`token`]: compact `GW1-...` codec for one-time worker-enrollment tokens.
//! - [`enroll`]: enrollment wire messages and claiming client -- verifies a worker's
//!   enrollment listener against the CA fingerprint a token commits to, then redeems it
//!   for a certificate bundle. Shared by `indicatrix-worker`'s `cert claim` and
//!   `indicatrix-cut`'s token-redeem UI.

/// Viewer-side protocol driver -- see this doc comment's "Modules" section.
pub mod client;
/// The v14 8-bit picture payloads (`DISPLAY_FRAME`, `FINAL_IMAGE`) -- see this doc
/// comment's "Modules" section.
pub mod display;
/// Enrollment wire messages and claiming client -- see this doc comment's "Modules"
/// section.
pub mod enroll;
/// Length-prefixed message framing over any `Read`/`Write` -- see this doc comment's
/// "Modules" section.
pub mod framing;
/// Build-compatibility check -- see this doc comment's "Modules" section.
pub mod handshake;
/// Read-only design-library sync protocol, always available -- see this doc comment's
/// "Modules" section.
pub mod library;
/// `HELLO`/`WELCOME` and the tagged `ClientMessage`/`StreamEvent` families -- see this
/// doc comment's "Modules" section.
pub mod messages;
/// Per-pixel radiance-buffer codec and the v14 lossless payload encodings -- see this
/// doc comment's "Modules" section.
pub mod radiance;
/// [`scene::SceneState`] (`render` feature only).
///
/// Everything a worker needs to trace a frame's samples, fully resolved -- see this doc
/// comment's "Modules" section.
#[cfg(feature = "render")]
pub mod scene;
/// Mutual-TLS config and the client-certificate fingerprint allowlist -- see this doc
/// comment's "Modules" section.
pub mod tls;
/// Compact `GW1-...` codec for one-time worker-enrollment tokens -- see this doc
/// comment's "Modules" section.
pub mod token;

#[cfg(feature = "render")]
pub use scene::SceneState;
