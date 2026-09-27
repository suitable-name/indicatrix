//! A design's printable cutting sequence as a single self-contained HTML file --
//! printable HTML, no new dependency, prints straight from the browser -- and the
//! standalone 2D-diagram export that shares its rendering pipeline.
//!
//! # Pure-function-plus-thin-callback
//!
//! Everything that decides what the sheet says is a pure function of `&Design` +
//! `&[SolvedTier]`: [`diagram::render_cut_diagram`] (mesh -> rendered/PNG-encoded
//! diagram, reusing `gui::solid_preview::diagram2d`, the same rasterizer the Edit
//! tab's "Diagram" view mode already draws) and [`html::cutting_sheet_html`] (that
//! diagram plus [`indicatrix_cut_core::Design::cutting_sheet`]'s own rows -> one
//! HTML string). Neither touches Slint, a file system, or a dialog, so both are
//! testable with a plain `Design` fixture and no window -- see this module's own
//! tests.
//!
//! [`write_cutting_sheet_html`]/[`write_diagram_png`] are the "thin callback"
//! half: they call the pure functions above, then do exactly the file-system work
//! (an `rfd` save dialog, one `std::fs::write`) that has to live somewhere. Wiring
//! either into an actual `EditorModel` callback is outside this file -- see the
//! FIX-lane report's handoff notes for the exact `gui/editor/mod.rs`,
//! `ui/models/editor.slint` and `editor_command_bar.slint` additions that wiring
//! needs.
//!
//! # Crown/Pavilion labelling
//!
//! The per-row Crown/Pavilion/Girdle label comes from the solver's own
//! `classify_blocks` (`design.meet_tier_inputs()` in, one `Block` per tier
//! out, aligned with [`indicatrix_cut_core::Design::cutting_sheet`]'s own
//! one-row-per-tier order) -- never derived from the sign of a formatted angle
//! string, a mistake this codebase already made and fixed once (see
//! `design/missing_anchor.rs`'s own `Block` usage for the established
//! precedent).
//!
//! # Model-unit vs. mm masts
//!
//! The "Mast (mm)" column only appears when [`indicatrix_cut_core::Design::girdle_diameter_mm`]
//! is set -- not when the mm conversion actually resolves. A design with a
//! stated girdle diameter that currently fails to close (or measures under the
//! trusted-width floor -- see `yield_metrics::scale::mm_per_unit`'s own doc
//! comment) still shows the column, with `"-"` cells, rather than silently
//! reverting to a model-units-only sheet the moment a design goes briefly
//! unsolved.
//!
//! # Module split
//!
//! [`diagram`] builds the rendered/PNG-encoded 2D facet diagram; [`html`] turns
//! that diagram plus a design's cutting-sheet rows into the self-contained HTML
//! string; [`write`] is the thin file-system callback half that ties the two
//! together for an `EditorModel` action to call.

mod diagram;
mod html;
#[cfg(test)]
mod tests;
mod write;

// `native_io`'s own export callbacks are the only outside callers -- everything
// else here (`DiagramImage`/`render_cut_diagram`/`cutting_sheet_html`) is reached
// only from within this module tree (`html`/`write`/`tests`), via their own
// `pub(super)`/`pub` items directly.
pub use write::{write_cutting_sheet_html, write_diagram_png};
