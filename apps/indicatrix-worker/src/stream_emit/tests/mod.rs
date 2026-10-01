//! Tests for `stream_emit`, split by the piece of pure logic each topic file exercises:
//! [`pending_delta`] (delta coalescing), [`batch_sizing`] (adaptive sub-batch sizing),
//! [`downsample`] (the reduced-resolution `PREVIEW` snapshot), [`cadence`] (effective
//! cadence averaging), [`emit_tick`] (one cadence-tick emission), and [`liveness`]
//! (stream-timeout classification and the heartbeat backstop), and [`stall`] (the
//! producer-stall watchdog, end to end). [`fixtures`] holds the
//! scene/state/request builders and `StreamEvent`-decoding helpers shared across them.

mod batch_sizing;
mod cadence;
mod downsample;
mod emit_tick;
mod fixtures;
mod liveness;
mod pending_delta;
mod stall;
