//! The frosting half of the catalogue staleness scans.
//!
//! A cached preview or tilt-curve set is stored under a fingerprint that names the design's
//! frosting (`bridge::preview_render::FingerprintMaterial::frosting`), so a frosted design
//! never reuses an unfrosted picture. The vault's scans
//! (`Database::entry_ids_missing_previews` / `entry_ids_missing_tilt_curves`) hand their
//! fingerprint function only the persisted material state, not the entry, and the frosting
//! lives in a side table (`Database::diagram_frosting`). So the scan is run once for the
//! polished fingerprint and once for each distinct frosting a stored design has, and each
//! design is judged by the run that matches its own frosting.

use crate::bridge::preview_render::frosting_hash;
use indicatrix_vault::{db::sqlite::Database, model::preview::PreviewMaterialState};
use std::collections::{BTreeMap, BTreeSet};

/// The [`frosting_hash`] of `entry_id`'s stored frosting; `0` for a polished design (and for
/// a read that fails, which is the polished fingerprint, the one every earlier row has).
#[must_use]
pub fn stored_frosting_hash(db: &Database, entry_id: i64) -> u64 {
    frosting_hash(&db.diagram_frosting(entry_id).unwrap_or_default())
}

/// The entry ids whose stored picture is outdated, with frosting taken into account.
///
/// `query` runs one of the vault's scans with the fingerprint function it is given;
/// `fingerprint` builds a row's current fingerprint from its persisted material state and the
/// [`frosting_hash`] of its design (`0` for none). A design without stored frosting is
/// outdated when the polished run lists it; a frosted design when the run for its own
/// frosting does. A scan that fails lists nothing, as the scans did before.
pub fn outdated_ids(
    db: &Database,
    fingerprint: impl Fn(&PreviewMaterialState, u64) -> String,
    query: impl Fn(&Database, &dyn Fn(&PreviewMaterialState) -> String) -> anyhow::Result<Vec<i64>>,
) -> Vec<i64> {
    let polished_run = query(db, &|state| fingerprint(state, 0)).unwrap_or_default();

    let mut frosted_by_hash: BTreeMap<u64, BTreeSet<i64>> = BTreeMap::new();
    // One query for every frosted design (an entry the scans skip is simply never listed by
    // a run, so it needs no filtering here). A failed read lists nothing frosted.
    for (entry_id, rows) in db.all_diagram_frosting().unwrap_or_default() {
        let hash = frosting_hash(&rows);
        if hash != 0 {
            frosted_by_hash.entry(hash).or_default().insert(entry_id);
        }
    }
    if frosted_by_hash.is_empty() {
        return polished_run;
    }

    let mut outdated: BTreeSet<i64> = polished_run
        .into_iter()
        .filter(|id| !frosted_by_hash.values().any(|ids| ids.contains(id)))
        .collect();
    for (hash, ids) in &frosted_by_hash {
        let run = query(db, &|state| fingerprint(state, *hash)).unwrap_or_default();
        outdated.extend(run.into_iter().filter(|id| ids.contains(id)));
    }
    outdated.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::preview_render::{CacheKind, FingerprintMaterial, cache_fingerprint};
    use indicatrix_vault::model::{entry::FacetingDiagramEntry, frosting::StoredTierFrosting};

    const PREVIEW: CacheKind = CacheKind::Preview {
        size: 160,
        spp: 4,
        max_bounces: 12,
    };

    fn entry(db: &Database, name: &str) -> i64 {
        db.save_diagram_entry(
            &FacetingDiagramEntry {
                title: name.to_string(),
                url: format!("local://{name}.asc"),
                design_id: String::new(),
            },
            "local-import",
        )
        .expect("the entry saves")
    }

    /// Stores a preview pair for `entry_id` under the fingerprint of a design with `frosting`.
    fn store_preview(db: &Database, entry_id: i64, frosting: u64) {
        let state = PreviewMaterialState::default();
        let fingerprint = cache_fingerprint(
            PREVIEW,
            FingerprintMaterial::from_state(&state).with_frosting(frosting),
        );
        let updated_at = db.entry_updated_at(entry_id).expect("the stamp reads");
        assert!(
            db.save_preview_images(
                entry_id,
                Some(b"front".as_slice()),
                Some(b"top".as_slice()),
                1,
                &fingerprint,
                updated_at,
            )
            .expect("the images save")
        );
    }

    fn scan(db: &Database) -> Vec<i64> {
        outdated_ids(
            db,
            |state, frosting| {
                cache_fingerprint(
                    PREVIEW,
                    FingerprintMaterial::from_state(state).with_frosting(frosting),
                )
            },
            |db, fingerprint| db.entry_ids_missing_previews(fingerprint),
        )
    }

    /// A frosted design does not reuse an unfrosted cached preview, one stored under its own
    /// frosting is current, a different frosting is outdated again, and polished designs are
    /// judged exactly as before.
    #[test]
    fn a_frosted_design_is_outdated_until_its_preview_names_its_frosting() {
        let db = Database::new(Some(":memory:")).expect("in-memory database opens");
        let polished = entry(&db, "polished");
        let frosted = entry(&db, "frosted");
        store_preview(&db, polished, 0);
        store_preview(&db, frosted, 0);
        assert_eq!(scan(&db), Vec::<i64>::new());

        let rows = [StoredTierFrosting::whole(1)];
        db.set_diagram_frosting(frosted, &rows)
            .expect("frosting stores");
        let hash = stored_frosting_hash(&db, frosted);
        assert_ne!(hash, 0);
        assert_eq!(
            scan(&db),
            vec![frosted],
            "the unfrosted picture of a frosted design is outdated"
        );

        store_preview(&db, frosted, hash);
        assert_eq!(
            scan(&db),
            Vec::<i64>::new(),
            "stored under its own frosting"
        );

        db.set_diagram_frosting(frosted, &[StoredTierFrosting::whole(2)])
            .expect("frosting stores");
        assert_eq!(
            scan(&db),
            vec![frosted],
            "another frosting is outdated again"
        );

        db.set_diagram_frosting(frosted, &[])
            .expect("frosting clears");
        assert_eq!(
            scan(&db),
            vec![frosted],
            "a picture stored frosted is outdated once the frosting is gone"
        );
        store_preview(&db, frosted, 0);
        assert_eq!(scan(&db), Vec::<i64>::new());
    }
}
