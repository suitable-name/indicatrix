//! Rough colour and colour zoning from rig photos (plan 2026-10-09). Only built with the `zoning`
//! feature.
//!
//! - [`forward`]: the forward path tracer of the rig (lane F1). It traces every working pixel's
//!   paths once, stores them as `(zone lengths, weight)` records, and evaluates any candidate
//!   absorption on them without tracing again.
//! - [`ColourRig`]: the rig profile together with its light model. It lives here and not inside
//!   [`RigProfile`](crate::rough_plan::locate::RigProfile), so the saved rig bytes stay unchanged.
//!
//! Later lanes add their own modules (`solve`, `zones`) next to `forward`.

pub mod forward;
mod rig;
pub mod solve;
pub mod zones;

pub use rig::ColourRig;
