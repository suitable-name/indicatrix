//! Everything specific to dispatching an export's remote lane against an
//! `indicatrix-worker`: whether remote is even available and within its advertised
//! limits ([`capability`]), how big a chunk to claim next given the rate observed so
//! far ([`rate`]), and running the dispatch loop for the whole concurrent phase
//! ([`dispatch`]).
//!
//! # Where disjointness comes from
//!
//! Nothing here hands out sample ranges itself: every chunk comes from
//! `SampleCursor::claim`, the single shared atomic claim point local and remote both
//! draw from. This module only decides how much to claim next
//! ([`rate::remote_chunk_samples`]) and runs each claimed chunk to completion or
//! failure ([`dispatch::run_remote_batch`]).
//!
//! # Partial completion, not failure
//!
//! A chunk that comes back short (dropped connection, worker-side error, early
//! `DONE`) is not automatically an export failure: [`dispatch::run_remote_lane`]
//! merges whatever prefix completed and hands the remainder back to
//! `SampleCursor::return_to_local` for the local lane, as long as one exists
//! (`ComputeTarget::Both`). Only `ComputeTarget::RemoteOnly`, with no local lane to
//! fall back to, turns a short chunk into a fatal [`dispatch::RemoteLaneOutcome`].
mod capability;
mod dispatch;
mod rate;

pub(in crate::bridge::export_thread) use capability::{REMOTE_MIN_SPP, exceeds_pixel_cap};
pub use capability::{RemoteCapability, probe_remote};
pub(in crate::bridge::export_thread) use dispatch::{
    RemoteLaneOutcome, RemoteProgress, run_remote_lane, scene_state_from_snapshot,
};
pub(in crate::bridge::export_thread) use rate::{
    REMOTE_CALIBRATION_SAMPLES, RemoteCalibration, calibrate_remote_rate,
};
