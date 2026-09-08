//! The two catalogue-wide background batches -- generating cached preview thumbnails
//! ([`preview`]) and computing tilt-performance curves ([`tilt`]) -- plus what they
//! share: a local/remote work-distribution queue ([`batch_queue`]) and the decoded
//! preview-thumbnail cache the design list reads from ([`preview_cache`]).
//!
//! Was 4 flat top-level `gui` files (`preview_batch.rs`, `tilt_batch.rs`,
//! `batch_queue.rs`, `preview_cache.rs`); grouped here since the two batches are
//! deliberately parallel in shape (see [`preview`]'s and [`tilt`]'s own module doc
//! comments for exactly what they share vs. why they stayed separate modules) and both
//! were already, individually, well over this codebase's ~700-line split threshold.

pub mod batch_queue;
pub mod preview;
pub mod preview_cache;
pub mod tilt;
