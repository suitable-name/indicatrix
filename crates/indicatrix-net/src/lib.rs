//! Wire protocol for `indicatrix-worker`: the read-only design-library sync protocol
//! every build speaks, and (optionally) offloading `indicatrix` spectral ray-sample
//! computation to it as a remote render worker.
//!
//! Types, codec, and framing only -- no networking. Every function operates on an
//! in-memory buffer, a `[u8]` slice, or a generic `Read`/`Write`, so the whole crate is
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
//! # Why sample-index partitioning
//!
//! Samples are additive and order-independent, so a remote node's contribution is just
//! more terms in the viewer's `Vec<Vec3>` sum -- no per-tile stitching needed. Work
//! splits by SAMPLE INDEX rather than screen-space tile, since a gem only occupies part
//! of the frame and background pixels are nearly free to trace (tile partitioning would
//! load-balance badly). Relies on RNG seeds deriving from `hash_u32(pixel_index,
//! sample_number)` with decorrelated per-bounce streams;
//! `tests/partition_correctness.rs` verifies additivity against `trace_spectral_ray`.
//!
//! # Modules
//!
//! - [`library`]: read-only design-library sync protocol, always available.
//! - [`scene`] (`render` feature): [`scene::SceneState`], everything a worker needs to
//!   trace a frame's samples, fully resolved.
//! - [`radiance`]: per-pixel `Vec<Vec3>` radiance-buffer codec, raw POD bytes via
//!   `bytemuck` (hot path, no serialization framework).
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

pub mod client;
pub mod enroll;
pub mod framing;
pub mod handshake;
pub mod library;
pub mod messages;
pub mod radiance;
#[cfg(feature = "render")]
pub mod scene;
pub mod tls;
pub mod token;

#[cfg(feature = "render")]
pub use scene::SceneState;
