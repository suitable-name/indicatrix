//! `indicatrix-vault`: plain data models, a SQLite-backed store, and local import/export
//! for the user's own faceting-design library.
//!
//! The storage layer and nothing more: no network client, no HTML/SVG parsing, no OCR.
//! The dependency direction is strictly one way -- anything that acquires designs
//! depends on this crate's [`model`] and [`db`], never the reverse.
//!
//! - [`db`]: SQLite storage and schema migrations. The schema is deliberately wider
//!   than this crate itself fills -- columns like `page_url`/`pdf_file` are just
//!   columns, and an existing `facet_diagrams.sqlite`, however it was populated, keeps
//!   opening and keeps every value it already holds.
//! - [`model`]: `FacetDiagramDetail`, `AngleSetting`, `AttachedFile`, search/range
//!   filters, and cross-source dedup types.
//! - [`local`]: import/export for the user's own `.asc` files (via `indicatrix_formats::asc`),
//!   independent of any online source.

pub mod db;
pub mod local;
pub mod model;
