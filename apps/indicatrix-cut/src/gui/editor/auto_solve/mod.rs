//! Background solving for the "Edit" sub-tab: moves `Design::solve` off the UI thread
//! for the explicit "Solve" action (button and F5, via [`super::view::refresh_all`]'s
//! large-design path) and, when the design is cheap enough, schedules the SAME
//! machinery automatically after an edit ("auto-solve") so small/medium designs get a
//! fresh path-traced view and masts without a click. See `mod.rs`'s "Never block the
//! UI thread with a solve" section for the rule this exists to uphold.
//!
//! # Why a `thread_local!`, not a new `EditorState` field
//!
//! `deep_solve`/`optimize`'s in-flight handles live directly on `EditorState`
//! (`state/mod.rs`), which is NOT this group's file to edit. Everything this module
//! needs beyond `EditorState::design`/`generation` (both already readable through a
//! plain `&EditorState`) -- the debounce timer, the running request counter, the last
//! measured solve time, and the shared solid-preview handles a completed background
//! solve pushes into -- lives instead in [`runtime::Runtime`], a `thread_local!`
//! singleton. That is sound for the identical reason `EditorState` itself gets away
//! with a plain `Rc<RefCell<..>>` rather than `Arc<Mutex<..>>` (see that struct's own
//! doc comment): every function in this group that touches `Runtime` runs on the
//! UI/event-loop thread, either directly from a Slint callback or from inside a
//! `Weak::upgrade_in_event_loop` closure. The worker threads this group spawns never
//! touch `Runtime` themselves -- they only compute pure `Design` -> data conversions
//! and hand the result back across the event-loop boundary.
//!
//! # Two kinds of "stale" a completed background solve must detect
//!
//! A background solve is dispatched against a snapshot: a cloned [`Design`] plus the
//! [`EditorState::generation`] counter's value at that moment. By the time it
//! completes, either or both of the following can have happened, and
//! [`apply::apply_background_solve_result`] must not let either corrupt the
//! display:
//!
//! 1. **A newer background solve was dispatched** (the user clicked Solve again, or
//!    another auto-solve fired) before this one returned. [`runtime::Runtime::current_seq`]
//!    is bumped on every dispatch; a completion only touches the UI at all while its
//!    own sequence number still matches -- an older one simply has nothing left to
//!    do, since the newer dispatch already owns the "Solving..." banner and will
//!    apply its own result in turn.
//! 2. **The design changed without a new dispatch** -- possible when auto-solve is
//!    disabled (or this design's own measured cost exceeds the budget) and an edit
//!    lands while an explicit Solve from BEFORE that edit is still running. Here this
//!    result's own sequence number is still current (nothing superseded it), but
//!    `generation` has moved past what was captured at dispatch time. The edit's own
//!    [`super::view::refresh_editor_panel_stale`] call already repainted the banner
//!    correctly at edit time, so this arm only needs to clear `editor_solve_running`
//!    (nothing else will) and otherwise touch nothing.
//!
//! Both checks are cheap `u64` comparisons; nothing here ever tries to reinterpret a
//! stale result's tier indices against a design it no longer describes.
//!
//! # Layout of this group
//!
//! [`runtime`] holds the `thread_local!` state and the handles [`init`] stashes into
//! it. [`scheduling`] decides whether/when a solve should run synchronously or be
//! debounced into an auto-solve. [`dispatch`] spawns the background solve and ticks
//! its banner; [`apply`] applies the completed result back onto the UI. [`replan`]
//! is the solid-preview replan handshake: converting an already-solved mast list to
//! viewport planes, stashing the design snapshot a matching preview frame consumes,
//! and the idle-replan follow-up for a partial frame. [`tests`] covers all five.

mod apply;
mod dispatch;
mod replan;
mod runtime;
mod scheduling;
#[cfg(test)]
mod tests;

pub(super) use dispatch::{
    cancel_in_flight_solve, dispatch_background_solve, likely_hit_plane_cap, solve_cancellably,
    too_many_planes_message,
};
pub(super) use replan::{
    design_to_gpu_planes_from_solved, schedule_idle_replan_if_stale, stash_current_design,
    take_matching_design,
};
pub(super) use runtime::{
    activity, editor_state, init, preview_state, render_ctx, solid_last_solved, stash_editor_state,
    stash_render_ctx,
};
pub(super) use scheduling::{
    last_solve, on_edit, record_solve_duration, reset_for_new_design, should_solve_synchronously,
};
