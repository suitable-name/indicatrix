//! [`Tag`]: one row of the catalogue's flat tag set (CAD audit item 190).
//!
//! Deliberately flat, not hierarchical -- a design can carry any number of tags, but a
//! tag itself has no parent/child structure (folders) and no other metadata. See
//! `crate::db::sqlite::migrations::Database::migrate_tag_tables`'s doc comment for the
//! schema and the reasoning for keeping this a side table pair rather than a column on
//! `diagram_entries`.

use serde::{Deserialize, Serialize};

/// One tag, as stored in the `tags` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tag {
    pub id: i64,
    pub name: String,
}
