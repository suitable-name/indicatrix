//! Tests for `serve::library`: [`design`] covers single-record lookups
//! (`FetchDesign`/`FetchAttachment`/`FetchDesignSource`/`FilterOptions`), [`search`]
//! covers the multi-row `Search`/`SearchPage` protocol and the pure
//! `sort_order_from_wire` mapping, [`handle`] covers `LibraryHandle`'s lazy open.
//! [`fixtures`] holds the shared populated-database builder the topic files seed their
//! assertions against.

mod design;
mod fixtures;
mod handle;
mod search;
