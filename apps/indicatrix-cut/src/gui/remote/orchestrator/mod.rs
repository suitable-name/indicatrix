//! The preview-then-handoff orchestrator: a repeating `slint::Timer` that polls
//! `RenderContext`'s camera/light pose, feeds `bridge::remote::handoff::HandoffMachine`, and
//! dispatches to `bridge::remote::remote_render` when the machine decides to hand off --
//! including the async guide-buffer and denoise generations that keep the (multi-second
//! at 4K) denoise pass off the Slint UI thread.
//!
//! `poll_tick` (in [`tick`]) is also the one place that decides "is the camera
//! currently moving" for `bridge::render_thread::local_preview` -- it writes
//! `RenderContext::camera_moving` from this same `HandoffMachine` instance's state
//! every tick, so both features share one definition of "settled".
//!
//! Every tick also checks whether the live remote lane is currently ALLOWED to
//! transmit at all (`tick::poll::live_remote_allowed`: the tab showing it is visible,
//! live rendering isn't paused, and no high-resolution export/batch job is running) and
//! suspends it the instant that turns false (`tick::poll::suspend_live_remote`),
//! independently of the settle/redispatch decisions above -- a Pause or an export
//! starting must stop the worker from transmitting immediately, not merely stop this
//! orchestrator from starting a NEW epoch. See [`tick`]'s own doc comment for the
//! suspend/resume mechanics.
//!
//! Split into the two off-thread background generations the orchestrator keeps
//! running ([`generation`]), turning a remote accumulator's running sum into a
//! displayed, denoised image ([`compositing`]), and the orchestrator's own state plus
//! the timer tick/dispatch that drives both ([`tick`]) -- `tick` depends on
//! `generation`, which in turn depends on `compositing`.

mod compositing;
mod generation;
mod tick;

pub use tick::{RemoteOrchestratorHandle, setup_remote_rendering};
