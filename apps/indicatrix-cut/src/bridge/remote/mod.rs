//! Everything specific to talking to a remote `indicatrix-worker`: claiming a one-time
//! enrollment token ([`enroll`]), owning the mutual-TLS socket and driving one render
//! request against it ([`remote_render`]), and the preview-then-handoff state machine
//! that decides when to switch a live viewport from local to remote rendering
//! ([`handoff`]). Distinct from [`super::render_thread`] (the local render loop) and
//! [`super::export_thread`]'s own export-specific remote dispatch.

pub mod enroll;
pub mod handoff;
pub mod remote_render;
