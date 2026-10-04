//! Computes the two version fields of the library protocol. `DesignSummary::version` is a
//! SHA-256 hash over exactly the fields a search row carries, in a fixed order, each
//! length-prefixed so adjacent fields can never collide, and each `Option` tagged
//! present/absent before its value. `DesignSummary::design_version` and
//! `DesignRecord::version` are one revision token ([`revision_token`]) derived from the
//! design's url and the vault's per-entry revision stamp, so a search row can carry it at
//! O(1) cost per row. See the `serve::library` module doc comment.

use indicatrix_net::library::DesignSummary;
use indicatrix_vault::db::sqlite::Database;
use sha2::{Digest, Sha256};

fn hash_str(hasher: &mut Sha256, s: &str) {
    hasher.update((s.len() as u64).to_le_bytes());
    hasher.update(s.as_bytes());
}

fn hash_opt_str(hasher: &mut Sha256, s: Option<&str>) {
    match s {
        Some(s) => {
            hasher.update([1u8]);
            hash_str(hasher, s);
        }
        None => hasher.update([0u8]),
    }
}

/// SHA-256 over every [`DesignSummary`] field (including `concave_tiers`) except [`DesignSummary::version`] itself,
/// [`DesignSummary::design_version`] and [`DesignSummary::entry_id`].
///
/// `entry_id` is the server database's row number, not part of the design: a mirror
/// keys designs by `url`, so renumbering the server's database (a rebuilt or
/// re-imported catalogue) must not make every design look changed and force a full
/// re-fetch. The price, paid once: the hash of every design differs from what earlier
/// builds produced, so each existing mirror re-syncs its whole catalogue one time.
pub(super) fn hash_summary(s: &DesignSummary) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hash_str(&mut hasher, &s.title);
    hash_str(&mut hasher, &s.url);
    hash_opt_str(&mut hasher, s.design_id.as_deref());
    hash_opt_str(&mut hasher, s.shape.as_deref());
    hash_opt_str(&mut hasher, s.index_gear.as_deref());
    hash_opt_str(&mut hasher, s.facets_count.as_deref());
    hash_opt_str(&mut hasher, s.designer_info.as_deref());
    hash_opt_str(&mut hasher, s.lw_ratio.as_deref());
    hash_opt_str(&mut hasher, s.refractive_index.as_deref());
    hash_opt_str(&mut hasher, s.volume.as_deref());
    hash_opt_str(&mut hasher, s.competition_diagram.as_deref());
    hasher.update([u8::from(s.ignored)]);
    // v19: appended last, so the hash of a planar design differs from v18's only by this
    // one zero word (every design's stamp still changes once -- see the protocol-19
    // README entry).
    hasher.update(s.concave_tiers.to_le_bytes());
    hasher.finalize().into()
}

/// The vault's revision stamp of one design (`diagram_entries.updated_at`), or the fact
/// that it could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Revision {
    /// The stamp as stored; `None` for a row that predates the column and was never
    /// re-saved.
    Stamped(Option<i64>),
    /// The stamp could not be read (no such row, or the query failed).
    Unreadable,
}

impl Revision {
    /// Reads `entry_id`'s stamp. Every write that replaces or edits a design bumps it
    /// strictly upward, so it moves on any change to the design's entry, detail,
    /// angle table or attachments.
    pub(super) fn read(db: &Database, entry_id: i64) -> Self {
        match db.entry_updated_at(entry_id) {
            Ok(stamp) => Self::Stamped(stamp),
            Err(e) => {
                tracing::debug!("library: no readable revision stamp for entry {entry_id}: {e:#}");
                Self::Unreadable
            }
        }
    }
}

/// The revision token of the design at `url`: SHA-256 over a domain tag, the url and the
/// stamp -- O(1) per design, so a whole search page carries it. A version token, not a
/// content hash: it changes exactly when the stamp does, which is on every edit, and says
/// nothing about what changed. All zero bytes for [`Revision::Unreadable`], which no
/// stamped design can produce, so a client comparing it against a stored token always
/// sees a change.
///
/// Neither the server's `entry_id` (renumbering a rebuilt catalogue must not look like
/// an edit of every design -- a mirror keys designs by `url`) nor any content goes in.
pub(super) fn revision_token(url: &str, revision: Revision) -> [u8; 32] {
    let Revision::Stamped(stamp) = revision else {
        return [0u8; 32];
    };
    let mut hasher = Sha256::new();
    hash_str(&mut hasher, "indicatrix/design-revision/1");
    hash_str(&mut hasher, url);
    match stamp {
        Some(t) => {
            hasher.update([1u8]);
            hasher.update(t.to_le_bytes());
        }
        None => hasher.update([0u8]),
    }
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::{Revision, hash_summary, revision_token};
    use indicatrix_net::library::DesignSummary;

    fn summary() -> DesignSummary {
        DesignSummary {
            entry_id: 1,
            title: "Scooped Round".to_string(),
            url: "https://example.test/1".to_string(),
            design_id: None,
            shape: Some("Round".to_string()),
            index_gear: Some("96".to_string()),
            facets_count: Some("57+8".to_string()),
            designer_info: None,
            lw_ratio: None,
            refractive_index: None,
            volume: None,
            competition_diagram: None,
            ignored: false,
            version: [0; 32],
            design_version: [0; 32],
            concave_tiers: 0,
        }
    }

    /// The summary hash must move when only a concave field moves, or a mirror would
    /// keep a design that gained a concave tier looking unchanged. It must also ignore
    /// `entry_id` and the two revision fields, as before.
    #[test]
    fn design_summary_version_hash_changes_when_concave_fields_change() {
        let planar = hash_summary(&summary());
        let concave = hash_summary(&DesignSummary {
            concave_tiers: 2,
            ..summary()
        });
        assert_ne!(planar, concave);
        assert_ne!(
            concave,
            hash_summary(&DesignSummary {
                concave_tiers: 3,
                ..summary()
            })
        );
        assert_eq!(
            planar,
            hash_summary(&DesignSummary {
                entry_id: 99,
                design_version: [5; 32],
                ..summary()
            })
        );
    }

    #[test]
    fn the_token_follows_the_url_and_the_stamp_only() {
        let a = revision_token("https://example.test/1", Revision::Stamped(Some(100)));
        assert_eq!(
            a,
            revision_token("https://example.test/1", Revision::Stamped(Some(100)))
        );
        assert_ne!(
            a,
            revision_token("https://example.test/1", Revision::Stamped(Some(101)))
        );
        assert_ne!(
            a,
            revision_token("https://example.test/2", Revision::Stamped(Some(100)))
        );
        assert_ne!(
            revision_token("https://example.test/1", Revision::Stamped(None)),
            revision_token("https://example.test/1", Revision::Stamped(Some(0)))
        );
        assert_ne!(a, [0u8; 32]);
    }

    #[test]
    fn an_unreadable_revision_is_the_all_zero_token() {
        assert_eq!(
            revision_token("https://example.test/1", Revision::Unreadable),
            [0u8; 32]
        );
    }
}
