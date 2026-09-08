//! [`state::Orchestrator`]'s own state, the repeating timer tick
//! ([`poll::setup_remote_rendering`]/`poll_tick`), and dispatching a settled pose to a
//! remote worker (`dispatch::start_remote_render`/`update::handle_remote_update`/
//! `update::redraw_from_accumulator`). See this group's own `mod.rs` doc comment.
//!
//! Split along natural seams: [`state`] (the `Orchestrator`/`Pose` types and the
//! `lock` helper every other file here uses), [`poll`] (the timer tick itself and
//! carrying out the actions it decides on), [`dispatch`] (starting a remote render for
//! a just-settled pose), and [`update`] (handling what comes back and redrawing).
//! `poll`, `dispatch`, and `update` call into each other in a real three-way cycle,
//! not a layering mistake: they are one coordinated state machine
//! (`bridge::handoff::HandoffMachine`), not a one-directional pipeline. Rust does not
//! require submodules of the same parent to form a DAG.

mod dispatch;
mod poll;
mod state;
mod update;

pub use poll::setup_remote_rendering;
