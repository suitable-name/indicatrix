//! The preview-then-handoff orchestrator: a repeating `slint::Timer` that polls
//! `RenderContext`'s camera/light pose, feeds `bridge::handoff::HandoffMachine`, and
//! dispatches to `bridge::remote_render` when the machine decides to hand off --
//! including the async guide-buffer and denoise generations that keep the (multi-second
//! at 4K) denoise pass off the Slint UI thread.
//!
//! `poll_tick` (in [`tick`]) is also the one place that decides "is the camera
//! currently moving" for `bridge::local_preview` -- it writes
//! `RenderContext::camera_moving` from this same `HandoffMachine` instance's state
//! every tick, so both features share one definition of "settled".
//!
//! Split into the two off-thread background generations the orchestrator keeps
//! running ([`generation`]), turning a remote accumulator's running sum into a
//! displayed, denoised image ([`compositing`]), and the orchestrator's own state plus
//! the timer tick/dispatch that drives both ([`tick`]) -- `tick` depends on
//! `generation`, which in turn depends on `compositing`.

mod compositing;
mod generation;
mod tick;

pub use tick::setup_remote_rendering;
