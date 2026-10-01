//! Re-export: the exponent<->sample-count mapping for the settings dialog's "Target
//! Samples" slider now lives in `indicatrix::render_setup::sample_scale` (moved there
//! unchanged, tests included, so the browser app's own samples slider maps exactly the
//! same way -- see that module's own doc comment) and is re-exported at this path so
//! every call site in this crate (`gui::mod`, `gui::startup_settings`, the remote
//! render sample budget) needs no changes.

pub use indicatrix::render_setup::sample_scale::*;
