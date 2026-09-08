//! The design-library UI: browsing/searching/filtering ([`diagram_list`], [`search`]),
//! opening/saving a design's own metadata ([`detail`]), copying detail fields to the
//! clipboard ([`clipboard`]), local-only CRUD -- import/organize/export -- ([`local`]),
//! and switching between the local database and a remote worker's library plus
//! driving a pull-mirror sync ([`remote`]).
//!
//! Was 6 flat top-level `gui` files (`library.rs`, `library_remote.rs`,
//! `diagram_list.rs`, `search.rs`, `detail.rs`, `clipboard.rs`); grouped here as the
//! one coherent "design library" domain, with `library.rs` itself (the largest, at
//! local-only import/organize/export) further split into [`local`]'s own submodules.

pub mod clipboard;
pub mod detail;
pub mod diagram_list;
pub mod local;
pub mod remote;
pub mod search;
