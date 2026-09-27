//! [`render_accumulation`]: the shared local/remote render core -- calibrates and runs
//! the local/remote concurrent phase and merges every engine's contribution exactly
//! once into a linear accumulation buffer, without writing anything to disk.
//! [`run_export`] is the still-image export's own thin wrapper around it (tone-map,
//! then PNG/ICC write); the tilt performance video (`gui::tilt::video_export`) calls
//! [`render_accumulation`] directly, once per swept frame, reusing one `GpuBackend`
//! and one [`AccumulationCarry`] across the whole sweep instead of the still export's
//! always-fresh-per-call state -- see [`AccumulationCarry`]'s own doc comment. Moved
//! out of `mod.rs` purely to keep that file from growing further; see this crate's
//! `export_thread` module doc comment for the wider layout this fits into.
//!
//! Split into [`types`] (the `Accumulation`/`AccumulationOutcome`/`AccumulationCarry`
//! types), [`core`] (`render_accumulation` and its `resolve_remote_capability`/
//! `resolve_remote_rate` helpers), [`export`] (`run_export`'s tone-map-then-write
//! wrapper), [`final_picture`] (`render_image_rgba`: one finished RGBA8 image via either
//! transfer -- full data through `render_accumulation`, or the remote's own final
//! picture), and [`tests`].

mod core;
mod export;
mod final_picture;
#[cfg(test)]
mod tests;
mod types;

pub(super) use export::run_export;
pub use final_picture::{RenderedImage, render_image_rgba};
pub use types::AccumulationCarry;
