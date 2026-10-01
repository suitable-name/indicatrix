//! Re-export: the ICC profile byte builder now lives in
//! `indicatrix::render_setup::icc_profile` (moved there, tests included, so the
//! browser app embeds byte-identical wide-gamut profiles -- see that module's own doc
//! comment) and is re-exported at this path so `tonemap_png`/`render_setup_pins` need
//! no changes.

pub use indicatrix::render_setup::icc_profile::build;
