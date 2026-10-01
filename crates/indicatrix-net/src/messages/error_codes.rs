//! The machine-readable `ErrorMsg::code` values every server in this workspace sends.
//!
//! One vocabulary for the `ErrorMsg` sent in place of `WELCOME`, as a
//! `StreamEvent::Error`, inside `TiltCurvesResponse::Error`, or inside
//! `LibraryResponse::Error` -- a single namespace: the library path already reuses
//! [`NO_RENDER_CAPACITY`] (a library request on a joined worker's connection), so its
//! own codes ([`LIBRARY_FAILED`], [`LIBRARY_REQUEST_INVALID`]) live here too, with
//! values no other code uses. Since v15 `ErrorMsg` carries an optional `request_id`: a
//! `StreamEvent::Error` naming one is epoch-gated to that request, and one without
//! describes the connection's handshake or its CURRENT request. A coordinator never
//! forwards a worker's `ErrorMsg` verbatim to a viewer; it translates it (see the
//! coordinator guide).
//!
//! Values are wire-load-bearing: never renumber, only add. The one deliberate
//! duplicate is [`VALIDATION_FAILED`] == [`NO_RENDER_CAPACITY`] (historical); every
//! other value is unique (pinned by this module's test).

/// The `HELLO` was refused: protocol-version, `build_hash` or `source_hash` mismatch.
///
/// Sent in place of `WELCOME`; the message names both sides' values.
pub const BUILD_MISMATCH: u32 = 1;

/// A render or tilt request reached a server with no render capacity.
///
/// A library-only build, or a connection paired as library-only.
pub const NO_RENDER_CAPACITY: u32 = 2;

/// A request failed validation (scene, sample range, stream config).
///
/// Shares its value with [`NO_RENDER_CAPACITY`] for historical reasons; the message
/// tells them apart.
pub const VALIDATION_FAILED: u32 = 2;

/// The tracer panicked on a validation-passing but pathological scene.
pub const TRACE_PANIC: u32 = 3;

/// The server is already at its `--max-connections` cap. Sent in place of `WELCOME`.
pub const CONNECTION_LIMIT_REACHED: u32 = 4;

/// v14: the request is well-formed but this server does not implement it.
///
/// For now a `FinalImageRequest`, or a `RenderRequest` with
/// `TransferMode::DisplayOnly`, on a plain worker. The connection stays usable.
pub const UNSUPPORTED_REQUEST: u32 = 5;

/// v14: a coordinator lost every lane that could finish the request.
///
/// All joined workers failed and it has no own render lane. Sent as
/// `StreamEvent::Error`; the stream ends there with no `DONE`, exactly like any other
/// stream error. A viewer falls back to local rendering for the unfinished part.
pub const ALL_WORKERS_LOST: u32 = 6;

/// v14: the `HELLO`'s role is not accepted on this port.
///
/// A `PeerRole::Worker` Hello on a server that does not accept joining workers (or on
/// its viewer port), a `PeerRole::Viewer` Hello on a worker port, or a Hello whose role
/// and capability disagree. Sent in place of `WELCOME`.
pub const ROLE_REFUSED: u32 = 7;

/// v14: a request's asset (an HDR environment map) could not be obtained.
///
/// The client did not send it in time, sent bytes whose SHA-256 differs, or the bytes do
/// not decode (malformed, or over the decode limits); or the server cannot store it.
/// Sent as `StreamEvent::Error` for the request that needed the asset; the connection
/// stays usable.
pub const ASSET_FAILED: u32 = 8;

/// v14: a `LibraryRequest` failed on the server's side (a database error).
///
/// Sent inside `LibraryResponse::Error` with a generic message; the real reason is only
/// logged server-side. Was the library-local value 4 before v14 (colliding with
/// [`CONNECTION_LIMIT_REACHED`]).
pub const LIBRARY_FAILED: u32 = 9;

/// v14: a `LibraryRequest` was refused before touching the database (currently only an
/// out-of-range tilt-performance filter).
///
/// Sent inside `LibraryResponse::Error`, so a client can tell "malformed request" apart
/// from [`LIBRARY_FAILED`]. Was the library-local value 5 before v14 (colliding with
/// [`UNSUPPORTED_REQUEST`]).
pub const LIBRARY_REQUEST_INVALID: u32 = 10;

/// v16: the server's render lane reported no progress for the configured stall window.
///
/// Sent as `StreamEvent::Error` for the stalled request, with no `DONE`; the connection
/// stays usable. A wedged tracer keeps the emitter's `PROGRESS` heartbeat going, so a
/// client's silence-based liveness deadline never fires -- this is the server saying so
/// itself. A client that does not know the code treats it like any other request failure
/// and falls back (it is a plain `u32`, never decoded into an enum).
pub const PRODUCER_STALLED: u32 = 11;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every code in the shared table has its own value, except the one documented
    /// historical alias ([`VALIDATION_FAILED`] == [`NO_RENDER_CAPACITY`]).
    #[test]
    fn every_code_is_unique_apart_from_the_documented_alias() {
        assert_eq!(VALIDATION_FAILED, NO_RENDER_CAPACITY);
        let codes = [
            ("BUILD_MISMATCH", BUILD_MISMATCH),
            ("NO_RENDER_CAPACITY / VALIDATION_FAILED", NO_RENDER_CAPACITY),
            ("TRACE_PANIC", TRACE_PANIC),
            ("CONNECTION_LIMIT_REACHED", CONNECTION_LIMIT_REACHED),
            ("UNSUPPORTED_REQUEST", UNSUPPORTED_REQUEST),
            ("ALL_WORKERS_LOST", ALL_WORKERS_LOST),
            ("ROLE_REFUSED", ROLE_REFUSED),
            ("ASSET_FAILED", ASSET_FAILED),
            ("LIBRARY_FAILED", LIBRARY_FAILED),
            ("LIBRARY_REQUEST_INVALID", LIBRARY_REQUEST_INVALID),
            ("PRODUCER_STALLED", PRODUCER_STALLED),
        ];
        for (i, (name_a, a)) in codes.iter().enumerate() {
            for (name_b, b) in &codes[i + 1..] {
                assert_ne!(a, b, "{name_a} and {name_b} share the value {a}");
            }
        }
    }
}
