//! The design-library UI: browsing/searching/filtering ([`diagram_list`], [`search`]),
//! opening/saving a design's own metadata ([`detail`]), copying detail fields to the
//! clipboard ([`clipboard`]), local-only CRUD -- import/organize/export -- ([`local`]),
//! and switching between the local database and a remote worker's library plus
//! driving a pull-mirror sync ([`remote`]).
//!
//! Grouped here as one coherent "design library" domain, with the local-only
//! import/organize/export logic further split into [`local`]'s own submodules.

pub mod clipboard;
pub mod detail;
pub mod diagram_list;
pub mod local;
pub mod remote;
pub mod search;
