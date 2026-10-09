//! Tests for `serve`, split by topic: [`handshake`] (`HELLO`/`WELCOME` and validation),
//! [`render_roundtrip`] (real-socket `RenderRequest`/CPU-GPU parity),
//! [`streaming`] (delta tiling, `FinalOnly` progress, `PREVIEW` isolation),
//! [`cancellation`] (`CANCEL`, the write-timeout bound, pipelining), [`tilt_curves`]
//! (the connection stays usable afterward), [`v14`] (payload-encoding negotiation and
//! compressed streams, `PING`/`PONG`, the v14 refusals), [`hdr_assets`] (an HDR
//! scene's map fetched once by `NEED_ASSET`/`ASSET`), [`lazy_library`] (a connection
//! opens the library database on its first library request, never for anything else),
//! [`batch`] (protocol v24 batched preview requests: shared PNG bytes, per-item failure,
//! `CANCEL`, two batches on one connection).
//! [`fixtures`] holds the scene/database builders and `Read + Write` test doubles shared
//! across them.
//!
//! [`mtls`] (real mutual-TLS handshakes over real loopback sockets) is a separate,
//! larger test suite kept in its own file, as it always has been.

mod batch;
mod cancellation;
pub mod fixtures;
mod handshake;
mod hdr_assets;
mod lazy_library;
mod render_roundtrip;
mod streaming;
mod tilt_curves;
mod v14;
#[cfg(feature = "zoning")]
mod zoning;

// `mtls` (below) still reaches these through `super::{..}`, exactly as when this whole
// folder was one flat `tests.rs` file -- kept as a private re-export here rather than
// editing `mtls.rs`'s own `use` line, since that file otherwise stays untouched.
use fixtures::{final_only, read_stream_until_done, test_db, tiny_scene};

// Each test builds a real throwaway private CA and issues real certificates via
// `crate::pki`, then drives a real TLS handshake over a real loopback `TcpStream` --
// nothing is mocked at the `rustls` layer, to catch what a mock would paper over (a
// missing SAN, a wrong trust anchor, an allowlist that isn't actually consulted).
mod mtls;
