//! The RENDER worker's own request-resolution state machine: its remembered
//! `WorkerMemory` (including the Diagram-mode `DiagramMemory` half), and
//! `resolve_request_state`, which turns one incoming `RedrawRequest` into the
//! plane/camera/style tuple `super::render::render_request` actually draws.
//!
//! All of it moved to `indicatrix_solid::preview` (shared with the web app, which
//! drives the same state machine on its main thread); re-exported here at the
//! old paths this module's worker and tests use.

pub use indicatrix_solid::preview::WorkerMemory;
#[cfg(test)]
pub use indicatrix_solid::preview::{escaping_tier_label, resolve_request_state};
