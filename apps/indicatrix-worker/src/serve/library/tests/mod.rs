//! Tests for `serve::library`: [`design`] covers single-record lookups
//! (`FetchDesign`/`FetchAttachment`/`FetchDesignSource`/`FilterOptions`), [`search`]
//! covers the multi-row `Search`/`SearchPage` protocol and the pure
//! `sort_order_from_wire` mapping. [`fixtures`] holds the shared populated-database
//! builder both topic files seed their assertions against.

mod design;
mod fixtures;
mod search;
