//! Tilt-curve UI wiring: the performance-graph dialog's own material/geometry-change
//! callback ([`tilt_profile`]) and the design list's tilt-curve hover preview
//! ([`tilt_hover_preview`]).
//!
//! Split from flat top-level `gui` files into their own group; distinct from
//! `gui::batch::tilt` (the batch tilt-curve *computation* job) -- these two modules are
//! interactive, single-design UI wiring, not the background batch.

pub mod tilt_hover_preview;
pub mod tilt_profile;
pub mod video_export;
