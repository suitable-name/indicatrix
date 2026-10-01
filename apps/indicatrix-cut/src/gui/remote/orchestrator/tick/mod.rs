//! [`state::Orchestrator`]'s own state, the repeating timer tick
//! ([`poll::setup_remote_rendering`]/`poll_tick`), and dispatching a settled pose to a
//! remote worker (`dispatch::start_remote_render`/`update::handle_remote_update`/
//! `update::redraw_from_epoch`). See this group's own `mod.rs` doc comment.
//!
//! Split along natural seams: [`state`] (the `Orchestrator`/`Pose` types and the
//! `lock` helper every other file here uses), [`poll`] (the timer tick itself and
//! carrying out the actions it decides on), [`dispatch`] (starting a remote render for
//! a just-settled pose), and [`update`] (handling what comes back and redrawing).
//! `poll`, `dispatch`, and `update` call into each other in a real three-way cycle,
//! not a layering mistake: they are one coordinated state machine
//! (`bridge::remote::handoff::HandoffMachine`), not a one-directional pipeline. Rust does not
//! require submodules of the same parent to form a DAG.
//!
//! # Suspending the live remote lane
//!
//! Independently of the settle/handoff cycle above, every tick also gates whether the
//! live remote lane may transmit ANYTHING right now (`poll::live_remote_allowed`: tab
//! visible, not paused, no export/batch job running) and reacts the instant that turns
//! false (`poll::update_live_remote_suspension`/`poll::suspend_live_remote`): a
//! display-only (final-picture) lane is abandoned outright, since its one request
//! cannot be resumed mid-stream; an accumulating lane just has its in-flight chunk
//! cancelled on the wire and is left idle in place, with the confirmed cancellation
//! (`update::on_chunk_paused`) merging its valid prefix without counting a failure or
//! touching the rate estimate. `poll::resume_idle_lane` and `poll::maybe_redispatch`
//! (via `decisions::RedispatchInputs::suspended`) both refuse to continue or start a
//! new epoch while this holds, so a scene change while paused (dragging while paused,
//! say) never sneaks a fresh epoch out to the worker either.

mod decisions;
mod dispatch;
mod poll;
mod state;
mod update;

pub use poll::{RemoteOrchestratorHandle, setup_remote_rendering};
