//! Storage for the catalogue's flat tag set (`tags`/`diagram_tag_links`). See
//! [`Database::migrate_tag_tables`](super::Database::migrate_tag_tables)'s doc
//! comment for the schema and why this is a side-table pair rather than a column on
//! `diagram_entries`.

use super::Database;
use crate::model::tag::Tag;
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use std::collections::HashMap;

impl Database {
    /// Every diagram entry's tag names, alphabetical within each entry, in ONE query
    /// -- the library list's per-row chips need this for every visible row at once,
    /// and a `tags_for_entry` call per row would be an N+1
    /// query against a catalogue that already tops out in the low thousands of rows.
    /// An entry with no tags simply has no key in the returned map (never an empty
    /// `Vec` -- the caller's own `unwrap_or_default()` on the lookup already reads
    /// the same as "no tags").
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying `SELECT` fails.
    pub fn tags_by_entry(&self) -> Result<HashMap<i64, Vec<String>>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT l.entry_id, t.name
                 FROM diagram_tag_links l
                 JOIN tags t ON t.id = l.tag_id
                 ORDER BY l.entry_id ASC, t.name COLLATE NOCASE ASC",
            )
            .context("Failed to prepare tags-by-entry query")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .context("Failed to run tags-by-entry query")?;

        let mut map: HashMap<i64, Vec<String>> = HashMap::new();
        for (entry_id, name) in rows.flatten() {
            map.entry(entry_id).or_default().push(name);
        }
        Ok(map)
    }

    /// Every tag in the catalogue, alphabetical (case-insensitive) -- the chip
    /// filter row's own option list.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying `SELECT` fails.
    pub fn list_tags(&self) -> Result<Vec<Tag>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM tags ORDER BY name COLLATE NOCASE ASC")
            .context("Failed to prepare tag list query")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Tag {
                    id: row.get(0)?,
                    name: row.get(1)?,
                })
            })
            .context("Failed to run tag list query")?
            .filter_map(std::result::Result::ok)
            .collect();
        Ok(rows)
    }

    /// Every tag attached to `entry_id`, alphabetical -- the card's own tag chips.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying `SELECT` fails.
    pub fn tags_for_entry(&self, entry_id: i64) -> Result<Vec<Tag>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT t.id, t.name
                 FROM tags t
                 JOIN diagram_tag_links l ON l.tag_id = t.id
                 WHERE l.entry_id = ?1
                 ORDER BY t.name COLLATE NOCASE ASC",
            )
            .context("Failed to prepare tags-for-entry query")?;
        let rows = stmt
            .query_map(params![entry_id], |row| {
                Ok(Tag {
                    id: row.get(0)?,
                    name: row.get(1)?,
                })
            })
            .context("Failed to run tags-for-entry query")?
            .filter_map(std::result::Result::ok)
            .collect();
        Ok(rows)
    }

    /// Attaches `tag_name` to `entry_id`, creating the tag itself first if no tag
    /// with that name (case-insensitively) exists yet -- this is the ONLY way a new
    /// tag is ever created, matching the house rule that adding a tag must be
    /// reachable without a modal redesign: a cutter just types a name into an
    /// existing inline-entry control and this call does the rest.
    ///
    /// Trims `tag_name` and rejects a blank one outright (never creates an empty-name
    /// tag). Idempotent: attaching a tag the entry already carries is a no-op, not an
    /// error.
    ///
    /// # Errors
    ///
    /// Returns an error if `tag_name` is blank after trimming, or if the underlying
    /// `INSERT`/`SELECT` statements fail.
    pub fn add_tag_to_entry(&self, entry_id: i64, tag_name: &str) -> Result<Tag> {
        let name = tag_name.trim();
        if name.is_empty() {
            anyhow::bail!("Tag name must not be blank.");
        }

        self.conn
            .execute(
                "INSERT INTO tags (name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
                params![name],
            )
            .context("Failed to insert tag")?;

        let tag_id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |row| row.get(0),
            )
            .context("Failed to look up tag id after insert")?;

        self.conn
            .execute(
                "INSERT OR IGNORE INTO diagram_tag_links (entry_id, tag_id) VALUES (?1, ?2)",
                params![entry_id, tag_id],
            )
            .context("Failed to link tag to diagram entry")?;

        // Re-read the tag's OWN stored name/casing rather than echoing back
        // `name` -- if this tag already existed under different casing (e.g.
        // "Competition" already on file, this call passed "competition"), the chip
        // must show the name the catalogue has actually settled on.
        self.conn
            .query_row(
                "SELECT id, name FROM tags WHERE id = ?1",
                params![tag_id],
                |row| {
                    Ok(Tag {
                        id: row.get(0)?,
                        name: row.get(1)?,
                    })
                },
            )
            .context("Failed to re-read tag after linking")
    }

    /// Detaches `tag_id` from `entry_id`. Leaves the tag row itself in place even if
    /// this was its last attachment -- an unused tag still shows up (with nothing
    /// tagged) in [`Self::list_tags`] rather than vanishing, so a cutter who created
    /// "Competition 2027" ahead of time doesn't lose it the moment they untag the
    /// one design they'd tentatively tried it on.
    ///
    /// Not an error if the link didn't exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `DELETE` fails.
    pub fn remove_tag_from_entry(&self, entry_id: i64, tag_id: i64) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM diagram_tag_links WHERE entry_id = ?1 AND tag_id = ?2",
                params![entry_id, tag_id],
            )
            .context("Failed to unlink tag from diagram entry")?;
        Ok(())
    }

    /// The tag id matching `name` (case-insensitively), if one exists -- used to
    /// resolve the chip filter row's selection into the id [`super::search::
    /// build_search_predicate`]'s `tag_filter` parameter wants, without the caller
    /// needing to keep its own name-to-id map in sync with [`Self::list_tags`].
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT` fails.
    pub fn tag_id_by_name(&self, name: &str) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
                params![name.trim()],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to look up tag id by name")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::entry::FacetDiagramEntry;

    fn temp_db_with_one_entry(label: &str) -> (Database, i64, std::path::PathBuf) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "indicatrix-vault-tags-test-{label}-{n}-{}.sqlite",
            std::process::id()
        ));
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        let entry_id = db
            .save_diagram_entry(
                &FacetDiagramEntry {
                    title: "Tag Test".to_string(),
                    url: format!("local://tag-test-{label}-{n}.asc"),
                    design_id: String::new(),
                },
                "local-import",
            )
            .unwrap();
        (db, entry_id, path)
    }

    #[test]
    fn add_tag_to_entry_creates_the_tag_and_links_it() {
        let (db, entry_id, path) = temp_db_with_one_entry("create");
        let tag = db.add_tag_to_entry(entry_id, "Competition").unwrap();
        assert_eq!(tag.name, "Competition");

        let all = db.list_tags().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Competition");

        let on_entry = db.tags_for_entry(entry_id).unwrap();
        assert_eq!(on_entry.len(), 1);
        assert_eq!(on_entry[0].id, tag.id);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn add_tag_to_entry_is_case_insensitive_and_idempotent() {
        let (db, entry_id, path) = temp_db_with_one_entry("case");
        let first = db.add_tag_to_entry(entry_id, "Competition").unwrap();
        let second = db.add_tag_to_entry(entry_id, "competition").unwrap();
        assert_eq!(first.id, second.id, "must resolve to the same tag row");
        assert_eq!(
            second.name, "Competition",
            "the tag's original casing must survive, not the second call's"
        );
        assert_eq!(
            db.list_tags().unwrap().len(),
            1,
            "must not create a second tag row"
        );
        assert_eq!(
            db.tags_for_entry(entry_id).unwrap().len(),
            1,
            "attaching the same tag twice must not duplicate the link"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn add_tag_to_entry_rejects_a_blank_name() {
        let (db, entry_id, path) = temp_db_with_one_entry("blank");
        assert!(db.add_tag_to_entry(entry_id, "   ").is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn remove_tag_from_entry_unlinks_but_keeps_the_tag_row() {
        let (db, entry_id, path) = temp_db_with_one_entry("remove");
        let tag = db.add_tag_to_entry(entry_id, "Draft").unwrap();
        db.remove_tag_from_entry(entry_id, tag.id).unwrap();

        assert_eq!(db.tags_for_entry(entry_id).unwrap(), Vec::new());
        assert_eq!(
            db.list_tags().unwrap().len(),
            1,
            "the tag itself must survive its last removal"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn tag_id_by_name_is_case_insensitive_and_none_when_missing() {
        let (db, entry_id, path) = temp_db_with_one_entry("lookup");
        let tag = db.add_tag_to_entry(entry_id, "Heirloom").unwrap();
        assert_eq!(db.tag_id_by_name("heirloom").unwrap(), Some(tag.id));
        assert_eq!(db.tag_id_by_name("nonexistent").unwrap(), None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn deleting_a_diagram_entry_cascades_its_tag_links() {
        let (db, entry_id, path) = temp_db_with_one_entry("cascade");
        db.add_tag_to_entry(entry_id, "Ephemeral").unwrap();
        db.conn
            .execute(
                "DELETE FROM diagram_entries WHERE id = ?1",
                params![entry_id],
            )
            .unwrap();
        assert!(
            db.tags_for_entry(entry_id).unwrap().is_empty(),
            "ON DELETE CASCADE must drop the link row with its entry"
        );
        assert_eq!(
            db.list_tags().unwrap().len(),
            1,
            "the tag row itself is not entry-owned and must survive"
        );
        let _ = std::fs::remove_file(&path);
    }
}
