//! `indicatrix-cut`: browse, search, and render your own faceting-design library --
//! import/export `.asc`, organize by rename/delete, and the full spectral renderer.
//!
//! Structured as a library (this file) plus a thin `main.rs` binary, so that a second
//! binary can reuse this crate's already-compiled Slint window ([`MainWindow`]) and
//! Rust wiring ([`gui::build_main_window`]) rather than duplicating either -- see
//! [`gui::MainWindowHandle`]'s doc comment for how such a binary builds on top of it.
//! That is the reason for the library/binary split, and the reason the items below
//! have the visibility they do.

// The trait solver overflows proving `Send` for the GPU export-thread closure in
// `bridge::export_thread::batch` -- it has to walk all the way through `wgpu`'s deeply
// nested handle types (`Buffer` -> `DispatchBuffer` -> `CoreBuffer` -> ... -> `Global` ->
// `Hub` -> `Registry<...>`) to conclude each one is `Send`. Not a lint: this is a
// compiler resource limit on trait-solving depth, and raising it doesn't silence
// anything -- it lets the solver finish a proof it was already correctly attempting.
// Default (128) isn't enough; 256 is.
#![recursion_limit = "256"]

slint::include_modules!();

// Not `pub`: neither is part of the library-facing surface described above --
// `gui::build_main_window` wires both up internally and hands back a `MainWindowHandle`
// whose only public field is the finished `MainWindow`. Keeping them crate-private
// also means their many pre-existing `pub fn`s (written back when this crate had no
// `[lib]` target at all, so nothing outside `run_gui` could ever see them) don't
// suddenly become part of a real public API needing `# Errors`/`# Panics` docs -- see
// `gui::MainWindowHandle`'s doc comment for the one seam that IS public.
mod bridge;
pub mod gui;
mod settings;
