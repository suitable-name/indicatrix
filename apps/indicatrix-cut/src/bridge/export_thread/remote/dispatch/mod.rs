//! Dispatching against a remote worker: one `RenderRequest` ([`run_batch::run_remote_batch`]),
//! building its `SceneState` ([`scene_state::scene_state_from_snapshot`]), the
//! cross-thread progress state a running lane publishes ([`progress::RemoteProgress`]),
//! and the lane itself that repeatedly claims and dispatches chunks for the whole
//! concurrent phase ([`lane::run_remote_lane`]).

mod lane;
mod progress;
mod run_batch;
mod scene_state;
#[cfg(test)]
mod tests;

pub(in crate::bridge::export_thread) use lane::{RemoteLaneOutcome, run_remote_lane};
pub(in crate::bridge::export_thread) use progress::RemoteProgress;
pub(in crate::bridge::export_thread) use run_batch::{
    CANCEL_WAIT_TIMEOUT, liveness_deadline, run_remote_batch,
};
pub(in crate::bridge::export_thread) use scene_state::scene_state_from_snapshot;
